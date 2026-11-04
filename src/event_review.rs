//! Read-only, case-bound acoustic event review for a native UI. Measured samples
//! remain unchanged; keeping a proposal confirms neither an aircraft nor a place.
use crate::{casework, detailed_data::sha256, jobs::FileIdentity};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_DISCOVERY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CSV_BYTES: u64 = 8 * 1024 * 1024;
const MAX_POINTS: usize = 10_000_000;

pub use crate::discovery::RidgePoint as MeasuredPoint;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Unreviewed,
    KeepForMatching,
    Reject,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewActor {
    Human,
    Agent,
    AutomatedTest,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GroupChoice {
    pub harmonic_group_id: String,
    pub decision: ReviewDecision,
    /// Optional explicit choice, in its measured harmonic's own units. Saving
    /// annotations alone does not require choosing a ridge or a pair.
    pub representative_event_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PointSegment {
    pub start_index: usize,
    pub end_index_exclusive: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewRidge {
    pub event_id: String,
    pub start_s: f64,
    pub end_s: f64,
    pub band_hz: [f64; 2],
    pub points: Vec<MeasuredPoint>,
    /// Plot each slice separately, including isolated measured points. Never
    /// join these slices across a gap or resample them into a continuous ridge.
    pub segments: Vec<PointSegment>,
    pub quality: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewGroup {
    pub harmonic_group_id: String,
    pub start_s: f64,
    pub end_s: f64,
    pub ridges: Vec<ReviewRidge>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewOverview {
    pub case_id: String,
    pub case_title: String,
    pub discovery_path: PathBuf,
    pub discovery_sha256: String,
    pub recording_path: PathBuf,
    pub recording_sha256: String,
    pub selected_channel_index: usize,
    pub available_channels: usize,
    pub duration_s: f64,
    pub frequency_bounds_hz: [f64; 2],
    pub max_gap_s: f64,
    pub group_count: usize,
    pub ridge_count: usize,
    pub measured_point_count: usize,
    pub limitations: Vec<String>,
    pub location_verified: bool,
}

#[derive(Clone, Debug)]
struct Dependency {
    requested_path: PathBuf,
    identity: FileIdentity,
}

/// Send this owned snapshot to the UI. Its read-only views cannot alter the
/// evidence that will be checked again when annotations are saved.
#[derive(Clone, Debug)]
pub struct ReviewSession {
    overview: ReviewOverview,
    groups: Vec<ReviewGroup>,
    case_manifest: Dependency,
    discovery: Dependency,
    recording: Dependency,
    csv_files: Vec<Dependency>,
}

impl ReviewSession {
    pub fn overview(&self) -> &ReviewOverview {
        &self.overview
    }

    pub fn groups(&self) -> &[ReviewGroup] {
        &self.groups
    }

    pub fn initial_choices(&self) -> Vec<GroupChoice> {
        self.groups
            .iter()
            .map(|g| GroupChoice {
                harmonic_group_id: g.harmonic_group_id.clone(),
                decision: ReviewDecision::Unreviewed,
                representative_event_id: None,
            })
            .collect()
    }

    fn verify_current(&self) -> Result<(), String> {
        for dependency in [&self.case_manifest, &self.discovery, &self.recording]
            .into_iter()
            .chain(self.csv_files.iter())
        {
            if identity(&dependency.requested_path)? != dependency.identity {
                return Err(format!(
                    "Review evidence changed; reload and review it again: {}",
                    dependency.requested_path.display()
                ));
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct DiscoveryReport {
    schema_version: u32,
    status: String,
    location_verified: bool,
    source: PathBuf,
    source_sha256: String,
    source_metadata: SourceMetadata,
    settings: ReviewSettings,
    group_count: usize,
    ridge_count: usize,
    groups: Vec<DiscoveryGroup>,
    events: Vec<DiscoveryEvent>,
    #[serde(default)]
    limitations: Vec<String>,
}

#[derive(Deserialize)]
struct SourceMetadata {
    selected_channel_index: usize,
    available_channels: usize,
    channels_averaged: bool,
    duration_s: f64,
}

#[derive(Deserialize)]
struct ReviewSettings {
    min_frequency_hz: f64,
    max_frequency_hz: f64,
    max_gap_s: f64,
}

#[derive(Deserialize)]
struct DiscoveryGroup {
    harmonic_group_id: String,
    start_s: f64,
    end_s: f64,
    ridge_count: usize,
    review_status: String,
    aircraft_identity_confirmed: bool,
}

#[derive(Deserialize)]
struct DiscoveryEvent {
    event_id: String,
    harmonic_group_id: String,
    start_s: f64,
    end_s: f64,
    band_hz: [f64; 2],
    ridge: Vec<MeasuredPoint>,
    quality: Value,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn window_valid(start: f64, end: f64, duration: f64) -> bool {
    start.is_finite() && end.is_finite() && 0. <= start && start < end && end <= duration
}

fn fragment_count_valid(value: &Value, point_count: usize) -> bool {
    // Early schema-1 discoveries serialized quality counts as floating-point
    // numbers (e.g. 1.0). Current reports use integer tokens. Both must describe
    // a positive integer, with no more fragments than measured samples. The
    // ridge-point budget keeps this bound well within exact f64 integer range.
    value.as_f64().is_some_and(|count| {
        count.is_finite() && count >= 1.0 && count.fract() == 0.0 && count <= point_count as f64
    })
}

fn identity(path: &Path) -> Result<FileIdentity, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Review input unavailable {}: {e}", path.display()))?;
    let metadata = fs::metadata(&canonical).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err(format!("Review input must be a file: {}", path.display()));
    }
    Ok(FileIdentity {
        path: canonical,
        bytes: metadata.len(),
        sha256: sha256(path)?,
    })
}

fn bounded_read(path: &Path, limit: u64) -> Result<(Vec<u8>, Dependency), String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Review input unavailable {}: {e}", path.display()))?;
    let file = File::open(&canonical).map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(format!(
            "Review input is not a file or exceeds its {limit}-byte limit: {}",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Review input grew beyond its byte limit".into());
    }
    let receipt = Dependency {
        requested_path: path.to_path_buf(),
        identity: FileIdentity {
            path: canonical,
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        },
    };
    Ok((bytes, receipt))
}

fn segments(points: &[MeasuredPoint], maximum_gap: f64) -> Vec<PointSegment> {
    let mut result = Vec::new();
    let mut start = 0;
    for (index, pair) in points.windows(2).enumerate() {
        if pair[1].time_s - pair[0].time_s > maximum_gap {
            result.push(PointSegment {
                start_index: start,
                end_index_exclusive: index + 1,
            });
            start = index + 1;
        }
    }
    result.push(PointSegment {
        start_index: start,
        end_index_exclusive: points.len(),
    });
    result
}

fn validate_csv(bytes: &[u8], expected: &[MeasuredPoint]) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("Invalid ridge CSV: {e}"))?;
    let mut lines = text.lines();
    if lines.next() != Some("time_s,frequency_hz,weight") {
        return Err("Ridge CSV needs exactly time_s,frequency_hz,weight columns".into());
    }
    for expected_point in expected {
        let line = lines.next().ok_or("Ridge CSV is missing measured rows")?;
        let values = line
            .split(',')
            .map(str::parse::<f64>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Invalid ridge CSV number: {e}"))?;
        if values.len() != 3
            || values
                .iter()
                .zip([
                    expected_point.time_s,
                    expected_point.frequency_hz,
                    expected_point.weight,
                ])
                .any(|(actual, expected)| actual.to_bits() != expected.to_bits())
        {
            return Err("Ridge CSV differs from the bound discovery measurements".into());
        }
    }
    if lines.next().is_some() {
        return Err("Ridge CSV contains extra measured rows".into());
    }
    Ok(())
}

/// Load an exact discovery artifact for one saved case and explicitly selected
/// channel. Job-backed callers must also verify their job's artifact receipt.
/// The recording is resolved through the case, allowing an identical relocated
/// source while retaining discovery's original exact checksum.
pub fn load(
    case_manifest: &Path,
    discovery_path: &Path,
    expected_discovery_sha256: &str,
    expected_channel: usize,
) -> Result<ReviewSession, String> {
    if !valid_hash(expected_discovery_sha256) {
        return Err("Expected discovery SHA-256 must be lowercase hexadecimal".into());
    }
    let case_identity = identity(case_manifest)?;
    let case = casework::read_case(case_manifest, true)?;
    let recording_path = casework::recording_path(case_manifest, &case);
    let recording_identity = identity(&recording_path)?;
    let (bytes, discovery) = bounded_read(discovery_path, MAX_DISCOVERY_BYTES)?;
    if discovery.identity.sha256 != expected_discovery_sha256 {
        return Err("Discovery fingerprint differs from the selected artifact".into());
    }
    let report: DiscoveryReport =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid discovery report: {e}"))?;
    let m = &report.source_metadata;
    let s = &report.settings;
    if report.schema_version != 1
        || report.location_verified
        || !matches!(report.status.as_str(), "events_proposed" | "no_events")
        || (report.status == "no_events") != report.events.is_empty()
        || report.group_count != report.groups.len()
        || report.ridge_count != report.events.len()
        || report.groups.len() > 100_000
        || report.events.len() > 100_000
        || report.source.as_os_str().is_empty()
        || report.source_sha256 != case.recording.sha256
        || recording_identity.sha256 != case.recording.sha256
        || recording_identity.bytes != case.recording.bytes
        || m.selected_channel_index != expected_channel
        || m.selected_channel_index >= m.available_channels
        || m.available_channels != case.recording.media.channels as usize
        || m.channels_averaged
        || !m.duration_s.is_finite()
        || m.duration_s <= 0.
        || m.duration_s > 86_401.
        || !s.min_frequency_hz.is_finite()
        || s.min_frequency_hz < 0.
        || !s.max_frequency_hz.is_finite()
        || s.max_frequency_hz <= s.min_frequency_hz
        || !s.max_gap_s.is_finite()
        || s.max_gap_s <= 0.
    {
        return Err(
            "Discovery has inconsistent case recording, channel, status, counts or settings".into(),
        );
    }
    let root = discovery
        .identity
        .path
        .parent()
        .ok_or("Discovery has no directory")?;
    let mut membership = BTreeMap::new();
    let mut groups = Vec::new();
    for group in &report.groups {
        if !valid_id(&group.harmonic_group_id)
            || group.aircraft_identity_confirmed
            || group.review_status != "unreviewed"
            || group.ridge_count == 0
            || !window_valid(group.start_s, group.end_s, m.duration_s)
            || membership
                .insert(group.harmonic_group_id.as_str(), groups.len())
                .is_some()
        {
            return Err("Discovery groups need unique safe IDs and valid unreviewed bounds".into());
        }
        groups.push(ReviewGroup {
            harmonic_group_id: group.harmonic_group_id.clone(),
            start_s: group.start_s,
            end_s: group.end_s,
            ridges: Vec::new(),
        });
    }
    let mut event_ids = BTreeSet::new();
    let mut csv_files = Vec::new();
    let mut total_points = 0usize;
    for event in report.events {
        let group_index = *membership
            .get(event.harmonic_group_id.as_str())
            .ok_or("Discovery event refers to an unknown group")?;
        let group = &mut groups[group_index];
        total_points = total_points.saturating_add(event.ridge.len());
        if !fragment_count_valid(&event.quality["fragment_count"], event.ridge.len()) {
            return Err(format!(
                "Discovery ridge {} needs a positive integer fragment_count no greater than its measured point count",
                event.event_id
            ));
        }
        if !valid_id(&event.event_id)
            || !event_ids.insert(event.event_id.clone())
            || !(3..=100_000).contains(&event.ridge.len())
            || total_points > MAX_POINTS
            || !window_valid(event.start_s, event.end_s, m.duration_s)
            || event.start_s < group.start_s
            || event.end_s > group.end_s
            || !event.band_hz.iter().all(|f| f.is_finite() && *f > 0.)
            || event.band_hz[0] > event.band_hz[1]
            || !event.quality.is_object()
            || event.quality["support_fraction"]
                .as_f64()
                .is_none_or(|v| !(0. ..=1.).contains(&v))
            || event.ridge.iter().any(|p| {
                !p.time_s.is_finite()
                    || p.time_s < event.start_s
                    || p.time_s > event.end_s
                    || !p.frequency_hz.is_finite()
                    || p.frequency_hz < event.band_hz[0]
                    || p.frequency_hz > event.band_hz[1]
                    || !p.weight.is_finite()
                    || p.weight <= 0.
            })
            || event.ridge.windows(2).any(|p| p[0].time_s >= p[1].time_s)
        {
            return Err("Invalid discovery ridge ID, measured points, quality or bounds".into());
        }
        // IDs cannot contain paths, and canonicalization also rejects a CSV
        // symlink/junction that points outside this discovery directory.
        let csv_path = root.join(format!("{}.csv", event.event_id));
        let canonical = csv_path.canonicalize().map_err(|e| {
            format!(
                "Missing or unavailable ridge CSV {}: {e}",
                csv_path.display()
            )
        })?;
        if !canonical.starts_with(root) {
            return Err("Ridge CSV resolves outside the discovery directory".into());
        }
        let (csv, dependency) = bounded_read(&csv_path, MAX_CSV_BYTES)?;
        if dependency.identity.path != canonical {
            return Err("Ridge CSV path changed during loading".into());
        }
        validate_csv(&csv, &event.ridge).map_err(|e| format!("{}: {e}", event.event_id))?;
        csv_files.push(dependency);
        group.ridges.push(ReviewRidge {
            event_id: event.event_id,
            start_s: event.start_s,
            end_s: event.end_s,
            band_hz: event.band_hz,
            segments: segments(&event.ridge, s.max_gap_s),
            points: event.ridge,
            quality: event.quality,
        });
    }
    if report
        .groups
        .iter()
        .zip(&groups)
        .any(|(raw, display)| raw.ridge_count != display.ridges.len())
    {
        return Err("Discovery group membership counts disagree".into());
    }
    let session = ReviewSession {
        overview: ReviewOverview {
            case_id: case.case_id,
            case_title: case.title,
            discovery_path: discovery.identity.path.clone(),
            discovery_sha256: discovery.identity.sha256.clone(),
            recording_path: recording_identity.path.clone(),
            recording_sha256: recording_identity.sha256.clone(),
            selected_channel_index: m.selected_channel_index,
            available_channels: m.available_channels,
            duration_s: m.duration_s,
            frequency_bounds_hz: [s.min_frequency_hz, s.max_frequency_hz],
            max_gap_s: s.max_gap_s,
            group_count: groups.len(),
            ridge_count: event_ids.len(),
            measured_point_count: total_points,
            limitations: report.limitations,
            location_verified: false,
        },
        groups,
        case_manifest: Dependency {
            requested_path: case_manifest.to_path_buf(),
            identity: case_identity,
        },
        discovery,
        recording: Dependency {
            requested_path: recording_path,
            identity: recording_identity,
        },
        csv_files,
    };
    session.verify_current()?;
    Ok(session)
}

fn validate_choices<'a>(
    session: &ReviewSession,
    choices: &'a [GroupChoice],
) -> Result<BTreeMap<&'a str, &'a GroupChoice>, String> {
    let mut by_id = BTreeMap::new();
    let groups_by_id = session
        .groups
        .iter()
        .map(|group| (group.harmonic_group_id.as_str(), group))
        .collect::<BTreeMap<_, _>>();
    for choice in choices {
        if by_id
            .insert(choice.harmonic_group_id.as_str(), choice)
            .is_some()
        {
            return Err("Each discovery group needs exactly one review decision".into());
        }
        let group = groups_by_id
            .get(choice.harmonic_group_id.as_str())
            .ok_or("Review choice refers to an unknown group")?;
        if let Some(id) = &choice.representative_event_id
            && (choice.decision != ReviewDecision::KeepForMatching
                || !group.ridges.iter().any(|ridge| ridge.event_id == *id))
        {
            return Err("A representative must be an existing ridge from its kept group".into());
        }
    }
    if by_id.len() != session.groups.len() {
        return Err("Review must explicitly account for every discovery group".into());
    }
    Ok(by_id)
}

/// Optional later pair-pipeline check; saving event annotations never calls it.
/// The caller's order is preserved and must match recording-time order.
pub fn selected_pair(
    session: &ReviewSession,
    choices: &[GroupChoice],
) -> Result<[crate::audio_priors::RidgeChoice; 2], String> {
    validate_choices(session, choices)?;
    let selected = choices
        .iter()
        .filter_map(|choice| {
            choice
                .representative_event_id
                .as_ref()
                .map(|id| (choice, id))
        })
        .collect::<Vec<_>>();
    if selected.len() != 2 {
        return Err("Select one representative from each of two distinct kept groups".into());
    }
    let ridge = |index: usize| {
        session
            .groups
            .iter()
            .find(|g| g.harmonic_group_id == selected[index].0.harmonic_group_id)
            .unwrap()
            .ridges
            .iter()
            .find(|r| r.event_id == *selected[index].1)
            .unwrap()
    };
    if ridge(0).points.last().unwrap().time_s >= ridge(1).points[0].time_s {
        return Err("Selected acoustic events must be ordered and nonoverlapping".into());
    }
    Ok(std::array::from_fn(|i| crate::audio_priors::RidgeChoice {
        harmonic_group_id: selected[i].0.harmonic_group_id.clone(),
        event_id: selected[i].1.clone(),
    }))
}

#[derive(Clone, Debug, Serialize)]
pub struct SavedReview {
    pub directory: PathBuf,
    pub review_selection: PathBuf,
    pub receipt: PathBuf,
    pub review_sha256: String,
}

fn write_new(path: &Path, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Cannot create new review file {}: {e}", path.display()))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

/// Reopen a completed native review only for the same unchanged case and
/// discovery. A changed decision file, stale receipt or foreign source fails.
/// Receipts provide local integrity checks, not signatures of reviewer identity.
/// Original absolute paths remain provenance, while byte count and SHA bind
/// relocated copies of otherwise identical relative case workspaces.
pub fn load_choices(
    session: &ReviewSession,
    review_directory: &Path,
) -> Result<Vec<GroupChoice>, String> {
    session.verify_current()?;
    let root = review_directory.canonicalize().map_err(|e| e.to_string())?;
    let receipt_path = root.join("receipt.json");
    let review_path = root.join("review-selection.json");
    for path in [&receipt_path, &review_path] {
        if !path
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(&root)
        {
            return Err("Saved review file resolves outside its directory".into());
        }
    }
    let (receipt_bytes, receipt_dependency) = bounded_read(&receipt_path, 32 * 1024 * 1024)?;
    let receipt: Value = serde_json::from_slice(&receipt_bytes)
        .map_err(|e| format!("Invalid review receipt: {e}"))?;
    if receipt["schema_version"] != 1
        || receipt["status"] != "completed"
        || receipt["location_verified"] != false
        || receipt["case_id"] != session.overview.case_id
        || !same_content(&receipt["case_manifest"], &session.case_manifest.identity)
        || !same_content(&receipt["discovery"], &session.discovery.identity)
        || !same_content(&receipt["recording"], &session.recording.identity)
        || receipt["selected_channel_index"] != session.overview.selected_channel_index
        || receipt["ridge_csv_files"].as_array().is_none_or(|files| {
            files.len() != session.csv_files.len()
                || files
                    .iter()
                    .zip(&session.csv_files)
                    .any(|(file, current)| !same_content(file, &current.identity))
        })
        || receipt["measured_point_count"] != session.overview.measured_point_count
        || receipt["measurements_modified"] != false
        || receipt["gaps_filled"] != false
    {
        return Err("Saved review receipt does not bind this unchanged case and discovery".into());
    }
    let choices: Vec<GroupChoice> = serde_json::from_value(receipt["choices"].clone())
        .map_err(|e| format!("Invalid saved review choices: {e}"))?;
    let actor: ReviewActor = serde_json::from_value(receipt["review_actor"].clone())
        .map_err(|e| format!("Invalid saved review actor: {e}"))?;
    let choices_by_id = validate_choices(session, &choices)?;
    let (review_bytes, review_dependency) = bounded_read(&review_path, 32 * 1024 * 1024)?;
    if !same_content(&receipt["review_selection"], &review_dependency.identity) {
        return Err("Saved review decision file differs from its receipt".into());
    }
    let review: Value = serde_json::from_slice(&review_bytes)
        .map_err(|e| format!("Invalid saved review decisions: {e}"))?;
    let expected = review_document(session, &choices_by_id, actor);
    if review != expected {
        return Err(
            "Saved decisions differ from the bound choices or schema-2 review format".into(),
        );
    }
    for dependency in [&receipt_dependency, &review_dependency] {
        if identity(&dependency.requested_path)? != dependency.identity {
            return Err("Saved review changed during loading".into());
        }
    }
    session.verify_current()?;
    Ok(choices)
}

fn same_content(value: &Value, expected: &FileIdentity) -> bool {
    serde_json::from_value::<FileIdentity>(value.clone()).is_ok_and(|original| {
        !original.path.as_os_str().is_empty()
            && original.bytes == expected.bytes
            && original.sha256 == expected.sha256
    })
}

const REVIEW_SCOPE: &str = "Review of acoustic proposals only; Keep permits further matching and does not confirm an aircraft or location";

fn review_document(
    session: &ReviewSession,
    choices_by_id: &BTreeMap<&str, &GroupChoice>,
    actor: ReviewActor,
) -> Value {
    json!({
        "schema_version":2,
        "discovery_sha256":session.discovery.identity.sha256,
        "source_sha256":session.recording.identity.sha256,
        "selected_channel_index":session.overview.selected_channel_index,
        "location_verified":false,"review_actor":actor,"scope":REVIEW_SCOPE,
        "groups":session.groups.iter().map(|group| json!({
            "harmonic_group_id":group.harmonic_group_id,
            "review":choices_by_id[group.harmonic_group_id.as_str()].decision,
            "event_ids":group.ridges.iter().map(|ridge| &ridge.event_id).collect::<Vec<_>>()
        })).collect::<Vec<_>>()
    })
}

/// Save a new, immutable review. The schema-2 decision file is accepted by the
/// existing reviewed-search bridge. Optional representative IDs live only in
/// its separate receipt because schema 2 deliberately has no selection field.
/// No search, flight identity or location estimate is read or inferred here.
pub fn save(
    session: &ReviewSession,
    choices: &[GroupChoice],
    actor: ReviewActor,
    output: &Path,
) -> Result<SavedReview, String> {
    let choices_by_id = validate_choices(session, choices)?;
    session.verify_current()?;
    let review = review_document(session, &choices_by_id, actor);
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::create_dir(output).map_err(|e| format!("Use a new review output directory: {e}"))?;
    let result = (|| {
        let review_path = output.join("review-selection.json");
        write_new(&review_path, &review)?;
        let review_identity = identity(&review_path)?;
        let receipt_path = output.join("receipt.json");
        let receipt = json!({
            "schema_version":1,"status":"completed","location_verified":false,"scope":REVIEW_SCOPE,
            "case_manifest":session.case_manifest.identity,"case_id":session.overview.case_id,
            "discovery":session.discovery.identity,"recording":session.recording.identity,
            "selected_channel_index":session.overview.selected_channel_index,
            "ridge_csv_files":session.csv_files.iter().map(|d| &d.identity).collect::<Vec<_>>(),
            "review_selection":review_identity,"review_actor":actor,"choices":choices,
            "representative_selection_basis":"Explicit reviewer selection among measured ridges; no fit to flight identity or location used",
            "measured_point_count":session.overview.measured_point_count,
            "measurements_modified":false,"gaps_filled":false
        });
        // Source changes during save invalidate the whole newly-created review.
        session.verify_current()?;
        write_new(&receipt_path, &receipt)?;
        Ok(SavedReview {
            directory: output.canonicalize().map_err(|e| e.to_string())?,
            review_selection: review_identity.path,
            receipt: receipt_path.canonicalize().map_err(|e| e.to_string())?,
            review_sha256: review_identity.sha256,
        })
    })();
    if let Err(error) = &result {
        let mut cleanup_errors = Vec::new();
        for name in ["review-selection.json", "receipt.json"] {
            let path = output.join(name);
            if path.exists()
                && let Err(e) = fs::remove_file(&path)
            {
                cleanup_errors.push(format!("{}: {e}", path.display()));
            }
        }
        let _ = write_new(
            &output.join("failure.json"),
            &json!({
                "status":"failed","location_verified":false,"error":error,
                "completion_artifact_cleanup_errors":cleanup_errors
            }),
        );
    }
    result
}
