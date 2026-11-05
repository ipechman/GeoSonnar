//! Read-only, bounded reopening of receipt-covered investigation snapshots.
//! External paths are retained as provenance and are never followed by loading
//! or restoring archived buffers. Archive hashes are not authenticity proofs.
use crate::{
    casework::{self, SavedCase},
    evidence_score::{self, LightningCatalog},
    investigation_report::{self, ExportReceipt},
    joint_evidence::{JointReport, JointRequest},
    map_data::{self, MapData},
    regional_search::{self, ModalityReport, RegionalReport, SearchStatus},
    workspace::ReviewWorkspace,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub const MAX_REPORT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_REPLAY_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_RECEIPT_BYTES: u64 = 1024 * 1024;
const MAX_ARTIFACTS: usize = 64;
const REPLAY_FILE: &str = "replay-inputs.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "report", rename_all = "snake_case")]
pub enum ReportKind {
    Regional(RegionalReport),
    Comparison(ModalityReport),
    /// Shared-cell frequency/envelope fusion, distinct from picked timing.
    JointEnvelope(Box<JointSnapshot>),
}

/// Frozen normalized request and full outcome; opening never follows source paths.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointSnapshot {
    pub request: JointRequest,
    pub report: JointReport,
}

pub fn validate_joint_snapshot(
    document: &ReviewWorkspace,
    snapshot: &JointSnapshot,
) -> Result<(), String> {
    if document != &snapshot.request.document {
        return Err("Joint result differs from its historical reviewed workspace".into());
    }
    validate_joint_request_inputs(&snapshot.request)?;
    crate::joint_evidence::validate_report(&snapshot.request, &snapshot.report)
}

pub(crate) fn validate_joint_request_inputs(request: &JointRequest) -> Result<(), String> {
    validate_map(&request.map)?;
    validate_catalog(&request.lightning)
}

#[derive(Clone, Debug)]
pub struct LoadedInvestigation {
    pub directory: PathBuf,
    pub receipt_sha256: String,
    pub document: ReviewWorkspace,
    pub report: ReportKind,
    pub inputs_sha256: String,
    pub has_replay_inputs: bool,
}
impl LoadedInvestigation {
    /// Metadata-only compatibility; this does not claim the recording exists.
    pub fn matches_case(&self, case: &SavedCase, channel: usize) -> Result<(), String> {
        if case.schema_version != 1
            || self.document.case_id != case.case_id
            || self.document.recording_sha256 != case.recording.sha256
            || self.document.channel != channel
            || channel >= case.recording.media.channels as usize
            || !case.recording.media.duration_s.is_finite()
            || (self.document.duration_s - case.recording.media.duration_s).abs() > 0.001
        {
            return Err(
                "Saved investigation belongs to another case, recording, channel or duration"
                    .into(),
            );
        }
        Ok(())
    }
}

/// Full normalized buffers, not executable paths or an instruction to download.
/// Layer order and original provenance/options are part of the input identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayInputs {
    pub schema_version: u32,
    pub layers: Vec<MapData>,
    pub lightning: Option<LightningCatalog>,
}

