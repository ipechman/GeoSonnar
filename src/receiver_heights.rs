//! Explicit, source-bound receiver heights for regional and manual searches.
//! No missing surface or geoid sample is replaced by a fixed height.
use crate::{
    evidence_score::ObserverLocation,
    terrain::{GeographicRaster, RasterMetadata},
    workspace::{ReviewWorkspace, SourceBinding},
    workspace_scoring,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceDatum {
    Wgs84Ellipsoid,
    Egm96,
    Egm2008,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoidSource {
    pub source_sha256: String,
    pub reference: SurfaceDatum,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverTerrain {
    pub surface_sha256s: Vec<String>,
    pub surface_datum: SurfaceDatum,
    pub geoid: Option<GeoidSource>,
    /// Height above the declared raster surface, which may be a DSM.
    pub above_surface_m: f64,
    pub notes: String,
}

impl ReceiverTerrain {
    pub fn validate(&self) -> Result<(), String> {
        let valid_hash = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        let unique: BTreeSet<_> = self.surface_sha256s.iter().collect();
        if self.surface_sha256s.is_empty()
            || self.surface_sha256s.len() > 64
            || unique.len() != self.surface_sha256s.len()
            || !self.surface_sha256s.iter().all(|s| valid_hash(s))
            || !self.above_surface_m.is_finite()
            || !(0.0..=1000.0).contains(&self.above_surface_m)
            || self.notes.len() > 16_384
        {
            return Err("Receiver terrain needs 1..64 distinct source hashes and 0..1000 m above the surface".into());
        }
        match (self.surface_datum, &self.geoid) {
            (SurfaceDatum::Wgs84Ellipsoid, None) => Ok(()),
            (SurfaceDatum::Wgs84Ellipsoid, Some(_)) => Err("Ellipsoid terrain must not receive a second geoid correction".into()),
            (datum, Some(geoid)) if geoid.reference == datum && valid_hash(&geoid.source_sha256)
                && !unique.contains(&geoid.source_sha256) => Ok(()),
            _ => Err("Orthometric terrain requires a separately registered geoid with the same EGM96 or EGM2008 reference".into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeightSample {
    pub observer_index: usize,
    pub surface_m: f64,
    pub geoid_undulation_m: f64,
    pub above_surface_m: f64,
    pub surface_sources: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeightFailure {
    pub observer_index: usize,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReceiverHeightReport {
    pub schema_version: u32,
    pub settings: Option<ReceiverTerrain>,
    /// Original order. Unavailable positions have no substituted height.
    pub observers: Vec<Option<ObserverLocation>>,
    pub samples: Vec<HeightSample>,
    pub unavailable: Vec<HeightFailure>,
    pub sources: Vec<RasterMetadata>,
    pub limitations: Vec<String>,
}

impl ReceiverHeightReport {
    pub fn complete_observers(&self) -> Result<Vec<ObserverLocation>, String> {
        if !self.unavailable.is_empty() || self.observers.iter().any(Option::is_none) {
            return Err(format!(
                "Receiver height coverage is incomplete: {} of {} positions unavailable. Inspect coverage or explicitly change the height model before searching.",
                self.observers.iter().filter(|p| p.is_none()).count(),
                self.observers.len()
            ));
        }
        Ok(self.observers.iter().flatten().cloned().collect())
    }
}

fn open_source(hash: &str, bindings: &[SourceBinding]) -> Result<GeographicRaster, String> {
    let matches: Vec<_> = bindings.iter().filter(|b| b.sha256 == hash).collect();
    if matches.len() != 1 || !matches[0].path.is_absolute() {
        return Err("Height source must identify exactly one restored, registered dataset".into());
    }
    let raster = GeographicRaster::open(&matches[0].path)?;
    if raster.metadata().sha256 != hash {
        return Err("Receiver height dataset changed after registration".into());
    }
    Ok(raster)
}

fn covers(metadata: &RasterMetadata, observer: &ObserverLocation) -> bool {
    let [west, north] = metadata.sample_zero_lon_lat;
    let [dx, dy] = metadata.step_deg;
    let east = west + dx * f64::from(metadata.width - 1);
    let south = north - dy * f64::from(metadata.height - 1);
    observer.longitude_deg >= west - dx * 1e-7
        && observer.longitude_deg <= east + dx * 1e-7
        && observer.latitude_deg >= south - dy * 1e-7
        && observer.latitude_deg <= north + dy * 1e-7
}

pub fn resolve(
    settings: Option<&ReceiverTerrain>,
    bindings: &[SourceBinding],
    observers: &[ObserverLocation],
) -> Result<ReceiverHeightReport, String> {
    if observers.is_empty() || observers.len() > workspace_scoring::MAX_OBSERVERS {
        return Err("Resolve heights for 1..10000 declared receiver positions".into());
    }
    for observer in observers {
        crate::physics::geodetic_to_ecef(
            observer.latitude_deg,
            observer.longitude_deg,
            observer.ellipsoid_height_m,
        )?;
    }
    let mut report = ReceiverHeightReport {
        schema_version: 1,
        settings: settings.cloned(),
        observers: observers.iter().cloned().map(Some).collect(),
        samples: vec![],
        unavailable: vec![],
        sources: vec![],
        limitations: vec![],
    };
    let Some(settings) = settings else {
        report.limitations.push("Receiver ellipsoid heights are explicitly declared fixed values; no terrain was sampled.".into());
        return Ok(report);
    };
    settings.validate()?;
    let mut surface_values: Vec<Vec<(String, f64)>> = vec![Vec::new(); observers.len()];
    let mut errors: Vec<Vec<String>> = vec![Vec::new(); observers.len()];
    let mut hashes = settings.surface_sha256s.clone();
    hashes.sort();
    // One surface raster at a time bounds cache memory independently of tile count.
    for hash in &hashes {
        let mut raster = open_source(hash, bindings)?;
        for (i, observer) in observers.iter().enumerate() {
            if covers(raster.metadata(), observer) {
                match raster.sample(observer.latitude_deg, observer.longitude_deg) {
                    Ok(value) => surface_values[i].push((hash.clone(), value)),
                    Err(error) => errors[i].push(format!("Surface {}: {error}", &hash[..8])),
                }
            }
        }
        raster.verify_unchanged()?;
        report.sources.push(raster.metadata().clone());
    }
    let mut geoid = settings
        .geoid
        .as_ref()
        .map(|g| open_source(&g.source_sha256, bindings))
        .transpose()?;
    for (i, original) in observers.iter().enumerate() {
        let surfaces = &surface_values[i];
        if surfaces.is_empty() {
            errors[i].push("No declared surface raster supplies this receiver".into());
        } else if surfaces
            .iter()
            .any(|(_, value)| (value - surfaces[0].1).abs() > 0.01)
        {
            errors[i].push("Overlapping surface rasters disagree by more than 0.01 m; choose a consistent source set".into());
        }
        let undulation = match &mut geoid {
            Some(raster) => match raster.sample(original.latitude_deg, original.longitude_deg) {
                Ok(value) => value,
                Err(error) => {
                    errors[i].push(format!("Geoid: {error}"));
                    0.0
                }
            },
            None => 0.0,
        };
        if errors[i].is_empty() {
            let height = surfaces[0].1 + undulation + settings.above_surface_m;
            if !height.is_finite() || !(-1000.0..=100_000.0).contains(&height) {
                errors[i].push(
                    "Computed receiver height is outside the supported physical range".into(),
                );
            } else {
                let mut observer = original.clone();
                observer.ellipsoid_height_m = height;
                report.observers[i] = Some(observer);
                report.samples.push(HeightSample {
                    observer_index: i,
                    surface_m: surfaces[0].1,
                    geoid_undulation_m: undulation,
                    above_surface_m: settings.above_surface_m,
                    surface_sources: surfaces.iter().map(|(hash, _)| hash.clone()).collect(),
                });
            }
        }
        if !errors[i].is_empty() {
            report.observers[i] = None;
            report.unavailable.push(HeightFailure {
                observer_index: i,
                latitude_deg: original.latitude_deg,
                longitude_deg: original.longitude_deg,
                reason: errors[i].join("; "),
            });
        }
    }
    if let Some(geoid) = geoid {
        geoid.verify_unchanged()?;
        report.sources.push(geoid.metadata().clone());
    }
    // Recheck every file after the batch, including surfaces opened earlier.
    for source in &report.sources {
        if crate::detailed_data::sha256(&source.path)? != source.sha256 {
            return Err("Receiver height source changed during sampling".into());
        }
    }
    report.limitations = vec![
        "Vertical references and metre-valued samples are analyst declarations, not inferred from a filename.".into(),
        "Ellipsoid height = declared surface height + compatible geoid undulation + height above surface. Ellipsoid surfaces receive no geoid correction.".into(),
        "A DSM includes vegetation and structures. Height above its surface is not necessarily height above surveyed bare ground.".into(),
        "Missing data and conflicting overlaps remain unavailable; searches require complete declared grid coverage.".into(),
        "Bilinear sampling stays within each raster's pixel-centre support; tile-edge gaps are not filled or extrapolated.".into(),
    ];
    Ok(report)
}

pub fn grid(document: &ReviewWorkspace) -> Result<ReceiverHeightReport, String> {
    document.validate()?;
    resolve(
        document.receiver_terrain.as_ref(),
        &document.datasets,
        &workspace_scoring::grid_observers(&document.map)?,
    )
}
