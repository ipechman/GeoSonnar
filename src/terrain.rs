//! Bounded native sampling of geographic WGS84 GeoTIFF terrain and geoid grids.
//!
//! Heights are scalar source values in metres. The caller declares their vertical
//! meaning: an orthometric DEM and a compatible geoid produce ellipsoid height
//! as H + N + microphone height. No missing raster value becomes zero.

use crate::{detailed_data::sha256 as sha256_file, verification::Observer};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};
use tiff::{
    ColorType,
    decoder::{Decoder, DecodingResult, Limits},
    tags::Tag,
};

// The public Copernicus GLO-90 COGs use padded 2048 x 2048 TIFF tiles.
// Keep decoding bounded to one such tile (32 MiB of f64 values).
const MAX_CHUNK_PIXELS: u64 = 4_194_304;
const MAX_CACHE_CHUNKS: usize = 4;
const MAX_DIMENSION: u32 = 1_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RasterMetadata {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub pixel_type: String,
    /// [longitude, latitude] at the centre of the top-left sample.
    pub sample_zero_lon_lat: [f64; 2],
    /// Positive longitude and southward-latitude sample spacing in degrees.
    pub step_deg: [f64; 2],
    pub chunk_dimensions: [u32; 2],
    pub nodata: Option<f64>,
    pub nodata_is_nan: bool,
    pub interpolation: String,
    pub maximum_cached_chunks: usize,
    pub maximum_decoded_chunk_pixels: u64,
}

struct Chunk {
    index: u32,
    width: u32,
    values: Vec<f64>,
}

pub struct GeographicRaster {
    decoder: Decoder<BufReader<File>>,
    metadata: RasterMetadata,
    cache: VecDeque<Chunk>,
    decoded_chunks: u64,
}

fn tiff_error(error: impl std::fmt::Display) -> String {
    format!("Cannot read terrain TIFF: {error}")
}

fn inline_key(keys: &[u16], requested: u16) -> Result<u16, String> {
    let matches: Vec<_> = keys[4..]
        .chunks_exact(4)
        .filter(|row| row[0] == requested)
        .collect();
    if matches.len() != 1 || matches[0][1] != 0 || matches[0][2] != 1 {
        return Err(format!(
            "Missing, duplicate or non-inline GeoTIFF key {requested}"
        ));
    }
    Ok(matches[0][3])
}