/// The exact pre-existing GUI merge: ordered trajectories only, empty display
/// metadata. Keeping this shared avoids reconstruction-dependent input hashes.
pub fn merge_layers(layers: &[MapData]) -> Result<MapData, String> {
    let mut merged = MapData {
        source_path: PathBuf::new(),
        source_sha256: String::new(),
        time_origin_utc_s: None,
        tracks: Vec::new(),
        candidates: Vec::new(),
        observers: Vec::new(),
        grid: None,
        terrain: None,
        lightning: Vec::new(),
        lightning_provenance: None,
        notes: Vec::new(),
        location_verified: false,
    };
    let mut ids = std::collections::BTreeSet::new();
    for layer in layers {
        for track in &layer.tracks {
            if !ids.insert(&track.id) {
                return Err(format!(
                    "Track {} occurs in multiple loaded layers; remove duplicate layers before scoring.",
                    track.id
                ));
            }
            merged.tracks.push(track.clone());
        }
    }
    Ok(merged)
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}
fn linked(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}
fn directory_guard(path: &Path) -> Result<File, String> {
    let meta =
        fs::symlink_metadata(path).map_err(|e| format!("Archive directory unavailable: {e}"))?;
    if !meta.is_dir() || linked(&meta) {
        return Err(
            "Archive paths must be regular directories, without links or reparse points".into(),
        );
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .share_mode(3)
            .custom_flags(0x0200_0000 | 0x0020_0000);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_dir() || linked(&meta) {
        return Err("Archive directory changed during opening".into());
    }
    Ok(file)
}
fn archive_root(path: &Path) -> Result<(PathBuf, Vec<File>), String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("Choose an archive directory without parent traversal".into());
    }
    let mut ancestors = path.ancestors().collect::<Vec<_>>();
    ancestors.reverse();
    let guards = ancestors
        .into_iter()
        .map(directory_guard)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((path, guards))
}
fn relative_path(value: &str) -> Result<PathBuf, String> {
    if value.is_empty()
        || value.len() > 240
        || value.contains('\\')
        || value.contains(':')
        || value.starts_with('/')
    {
        return Err("Receipt artifact path is not a safe relative path".into());
    }
    for part in value.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with('.')
            || !part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
        {
            return Err("Receipt artifact path contains unsafe components".into());
        }
        let stem = part.split('.').next().unwrap().to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
        {
            return Err("Receipt artifact path uses a reserved device name".into());
        }
    }
    Ok(PathBuf::from(value))
}
fn known_artifact(path: &str) -> bool {
    const REGIONAL: &[&str] = &[
        "index.html",
        "hypotheses.csv",
        "locations.csv",
        "workspace.json",
        "regional-report.json",
    ];
    if REGIONAL.contains(&path)
        || matches!(
            path,
            "comparison.json" | "joint-envelope.json" | "joint-cells.csv" | REPLAY_FILE
        )
    {
        return true;
    }
    let Some((folder, file)) = path.split_once('/') else {
        return false;
    };
    matches!(folder, "doppler-only" | "thunder-only" | "combined")
        && (REGIONAL.contains(&file) || file == "receipt.json")
}
fn open_regular(path: &Path) -> Result<File, String> {
    let meta =
        fs::symlink_metadata(path).map_err(|e| format!("Archive artifact unavailable: {e}"))?;
    if !meta.is_file() || linked(&meta) {
        return Err(
            "Archive artifacts must be regular files without links or reparse points".into(),
        );
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || linked(&meta) {
        return Err("Archive artifact changed during opening".into());
    }
    Ok(file)
}
fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = open_regular(path)?;
    if file.metadata().map_err(|e| e.to_string())?.len() > limit {
        return Err("Archive artifact exceeds its read limit".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Archive artifact grew beyond its read limit".into());
    }
    Ok(bytes)
}

struct VerifiedArchive {
    directory: PathBuf,
    receipt: ExportReceipt,
    receipt_hash: String,
    files: BTreeMap<String, Vec<u8>>,
    _guards: Vec<File>,
}
fn verify_archive(
    directory: &Path,
    include_replay: bool,
    allow_replay: bool,
) -> Result<VerifiedArchive, String> {
    let (directory, mut guards) = archive_root(directory)?;
    if fs::symlink_metadata(directory.join("INCOMPLETE.txt")).is_ok() {
        return Err("Investigation export is incomplete".into());
    }
    let receipt_bytes = read_limited(&directory.join("receipt.json"), MAX_RECEIPT_BYTES)?;
    let receipt: ExportReceipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|e| format!("Invalid dossier receipt: {e}"))?;
    if receipt.schema_version != 1
        || receipt.location_verified
        || !valid_hash(&receipt.document_sha256)
        || !valid_hash(&receipt.inputs_sha256)
        || receipt.artifacts.is_empty()
        || receipt.artifacts.len() > MAX_ARTIFACTS
    {
        return Err("Invalid or oversized dossier receipt".into());
    }
    let mut total = 0u64;
    let mut names = std::collections::BTreeSet::new();
    let mut files = BTreeMap::new();
    for artifact in &receipt.artifacts {
        if artifact.path == REPLAY_FILE && !allow_replay {
            return Err("Replay inputs belong only in the top-level investigation receipt".into());
        }
        let relative = relative_path(&artifact.path)?;
        if !known_artifact(&artifact.path)
            || !names.insert(artifact.path.to_ascii_lowercase())
            || !valid_hash(&artifact.sha256)
        {
            return Err("Unknown, duplicate, self-referencing or invalid receipt artifact".into());
        }
        let limit = if artifact.path == REPLAY_FILE {
            MAX_REPLAY_BYTES
        } else {
            MAX_REPORT_BYTES
        };
        total = total
            .checked_add(artifact.bytes)
            .ok_or("Archive byte count overflow")?;
        if artifact.bytes > limit || total > MAX_ARCHIVE_BYTES {
            return Err("Investigation exceeds its bounded artifact or aggregate size".into());
        }
        let mut parent = directory.clone();
        let parts = relative.components().collect::<Vec<_>>();
        for part in &parts[..parts.len() - 1] {
            parent.push(part.as_os_str());
            guards.push(directory_guard(&parent)?);
        }
        let path = directory.join(relative);
        let mut file = open_regular(&path)?;
        if file.metadata().map_err(|e| e.to_string())?.len() != artifact.bytes {
            return Err(format!("Artifact size changed: {}", artifact.path));
        }
        let mut digest = Sha256::new();
        let mut count = 0u64;
        let mut saved = Vec::new();
        let mut buffer = [0u8; 65536];
        let retain = matches!(
            artifact.path.as_str(),
            "workspace.json" | "regional-report.json" | "comparison.json" | "joint-envelope.json"
        ) || (include_replay && artifact.path == REPLAY_FILE);
        loop {
            let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            count += n as u64;
            if count > artifact.bytes {
                return Err("Archive artifact changed while reading".into());
            }
            digest.update(&buffer[..n]);
            if retain {
                saved.extend_from_slice(&buffer[..n]);
            }
        }
        if count != artifact.bytes || format!("{:x}", digest.finalize()) != artifact.sha256 {
            return Err(format!("Artifact hash changed: {}", artifact.path));
        }
        if retain {
            files.insert(artifact.path.clone(), saved);
        }
    }
    if read_limited(&directory.join("receipt.json"), MAX_RECEIPT_BYTES)? != receipt_bytes {
        return Err("Archive receipt changed during reading".into());
    }
    Ok(VerifiedArchive {
        directory,
        receipt,
        receipt_hash: hash(&receipt_bytes),
        files,
        _guards: guards,
    })
}

