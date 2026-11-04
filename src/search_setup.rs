//! Small-region grid preparation. Nationwide work requires tiled corridor grids;
//! this intentionally refuses large rectangles rather than hiding that cost.
use crate::{
    physics::{EventTrace, PairConfig, Track},
    verification::{CoarseBounds, Observer, SearchInput, read_json},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
pub struct SearchSetup {
    pub normalized_tracks: PathBuf,
    #[serde(default)]
    pub event_files: [PathBuf; 2],
    /// south, west, north, east in WGS84 degrees.
    pub observer_bounds: [f64; 4],
    pub grid_spacing_m: f64,
    pub observer_ellipsoid_height_m: f64,
    /// Optional terrain-sampled observers, using the same grid bounds.
    #[serde(default)]
    pub observers_file: Option<PathBuf>,
    pub pair_config: PairConfig,
    pub coarse_bounds: [CoarseBounds; 2],
    pub measured_separation_s: f64,
    pub separation_tolerance_s: f64,
    pub provenance: Vec<String>,
}

pub fn prepare(path: &Path) -> Result<SearchInput, String> {
    let config: SearchSetup = read_json(path)?;
    let base = path.parent().unwrap_or(Path::new("."));
    if config.event_files.iter().any(|p| p.as_os_str().is_empty()) {
        return Err("Ordinary prepare-search requires two event_files; reviewed discovery uses prepare-reviewed-search".into());
    }
    let events = [
        read_json(&base.join(&config.event_files[0]))?,
        read_json(&base.join(&config.event_files[1]))?,
    ];
    prepare_events(config, base, events)
}

/// Use explicitly reviewed measurements with the same geometry and timing
/// configuration. No legacy event file is read by this entry point.
pub fn prepare_with_events(path: &Path, events: [EventTrace; 2]) -> Result<SearchInput, String> {
    let config: SearchSetup = read_json(path)?;
    prepare_events(config, path.parent().unwrap_or(Path::new(".")), events)
}

pub(crate) fn prepare_events(
    config: SearchSetup,
    base: &Path,
    events: [EventTrace; 2],
) -> Result<SearchInput, String> {
    let report: Value = read_json(&base.join(&config.normalized_tracks))?;
    let rows = report["passes"]
        .as_array()
        .ok_or("Normalized report needs a passes array")?;
    let tracks: Vec<Track> = rows
        .iter()
        .map(|r| serde_json::from_value(r["track"].clone()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let [south, west, north, east] = config.observer_bounds;
    if !config.observer_bounds.iter().all(|v| v.is_finite())
        || south >= north
        || west >= east
        || south < -85.
        || north > 85.
        || west < -180.
        || east > 180.
        || north - south > 5.
        || east - west > 5.
    {
        return Err("Use an ordered local box south,west,north,east, <=5 degrees per side and within +/-85 latitude; split date-line areas into separate boxes".into());
    }
    if !config.grid_spacing_m.is_finite()
        || config.grid_spacing_m <= 0.
        || !config.observer_ellipsoid_height_m.is_finite()
    {
        return Err("Grid spacing must be positive and height finite".into());
    }
    // WGS84 local radii give metre-scale steps; ranges remain geodetic and ECEF
    // is used for all source-to-observer physics. No lon/lat distance scoring.
    let lat = ((south + north) / 2.).to_radians();
    let e2 = 6.694_379_990_14e-3;
    let denom = 1. - e2 * lat.sin().powi(2);
    let lat_m_per_deg = 6_378_137. * (1. - e2) / denom.powf(1.5) * std::f64::consts::PI / 180.;
    let lon_m_per_deg = 6_378_137. / denom.sqrt() * lat.cos() * std::f64::consts::PI / 180.;
    let rows_f = ((north - south) * lat_m_per_deg / config.grid_spacing_m).ceil();
    let cols_f = ((east - west) * lon_m_per_deg / config.grid_spacing_m).ceil();
    if (rows_f + 1.) * (cols_f + 1.) > 100_000. {
        return Err("This local workbench limits a run to 100,000 observer points. Increase spacing or split the region.".into());
    }
    let (rows, cols) = (rows_f as usize, cols_f as usize);
    let mut observers = Vec::new();
    for row in 0..=rows {
        for col in 0..=cols {
            observers.push(Observer {
                id: format!("r{row}-c{col}"),
                latitude_deg: south + (north - south) * row as f64 / rows as f64,
                longitude_deg: west + (east - west) * col as f64 / cols as f64,
                ellipsoid_height_m: config.observer_ellipsoid_height_m,
            });
        }
    }
    let sampled_observers = config.observers_file.is_some();
    if let Some(ref observer_file) = config.observers_file {
        observers = read_json(&base.join(observer_file))?;
        if observers.is_empty()
            || observers.len() > 100_000
            || observers.iter().any(|o| {
                !o.latitude_deg.is_finite()
                    || !o.longitude_deg.is_finite()
                    || !o.ellipsoid_height_m.is_finite()
                    || o.latitude_deg < south
                    || o.latitude_deg > north
                    || o.longitude_deg < west
                    || o.longitude_deg > east
            })
        {
            return Err("Terrain-sampled observers must be finite, inside the declared region, and number 1..100000".into());
        }
    }
    let mut provenance = config.provenance;
    provenance.push(format!(
        "Normalized tracks: {}",
        base.join(config.normalized_tracks).display()
    ));
    if sampled_observers {
        provenance.push(format!(
            "Observer coordinates and per-point ellipsoidal heights loaded from {}",
            base.join(config.observers_file.unwrap()).display()
        ));
    } else {
        provenance.push(format!("Uniform observer ellipsoidal height {}m is an explicit approximation; this run does not sample terrain",config.observer_ellipsoid_height_m));
    }
    provenance.push(format!(
        "Normalization metadata: {}",
        report.get("config").unwrap_or(&Value::Null)
    ));
    provenance.push(format!(
        "Normalization exclusions/statistics: {}",
        report.get("stats").unwrap_or(&Value::Null)
    ));
    provenance.push(format!(
        "Normalization quality notes: {}",
        report.get("quality_notes").unwrap_or(&Value::Null)
    ));
    provenance.push(format!(
        "Retained normalization issue examples: {}; see the original report for each issue",
        report
            .get("issues")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    ));
    provenance.push(format!("Acquisition coverage supplied by this importer: {} (null means unspecified, never complete by inference)",report.get("acquisition").unwrap_or(&Value::Null)));
    Ok(SearchInput {
        schema_version: 1,
        evidence_kind: "real_recording".into(),
        provenance,
        tracks,
        observers,
        events,
        pair_config: config.pair_config,
        coarse_bounds: config.coarse_bounds,
        measured_separation_s: config.measured_separation_s,
        separation_tolerance_s: config.separation_tolerance_s,
    })
}
