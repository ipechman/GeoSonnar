//! Bounded wide-area two-pass Doppler search. Storage batches do not change the
//! source pool, receiver lattice, local-CPA pairing, or shared-clock physics.
use crate::{
    evidence_score::ObserverLocation,
    physics::{self, ClosestApproach, EventTrace, PairConfig, PairScore},
    receiver_grid::GridLayout,
    track_store::{PassMetadata, TrackStore, TrackStoreBinding},
    verification::CoarseBounds,
    wide_area_journal::{Journal, PageReceipt},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

pub const MAX_RETAIN: usize = 1000;
pub const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_REPORT_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENT_POINTS: usize = 10_000;
const MAX_CPA_ENTRIES: usize = 250_000;
pub const DEFAULT_INDEX_MEMORY_BYTES: u64 = 512 * 1024 * 1024;

fn default_index_memory_bytes() -> u64 {
    DEFAULT_INDEX_MEMORY_BYTES
}
fn is_default_index_memory_bytes(value: &u64) -> bool {
    *value == DEFAULT_INDEX_MEMORY_BYTES
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WideBudgets {
    pub max_work_units: u64,
    pub max_output_bytes: u64,
    pub max_duration_s: u64,
    /// Combined source metadata, spatial-index construction/query and retained
    /// local-CPA memory. Separate from the track cache and native model scratch.
    /// Omitting the default preserves hashes of pre-existing request journals.
    #[serde(
        default = "default_index_memory_bytes",
        skip_serializing_if = "is_default_index_memory_bytes"
    )]
    pub max_index_memory_bytes: u64,
}
impl Default for WideBudgets {
    fn default() -> Self {
        Self {
            max_work_units: 250_000_000,
            max_output_bytes: 256 * 1024 * 1024,
            max_duration_s: 1800,
            max_index_memory_bytes: DEFAULT_INDEX_MEMORY_BYTES,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidePairRequest {
    pub schema_version: u32,
    pub document_sha256: String,
    pub recording_sha256: String,
    pub channel: usize,
    pub grid: GridLayout,
    pub source: TrackStoreBinding,
    pub events: [EventTrace; 2],
    pub pair_config: PairConfig,
    pub coarse_bounds: [CoarseBounds; 2],
    pub measured_separation_s: f64,
    pub separation_tolerance_s: f64,
    pub budgets: WideBudgets,
    pub retain: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WideEstimate {
    pub receiver_count: u64,
    pub source_passes: u64,
    pub source_points: u64,
    pub unpruned_pass_checks: u64,
    pub worst_scan_point_visits: u64,
    pub minimum_output_bytes: u64,
    pub source_time_origin_utc_s: f64,
    /// Exact lower bound under the current accounting, using the shortest
    /// source pass twice (one continuous aircraft may supply distinct passes).
    pub minimum_work_units_per_timing_pair: u64,
    /// Optimistic ceiling before any index, census or other work is charged.
    pub maximum_timing_pairs_before_scan: u64,
    /// Verified-store reservation, including retained metadata and final source
    /// verification scratch; this is not a measured process-RAM value.
    #[serde(default)]
    pub source_metadata_bytes: u64,
    #[serde(default)]
    pub spatial_index_memory_bytes: u64,
    #[serde(default)]
    pub local_cpa_memory_reserve_bytes: u64,
    #[serde(default)]
    pub required_index_memory_bytes: u64,
    pub limitations: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WideProgress {
    pub total_receivers: u64,
    pub completed_receivers: u64,
    pub scanned_passes: u64,
    pub potential_pass_checks: u64,
    pub bbox_excluded_passes: u64,
    pub timing_candidates: u64,
    pub scored_candidates: u64,
    pub work_units: u64,
    pub stage: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WideStatus {
    Completed,
    Cancelled,
    BudgetExceeded,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WideCandidate {
    pub receiver_index: u64,
    pub observer: ObserverLocation,
    pub score: PairScore,
    pub encounter_index: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WidePairReport {
    pub schema_version: u32,
    pub request_sha256: String,
    pub source: TrackStoreBinding,
    /// None when preparation stopped before the source was verified.
    pub source_time_origin_utc_s: Option<f64>,
    pub status: WideStatus,
    pub progress: WideProgress,
    pub best_hypotheses: Vec<WideCandidate>,
    pub best_flight_pairs: Vec<WideCandidate>,
    pub retain: usize,
    pub retained_truncated: bool,
    pub pair_retention_truncated: bool,
    pub local_cpa_count: u64,
    pub boundary_cpa_rejections: u64,
    pub model_rejections: u64,
    pub scoring_rejections: u64,
    pub output_directory: PathBuf,
    pub pages: Vec<PageReceipt>,
    pub error: Option<String>,
    pub location_verified: bool,
    pub source_verified_at_end: bool,
    pub limitations: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReceiverResult {
    pub receiver_index: u64,
    pub observer: ObserverLocation,
    pub status: String,
    pub eligible_event_cpas: [usize; 2],
    pub timing_candidates: u64,
    pub scored_candidates: u64,
    pub model_rejections: u64,
    pub scoring_rejections: u64,
    pub best: Option<WideCandidate>,
}

pub fn request_sha256(request: &WidePairRequest) -> Result<String, String> {
    let bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("Wide-area request exceeds 4 MiB".into());
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn mul(a: u64, b: u64) -> Result<u64, String> {
    a.checked_mul(b)
        .ok_or_else(|| "Wide-area work count overflow".into())
}
pub(crate) fn validate_request(request: &WidePairRequest) -> Result<(), String> {
    if request.schema_version != 1
        || !valid_hash(&request.document_sha256)
        || !valid_hash(&request.recording_sha256)
        || !valid_hash(&request.source.manifest_sha256)
        || request.channel > 255
        || request.retain == 0
        || request.retain > MAX_RETAIN
    {
        return Err("Invalid wide-area identity, channel or retention limit".into());
    }
    request.grid.validate()?;
    request_sha256(request)?;
    let b = &request.budgets;
    if b.max_work_units == 0
        || b.max_work_units > 100_000_000_000
        || b.max_output_bytes < 16 * 1024 * 1024
        || b.max_output_bytes > 8 * 1024 * 1024 * 1024
        || b.max_duration_s == 0
        || b.max_duration_s > 86400
        || !(64 * 1024 * 1024..=4 * 1024 * 1024 * 1024).contains(&b.max_index_memory_bytes)
    {
        return Err(
            "Declare positive work, 16 MiB..8 GiB output, 1..86400 s wall and 64 MiB..4 GiB index-memory limits".into(),
        );
    }
    let config = &request.pair_config;
    if !config.event1_cpa_recording_s.is_finite()
        || config.event1_cpa_recording_s < 0.0
        || config.shared_shifts_s.is_empty()
        || config.shared_shifts_s.len() > 1001
        || config.shared_shifts_s.iter().any(|v| !v.is_finite())
        || !config.model.sound_speed_mps.is_finite()
        || config.model.sound_speed_mps <= 0.0
        || !config.model.max_gap_s.is_finite()
        || config.model.max_gap_s <= 0.0
        || config
            .clock_offset_bounds_s
            .is_none_or(|[a, b]| !a.is_finite() || !b.is_finite() || a > b)
    {
        return Err("Two-pass search needs explicit finite common recording-start bounds, CPA, shared shifts and constant propagation settings".into());
    }
    for event in &request.events {
        if !(3..=MAX_EVENT_POINTS).contains(&event.points.len())
            || !(1..=1001).contains(&event.f0_hz.len())
            || event.f0_hz.iter().any(|f| !f.is_finite() || *f <= 0.0)
            || event.points.iter().any(|p| {
                !p.t_rec_s.is_finite()
                    || p.t_rec_s < 0.0
                    || !p.frequency_hz.is_finite()
                    || p.frequency_hz <= 0.0
            })
            || event
                .points
                .windows(2)
                .any(|w| w[0].t_rec_s >= w[1].t_rec_s)
        {
            return Err("Two fixed reviewed traces need 3..10000 ordered points and 1..1001 explicit positive emitted tones".into());
        }
    }
    if request.events[0].points.last().unwrap().t_rec_s >= request.events[1].points[0].t_rec_s
        || !request.measured_separation_s.is_finite()
        || request.measured_separation_s <= 0.0
        || !request.separation_tolerance_s.is_finite()
        || request.separation_tolerance_s < 0.0
    {
        return Err("Events must be ordered and disjoint with a positive measured separation and nonnegative tolerance".into());
    }
    for bounds in &request.coarse_bounds {
        for [a, b] in [bounds.speed_mps, bounds.slant_distance_m] {
            if !a.is_finite() || !b.is_finite() || a < 0.0 || a > b {
                return Err("Invalid declared coarse speed/slant bounds".into());
            }
        }
    }
    Ok(())
}
fn estimate_store(request: &WidePairRequest, store: &TrackStore) -> Result<WideEstimate, Stop> {
    validate_request(request)?;
    if store.binding().manifest_sha256 != request.source.manifest_sha256
        || store.passes().is_empty()
    {
        return Err("Changed or empty indexed aircraft source".into());
    }
    let memory = IndexMemoryPlan::new(
        store.resident_memory_bytes(),
        store.passes().len(),
        store.total_points(),
        store
            .passes()
            .iter()
            .map(|pass| pass.point_count)
            .max()
            .unwrap_or(0),
    )?;
    memory.require_budget(request.budgets.max_index_memory_bytes)?;
    let origin = store.time_origin_utc_s();
    let [a, b] = request.pair_config.clock_offset_bounds_s.unwrap();
    if !origin.is_finite()
        || [origin + a, origin + b]
            .iter()
            .any(|v| !(0.0..253_402_300_800.0).contains(v))
    {
        return Err("Declared common recording-start bounds are outside UTC support".into());
    }
    let minimum_output_bytes = mul(request.grid.count, 96)?
        .checked_add(16 * 1024 * 1024)
        .ok_or("Output estimate overflow")?;
    if minimum_output_bytes > request.budgets.max_output_bytes
        || request.grid.count > request.budgets.max_work_units
    {
        return Err("The full declared grid cannot fit the explicit minimum work/output budget; no receivers were removed".into());
    }
    let shortest_pass = store
        .passes()
        .iter()
        .map(|pass| pass.point_count as u64)
        .min()
        .ok_or("Empty source pool")?;
    let minimum_work_units_per_timing_pair =
        pair_work(request, mul(shortest_pass, 2)?, trace_work(request)?)?;
    Ok(WideEstimate { receiver_count:request.grid.count,source_passes:store.passes().len() as u64,
        source_points:store.total_points(),unpruned_pass_checks:mul(request.grid.count,store.passes().len() as u64)?,
        worst_scan_point_visits:mul(request.grid.count,store.total_points())?,minimum_output_bytes,
        source_time_origin_utc_s:origin,minimum_work_units_per_timing_pair,
        source_metadata_bytes:memory.source_metadata_bytes,spatial_index_memory_bytes:memory.spatial_index_memory_bytes,
        local_cpa_memory_reserve_bytes:memory.local_cpa_memory_reserve_bytes,required_index_memory_bytes:memory.total_bytes,
        maximum_timing_pairs_before_scan:request.budgets.max_work_units/minimum_work_units_per_timing_pair,limitations:vec![
            "Unpruned scan counts are conservative upper bounds, not a completion-time promise. Actual pair work is counted after the local-CPA separation join and charged to the same global budget.".into(),
            "The affordable-pair ceiling spends the entire work budget on the shortest possible pair. Actual capacity is lower after index traversal, pass census and longer tracks. It is not an ETA, required-pair estimate or guarantee that this grid can finish.".into(),
            "The global equal-angle receiver lattice includes every boundary; displayed previews may be sparse. All physics uses full indexed continuous passes and fixed trace samples.".into(),
            "Index memory is a conservative allocation budget for verified source metadata/verification scratch, spatial-index construction/query and the bounded local-CPA inventory/working arrays. It is separate from the 256 MiB track cache, other native pass/model preparation and report buffers, and is not an OS process-RAM hard limit. No source-count cutoff or silent pruning is used.".into(),
        ] })
}
pub fn estimate(request: &WidePairRequest) -> Result<WideEstimate, String> {
    estimate_with_cancel(request, &AtomicBool::new(false), |_| {})
}
pub fn estimate_with_cancel<F: FnMut(WideProgress)>(
    request: &WidePairRequest,
    cancel: &AtomicBool,
    mut callback: F,
) -> Result<WideEstimate, String> {
    let started = Instant::now();
    validate_request(request)?;
    let mut progress = WideProgress {
        total_receivers: request.grid.count,
        stage: "verifying_source".into(),
        ..Default::default()
    };
    let mut guard = Guard {
        start: started,
        cancel,
        budgets: &request.budgets,
    };
    let mut stopped = None;
    let store = TrackStore::open_with_memory_limit(
        &request.source.manifest_path,
        request.budgets.max_index_memory_bytes,
        || source_checkpoint(&mut guard, &mut progress, &mut callback, &mut stopped),
    )?;
    let result = estimate_store(request, &store).map_err(|stop| stop.reason)?;
    guard.charge(0, &mut progress).map_err(|stop| stop.reason)?;
    // open already verified every source byte and all index geometry. The
    // estimate only reads this verified snapshot; execution opens it anew.
    progress.stage = "estimated".into();
    callback(progress);
    Ok(result)
}

fn trace_work(request: &WidePairRequest) -> Result<u64, String> {
    let work = request.events.iter().try_fold(0u64, |sum, event| {
        let work = mul(event.points.len() as u64, event.f0_hz.len() as u64 + 128)?;
        sum.checked_add(work)
            .ok_or_else(|| "Pair trace work overflow".to_owned())
    })?;
    mul(work, request.pair_config.shared_shifts_s.len() as u64)
}
fn pair_work(request: &WidePairRequest, points: u64, trace_work: u64) -> Result<u64, String> {
    mul(points, 16)?
        .checked_add(trace_work)
        .and_then(|n| n.checked_add(request.retain as u64 * 4))
        .ok_or_else(|| "Pair work overflow".to_owned())
}

type CpaEntry = (usize, usize, ClosestApproach);

struct IndexMemoryPlan {
    source_metadata_bytes: u64,
    spatial_index_memory_bytes: u64,
    local_cpa_memory_reserve_bytes: u64,
    total_bytes: u64,
}
fn node_capacity(pass_count: usize) -> Result<usize, String> {
    if pass_count == 0 {
        return Err("Cannot index an empty source pool".into());
    }
    if pass_count <= 8 {
        return Ok(1);
    }
    // Every leaf of the balanced split contains at least four passes. Reserving
    // this upper bound avoids geometric Vec growth during recursive building.
    pass_count
        .checked_add(3)
        .map(|n| n / 4)
        .and_then(|leaves| leaves.checked_mul(2))
        .and_then(|nodes| nodes.checked_sub(1))
        .ok_or_else(|| "Spatial-index node count overflow".into())
}
impl IndexMemoryPlan {
    fn new(
        source_metadata_bytes: u64,
        pass_count: usize,
        points: u64,
        max_pass_points: usize,
    ) -> Result<Self, String> {
        let nodes = node_capacity(pass_count)? as u64;
        // Peak includes all node/leaf capacities, the full build-index array,
        // stable-sort scratch, full query results and traversal-stack slack.
        // Build and query temporaries do not coexist, but both are reserved.
        let spatial_index_memory_bytes = mul(nodes, std::mem::size_of::<BoxNode>() as u64 + 32)?
            .checked_add(mul(
                pass_count as u64,
                4 * std::mem::size_of::<usize>() as u64,
            )?)
            .and_then(|n| n.checked_add(64 * 1024))
            .ok_or("Spatial-index allocation estimate overflow")?;
        // A pass of P points produces at most 2P local minima; each can enter
        // both events. The independent 250k inventory safety limit is retained.
        let entries = points.saturating_mul(4).min(MAX_CPA_ENTRIES as u64);
        // Incremental exact reservations leave at most 256 spare entries in
        // each event vector. Include stable event2-sort scratch and extra slack.
        // A single native CPA census can transiently hold growing minima and
        // the deduplicated/returned copy (at most six times its point count).
        // Reserve two more copies per point for the first prepared pass while
        // the second is built during pair scoring. Velocity/arrival arrays
        // remain separate native model scratch, outside this named budget.
        let census_cpa_bytes = mul(
            max_pass_points as u64,
            8 * std::mem::size_of::<ClosestApproach>() as u64,
        )?;
        let local_cpa_memory_reserve_bytes =
            mul(entries * 3 + 512, std::mem::size_of::<CpaEntry>() as u64)?
                .checked_add(census_cpa_bytes)
                .ok_or("Local-CPA census allocation estimate overflow")?
                .checked_add(64 * 1024)
                .ok_or("Local-CPA allocation estimate overflow")?;
        let total_bytes = source_metadata_bytes
            .checked_add(spatial_index_memory_bytes)
            .and_then(|n| n.checked_add(local_cpa_memory_reserve_bytes))
            .ok_or("Combined index allocation estimate overflow")?;
        Ok(Self {
            source_metadata_bytes,
            spatial_index_memory_bytes,
            local_cpa_memory_reserve_bytes,
            total_bytes,
        })
    }
    fn require_budget(&self, budget: u64) -> Result<(), Stop> {
        if self.total_bytes > budget {
            return Err(Stop {
                status: WideStatus::BudgetExceeded,
                reason: format!(
                    "Combined source/spatial-index/local-CPA memory needs {} bytes, exceeding the declared {} bytes; no source pass or receiver was removed",
                    self.total_bytes, budget
                ),
            });
        }
        Ok(())
    }
}
fn allocation_stop(label: &str) -> Stop {
    Stop {
        status: WideStatus::BudgetExceeded,
        reason: format!(
            "Cannot reserve bounded {label} memory; no source pass or receiver was removed"
        ),
    }
}

struct BoxNode {
    min: [f64; 3],
    max: [f64; 3],
    children: Option<(usize, usize)>,
    passes: Vec<usize>,
}
struct PassIndex {
    nodes: Vec<BoxNode>,
}
fn box_distance2(min: [f64; 3], max: [f64; 3], xyz: [f64; 3]) -> f64 {
    (0..3)
        .map(|i| {
            let d = (min[i] - xyz[i]).max(0.0).max(xyz[i] - max[i]);
            d * d
        })
        .sum()
}
impl PassIndex {
    fn new(
        passes: &[PassMetadata],
        mut checkpoint: impl FnMut() -> Result<(), Stop>,
    ) -> Result<Self, Stop> {
        checkpoint()?;
        let mut index = Self { nodes: Vec::new() };
        index
            .nodes
            .try_reserve_exact(node_capacity(passes.len())?)
            .map_err(|_| allocation_stop("spatial-index nodes"))?;
        let mut indices = Vec::new();
        indices
            .try_reserve_exact(passes.len())
            .map_err(|_| allocation_stop("spatial-index build indices"))?;
        indices.extend(0..passes.len());
        index.build(&mut indices, passes, &mut checkpoint)?;
        checkpoint()?;
        Ok(index)
    }
    fn build(
        &mut self,
        indices: &mut [usize],
        passes: &[PassMetadata],
        checkpoint: &mut impl FnMut() -> Result<(), Stop>,
    ) -> Result<usize, Stop> {
        checkpoint()?;
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for (position, &index) in indices.iter().enumerate() {
            if position.is_multiple_of(1024) {
                checkpoint()?;
            }
            for axis in 0..3 {
                min[axis] = min[axis].min(passes[index].ecef_min[axis]);
                max[axis] = max[axis].max(passes[index].ecef_max[axis]);
            }
        }
        let id = self.nodes.len();
        self.nodes.push(BoxNode {
            min,
            max,
            children: None,
            passes: Vec::new(),
        });
        if indices.len() <= 8 {
            self.nodes[id]
                .passes
                .try_reserve_exact(indices.len())
                .map_err(|_| allocation_stop("spatial-index leaf indices"))?;
            self.nodes[id].passes.extend_from_slice(indices);
        } else {
            let axis = (0..3)
                .max_by(|&a, &b| (max[a] - min[a]).total_cmp(&(max[b] - min[b])))
                .unwrap();
            checkpoint()?;
            indices.sort_by(|&a, &b| {
                (passes[a].ecef_min[axis] + passes[a].ecef_max[axis])
                    .total_cmp(&(passes[b].ecef_min[axis] + passes[b].ecef_max[axis]))
                    .then(a.cmp(&b))
            });
            checkpoint()?;
            let middle = indices.len() / 2;
            let (left, right) = indices.split_at_mut(middle);
            let left = self.build(left, passes, checkpoint)?;
            let right = self.build(right, passes, checkpoint)?;
            self.nodes[id].children = Some((left, right));
        }
        checkpoint()?;
        Ok(id)
    }
    fn query(
        &self,
        xyz: [f64; 3],
        distance: f64,
        passes: &[PassMetadata],
        guard: &mut Guard<'_>,
        progress: &mut WideProgress,
    ) -> Result<Vec<usize>, Stop> {
        let mut stack = Vec::new();
        stack
            .try_reserve_exact(usize::BITS as usize + 2)
            .map_err(|_| allocation_stop("spatial-index traversal"))?;
        stack.push(0);
        let mut result = Vec::new();
        result
            .try_reserve_exact(passes.len())
            .map_err(|_| allocation_stop("spatial-index query results"))?;
        // Expand by roundoff allowance: a conservative lower bound must never
        // exclude a pass exactly on the inclusive coarse slant-distance edge.
        let radius = distance + distance.abs() * 1e-12 + 1e-6;
        let radius2 = radius * radius;
        while let Some(id) = stack.pop() {
            guard.charge(1, progress)?;
            let node = &self.nodes[id];
            if box_distance2(node.min, node.max, xyz) > radius2 {
                continue;
            }
            if let Some((a, b)) = node.children {
                stack.push(b);
                stack.push(a);
            } else {
                for &index in &node.passes {
                    guard.charge(1, progress)?;
                    if box_distance2(passes[index].ecef_min, passes[index].ecef_max, xyz) <= radius2
                    {
                        result.push(index);
                    }
                }
            }
        }
        result.sort_unstable();
        Ok(result)
    }
}

struct TrackCache {
    tracks: std::collections::BTreeMap<usize, (std::sync::Arc<physics::Track>, u64, u64)>,
    ages: std::collections::BTreeMap<u64, usize>,
    tick: u64,
    bytes: u64,
}
impl TrackCache {
    fn new() -> Self {
        Self {
            tracks: Default::default(),
            ages: Default::default(),
            tick: 0,
            bytes: 0,
        }
    }
    fn load(
        &mut self,
        index: usize,
        store: &TrackStore,
        model: physics::ModelConfig,
    ) -> Result<std::sync::Arc<physics::Track>, String> {
        self.tick += 1;
        if let Some((track, _, age)) = self.tracks.get_mut(&index) {
            self.ages.remove(age);
            *age = self.tick;
            self.ages.insert(self.tick, index);
            return Ok(track.clone());
        }
        let metadata = &store.passes()[index];
        let bytes = (metadata.point_count as u64)
            .checked_mul(std::mem::size_of::<physics::TrackPoint>() as u64)
            .and_then(|n| n.checked_add(metadata.id.len() as u64 + 256))
            .ok_or("Track allocation overflow")?;
        const CACHE: u64 = 256 * 1024 * 1024;
        if bytes > CACHE {
            return Err(
                "One continuous pass exceeds the 256 MiB track-cache bound; it was not truncated"
                    .into(),
            );
        }
        while self.bytes + bytes > CACHE || self.tracks.len() >= 4096 {
            let (_, old) = self
                .ages
                .pop_first()
                .ok_or("Track cache accounting failed")?;
            let (_, size, _) = self
                .tracks
                .remove(&old)
                .ok_or("Track cache index differs")?;
            self.bytes -= size;
        }
        let track = store.load_track(index)?;
        physics::validate_track(&track, model)?;
        if track.flight_id != metadata.id || track.points.len() != metadata.point_count {
            return Err("Loaded continuous pass differs from its indexed identity".into());
        }
        let track = std::sync::Arc::new(track);
        self.bytes += bytes;
        self.ages.insert(self.tick, index);
        self.tracks.insert(index, (track.clone(), bytes, self.tick));
        Ok(track)
    }
}
#[derive(Debug)]
struct Stop {
    status: WideStatus,
    reason: String,
}
impl From<String> for Stop {
    fn from(reason: String) -> Self {
        Self {
            status: WideStatus::Failed,
            reason,
        }
    }
}
impl From<&str> for Stop {
    fn from(reason: &str) -> Self {
        Self::from(reason.to_owned())
    }
}
struct Guard<'a> {
    start: Instant,
    cancel: &'a AtomicBool,
    budgets: &'a WideBudgets,
}
impl Guard<'_> {
    fn charge(&mut self, units: u64, progress: &mut WideProgress) -> Result<(), Stop> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Stop {
                status: WideStatus::Cancelled,
                reason: "Cancelled by operator; unfinished receiver was not committed".into(),
            });
        }
        if self.start.elapsed().as_secs_f64() >= self.budgets.max_duration_s as f64 {
            return Err(Stop {
                status: WideStatus::BudgetExceeded,
                reason: "Global wide-area wall budget exhausted".into(),
            });
        }
        if progress
            .work_units
            .checked_add(units)
            .is_none_or(|n| n > self.budgets.max_work_units)
        {
            return Err(Stop {
                status: WideStatus::BudgetExceeded,
                reason: "Global wide-area work budget exhausted".into(),
            });
        }
        progress.work_units += units;
        Ok(())
    }
}
fn source_checkpoint<F: FnMut(WideProgress)>(
    guard: &mut Guard<'_>,
    progress: &mut WideProgress,
    callback: &mut F,
    stopped: &mut Option<Stop>,
) -> Result<(), String> {
    let outcome = guard.charge(0, progress).and_then(|()| {
        callback(progress.clone());
        guard.charge(0, progress)
    });
    if let Err(stop) = outcome {
        let message = stop.reason.clone();
        *stopped = Some(stop);
        return Err(message);
    }
    Ok(())
}
fn in_bounds(cpa: &ClosestApproach, bounds: &CoarseBounds) -> bool {
    !cpa.at_track_boundary
        && cpa.speed_mps >= bounds.speed_mps[0]
        && cpa.speed_mps <= bounds.speed_mps[1]
        && cpa.slant_distance_m >= bounds.slant_distance_m[0]
        && cpa.slant_distance_m <= bounds.slant_distance_m[1]
}
fn order(a: &WideCandidate, b: &WideCandidate) -> std::cmp::Ordering {
    a.score
        .combined_rmse_hz
        .total_cmp(&b.score.combined_rmse_hz)
        .then(a.encounter_index.cmp(&b.encounter_index))
}
fn retain_candidate(
    rows: &mut Vec<WideCandidate>,
    candidate: WideCandidate,
    limit: usize,
    pairs: bool,
) -> bool {
    if pairs
        && let Some(index) = rows
            .iter()
            .position(|row| row.score.flight_ids == candidate.score.flight_ids)
    {
        if order(&candidate, &rows[index]).is_lt() {
            rows[index] = candidate;
            rows.sort_by(order);
        }
        return false;
    }
    rows.push(candidate);
    rows.sort_by(order);
    if rows.len() > limit {
        rows.pop();
        true
    } else {
        false
    }
}
struct LocalResult {
    row: ReceiverResult,
    candidates: Vec<WideCandidate>,
    pairs: Vec<WideCandidate>,
    pair_truncated: bool,
    local_cpas: u64,
    boundary: u64,
}
fn push_cpa(entries: &mut Vec<CpaEntry>, entry: CpaEntry) -> Result<(), Stop> {
    if entries.len() == entries.capacity() {
        let additional = 256.min(MAX_CPA_ENTRIES - entries.len());
        entries
            .try_reserve_exact(additional)
            .map_err(|_| allocation_stop("local-CPA inventory"))?;
    }
    entries.push(entry);
    Ok(())
}
fn check_cpa_capacity(current: usize, additional: usize) -> Result<(), Stop> {
    if current
        .checked_add(additional)
        .is_none_or(|count| count > MAX_CPA_ENTRIES)
    {
        return Err(Stop { status: WideStatus::BudgetExceeded, reason:
            "One receiver exceeds the separate 250000-entry local-CPA memory/safety inventory; this receiver is incomplete and no pass was dropped".into() });
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn receiver<F: FnMut(WideProgress)>(
    request: &WidePairRequest,
    store: &TrackStore,
    index: &PassIndex,
    cache: &mut TrackCache,
    receiver_index: u64,
    guard: &mut Guard<'_>,
    progress: &mut WideProgress,
    callback: &mut F,
) -> Result<LocalResult, Stop> {
    let observer = request.grid.observer(receiver_index)?;
    let xyz = physics::geodetic_to_ecef(
        observer.latitude_deg,
        observer.longitude_deg,
        observer.ellipsoid_height_m,
    )?;
    let radius = request.coarse_bounds[0].slant_distance_m[1]
        .max(request.coarse_bounds[1].slant_distance_m[1]);
    let pass_indices = index.query(xyz, radius, store.passes(), guard, progress)?;
    progress.potential_pass_checks += store.passes().len() as u64;
    progress.bbox_excluded_passes += (store.passes().len() - pass_indices.len()) as u64;
    let mut event1 = Vec::new();
    let mut event2 = Vec::new();
    let mut local_cpas = 0;
    let mut boundary = 0;
    progress.stage = "local_cpa_census".into();
    for (position, &pass_index) in pass_indices.iter().enumerate() {
        guard.charge(
            mul(store.passes()[pass_index].point_count as u64, 16)?,
            progress,
        )?;
        let track = cache.load(pass_index, store, request.pair_config.model)?;
        let cpas = physics::local_closest_approaches(&track, xyz, request.pair_config.model)?;
        guard.charge(0, progress)?;
        for (cpa_index, cpa) in cpas.into_iter().enumerate() {
            local_cpas += 1;
            if cpa.at_track_boundary {
                boundary += 1;
            }
            let eligible1 = in_bounds(&cpa, &request.coarse_bounds[0]);
            let eligible2 = in_bounds(&cpa, &request.coarse_bounds[1]);
            check_cpa_capacity(
                event1.len() + event2.len(),
                usize::from(eligible1) + usize::from(eligible2),
            )?;
            if eligible1 {
                push_cpa(&mut event1, (pass_index, cpa_index, cpa))?;
            }
            if eligible2 {
                push_cpa(&mut event2, (pass_index, cpa_index, cpa))?;
            }
        }
        progress.scanned_passes += 1;
        if position % 32 == 0 {
            callback(progress.clone());
        }
    }
    event2.sort_by(|a, b| a.2.arrival_time_s.total_cmp(&b.2.arrival_time_s));
    let mut local = LocalResult {
        row: ReceiverResult {
            receiver_index,
            observer: observer.clone(),
            status: "no_supported_pair".into(),
            eligible_event_cpas: [event1.len(), event2.len()],
            timing_candidates: 0,
            scored_candidates: 0,
            model_rejections: 0,
            scoring_rejections: 0,
            best: None,
        },
        candidates: Vec::new(),
        pairs: Vec::new(),
        pair_truncated: false,
        local_cpas,
        boundary,
    };
    progress.stage = "shared_clock_pair_scoring".into();
    let trace_work = trace_work(request)?;
    for (i, cpa1_index, cpa1) in event1 {
        guard.charge(1, progress)?;
        let target = cpa1.arrival_time_s + request.measured_separation_s;
        let first = event2.partition_point(|(_, _, c)| {
            c.arrival_time_s < target - request.separation_tolerance_s
        });
        for &(j, cpa2_index, cpa2) in &event2[first..] {
            if cpa2.arrival_time_s > target + request.separation_tolerance_s {
                break;
            }
            if cpa2.arrival_time_s <= cpa1.arrival_time_s || (i == j && cpa1_index == cpa2_index) {
                continue;
            }
            let points =
                store.passes()[i].point_count as u64 + store.passes()[j].point_count as u64;
            let work = pair_work(request, points, trace_work)?;
            guard.charge(work, progress)?;
            let first_track = cache.load(i, store, request.pair_config.model)?;
            let second_track = cache.load(j, store, request.pair_config.model)?;
            let encounter = progress.timing_candidates;
            progress.timing_candidates += 1;
            local.row.timing_candidates += 1;
            let mut stopped = None;
            match physics::score_pair_at_cpa_with_checkpoint(
                [&first_track, &second_track],
                xyz,
                [&request.events[0], &request.events[1]],
                &request.pair_config,
                [cpa1_index, cpa2_index],
                || source_checkpoint(guard, progress, callback, &mut stopped),
            ) {
                Ok(score) => {
                    let candidate = WideCandidate {
                        receiver_index,
                        observer: observer.clone(),
                        score,
                        encounter_index: encounter,
                    };
                    retain_candidate(
                        &mut local.candidates,
                        candidate.clone(),
                        request.retain,
                        false,
                    );
                    local.pair_truncated |=
                        retain_candidate(&mut local.pairs, candidate, request.retain, true);
                    local.row.scored_candidates += 1;
                    progress.scored_candidates += 1;
                }
                Err(_) => {
                    if let Some(stop) = stopped {
                        return Err(stop);
                    }
                    local.row.scoring_rejections += 1;
                }
            }
            guard.charge(0, progress)?;
            if progress.timing_candidates.is_multiple_of(32) {
                callback(progress.clone());
            }
        }
    }
    local.row.best = local.candidates.first().cloned();
    local.row.status = if local.row.best.is_some() {
        "scored"
    } else if local.row.scoring_rejections > 0 {
        "unavailable_pair_support"
    } else {
        "no_supported_pair"
    }
    .into();
    Ok(local)
}

/// Fresh-only execution. Every committed receiver has searched the complete
/// eligible pass pool; cancellation never commits a partially scored receiver.
pub fn run<F: FnMut(WideProgress)>(
    request: &WidePairRequest,
    output: &Path,
    cancel: &AtomicBool,
    mut callback: F,
) -> Result<WidePairReport, String> {
    let started = Instant::now();
    validate_request(request)?;
    let mut journal = Journal::create(output, request)?;
    let mut report=WidePairReport{schema_version:1,request_sha256:request_sha256(request)?,source:request.source.clone(),source_time_origin_utc_s:None,
        status:WideStatus::Completed,progress:WideProgress{total_receivers:request.grid.count,stage:"verifying_source".into(),..Default::default()},best_hypotheses:Vec::new(),best_flight_pairs:Vec::new(),retain:request.retain,retained_truncated:false,pair_retention_truncated:false,
        local_cpa_count:0,boundary_cpa_rejections:0,model_rejections:0,scoring_rejections:0,output_directory:journal.directory.clone(),pages:Vec::new(),error:None,location_verified:false,source_verified_at_end:false,
        limitations:vec![
            "Two reviewed traced ridges, all eligible ordered local CPA pairs, discrete emitted tones and one shared clock. This is the reproducible equal-event RMSE method, not the video's unpublished brightness normalization.".into(),
            "All declared global receivers are accounted for. A partial result is provisional and cannot establish the full-domain winner. Work counters include attempted work on an unfinished receiver; only complete receivers enter retained rankings/pages.".into(),
            "Source passes are ordered by the immutable index. Conservative full-pass ECEF boxes only exclude passes outside both inclusive slant bounds. Storage blocks do not create physical gaps, CPA boundaries or separate pair domains.".into(),
            "Local-CPA and boundary counts describe evaluated passes only; excluded pass counts remain explicit. These diagnostic totals differ from an unpruned legacy scan, while all surviving pair scores have the same domain and physics.".into(),
            "Constant sound speed and explicit fixed receiver ellipsoid height only. No atmospheric refinement or thunder evidence is silently enabled. Grid spacing is angular, not equal-area; receiver rank is not confidence or continuous-space uniqueness.".into(),
            "Bounded top hypotheses and best ordered flight pairs are retained; per-receiver summaries preserve every completed location. Individual discarded pair scores are not archived. Empty support is not proof that no source existed.".into(),
            "Cancellation and deadlines are checked throughout source verification, spatial-index construction, shared shifts, frequency-factor batches and emitted-tone fitting. Individual native pass preparation and each stable spatial-index sort remain bounded operations between checks. The first version writes fresh-only durable pages and terminal reports; interrupted runs cannot yet resume.".into(),
            "Work units charge index traversal, full track preparation, every trace/frequency/shared-shift evaluation and bounded ranking work. They are an operational cap, not CPU instructions or a calibrated runtime forecast.".into(),
            "The full bounded work of each pair is reserved before it starts; a cancelled pair may consume less CPU than its charged reservation. A cancellation is not a rejected acoustic match.".into(),
            "The declared index-memory budget covers verified source metadata/verification scratch, conservative spatial-index construction/query buffers and local-CPA inventory/working arrays. The 256 MiB track cache, other native model preparation and report buffers are separate; this is not a process-RAM hard limit. There is no source-pass count cutoff. The separate 250000-entry local-CPA cap stops with a provisional budget-exceeded result instead of dropping sources.".into(),
        ]};
    let mut guard = Guard {
        start: started,
        cancel,
        budgets: &request.budgets,
    };
    callback(report.progress.clone());
    let outcome = (|| -> Result<(), Stop> {
        let mut stopped = None;
        let store = TrackStore::open_with_memory_limit(
            &request.source.manifest_path,
            request.budgets.max_index_memory_bytes,
            || {
                source_checkpoint(
                    &mut guard,
                    &mut report.progress,
                    &mut callback,
                    &mut stopped,
                )
            },
        )
        .map_err(|reason| {
            stopped.unwrap_or(Stop {
                status: if crate::track_store::is_memory_limit_error(&reason) {
                    WideStatus::BudgetExceeded
                } else {
                    WideStatus::Failed
                },
                reason,
            })
        })?;
        let estimate = estimate_store(request, &store)?;
        report.source_time_origin_utc_s = Some(estimate.source_time_origin_utc_s);
        report.progress.stage = "preparing_index".into();
        callback(report.progress.clone());
        let n = store.passes().len() as u64;
        let depth = 64 - n.leading_zeros() as u64;
        guard.charge(mul(n, depth + 4)?, &mut report.progress)?;
        let index = PassIndex::new(store.passes(), || {
            callback(report.progress.clone());
            guard.charge(0, &mut report.progress)
        })?;
        let mut cache = TrackCache::new();
        for receiver_index in 0..request.grid.count {
            guard.charge(1, &mut report.progress)?;
            // Reserve row/checkpoint headroom before doing receiver work.
            if !journal.can_write(20 * 1024) {
                return Err(Stop {
                    status: WideStatus::BudgetExceeded,
                    reason: "Global wide-area output budget exhausted".into(),
                });
            }
            let local = receiver(
                request,
                &store,
                &index,
                &mut cache,
                receiver_index,
                &mut guard,
                &mut report.progress,
                &mut callback,
            )?;
            guard.charge(0, &mut report.progress)?;
            journal.write_receiver(&local.row)?;
            report.local_cpa_count += local.local_cpas;
            report.boundary_cpa_rejections += local.boundary;
            report.model_rejections += local.row.model_rejections;
            report.scoring_rejections += local.row.scoring_rejections;
            report.retained_truncated |=
                local.row.scored_candidates > local.candidates.len() as u64;
            for candidate in local.candidates {
                report.retained_truncated |= retain_candidate(
                    &mut report.best_hypotheses,
                    candidate,
                    request.retain,
                    false,
                );
            }
            report.pair_retention_truncated |= local.pair_truncated;
            for candidate in local.pairs {
                report.pair_retention_truncated |= retain_candidate(
                    &mut report.best_flight_pairs,
                    candidate,
                    request.retain,
                    true,
                );
            }
            report.progress.completed_receivers += 1;
            callback(report.progress.clone());
        }
        guard.charge(0, &mut report.progress)?;
        report.progress.stage = "verifying_source_final".into();
        let mut stopped = None;
        store
            .verify_unchanged_with_checkpoint(|| {
                source_checkpoint(
                    &mut guard,
                    &mut report.progress,
                    &mut callback,
                    &mut stopped,
                )
            })
            .map_err(|reason| {
                stopped.unwrap_or(Stop {
                    status: WideStatus::Failed,
                    reason,
                })
            })?;
        report.source_verified_at_end = true;
        guard.charge(0, &mut report.progress)?;
        Ok(())
    })();
    if let Err(stop) = outcome {
        report.status = stop.status;
        report.error = Some(stop.reason);
    }
    // Stopping must not launch another full-dataset read. Consumed blocks were
    // checked when opened; incomplete results explicitly lack the final recheck.
    report.progress.stage = "finished".into();
    journal.finish(&mut report)?;
    callback(report.progress.clone());
    Ok(report)
}
pub fn load_report(directory: &Path) -> Result<WidePairReport, String> {
    crate::wide_area_journal::load_report(directory)
}

#[cfg(test)]
mod index_memory_tests {
    use super::*;

    #[test]
    fn index_memory_preflight_has_no_two_hundred_fifty_thousand_pass_cutoff() {
        let memory = IndexMemoryPlan::new(80 * 1024 * 1024, 500_001, 1_000_002, 1000).unwrap();
        memory.require_budget(DEFAULT_INDEX_MEMORY_BYTES).unwrap();
        assert_eq!(
            memory.total_bytes,
            memory.source_metadata_bytes
                + memory.spatial_index_memory_bytes
                + memory.local_cpa_memory_reserve_bytes
        );
        assert!(memory.spatial_index_memory_bytes > 500_001 * std::mem::size_of::<usize>() as u64);
        assert!(
            memory.local_cpa_memory_reserve_bytes
                >= MAX_CPA_ENTRIES as u64 * std::mem::size_of::<CpaEntry>() as u64
        );
    }

    #[test]
    fn combined_index_memory_refusal_is_a_budget_stop_before_allocation() {
        let memory = IndexMemoryPlan::new(40 * 1024 * 1024, 500_001, 1_000_002, 1000).unwrap();
        let error = memory.require_budget(64 * 1024 * 1024).unwrap_err();
        assert_eq!(error.status, WideStatus::BudgetExceeded);
        assert!(
            error
                .reason
                .contains("no source pass or receiver was removed")
        );
        assert_eq!(
            memory
                .require_budget(memory.total_bytes - 1)
                .unwrap_err()
                .status,
            WideStatus::BudgetExceeded
        );
        memory.require_budget(memory.total_bytes).unwrap();
        assert!(IndexMemoryPlan::new(u64::MAX, 1, 2, 2).is_err());
        assert!(node_capacity(usize::MAX).is_err());
    }

    #[test]
    fn slice_bvh_preserves_complete_sorted_queries_and_bounded_leaf_capacity() {
        let passes: Vec<_> = (0..65)
            .map(|i| {
                // Repeated boxes exercise exact comparator ties across recursive
                // splits. The pass index is the original deterministic tie breaker.
                let x = (i % 7) as f64;
                PassMetadata {
                    id: format!("pass-{i:03}"),
                    point_count: 2,
                    start_s: 0.0,
                    end_s: 1.0,
                    ecef_min: [x, -1.0, -1.0],
                    ecef_max: [x + 1.0, 1.0, 1.0],
                    blocks: Vec::new(),
                }
            })
            .collect();
        let index = PassIndex::new(&passes, || Ok(())).unwrap();
        assert!(index.nodes.len() <= node_capacity(passes.len()).unwrap());
        assert_eq!(
            index
                .nodes
                .iter()
                .map(|node| node.passes.len())
                .sum::<usize>(),
            passes.len()
        );
        assert!(index.nodes.iter().all(|node| node.passes.capacity() <= 8));
        let actual_resident = index.nodes.capacity() as u64 * std::mem::size_of::<BoxNode>() as u64
            + index
                .nodes
                .iter()
                .map(|node| {
                    node.passes.capacity() as u64 * std::mem::size_of::<usize>() as u64 + 32
                })
                .sum::<u64>();
        let memory = IndexMemoryPlan::new(0, passes.len(), 130, 2).unwrap();
        assert!(actual_resident <= memory.spatial_index_memory_bytes);
        let budgets = WideBudgets::default();
        let cancel = AtomicBool::new(false);
        let mut guard = Guard {
            start: Instant::now(),
            cancel: &cancel,
            budgets: &budgets,
        };
        for radius in [0.0, 1.0, 3.0, 100.0] {
            let xyz = [0.0; 3];
            let expanded: f64 = radius + radius * 1e-12 + 1e-6;
            let expected: Vec<_> = passes
                .iter()
                .enumerate()
                .filter(|(_, pass)| {
                    box_distance2(pass.ecef_min, pass.ecef_max, xyz) <= expanded * expanded
                })
                .map(|(i, _)| i)
                .collect();
            let actual = index
                .query(
                    xyz,
                    radius,
                    &passes,
                    &mut guard,
                    &mut WideProgress::default(),
                )
                .unwrap();
            assert_eq!(actual, expected);
        }
        let mut checks = 0;
        let stopped = PassIndex::new(&passes, || {
            checks += 1;
            if checks == 5 {
                return Err(Stop {
                    status: WideStatus::Cancelled,
                    reason: "cancel inside index construction".into(),
                });
            }
            Ok(())
        });
        assert!(matches!(
            stopped,
            Err(Stop {
                status: WideStatus::Cancelled,
                ..
            })
        ));
        assert_eq!(checks, 5);
    }

    #[test]
    fn separate_cpa_inventory_cap_stops_before_inserting_either_event() {
        check_cpa_capacity(MAX_CPA_ENTRIES - 2, 2).unwrap();
        check_cpa_capacity(MAX_CPA_ENTRIES, 0).unwrap();
        assert_eq!(
            check_cpa_capacity(MAX_CPA_ENTRIES - 1, 2)
                .unwrap_err()
                .status,
            WideStatus::BudgetExceeded
        );
        assert_eq!(
            check_cpa_capacity(usize::MAX, 1).unwrap_err().status,
            WideStatus::BudgetExceeded
        );
    }
}
