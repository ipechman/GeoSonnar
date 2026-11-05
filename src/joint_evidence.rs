//! Optional, explicit frequency/envelope comparison on one receiver-clock grid.
//! The existing physical engines are composed without changing their equations.
//! Scores remain separate; full-domain rank minimax is an uncalibrated ordering.

use crate::{
    evidence_score::{LightningCatalog, ObserverLocation},
    map_data::MapData,
    regional_search::{self, ClockRange, LocationAssignment, RegionalPlan},
    thunder_envelope::{self, EnvelopeReceiver, EnvelopeSearchPlan, ObservedEnvelope},
    workspace::{EventKind, ReviewWorkspace, parse_utc},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};

pub const MAX_CELLS: usize = 100_000;
pub const MAX_REQUEST_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_REPORT_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_MEMORY_BYTES: u64 = 256 * 1024 * 1024;
pub const DOPPLER_TOLERANCE: f64 = 1e-10;
pub const ENVELOPE_TOLERANCE: f64 = 1e-7;
const REASON_BYTES: usize = 192;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointLimits {
    pub max_cells: usize,
    pub max_doppler_evaluations: u64,
    pub max_doppler_residual_evaluations: u64,
    pub max_doppler_trajectory_point_visits: u64,
    pub max_envelope_work_units: u64,
    pub max_working_memory_bytes: u64,
    pub max_serialized_report_bytes: u64,
}
impl Default for JointLimits {
    fn default() -> Self {
        Self {
            max_cells: 25_000,
            max_doppler_evaluations: regional_search::MAX_EVALUATIONS,
            max_doppler_residual_evaluations: regional_search::MAX_RESIDUAL_EVALUATIONS,
            max_doppler_trajectory_point_visits: regional_search::MAX_TRAJECTORY_POINT_VISITS,
            max_envelope_work_units: 200_000_000,
            max_working_memory_bytes: MAX_MEMORY_BYTES,
            max_serialized_report_bytes: MAX_REPORT_BYTES,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointRequest {
    pub schema_version: u32,
    pub document: ReviewWorkspace,
    pub map: MapData,
    pub lightning: LightningCatalog,
    pub observation: ObservedEnvelope,
    pub regional_plan: RegionalPlan,
    /// Receivers here are the sole ordered receiver inventory for both engines.
    pub envelope_plan: EnvelopeSearchPlan,
    pub limits: JointLimits,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JointEstimate {
    pub request_sha256: String,
    pub request_bytes: u64,
    pub receivers: usize,
    pub clocks: usize,
    pub cells: usize,
    pub doppler_runs: usize,
    pub envelope_runs: usize,
    pub doppler_evaluations: u64,
    pub doppler_residual_evaluations: u64,
    pub doppler_trajectory_point_visits: u64,
    pub envelope_work_units: u64,
    pub peak_engine_memory_bytes: u64,
    pub working_memory_bound_bytes: u64,
    pub serialized_report_bound_bytes: u64,
    pub serialized_snapshot_bound_bytes: u64,
    pub ranking_work_bound: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JointStage {
    Doppler,
    Envelope,
    Ranking,
    Finished,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JointProgress {
    pub stage: JointStage,
    pub completed_doppler_runs: usize,
    pub total_doppler_runs: usize,
    pub completed_envelope_runs: usize,
    pub total_envelope_runs: usize,
    pub completed_doppler_cells: usize,
    pub completed_envelope_cells: usize,
    pub total_cells: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JointStatus {
    Completed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DopplerState {
    NotStarted,
    Available {
        score: f64,
        assignments: Vec<LocationAssignment>,
        frequency_at_boundary: bool,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EnvelopeState {
    NotStarted,
    Available {
        cosine_similarity: f64,
        contributing_source_rows: usize,
        distance_excluded_source_rows: usize,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JointCellRanks {
    pub doppler: f64,
    pub envelope: f64,
    pub combined: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JointCell {
    pub receiver_index: usize,
    pub clock_index: usize,
    pub clock_offset_s: f64,
    pub clock_at_boundary: bool,
    pub spatial_at_boundary: bool,
    pub doppler: DopplerState,
    pub envelope: EnvelopeState,
    pub ranks: Option<JointCellRanks>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JointReport {
    pub schema_version: u32,
    pub request_sha256: String,
    pub document_sha256: String,
    pub inputs_sha256: String,
    pub receivers: Vec<EnvelopeReceiver>,
    pub clock_offsets_s: Vec<f64>,
    pub nominal_recording_start_utc_s: f64,
    pub estimate: JointEstimate,
    pub progress: JointProgress,
    pub status: JointStatus,
    /// Completed execution is distinct from available scores or fused ranks.
    pub all_cells_available: bool,
    pub ranking_available: bool,
    pub cells: Vec<JointCell>,
    pub location_verified: bool,
    pub independent_field_validation: bool,
    pub limitations: Vec<String>,
}

struct LimitedHash {
    hash: Sha256,
    bytes: u64,
    limit: u64,
}
impl Write for LimitedHash {
    fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
        let next = self
            .bytes
            .checked_add(value.len() as u64)
            .ok_or_else(|| std::io::Error::other("Joint serialized size overflow"))?;
        if next > self.limit {
            return Err(std::io::Error::other(
                "Joint serialized data exceeds its bound",
            ));
        }
        self.hash.update(value);
        self.bytes = next;
        Ok(value.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn serialized<T: Serialize>(value: &T, limit: u64) -> Result<(String, u64), String> {
    let mut writer = LimitedHash {
        hash: Sha256::new(),
        bytes: 0,
        limit,
    };
    serde_json::to_writer(&mut writer, value).map_err(|e| e.to_string())?;
    Ok((format!("{:x}", writer.hash.finalize()), writer.bytes))
}
pub fn request_sha256(request: &JointRequest) -> Result<String, String> {
    serialized(request, MAX_REQUEST_BYTES).map(|v| v.0)
}
pub fn inputs_sha256(request: &JointRequest) -> Result<String, String> {
    regional_search::inputs_sha256(&request.map, &Some(request.lightning.clone()))
}
fn add(a: u64, b: u64) -> Result<u64, String> {
    a.checked_add(b)
        .ok_or_else(|| "Joint resource count overflow".into())
}
fn mul(a: u64, b: u64) -> Result<u64, String> {
    a.checked_mul(b)
        .ok_or_else(|| "Joint resource count overflow".into())
}
fn hash_ok(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn near(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-9
}
fn same_location(a: &ObserverLocation, b: &ObserverLocation) -> bool {
    a.latitude_deg == b.latitude_deg
        && a.longitude_deg == b.longitude_deg
        && a.ellipsoid_height_m == b.ellipsoid_height_m
}
fn at_boundary(value: f64, limits: [f64; 2]) -> bool {
    (value - limits[0]).abs().min((value - limits[1]).abs()) <= 1e-9
}
fn reason(value: &str) -> String {
    let mut result = String::new();
    for ch in value.chars() {
        let ch = if ch.is_control() {
            ' '
        } else if ch == '"' {
            '\''
        } else if ch == '\\' {
            '/'
        } else {
            ch
        };
        if result.len() + ch.len_utf8() > REASON_BYTES {
            break;
        }
        result.push(ch);
    }
    if result.trim().is_empty() {
        "Native comparison unavailable".into()
    } else {
        result
    }
}

#[derive(Clone, Copy)]
struct Tile {
    start: usize,
    len: usize,
}
struct Prepared {
    estimate: JointEstimate,
    observers: Vec<ObserverLocation>,
    clocks: Vec<f64>,
    tiles: Vec<Tile>,
}
fn envelope_tile(
    request: &JointRequest,
    clocks: &[f64],
    receiver: usize,
    tile: Tile,
) -> thunder_envelope::EnvelopeSearchRequest {
    let mut plan = request.envelope_plan.clone();
    plan.receivers = vec![plan.receivers[receiver].clone()];
    plan.min_clock_offset_s = clocks[tile.start];
    plan.max_clock_offset_s = clocks[tile.start + tile.len - 1];
    plan.keep_best = tile.len;
    plan.with_observation(request.observation.clone())
}
fn clock_plan(request: &JointRequest, clock: f64) -> RegionalPlan {
    let mut plan = request.regional_plan.clone();
    plan.clock = ClockRange {
        min_offset_s: clock,
        max_offset_s: clock,
        step_s: request.regional_plan.clock.step_s,
    };
    // Location summaries retain the best assignment at EVERY receiver. Keeping
    // only one global hypothesis does not truncate this complete cell inventory.
    plan.retain_hypotheses = 1;
    plan
}
fn valid_limits(limits: &JointLimits) -> bool {
    limits.max_cells > 0
        && limits.max_cells <= MAX_CELLS
        && limits.max_doppler_evaluations > 0
        && limits.max_doppler_evaluations <= regional_search::MAX_EVALUATIONS
        && limits.max_doppler_residual_evaluations > 0
        && limits.max_doppler_residual_evaluations <= regional_search::MAX_RESIDUAL_EVALUATIONS
        && limits.max_doppler_trajectory_point_visits > 0
        && limits.max_doppler_trajectory_point_visits
            <= regional_search::MAX_TRAJECTORY_POINT_VISITS
        && limits.max_envelope_work_units > 0
        && limits.max_envelope_work_units <= thunder_envelope::MAX_WORK_UNITS
        && limits.max_working_memory_bytes > 0
        && limits.max_working_memory_bytes <= MAX_MEMORY_BYTES
        && limits.max_serialized_report_bytes > 0
        && limits.max_serialized_report_bytes <= MAX_REPORT_BYTES
}
fn validate_trajectories(map: &MapData) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    let mut point_count = 0_usize;
    let same_optional = |a: Option<f64>, b: Option<f64>| match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => near(a, b),
        _ => false,
    };
    for track in &map.tracks {
        point_count = point_count
            .checked_add(track.points.len())
            .ok_or("Joint trajectory count overflow")?;
        if point_count > 2_000_000
            || !ids.insert(&track.id)
            || track.id != track.physics_track.flight_id
            || track.points.len() != track.physics_track.points.len()
        {
            return Err(
                "Joint displayed and physical aircraft inventories disagree or exceed bounds"
                    .into(),
            );
        }
        let mut config = serde_json::Map::new();
        if let Some(origin) = track.time_origin_utc_s {
            config.insert("time_origin_utc_s".into(), serde_json::json!(origin));
        }
        if let Some(gap) = track.max_gap_s {
            config.insert("max_gap_s".into(), serde_json::json!(gap));
        }
        let parsed = crate::map_data::parse_value(
            &serde_json::json!({"config":config,"tracks":[track.physics_track]}),
        )?;
        let expected = parsed
            .tracks
            .first()
            .ok_or("Joint trajectory reconstruction is empty")?;
        if track.segments != expected.segments
            || !same_optional(track.time_origin_utc_s, expected.time_origin_utc_s)
            || !same_optional(track.max_gap_s, expected.max_gap_s)
        {
            return Err(
                "Joint trajectory segments or UTC support changed from their native interpretation"
                    .into(),
            );
        }
        for (saved, reconstructed) in track.points.iter().zip(&expected.points) {
            if !near(saved.latitude_deg, reconstructed.latitude_deg)
                || !near(saved.longitude_deg, reconstructed.longitude_deg)
                || saved
                    .height_m
                    .zip(reconstructed.height_m)
                    .is_none_or(|(a, b)| !a.is_finite() || (a - b).abs() > 1e-4)
                || !same_optional(saved.time_s, reconstructed.time_s)
                || !same_optional(saved.time_utc_s, reconstructed.time_utc_s)
            {
                return Err("Joint displayed coordinates/times disagree with their physical aircraft trajectory".into());
            }
        }
    }
    Ok(())
}
fn prepare(request: &JointRequest) -> Result<Prepared, String> {
    if request.schema_version != 1 || !valid_limits(&request.limits) {
        return Err(
            "Joint comparison requires schema1 and positive declared limits within hard caps"
                .into(),
        );
    }
    let document = &request.document;
    if !document.mix.doppler_frequency
        || document.mix.doppler_timing
        || document.mix.thunder_timing
        || document.atmosphere.is_some()
    {
        return Err("This separate joint method requires explicitly selected Doppler frequency only, no picked Doppler/thunder timing and no atmosphere; existing evidence selections are never silently changed".into());
    }
    if document.recording_sha256 != request.observation.recording_sha256
        || document.channel != request.observation.channel
    {
        return Err(
            "Doppler workspace and envelope must refer to the same recording bytes and channel"
                .into(),
        );
    }
    if document
        .envelope_review
        .as_ref()
        .is_some_and(|review| review.observation.as_ref() != Some(&request.observation))
        || document
            .regional_plan
            .as_ref()
            .is_some_and(|plan| plan != &request.regional_plan)
    {
        return Err("Joint observations and approved alternatives must exactly match their saved review drafts when present".into());
    }
    if parse_utc(&document.recording_start_utc)?
        != request.envelope_plan.nominal_recording_start_utc_s
        || document.sound_speed_mps != request.envelope_plan.model.sound_speed_mps
    {
        return Err(
            "Both modalities require the same nominal UTC and sound-speed assumption".into(),
        );
    }
    let merged =
        request.map.source_sha256.is_empty() && request.map.source_path.as_os_str().is_empty();
    let has_original_binding = document.datasets.iter().any(|binding| {
        !binding.path.as_os_str().is_empty()
            && hash_ok(&binding.sha256)
            && binding.sha256 != request.lightning.input_sha256
    });
    if (merged && !has_original_binding)
        || (!merged
            && (!hash_ok(&request.map.source_sha256)
                || request.map.source_path.as_os_str().is_empty()))
    {
        return Err("Joint comparison needs an imported aircraft map or the canonical merged tracks with registered original source bindings".into());
    }
    if !document.datasets.iter().any(|binding| {
        binding.path == request.lightning.input_path
            && binding.sha256 == request.lightning.input_sha256
    }) || (!merged
        && !document.datasets.iter().any(|binding| {
            binding.path == request.map.source_path && binding.sha256 == request.map.source_sha256
        }))
    {
        return Err("Aircraft and lightning originals must be registered with their exact paths and hashes in the frozen workspace".into());
    }
    let (request_hash, request_bytes) = serialized(request, MAX_REQUEST_BYTES)?;
    let clocks = request.regional_plan.clock.values()?;
    let receiver_count = request.envelope_plan.receivers.len();
    let cell_count = receiver_count
        .checked_mul(clocks.len())
        .ok_or("Joint cell count overflow")?;
    if cell_count == 0 || cell_count > request.limits.max_cells || cell_count > MAX_CELLS {
        return Err(format!(
            "Joint comparison needs {cell_count} cells; narrow the shared grid/clock or increase the explicit cell limit within {MAX_CELLS}"
        ));
    }
    if request
        .observation
        .envelope
        .samples
        .first()
        .is_none_or(|s| s.time_s < 0.0)
        || request
            .observation
            .envelope
            .samples
            .last()
            .is_none_or(|s| s.time_s > document.duration_s)
    {
        return Err("Envelope sample centers must lie inside the same recording's duration".into());
    }
    let plan = &request.envelope_plan;
    if plan.keep_best == 0
        || plan.keep_best > thunder_envelope::MAX_KEEP_BEST
        || plan.limits.max_work_units == 0
        || plan.limits.max_work_units > thunder_envelope::MAX_WORK_UNITS
        || plan.limits.max_memory_bytes == 0
        || plan.limits.max_memory_bytes > thunder_envelope::MAX_MEMORY_BYTES
    {
        return Err(
            "Envelope retention and declared budgets must remain within native bounds".into(),
        );
    }
    // Validate the whole temporal support and source semantics once. Native
    // physics preflight is allocation-free; actual work is the tile sum below.
    let mut probe = plan.clone().with_observation(request.observation.clone());
    probe.plan.receivers = vec![plan.receivers[0].clone()];
    probe.plan.limits.max_work_units = thunder_envelope::MAX_WORK_UNITS;
    probe.plan.limits.max_memory_bytes = thunder_envelope::MAX_MEMORY_BYTES;
    let envelope_probe = thunder_envelope::preflight(&probe, &request.lightning)?;
    if !near(request.regional_plan.clock.step_s, envelope_probe.cadence_s)
        || !near(
            request.regional_plan.clock.min_offset_s,
            plan.min_clock_offset_s,
        )
        || !near(
            request.regional_plan.clock.max_offset_s,
            plan.max_clock_offset_s,
        )
        || clocks.len() != envelope_probe.offsets_per_receiver
        || clocks.iter().enumerate().any(|(i, &v)| {
            !near(
                v,
                plan.min_clock_offset_s + i as f64 * envelope_probe.cadence_s,
            )
        })
    {
        return Err("Doppler and envelope require one identical complete clock grid at the measured envelope cadence; endpoints are not rounded or truncated".into());
    }
    let mut receiver_ids = BTreeSet::new();
    let mut receiver_positions = BTreeSet::new();
    let coordinate_bits = |value: f64| {
        if value == 0.0 {
            0.0_f64.to_bits()
        } else {
            value.to_bits()
        }
    };
    for receiver in &plan.receivers {
        if receiver.id.trim().is_empty()
            || receiver.id.len() > 512
            || !receiver_ids.insert(&receiver.id)
        {
            return Err("Receiver IDs must be unique, nonempty and at most512 bytes".into());
        }
        if !receiver.location.ellipsoid_height_m.is_finite()
            || !(-1000.0..=100_000.0).contains(&receiver.location.ellipsoid_height_m)
        {
            return Err("Every joint receiver needs a supported explicit ellipsoid height".into());
        }
        let location = &receiver.location;
        let longitude = if location.latitude_deg.abs() == 90.0 {
            0.0
        } else if location.longitude_deg.abs() == 180.0 {
            180.0
        } else {
            location.longitude_deg
        };
        let position = (
            coordinate_bits(location.latitude_deg),
            coordinate_bits(longitude),
            coordinate_bits(location.ellipsoid_height_m),
        );
        if !receiver_positions.insert(position) {
            return Err("Joint receivers must have distinct physical positions; duplicate coordinates bias the full-domain ranks".into());
        }
    }
    let observers = plan
        .receivers
        .iter()
        .map(|r| r.location.clone())
        .collect::<Vec<_>>();
    let no_lightning = None;
    let regional = regional_search::estimate(
        document,
        &request.map,
        &no_lightning,
        &observers,
        &request.regional_plan,
    )?;
    let active = document.active_scoring_events().collect::<Vec<_>>();
    if active
        .iter()
        .any(|event| event.kind != EventKind::Doppler || event.label.len() > 1024)
    {
        return Err("This method accepts bounded explicitly selected Doppler events only".into());
    }
    let mut assignment_bytes = 0_u64;
    let mut max_event_points = 0_u64;
    let mut max_track_points = 0_u64;
    for event in &request.regional_plan.events {
        let id_bytes = serde_json::to_string(&event.event_id)
            .map_err(|e| e.to_string())?
            .len() as u64;
        let mut source_bytes = 0_u64;
        for id in &event.source_ids {
            let track = request
                .map
                .tracks
                .iter()
                .find(|t| &t.id == id)
                .ok_or("Approved trajectory missing")?;
            if track.physics_track.flight_id != track.id {
                return Err(
                    "Imported trajectory identities disagree with their physical track IDs".into(),
                );
            }
            max_track_points = max_track_points.max(track.physics_track.points.len() as u64);
            source_bytes = source_bytes
                .max(serde_json::to_string(id).map_err(|e| e.to_string())?.len() as u64);
        }
        assignment_bytes = add(assignment_bytes, add(128, add(id_bytes, source_bytes)?)?)?;
    }
    for event in &active {
        max_event_points = max_event_points.max(event.points.len() as u64);
    }
    let tiles = (0..clocks.len())
        .step_by(thunder_envelope::MAX_KEEP_BEST)
        .map(|start| Tile {
            start,
            len: (clocks.len() - start).min(thunder_envelope::MAX_KEEP_BEST),
        })
        .collect::<Vec<_>>();
    let mut envelope_work = 0_u64;
    let mut envelope_memory = 0_u64;
    for &tile in &tiles {
        let child = envelope_tile(request, &clocks, 0, tile);
        let estimate = thunder_envelope::preflight(&child, &request.lightning)?;
        if estimate.offsets_per_receiver != tile.len {
            return Err("Tile lost a declared clock endpoint".into());
        }
        envelope_work = add(
            envelope_work,
            mul(estimate.estimated_work_units, receiver_count as u64)?,
        )?;
        envelope_memory = envelope_memory.max(estimate.estimated_memory_bytes);
        if envelope_work > request.limits.max_envelope_work_units
            || envelope_work > plan.limits.max_work_units
        {
            return Err(format!(
                "Full joint envelope tiles need more than the declared aggregate work budget ({envelope_work} units so far); tile budgets never reset the total"
            ));
        }
    }
    let per_cell_bytes = add(448 + 240, add(128, assignment_bytes)?.max(240))?;
    let receiver_bytes = serialized(&plan.receivers, MAX_REQUEST_BYTES)?.1;
    let report_bytes = add(
        32_768,
        add(
            receiver_bytes,
            add(
                mul(clocks.len() as u64, 32)?,
                mul(cell_count as u64, per_cell_bytes)?,
            )?,
        )?,
    )?;
    let snapshot_bytes = add(add(request_bytes, report_bytes)?, 4096)?;
    // The regional engine has no published memory estimate. Bound its reusable
    // document/source clones, sixteen-receiver residual batches and bounded
    // unavailable diagnostics explicitly instead of presenting zero memory.
    let clones = mul(request_bytes, mul(8, add(active.len() as u64, 2)?)?)?;
    let residual_memory = mul(mul(max_event_points, 16)?, 2048)?;
    let trajectory_memory = mul(mul(max_track_points, 16)?, 256)?;
    let option_memory = mul(
        mul(regional.source_alternatives as u64, 16)?,
        add(1024, assignment_bytes)?,
    )?;
    let source_path_bytes = serialized(&request.map.source_path, MAX_REQUEST_BYTES)?.1;
    let source_path_memory = mul(
        add(source_path_bytes, 256)?,
        regional.source_alternatives as u64,
    )?;
    let regional_memory = add(
        16 * 1024 * 1024,
        add(
            clones,
            add(
                residual_memory,
                add(trajectory_memory, add(option_memory, source_path_memory)?)?,
            )?,
        )?,
    )?;
    let engine_memory = regional_memory.max(envelope_memory);
    let rank_memory = mul(cell_count as u64, 128)?;
    let working_memory = add(engine_memory, add(mul(report_bytes, 2)?, rank_memory)?)?;
    let log = (cell_count.max(2) as u64).ilog2() as u64 + 1;
    let ranking_work = mul(cell_count as u64, add(mul(log, 4)?, 16)?)?;
    if regional.evaluations > request.limits.max_doppler_evaluations
        || regional.residual_evaluations > request.limits.max_doppler_residual_evaluations
        || regional.trajectory_point_visits > request.limits.max_doppler_trajectory_point_visits
        || report_bytes > request.limits.max_serialized_report_bytes
        || working_memory > request.limits.max_working_memory_bytes
        || envelope_memory > plan.limits.max_memory_bytes
    {
        let (report_required, report_allowed) = crate::data_size::format_gib_pair(
            report_bytes,
            request.limits.max_serialized_report_bytes,
        );
        let (memory_required, memory_allowed) = crate::data_size::format_gib_pair(
            working_memory,
            request.limits.max_working_memory_bytes,
        );
        return Err(format!(
            "Joint aggregate limits exceeded: Doppler {} evaluations/{} residuals/{} track visits, envelope {envelope_work} units, report bound {report_required} / {report_allowed} allowed, working-memory bound {memory_required} / {memory_allowed} allowed; explicitly narrow data or increase declared limits within hard caps",
            regional.evaluations, regional.residual_evaluations, regional.trajectory_point_visits
        ));
    }
    validate_trajectories(&request.map)?;
    let estimate = JointEstimate {
        request_sha256: request_hash,
        request_bytes,
        receivers: receiver_count,
        clocks: clocks.len(),
        cells: cell_count,
        doppler_runs: clocks.len(),
        envelope_runs: receiver_count
            .checked_mul(tiles.len())
            .ok_or("Tile count overflow")?,
        doppler_evaluations: regional.evaluations,
        doppler_residual_evaluations: regional.residual_evaluations,
        doppler_trajectory_point_visits: regional.trajectory_point_visits,
        envelope_work_units: envelope_work,
        peak_engine_memory_bytes: engine_memory,
        working_memory_bound_bytes: working_memory,
        serialized_report_bound_bytes: report_bytes,
        serialized_snapshot_bound_bytes: snapshot_bytes,
        ranking_work_bound: ranking_work,
    };
    Ok(Prepared {
        estimate,
        observers,
        clocks,
        tiles,
    })
}

/// Pure bounded preflight. No files, network, GUI, or evidence mutations.
pub fn estimate(request: &JointRequest) -> Result<JointEstimate, String> {
    prepare(request).map(|p| p.estimate)
}

fn midranks(values: &[f64], tolerance: f64, cancel: &AtomicBool) -> Option<Vec<f64>> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let mut order = (0..values.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| values[a].total_cmp(&values[b]).then(a.cmp(&b)));
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let mut ranks = vec![0.0; values.len()];
    let mut start = 0;
    while start < order.len() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let mut end = start + 1;
        // Conservative adjacent-transitive closure matches the frozen pilot.
        // A chain's complete range may exceed tolerance; it is not split by an
        // arbitrary first-member anchor or source/candidate ordering.
        while end < order.len() && values[order[end]] - values[order[end - 1]] <= tolerance {
            if end.is_multiple_of(1024) && cancel.load(Ordering::Relaxed) {
                return None;
            }
            end += 1;
        }
        let rank = ((start + 1) as f64 + end as f64) / 2.0;
        for &index in &order[start..end] {
            ranks[index] = rank;
        }
        start = end;
    }
    Some(ranks)
}
fn full_ranks(cells: &[JointCell], cancel: &AtomicBool) -> Option<Vec<JointCellRanks>> {
    let mut d = Vec::with_capacity(cells.len());
    let mut e = Vec::with_capacity(cells.len());
    for cell in cells {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        match (&cell.doppler, &cell.envelope) {
            (
                DopplerState::Available { score, .. },
                EnvelopeState::Available {
                    cosine_similarity, ..
                },
            ) => {
                d.push(*score);
                e.push(-*cosine_similarity);
            }
            _ => return None,
        }
    }
    let dr = midranks(&d, DOPPLER_TOLERANCE, cancel)?;
    let er = midranks(&e, ENVELOPE_TOLERANCE, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    Some(
        dr.into_iter()
            .zip(er)
            .map(|(doppler, envelope)| JointCellRanks {
                doppler,
                envelope,
                combined: doppler.max(envelope),
            })
            .collect(),
    )
}
fn available(cell: &JointCell) -> bool {
    matches!(cell.doppler, DopplerState::Available { .. })
        && matches!(cell.envelope, EnvelopeState::Available { .. })
}
fn limitations() -> Vec<String> {
    vec![
        "This optional method compares the same recording/channel, receiver list and complete shared clock grid. Only reviewed Doppler frequency and a separately selected complete envelope are used; picked timing and atmosphere are not silently added or removed.".into(),
        "Every Doppler cell profiles only the explicitly approved aircraft alternatives and emitted-tone bounds. Every envelope cell retains native source/height/energy-kernel assumptions. A high score or nearby point does not establish the correct aircraft, lightning catalog or clock.".into(),
        "Full-domain adjacent-transitive midpoint ranks use Doppler tolerance1e-10 and envelope tolerance1e-7. Combined cost is the maximum of those two ranks before geographic clock profiling; lower is better. This is an uncalibrated ordering, not likelihood, confidence or automatic acceptance.".into(),
        "Missing, cancelled or numerically unavailable modality cells prevent all rank fusion. Completed individual values are preserved without substituting the other modality or silently changing the comparison domain.".into(),
        "Envelope clocks are evaluated in native tiles of at most40, retaining and directly recomputing every available score. Source-row counts describe support over their tile, not verified per-cell acoustic associations. Near-equal floating-point results can differ across tiles; conservative rank ties remain explicit.".into(),
        "The frozen synthetic pilot found some shared receiver-clock shortlist benefit, geographic losses and wrong-catalog harm. Frequency separation does not establish independent evidence; no independent field accuracy, calibrated conflict threshold or general hybrid improvement is established.".into(),
        "Memory and serialized-size preflight bounds include wrapper results, ranking arrays and conservative regional-engine allocations. Counters aggregate all native calls before work; tile limits never reset the total budget.".into(),
    ]
}
fn new_report(request: &JointRequest, prepared: &Prepared) -> Result<JointReport, String> {
    let mut cells = Vec::with_capacity(prepared.estimate.cells);
    let [south, west, north, east] = request.document.map.bounds;
    for (receiver_index, receiver) in request.envelope_plan.receivers.iter().enumerate() {
        for (clock_index, &clock_offset_s) in prepared.clocks.iter().enumerate() {
            cells.push(JointCell {
                receiver_index,
                clock_index,
                clock_offset_s,
                clock_at_boundary: clock_index == 0 || clock_index + 1 == prepared.clocks.len(),
                spatial_at_boundary: at_boundary(receiver.location.latitude_deg, [south, north])
                    || at_boundary(receiver.location.longitude_deg, [west, east]),
                doppler: DopplerState::NotStarted,
                envelope: EnvelopeState::NotStarted,
                ranks: None,
            });
        }
    }
    Ok(JointReport {
        schema_version: 1,
        request_sha256: prepared.estimate.request_sha256.clone(),
        document_sha256: regional_search::document_sha256(&request.document)?,
        inputs_sha256: inputs_sha256(request)?,
        receivers: request.envelope_plan.receivers.clone(),
        clock_offsets_s: prepared.clocks.clone(),
        nominal_recording_start_utc_s: request.envelope_plan.nominal_recording_start_utc_s,
        estimate: prepared.estimate.clone(),
        progress: JointProgress {
            stage: JointStage::Doppler,
            completed_doppler_runs: 0,
            total_doppler_runs: prepared.estimate.doppler_runs,
            completed_envelope_runs: 0,
            total_envelope_runs: prepared.estimate.envelope_runs,
            completed_doppler_cells: 0,
            completed_envelope_cells: 0,
            total_cells: prepared.estimate.cells,
        },
        status: JointStatus::Completed,
        all_cells_available: false,
        ranking_available: false,
        cells,
        location_verified: false,
        independent_field_validation: false,
        limitations: limitations(),
    })
}
fn frequency_boundary(request: &JointRequest, assignments: &[LocationAssignment]) -> bool {
    assignments.iter().any(|assignment| {
        request
            .regional_plan
            .events
            .iter()
            .find(|e| e.event_id == assignment.event_id)
            .and_then(|e| e.emitted_frequency_hz)
            .zip(assignment.emitted_frequency_hz)
            .is_some_and(|([lo, hi], value)| {
                lo != hi
                    && (value - lo).abs().min((value - hi).abs())
                        <= 1e-9_f64.max(value.abs() * 1e-12)
            })
    })
}

/// Execute the bounded complete domain. Callback counters may advance inside an
/// engine call; terminal cells preserve only the engine's completed outcomes.
pub fn run(
    request: &JointRequest,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&JointProgress),
) -> Result<JointReport, String> {
    let prepared = prepare(request)?;
    let mut report = new_report(request, &prepared)?;
    progress(&report.progress);
    let clock_count = prepared.clocks.len();
    for (clock_index, &clock) in prepared.clocks.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let base_progress = report.progress.clone();
        let child = regional_search::run(
            &request.document,
            &request.map,
            &None,
            &prepared.observers,
            &clock_plan(request, clock),
            cancel,
            |p| {
                let mut current = base_progress.clone();
                current.completed_doppler_cells += p.completed_cells as usize;
                progress(&current);
            },
        );
        let mut completed = 0;
        match child {
            Ok(child) => {
                if child.location_summaries.len() != prepared.observers.len() {
                    return Err("Native Doppler result lost the complete receiver inventory".into());
                }
                for (index, location) in child.location_summaries.iter().enumerate() {
                    if location.completed_clocks == 0 {
                        continue;
                    }
                    if location.completed_clocks != 1
                        || location.observer_index != index
                        || !same_location(&location.observer, &prepared.observers[index])
                    {
                        return Err("Native Doppler cell identity or completion changed".into());
                    }
                    let state = if let Some(score) = location.best_score {
                        if !score.is_finite()
                            || score < 0.0
                            || location.best_clock_offset_s != Some(clock)
                            || location.best_assignments.len() != request.regional_plan.events.len()
                        {
                            return Err("Native Doppler cell returned an invalid score or assignment inventory".into());
                        }
                        DopplerState::Available {
                            score,
                            assignments: location.best_assignments.clone(),
                            frequency_at_boundary: frequency_boundary(
                                request,
                                &location.best_assignments,
                            ),
                        }
                    } else {
                        let detail = child.unavailable.iter().find(|u| u.observer_index == index)
                            .and_then(|u| u.reasons.first()).map(String::as_str)
                            .unwrap_or("No complete approved aircraft assignment supports every selected measured frequency point");
                        DopplerState::Unavailable {
                            reason: reason(detail),
                        }
                    };
                    report.cells[index * clock_count + clock_index].doppler = state;
                    completed += 1;
                }
            }
            Err(error) => {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                for index in 0..prepared.observers.len() {
                    report.cells[index * clock_count + clock_index].doppler =
                        DopplerState::Unavailable {
                            reason: reason(&error),
                        };
                    completed += 1;
                }
            }
        }
        report.progress.completed_doppler_cells += completed;
        if completed == prepared.observers.len() {
            report.progress.completed_doppler_runs += 1;
        }
        progress(&report.progress);
        if completed < prepared.observers.len() {
            break;
        }
    }
    report.progress.stage = JointStage::Envelope;
    progress(&report.progress);
    'envelope: for receiver in 0..prepared.observers.len() {
        for &tile in &prepared.tiles {
            if cancel.load(Ordering::Relaxed) {
                break 'envelope;
            }
            let child_request = envelope_tile(request, &prepared.clocks, receiver, tile);
            let base_progress = report.progress.clone();
            let child = thunder_envelope::search(&child_request, &request.lightning, cancel, |p| {
                let mut current = base_progress.clone();
                current.completed_envelope_cells += p.evaluated_offsets as usize;
                progress(&current);
            });
            match child {
                Ok(child) => {
                    if !child.complete {
                        if child.cancelled || cancel.load(Ordering::Relaxed) {
                            break 'envelope;
                        }
                        return Err(
                            "Native envelope returned an incomplete uncancelled tile".into()
                        );
                    }
                    if child.receivers.len() != 1
                        || child.estimate.offsets_per_receiver != tile.len
                        || child.progress.evaluated_offsets as usize != tile.len
                    {
                        return Err(
                            "Native envelope tile lost declared receiver-clock cells".into()
                        );
                    }
                    let summary = &child.receivers[0];
                    if child.hypotheses.len() != summary.rankable_offsets
                        || summary.rankable_offsets
                            + summary.zero_energy_offsets
                            + summary.numerically_unresolved_offsets
                            != tile.len
                    {
                        return Err("Native envelope tile truncated supported clocks".into());
                    }
                    let missing = reason(&format!(
                        "Native envelope withheld this offset; tile contains {} zero-energy and {} numerically unresolved offsets",
                        summary.zero_energy_offsets, summary.numerically_unresolved_offsets
                    ));
                    for local in 0..tile.len {
                        report.cells[receiver * clock_count + tile.start + local].envelope =
                            EnvelopeState::Unavailable {
                                reason: missing.clone(),
                            };
                    }
                    let mut seen = BTreeSet::new();
                    for hypothesis in &child.hypotheses {
                        if hypothesis.receiver_index != 0
                            || hypothesis.offset_index >= tile.len
                            || !seen.insert(hypothesis.offset_index)
                            || hypothesis.receiver_id
                                != request.envelope_plan.receivers[receiver].id
                            || !same_location(&hypothesis.observer, &prepared.observers[receiver])
                            || !near(
                                hypothesis.clock_offset_s,
                                prepared.clocks[tile.start + hypothesis.offset_index],
                            )
                            || !hypothesis.cosine_similarity.is_finite()
                            || !(0.0..=1.0).contains(&hypothesis.cosine_similarity)
                        {
                            return Err(
                                "Native envelope tile returned a mismatched or invalid cell".into(),
                            );
                        }
                        report.cells
                            [receiver * clock_count + tile.start + hypothesis.offset_index]
                            .envelope = EnvelopeState::Available {
                            cosine_similarity: hypothesis.cosine_similarity,
                            contributing_source_rows: hypothesis.contributing_source_rows,
                            distance_excluded_source_rows: hypothesis.distance_excluded_source_rows,
                        };
                    }
                }
                Err(error) => {
                    if cancel.load(Ordering::Relaxed) {
                        break 'envelope;
                    }
                    for local in 0..tile.len {
                        report.cells[receiver * clock_count + tile.start + local].envelope =
                            EnvelopeState::Unavailable {
                                reason: reason(&error),
                            };
                    }
                }
            }
            report.progress.completed_envelope_cells += tile.len;
            report.progress.completed_envelope_runs += 1;
            progress(&report.progress);
        }
    }
    report.all_cells_available = report.cells.iter().all(available);
    report.progress.stage = JointStage::Ranking;
    progress(&report.progress);
    if report.all_cells_available
        && let Some(ranks) = full_ranks(&report.cells, cancel)
        && !cancel.load(Ordering::Relaxed)
    {
        for (cell, rank) in report.cells.iter_mut().zip(ranks) {
            cell.ranks = Some(rank);
        }
        report.ranking_available = true;
    }
    report.status = if cancel.load(Ordering::Relaxed)
        || report.progress.completed_doppler_cells < prepared.estimate.cells
        || report.progress.completed_envelope_cells < prepared.estimate.cells
    {
        JointStatus::Cancelled
    } else {
        JointStatus::Completed
    };
    report.progress.stage = JointStage::Finished;
    serialized(&report, request.limits.max_serialized_report_bytes)?;
    progress(&report.progress);
    Ok(report)
}

/// Validate an offline snapshot's identities, bounded cell inventory, native
/// assignment eligibility, execution counters and full-domain rank arithmetic.
/// This does not reopen original files or claim to rerun either physical model;
/// original-byte verification and explicit replay remain separate operations.
pub fn validate_report(request: &JointRequest, report: &JointReport) -> Result<(), String> {
    let (_, actual_bytes) = serialized(
        report,
        request
            .limits
            .max_serialized_report_bytes
            .min(MAX_REPORT_BYTES),
    )?;
    let prepared = prepare(request)?;
    let expected = &prepared.estimate;
    if report.schema_version != 1
        || report.request_sha256 != expected.request_sha256
        || report.document_sha256 != regional_search::document_sha256(&request.document)?
        || report.inputs_sha256 != inputs_sha256(request)?
        || report.estimate != *expected
        || actual_bytes > expected.serialized_report_bound_bytes
        || report.location_verified
        || report.independent_field_validation
        || report.limitations != limitations()
    {
        return Err(
            "Joint snapshot identity, estimate, size or unverified-method declaration changed"
                .into(),
        );
    }
    if report.receivers.len() != expected.receivers
        || report.clock_offsets_s != prepared.clocks
        || report.nominal_recording_start_utc_s
            != request.envelope_plan.nominal_recording_start_utc_s
        || report.cells.len() != expected.cells
        || report
            .receivers
            .iter()
            .zip(&request.envelope_plan.receivers)
            .any(|(a, b)| a.id != b.id || !same_location(&a.location, &b.location))
    {
        return Err(
            "Joint snapshot receiver or clock domain differs from its frozen request".into(),
        );
    }
    let clocks = expected.clocks;
    let [south, west, north, east] = request.document.map.bounds;
    let mut doppler_cells = 0;
    let mut envelope_cells = 0;
    for (index, cell) in report.cells.iter().enumerate() {
        let receiver = index / clocks;
        let clock = index % clocks;
        let location = &prepared.observers[receiver];
        if cell.receiver_index != receiver
            || cell.clock_index != clock
            || cell.clock_offset_s != prepared.clocks[clock]
            || cell.clock_at_boundary != (clock == 0 || clock + 1 == clocks)
            || cell.spatial_at_boundary
                != (at_boundary(location.latitude_deg, [south, north])
                    || at_boundary(location.longitude_deg, [west, east]))
        {
            return Err(
                "Joint cell ordering, coordinate/clock identity or boundary flags changed".into(),
            );
        }
        match &cell.doppler {
            DopplerState::NotStarted => {}
            DopplerState::Unavailable { reason: detail } => {
                if detail != &reason(detail) {
                    return Err("Joint unavailable reason is not bounded plain text".into());
                }
                doppler_cells += 1;
            }
            DopplerState::Available {
                score,
                assignments,
                frequency_at_boundary,
            } => {
                if !score.is_finite()
                    || *score < 0.0
                    || assignments.len() != request.regional_plan.events.len()
                    || *frequency_at_boundary != frequency_boundary(request, assignments)
                {
                    return Err("Joint Doppler score or assignment inventory is invalid".into());
                }
                let mut seen = BTreeSet::new();
                for assignment in assignments {
                    let alternative = request
                        .regional_plan
                        .events
                        .iter()
                        .find(|p| p.event_id == assignment.event_id)
                        .ok_or("Joint assignment names an unapproved event")?;
                    let [lo, hi] = alternative
                        .emitted_frequency_hz
                        .ok_or("Joint frequency bounds missing")?;
                    if assignment.kind != EventKind::Doppler
                        || !seen.insert(&assignment.event_id)
                        || !alternative.source_ids.contains(&assignment.source_id)
                        || assignment
                            .emitted_frequency_hz
                            .is_none_or(|f| !f.is_finite() || f < lo || f > hi)
                    {
                        return Err("Joint assignment changed its approved source, event or emitted frequency bounds".into());
                    }
                }
                doppler_cells += 1;
            }
        }
        match &cell.envelope {
            EnvelopeState::NotStarted => {}
            EnvelopeState::Unavailable { reason: detail } => {
                if detail != &reason(detail) {
                    return Err("Joint unavailable reason is not bounded plain text".into());
                }
                envelope_cells += 1;
            }
            EnvelopeState::Available {
                cosine_similarity,
                contributing_source_rows,
                distance_excluded_source_rows,
            } => {
                if !cosine_similarity.is_finite()
                    || !(0.0..=1.0).contains(cosine_similarity)
                    || *contributing_source_rows == 0
                    || contributing_source_rows
                        .checked_add(*distance_excluded_source_rows)
                        .is_none_or(|count| count > request.lightning.strikes.len())
                {
                    return Err("Joint envelope score or source-row support is invalid".into());
                }
                envelope_cells += 1;
            }
        }
    }
    let mut doppler_runs = 0;
    let mut doppler_stopped = false;
    for clock in 0..clocks {
        let attempted = (0..expected.receivers)
            .filter(|&r| {
                !matches!(
                    report.cells[r * clocks + clock].doppler,
                    DopplerState::NotStarted
                )
            })
            .count();
        if doppler_stopped && attempted > 0 {
            return Err("Joint Doppler outcomes appear after an unfinished clock run".into());
        }
        if attempted == expected.receivers {
            doppler_runs += 1;
        } else {
            doppler_stopped = true;
        }
    }
    let mut envelope_runs = 0;
    let mut envelope_stopped = false;
    for receiver in 0..expected.receivers {
        for tile in &prepared.tiles {
            let attempted = (tile.start..tile.start + tile.len)
                .filter(|&c| {
                    !matches!(
                        report.cells[receiver * clocks + c].envelope,
                        EnvelopeState::NotStarted
                    )
                })
                .count();
            if attempted > 0 && (attempted != tile.len || envelope_stopped) {
                return Err(
                    "Joint envelope contains a partial tile or outcomes after an unfinished tile"
                        .into(),
                );
            }
            if attempted == tile.len {
                envelope_runs += 1;
            } else {
                envelope_stopped = true;
            }
        }
    }
    if envelope_cells > 0 && doppler_cells != expected.cells {
        return Err("Joint envelope work cannot precede unfinished Doppler work".into());
    }
    let expected_progress = JointProgress {
        stage: JointStage::Finished,
        completed_doppler_runs: doppler_runs,
        total_doppler_runs: expected.doppler_runs,
        completed_envelope_runs: envelope_runs,
        total_envelope_runs: expected.envelope_runs,
        completed_doppler_cells: doppler_cells,
        completed_envelope_cells: envelope_cells,
        total_cells: expected.cells,
    };
    if report.progress != expected_progress
        || (report.status == JointStatus::Completed
            && (doppler_cells != expected.cells || envelope_cells != expected.cells))
    {
        return Err(
            "Joint execution status or progress does not match its retained outcomes".into(),
        );
    }
    let all_available = report.cells.iter().all(available);
    let ranked = report.cells.iter().filter(|c| c.ranks.is_some()).count();
    if report.all_cells_available != all_available
        || report.ranking_available != (ranked == expected.cells)
        || (ranked != 0 && ranked != expected.cells)
        || (ranked != 0 && !all_available)
        || (all_available && report.status == JointStatus::Completed && !report.ranking_available)
    {
        return Err("Joint full-domain ranking availability is inconsistent with its cells".into());
    }
    if report.ranking_available {
        let ranks = full_ranks(&report.cells, &AtomicBool::new(false))
            .ok_or("Joint full-domain ranks cannot be reconstructed")?;
        if report
            .cells
            .iter()
            .zip(ranks)
            .any(|(cell, rank)| cell.ranks.as_ref() != Some(&rank))
        {
            return Err(
                "Joint ranks differ from full-domain adjacent-tie midpoint minimax arithmetic"
                    .into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_ties_close_transitively_before_midpoint_ranking() {
        let ranks = midranks(&[5.0, 0.0, 0.75, 1.5, 5.0], 1.0, &AtomicBool::new(false)).unwrap();
        assert_eq!(ranks, [4.5, 2.0, 2.0, 2.0, 4.5]);
        assert!(midranks(&[1.0], 0.0, &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn bounded_reasons_preserve_utf8_and_remove_json_expansion() {
        let value = reason(&format!("\n\"{}\\", "雷".repeat(100)));
        assert!(value.len() <= REASON_BYTES);
        assert_eq!(reason(&value), value);
        assert_eq!(reason("\n\t"), "Native comparison unavailable");
    }
}