pub fn load(directory: &Path) -> Result<LoadedInvestigation, String> {
    load_internal(directory, true)
}
fn load_internal(directory: &Path, allow_comparison: bool) -> Result<LoadedInvestigation, String> {
    let archive = verify_archive(directory, false, allow_comparison)?;
    let document: ReviewWorkspace = serde_json::from_slice(
        archive
            .files
            .get("workspace.json")
            .ok_or("Receipt omits workspace.json")?,
    )
    .map_err(|e| e.to_string())?;
    document.validate()?;
    if regional_search::document_sha256(&document)? != archive.receipt.document_sha256 {
        return Err("Archived review identity differs from receipt".into());
    }
    let report = match (
        archive.files.get("regional-report.json"),
        archive.files.get("comparison.json"),
        archive.files.get("joint-envelope.json"),
    ) {
        (Some(bytes), None, None) => {
            let report: RegionalReport =
                serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            investigation_report::validate(&document, &report)?;
            let expected = if report.status == SearchStatus::Completed {
                "completed_search_exported"
            } else {
                "partial_search_exported"
            };
            if archive.receipt.status != expected
                || archive.receipt.inputs_sha256 != report.inputs_sha256
            {
                return Err("Receipt and regional report disagree".into());
            }
            ReportKind::Regional(report)
        }
        (None, Some(bytes), None) if allow_comparison => {
            let report: ModalityReport =
                serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            investigation_report::validate_comparison(&document, &report)?;
            let expected = if report.status == SearchStatus::Completed {
                "completed_comparison_exported"
            } else {
                "partial_comparison_exported"
            };
            if archive.receipt.status != expected
                || archive.receipt.inputs_sha256 != report.inputs_sha256
            {
                return Err("Receipt and comparison report disagree".into());
            }
            ReportKind::Comparison(report)
        }
        (None, None, Some(bytes)) if allow_comparison => {
            let snapshot: JointSnapshot =
                serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            validate_joint_snapshot(&document, &snapshot)?;
            if archive.receipt.status != investigation_report::joint_export_status(&snapshot)
                || archive.receipt.inputs_sha256 != snapshot.report.inputs_sha256
            {
                return Err("Receipt and shared-cell joint result disagree".into());
            }
            ReportKind::JointEnvelope(Box::new(snapshot))
        }
        _ => return Err("Receipt must include exactly one regional, picked-timing comparison or shared-cell joint report".into()),
    };
    let mut required =
        std::collections::BTreeSet::from(["index.html".to_string(), "workspace.json".to_string()]);
    match &report {
        ReportKind::Regional(_) => {
            required.extend(
                ["hypotheses.csv", "locations.csv", "regional-report.json"].map(str::to_string),
            );
        }
        ReportKind::Comparison(comparison) => {
            required.insert("comparison.json".into());
            for entry in &comparison.modes {
                if let Some(report) = &entry.report {
                    let folder = match entry.mode {
                        regional_search::SearchMode::DopplerOnly => "doppler-only",
                        regional_search::SearchMode::ThunderOnly => "thunder-only",
                        regional_search::SearchMode::Combined => "combined",
                    };
                    for name in [
                        "index.html",
                        "workspace.json",
                        "hypotheses.csv",
                        "locations.csv",
                        "regional-report.json",
                        "receipt.json",
                    ] {
                        required.insert(format!("{folder}/{name}"));
                    }
                    let child_receipt = archive
                        .receipt
                        .artifacts
                        .iter()
                        .find(|a| a.path == format!("{folder}/receipt.json"))
                        .ok_or("Comparison receipt omits a mode dossier")?;
                    let child = load_internal(&archive.directory.join(folder), false)?;
                    let expected = regional_search::mode_document(
                        &document,
                        &comparison.request.regional,
                        entry.mode,
                    )?;
                    if child.receipt_sha256 != child_receipt.sha256
                        || child.document != expected
                        || serde_json::to_value(&child.report).map_err(|e| e.to_string())?
                            != serde_json::to_value(ReportKind::Regional(report.clone()))
                                .map_err(|e| e.to_string())?
                    {
                        return Err("Mode dossier differs from its original comparison".into());
                    }
                }
            }
        }
        ReportKind::JointEnvelope(_) => {
            required.extend(["joint-envelope.json", "joint-cells.csv"].map(str::to_string));
            if archive
                .receipt
                .artifacts
                .iter()
                .any(|a| a.path == REPLAY_FILE)
            {
                return Err("Joint snapshots retain their own frozen request; legacy replay buffers cannot replace it".into());
            }
        }
    }
    if archive
        .receipt
        .artifacts
        .iter()
        .any(|a| a.path == REPLAY_FILE)
    {
        required.insert(REPLAY_FILE.into());
    }
    if required
        != archive
            .receipt
            .artifacts
            .iter()
            .map(|a| a.path.clone())
            .collect()
    {
        return Err(
            "Receipt artifact inventory differs from the saved report type and modes".into(),
        );
    }
    Ok(LoadedInvestigation {
        directory: archive.directory,
        receipt_sha256: archive.receipt_hash,
        document,
        report,
        inputs_sha256: archive.receipt.inputs_sha256,
        has_replay_inputs: archive
            .receipt
            .artifacts
            .iter()
            .any(|entry| entry.path == REPLAY_FILE),
    })
}

