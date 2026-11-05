//! Explicit, uncalibrated comparisons using one shared set of selected evidence.
//! Lower scores mean smaller normalized residuals, not a verified location.
//! Thunder uses a point-source, straight-line, still-air approximation. Real
//! lightning has an extended channel: the first sound may come from its nearest
//! portion (https://www.weather.gov/safety/lightning-science-thunder).

use crate::{
    physics::geodetic_to_ecef,
    thunder::{ThunderAnnotation, validate_annotation},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_IMPORT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_STRIKES: usize = 250_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightningAltitudeDatum {
    Wgs84Ellipsoid,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightningProvenance {
    pub provider: String,
    pub source: String,
    pub license: Option<String>,
    pub notes: Option<String>,
    #[serde(default)]
    pub altitude_datum: LightningAltitudeDatum,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_metadata: Option<crate::lightning_source::FmiSourceMetadata>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightningStrike {
    pub id: String,
    /// POSIX/Unix UTC seconds; not local time, GPS seconds or milliseconds.
    pub time_utc_s: f64,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub altitude_m: Option<f64>,
    /// Preserved without inferring units, polarity or probability.
    pub intensity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_record: Option<crate::lightning_source::FmiRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightningDocument {
    pub schema_version: u32,
    pub strikes: Vec<LightningStrike>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<LightningProvenance>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LightningCatalog {
    pub schema_version: u32,
    pub input_path: PathBuf,
    pub input_bytes: u64,
    pub input_sha256: String,
    pub provenance: LightningProvenance,
    pub strikes: Vec<LightningStrike>,
}

fn finite_positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}
fn id_ok(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512
}

pub(crate) fn validate_strikes(strikes: &[LightningStrike]) -> Result<(), String> {
    if strikes.is_empty() || strikes.len() > MAX_STRIKES {
        return Err("Lightning input needs 1..250000 strikes".into());
    }
    let mut ids = BTreeSet::new();
    for strike in strikes {
        if !id_ok(&strike.id)
            || !ids.insert(&strike.id)
            || !strike.time_utc_s.is_finite()
            || !(0.0..253_402_300_800.0).contains(&strike.time_utc_s)
            || !strike.latitude_deg.is_finite()
            || !(-90.0..=90.0).contains(&strike.latitude_deg)
            || !strike.longitude_deg.is_finite()
            || !(-180.0..=180.0).contains(&strike.longitude_deg)
            || strike
                .altitude_m
                .is_some_and(|h| !h.is_finite() || !(-1000.0..=100_000.0).contains(&h))
            || strike.intensity.is_some_and(|x| !x.is_finite())
        {
            return Err(format!(
                "Invalid or duplicate lightning strike {:?}; check UTC seconds, coordinates and optional values",
                strike.id
            ));
        }
    }
    Ok(())
}

// Strict CSV subset with RFC4180 quoted fields, escaped quotes and CRLF. A quoted
// newline is supported; stray quotes or bytes after a closing quote are errors.
fn csv_rows(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut closed = false;
    let mut at_start = true;
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                    closed = true;
                }
            } else {
                field.push(ch);
            }
            continue;
        }
        if ch == '"' && at_start {
            quoted = true;
            at_start = false;
            continue;
        }
        if ch == ',' || ch == '\n' || ch == '\r' {
            if ch == '\r' && chars.next() != Some('\n') {
                return Err("CSV requires LF or CRLF line endings".into());
            }
            row.push(std::mem::take(&mut field));
            closed = false;
            at_start = true;
            if ch != ',' {
                rows.push(std::mem::take(&mut row));
                if rows.len() > MAX_STRIKES + 1 {
                    return Err("CSV contains too many rows".into());
                }
            }
        } else {
            if closed || ch == '"' {
                return Err("Malformed CSV quoting".into());
            }
            field.push(ch);
            at_start = false;
        }
    }
    if quoted {
        return Err("Unterminated CSV quote".into());
    }
    if !row.is_empty() || !field.is_empty() || closed {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

fn parse_csv(text: &str) -> Result<Vec<LightningStrike>, String> {
    let rows = csv_rows(text)?;
    let header = rows.first().ok_or("Empty lightning CSV")?;
    let allowed = [
        "id",
        "time_utc_s",
        "latitude_deg",
        "longitude_deg",
        "altitude_m",
        "intensity",
    ];
    let mut columns = BTreeMap::new();
    for (i, name) in header.iter().enumerate() {
        if !allowed.contains(&name.as_str()) || columns.insert(name.as_str(), i).is_some() {
            return Err(format!(
                "Unknown or duplicate lightning CSV column {name:?}"
            ));
        }
    }
    for name in &allowed[..4] {
        if !columns.contains_key(name) {
            return Err(format!("Lightning CSV needs column {name}"));
        }
    }
    rows.iter()
        .skip(1)
        .enumerate()
        .map(|(i, row)| {
            if row.len() != header.len() {
                return Err(format!(
                    "Lightning CSV row {} has the wrong field count",
                    i + 2
                ));
            }
            let number = |name: &str| -> Result<f64, String> {
                row[columns[name]]
                    .trim()
                    .parse()
                    .map_err(|_| format!("Invalid {name} in CSV row {}", i + 2))
            };
            let optional = |name: &str| -> Result<Option<f64>, String> {
                match columns.get(name) {
                    Some(&index) if !row[index].trim().is_empty() => number(name).map(Some),
                    _ => Ok(None),
                }
            };
            Ok(LightningStrike {
                id: row[columns["id"]].clone(),
                time_utc_s: number("time_utc_s")?,
                latitude_deg: number("latitude_deg")?,
                longitude_deg: number("longitude_deg")?,
                altitude_m: optional("altitude_m")?,
                intensity: optional("intensity")?,
                source_record: None,
            })
        })
        .collect()
}

/// Native interchange format; this is not a parser for any provider's proprietary
/// export. Provenance is declared by the importer and preserved, not authenticated.
pub fn import_lightning(
    path: &Path,
    provenance: &LightningProvenance,
) -> Result<LightningCatalog, String> {
    let path = fs::canonicalize(path).map_err(|e| format!("Read lightning path: {e}"))?;
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .map_err(|e| format!("Open lightning input: {e}"))?
        .take(MAX_IMPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Read lightning input: {e}"))?;
    if bytes.len() as u64 > MAX_IMPORT_BYTES {
        return Err(format!(
            "Lightning import exceeds {}",
            crate::data_size::format_gib(64 * 1024 * 1024)
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "Lightning input must be UTF-8")?
        .trim_start_matches('\u{feff}');
    let document = match path
        .extension()
        .and_then(|x| x.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => {
            serde_json::from_str(text).map_err(|e| format!("Invalid lightning JSON: {e}"))?
        }
        Some("csv") => LightningDocument {
            schema_version: 1,
            strikes: parse_csv(text)?,
            provenance: None,
        },
        _ => return Err("Lightning input must use .json or .csv".into()),
    };
    validate_lightning_document(&document)?;
    let provenance = document.provenance.as_ref().unwrap_or(provenance);
    if !id_ok(&provenance.provider) || provenance.source.trim().is_empty() {
        return Err("Lightning import needs a provider and source description".into());
    }
    crate::lightning_source::validate_source(Some(provenance), &document.strikes)?;
    Ok(LightningCatalog {
        schema_version: 1,
        input_path: path,
        input_bytes: bytes.len() as u64,
        input_sha256: format!("{:x}", Sha256::digest(&bytes)),
        provenance: provenance.clone(),
        strikes: document.strikes,
    })
}

/// Validate normalized lightning without discarding its embedded source semantics.
pub fn validate_lightning_document(document: &LightningDocument) -> Result<(), String> {
    if document.schema_version != 1 {
        return Err("Unsupported lightning schema_version".into());
    }
    validate_strikes(&document.strikes)?;
    if let Some(provenance) = &document.provenance
        && (!id_ok(&provenance.provider) || provenance.source.trim().is_empty())
    {
        return Err("Lightning document needs a valid provider and source description".into());
    }
    crate::lightning_source::validate_source(document.provenance.as_ref(), &document.strikes)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrequencyObservation {
    pub id: String,
    pub event_id: String,
    pub time_s: f64,
    pub frequency_hz: f64,
    pub uncertainty_hz: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingObservation {
    pub id: String,
    pub event_id: String,
    pub time_s: f64,
    pub uncertainty_s: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssociationKind {
    UserReviewed,
    ExploratoryUserSelected,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThunderTimingObservation {
    pub annotation: ThunderAnnotation,
    pub strike_id: String,
    pub association: AssociationKind,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceSet {
    pub frequency: Vec<FrequencyObservation>,
    pub doppler_timing: Vec<TimingObservation>,
    pub thunder: Vec<ThunderTimingObservation>,
    pub lightning: Option<LightningCatalog>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TermSettings {
    pub enabled: bool,
    pub weight: f64,
    /// Model/measurement floor in Hz for frequency, seconds for timing. Combined
    /// in quadrature with each observation uncertainty, not estimated from fits.
    pub residual_scale: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoreConfig {
    pub frequency: TermSettings,
    pub doppler_timing: TermSettings,
    pub thunder_timing: TermSettings,
    pub sound_speed_mps: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atmosphere: Option<crate::atmosphere::AtmosphereProfile>,
    /// User-declared WGS84 ellipsoid height used if strike height/datum is absent.
    pub fallback_strike_ellipsoid_height_m: Option<f64>,
    pub allow_exploratory_associations: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightning_source_policy: Option<crate::lightning_source::LightningSourcePolicy>,
}

impl Default for ScoreConfig {
    fn default() -> Self {
        Self {
            frequency: TermSettings {
                enabled: true,
                weight: 1.0,
                residual_scale: 1.0,
            },
            doppler_timing: TermSettings {
                enabled: false,
                weight: 1.0,
                residual_scale: 1.0,
            },
            thunder_timing: TermSettings {
                enabled: false,
                weight: 1.0,
                residual_scale: 1.0,
            },
            sound_speed_mps: 343.0,
            atmosphere: None,
            fallback_strike_ellipsoid_height_m: None,
            allow_exploratory_associations: false,
            lightning_source_policy: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObserverLocation {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub ellipsoid_height_m: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrequencyPrediction {
    pub observation_id: String,
    pub time_s: f64,
    pub predicted_hz: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingPrediction {
    pub observation_id: String,
    pub predicted_arrival_utc_s: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateEvidence {
    pub id: String,
    pub observer: ObserverLocation,
    pub recording_start_utc_s: f64,
    /// Caller-supplied model predictions at the exact selected observation times.
    /// No frequency or clock fitting is silently performed by this comparison.
    pub frequency: Vec<FrequencyPrediction>,
    pub doppler_timing: Vec<TimingPrediction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTerm {
    DopplerFrequency,
    DopplerTiming,
    ThunderTiming,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThunderPrediction {
    pub strike_id: String,
    pub association: AssociationKind,
    pub slant_distance_m: f64,
    pub propagation_s: f64,
    pub strike_height_assumed: bool,
    pub strike_ellipsoid_height_m: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_interpretation: Option<crate::lightning_source::LightningInterpretationDiagnostic>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResidualDiagnostic {
    pub observation_id: String,
    pub event_id: String,
    pub observed: f64,
    pub predicted: f64,
    pub unit: String,
    pub residual: f64,
    pub observation_uncertainty: f64,
    pub effective_scale: f64,
    pub normalized_residual: f64,
    pub squared_normalized_residual: f64,
    pub thunder_prediction: Option<ThunderPrediction>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TermContribution {
    pub term: EvidenceTerm,
    pub weight: f64,
    pub residual_scale: f64,
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub residuals: Vec<ResidualDiagnostic>,
    /// Average samples within each event, then average events equally.
    pub event_mean_squared_residuals: BTreeMap<String, f64>,
    pub mean_normalized_squared_residual: Option<f64>,
    /// term mean * weight / sum of all selected term weights.
    pub weighted_contribution: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateScore {
    pub candidate: CandidateEvidence,
    pub rank: Option<usize>,
    pub score: Option<f64>,
    pub terms: Vec<TermContribution>,
    pub unavailable_reasons: Vec<String>,
    pub location_verified: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoreReport {
    pub schema_version: u32,
    pub config: ScoreConfig,
    pub selected_terms: Vec<EvidenceTerm>,
    pub evidence_sha256: String,
    pub ranked_candidate_ids: Vec<String>,
    pub candidates: Vec<CandidateScore>,
    pub calibration: String,
    pub location_verified: bool,
    pub limitations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<crate::modality_diagnostics::ModalityDiagnostics>,
}

fn selected(config: &ScoreConfig) -> Vec<(EvidenceTerm, &TermSettings)> {
    [
        (EvidenceTerm::DopplerFrequency, &config.frequency),
        (EvidenceTerm::DopplerTiming, &config.doppler_timing),
        (EvidenceTerm::ThunderTiming, &config.thunder_timing),
    ]
    .into_iter()
    .filter(|(_, s)| s.enabled)
    .collect()
}

fn validate_evidence(evidence: &EvidenceSet, config: &ScoreConfig) -> Result<(), String> {
    if let Some(policy) = &config.lightning_source_policy {
        policy.validate_shape()?;
    }
    if let Some(profile) = &config.atmosphere {
        profile.validate()?;
    }
    let terms = selected(config);
    if terms.is_empty() {
        return Err("Select at least one evidence term".into());
    }
    for (_, s) in &terms {
        if !finite_positive(s.weight) || !finite_positive(s.residual_scale) {
            return Err("Selected terms need finite positive weights and residual scales".into());
        }
    }
    if !terms.iter().map(|(_, s)| s.weight).sum::<f64>().is_finite() {
        return Err("Selected weights overflow".into());
    }
    if config.frequency.enabled {
        if evidence.frequency.is_empty() {
            return Err("Selected frequency term has no observations".into());
        }
        let mut ids = BTreeSet::new();
        for o in &evidence.frequency {
            if !id_ok(&o.id)
                || !id_ok(&o.event_id)
                || !ids.insert(&o.id)
                || !o.time_s.is_finite()
                || o.time_s < 0.0
                || !finite_positive(o.frequency_hz)
                || !finite_positive(o.uncertainty_hz)
            {
                return Err("Invalid or duplicate selected frequency observation".into());
            }
        }
    }
    if config.doppler_timing.enabled {
        if evidence.doppler_timing.is_empty() {
            return Err("Selected Doppler timing term has no observations".into());
        }
        let mut ids = BTreeSet::new();
        for o in &evidence.doppler_timing {
            if !id_ok(&o.id)
                || !id_ok(&o.event_id)
                || !ids.insert(&o.id)
                || !o.time_s.is_finite()
                || o.time_s < 0.0
                || !finite_positive(o.uncertainty_s)
            {
                return Err("Invalid or duplicate selected Doppler timing observation".into());
            }
        }
    }
    if config.thunder_timing.enabled {
        if !finite_positive(config.sound_speed_mps)
            || !(250.0..=400.0).contains(&config.sound_speed_mps)
        {
            return Err("Thunder sound speed must be within the declared atmospheric model bounds 250..400 m/s".into());
        }
        if config
            .fallback_strike_ellipsoid_height_m
            .is_some_and(|h| !h.is_finite() || !(-1000.0..=100_000.0).contains(&h))
        {
            return Err("Invalid fallback strike ellipsoid height".into());
        }
        if evidence.thunder.is_empty() {
            return Err("Selected thunder timing term has no reviewed observations".into());
        }
        let catalog = evidence
            .lightning
            .as_ref()
            .ok_or("Selected thunder timing needs a lightning catalog")?;
        if catalog.schema_version != 1
            || !id_ok(&catalog.provenance.provider)
            || catalog.provenance.source.trim().is_empty()
        {
            return Err("Invalid lightning catalog provenance".into());
        }
        crate::lightning_source::validate_policy(
            catalog,
            config.lightning_source_policy.as_ref(),
            config.thunder_timing.residual_scale,
        )?;
        let mut ids = BTreeSet::new();
        let strikes: BTreeSet<_> = catalog.strikes.iter().map(|s| s.id.as_str()).collect();
        for o in &evidence.thunder {
            validate_annotation(&o.annotation)?;
            if !o.annotation.reviewed_as_thunder
                || !ids.insert(&o.annotation.id)
                || !strikes.contains(o.strike_id.as_str())
            {
                return Err("Thunder scoring needs unique reviewed annotations with an explicit catalog strike association".into());
            }
            if o.association == AssociationKind::ExploratoryUserSelected
                && !config.allow_exploratory_associations
            {
                return Err("Exploratory lightning association needs explicit opt-in".into());
            }
        }
    }
    Ok(())
}

fn residual(
    id: &str,
    event: &str,
    observed: f64,
    predicted: f64,
    uncertainty: f64,
    scale: f64,
    unit: &str,
) -> Result<ResidualDiagnostic, String> {
    let effective_scale = uncertainty.hypot(scale);
    let residual = predicted - observed;
    let normalized = residual / effective_scale;
    let squared = normalized * normalized;
    if !predicted.is_finite()
        || !effective_scale.is_finite()
        || !normalized.is_finite()
        || !squared.is_finite()
    {
        return Err("Residual calculation overflowed or prediction is nonfinite".into());
    }
    Ok(ResidualDiagnostic {
        observation_id: id.into(),
        event_id: event.into(),
        observed,
        predicted,
        unit: unit.into(),
        residual,
        observation_uncertainty: uncertainty,
        effective_scale,
        normalized_residual: normalized,
        squared_normalized_residual: squared,
        thunder_prediction: None,
    })
}

fn term_residuals(
    term: EvidenceTerm,
    settings: &TermSettings,
    evidence: &EvidenceSet,
    config: &ScoreConfig,
    candidate: &CandidateEvidence,
    atmosphere: Option<&crate::atmosphere::PreparedAtmosphere<'_>>,
) -> Result<Vec<ResidualDiagnostic>, String> {
    match term {
        EvidenceTerm::DopplerFrequency => {
            let mut predictions = BTreeMap::new();
            for p in &candidate.frequency {
                if predictions.insert(p.observation_id.as_str(), p).is_some() {
                    return Err("Duplicate frequency prediction".into());
                }
            }
            if predictions.len() != evidence.frequency.len() {
                return Err(
                    "Frequency predictions do not cover exactly the selected observations".into(),
                );
            }
            evidence.frequency.iter().map(|o| {
                let p = predictions.get(o.id.as_str()).ok_or_else(||format!("Missing frequency prediction {}",o.id))?;
                if !p.time_s.is_finite() || (p.time_s-o.time_s).abs()>1e-7 || !finite_positive(p.predicted_hz) { return Err(format!("Frequency prediction {} has a different observation time or invalid frequency",o.id)); }
                residual(&o.id,&o.event_id,o.frequency_hz,p.predicted_hz,o.uncertainty_hz,settings.residual_scale,"Hz")
            }).collect()
        }
        EvidenceTerm::DopplerTiming => {
            let mut predictions = BTreeMap::new();
            for p in &candidate.doppler_timing {
                if predictions.insert(p.observation_id.as_str(), p).is_some() {
                    return Err("Duplicate timing prediction".into());
                }
            }
            if predictions.len() != evidence.doppler_timing.len() {
                return Err(
                    "Timing predictions do not cover exactly the selected observations".into(),
                );
            }
            evidence
                .doppler_timing
                .iter()
                .map(|o| {
                    let p = predictions
                        .get(o.id.as_str())
                        .ok_or_else(|| format!("Missing timing prediction {}", o.id))?;
                    residual(
                        &o.id,
                        &o.event_id,
                        o.time_s,
                        p.predicted_arrival_utc_s - candidate.recording_start_utc_s,
                        o.uncertainty_s,
                        settings.residual_scale,
                        "s",
                    )
                })
                .collect()
        }
        EvidenceTerm::ThunderTiming => {
            let catalog = evidence.lightning.as_ref().unwrap();
            let strikes: BTreeMap<_, _> =
                catalog.strikes.iter().map(|s| (s.id.as_str(), s)).collect();
            let observer = geodetic_to_ecef(
                candidate.observer.latitude_deg,
                candidate.observer.longitude_deg,
                candidate.observer.ellipsoid_height_m,
            )?;
            evidence.thunder.iter().map(|o| {
                let strike = strikes[o.strike_id.as_str()];
                let known_height = if catalog.provenance.altitude_datum == LightningAltitudeDatum::Wgs84Ellipsoid { strike.altitude_m } else { None };
                let height = known_height.or(config.fallback_strike_ellipsoid_height_m).ok_or_else(||format!("Lightning strike {} needs a known ellipsoid height or an explicit fallback assumption",strike.id))?;
                let xyz = geodetic_to_ecef(strike.latitude_deg,strike.longitude_deg,height)?;
                let distance = (xyz[0]-observer[0]).hypot(xyz[1]-observer[1]).hypot(xyz[2]-observer[2]);
                let propagation = if let Some(profile) = atmosphere {
                    profile.travel_time(xyz, observer, strike.time_utc_s)?.travel_time_s
                } else {
                    distance/config.sound_speed_mps
                };
                // Subtract the epoch before adding travel time, avoiding loss of
                // precision when a subsecond residual is compared at Unix epochs.
                let predicted = (strike.time_utc_s-candidate.recording_start_utc_s)+propagation;
                let mut r = residual(&o.annotation.id,&o.annotation.id,o.annotation.arrival_s,predicted,o.annotation.uncertainty_s,settings.residual_scale,"s")?;
                r.thunder_prediction = Some(ThunderPrediction {strike_id:strike.id.clone(),association:o.association,slant_distance_m:distance,propagation_s:propagation,strike_height_assumed:known_height.is_none(),strike_ellipsoid_height_m:height,source_interpretation:crate::lightning_source::interpretation(catalog, config.lightning_source_policy.as_ref())});
                Ok(r)
            }).collect()
        }
    }
}

/// Every rankable candidate uses exactly the same selected observations, terms,
/// scales, weights and strike associations. Missing evidence makes it unrankable;
/// the denominator is never reduced to reward incomplete coverage.
pub fn score_candidates(
    evidence: &EvidenceSet,
    config: &ScoreConfig,
    candidates: &[CandidateEvidence],
) -> Result<ScoreReport, String> {
    validate_evidence(evidence, config)?;
    let atmosphere = config
        .atmosphere
        .as_ref()
        .map(crate::atmosphere::PreparedAtmosphere::new)
        .transpose()?;
    if candidates.is_empty() || candidates.len() > 100_000 {
        return Err("Compare 1..100000 candidates".into());
    }
    let selected = selected(config);
    let total_weight: f64 = selected.iter().map(|(_, s)| s.weight).sum();
    let mut ids = BTreeSet::new();
    let mut scores = Vec::new();
    for candidate in candidates {
        if !id_ok(&candidate.id) || !ids.insert(&candidate.id) {
            return Err("Candidate IDs must be nonempty and unique".into());
        }
        let candidate_valid = geodetic_to_ecef(
            candidate.observer.latitude_deg,
            candidate.observer.longitude_deg,
            candidate.observer.ellipsoid_height_m,
        )
        .and_then(|_| {
            if candidate.recording_start_utc_s.is_finite()
                && (0.0..253_402_300_800.0).contains(&candidate.recording_start_utc_s)
            {
                Ok(())
            } else {
                Err("Candidate recording start must use finite Unix UTC seconds".into())
            }
        });
        let mut terms = Vec::new();
        let mut unavailable = Vec::new();
        for &(term, settings) in &selected {
            let calculated = candidate_valid
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|_| {
                    term_residuals(
                        term,
                        settings,
                        evidence,
                        config,
                        candidate,
                        atmosphere.as_ref(),
                    )
                });
            let mut contribution = TermContribution {
                term,
                weight: settings.weight,
                residual_scale: settings.residual_scale,
                available: false,
                unavailable_reason: None,
                residuals: Vec::new(),
                event_mean_squared_residuals: BTreeMap::new(),
                mean_normalized_squared_residual: None,
                weighted_contribution: None,
            };
            match calculated {
                Ok(residuals) => {
                    let mut groups: BTreeMap<String, Vec<f64>> = BTreeMap::new();
                    for r in &residuals {
                        groups
                            .entry(r.event_id.clone())
                            .or_default()
                            .push(r.squared_normalized_residual);
                    }
                    for (event, values) in groups {
                        contribution
                            .event_mean_squared_residuals
                            .insert(event, values.iter().map(|x| x / values.len() as f64).sum());
                    }
                    let means = &contribution.event_mean_squared_residuals;
                    let mean = means.values().map(|x| x / means.len() as f64).sum::<f64>();
                    let weighted = mean * (settings.weight / total_weight);
                    if !mean.is_finite() || !weighted.is_finite() {
                        contribution.unavailable_reason =
                            Some("Term aggregation overflowed".into());
                        unavailable.push(format!("{term:?}: term aggregation overflowed"));
                    } else {
                        contribution.available = true;
                        contribution.mean_normalized_squared_residual = Some(mean);
                        contribution.weighted_contribution = Some(weighted);
                        contribution.residuals = residuals;
                    }
                }
                Err(reason) => {
                    unavailable.push(format!("{term:?}: {reason}"));
                    contribution.unavailable_reason = Some(reason);
                }
            }
            terms.push(contribution);
        }
        let score = if unavailable.is_empty() {
            Some(
                terms
                    .iter()
                    .filter_map(|t| t.weighted_contribution)
                    .sum::<f64>(),
            )
        } else {
            None
        };
        scores.push(CandidateScore {
            candidate: candidate.clone(),
            rank: None,
            score,
            terms,
            unavailable_reasons: unavailable,
            location_verified: false,
        });
    }
    let mut ranking: Vec<usize> = (0..scores.len())
        .filter(|&i| scores[i].score.is_some())
        .collect();
    ranking.sort_by(|&a, &b| {
        scores[a]
            .score
            .unwrap()
            .total_cmp(&scores[b].score.unwrap())
            .then_with(|| scores[a].candidate.id.cmp(&scores[b].candidate.id))
    });
    for (rank, &i) in ranking.iter().enumerate() {
        scores[i].rank = Some(rank + 1);
    }
    let ranked_candidate_ids = ranking
        .iter()
        .map(|&i| scores[i].candidate.id.clone())
        .collect();
    let evidence_bytes =
        serde_json::to_vec(evidence).map_err(|e| format!("Serialize evidence identity: {e}"))?;
    let mut report = ScoreReport {schema_version:1,config:config.clone(),selected_terms:selected.iter().map(|(t,_)|*t).collect(),evidence_sha256:format!("{:x}",Sha256::digest(evidence_bytes)),ranked_candidate_ids,candidates:scores,calibration:"uncalibrated normalized residual score; not a probability or confidence interval".into(),location_verified:false,modalities:None,limitations:vec![
        "Predictions and event/strike associations are explicit user-selected hypotheses. This comparison neither authenticates their origin nor establishes global uniqueness.".into(),
        "Each selected term averages samples within events then events equally; the final score is the selected-weight mean. Correlated terms are not independent probabilistic evidence.".into(),
        if config.atmosphere.is_some() {
            "Experimental atmospheric profile: altitude-dependent temperature, pressure, humidity and directional wind along a straight path. Thunder uses a point emitter; channel extent, terrain obstruction, refraction and time-varying weather are not modeled. No constant-speed fallback is used outside profile coverage.".into()
        } else {
            "Thunder timing assumes one point emitter, straight-line slant distance, still air and constant sound speed. Channel extent, terrain, wind and refraction are not modeled; expand uncertainty or exclude the term when unjustified.".into()
        },
        "Lightning intensity is preserved only as source data and is not used as acoustic amplitude or likelihood. No unknown association is silently optimized.".into(),
    ]};
    if config.thunder_timing.enabled
        && let Some(catalog) = &evidence.lightning
    {
        report
            .limitations
            .extend(crate::lightning_source::limitations(
                catalog,
                config.lightning_source_policy.as_ref(),
            ));
    }
    match crate::modality_diagnostics::summarize(
        &report,
        None,
        &crate::modality_diagnostics::DiagnosticOptions::default(),
    ) {
        Ok(diagnostics) => report.modalities = Some(diagnostics),
        Err(error) => report
            .limitations
            .push(format!("Modality diagnostics unavailable: {error}")),
    }
    Ok(report)
}
