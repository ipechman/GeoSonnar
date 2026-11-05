//! Explicit bounded downloads for terrain and authorized lightning exports.
//! Provider pages: https://copernicus-dem-90m.s3.amazonaws.com/readme.html and
//! https://www.blitzortung.org/en/cover_your_area.php .
use super::*;
use crate::{evidence_score, terrain::GeographicRaster, workspace};

pub const TERRAIN_PROVIDER_URL: &str = "https://copernicus-dem-90m.s3.amazonaws.com/readme.html";
pub const MAX_TERRAIN_TILES: usize = 64;
pub const MAX_LIGHTNING_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerrainResolution {
    Glo90,
    Glo30,
}
impl TerrainResolution {
    pub fn label(self) -> &'static str {
        match self {
            Self::Glo90 => "GLO-90 · about 90 m",
            Self::Glo30 => "GLO-30 · about 30 m",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerrainTile {
    pub name: String,
    pub url: String,
    pub bounds: [f64; 4],
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerrainPlan {
    pub resolution: TerrainResolution,
    pub bounds: [f64; 4],
    pub tiles: Vec<TerrainTile>,
    pub max_bytes: u64,
}

/// Tile planning is local: it does not claim that every tile exists. Ocean and
/// restricted areas can be missing; a failed tile remains an explicit gap.
pub fn plan_terrain(
    bounds: [f64; 4],
    resolution: TerrainResolution,
    max_bytes: u64,
) -> Result<TerrainPlan, String> {
    validate_bounds(Some(bounds))?;
    if max_bytes == 0 || max_bytes > 100_000_000_000 {
        return Err("Choose a total terrain download allowance between 0.0000000009 GiB and 93.1322574615 GiB".into());
    }
    let [south, west, north, east] = bounds;
    let spans = if west < east {
        vec![(west, east)]
    } else {
        vec![(west, 180.0), (-180.0, east)]
    };
    let rows = north.ceil() as i32 - south.floor() as i32;
    let columns: i32 = spans
        .iter()
        .map(|(w, e)| e.ceil() as i32 - w.floor() as i32)
        .sum();
    if rows <= 0 || columns <= 0 || i64::from(rows) * i64::from(columns) > MAX_TERRAIN_TILES as i64
    {
        return Err(format!(
            "Terrain requests are limited to {MAX_TERRAIN_TILES} one-degree tiles. Narrow the region first."
        ));
    }
    let (bucket, sampling) = match resolution {
        TerrainResolution::Glo90 => ("90m", "30"),
        TerrainResolution::Glo30 => ("30m", "10"),
    };
    let mut tiles = Vec::new();
    for latitude in south.floor() as i32..north.ceil() as i32 {
        for (w, e) in &spans {
            for longitude in w.floor() as i32..e.ceil() as i32 {
                let name = format!(
                    "Copernicus_DSM_COG_{sampling}_{}{:02}_00_{}{:03}_00_DEM",
                    if latitude < 0 { "S" } else { "N" },
                    latitude.abs(),
                    if longitude < 0 { "W" } else { "E" },
                    longitude.abs()
                );
                tiles.push(TerrainTile {
                    url: format!(
                        "https://copernicus-dem-{bucket}.s3.amazonaws.com/{name}/{name}.tif"
                    ),
                    name,
                    bounds: [
                        f64::from(latitude),
                        f64::from(longitude),
                        f64::from(latitude + 1),
                        f64::from(longitude + 1),
                    ],
                });
            }
        }
    }
    Ok(TerrainPlan {
        resolution,
        bounds,
        tiles,
        max_bytes,
    })
}

pub fn download_terrain_with_cancel(
    manifest: &Path,
    plan: &TerrainPlan,
    cancel: &AtomicBool,
) -> Result<Vec<CatalogEntry>, String> {
    // Never trust editable serialized plans to supply arbitrary download URLs.
    if &plan_terrain(plan.bounds, plan.resolution, plan.max_bytes)? != plan {
        return Err("Terrain plan changed; prepare it again before downloading".into());
    }
    let existing = list(manifest)?;
    let mut remaining = plan.max_bytes;
    let mut entries = Vec::new();
    for tile in &plan.tiles {
        if cancel.load(Ordering::Relaxed) {
            return Err(format!(
                "Terrain download canceled after {} tiles; completed managed tiles remain available",
                entries.len()
            ));
        }
        // Reuse a byte-verified prior request for this exact public tile.
        if let Some(item) = existing.iter().find(|item| {
            item.status == "available"
                && item.entry.kind == DataKind::Terrain
                && item.entry.source["url_without_query"] == tile.url
                && verified_files(manifest, &item.entry.id).is_ok()
        }) {
            let files = verified_files(manifest, &item.entry.id)?;
            validate_terrain_tile(files.first().ok_or("Cached tile has no file")?, tile, plan.resolution)
                .map_err(|error| format!("Cached terrain {} is unsuitable: {error}. Delete its managed copy before retrying.", tile.name))?;
            entries.push(item.entry.clone());
            continue;
        }
        if remaining == 0 {
            return Err("Terrain request exhausted its total transfer allowance; completed tiles remain in the library".into());
        }
        let request = ImportRequest {
            label: tile.name.clone(),
            kind: DataKind::Terrain,
            date_utc: None,
            bounds: Some(tile.bounds),
            max_bytes: remaining,
            notes: format!(
                "Copernicus DEM {}. Static surface model including buildings/vegetation; heights reference EGM2008, not WGS84 ellipsoid heights. © DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018. Copernicus DEM license and provider: {TERRAIN_PROVIDER_URL}. Requested region {:?}. Loading shows coverage; a terrain/geoid model is still required to apply receiver heights.",
                plan.resolution.label(),
                plan.bounds
            ),
        };
        let entry = download_url_with_cancel(manifest, &tile.url, &request, cancel)
            .map_err(|error| format!("Terrain tile {} failed after {} completed tiles: {error}. Missing tiles are not zero-height terrain.", tile.name, entries.len()))?;
        let paths = verified_files(manifest, &entry.id)?;
        validate_terrain_tile(
            paths.first().ok_or("Downloaded tile has no file")?,
            tile,
            plan.resolution,
        )
        .map_err(|e| {
            format!(
                "Downloaded {} but its raster cannot be used: {e}",
                tile.name
            )
        })?;
        let bytes: u64 = entry.files.iter().map(|f| f.bytes).sum();
        remaining = remaining
            .checked_sub(bytes)
            .ok_or("Terrain download exceeded its total transfer allowance")?;
        entries.push(entry);
    }
    Ok(entries)
}

/// Check actual raster support, sampling and decoding, including cached bytes.
/// A hash matching a previous download is not a provider-content validation.
pub fn validate_terrain_tile(
    path: &Path,
    tile: &TerrainTile,
    resolution: TerrainResolution,
) -> Result<(), String> {
    let mut raster = GeographicRaster::open(path)?;
    let metadata = raster.metadata();
    let height = match resolution {
        TerrainResolution::Glo90 => 1200,
        TerrainResolution::Glo30 => 3600,
    };
    let expected_step = 1.0 / f64::from(height);
    let [dx, dy] = metadata.step_deg;
    let [west, north] = metadata.sample_zero_lon_lat;
    let south = north - f64::from(metadata.height - 1) * dy;
    let east = west + f64::from(metadata.width - 1) * dx;
    let [s, w, n, e] = tile.bounds;
    let supported_longitude_spacing = [1.0, 1.5, 2.0, 3.0, 5.0, 10.0]
        .iter()
        .any(|factor| (dx / expected_step - factor).abs() < 1e-6);
    if metadata.height != height
        || (dy - expected_step).abs() > 1e-10
        || !supported_longitude_spacing
        || (west - w).abs() > 2.0 * dx
        || (east - e).abs() > 2.0 * dx
        || (north - n).abs() > 2.0 * dy
        || (south - s).abs() > 2.0 * dy
    {
        return Err("Raster dimensions, resolution or geographic support differ from the requested Copernicus tile".into());
    }
    raster.sample((north + south) / 2.0, (west + east) / 2.0)?;
    raster.verify_unchanged()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightningFormat {
    Csv,
    Json,
}
impl LightningFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LightningQuery {
    pub provider: String,
    pub start_utc: String,
    pub end_utc: String,
    pub bounds: [f64; 4],
}
impl LightningQuery {
    pub fn validate(&self) -> Result<[f64; 2], String> {
        validate_bounds(Some(self.bounds))?;
        if self.provider.trim().is_empty() || self.provider.len() > 240 {
            return Err("Enter the lightning export provider name".into());
        }
        let start = workspace::parse_utc(self.start_utc.trim())?;
        let end = workspace::parse_utc(self.end_utc.trim())?;
        if end <= start || end - start > 31.0 * 86400.0 {
            return Err("Choose an increasing lightning UTC interval of at most 31 days".into());
        }
        Ok([start, end])
    }
    pub fn request_text(&self) -> Result<String, String> {
        self.validate()?;
        Ok(format!(
            "Lightning export request\nProvider: {}\nStart UTC: {}\nEnd UTC: {}\nSouth, west, north, east: {:?}\nPlease provide authorized CSV or JSON with id, time_utc_s, latitude_deg, longitude_deg; optional altitude_m and intensity. UTC times must be Unix seconds. Include coverage and license notes.\n",
            self.provider, self.start_utc, self.end_utc, self.bounds
        ))
    }
}

/// Download a provider-authorized URL, then select actual records locally. The
/// rectangle/time fields are never represented as a server-side coverage claim.
pub fn download_lightning_with_cancel(
    manifest: &Path,
    url: &str,
    format: LightningFormat,
    query: &LightningQuery,
    notes: &str,
    max_bytes: u64,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, String> {
    query.validate()?;
    if max_bytes == 0 || max_bytes > MAX_LIGHTNING_BYTES {
        return Err("Lightning export import supports at most 0.0625 GiB per request".into());
    }
    let request = ImportRequest {
        label: format!("{} source export", query.provider),
        kind: DataKind::Lightning,
        date_utc: Some(query.start_utc.trim()[..10].into()),
        bounds: None,
        notes: notes.into(),
        max_bytes,
    };
    let raw =
        download_url_as_with_cancel(manifest, url, &request, cancel, Some(format.extension()))?;
    filter_lightning_export(manifest, &raw.id, query, notes, cancel)
}

pub fn filter_lightning_export(
    manifest: &Path,
    source_id: &str,
    query: &LightningQuery,
    notes: &str,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, String> {
    let [start, end] = query.validate()?;
    if notes.len() > 20_000 {
        return Err("Source notes exceed 20000 characters".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Lightning selection canceled; original export is retained".into());
    }
    let (source_dir, case_hash) = item_dir(manifest, source_id)?;
    let source = entry(&source_dir, &case_hash)?;
    if source.kind != DataKind::Lightning {
        return Err("Select a lightning export to filter".into());
    }
    let paths = verified_files(manifest, source_id)?;
    let path = paths
        .into_iter()
        .find(|p| p.ends_with(&source.primary_file))
        .ok_or("Lightning export is unavailable")?;
    let catalog = evidence_score::import_lightning(
        &path,
        &evidence_score::LightningProvenance {
            provider: query.provider.clone(),
            source: format!("Managed export {source_id}"),
            license: None,
            notes: Some(notes.into()),
            altitude_datum: evidence_score::LightningAltitudeDatum::Unknown,
            source_metadata: None,
        },
    )?;
    let original_count = catalog.strikes.len();
    let [s, w, n, e] = query.bounds;
    let strikes: Vec<_> = catalog
        .strikes
        .into_iter()
        .filter(|strike| {
            (start..=end).contains(&strike.time_utc_s)
                && (s..=n).contains(&strike.latitude_deg)
                && if w < e {
                    (w..=e).contains(&strike.longitude_deg)
                } else {
                    strike.longitude_deg >= w || strike.longitude_deg <= e
                }
        })
        .collect();
    if strikes.is_empty() {
        return Err("The export contains no strikes inside the requested time and region. Source export retained; this does not establish that lightning was absent.".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Lightning selection canceled; original export is retained".into());
    }
    if sha256(&path)? != catalog.input_sha256 {
        return Err("Lightning source changed during selection".into());
    }
    let count = strikes.len();
    let document = evidence_score::LightningDocument {
        schema_version: 1,
        provenance: Some(catalog.provenance.clone()),
        strikes,
    };
    // Compact input may grow when normalized; the result must remain loadable.
    if serde_json::to_vec_pretty(&document)
        .map_err(|e| e.to_string())?
        .len() as u64
        > MAX_LIGHTNING_BYTES
    {
        return Err("Selected strikes exceed the 0.0625 GiB normalized evidence allowance. Narrow the interval or region; original export retained.".into());
    }
    let (directory, id, hash) = new_item(manifest)?;
    let _lock = lock(&directory)?;
    write_new(&directory.join("lightning.json"), &document)?;
    write_new(&directory.join("request.json"), query)?;
    let value = CatalogEntry {
        schema_version: 1,
        id,
        case_manifest_sha256: hash,
        label: format!("{} · {} selected strikes", query.provider, count),
        kind: DataKind::Lightning,
        format: "filtered_lightning".into(),
        primary_file: "lightning.json".into(),
        created_unix_s: now()?.as_secs(),
        source: json!({"type":"local_lightning_time_region_selection","source_catalog_id":source_id,"source_sha256":catalog.input_sha256,"request":query,"original_strikes":original_count,"selected_strikes":count,"provider_coverage_verified":false,"filters_applied_locally":true}),
        files: vec![
            identity(&directory, &directory.join("lightning.json"))?,
            identity(&directory, &directory.join("request.json"))?,
        ],
        date_utc: Some(query.start_utc.trim()[..10].into()),
        bounds: Some(query.bounds),
        notes: format!(
            "{notes}\nSelected locally from an authorized export. This subset does not establish complete provider coverage. Strike altitude datum remains unconfirmed until declared in review."
        ),
    };
    unchanged_case(manifest, &value.case_manifest_sha256)?;
    write_new(&directory.join("entry.json"), &value)?;
    Ok(value)
}