fn unchanged(original: &LoadedInvestigation, current: &LoadedInvestigation) -> Result<(), String> {
    if original.receipt_sha256 != current.receipt_sha256
        || original.document != current.document
        || original.inputs_sha256 != current.inputs_sha256
        || original.has_replay_inputs != current.has_replay_inputs
        || serde_json::to_value(&original.report).map_err(|e| e.to_string())?
            != serde_json::to_value(&current.report).map_err(|e| e.to_string())?
    {
        return Err("Saved investigation changed; reopen it before restoring or inspecting".into());
    }
    Ok(())
}
pub fn load_replay_inputs(loaded: &LoadedInvestigation) -> Result<ReplayInputs, String> {
    let current = load(&loaded.directory)?;
    unchanged(loaded, &current)?;
    if matches!(&current.report, ReportKind::JointEnvelope(_)) {
        return Err("Joint normalized inputs remain in its frozen snapshot; verify original sources explicitly before preparing a new run".into());
    }
    if !current.has_replay_inputs {
        return Err("This older dossier has no archived replay inputs; load independently matching data to inspect it".into());
    }
    let archive = verify_archive(&loaded.directory, true, true)?;
    if archive.receipt_hash != loaded.receipt_sha256 {
        return Err("Archive changed during replay restoration".into());
    }
    let inputs: ReplayInputs = serde_json::from_slice(
        archive
            .files
            .get(REPLAY_FILE)
            .ok_or("Replay artifact is missing")?,
    )
    .map_err(|e| e.to_string())?;
    validate_replay_inputs(&loaded.document, &inputs, &loaded.inputs_sha256)?;
    Ok(inputs)
}