impl GeographicRaster {
    pub fn open(path: &Path) -> Result<Self, String> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("Cannot open terrain {}: {e}", path.display()))?;
        let bytes = std::fs::metadata(&path).map_err(tiff_error)?.len();
        let sha256 = sha256_file(&path)?;
        let file = File::open(&path).map_err(tiff_error)?;
        let mut limits = Limits::default();
        limits.decoding_buffer_size = (MAX_CHUNK_PIXELS * 8) as usize;
        limits.intermediate_buffer_size = 16 * 1024 * 1024;
        limits.ifd_value_size = 1024 * 1024;
        let mut decoder = Decoder::new(BufReader::new(file))
            .map_err(tiff_error)?
            .with_limits(limits);
        let (width, height) = decoder.dimensions().map_err(tiff_error)?;
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err("Terrain raster dimensions exceed the supported domain".into());
        }
        if !matches!(
            decoder.colortype().map_err(tiff_error)?,
            ColorType::Gray(8 | 16 | 32 | 64)
        ) {
            return Err("Terrain must be a single-channel scalar grayscale raster".into());
        }
        if decoder
            .find_tag_unsigned::<u16>(Tag::Orientation)
            .map_err(tiff_error)?
            .unwrap_or(1)
            != 1
            || decoder
                .find_tag_unsigned::<u16>(Tag::SamplesPerPixel)
                .map_err(tiff_error)?
                .unwrap_or(1)
                != 1
            || decoder
                .find_tag_unsigned::<u16>(Tag::PhotometricInterpretation)
                .map_err(tiff_error)?
                != Some(1)
        {
            return Err(
                "Terrain requires one unchanged, top-left, BlackIsZero scalar channel".into(),
            );
        }
        if decoder
            .find_tag(Tag::ModelTransformationTag)
            .map_err(tiff_error)?
            .is_some()
        {
            return Err(
                "Rotated/transformed terrain is unsupported; provide a north-up WGS84 grid".into(),
            );
        }
        let keys = decoder
            .get_tag_u16_vec(Tag::GeoKeyDirectoryTag)
            .map_err(tiff_error)?;
        if keys.len() < 4
            || keys[0] != 1
            || keys[1] != 1
            || keys[2] > 1
            || keys.len() != 4 + usize::from(keys[3]) * 4
        {
            return Err("Malformed or unsupported GeoTIFF key directory".into());
        }
        if inline_key(&keys, 1024)? != 2 || inline_key(&keys, 2048)? != 4326 {
            return Err("Terrain requires geographic WGS84 (EPSG:4326) coordinates".into());
        }
        // EPSG:4326 implies degrees; an explicit contradictory angular unit is invalid.
        if keys[4..].chunks_exact(4).any(|row| row[0] == 2054) && inline_key(&keys, 2054)? != 9102 {
            return Err("Terrain geographic angular units must be degrees".into());
        }
        let raster_type = inline_key(&keys, 1025)?;
        let centre_offset = match raster_type {
            1 => 0.5,
            2 => 0.0,
            _ => {
                return Err(
                    "Terrain must explicitly declare pixel-is-area or pixel-is-point".into(),
                );
            }
        };
        let scale = decoder
            .get_tag_f64_vec(Tag::ModelPixelScaleTag)
            .map_err(tiff_error)?;
        let tie = decoder
            .get_tag_f64_vec(Tag::ModelTiepointTag)
            .map_err(tiff_error)?;
        if scale.len() != 3
            || tie.len() != 6
            || !scale.iter().chain(&tie).all(|x| x.is_finite())
            || scale[0] <= 0.0
            || scale[1] <= 0.0
        {
            return Err(
                "Terrain requires one finite tiepoint and positive north-up pixel scales".into(),
            );
        }
        let lon0 = tie[3] - tie[0] * scale[0] + centre_offset * scale[0];
        let lat0 = tie[4] + tie[1] * scale[1] - centre_offset * scale[1];
        let east = lon0 + f64::from(width - 1) * scale[0];
        let south = lat0 - f64::from(height - 1) * scale[1];
        if ![lon0, lat0, east, south].iter().all(|x| x.is_finite())
            || lon0 < -180.0 - 1e-7
            || east > 180.0 + 1e-7
            || south < -90.0 - 1e-7
            || lat0 > 90.0 + 1e-7
        {
            return Err("Terrain sample support is outside valid WGS84 coordinates".into());
        }
        let raw_nodata = decoder.find_tag(Tag::GdalNodata).map_err(tiff_error)?;
        let nodata = if raw_nodata.is_some() {
            let text = decoder
                .get_tag_ascii_string(Tag::GdalNodata)
                .map_err(tiff_error)?;
            let value = text
                .trim_end_matches('\0')
                .trim()
                .parse::<f64>()
                .map_err(|_| "Terrain no-data tag is not a numeric value".to_string())?;
            if value.is_infinite() {
                return Err("Infinite terrain no-data declaration is unsupported".into());
            }
            Some(value)
        } else {
            None
        };
        let (chunk_width, chunk_height) = decoder.chunk_dimensions();
        if chunk_width == 0
            || chunk_height == 0
            || u64::from(chunk_width) * u64::from(chunk_height) > MAX_CHUNK_PIXELS
        {
            return Err(
                "Terrain TIFF chunk exceeds the bounded decoding budget; retile the raster".into(),
            );
        }
        let metadata = RasterMetadata {
            path,
            bytes,
            sha256,
            width,
            height,
            pixel_type: if raster_type == 1 { "area" } else { "point" }.into(),
            sample_zero_lon_lat: [lon0, lat0],
            step_deg: [scale[0], scale[1]],
            chunk_dimensions: [chunk_width, chunk_height],
            nodata: nodata.filter(|x| x.is_finite()),
            nodata_is_nan: nodata.is_some_and(f64::is_nan),
            interpolation: "bilinear at sample centres; reject outside support or no-data".into(),
            maximum_cached_chunks: MAX_CACHE_CHUNKS,
            maximum_decoded_chunk_pixels: MAX_CHUNK_PIXELS,
        };
        Ok(Self {
            decoder,
            metadata,
            cache: VecDeque::new(),
            decoded_chunks: 0,
        })
    }

    pub fn metadata(&self) -> &RasterMetadata {
        &self.metadata
    }

    pub fn cached_chunk_count(&self) -> usize {
        self.cache.len()
    }

    pub fn decoded_chunk_count(&self) -> u64 {
        self.decoded_chunks
    }

    /// Call after a batch of sampling before committing outputs.
    pub fn verify_unchanged(&self) -> Result<(), String> {
        if std::fs::metadata(&self.metadata.path)
            .map_err(tiff_error)?
            .len()
            != self.metadata.bytes
            || sha256_file(&self.metadata.path)? != self.metadata.sha256
        {
            return Err("Terrain file changed while sampling".into());
        }
        Ok(())
    }

    fn pixel(&mut self, row: u32, col: u32) -> Result<f64, String> {
        let [cw, ch] = self.metadata.chunk_dimensions;
        let across = self.metadata.width.div_ceil(cw);
        let index = (row / ch)
            .checked_mul(across)
            .and_then(|r| r.checked_add(col / cw))
            .ok_or("Terrain tile index overflow")?;
        let slot = self.cache.iter().position(|entry| entry.index == index);
        let chunk = if let Some(slot) = slot {
            self.cache
                .remove(slot)
                .ok_or("Terrain cache lookup failed")?
        } else {
            let (width, height) = self.decoder.chunk_data_dimensions(index);
            let values = match self.decoder.read_chunk(index).map_err(tiff_error)? {
                DecodingResult::F32(v) => v.into_iter().map(f64::from).collect(),
                DecodingResult::F64(v) => v,
                DecodingResult::I8(v) => v.into_iter().map(f64::from).collect(),
                DecodingResult::I16(v) => v.into_iter().map(f64::from).collect(),
                DecodingResult::I32(v) => v.into_iter().map(f64::from).collect(),
                DecodingResult::U8(v) => v.into_iter().map(f64::from).collect(),
                DecodingResult::U16(v) => v.into_iter().map(f64::from).collect(),
                DecodingResult::U32(v) => v.into_iter().map(f64::from).collect(),
                _ => {
                    return Err(
                        "Unsupported terrain scalar type; use float32/64 or <=32-bit integers"
                            .into(),
                    );
                }
            };
            if values.len() as u64 != u64::from(width) * u64::from(height)
                || values.len() as u64 > MAX_CHUNK_PIXELS
            {
                return Err("Terrain chunk has inconsistent decoded dimensions".into());
            }
            self.decoded_chunks += 1;
            Chunk {
                index,
                width,
                values,
            }
        };
        let offset = (row % ch) as usize * chunk.width as usize + (col % cw) as usize;
        let value = *chunk
            .values
            .get(offset)
            .ok_or("Terrain sample lies outside decoded chunk")?;
        if self.cache.len() == MAX_CACHE_CHUNKS {
            self.cache.pop_front();
        }
        self.cache.push_back(chunk);
        if !value.is_finite() || self.metadata.nodata == Some(value) {
            return Err(format!("Missing terrain data at row {row}, column {col}"));
        }
        Ok(value)
    }

    pub fn sample(&mut self, latitude_deg: f64, longitude_deg: f64) -> Result<f64, String> {
        if !latitude_deg.is_finite()
            || !longitude_deg.is_finite()
            || !(-90.0..=90.0).contains(&latitude_deg)
            || !(-180.0..=180.0).contains(&longitude_deg)
        {
            return Err("Terrain query requires finite WGS84 latitude/longitude".into());
        }
        let [lon0, lat0] = self.metadata.sample_zero_lon_lat;
        let [dx, dy] = self.metadata.step_deg;
        let mut row = (lat0 - latitude_deg) / dy;
        let mut col = (longitude_deg - lon0) / dx;
        let last_row = f64::from(self.metadata.height - 1);
        let last_col = f64::from(self.metadata.width - 1);
        if !(-1e-7..=last_row + 1e-7).contains(&row) || !(-1e-7..=last_col + 1e-7).contains(&col) {
            return Err(format!(
                "Coordinate ({latitude_deg}, {longitude_deg}) is outside terrain sample support"
            ));
        }
        row = row.clamp(0.0, last_row);
        col = col.clamp(0.0, last_col);
        let (y, x) = (row.floor() as u32, col.floor() as u32);
        let (y1, x1) = (
            (y + 1).min(self.metadata.height - 1),
            (x + 1).min(self.metadata.width - 1),
        );
        let values = [
            self.pixel(y, x)?,
            self.pixel(y, x1)?,
            self.pixel(y1, x)?,
            self.pixel(y1, x1)?,
        ];
        let (fy, fx) = (row - f64::from(y), col - f64::from(x));
        let weights = [
            (1.0 - fx) * (1.0 - fy),
            fx * (1.0 - fy),
            (1.0 - fx) * fy,
            fx * fy,
        ];
        let result: f64 = values.into_iter().zip(weights).map(|(v, w)| v * w).sum();
        if !result.is_finite() {
            return Err("Terrain interpolation overflowed".into());
        }
        Ok(result)
    }
}

