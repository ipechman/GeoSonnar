//! User-owned evidence annotations. Detection never selects scoring evidence.
use crate::{casework, detailed_data::sha256, event_review::ReviewSession};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_DOCUMENT_BYTES: usize = 64 * 1024 * 1024;
static REVISION_SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Doppler,
    Thunder,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Pending,
    Confirmed,
    Rejected,
}
/// Analyst assessment of the acoustic source class, separate from measurement
/// review and from any particular aircraft or lightning association.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum AcousticSourceAssessment {
    #[default]
    Unassessed,
    Compatible,
    UncertainHypothesis {
        rationale: String,
    },
    Incompatible,
}
impl AcousticSourceAssessment {
    pub fn is_unassessed(&self) -> bool {
        matches!(self, Self::Unassessed)
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Unassessed => "Unassessed",
            Self::Compatible => "Compatible source class",
            Self::UncertainHypothesis { .. } => "Uncertain source hypothesis",
            Self::Incompatible => "Incompatible source class",
        }
    }
    pub fn rationale(&self) -> Option<&str> {
        match self {
            Self::UncertainHypothesis { rationale } => Some(rationale),
            _ => None,
        }
    }
    pub fn scoring_block_reason(&self) -> Option<String> {
        match self {
            Self::Unassessed => Some(
                "acoustic source is unassessed; assess the plausible sound class before scoring"
                    .into(),
            ),
            Self::Incompatible => Some(
                "acoustic source is assessed as incompatible with its selected sound class".into(),
            ),
            Self::UncertainHypothesis { rationale } if rationale.trim().is_empty() => Some(
                "uncertain acoustic source hypothesis needs an explicit rationale before scoring"
                    .into(),
            ),
            Self::Compatible | Self::UncertainHypothesis { .. } => None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TracePoint {
    pub time_s: f64,
    pub frequency_hz: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceBinding {
    pub path: PathBuf,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceEvent {
    pub id: String,
    pub label: String,
    pub kind: EventKind,
    pub origin: String,
    pub review: ReviewState,
    pub selected_for_scoring: bool,
    #[serde(
        default,
        skip_serializing_if = "AcousticSourceAssessment::is_unassessed"
    )]
    pub acoustic_source: AcousticSourceAssessment,
    pub start_s: f64,
    pub end_s: f64,
    pub arrival_s: f64,
    pub points: Vec<TracePoint>,
    pub uncertainty_s: f64,
    pub frequency_sigma_hz: f64,
    pub flight_id: String,
    pub emitted_frequency_hz: f64,
    pub strike_id: String,
    pub source: Option<SourceBinding>,
    pub diagnostics: serde_json::Value,
}
impl EvidenceEvent {
    pub fn manual(id: String, kind: EventKind, start_s: f64, end_s: f64) -> Self {
        Self {
            label: match kind {
                EventKind::Doppler => "Manual Doppler",
                EventKind::Thunder => "Manual thunder",
            }
            .into(),
            id,
            kind,
            origin: "manual annotation".into(),
            review: ReviewState::Pending,
            selected_for_scoring: false,
            acoustic_source: AcousticSourceAssessment::Unassessed,
            start_s,
            end_s,
            arrival_s: start_s,
            points: Vec::new(),
            uncertainty_s: 2.0,
            frequency_sigma_hz: 1.0,
            flight_id: String::new(),
            emitted_frequency_hz: 0.0,
            strike_id: String::new(),
            source: None,
            diagnostics: serde_json::Value::Null,
        }
    }
    pub fn acoustic_source_block_reason(&self) -> Option<String> {
        self.acoustic_source
            .scoring_block_reason()
            .map(|reason| format!("{} ({}): {reason}", self.id, self.label))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceMix {
    pub doppler_frequency: bool,
    pub doppler_timing: bool,
    pub thunder_timing: bool,
    pub frequency_weight: f64,
    pub doppler_timing_weight: f64,
    pub thunder_weight: f64,
    pub frequency_scale_hz: f64,
    pub doppler_timing_scale_s: f64,
    pub thunder_scale_s: f64,
    /// Explicit source-bound horizontal/time interpretation; no automatic datum
    /// conversion or residual-scale adjustment is performed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightning_source_policy: Option<crate::lightning_source::LightningSourcePolicy>,
}
impl Default for EvidenceMix {
    fn default() -> Self {
        Self {
            doppler_frequency: false,
            doppler_timing: false,
            thunder_timing: false,
            frequency_weight: 1.0,
            doppler_timing_weight: 1.0,
            thunder_weight: 1.0,
            frequency_scale_hz: 1.0,
            doppler_timing_scale_s: 1.0,
            thunder_scale_s: 1.0,
            lightning_source_policy: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MapSelection {
    pub bounds: [f64; 4],
    pub grid_spacing_m: f64,
    pub receiver_height_m: f64,
    pub clock_adjustment_s: f64,
}
impl Default for MapSelection {
    fn default() -> Self {
        Self {
            bounds: [-60.0, -180.0, 80.0, 180.0],
            grid_spacing_m: 1000.0,
            receiver_height_m: 0.0,
            clock_adjustment_s: 0.0,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DatasetOptions {
    pub source_sha256: String,
    pub time_origin_utc_s: Option<f64>,
    pub lightning_altitude_wgs84: bool,
}
/// Explicit selection in original audio time, independent of the plot viewport.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeSelection {
    pub start_s: f64,
    pub end_s: f64,
    pub fft_size: usize,
    pub hop_s: f64,
    pub band_hz: [f64; 2],
    pub reviewed_selection_note: String,
}
impl EnvelopeSelection {
    pub fn validate(&self, duration_s: f64) -> Result<(), String> {
        if self.end_s > duration_s
            || !(0.05..=5.0).contains(&self.hop_s)
            || self.band_hz.iter().any(|v| !v.is_finite())
            || self.band_hz[0] < 20.0
            || self.band_hz[1] > 250.0
            || self.band_hz[0] >= self.band_hz[1]
            || self.reviewed_selection_note.len() > 4000
        {
            return Err("Envelope selection needs a bounded interval, 20–250 Hz band and a note of at most 4000 bytes".into());
        }
        crate::review_media::WindowRequest {
            input: PathBuf::new(),
            ffmpeg: PathBuf::new(),
            ffprobe: PathBuf::new(),
            channel: 0,
            start_s: self.start_s,
            end_s: self.end_s,
            fft_size: self.fft_size,
            hop_s: self.hop_s,
            max_frequency_hz: 250.0,
        }
        .validate()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeReview {
    pub selection: EnvelopeSelection,
    pub observation: Option<crate::thunder_envelope::ObservedEnvelope>,
}
impl EnvelopeReview {
    pub fn validate(&self, document: &ReviewWorkspace) -> Result<(), String> {
        self.selection.validate(document.duration_s)?;
        if let Some(observation) = &self.observation {
            let samples = &observation.envelope.samples;
            let selection = &self.selection;
            let cadence = (selection.hop_s * 4000.0).round().max(1.0) / 4000.0;
            let first =
                ((selection.start_s * 4000.0).ceil() + selection.fft_size as f64 / 2.0) / 4000.0;
            let spacing = 4000.0 / selection.fft_size as f64;
            let sample_count = (selection.end_s * 4000.0).floor() as usize
                - (selection.start_s * 4000.0).ceil() as usize;
            let expected_frames =
                1 + (sample_count - selection.fft_size) / (cadence * 4000.0).round() as usize;
            if observation.recording_sha256 != document.recording_sha256
                || observation.channel != document.channel
                || observation.reviewed_selection_note != selection.reviewed_selection_note
                || observation.reviewed_selection_note.trim().is_empty()
                || !(2..=crate::thunder_envelope::MAX_SAMPLES).contains(&samples.len())
                || samples.len() != expected_frames
                || observation.envelope.method.trim().is_empty()
                || observation.envelope.method.len() > 4000
                || observation.envelope.band_low_hz
                    != (selection.band_hz[0] / spacing).ceil() * spacing
                || observation.envelope.band_high_hz
                    != (selection.band_hz[1] / spacing).floor() * spacing
                || samples.iter().enumerate().any(|(i, p)| {
                    !p.time_s.is_finite()
                        || !p.rms.is_finite()
                        || p.rms < 0.0
                        || (p.time_s - (first + i as f64 * cadence)).abs() > 1e-8
                        || p.time_s + selection.fft_size as f64 / 8000.0 > selection.end_s + 1e-8
                })
                || !samples.iter().any(|p| p.rms > 0.0)
            {
                return Err("Measured envelope no longer matches the reviewed recording, channel or selection; measure it again explicitly".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReviewWorkspace {
    pub schema_version: u32,
    pub case_id: String,
    pub recording_sha256: String,
    pub channel: usize,
    pub duration_s: f64,
    pub recording_start_utc: String,
    pub sound_speed_mps: f64,
    /// Optional immutable local weather profile. Absent means fixed sound speed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atmosphere: Option<crate::atmosphere::AtmosphereProfile>,
    /// Analyst-owned draft; complete search constraints are checked before running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regional_plan: Option<crate::regional_search::RegionalPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regional_comparison: Option<crate::regional_search::ComparisonOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope_review: Option<EnvelopeReview>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_terrain: Option<crate::receiver_heights::ReceiverTerrain>,
    pub events: Vec<EvidenceEvent>,
    pub notes: String,
    pub mix: EvidenceMix,
    pub map: MapSelection,
    pub datasets: Vec<SourceBinding>,
    #[serde(default)]
    pub dataset_options: Vec<DatasetOptions>,
    #[serde(default)]
    pub strike_fallback_height_m: Option<f64>,
}
impl ReviewWorkspace {
    pub fn new(case: &casework::SavedCase, channel: usize) -> Self {
        Self {
            schema_version: 1,
            case_id: case.case_id.clone(),
            recording_sha256: case.recording.sha256.clone(),
            channel,
            duration_s: case.recording.media.duration_s,
            recording_start_utc: String::new(),
            sound_speed_mps: 343.0,
            atmosphere: None,
            regional_plan: None,
            regional_comparison: None,
            envelope_review: None,
            receiver_terrain: None,
            events: Vec::new(),
            notes: String::new(),
            mix: EvidenceMix::default(),
            map: MapSelection::default(),
            datasets: Vec::new(),
            dataset_options: Vec::new(),
            strike_fallback_height_m: None,
        }
    }
    /// Register a worker-verified import and its interpretation. Content hashes
    /// are rechecked by `verify`; this helper does no large UI-thread file read.
    pub fn register_dataset(
        &mut self,
        binding: SourceBinding,
        origin: Option<f64>,
        lightning_ellipsoid: bool,
    ) -> Result<(), String> {
        validate_binding(&binding)?;
        if !binding.path.is_absolute() {
            return Err("Dataset registration requires an absolute imported path".into());
        }
        let binding = SourceBinding {
            path: binding.path.canonicalize().map_err(|e| e.to_string())?,
            sha256: binding.sha256,
        };
        if !fs::metadata(&binding.path)
            .map_err(|e| e.to_string())?
            .is_file()
        {
            return Err("Dataset source must be a regular file".into());
        }
        let mut datasets = self.datasets.clone();
        datasets.retain(|old| {
            path_key(&old.path) != path_key(&binding.path) && old.sha256 != binding.sha256
        });
        datasets.push(binding.clone());
        let mut options = self.dataset_options.clone();
        options.retain(|old| {
            old.source_sha256 != binding.sha256
                && datasets.iter().any(|d| d.sha256 == old.source_sha256)
        });
        options.push(DatasetOptions {
            source_sha256: binding.sha256,
            time_origin_utc_s: origin,
            lightning_altitude_wgs84: lightning_ellipsoid,
        });
        validate_datasets(&datasets, &options)?;
        self.datasets = datasets;
        self.dataset_options = options;
        Ok(())
    }
    pub fn import_discovery(&mut self, session: &ReviewSession) -> Result<usize, String> {
        let o = session.overview();
        if o.case_id != self.case_id
            || o.recording_sha256 != self.recording_sha256
            || o.selected_channel_index != self.channel
        {
            return Err("Discovery belongs to another case or channel".into());
        }
        let mut added = 0;
        for group in session.groups() {
            for ridge in &group.ridges {
                let id = format!("d-{}-{}", &o.discovery_sha256[..12], ridge.event_id);
                if self.events.iter().any(|e| e.id == id) {
                    continue;
                }
                let mut e =
                    EvidenceEvent::manual(id, EventKind::Doppler, ridge.start_s, ridge.end_s);
                e.label = format!("{} / {}", group.harmonic_group_id, ridge.event_id);
                e.origin = "automatic moving-tone proposal".into();
                e.points = ridge
                    .points
                    .iter()
                    .map(|p| TracePoint {
                        time_s: p.time_s,
                        frequency_hz: p.frequency_hz,
                    })
                    .collect();
                e.arrival_s = (ridge.start_s + ridge.end_s) / 2.0;
                e.frequency_sigma_hz = ridge.quality["frequency_sigma_hz"]
                    .as_f64()
                    .filter(|x| x.is_finite() && *x > 0.0)
                    .unwrap_or(1.0);
                e.source = Some(SourceBinding {
                    path: o.discovery_path.clone(),
                    sha256: o.discovery_sha256.clone(),
                });
                e.diagnostics = serde_json::json!({"quality":ridge.quality,"harmonic_group":group.harmonic_group_id,"max_gap_s":o.max_gap_s,"arrival_requires_manual_review":true});
                self.events.push(e);
                added += 1;
            }
        }
        Ok(added)
    }
    /// Active observations are retained even if source assessment blocks them.
    /// Call the separate gate rather than silently dropping unassessed events.
    pub fn active_scoring_events(&self) -> impl Iterator<Item = &EvidenceEvent> {
        self.events.iter().filter(move |event| {
            event.review == ReviewState::Confirmed
                && event.selected_for_scoring
                && match event.kind {
                    EventKind::Doppler => self.mix.doppler_frequency || self.mix.doppler_timing,
                    EventKind::Thunder => self.mix.thunder_timing,
                }
        })
    }
    pub fn validate_acoustic_sources_for_scoring(&self) -> Result<(), String> {
        let blocked = self
            .active_scoring_events()
            .filter_map(EvidenceEvent::acoustic_source_block_reason)
            .collect::<Vec<_>>();
        if blocked.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Acoustic source assessment blocks scoring:\n{}",
                blocked.join("\n")
            ))
        }
    }
    pub fn acoustic_source_warnings(&self) -> Vec<String> {
        self.active_scoring_events().filter_map(|event| {
            event.acoustic_source.rationale().map(|rationale| {
                format!("UNCERTAIN ACOUSTIC SOURCE — {} ({}): {}. This is an explicitly permitted source-class hypothesis, not an identified aircraft, lightning strike or verified location.", event.id, event.label, rationale.trim())
            })
        }).collect()
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1
            || self.case_id.is_empty()
            || !valid_hash(&self.recording_sha256)
            || !self.duration_s.is_finite()
            || self.duration_s <= 0.0
            || self.events.len() > 10_000
        {
            return Err("Invalid review workspace identity or size".into());
        }
        validate_datasets(&self.datasets, &self.dataset_options)?;
        if self
            .strike_fallback_height_m
            .is_some_and(|h| !h.is_finite() || !(-1000.0..=100_000.0).contains(&h))
        {
            return Err("Invalid fallback strike ellipsoid height".into());
        }
        if !(250.0..=400.0).contains(&self.sound_speed_mps) {
            return Err("Sound speed must be 250–400 m/s".into());
        }
        if let Some(profile) = &self.atmosphere {
            profile.validate()?;
        }
        if let Some(terrain) = &self.receiver_terrain {
            terrain.validate()?;
        }
        if let Some(plan) = &self.regional_plan {
            plan.validate_draft()?;
        }
        if let Some(options) = &self.regional_comparison {
            options.validate_draft()?;
        }
        if let Some(review) = &self.envelope_review {
            review.validate(self)?;
        }
        let [s, w, n, e] = self.map.bounds;
        if !self.map.bounds.iter().all(|v| v.is_finite())
            || s >= n
            || w >= e
            || s < -85.0
            || n > 85.0
            || w < -180.0
            || e > 180.0
            || !self.map.grid_spacing_m.is_finite()
            || self.map.grid_spacing_m < 10.0
            || !self.map.receiver_height_m.is_finite()
            || !self.map.clock_adjustment_s.is_finite()
        {
            return Err("Invalid map bounds, grid spacing, height or clock adjustment".into());
        }
        if [
            self.mix.frequency_weight,
            self.mix.doppler_timing_weight,
            self.mix.thunder_weight,
            self.mix.frequency_scale_hz,
            self.mix.doppler_timing_scale_s,
            self.mix.thunder_scale_s,
        ]
        .iter()
        .any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err("Evidence weights and scales must be positive".into());
        }
        if !self.recording_start_utc.trim().is_empty() {
            parse_utc(&self.recording_start_utc)?;
        }
        if let Some(policy) = &self.mix.lightning_source_policy {
            policy.validate_shape()?;
        }
        let mut ids = HashSet::new();
        let mut total = 0usize;
        for e in &self.events {
            total += e.points.len();
            if e.acoustic_source
                .rationale()
                .is_some_and(|rationale| rationale.len() > 4000)
            {
                return Err(format!(
                    "Acoustic source rationale for {} exceeds 4000 bytes",
                    e.id
                ));
            }
            if !safe_event_id(&e.id) || !ids.insert(e.id.to_ascii_lowercase()) {
                return Err(
                    "Event IDs must be unique Windows-safe names, excluding reserved export names"
                        .into(),
                );
            }
            if let Some(binding) = &e.source {
                validate_binding(binding)?;
            }
            if ![
                e.start_s,
                e.end_s,
                e.arrival_s,
                e.uncertainty_s,
                e.frequency_sigma_hz,
                e.emitted_frequency_hz,
            ]
            .iter()
            .all(|x| x.is_finite())
                || e.start_s < 0.0
                || e.end_s > self.duration_s
                || e.start_s >= e.end_s
                || e.arrival_s < e.start_s
                || e.arrival_s > e.end_s
                || e.uncertainty_s <= 0.0
                || e.frequency_sigma_hz <= 0.0
                || e.emitted_frequency_hz < 0.0
            {
                return Err(format!("Invalid interval or uncertainty for {}", e.label));
            }
            if e.selected_for_scoring && e.review != ReviewState::Confirmed {
                return Err(format!(
                    "Confirm {} before including it in scoring",
                    e.label
                ));
            }
            if e.points.len() > 100_000
                || total > 1_000_000
                || e.points.iter().any(|p| {
                    !p.time_s.is_finite()
                        || !p.frequency_hz.is_finite()
                        || p.time_s < e.start_s
                        || p.time_s > e.end_s
                        || p.frequency_hz <= 0.0
                        || p.frequency_hz > 2000.0
                })
                || e.points.windows(2).any(|p| p[0].time_s >= p[1].time_s)
            {
                return Err(format!("Invalid measured trace for {}", e.label));
            }
            if e.kind == EventKind::Thunder && !e.points.is_empty() {
                return Err("Thunder events use intervals, not frequency traces".into());
            }
        }
        Ok(())
    }
}
pub fn parse_utc(value: &str) -> Result<f64, String> {
    let text = value.trim();
    let v = text.strip_suffix('Z').unwrap_or(text);
    if v.len() != 19
        || !v.is_ascii()
        || !matches!(v.as_bytes()[10], b'T' | b' ')
        || v.as_bytes()[13] != b':'
        || v.as_bytes()[16] != b':'
        || ![11, 12, 14, 15, 17, 18]
            .iter()
            .all(|&i| v.as_bytes()[i].is_ascii_digit())
    {
        return Err("Use UTC YYYY-MM-DD HH:MM:SS with optional single Z".into());
    }
    let day = crate::detailed_data::utc_midnight(&v[..10])?;
    let h = v[11..13].parse::<u32>().map_err(|_| "Invalid UTC hour")?;
    let m = v[14..16].parse::<u32>().map_err(|_| "Invalid UTC minute")?;
    let s = v[17..19].parse::<u32>().map_err(|_| "Invalid UTC second")?;
    if h > 23 || m > 59 || s > 59 {
        return Err("Invalid UTC time".into());
    }
    Ok(day + (h * 3600 + m * 60 + s) as f64)
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}
fn safe_event_id(id: &str) -> bool {
    if id.is_empty()
        || id.len() > 160
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return false;
    }
    let lower = id.to_ascii_lowercase();
    if ["workspace", "con", "prn", "aux", "nul", "clock$"].contains(&lower.as_str()) {
        return false;
    }
    !((lower.starts_with("com") || lower.starts_with("lpt"))
        && lower.len() == 4
        && matches!(lower.as_bytes()[3], b'1'..=b'9'))
}
fn path_key(path: &Path) -> String {
    #[cfg(windows)]
    {
        path.to_string_lossy().to_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned()
    }
}
fn validate_binding(binding: &SourceBinding) -> Result<(), String> {
    if !valid_hash(&binding.sha256) || binding.path.as_os_str().is_empty() {
        return Err("Source bindings need a path and lowercase SHA-256".into());
    }
    if !binding.path.is_absolute()
        && binding
            .path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("Relative evidence paths must stay inside the case directory".into());
    }
    Ok(())
}
fn validate_datasets(datasets: &[SourceBinding], options: &[DatasetOptions]) -> Result<(), String> {
    if datasets.len() > 1000 || options.len() > 1000 {
        return Err("Too many registered datasets".into());
    }
    let mut paths = HashSet::new();
    let mut hashes = HashSet::new();
    for binding in datasets {
        validate_binding(binding)?;
        if !paths.insert(path_key(&binding.path)) || !hashes.insert(binding.sha256.as_str()) {
            return Err("Dataset bindings must not repeat a path or source hash".into());
        }
    }
    let mut configured = HashSet::new();
    for option in options {
        if !valid_hash(&option.source_sha256)
            || !hashes.contains(option.source_sha256.as_str())
            || !configured.insert(&option.source_sha256)
            || option
                .time_origin_utc_s
                .is_some_and(|t| !t.is_finite() || !(0.0..253_402_300_800.0).contains(&t))
        {
            return Err(
                "Dataset options need one matching source hash and a finite Unix UTC origin".into(),
            );
        }
    }
    Ok(())
}
fn case_directory(manifest: &Path) -> Result<PathBuf, String> {
    manifest
        .canonicalize()
        .map_err(|e| e.to_string())?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Case has no directory".into())
}
fn resolve_binding(base: &Path, binding: &SourceBinding) -> Result<PathBuf, String> {
    validate_binding(binding)?;
    let candidate = if binding.path.is_absolute() {
        binding.path.clone()
    } else {
        base.join(&binding.path)
    };
    let path = candidate
        .canonicalize()
        .map_err(|e| format!("Evidence file unavailable {}: {e}", candidate.display()))?;
    if !binding.path.is_absolute() && !path.starts_with(base) {
        return Err("Relative evidence path resolves outside the case directory".into());
    }
    if !fs::metadata(&path).map_err(|e| e.to_string())?.is_file() {
        return Err("Evidence source must be a regular file".into());
    }
    Ok(path)
}
pub fn verify(manifest: &Path, document: &ReviewWorkspace) -> Result<(), String> {
    verify_with_checkpoint(manifest, document, || Ok(()))
}

pub fn verify_with_cancel(
    manifest: &Path,
    document: &ReviewWorkspace,
    cancel: &AtomicBool,
) -> Result<(), String> {
    verify_with_checkpoint(manifest, document, || {
        if cancel.load(Ordering::Relaxed) {
            Err("Workspace verification cancelled".into())
        } else {
            Ok(())
        }
    })
}

pub fn verify_with_checkpoint(
    manifest: &Path,
    document: &ReviewWorkspace,
    mut checkpoint: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    checkpoint()?;
    document.validate()?;
    let c = casework::read_case_with_checkpoint(manifest, true, &mut checkpoint)?;
    if c.case_id != document.case_id
        || c.recording.sha256 != document.recording_sha256
        || document.channel >= c.recording.media.channels as usize
        || (c.recording.media.duration_s - document.duration_s).abs() > 0.001
    {
        return Err("Workspace belongs to a different recording or channel".into());
    }
    let base = case_directory(manifest)?;
    let mut checked: HashMap<PathBuf, String> = HashMap::new();
    for binding in document
        .datasets
        .iter()
        .chain(document.events.iter().filter_map(|e| e.source.as_ref()))
    {
        checkpoint()?;
        let path = resolve_binding(&base, binding)?;
        let actual = match checked.get(&path) {
            Some(hash) => hash.clone(),
            None => {
                let hash = crate::detailed_data::sha256_with_checkpoint(&path, &mut checkpoint)?;
                checked.insert(path.clone(), hash.clone());
                hash
            }
        };
        if actual != binding.sha256 {
            return Err(format!("Evidence file changed: {}", binding.path.display()));
        }
    }
    checkpoint()?;
    Ok(())
}
fn transformed_bindings(
    document: &ReviewWorkspace,
    base: &Path,
    portable: bool,
) -> Result<ReviewWorkspace, String> {
    let mut result = document.clone();
    for binding in result
        .datasets
        .iter_mut()
        .chain(result.events.iter_mut().filter_map(|e| e.source.as_mut()))
    {
        let absolute = resolve_binding(base, binding)?;
        binding.path = if portable {
            absolute
                .strip_prefix(base)
                .map(Path::to_path_buf)
                .unwrap_or(absolute)
        } else {
            absolute
        };
    }
    Ok(result)
}
fn regular_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Review directory must be a regular directory, not a link".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("Review directory must not be a reparse point or junction".into());
        }
    }
    Ok(())
}
// Hold directory handles for the whole write. On Windows, denying delete sharing
// prevents replacement/rename of an output directory after the reparse check.
fn guard_directory(path: &Path) -> Result<fs::File, String> {
    regular_directory(path)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .share_mode(3)
            .custom_flags(0x0200_0000 | 0x0020_0000);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("Cannot hold review directory: {e}"))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_dir() {
        return Err("Review directory changed during opening".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("Review directory became a reparse point".into());
        }
    }
    Ok(file)
}
fn write_new_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|e| e.to_string())?;
    file.write_all(b"\n")
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn publish_new(staged: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
        }
        let from: Vec<_> = staged.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<_> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x8) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::hard_link(staged, destination).map_err(|e| e.to_string())?;
        fs::remove_file(staged).map_err(|e| e.to_string())
    }
}
pub fn save(manifest: &Path, document: &ReviewWorkspace) -> Result<PathBuf, String> {
    let manifest = manifest.canonicalize().map_err(|e| e.to_string())?;
    let initial_case_hash = sha256(&manifest)?;
    let case_base = case_directory(&manifest)?;
    let _case_guard = guard_directory(&case_base)?;
    verify(&manifest, document)?;
    let portable = transformed_bindings(document, &case_base, true)?;
    let bytes = serde_json::to_vec_pretty(&portable).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "Workspace exceeds {}",
            crate::data_size::format_gib(64 * 1024 * 1024)
        ));
    }
    let base = case_base.join("workspace-reviews");
    match fs::create_dir(&base) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
    }
    let _base_guard = guard_directory(&base)?;
    if base.canonicalize().map_err(|e| e.to_string())? != base {
        return Err("Review directory escaped its case".into());
    }
    let path = base.join(format!(
        "review-{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos(),
        std::process::id(),
        REVISION_SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).map_err(|e| e.to_string())?;
    let _revision_guard = guard_directory(&path)?;
    for e in document
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Doppler && !e.points.is_empty())
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.join(format!("{}.csv", e.id)))
            .map_err(|e| e.to_string())?;
        writeln!(f, "time_s,frequency_hz,weight").map_err(|e| e.to_string())?;
        for p in &e.points {
            writeln!(f, "{},{},1", p.time_s, p.frequency_hz).map_err(|e| e.to_string())?;
        }
        f.sync_all().map_err(|e| e.to_string())?;
        if e.emitted_frequency_hz > 0.0 {
            let trace = crate::physics::EventTrace {
                points: e
                    .points
                    .iter()
                    .map(|p| crate::physics::TracePoint {
                        t_rec_s: p.time_s,
                        frequency_hz: p.frequency_hz,
                    })
                    .collect(),
                f0_hz: vec![e.emitted_frequency_hz],
            };
            write_new_json(&path.join(format!("{}.json", e.id)), &trace)?;
        }
    }
    let staged = path.join(".workspace-pending.json");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    verify(&manifest, &portable)?;
    if sha256(&manifest)? != initial_case_hash {
        return Err("Case changed while saving review".into());
    }
    let destination = path.join("workspace.json");
    publish_new(&staged, &destination)?;
    Ok(destination)
}
pub fn load(manifest: &Path, path: &Path) -> Result<ReviewWorkspace, String> {
    let initial_case_hash = sha256(manifest)?;
    let mut bytes = Vec::new();
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Workspace must be a regular file".into());
    }
    file.take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "Workspace file exceeds {}",
            crate::data_size::format_gib(64 * 1024 * 1024)
        ));
    }
    let doc: ReviewWorkspace = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    verify(manifest, &doc)?;
    let absolute = transformed_bindings(&doc, &case_directory(manifest)?, false)?;
    verify(manifest, &absolute)?;
    if sha256(manifest)? != initial_case_hash {
        return Err("Case changed while reopening review".into());
    }
    Ok(absolute)
}
