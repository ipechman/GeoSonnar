//! Read-only geographic overlays from actual native data. Displaying a candidate
//! is not a location verdict; absent UTC origins and measurement gaps stay explicit.
use crate::{
    detailed_data::sha256,
    evidence_score::{self, LightningAltitudeDatum, LightningProvenance, LightningStrike},
    physics::Track,
    terrain::GeographicRaster,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

const MAX_JSON_BYTES: u64 = 256 * 1024 * 1024;
const MAX_POINTS: usize = 2_000_000;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeoPoint {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub height_m: Option<f64>,
    pub time_s: Option<f64>,
    pub time_utc_s: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeoTrack {
    pub id: String,
    pub points: Vec<GeoPoint>,
    /// [start,end_exclusive]; never join adjacent slices across missing support.
    pub segments: Vec<[usize; 2]>,
    pub time_origin_utc_s: Option<f64>,
    pub max_gap_s: Option<f64>,
    pub physics_track: Track,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MapCandidate {
    pub id: String,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub height_m: Option<f64>,
    pub score: Option<f64>,
    pub rank: usize,
    pub flight_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GridInfo {
    pub bounds: [f64; 4],
    pub point_count: usize,
    pub declared_spacing_m: Option<f64>,
    pub latitude_step_deg: Option<f64>,
    pub longitude_step_deg: Option<f64>,
    pub regular_spacing: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TerrainInfo {
    pub bounds: [f64; 4],
    pub width: u32,
    pub height: u32,
    pub step_deg: [f64; 2],
    pub nodata: Option<f64>,
    pub height_meaning: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MapData {
    pub source_path: PathBuf,
    pub source_sha256: String,
    pub time_origin_utc_s: Option<f64>,
    pub tracks: Vec<GeoTrack>,
    pub candidates: Vec<MapCandidate>,
    pub observers: Vec<GeoPoint>,
    pub grid: Option<GridInfo>,
    pub terrain: Option<TerrainInfo>,
    pub lightning: Vec<LightningStrike>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightning_provenance: Option<LightningProvenance>,
    pub notes: Vec<String>,
    pub location_verified: bool,
}

fn number(v: &Value, name: &str) -> Result<f64, String> {
    v[name]
        .as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("Missing or nonfinite {name}"))
}
fn coordinate(lat: f64, lon: f64) -> Result<(), String> {
    if !lat.is_finite()
        || !lon.is_finite()
        || !(-90. ..=90.).contains(&lat)
        || !(-180. ..=180.).contains(&lon)
    {
        Err("Invalid geographic coordinate".into())
    } else {
        Ok(())
    }
}
fn geo(v: &Value) -> Result<GeoPoint, String> {
    let lat = number(v, "latitude_deg")?;
    let lon = number(v, "longitude_deg")?;
    coordinate(lat, lon)?;
    let height = v
        .get("ellipsoid_height_m")
        .or_else(|| v.get("height_m"))
        .and_then(Value::as_f64);
    if height.is_some_and(|h| !h.is_finite()) {
        return Err("Nonfinite map height".into());
    }
    Ok(GeoPoint {
        latitude_deg: lat,
        longitude_deg: lon,
        height_m: height,
        time_s: None,
        time_utc_s: None,
    })
}
/// WGS84 inverse. Reject local ENU/demo coordinates rather than plotting them as ECEF.
pub fn ecef_to_geodetic(xyz: [f64; 3]) -> Result<[f64; 3], String> {
    let [x, y, z] = xyz;
    if !xyz.iter().all(|n| n.is_finite()) {
        return Err("Nonfinite ECEF position".into());
    }
    let radius = x.hypot(y).hypot(z);
    if !(5_000_000. ..=8_000_000.).contains(&radius) {
        return Err("Map tracks need WGS84 ECEF positions, not local Cartesian coordinates".into());
    }
    let a = 6_378_137.0;
    let e2 = 6.694_379_990_141_316_5e-3;
    let p = x.hypot(y);
    if p < 1e-8 {
        return Ok([
            if z >= 0. { 90. } else { -90. },
            0.,
            z.abs() - 6_356_752.314_245_179,
        ]);
    }
    let mut lat = z.atan2(p * (1. - e2));
    for _ in 0..12 {
        let n = a / (1. - e2 * lat.sin().powi(2)).sqrt();
        let next = (z + e2 * n * lat.sin()).atan2(p);
        if (next - lat).abs() < 1e-14 {
            lat = next;
            break;
        }
        lat = next;
    }
    let n = a / (1. - e2 * lat.sin().powi(2)).sqrt();
    let height = if lat.cos().abs() > 1e-8 {
        p / lat.cos() - n
    } else {
        z / lat.sin() - n * (1. - e2)
    };
    let result = [lat.to_degrees(), y.atan2(x).to_degrees(), height];
    coordinate(result[0], result[1])?;
    Ok(result)
}
pub fn geographic_bounds(points: &[GeoPoint]) -> Option<[f64; 4]> {
    if points.is_empty() {
        return None;
    }
    let south = points
        .iter()
        .map(|p| p.latitude_deg)
        .fold(f64::INFINITY, f64::min);
    let north = points
        .iter()
        .map(|p| p.latitude_deg)
        .fold(f64::NEG_INFINITY, f64::max);
    let mut longitudes: Vec<_> = points.iter().map(|p| p.longitude_deg).collect();
    longitudes.sort_by(f64::total_cmp);
    longitudes.dedup();
    let mut largest = -1.;
    let mut index = 0;
    for i in 0..longitudes.len() {
        let next = if i + 1 < longitudes.len() {
            longitudes[i + 1]
        } else {
            longitudes[0] + 360.
        };
        if next - longitudes[i] > largest {
            largest = next - longitudes[i];
            index = i;
        }
    }
    Some([
        south,
        longitudes[(index + 1) % longitudes.len()],
        north,
        longitudes[index],
    ])
}
fn spacing(mut values: Vec<f64>) -> (Option<f64>, bool) {
    values.sort_by(f64::total_cmp);
    values.dedup_by(|a, b| (*a - *b).abs() < 1e-10);
    if values.len() < 2 {
        return (None, true);
    }
    let gaps: Vec<_> = values.windows(2).map(|v| v[1] - v[0]).collect();
    let first = gaps[0];
    let regular = gaps
        .iter()
        .all(|g| (*g - first).abs() < 1e-8_f64.max(first.abs() * 1e-6));
    (regular.then_some(first), regular)
}
fn grid(points: &[GeoPoint], declared: Option<f64>) -> Option<GridInfo> {
    let bounds = geographic_bounds(points)?;
    let (lat, lr) = spacing(points.iter().map(|p| p.latitude_deg).collect());
    let (lon, or) = spacing(points.iter().map(|p| p.longitude_deg).collect());
    let mut locations = BTreeSet::new();
    for p in points {
        locations.insert((p.latitude_deg.to_bits(), p.longitude_deg.to_bits()));
    }
    let mut lats: Vec<_> = points.iter().map(|p| p.latitude_deg).collect();
    lats.sort_by(f64::total_cmp);
    lats.dedup();
    let mut lons: Vec<_> = points.iter().map(|p| p.longitude_deg).collect();
    lons.sort_by(f64::total_cmp);
    lons.dedup();
    Some(GridInfo {
        bounds,
        point_count: points.len(),
        declared_spacing_m: declared,
        latitude_step_deg: lat,
        longitude_step_deg: lon,
        regular_spacing: lr
            && or
            && locations.len() == points.len()
            && lats.len().checked_mul(lons.len()) == Some(points.len()),
    })
}
fn origin(v: &Value) -> Option<f64> {
    v.get("time_origin_utc_s")
        .or_else(|| v.pointer("/config/time_origin_utc_s"))
        .and_then(Value::as_f64)
}
fn track(v: &Value, time_origin: Option<f64>, max_gap: Option<f64>) -> Result<GeoTrack, String> {
    let raw: Track =
        serde_json::from_value(v.clone()).map_err(|e| format!("Unsupported native track: {e}"))?;
    if raw.flight_id.is_empty() || raw.points.is_empty() || raw.points.len() > MAX_POINTS {
        return Err("Track needs an identity and bounded measured points".into());
    }
    let mut points = Vec::with_capacity(raw.points.len());
    let mut segments = Vec::new();
    let mut start = 0;
    for (i, p) in raw.points.iter().enumerate() {
        if !p.t_s.is_finite() || (i > 0 && p.t_s <= raw.points[i - 1].t_s) {
            return Err("Track timestamps must be finite and strictly increasing".into());
        }
        let [lat, lon, h] = ecef_to_geodetic(p.xyz_m)?;
        if i > 0 && max_gap.is_none_or(|limit| p.t_s - raw.points[i - 1].t_s > limit) {
            segments.push([start, i]);
            start = i;
        }
        points.push(GeoPoint {
            latitude_deg: lat,
            longitude_deg: lon,
            height_m: Some(h),
            time_s: Some(p.t_s),
            time_utc_s: time_origin.map(|t| t + p.t_s),
        });
        if points
            .last()
            .unwrap()
            .time_utc_s
            .is_some_and(|t| !t.is_finite() || !(0. ..253_402_300_800.).contains(&t))
        {
            return Err("Track timestamp falls outside the supported UTC range".into());
        }
    }
    segments.push([start, points.len()]);
    Ok(GeoTrack {
        id: raw.flight_id.clone(),
        points,
        segments,
        time_origin_utc_s: time_origin,
        max_gap_s: max_gap,
        physics_track: raw,
    })
}
fn raster_info(v: &Value) -> Result<TerrainInfo, String> {
    let width = v["width"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or("Invalid raster width")?;
    let height = v["height"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or("Invalid raster height")?;
    let pair = |key: &str| -> Result<[f64; 2], String> {
        let a = v[key]
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or("Invalid raster coordinates")?;
        Ok([
            a[0].as_f64()
                .filter(|n| n.is_finite())
                .ok_or("Invalid raster coordinates")?,
            a[1].as_f64()
                .filter(|n| n.is_finite())
                .ok_or("Invalid raster coordinates")?,
        ])
    };
    let [west, north] = pair("sample_zero_lon_lat")?;
    let step = pair("step_deg")?;
    if step.iter().any(|v| *v <= 0.) {
        return Err("Raster sample spacing must be positive".into());
    }
    let east = west + f64::from(width - 1) * step[0];
    let south = north - f64::from(height - 1) * step[1];
    coordinate(north, west)?;
    coordinate(south, east)?;
    Ok(TerrainInfo{bounds:[south,west,north,east],width,height,step_deg:step,nodata:v["nodata"].as_f64(),height_meaning:"Source raster values; vertical datum must be declared separately. Bounds describe sample support.".into()})
}
pub fn parse_value(value: &Value) -> Result<MapData, String> {
    parse_value_with_origin(value, None)
}
pub fn parse_value_with_origin(
    value: &Value,
    explicit_origin: Option<f64>,
) -> Result<MapData, String> {
    // terrain_grid writes its observer list as a standalone native JSON array.
    if value.is_array() {
        return parse_value_with_origin(&serde_json::json!({"observers":value}), explicit_origin);
    }
    for key in ["tracks", "passes", "candidates", "observers", "strikes"] {
        if value.get(key).is_some_and(|v| !v.is_array()) {
            return Err(format!("Map field {key} must be an array"));
        }
    }
    if value
        .get("time_origin_utc_s")
        .or_else(|| value.pointer("/config/time_origin_utc_s"))
        .is_some_and(|v| !v.is_number())
    {
        return Err("Declared track UTC origin must be numeric Unix seconds".into());
    }
    let declared = origin(value);
    if let (Some(a), Some(b)) = (
        value.get("time_origin_utc_s"),
        value.pointer("/config/time_origin_utc_s"),
    ) && a != b
    {
        return Err("Root and configuration UTC origins disagree".into());
    }
    if declared
        .into_iter()
        .chain(explicit_origin)
        .any(|n| !n.is_finite() || !(0. ..253_402_300_800.).contains(&n))
    {
        return Err("UTC origin must be finite Unix seconds".into());
    }
    if let (Some(a), Some(b)) = (declared, explicit_origin)
        && a != b
    {
        return Err("Explicit UTC origin conflicts with the file's declared origin".into());
    }
    let time_origin = declared.or(explicit_origin);
    let mut result=MapData{source_path:PathBuf::new(),source_sha256:String::new(),time_origin_utc_s:time_origin,tracks:Vec::new(),candidates:Vec::new(),observers:Vec::new(),grid:None,terrain:None,lightning:Vec::new(),lightning_provenance:None,notes:vec!["Map candidates and scores are hypotheses, not calibrated confidence or a verified location.".into()],location_verified:false};
    let max_gap = value
        .pointer("/config/max_gap_s")
        .or_else(|| value.pointer("/pair_config/model/max_gap_s"))
        .or_else(|| value.pointer("/model/max_gap_s"))
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.);
    let values: Vec<&Value> = if let Some(passes) = value["passes"].as_array() {
        passes.iter().map(|p| &p["track"]).collect()
    } else if let Some(tracks) = value["tracks"].as_array() {
        tracks.iter().collect()
    } else {
        Vec::new()
    };
    let mut total = 0usize;
    let mut ids = BTreeSet::new();
    for v in values {
        let item = track(v, time_origin, max_gap)?;
        total = total.saturating_add(item.points.len());
        if total > MAX_POINTS || !ids.insert(item.id.clone()) {
            return Err("Map track input exceeds its point limit or repeats an identity".into());
        }
        result.tracks.push(item);
    }
    if !result.tracks.is_empty() && time_origin.is_none() {
        result.notes.push("Track time origin is absent. Relative seconds are retained; a UTC timeline requires an explicit origin.".into());
    }
    if !result.tracks.is_empty() && max_gap.is_none() {
        result.notes.push("No maximum interpolation gap was declared. Track samples are displayed as isolated measurements.".into());
    }
    if let Some(observers) = value["observers"].as_array() {
        if observers.len() > 100_000 {
            return Err("Observer overlay exceeds 100,000 points".into());
        }
        result.observers = observers.iter().map(geo).collect::<Result<_, _>>()?;
    }
    let declared_spacing = value
        .get("maximum_grid_spacing_m")
        .or_else(|| value.get("grid_spacing_m"))
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n > 0.);
    result.grid = grid(&result.observers, declared_spacing);
    if let Some(candidates) = value["candidates"].as_array() {
        if candidates.len() > 100_000 {
            return Err("Candidate overlay exceeds 100,000 locations".into());
        }
        for (i, candidate) in candidates.iter().enumerate() {
            let p = geo(&candidate["observer"])?;
            let score = candidate
                .pointer("/score/combined_rmse_hz")
                .or_else(|| candidate.pointer("/score/rmse_hz"))
                .and_then(Value::as_f64);
            if score.is_some_and(|n| !n.is_finite() || n < 0.) {
                return Err("Invalid candidate residual score".into());
            }
            result.candidates.push(MapCandidate {
                id: candidate
                    .pointer("/observer/id")
                    .and_then(Value::as_str)
                    .unwrap_or("observer")
                    .into(),
                latitude_deg: p.latitude_deg,
                longitude_deg: p.longitude_deg,
                height_m: p.height_m,
                score,
                rank: i + 1,
                flight_ids: candidate
                    .pointer("/score/flight_ids")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
    }
    if value.get("strikes").is_some() {
        let document:evidence_score::LightningDocument=serde_json::from_value(serde_json::json!({"schema_version":value["schema_version"],"strikes":value["strikes"],"provenance":value.get("provenance")})).map_err(|e|e.to_string())?;
        if document.schema_version != 1 || document.strikes.len() > 250_000 {
            return Err("Unsupported lightning schema or too many strikes".into());
        }
        let mut seen = BTreeSet::new();
        for strike in &document.strikes {
            coordinate(strike.latitude_deg, strike.longitude_deg)?;
            if strike.id.is_empty()
                || !seen.insert(&strike.id)
                || !strike.time_utc_s.is_finite()
                || !(0. ..253_402_300_800.).contains(&strike.time_utc_s)
            {
                return Err("Invalid lightning identity or UTC time".into());
            }
        }
        crate::lightning_source::validate_source(document.provenance.as_ref(), &document.strikes)?;
        if let Some(provenance) = &document.provenance {
            result
                .notes
                .extend(crate::lightning_source::source_notes(provenance));
        }
        result.lightning_provenance = document.provenance;
        result.lightning = document.strikes;
    }
    if value.get("sample_zero_lon_lat").is_some() {
        result.terrain = Some(raster_info(value)?);
    } else if value["dem"].get("sample_zero_lon_lat").is_some() {
        result.terrain = Some(raster_info(&value["dem"])?);
    }
    if result.tracks.is_empty()
        && result.candidates.is_empty()
        && result.observers.is_empty()
        && result.lightning.is_empty()
        && result.terrain.is_none()
        && !["tracks", "passes", "candidates", "observers", "strikes"]
            .iter()
            .any(|k| value.get(k).is_some())
    {
        return Err("No supported native tracks, search candidates, observer grid, lightning export or geographic terrain metadata found".into());
    }
    Ok(result)
}
pub fn load(path: &Path) -> Result<MapData, String> {
    load_with_time_origin(path, None)
}
pub fn load_with_time_origin(path: &Path, explicit_origin: Option<f64>) -> Result<MapData, String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !["tif", "tiff", "csv"].contains(&extension.as_str())
        && fs::metadata(&path).map_err(|e| e.to_string())?.len() > MAX_JSON_BYTES
    {
        return Err(format!(
            "Map JSON exceeds {}; import a bounded region first",
            crate::data_size::format_gib(256 * 1024 * 1024)
        ));
    }
    let before = sha256(&path)?;
    let mut result = if ["tif", "tiff"].contains(&extension.as_str()) {
        let raster = GeographicRaster::open(&path)?;
        let v = serde_json::to_value(raster.metadata()).map_err(|e| e.to_string())?;
        let result = parse_value_with_origin(&v, explicit_origin)?;
        raster.verify_unchanged()?;
        result
    } else if extension == "csv" {
        let catalog = evidence_score::import_lightning(
            &path,
            &LightningProvenance {
                provider: "User-supplied export".into(),
                source: path.display().to_string(),
                license: None,
                notes: Some(
                    "Map display only; verify provider and vertical datum before thunder scoring."
                        .into(),
                ),
                altitude_datum: LightningAltitudeDatum::Unknown,
                source_metadata: None,
            },
        )?;
        parse_value(&serde_json::json!({"schema_version":1,"strikes":catalog.strikes}))?
    } else {
        if fs::metadata(&path).map_err(|e| e.to_string())?.len() > MAX_JSON_BYTES {
            return Err(format!(
                "Map JSON exceeds {}; import a bounded region first",
                crate::data_size::format_gib(256 * 1024 * 1024)
            ));
        }
        let mut bytes = Vec::new();
        File::open(&path)
            .map_err(|e| e.to_string())?
            .take(MAX_JSON_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_JSON_BYTES {
            return Err("Map input grew beyond its byte limit".into());
        }
        let value =
            serde_json::from_slice(&bytes).map_err(|e| format!("Map JSON cannot be read: {e}"))?;
        parse_value_with_origin(&value, explicit_origin)?
    };
    if sha256(&path)? != before {
        return Err("Map input changed while loading".into());
    }
    result.source_path = path;
    result.source_sha256 = before;
    Ok(result)
}