/// Explicit archived inspection. The active case's recording must be verified.
/// Joint replay additionally verifies the original workspace-bound sources;
/// merely opening either kind of archive never follows external paths.
pub fn verify_archived_inspection(
    manifest: &Path,
    loaded: &LoadedInvestigation,
    document: &ReviewWorkspace,
    map: &MapData,
    lightning: &Option<LightningCatalog>,
) -> Result<(), String> {
    let current = load(&loaded.directory)?;
    unchanged(loaded, &current)?;
    if document != &current.document {
        return Err("Current review differs from the saved investigation".into());
    }
    if let ReportKind::JointEnvelope(snapshot) = &current.report {
        return verify_joint_sources(manifest, snapshot, document, map, lightning);
    }
    let case = casework::read_case(manifest, true)?;
    current.matches_case(&case, document.channel)?;
    if regional_search::inputs_sha256(map, lightning)? != current.inputs_sha256 {
        return Err("Loaded inputs do not reproduce the saved investigation".into());
    }
    validate_map(map)?;
    if let Some(catalog) = lightning {
        validate_catalog(catalog)?;
    }
    Ok(())
}

/// Explicit pre-replay verification. This is never called by archive opening.
/// Saved normalized buffers alone do not establish that the original evidence
/// is still available and unchanged.
pub fn verify_joint_sources(
    manifest: &Path,
    snapshot: &JointSnapshot,
    document: &ReviewWorkspace,
    map: &MapData,
    lightning: &Option<LightningCatalog>,
) -> Result<(), String> {
    validate_joint_snapshot(document, snapshot)?;
    crate::workspace::verify(manifest, document)?;
    if regional_search::inputs_sha256(map, lightning)? != snapshot.report.inputs_sha256 {
        return Err("Loaded original datasets differ from the frozen joint inputs".into());
    }
    validate_map(map)?;
    if let Some(catalog) = lightning {
        validate_catalog(catalog)?;
    } else {
        return Err("Original lightning data must be loaded and verified before replay".into());
    }
    Ok(())
}

