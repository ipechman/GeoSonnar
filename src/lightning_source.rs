//! Retained FMI source semantics and explicit analyst interpretation.
//! Whole-second representation is not a calibrated clock error; ETRS89 values
//! are never silently relabeled as a surveyed WGS84 transformation.
use crate::evidence_score::{
    LightningAltitudeDatum, LightningCatalog, LightningProvenance, LightningStrike,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FmiHorizontalCrs {
    Etrs89Epsg4258,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimestampRounding {
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EllipseMajorUnit {
    ConflictingMetresKilometres,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FmiSourceMetadata {
    pub schema_version: u32,
    pub horizontal_crs: FmiHorizontalCrs,
    pub time_resolution_s: f64,
    pub timestamp_rounding: TimestampRounding,
    pub ellipse_major_unit: EllipseMajorUnit,
}
impl Default for FmiSourceMetadata {
    fn default() -> Self {
        Self {
            schema_version: 1,
            horizontal_crs: FmiHorizontalCrs::Etrs89Epsg4258,
            time_resolution_s: 1.0,
            timestamp_rounding: TimestampRounding::Unknown,
            ellipse_major_unit: EllipseMajorUnit::ConflictingMetresKilometres,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FmiRecord {
    pub response_sha256: String,
    /// Zero-based original row in the immutable provider response, before filtering.
    pub row_ordinal: usize,
    pub multiplicity: u32,
    pub peak_current_ka: f64,
    pub cloud_indicator: u8,
    /// Raw value only. Source documentation disagrees on units and does not
    /// establish the confidence/axis convention; never use as an error radius.
    pub ellipse_major_raw: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightningSourcePolicy {
    pub catalog_sha256: String,
    pub etrs89_as_wgs84_rationale: String,
    /// Analyst-declared representation allowance, not a Gaussian sigma. The
    /// existing thunder model scale must already be at least this large.
    pub time_representation_allowance_s: f64,
}
impl LightningSourcePolicy {
    /// Structural bounds also protect saved, currently inactive assumptions.
    /// Readiness (nonblank rationale, matching bytes, >=1s) is checked only when
    /// the FMI thunder source is actually used.
    pub fn validate_shape(&self) -> Result<(), String> {
        if !hash_valid(&self.catalog_sha256)
            || self.etrs89_as_wgs84_rationale.len() > 4000
            || !self.time_representation_allowance_s.is_finite()
            || !(0.0..=3600.0).contains(&self.time_representation_allowance_s)
        {
            return Err("Saved lightning interpretation needs a valid source hash, rationale at most4000 bytes and finite allowance within0..3600s".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightningInterpretationDiagnostic {
    pub source_crs: FmiHorizontalCrs,
    pub source_time_resolution_s: f64,
    pub timestamp_rounding: TimestampRounding,
    pub declared_time_representation_allowance_s: f64,
    pub etrs89_as_wgs84_rationale: String,
    pub datum_transform_applied: bool,
    pub source_clock_accuracy_calibrated: bool,
}

fn hash_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Also called by map/native import paths, where empty provider results may be
/// represented for inspection but are not a scoreable lightning catalog.
pub fn validate_source(
    provenance: Option<&LightningProvenance>,
    strikes: &[LightningStrike],
) -> Result<(), String> {
    let metadata = provenance.and_then(|p| p.source_metadata.as_ref());
    if let Some(metadata) = metadata {
        if metadata.schema_version != 1 || metadata.time_resolution_s.to_bits() != 1.0_f64.to_bits()
        {
            return Err("FMI source metadata requires schema 1 and its original one-second timestamp representation".into());
        }
        let provenance = provenance.expect("metadata has provenance");
        if provenance.provider.trim().is_empty()
            || provenance.source.trim().is_empty()
            || provenance
                .license
                .as_ref()
                .is_none_or(|v| v.trim().is_empty())
            || provenance.provider.len() > 512
            || provenance.source.len() > 8192
            || provenance.license.as_ref().is_some_and(|v| v.len() > 2048)
            || provenance.notes.as_ref().is_some_and(|v| v.len() > 20_000)
            || provenance.altitude_datum != LightningAltitudeDatum::Unknown
        {
            return Err("FMI provenance must retain bounded provider/source/license/notes (512/8192/2048/20000 bytes) and unknown strike altitude datum".into());
        }
    }
    let mut provider_rows = BTreeSet::new();
    for strike in strikes {
        match (&metadata, &strike.source_record) {
            (Some(_), Some(record)) => {
                if !strike.id.starts_with("fmi-") || !hash_valid(&record.response_sha256)
                    || record.row_ordinal >= 250_000 || !record.peak_current_ka.is_finite()
                    || !provider_rows.insert((record.response_sha256.as_str(), record.row_ordinal))
                    || record.cloud_indicator > 1 || !record.ellipse_major_raw.is_finite()
                    || record.ellipse_major_raw < 0.0 || !strike.time_utc_s.is_finite()
                    || strike.time_utc_s.fract() != 0.0 || strike.altitude_m.is_some()
                    || strike.intensity.is_some() {
                    return Err(format!("FMI strike {} has invalid or altered source semantics; retain integer UTC, raw fields and unknown acoustic height", strike.id));
                }
            }
            (None, None) if !strike.id.starts_with("fmi-") => {}
            _ => return Err("FMI record metadata and source provenance must remain together; source semantics cannot be dropped during import or filtering".into()),
        }
    }
    Ok(())
}

pub fn validate_catalog_source(catalog: &LightningCatalog) -> Result<(), String> {
    crate::evidence_score::validate_strikes(&catalog.strikes)?;
    validate_source(Some(&catalog.provenance), &catalog.strikes)
}

pub fn validate_policy(
    catalog: &LightningCatalog,
    policy: Option<&LightningSourcePolicy>,
    thunder_scale_s: f64,
) -> Result<(), String> {
    validate_catalog_source(catalog)?;
    let Some(metadata) = catalog.provenance.source_metadata.as_ref() else {
        return Ok(());
    };
    validate_policy_parameters(&catalog.input_sha256, metadata, policy, thunder_scale_s)
}

/// Constant-size readiness check for an already validated immutable catalog.
/// GUI callers must not rescan every strike while drawing a draft policy.
pub fn validate_policy_parameters(
    catalog_sha256: &str,
    metadata: &FmiSourceMetadata,
    policy: Option<&LightningSourcePolicy>,
    thunder_scale_s: f64,
) -> Result<(), String> {
    if metadata.schema_version != 1 || metadata.time_resolution_s.to_bits() != 1.0_f64.to_bits() {
        return Err(
            "FMI timestamp metadata must retain schema1 and one-second representation".into(),
        );
    }
    let policy = policy.ok_or("FMI thunder scoring requires an explicit, source-bound ETRS89 interpretation and timestamp representation allowance")?;
    policy.validate_shape()?;
    if !hash_valid(catalog_sha256) || policy.catalog_sha256 != catalog_sha256 {
        return Err("Lightning source interpretation belongs to different input bytes; review this catalog explicitly".into());
    }
    if policy.etrs89_as_wgs84_rationale.trim().is_empty()
        || policy.etrs89_as_wgs84_rationale.len() > 4000
    {
        return Err("Explain the explicit ETRS89-as-WGS84 approximation in 1..4000 bytes; no datum transformation is performed".into());
    }
    let allowance = policy.time_representation_allowance_s;
    if !allowance.is_finite() || !(metadata.time_resolution_s..=3600.0).contains(&allowance) {
        return Err("FMI timestamp representation allowance must be explicitly declared within 1..3600 seconds".into());
    }
    if !thunder_scale_s.is_finite() || thunder_scale_s < allowance {
        return Err("Thunder model residual scale is smaller than the declared FMI timestamp allowance; change it explicitly before scoring".into());
    }
    Ok(())
}

pub fn interpretation(
    catalog: &LightningCatalog,
    policy: Option<&LightningSourcePolicy>,
) -> Option<LightningInterpretationDiagnostic> {
    let metadata = catalog.provenance.source_metadata.as_ref()?;
    let policy = policy?;
    Some(LightningInterpretationDiagnostic {
        source_crs: metadata.horizontal_crs,
        source_time_resolution_s: metadata.time_resolution_s,
        timestamp_rounding: metadata.timestamp_rounding,
        declared_time_representation_allowance_s: policy.time_representation_allowance_s,
        etrs89_as_wgs84_rationale: policy.etrs89_as_wgs84_rationale.clone(),
        datum_transform_applied: false,
        source_clock_accuracy_calibrated: false,
    })
}

pub fn limitations(
    catalog: &LightningCatalog,
    policy: Option<&LightningSourcePolicy>,
) -> Vec<String> {
    let Some(metadata) = &catalog.provenance.source_metadata else {
        return Vec::new();
    };
    let mut result = vec![
        format!("FMI/NORDLIS source coordinates remain ETRS89 (EPSG:4258); timestamps retain {} s representation. Timestamp rounding and physical source-clock accuracy are unverified. Coverage is Northern European detected lightning, not complete lightning activity.", metadata.time_resolution_s),
        "FMI ellipse_major remains raw because documented units conflict (metres/kilometres); it is not used as a calibrated position error or score weight. Peak current and multiplicity do not supply acoustic amplitude or source height.".into(),
    ];
    if let Some(policy) = policy {
        result.push(format!("Analyst permits an ETRS89-as-WGS84 approximation for these source bytes: {}. No datum transformation is applied. Declared timestamp representation allowance: {} s, already bounded by the chosen thunder model residual scale; this is a heuristic allowance, not a calibrated Gaussian sigma. Observed pick uncertainty and shared recording clock remain separate.", policy.etrs89_as_wgs84_rationale, policy.time_representation_allowance_s));
    }
    result
}

pub fn source_notes(provenance: &LightningProvenance) -> Vec<String> {
    if provenance.source_metadata.is_none() {
        return Vec::new();
    }
    vec![format!("{} · {}", provenance.provider, provenance.license.as_deref().unwrap_or("License unavailable")),
        "FMI map positions are retained ETRS89 (EPSG:4258), shown approximately on the geographic map. No WGS84 datum transformation is applied. Source times have one-second representation, with unverified rounding and clock accuracy. Explicit source interpretation is required before scoring.".into(),
        "Raw ellipse_major has conflicting metre/kilometre documentation and is not drawn as an uncertainty radius. Acoustic strike height is unknown.".into()]
}