#[derive(Debug, Serialize)]
pub struct ObserverHeightSample {
    pub id: String,
    pub dem_orthometric_height_m: f64,
    pub geoid_undulation_m: f64,
    pub microphone_height_m: f64,
}

#[derive(Debug, Serialize)]
pub struct TerrainGrid {
    pub schema_version: u32,
    pub observers: Vec<Observer>,
    pub samples: Vec<ObserverHeightSample>,
    pub bounds_south_west_north_east: [f64; 4],
    pub maximum_grid_spacing_m: f64,
    pub microphone_height_m: f64,
    pub height_formula: String,
    pub dem: RasterMetadata,
    pub geoid: RasterMetadata,
    pub location_verified: bool,
    pub limitations: Vec<String>,
}

pub fn observer_grid(
    dem: &mut GeographicRaster,
    geoid: &mut GeographicRaster,
    bounds: [f64; 4],
    spacing_m: f64,
    microphone_height_m: f64,
) -> Result<TerrainGrid, String> {
    let [south, west, north, east] = bounds;
    if !bounds.iter().all(|x| x.is_finite())
        || south >= north
        || west >= east
        || south < -85.0
        || north > 85.0
        || west < -180.0
        || east > 180.0
        || north - south > 5.0
        || east - west > 5.0
        || !spacing_m.is_finite()
        || spacing_m <= 0.0
        || !microphone_height_m.is_finite()
        || microphone_height_m < 0.0
    {
        return Err("Use a finite local box <=5 degrees per side within +/-85 latitude, positive spacing and nonnegative microphone height".into());
    }
    let lat = ((south + north) / 2.0).to_radians();
    let e2 = 6.694_379_990_141_316_5e-3;
    let denominator = 1.0 - e2 * lat.sin().powi(2);
    let lat_m = 6_378_137.0 * (1.0 - e2) / denominator.powf(1.5) * std::f64::consts::PI / 180.0;
    let lon_m = 6_378_137.0 / denominator.sqrt() * lat.cos() * std::f64::consts::PI / 180.0;
    let rows = ((north - south) * lat_m / spacing_m).ceil();
    let cols = ((east - west) * lon_m / spacing_m).ceil();
    if !rows.is_finite() || !cols.is_finite() || (rows + 1.0) * (cols + 1.0) > 100_000.0 {
        return Err("Terrain grid exceeds the 100,000-observer budget".into());
    }
    let (rows, cols) = (rows as usize, cols as usize);
    let mut observers = Vec::new();
    let mut samples = Vec::new();
    for row in 0..=rows {
        let latitude = if row == rows {
            north
        } else {
            south + (north - south) / rows as f64 * row as f64
        };
        for col in 0..=cols {
            let longitude = if col == cols {
                east
            } else {
                west + (east - west) / cols as f64 * col as f64
            };
            let ground = dem.sample(latitude, longitude)?;
            let undulation = geoid.sample(latitude, longitude)?;
            let height = ground + undulation + microphone_height_m;
            if !height.is_finite() {
                return Err("Observer height overflowed".into());
            }
            let id = format!("r{row}-c{col}");
            observers.push(Observer {
                id: id.clone(),
                latitude_deg: latitude,
                longitude_deg: longitude,
                ellipsoid_height_m: height,
            });
            samples.push(ObserverHeightSample {
                id,
                dem_orthometric_height_m: ground,
                geoid_undulation_m: undulation,
                microphone_height_m,
            });
        }
    }
    dem.verify_unchanged()?;
    geoid.verify_unchanged()?;
    Ok(TerrainGrid { schema_version: 1, observers, samples,
        bounds_south_west_north_east: bounds, maximum_grid_spacing_m: spacing_m, microphone_height_m,
        height_formula: "ellipsoid = declared DEM orthometric height + compatible geoid undulation + microphone height".into(),
        dem: dem.metadata.clone(), geoid: geoid.metadata.clone(), location_verified: false,
        limitations: vec!["The caller must supply compatible DEM and geoid vertical datums and metre-valued samples.".into(),
            "A DSM includes surface objects and is not surveyed bare earth; microphone height remains an assumption.".into(),
            "Outside-support and missing samples fail; multi-tile mosaics and date-line wrapping require explicit separate handling.".into()] })
}