fn same_optional(a: Option<f64>, b: Option<f64>) -> bool {
    a.map(f64::to_bits) == b.map(f64::to_bits)
}
fn coordinate(lat: f64, lon: f64) -> Result<(), String> {
    if !lat.is_finite()
        || !lon.is_finite()
        || !(-90.0..=90.0).contains(&lat)
        || !(-180.0..=180.0).contains(&lon)
    {
        return Err("Invalid archived geographic coordinate".into());
    }
    Ok(())
}
fn validate_map(map: &MapData) -> Result<(), String> {
    if map.location_verified
        || map.tracks.len() > 100_000
        || map.candidates.len() > 100_000
        || map.observers.len() > 100_000
        || map
            .time_origin_utc_s
            .is_some_and(|t| !t.is_finite() || !(0.0..253_402_300_800.0).contains(&t))
    {
        return Err("Invalid or oversized archived map".into());
    }
    let mut points = 0usize;
    let mut ids = std::collections::BTreeSet::new();
    for track in &map.tracks {
        points = points
            .checked_add(track.points.len())
            .ok_or("Archived point count overflow")?;
        if points > 2_000_000
            || !ids.insert(&track.id)
            || track.id != track.physics_track.flight_id
            || track.points.len() != track.physics_track.points.len()
        {
            return Err("Archived trajectory inventory is inconsistent or too large".into());
        }
        // Use the native importer to validate timestamps, ECEF geometry, gap
        // interpretation and segment boundaries, without reading source paths.
        let mut value = serde_json::json!({"tracks":[track.physics_track]});
        let mut config = serde_json::Map::new();
        if let Some(origin) = track.time_origin_utc_s {
            config.insert("time_origin_utc_s".into(), serde_json::json!(origin));
        }
        if let Some(gap) = track.max_gap_s {
            config.insert("max_gap_s".into(), serde_json::json!(gap));
        }
        value["config"] = serde_json::Value::Object(config);
        let parsed = map_data::parse_value(&value)?;
        let expected = &parsed.tracks[0];
        if track.segments != expected.segments
            || !same_optional(track.time_origin_utc_s, expected.time_origin_utc_s)
            || !same_optional(track.max_gap_s, expected.max_gap_s)
        {
            return Err("Archived trajectory support or UTC interpretation is inconsistent".into());
        }
        for (saved, reconstructed) in track.points.iter().zip(&expected.points) {
            coordinate(saved.latitude_deg, saved.longitude_deg)?;
            // Recompute only to check physical consistency. Keep original bits:
            // 1e-9 degree (~0.1 mm) / 0.1 mm height allows math-library roundoff.
            if (saved.latitude_deg - reconstructed.latitude_deg).abs() > 1e-9
                || (saved.longitude_deg - reconstructed.longitude_deg).abs() > 1e-9
                || saved
                    .height_m
                    .zip(reconstructed.height_m)
                    .is_none_or(|(a, b)| !a.is_finite() || (a - b).abs() > 1e-4)
                || !same_optional(saved.time_s, reconstructed.time_s)
                || !same_optional(saved.time_utc_s, reconstructed.time_utc_s)
            {
                return Err("Archived displayed coordinates or times disagree with their physical trajectory".into());
            }
        }
    }
    for point in &map.observers {
        coordinate(point.latitude_deg, point.longitude_deg)?;
        if point.height_m.is_some_and(|h| !h.is_finite())
            || point.time_s.is_some()
            || point.time_utc_s.is_some()
        {
            return Err("Archived observer metadata is invalid".into());
        }
    }
    for candidate in &map.candidates {
        coordinate(candidate.latitude_deg, candidate.longitude_deg)?;
        if candidate.height_m.is_some_and(|h| !h.is_finite())
            || candidate.score.is_some_and(|s| !s.is_finite() || s < 0.0)
        {
            return Err("Archived candidate metadata is invalid".into());
        }
    }
    if let Some(grid) = &map.grid {
        coordinate(grid.bounds[0], grid.bounds[1])?;
        coordinate(grid.bounds[2], grid.bounds[3])?;
        if grid.bounds[0] > grid.bounds[2]
            || grid.point_count != map.observers.len()
            || [
                grid.declared_spacing_m,
                grid.latitude_step_deg,
                grid.longitude_step_deg,
            ]
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err("Archived grid metadata is invalid".into());
        }
    }
    if let Some(terrain) = &map.terrain {
        coordinate(terrain.bounds[0], terrain.bounds[1])?;
        coordinate(terrain.bounds[2], terrain.bounds[3])?;
        if terrain.width == 0
            || terrain.height == 0
            || terrain.bounds[0] > terrain.bounds[2]
            || terrain.step_deg.iter().any(|s| !s.is_finite() || *s <= 0.0)
            || terrain.nodata.is_some_and(|n| !n.is_finite())
        {
            return Err("Archived terrain metadata is invalid".into());
        }
    }
    if !map.lightning.is_empty() {
        evidence_score::validate_strikes(&map.lightning)?;
    }
    crate::lightning_source::validate_source(map.lightning_provenance.as_ref(), &map.lightning)?;
    Ok(())
}
fn validate_catalog(catalog: &LightningCatalog) -> Result<(), String> {
    if catalog.schema_version != 1
        || !valid_hash(&catalog.input_sha256)
        || catalog.input_path.as_os_str().is_empty()
    {
        return Err("Archived lightning source identity is invalid".into());
    }
    crate::lightning_source::validate_catalog_source(catalog)
}
fn validate_bindings(document: &ReviewWorkspace, inputs: &ReplayInputs) -> Result<(), String> {
    document.validate()?;
    if inputs.schema_version != 1 || inputs.layers.len() > 128 {
        return Err("Replay snapshot needs schema 1 and at most 128 ordered layers".into());
    }
    let mut count = 0usize;
    let mut layer_keys = std::collections::BTreeSet::new();
    for layer in &inputs.layers {
        if !layer_keys.insert((&layer.source_path, &layer.source_sha256))
            || !valid_hash(&layer.source_sha256)
            || !document.datasets.iter().any(|binding| {
                binding.path == layer.source_path && binding.sha256 == layer.source_sha256
            })
        {
            return Err("Archived map layer does not match an original review binding".into());
        }
        let options = document
            .dataset_options
            .iter()
            .find(|o| o.source_sha256 == layer.source_sha256)
            .ok_or("Archived layer has no saved interpretation")?;
        if !same_optional(options.time_origin_utc_s, layer.time_origin_utc_s)
            || layer
                .tracks
                .iter()
                .any(|track| !same_optional(track.time_origin_utc_s, layer.time_origin_utc_s))
        {
            return Err("Archived UTC interpretation differs from the saved review".into());
        }
        count = layer
            .tracks
            .iter()
            .try_fold(count, |sum, track| sum.checked_add(track.points.len()))
            .ok_or("Replay point count overflow")?;
        if count > 2_000_000 {
            return Err("Replay snapshot exceeds two million trajectory points".into());
        }
        validate_map(layer)?;
    }
    if let Some(catalog) = &inputs.lightning {
        validate_catalog(catalog)?;
        if !document.datasets.iter().any(|binding| {
            binding.path == catalog.input_path && binding.sha256 == catalog.input_sha256
        }) {
            return Err("Archived lightning does not match the original source binding".into());
        }
        let options = document
            .dataset_options
            .iter()
            .find(|o| o.source_sha256 == catalog.input_sha256)
            .ok_or("Archived lightning has no saved height interpretation")?;
        if options.lightning_altitude_wgs84
            != (catalog.provenance.altitude_datum
                == evidence_score::LightningAltitudeDatum::Wgs84Ellipsoid)
        {
            return Err(
                "Archived lightning altitude interpretation differs from the review".into(),
            );
        }
    }
    Ok(())
}
pub fn capture_replay_inputs(
    document: &ReviewWorkspace,
    layers: &[MapData],
    lightning: &Option<LightningCatalog>,
) -> Result<ReplayInputs, String> {
    if layers.len() > 128
        || layers
            .iter()
            .flat_map(|layer| &layer.tracks)
            .try_fold(0usize, |sum, track| sum.checked_add(track.points.len()))
            .is_none_or(|n| n > 2_000_000)
    {
        return Err("Replay layer or point inventory exceeds its bound".into());
    }
    let inputs = ReplayInputs {
        schema_version: 1,
        layers: layers.to_vec(),
        lightning: lightning.clone(),
    };
    validate_bindings(document, &inputs)?;
    merge_layers(&inputs.layers)?;
    replay_bytes(&inputs)?;
    Ok(inputs)
}
pub fn validate_replay_inputs(
    document: &ReviewWorkspace,
    inputs: &ReplayInputs,
    expected_hash: &str,
) -> Result<(), String> {
    validate_bindings(document, inputs)?;
    if regional_search::inputs_sha256(&merge_layers(&inputs.layers)?, &inputs.lightning)?
        != expected_hash
    {
        return Err(
            "Archived replay buffers do not reproduce the original ordered input identity".into(),
        );
    }
    Ok(())
}
struct BoundedBytes {
    bytes: Vec<u8>,
    limit: u64,
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if (self.bytes.len() as u64).saturating_add(bytes.len() as u64) > self.limit {
            return Err(std::io::Error::other(format!(
                "Replay snapshot exceeds {}; export results without replay data or reduce source layers",
                crate::data_size::format_gib(self.limit)
            )));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) fn replay_bytes(inputs: &ReplayInputs) -> Result<Vec<u8>, String> {
    let mut writer = BoundedBytes {
        bytes: Vec::new(),
        limit: MAX_REPLAY_BYTES,
    };
    serde_json::to_writer(&mut writer, inputs).map_err(|e| e.to_string())?;
    Ok(writer.bytes)
}
