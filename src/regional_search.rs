//! Analyst-approved regional hypotheses. Only the declared source alternatives,
//! one shared clock grid and bounded emitted tones are searched. Event choices
//! factor exactly under the existing equal-event quadratic scoring rule.
use crate::{
    evidence_score::{
        CandidateScore, EvidenceTerm, LightningCatalog, ObserverLocation, TermContribution,
    },
    map_data::MapData,
    physics::geodetic_to_ecef,
    workspace::{EventKind, EvidenceEvent, ReviewWorkspace, parse_utc},
    workspace_scoring,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, BinaryHeap},
    sync::atomic::{AtomicBool, Ordering as AtomicOrdering},
};

pub const MAX_EVALUATIONS: u64 = 2_000_000;
pub const MAX_RESIDUAL_EVALUATIONS: u64 = 20_000_000;
pub const MAX_TRAJECTORY_POINT_VISITS: u64 = 500_000_000;
const MAX_EVENTS: usize = 32;
pub const MAX_SOURCES_PER_EVENT: usize = 256;
pub const MAX_RETAINED: usize = 1000;
const MAX_UNAVAILABLE_EXAMPLES: usize = 2000;
const OBSERVER_BATCH: usize = 16;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClockRange {
    /// Absolute adjustment relative to recording_start_utc; not an additional
    /// adjustment on top of the current map.clock_adjustment_s.
    pub min_offset_s: f64,
    pub max_offset_s: f64,
    pub step_s: f64,
}
impl Default for ClockRange {
    fn default() -> Self {
        Self {
            min_offset_s: 0.0,
            max_offset_s: 0.0,
            step_s: 1.0,
        }
    }
}
impl ClockRange {
    pub fn values(&self) -> Result<Vec<f64>, String> {
        if ![self.min_offset_s, self.max_offset_s, self.step_s]
            .iter()
            .all(|v| v.is_finite())
            || self.min_offset_s > self.max_offset_s
            || self.step_s < 0.001
            || self.step_s > 86400.0
            || self.max_offset_s - self.min_offset_s > 31.0 * 86400.0
        {
            return Err("Clock search needs finite ordered offsets, a 0.001..86400 s step and at most 31 days of uncertainty".into());
        }
        let steps = ((self.max_offset_s - self.min_offset_s) / self.step_s).ceil();
        if steps > 10_000.0 {
            return Err(
                "Clock search exceeds 10001 samples; increase the step or narrow its range".into(),
            );
        }
        let mut values = Vec::with_capacity(steps as usize + 1);
        for i in 0..=steps as usize {
            let value = (self.min_offset_s + self.step_s * i as f64).min(self.max_offset_s);
            if values.last().is_none_or(|last| value > *last) {
                values.push(value);
            }
        }
        if values.last().copied() != Some(self.max_offset_s) {
            values.push(self.max_offset_s);
        }
        Ok(values)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EventAlternatives {
    pub event_id: String,
    /// Explicitly approved alternatives. Suggestions do not belong here until
    /// the operator selects them. IDs refer to loaded tracks or lightning.
    pub source_ids: Vec<String>,
    /// Bounded analytic fit, inclusive. Equal endpoints hold the tone fixed.
    /// Required only when Doppler frequency is selected for this event.
    pub emitted_frequency_hz: Option<[f64; 2]>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RegionalPlan {
    pub schema_version: u32,
    pub clock: ClockRange,
    pub events: Vec<EventAlternatives>,
    pub max_evaluations: u64,
    pub retain_hypotheses: usize,
}
impl Default for RegionalPlan {
    fn default() -> Self {
        Self {
            schema_version: 1,
            clock: ClockRange::default(),
            events: Vec::new(),
            max_evaluations: 200_000,
            retain_hypotheses: 50,
        }
    }
}
impl RegionalPlan {
    /// Drafts may contain no approved alternatives or temporarily inverted
    /// ranges. This only guards serialized size and nonfinite values.
    pub fn validate_draft(&self) -> Result<(), String> {
        if self.schema_version != 1
            || self.events.len() > MAX_EVENTS
            || ![
                self.clock.min_offset_s,
                self.clock.max_offset_s,
                self.clock.step_s,
            ]
            .iter()
            .all(|v| v.is_finite())
            || self.events.iter().any(|e| {
                e.event_id.len() > 160
                    || e.source_ids.len() > MAX_SOURCES_PER_EVENT
                    || e.source_ids.iter().any(|id| id.len() > 512)
                    || e.emitted_frequency_hz
                        .is_some_and(|r| r.iter().any(|v| !v.is_finite()))
            })
        {
            return Err("Unsupported or oversized regional-search draft".into());
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), String> {
        self.validate_draft()?;
        self.clock.values()?;
        if self.events.is_empty()
            || self.max_evaluations == 0
            || self.max_evaluations > MAX_EVALUATIONS
            || self.retain_hypotheses == 0
            || self.retain_hypotheses > MAX_RETAINED
        {
            return Err(format!(
                "Regional search needs reviewed events, 1..{MAX_EVALUATIONS} allowed evaluations and 1..{MAX_RETAINED} retained hypotheses"
            ));
        }
        let mut ids = BTreeSet::new();
        for event in &self.events {
            if event.event_id.is_empty()
                || !ids.insert(&event.event_id)
                || event.source_ids.is_empty()
                || event.source_ids.iter().any(|id| id.trim().is_empty())
                || event.source_ids.iter().collect::<BTreeSet<_>>().len() != event.source_ids.len()
            {
                return Err("Each search event needs a unique ID and at least one distinct, explicitly approved source".into());
            }
            if let Some([lo, hi]) = event.emitted_frequency_hz
                && (!lo.is_finite() || !hi.is_finite() || lo <= 0.0 || lo > hi || hi > 4000.0)
            {
                return Err("Emitted-tone bounds must be ordered and within (0,4000] Hz".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchEstimate {
    pub observer_count: usize,
    pub clock_count: usize,
    pub event_count: usize,
    pub source_alternatives: usize,
    /// One event/source/receiver/shared-clock evaluation; tone fitting is analytic.
    pub evaluations: u64,
    pub residual_evaluations: u64,
    pub trajectory_point_visits: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SearchProgress {
    pub completed_evaluations: u64,
    pub total_evaluations: u64,
    pub completed_cells: u64,
    pub total_cells: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStatus {
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    DopplerOnly,
    ThunderOnly,
    Combined,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonOptions {
    pub modes: Vec<SearchMode>,
    pub max_total_evaluations: u64,
}
impl Default for ComparisonOptions {
    fn default() -> Self {
        Self {
            modes: vec![
                SearchMode::DopplerOnly,
                SearchMode::ThunderOnly,
                SearchMode::Combined,
            ],
            max_total_evaluations: 200_000,
        }
    }
}
impl ComparisonOptions {
    pub fn validate_draft(&self) -> Result<(), String> {
        if self.modes.len() > 3 {
            return Err("At most three independent scoring modes may be selected".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModalityPlan {
    pub regional: RegionalPlan,
    pub modes: Vec<SearchMode>,
    pub max_total_evaluations: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModeEstimate {
    pub mode: SearchMode,
    pub estimate: SearchEstimate,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModalityEstimate {
    pub modes: Vec<ModeEstimate>,
    pub evaluations: u64,
    pub residual_evaluations: u64,
    pub trajectory_point_visits: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModalityProgress {
    pub mode: SearchMode,
    pub mode_progress: SearchProgress,
    pub completed_evaluations: u64,
    pub total_evaluations: u64,
    pub completed_modes: usize,
    pub total_modes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeStatus {
    Completed,
    Cancelled,
    NotStarted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModeResult {
    pub mode: SearchMode,
    pub status: ModeStatus,
    pub report: Option<RegionalReport>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModalityReport {
    pub schema_version: u32,
    pub document_sha256: String,
    pub inputs_sha256: String,
    pub request: ModalityPlan,
    pub status: SearchStatus,
    pub estimate: ModalityEstimate,
    pub progress: ModalityProgress,
    pub modes: Vec<ModeResult>,
    pub location_verified: bool,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocationAssignment {
    pub event_id: String,
    pub kind: EventKind,
    pub source_id: String,
    pub emitted_frequency_hz: Option<f64>,
}
impl From<&EventAssignment> for LocationAssignment {
    fn from(value: &EventAssignment) -> Self {
        Self {
            event_id: value.event_id.clone(),
            kind: value.kind,
            source_id: value.source_id.clone(),
            emitted_frequency_hz: value.emitted_frequency_hz,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocationSummary {
    pub observer_index: usize,
    pub observer: ObserverLocation,
    pub completed_clocks: usize,
    pub rankable_clocks: usize,
    pub unavailable_clocks: usize,
    pub best_score: Option<f64>,
    pub best_clock_offset_s: Option<f64>,
    pub best_hypothesis_id: Option<String>,
    pub best_assignments: Vec<LocationAssignment>,
    pub spatial_at_boundary: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventAssignment {
    pub event_id: String,
    pub kind: EventKind,
    pub source_id: String,
    pub emitted_frequency_hz: Option<f64>,
    pub frequency_at_boundary: bool,
    pub weighted_contribution: f64,
    /// Compact exact event means/contributions. Individual residual vectors are
    /// omitted; explicitly applying and rescoring reproduces their full detail.
    pub terms: Vec<TermContribution>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegionalHypothesis {
    pub id: String,
    pub rank: usize,
    pub score: f64,
    pub observer: ObserverLocation,
    pub clock_offset_s: f64,
    pub recording_start_utc_s: f64,
    pub assignments: Vec<EventAssignment>,
    pub clock_at_boundary: bool,
    pub frequency_at_boundary: bool,
    pub spatial_at_boundary: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnavailableAlternative {
    pub event_id: String,
    pub source_id: String,
    pub clock_offset_s: f64,
    pub observer_index: usize,
    pub reasons: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegionalReport {
    pub schema_version: u32,
    pub document_sha256: String,
    pub inputs_sha256: String,
    pub observers_sha256: String,
    pub plan: RegionalPlan,
    pub status: SearchStatus,
    pub estimate: SearchEstimate,
    pub progress: SearchProgress,
    pub hypotheses: Vec<RegionalHypothesis>,
    /// Best supported joint assignment at every declared receiver, after all
    /// evaluated clocks. Independent of global top-K hypothesis truncation.
    #[serde(default)]
    pub location_summaries: Vec<LocationSummary>,
    pub unavailable: Vec<UnavailableAlternative>,
    pub unavailable_count: u64,
    pub rankable_cells: u64,
    pub unavailable_cells: u64,
    /// Exact total supported joint assignments, unless u64 overflowed. The
    /// algorithm enumerates only the provably best K at each receiver/clock.
    pub supported_hypotheses: Option<u64>,
    pub retained_truncated: bool,
    pub location_verified: bool,
    pub limitations: Vec<String>,
}

fn identity<T: Serialize + ?Sized>(value: &T) -> Result<String, String> {
    serde_json::to_vec(value)
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|e| e.to_string())
}
pub fn document_sha256(document: &ReviewWorkspace) -> Result<String, String> {
    identity(document)
}
pub fn inputs_sha256(
    map: &MapData,
    lightning: &Option<LightningCatalog>,
) -> Result<String, String> {
    identity(&(map, lightning))
}

fn active_events(document: &ReviewWorkspace) -> Vec<&EvidenceEvent> {
    let mut events: Vec<_> = document.active_scoring_events().collect();
    events.sort_by(|a, b| a.id.cmp(&b.id));
    events
}
fn checked_product(a: u64, b: u64) -> Result<u64, String> {
    a.checked_mul(b)
        .ok_or_else(|| "Regional work estimate overflowed".into())
}

pub fn estimate(
    document: &ReviewWorkspace,
    map: &MapData,
    lightning: &Option<LightningCatalog>,
    observers: &[ObserverLocation],
    plan: &RegionalPlan,
) -> Result<SearchEstimate, String> {
    document.validate()?;
    document.validate_acoustic_sources_for_scoring()?;
    plan.validate()?;
    let clocks = plan.clock.values()?;
    let start = parse_utc(&document.recording_start_utc)?;
    if clocks
        .iter()
        .any(|v| !(0.0..253_402_300_800.0).contains(&(start + v)))
    {
        return Err("Searched recording starts fall outside Unix UTC range".into());
    }
    let limit = if document.atmosphere.is_some() {
        workspace_scoring::MAX_ATMOSPHERE_OBSERVERS
    } else {
        workspace_scoring::MAX_OBSERVERS
    };
    if observers.is_empty() || observers.len() > limit {
        return Err(format!(
            "Regional search needs 1..{limit} receiver locations"
        ));
    }
    for observer in observers {
        geodetic_to_ecef(
            observer.latitude_deg,
            observer.longitude_deg,
            observer.ellipsoid_height_m,
        )?;
        let [south, west, north, east] = document.map.bounds;
        if observer.latitude_deg < south
            || observer.latitude_deg > north
            || observer.longitude_deg < west
            || observer.longitude_deg > east
        {
            return Err(
                "Every regional receiver must lie inside the declared search rectangle".into(),
            );
        }
    }
    if !weighted_total(document).is_finite() || weighted_total(document) <= 0.0 {
        return Err("Selected regional scoring weights must have a finite positive sum".into());
    }
    let events = active_events(document);
    if events.is_empty() || events.len() != plan.events.len() {
        return Err("The plan must include exactly every confirmed, selected event with an active scoring term".into());
    }
    if events.iter().any(|event| event.kind == EventKind::Thunder)
        && let Some(catalog) = lightning
    {
        crate::lightning_source::validate_policy(
            catalog,
            document.mix.lightning_source_policy.as_ref(),
            document.mix.thunder_scale_s,
        )?;
    }
    let mut source_alternatives = 0usize;
    let mut residuals = 0u64;
    let mut track_visits = 0u64;
    let mut doppler_events = 0usize;
    let mut thunder_events = 0usize;
    let single_residual_limit = if document.atmosphere.is_some() {
        20_000
    } else {
        workspace_scoring::MAX_RESIDUALS
    };
    let single_track_limit = if document.atmosphere.is_some() {
        150_000
    } else {
        workspace_scoring::MAX_TRACK_POINT_VISITS
    };
    for event in events {
        let choice = plan
            .events
            .iter()
            .find(|choice| choice.event_id == event.id)
            .ok_or_else(|| format!("Missing search alternatives for {}", event.id))?;
        source_alternatives += choice.source_ids.len();
        let measures = match event.kind {
            EventKind::Doppler => {
                doppler_events += 1;
                if document.mix.doppler_frequency
                    && (event.points.is_empty() || choice.emitted_frequency_hz.is_none())
                {
                    return Err(format!(
                        "{} needs measured points and explicit emitted-tone bounds",
                        event.label
                    ));
                }
                if !document.mix.doppler_frequency && choice.emitted_frequency_hz.is_some() {
                    return Err(
                        "Tone ranges must be omitted when Doppler frequency is disabled".into(),
                    );
                }
                for source in &choice.source_ids {
                    let tracks: Vec<_> = map
                        .tracks
                        .iter()
                        .filter(|track| track.id == *source)
                        .collect();
                    if tracks.len() != 1 {
                        return Err(format!(
                            "Approved aircraft {source} must identify exactly one loaded trajectory"
                        ));
                    }
                    if tracks[0].physics_track.points.len() > single_track_limit {
                        return Err(format!(
                            "Approved aircraft {source} exceeds the per-receiver trajectory budget; import a shorter UTC interval"
                        ));
                    }
                    track_visits = track_visits
                        .checked_add(tracks[0].physics_track.points.len() as u64)
                        .ok_or("Trajectory estimate overflowed")?;
                }
                (if document.mix.doppler_frequency {
                    event.points.len()
                } else {
                    0
                }) + usize::from(document.mix.doppler_timing)
            }
            EventKind::Thunder => {
                thunder_events += 1;
                if choice.emitted_frequency_hz.is_some() {
                    return Err("Thunder alternatives cannot have emitted-tone bounds".into());
                }
                let catalog = lightning
                    .as_ref()
                    .ok_or("Selected thunder needs a loaded lightning catalog")?;
                for source in &choice.source_ids {
                    if catalog
                        .strikes
                        .iter()
                        .filter(|strike| strike.id == *source)
                        .count()
                        != 1
                    {
                        return Err(format!(
                            "Approved strike {source} must identify exactly one loaded strike"
                        ));
                    }
                }
                1
            }
        };
        if measures > single_residual_limit {
            return Err("An event exceeds the per-receiver residual budget; reduce selected measurements explicitly".into());
        }
        residuals = residuals
            .checked_add(checked_product(
                measures as u64,
                choice.source_ids.len() as u64,
            )?)
            .ok_or("Residual estimate overflowed")?;
    }
    if (document.mix.doppler_frequency || document.mix.doppler_timing) && doppler_events == 0
        || document.mix.thunder_timing && thunder_events == 0
    {
        return Err("Every enabled evidence term needs at least one selected event".into());
    }
    let cells = checked_product(observers.len() as u64, clocks.len() as u64)?;
    let evaluations = checked_product(cells, source_alternatives as u64)?;
    let residual_evaluations = checked_product(cells, residuals)?;
    let trajectory_point_visits = checked_product(cells, track_visits)?;
    if evaluations > plan.max_evaluations {
        return Err(format!(
            "Search needs {evaluations} evaluations, above the declared {} budget; narrow time/region or explicitly review fewer alternatives",
            plan.max_evaluations
        ));
    }
    if residual_evaluations > MAX_RESIDUAL_EVALUATIONS
        || trajectory_point_visits > MAX_TRAJECTORY_POINT_VISITS
    {
        return Err(format!(
            "Search exceeds physical work limits ({residual_evaluations} residuals, {trajectory_point_visits} trajectory-point visits); narrow the declared search"
        ));
    }
    Ok(SearchEstimate {
        observer_count: observers.len(),
        clock_count: clocks.len(),
        event_count: plan.events.len(),
        source_alternatives,
        evaluations,
        residual_evaluations,
        trajectory_point_visits,
    })
}

fn one_source_map(map: &MapData, source: &str) -> MapData {
    MapData {
        source_path: map.source_path.clone(),
        source_sha256: map.source_sha256.clone(),
        time_origin_utc_s: map.time_origin_utc_s,
        tracks: map
            .tracks
            .iter()
            .filter(|track| track.id == source)
            .cloned()
            .collect(),
        candidates: Vec::new(),
        observers: Vec::new(),
        grid: None,
        terrain: None,
        lightning: Vec::new(),
        lightning_provenance: None,
        notes: Vec::new(),
        location_verified: false,
    }
}
fn weighted_total(document: &ReviewWorkspace) -> f64 {
    (if document.mix.doppler_frequency {
        document.mix.frequency_weight
    } else {
        0.0
    }) + (if document.mix.doppler_timing {
        document.mix.doppler_timing_weight
    } else {
        0.0
    }) + (if document.mix.thunder_timing {
        document.mix.thunder_weight
    } else {
        0.0
    })
}
fn boundary(value: f64, range: [f64; 2]) -> bool {
    (value - range[0]).abs() <= 1e-9_f64.max(value.abs() * 1e-12)
        || (value - range[1]).abs() <= 1e-9_f64.max(value.abs() * 1e-12)
}
fn assignment(
    document: &ReviewWorkspace,
    event: &EvidenceEvent,
    choice: &EventAlternatives,
    source: &str,
    mut candidate: CandidateScore,
    doppler_count: usize,
    thunder_count: usize,
) -> Result<EventAssignment, Vec<String>> {
    if candidate.score.is_none() {
        return Err(candidate.unavailable_reasons);
    }
    let tone = if event.kind == EventKind::Doppler && document.mix.doppler_frequency {
        let [lo, hi] = choice.emitted_frequency_hz.expect("checked by estimate");
        let term = candidate
            .terms
            .iter_mut()
            .find(|term| term.term == EvidenceTerm::DopplerFrequency)
            .expect("frequency enabled");
        let denominator = term
            .residuals
            .iter()
            .map(|r| (r.predicted / r.effective_scale).powi(2))
            .sum::<f64>();
        let numerator = term
            .residuals
            .iter()
            .map(|r| (r.predicted / r.effective_scale) * (r.observed / r.effective_scale))
            .sum::<f64>();
        if !denominator.is_finite() || denominator <= 0.0 || !numerator.is_finite() {
            return Err(vec!["Emitted-tone fit is numerically unresolved".into()]);
        }
        let fitted = (numerator / denominator).clamp(lo, hi);
        let mut squared = 0.0;
        for r in &term.residuals {
            let residual = (r.predicted * fitted - r.observed) / r.effective_scale;
            squared += residual * residual / term.residuals.len() as f64;
        }
        if !fitted.is_finite() || !squared.is_finite() {
            return Err(vec!["Emitted-tone fit overflowed".into()]);
        }
        term.mean_normalized_squared_residual = Some(squared);
        term.event_mean_squared_residuals
            .insert(event.id.clone(), squared);
        Some(fitted)
    } else {
        None
    };
    let mut contribution = 0.0;
    let weight_total = weighted_total(document);
    for term in &mut candidate.terms {
        let count = match term.term {
            EvidenceTerm::DopplerFrequency | EvidenceTerm::DopplerTiming => doppler_count,
            EvidenceTerm::ThunderTiming => thunder_count,
        };
        let part = term
            .mean_normalized_squared_residual
            .expect("rankable term")
            * (term.weight / weight_total)
            / count as f64;
        if !part.is_finite() {
            return Err(vec!["Event contribution overflowed".into()]);
        }
        term.weighted_contribution = Some(part);
        contribution += part;
        term.residuals.clear();
    }
    if !contribution.is_finite() {
        return Err(vec!["Joint event contribution overflowed".into()]);
    }
    Ok(EventAssignment {
        event_id: event.id.clone(),
        kind: event.kind,
        source_id: source.into(),
        emitted_frequency_hz: tone,
        frequency_at_boundary: tone
            .zip(choice.emitted_frequency_hz)
            .is_some_and(|(value, r)| r[0] != r[1] && boundary(value, r)),
        weighted_contribution: contribution,
        terms: candidate.terms,
    })
}

#[derive(Clone)]
struct Combination {
    score: f64,
    indices: Vec<usize>,
}
impl PartialEq for Combination {
    fn eq(&self, other: &Self) -> bool {
        self.score.total_cmp(&other.score) == Ordering::Equal && self.indices == other.indices
    }
}
impl Eq for Combination {}
impl PartialOrd for Combination {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Combination {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then_with(|| other.indices.cmp(&self.indices))
    }
}
fn best_combinations(options: &[Vec<EventAssignment>], limit: usize) -> Vec<Combination> {
    if options.iter().any(Vec::is_empty) {
        return Vec::new();
    }
    let zero = vec![0; options.len()];
    let score = |indices: &[usize]| {
        options
            .iter()
            .zip(indices)
            .map(|(values, &i)| values[i].weighted_contribution)
            .sum::<f64>()
    };
    let mut seen = BTreeSet::from([zero.clone()]);
    let mut queue = BinaryHeap::from([Combination {
        score: score(&zero),
        indices: zero,
    }]);
    let mut best = Vec::new();
    while best.len() < limit {
        let Some(next) = queue.pop() else { break };
        for event in 0..options.len() {
            let mut neighbor = next.indices.clone();
            neighbor[event] += 1;
            if neighbor[event] < options[event].len() && seen.insert(neighbor.clone()) {
                queue.push(Combination {
                    score: score(&neighbor),
                    indices: neighbor,
                });
            }
        }
        best.push(next);
    }
    best
}
struct Ranked(RegionalHypothesis);
impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Ranked {}
impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .score
            .total_cmp(&other.0.score)
            .then_with(|| self.0.id.cmp(&other.0.id))
    }
}

pub fn run<F: FnMut(SearchProgress)>(
    document: &ReviewWorkspace,
    map: &MapData,
    lightning: &Option<LightningCatalog>,
    observers: &[ObserverLocation],
    plan: &RegionalPlan,
    cancel: &AtomicBool,
    mut progress: F,
) -> Result<RegionalReport, String> {
    let estimate = estimate(document, map, lightning, observers, plan)?;
    let events = active_events(document);
    let doppler_count = events
        .iter()
        .filter(|event| event.kind == EventKind::Doppler)
        .count();
    let thunder_count = events.len() - doppler_count;
    let clocks = plan.clock.values()?;
    let base = parse_utc(&document.recording_start_utc)?;
    let mut report=RegionalReport{schema_version:1,document_sha256:document_sha256(document)?,inputs_sha256:inputs_sha256(map,lightning)?,observers_sha256:identity(observers)?,plan:plan.clone(),status:SearchStatus::Completed,
        progress:SearchProgress{total_evaluations:estimate.evaluations,total_cells:estimate.observer_count as u64*estimate.clock_count as u64,..Default::default()},estimate,
        hypotheses:Vec::new(),location_summaries:observers.iter().enumerate().map(|(index,observer)|LocationSummary{observer_index:index,observer:observer.clone(),completed_clocks:0,rankable_clocks:0,unavailable_clocks:0,best_score:None,best_clock_offset_s:None,best_hypothesis_id:None,best_assignments:Vec::new(),spatial_at_boundary:boundary(observer.latitude_deg,[document.map.bounds[0],document.map.bounds[2]])||boundary(observer.longitude_deg,[document.map.bounds[1],document.map.bounds[3]])}).collect(),unavailable:Vec::new(),unavailable_count:0,rankable_cells:0,unavailable_cells:0,supported_hypotheses:Some(0),retained_truncated:false,location_verified:false,
        limitations:vec![
            "Only the declared receivers, shared clock samples, approved source IDs and emitted-tone bounds were searched. This is a regional shortlist, not a probability or verified location.".into(),
            "Source associations remain hypotheses. The complete selected evidence is retained; a missing event makes that receiver/clock combination unavailable. Missing provider coverage is not evidence that no aircraft or lightning existed.".into(),
            "The top retained assignments are exact for the declared separable quadratic score. Event choices share one clock and receiver, but no one-to-one source constraint is imposed; correlated events/harmonics are not independent evidence.".into(),
            "Emitted tones are fitted analytically within explicit bounds. Clock and location are sampled; good alternatives between samples or outside bounds can be missed. Boundary flags include the declared search rectangle, not arbitrary holes in custom receiver sets.".into(),
            "Compact assignment terms retain exact means and contributions, not individual residual vectors. Explicitly apply a hypothesis and run the fixed comparison to inspect full residuals.".into(),
            "Cancellation preserves only fully evaluated receiver/clock cells. Retained results are a bounded top-K shortlist; unavailable examples are capped at 2000 while their total count remains explicit.".into(),
        ]};
    report
        .limitations
        .splice(0..0, document.acoustic_source_warnings());
    if document.mix.thunder_timing
        && let Some(catalog) = lightning
    {
        report
            .limitations
            .extend(crate::lightning_source::limitations(
                catalog,
                document.mix.lightning_source_policy.as_ref(),
            ));
    }
    let mut retained = BinaryHeap::<Ranked>::new();
    // Allocate each selected trajectory and single-strike catalog once. The
    // per-event document is reused while only clock/source strings are changed.
    let mut templates = Vec::with_capacity(events.len());
    let mut source_maps = BTreeMap::new();
    let mut source_lightning = BTreeMap::new();
    let empty_map = one_source_map(map, "");
    for &event in &events {
        let mut local = document.clone();
        local.regional_plan = None;
        local.events = vec![event.clone()];
        if event.kind == EventKind::Doppler {
            local.events[0].emitted_frequency_hz = 1.0;
            local.mix.thunder_timing = false;
        } else {
            local.mix.doppler_frequency = false;
            local.mix.doppler_timing = false;
        }
        let choice = plan
            .events
            .iter()
            .find(|choice| choice.event_id == event.id)
            .expect("validated event");
        for source in &choice.source_ids {
            if event.kind == EventKind::Doppler {
                source_maps
                    .entry(source.clone())
                    .or_insert_with(|| one_source_map(map, source));
            } else {
                source_lightning.entry(source.clone()).or_insert_with(|| {
                    lightning.as_ref().map(|catalog| LightningCatalog {
                        schema_version: catalog.schema_version,
                        input_path: catalog.input_path.clone(),
                        input_bytes: catalog.input_bytes,
                        input_sha256: catalog.input_sha256.clone(),
                        provenance: catalog.provenance.clone(),
                        strikes: catalog
                            .strikes
                            .iter()
                            .filter(|strike| strike.id == *source)
                            .cloned()
                            .collect(),
                    })
                });
            }
        }
        templates.push(local);
    }
    // Stay within the fixed scorer's per-call budgets without turning a work
    // limit into fictitious missing physical support.
    let max_measures = events
        .iter()
        .map(|event| {
            if event.kind == EventKind::Doppler {
                (if document.mix.doppler_frequency {
                    event.points.len()
                } else {
                    0
                }) + usize::from(document.mix.doppler_timing)
            } else {
                1
            }
        })
        .max()
        .unwrap_or(1);
    let max_points = plan
        .events
        .iter()
        .flat_map(|event| &event.source_ids)
        .filter_map(|id| map.tracks.iter().find(|track| track.id == *id))
        .map(|track| track.physics_track.points.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let residual_limit = if document.atmosphere.is_some() {
        20_000
    } else {
        workspace_scoring::MAX_RESIDUALS
    };
    let track_limit = if document.atmosphere.is_some() {
        150_000
    } else {
        workspace_scoring::MAX_TRACK_POINT_VISITS
    };
    let batch_size = OBSERVER_BATCH
        .min(residual_limit / max_measures)
        .min(track_limit / max_points)
        .max(1);
    progress(report.progress.clone());
    'search: for (clock_index, &offset) in clocks.iter().enumerate() {
        for batch_start in (0..observers.len()).step_by(batch_size) {
            if cancel.load(AtomicOrdering::Relaxed) {
                report.status = SearchStatus::Cancelled;
                break 'search;
            }
            let batch = &observers[batch_start..(batch_start + batch_size).min(observers.len())];
            let mut options: Vec<Vec<Vec<EventAssignment>>> =
                vec![vec![Vec::new(); events.len()]; batch.len()];
            for (event_index, &event) in events.iter().enumerate() {
                let choice = plan
                    .events
                    .iter()
                    .find(|choice| choice.event_id == event.id)
                    .expect("validated event");
                let mut sources: Vec<_> = choice.source_ids.iter().collect();
                sources.sort();
                for source in sources {
                    if cancel.load(AtomicOrdering::Relaxed) {
                        report.status = SearchStatus::Cancelled;
                        break 'search;
                    }
                    let local = &mut templates[event_index];
                    local.map.clock_adjustment_s = offset;
                    let selected = &mut local.events[0];
                    let local_map;
                    let no_lightning = None;
                    let local_lightning;
                    if event.kind == EventKind::Doppler {
                        selected.flight_id = source.clone();
                        local_map = &source_maps[source];
                        local_lightning = &no_lightning;
                    } else {
                        selected.strike_id = source.clone();
                        local_map = &empty_map;
                        local_lightning = &source_lightning[source];
                    }
                    let calculated = workspace_scoring::score_observers(
                        local,
                        local_map,
                        local_lightning,
                        document.strike_fallback_height_m,
                        batch,
                    );
                    let results: Vec<_> = match calculated {
                        Ok(result) => result
                            .candidates
                            .into_iter()
                            .map(|candidate| {
                                assignment(
                                    document,
                                    event,
                                    choice,
                                    source,
                                    candidate,
                                    doppler_count,
                                    thunder_count,
                                )
                            })
                            .collect(),
                        Err(reason) => vec![Err(vec![reason]); batch.len()],
                    };
                    for (i, result) in results.into_iter().enumerate() {
                        match result {
                            Ok(value) => options[i][event_index].push(value),
                            Err(mut reasons) => {
                                report.unavailable_count += 1;
                                if report.unavailable.len() < MAX_UNAVAILABLE_EXAMPLES {
                                    if reasons.len() > 8 {
                                        let omitted = reasons.len() - 8;
                                        reasons.truncate(8);
                                        reasons.push(format!(
                                            "{omitted} additional unavailable details omitted"
                                        ));
                                    }
                                    report.unavailable.push(UnavailableAlternative {
                                        event_id: event.id.clone(),
                                        source_id: source.clone(),
                                        clock_offset_s: offset,
                                        observer_index: batch_start + i,
                                        reasons,
                                    });
                                }
                            }
                        }
                    }
                    report.progress.completed_evaluations += batch.len() as u64;
                    progress(report.progress.clone());
                }
            }
            for (i, mut cell) in options.into_iter().enumerate() {
                report.progress.completed_cells += 1;
                let location = &mut report.location_summaries[batch_start + i];
                location.completed_clocks += 1;
                if cell.iter().any(Vec::is_empty) {
                    report.unavailable_cells += 1;
                    location.unavailable_clocks += 1;
                    continue;
                }
                report.rankable_cells += 1;
                location.rankable_clocks += 1;
                let combinations = cell.iter().try_fold(1u64, |product, event| {
                    product.checked_mul(event.len() as u64)
                });
                report.supported_hypotheses = report
                    .supported_hypotheses
                    .zip(combinations)
                    .and_then(|(sum, count)| sum.checked_add(count));
                for choices in &mut cell {
                    choices.sort_by(|a, b| {
                        a.weighted_contribution
                            .total_cmp(&b.weighted_contribution)
                            .then_with(|| a.source_id.cmp(&b.source_id))
                    });
                }
                let observer = &batch[i];
                for combination in best_combinations(&cell, plan.retain_hypotheses) {
                    let assignments: Vec<_> = cell
                        .iter()
                        .zip(&combination.indices)
                        .map(|(choices, &index)| choices[index].clone())
                        .collect();
                    let association_id = identity(
                        &assignments
                            .iter()
                            .map(|a| (&a.event_id, &a.source_id))
                            .collect::<Vec<_>>(),
                    )?;
                    let id = format!(
                        "regional-{clock_index:05}-{:05}-{association_id}",
                        batch_start + i
                    );
                    let hypothesis = RegionalHypothesis {
                        id,
                        rank: 0,
                        score: combination.score,
                        observer: observer.clone(),
                        clock_offset_s: offset,
                        recording_start_utc_s: base + offset,
                        clock_at_boundary: plan.clock.min_offset_s != plan.clock.max_offset_s
                            && boundary(offset, [plan.clock.min_offset_s, plan.clock.max_offset_s]),
                        frequency_at_boundary: assignments.iter().any(|a| a.frequency_at_boundary),
                        spatial_at_boundary: boundary(
                            observer.latitude_deg,
                            [document.map.bounds[0], document.map.bounds[2]],
                        ) || boundary(
                            observer.longitude_deg,
                            [document.map.bounds[1], document.map.bounds[3]],
                        ),
                        assignments,
                    };
                    if location.best_score.is_none_or(|score| {
                        hypothesis.score < score
                            || (hypothesis.score == score
                                && location
                                    .best_hypothesis_id
                                    .as_ref()
                                    .is_none_or(|id| &hypothesis.id < id))
                    }) {
                        location.best_score = Some(hypothesis.score);
                        location.best_clock_offset_s = Some(offset);
                        location.best_hypothesis_id = Some(hypothesis.id.clone());
                        location.best_assignments = hypothesis
                            .assignments
                            .iter()
                            .map(LocationAssignment::from)
                            .collect();
                    }
                    retained.push(Ranked(hypothesis));
                    if retained.len() > plan.retain_hypotheses {
                        retained.pop();
                    }
                }
            }
            progress(report.progress.clone());
        }
    }
    report.hypotheses = retained
        .into_sorted_vec()
        .into_iter()
        .map(|ranked| ranked.0)
        .collect();
    for (index, hypothesis) in report.hypotheses.iter_mut().enumerate() {
        hypothesis.rank = index + 1;
    }
    report.retained_truncated = report
        .supported_hypotheses
        .is_none_or(|total| total > report.hypotheses.len() as u64);
    if report.progress.completed_cells < report.progress.total_cells {
        report.status = SearchStatus::Cancelled;
    }
    progress(report.progress.clone());
    Ok(report)
}

/// Explicit operator action only. This never confirms events, changes measured
/// points or silently marks the location verified. The caller also verifies
/// report.inputs_sha256 against the currently loaded data before applying.
fn validate_for_apply(document: &ReviewWorkspace, report: &RegionalReport) -> Result<(), String> {
    if report.schema_version != 1
        || report.status != SearchStatus::Completed
        || report.progress.completed_cells != report.progress.total_cells
        || report.progress.completed_evaluations != report.progress.total_evaluations
    {
        return Err("Only a completed regional search can be applied; review partial results or run the complete plan".into());
    }
    if document_sha256(document)? != report.document_sha256 {
        return Err(
            "Evidence or search assumptions changed; run the regional search again before applying"
                .into(),
        );
    }
    report.plan.validate()
}
fn apply_choices(
    document: &ReviewWorkspace,
    plan: &RegionalPlan,
    clock_offset_s: f64,
    observer: &ObserverLocation,
    assignments: &[LocationAssignment],
) -> Result<ReviewWorkspace, String> {
    if !plan.clock.values()?.contains(&clock_offset_s) {
        return Err("Hypothesis is outside the declared shared-clock grid".into());
    }
    geodetic_to_ecef(
        observer.latitude_deg,
        observer.longitude_deg,
        observer.ellipsoid_height_m,
    )?;
    let events = active_events(document);
    if events.len() != assignments.len() {
        return Err("Hypothesis does not preserve the selected event set".into());
    }
    let mut updated = document.clone();
    let mut applied = BTreeSet::new();
    for assignment in assignments {
        let approved = plan
            .events
            .iter()
            .find(|e| e.event_id == assignment.event_id)
            .ok_or("Hypothesis event was not approved")?;
        let event = events
            .iter()
            .find(|e| e.id == assignment.event_id)
            .ok_or("Hypothesis event is no longer selected")?;
        if !applied.insert(&assignment.event_id)
            || event.kind != assignment.kind
            || !approved.source_ids.contains(&assignment.source_id)
        {
            return Err("Hypothesis includes an unapproved or repeated event/source".into());
        }
        let target = updated
            .events
            .iter_mut()
            .find(|e| e.id == assignment.event_id)
            .expect("checked selected event");
        match event.kind {
            EventKind::Doppler => {
                target.flight_id = assignment.source_id.clone();
                if document.mix.doppler_frequency {
                    let value = assignment
                        .emitted_frequency_hz
                        .ok_or("Hypothesis omits the emitted tone")?;
                    let [lo, hi] = approved
                        .emitted_frequency_hz
                        .ok_or("Tone fitting was not approved")?;
                    if !value.is_finite() || value < lo || value > hi {
                        return Err("Fitted tone is outside approved bounds".into());
                    }
                    target.emitted_frequency_hz = value;
                }
            }
            EventKind::Thunder => target.strike_id = assignment.source_id.clone(),
        }
    }
    updated.map.clock_adjustment_s = clock_offset_s;
    updated.map.receiver_height_m = observer.ellipsoid_height_m;
    updated.validate()?;
    Ok(updated)
}
pub fn apply_hypothesis(
    document: &ReviewWorkspace,
    report: &RegionalReport,
    id: &str,
) -> Result<ReviewWorkspace, String> {
    validate_for_apply(document, report)?;
    let matches: Vec<_> = report.hypotheses.iter().filter(|h| h.id == id).collect();
    if matches.len() != 1 {
        return Err("Select exactly one retained hypothesis".into());
    }
    let h = matches[0];
    if !h.score.is_finite() || h.score < 0.0 {
        return Err("Hypothesis score is invalid".into());
    }
    apply_choices(
        document,
        &report.plan,
        h.clock_offset_s,
        &h.observer,
        &h.assignments
            .iter()
            .map(LocationAssignment::from)
            .collect::<Vec<_>>(),
    )
}
pub fn apply_location_hypothesis(
    document: &ReviewWorkspace,
    report: &RegionalReport,
    observer_index: usize,
) -> Result<ReviewWorkspace, String> {
    validate_for_apply(document, report)?;
    let matches: Vec<_> = report
        .location_summaries
        .iter()
        .filter(|row| row.observer_index == observer_index)
        .collect();
    if matches.len() != 1 {
        return Err("Select one receiver with a geographic summary".into());
    }
    let row = matches[0];
    if row.completed_clocks != report.estimate.clock_count
        || row.rankable_clocks == 0
        || row
            .best_score
            .is_none_or(|score| !score.is_finite() || score < 0.0)
    {
        return Err("Receiver has no complete, rankable clock search".into());
    }
    apply_choices(
        document,
        &report.plan,
        row.best_clock_offset_s
            .ok_or("Receiver has no best clock")?,
        &row.observer,
        &row.best_assignments,
    )
}

/// Derive one explicitly requested mode without changing observations, approved
/// alternatives within that modality, clock bounds, weights or receiver data.
pub fn mode_document(
    document: &ReviewWorkspace,
    plan: &RegionalPlan,
    mode: SearchMode,
) -> Result<ReviewWorkspace, String> {
    document.validate()?;
    plan.validate()?;
    let has_doppler = document.mix.doppler_frequency || document.mix.doppler_timing;
    let has_thunder = document.mix.thunder_timing;
    if matches!(mode, SearchMode::DopplerOnly | SearchMode::Combined) && !has_doppler
        || matches!(mode, SearchMode::ThunderOnly | SearchMode::Combined) && !has_thunder
    {
        return Err("A requested scoring mode has no enabled evidence terms; select its terms explicitly before comparison".into());
    }
    let selected = active_events(document);
    let expected: BTreeSet<_> = selected.iter().map(|e| e.id.as_str()).collect();
    let supplied: BTreeSet<_> = plan.events.iter().map(|e| e.event_id.as_str()).collect();
    if expected != supplied {
        return Err(
            "The comparison plan must preserve exactly the original selected event set".into(),
        );
    }
    let mut derived = document.clone();
    match mode {
        SearchMode::DopplerOnly => derived.mix.thunder_timing = false,
        SearchMode::ThunderOnly => {
            derived.mix.doppler_frequency = false;
            derived.mix.doppler_timing = false;
        }
        SearchMode::Combined => {}
    }
    let included: BTreeSet<_> = active_events(&derived)
        .iter()
        .map(|e| e.id.clone())
        .collect();
    let mut filtered = plan.clone();
    filtered.events.retain(|e| included.contains(&e.event_id));
    filtered.validate()?;
    derived.regional_plan = Some(filtered);
    derived.regional_comparison = None;
    Ok(derived)
}

pub fn estimate_modalities(
    document: &ReviewWorkspace,
    map: &MapData,
    lightning: &Option<LightningCatalog>,
    observers: &[ObserverLocation],
    request: &ModalityPlan,
) -> Result<ModalityEstimate, String> {
    if request.modes.is_empty()
        || request.modes.len() > 3
        || request
            .modes
            .iter()
            .enumerate()
            .any(|(i, mode)| request.modes[..i].contains(mode))
        || request.max_total_evaluations == 0
        || request.max_total_evaluations > 3 * MAX_EVALUATIONS
    {
        return Err(format!(
            "Select 1..3 distinct scoring modes and a combined work budget within 1..{}",
            3 * MAX_EVALUATIONS
        ));
    }
    let mut result = ModalityEstimate {
        modes: Vec::new(),
        evaluations: 0,
        residual_evaluations: 0,
        trajectory_point_visits: 0,
    };
    for &mode in &request.modes {
        let derived = mode_document(document, &request.regional, mode)?;
        let work = estimate(
            &derived,
            map,
            lightning,
            observers,
            derived.regional_plan.as_ref().expect("derived plan"),
        )?;
        result.evaluations = result
            .evaluations
            .checked_add(work.evaluations)
            .ok_or("Comparison work overflowed")?;
        result.residual_evaluations = result
            .residual_evaluations
            .checked_add(work.residual_evaluations)
            .ok_or("Comparison work overflowed")?;
        result.trajectory_point_visits = result
            .trajectory_point_visits
            .checked_add(work.trajectory_point_visits)
            .ok_or("Comparison work overflowed")?;
        result.modes.push(ModeEstimate {
            mode,
            estimate: work,
        });
    }
    if result.evaluations > request.max_total_evaluations {
        return Err(format!(
            "Independent mode comparison needs {} evaluations across all requested modes, above its declared {} total budget",
            result.evaluations, request.max_total_evaluations
        ));
    }
    Ok(result)
}

pub fn run_modalities<F: FnMut(ModalityProgress)>(
    document: &ReviewWorkspace,
    map: &MapData,
    lightning: &Option<LightningCatalog>,
    observers: &[ObserverLocation],
    request: &ModalityPlan,
    cancel: &AtomicBool,
    mut progress: F,
) -> Result<ModalityReport, String> {
    let estimate = estimate_modalities(document, map, lightning, observers, request)?;
    let total = estimate.evaluations;
    let first = request.modes[0];
    let mut report=ModalityReport{schema_version:1,document_sha256:document_sha256(document)?,inputs_sha256:inputs_sha256(map,lightning)?,request:request.clone(),status:SearchStatus::Completed,
        progress:ModalityProgress{mode:first,mode_progress:SearchProgress::default(),completed_evaluations:0,total_evaluations:total,completed_modes:0,total_modes:request.modes.len()},estimate,
        modes:request.modes.iter().map(|&mode|ModeResult{mode,status:ModeStatus::NotStarted,report:None}).collect(),location_verified:false,
        limitations:vec!["Modes run independently on identical declared receivers and shared-clock bounds, with the same observations and approved alternatives within each modality. Only disabled modality terms/events are removed; no weights or scales are retuned.".into(),
            "Each mode's geographic summaries profile its own approved source and clock alternatives. Spatial overlap after independent clock fitting is not shared spatiotemporal support. Only the best clock is retained per receiver; compare clocks and combined residuals rather than treating overlap as confirmation.".into(),
            "Cancellation leaves later requested modes explicitly not started. An incomplete comparison cannot establish agreement or disagreement for those modes.".into()]};
    let mut source_warnings = BTreeSet::new();
    for &mode in &request.modes {
        let derived = mode_document(document, &request.regional, mode)?;
        source_warnings.extend(derived.acoustic_source_warnings());
        if derived.mix.thunder_timing
            && let Some(catalog) = lightning
        {
            source_warnings.extend(crate::lightning_source::limitations(
                catalog,
                derived.mix.lightning_source_policy.as_ref(),
            ));
        }
    }
    report.limitations.splice(0..0, source_warnings);
    progress(report.progress.clone());
    for (index, &mode) in request.modes.iter().enumerate() {
        if cancel.load(AtomicOrdering::Relaxed) {
            report.status = SearchStatus::Cancelled;
            break;
        }
        let derived = mode_document(document, &request.regional, mode)?;
        let completed_before = report.progress.completed_evaluations;
        let completed_modes = report.progress.completed_modes;
        let mut last = report.progress.clone();
        let nested = run(
            &derived,
            map,
            lightning,
            observers,
            derived.regional_plan.as_ref().expect("derived plan"),
            cancel,
            |mode_progress| {
                last = ModalityProgress {
                    mode,
                    completed_evaluations: completed_before + mode_progress.completed_evaluations,
                    total_evaluations: total,
                    completed_modes,
                    total_modes: request.modes.len(),
                    mode_progress,
                };
                progress(last.clone());
            },
        )?;
        report.progress = last;
        let completed = nested.status == SearchStatus::Completed;
        report.modes[index] = ModeResult {
            mode,
            status: if completed {
                ModeStatus::Completed
            } else {
                ModeStatus::Cancelled
            },
            report: Some(nested),
        };
        if completed {
            report.progress.completed_modes += 1;
        } else {
            report.status = SearchStatus::Cancelled;
        }
        progress(report.progress.clone());
        if !completed {
            break;
        }
    }
    if report.progress.completed_modes != request.modes.len() {
        report.status = SearchStatus::Cancelled;
    }
    progress(report.progress.clone());
    Ok(report)
}

fn completed_mode<'a>(
    document: &ReviewWorkspace,
    comparison: &'a ModalityReport,
    mode: SearchMode,
) -> Result<(ReviewWorkspace, &'a RegionalReport), String> {
    if comparison.schema_version != 1
        || comparison.status != SearchStatus::Completed
        || comparison.progress.completed_modes != comparison.request.modes.len()
        || document_sha256(document)? != comparison.document_sha256
    {
        return Err(
            "Apply a mode only from a completed comparison of the unchanged original review".into(),
        );
    }
    let entries: Vec<_> = comparison
        .modes
        .iter()
        .filter(|entry| entry.mode == mode)
        .collect();
    if entries.len() != 1 || entries[0].status != ModeStatus::Completed {
        return Err("Requested scoring mode was not completed".into());
    }
    Ok((
        mode_document(document, &comparison.request.regional, mode)?,
        entries[0]
            .report
            .as_ref()
            .ok_or("Completed mode report is missing")?,
    ))
}
/// Explicitly applying a mode also changes selected term flags to that mode.
/// The original review is untouched; callers retain it for Undo and provenance.
pub fn apply_modality_hypothesis(
    document: &ReviewWorkspace,
    comparison: &ModalityReport,
    mode: SearchMode,
    id: &str,
) -> Result<ReviewWorkspace, String> {
    let (derived, report) = completed_mode(document, comparison, mode)?;
    apply_hypothesis(&derived, report, id)
}
pub fn apply_modality_location(
    document: &ReviewWorkspace,
    comparison: &ModalityReport,
    mode: SearchMode,
    observer_index: usize,
) -> Result<ReviewWorkspace, String> {
    let (derived, report) = completed_mode(document, comparison, mode)?;
    apply_location_hypothesis(&derived, report, observer_index)
}
