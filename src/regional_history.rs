//! Bounded local journal of explicitly requested regional computations.
//! Saved results retain the reviewed workspace and exact native input hash;
//! replay still requires original data. Receipts detect changes, not forgery.
use crate::{
    casework,
    investigation_archive::ReportKind,
    investigation_report,
    joint_evidence::{self, JointRequest},
    regional_search,
    regional_search::SearchStatus,
    workspace::ReviewWorkspace,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_RUNS: usize = 256;
pub const MAX_HISTORY_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_RUN_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_RESULT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_JOINT_REQUEST_BYTES: u64 = 32 * 1024 * 1024;
const MAX_DOCUMENT_BYTES: u64 = 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * 1024;
const FILES: &[&str] = &[
    "active.lock",
    "workspace.json",
    "start.json",
    "result.json",
    "terminal.json",
    "joint-request.json",
];
const DELETE_RECEIPT: &str = "delete.json";
const MAX_RUN_FILES: usize = 32;
static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Interrupted,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug)]
pub struct RunSummary {
    pub id: String,
    pub created_unix_ms: u64,
    pub status: RunStatus,
    pub inputs_sha256: String,
    pub document_sha256: String,
    pub has_result: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug)]
pub struct LoadedRun {
    pub summary: RunSummary,
    pub document: ReviewWorkspace,
    pub report: Option<ReportKind>,
    pub joint_request: Option<JointRequest>,
    /// Exact immutable artifact identity used to reject a changed saved result.
    pub result_sha256: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    schema_version: u32,
    case_manifest_sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    bytes: u64,
    sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    schema_version: u32,
    id: String,
    created_unix_ms: u64,
    version: String,
    case_manifest_sha256: String,
    document_sha256: String,
    inputs_sha256: String,
    workspace: Artifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    joint_request: Option<Artifact>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Terminal {
    schema_version: u32,
    start_sha256: String,
    finished_unix_ms: u64,
    status: RunStatus,
    result: Option<Artifact>,
    error: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Deletion {
    schema_version: u32,
    id: String,
    case_manifest_sha256: String,
    files: std::collections::BTreeMap<String, Artifact>,
}
pub struct RunHandle {
    manifest: PathBuf,
    root: PathBuf,
    directory: PathBuf,
    start: Start,
    start_sha256: String,
    _active: File,
    _directory: File,
    _ancestors: Vec<File>,
}
impl RunHandle {
    pub fn id(&self) -> &str {
        &self.start.id
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn valid_id(s: &str) -> bool {
    s.starts_with("run-")
        && s.len() <= 80
        && s[4..].bytes().all(|b| b.is_ascii_digit() || b == b'-')
        && s.len() > 4
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
fn guard(path: &Path) -> Result<File, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("Run history directory: {e}"))?;
    if !meta.is_dir() || linked(&meta) {
        return Err("Run history must use regular directories without links".into());
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
    let opened = file.metadata().map_err(|e| e.to_string())?;
    if !opened.is_dir() || linked(&opened) {
        return Err("Run directory changed".into());
    }
    Ok(file)
}
// Keep every ancestor pinned on Windows, not just the final run directory.
// Otherwise renaming a parent can redirect later path-based operations.
fn absolute(path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("Run history paths must not contain parent traversal".into());
    }
    Ok(path)
}
fn ancestors(path: &Path) -> Result<Vec<File>, String> {
    let mut guards = Vec::new();
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        guards.push(guard(&current)?);
    }
    Ok(guards)
}
fn single_link(file: &File) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        #[derive(Default)]
        struct Information {
            attributes: u32,
            creation: [u32; 2],
            access: [u32; 2],
            write: [u32; 2],
            volume: u32,
            size_high: u32,
            size_low: u32,
            links: u32,
            index_high: u32,
            index_low: u32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                handle: *mut std::ffi::c_void,
                info: *mut Information,
            ) -> i32;
        }
        let mut info = Information::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if info.links != 1 {
            return Err("Run history files must not be hard-linked".into());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if file.metadata().map_err(|e| e.to_string())?.nlink() != 1 {
            return Err("Run history files must not be hard-linked".into());
        }
    }
    Ok(())
}
fn regular(path: &Path, max: u64) -> Result<File, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() || linked(&meta) || meta.len() > max {
        return Err(format!(
            "Unsafe or oversized run history file: {}",
            path.display()
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let lock = path
            .file_name()
            .is_some_and(|name| name == "active.lock" || name == "history.lock");
        options
            .share_mode(if lock { 3 } else { 1 })
            .custom_flags(0x0020_0000);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    if !opened.is_file() || linked(&opened) || opened.len() > max {
        return Err("Run file changed while opening".into());
    }
    single_link(&file)?;
    Ok(file)
}
fn read(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let file = regular(path, max)?;
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > max {
        return Err("Run history file grew past its limit".into());
    }
    Ok(bytes)
}
fn json<T: Serialize>(value: &T, max: u64) -> Result<Vec<u8>, String> {
    struct Bounded {
        bytes: Vec<u8>,
        max: u64,
    }
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if (self.bytes.len() as u64).saturating_add(bytes.len() as u64) > self.max {
                return Err(std::io::Error::other(
                    "Run history artifact exceeds its storage limit",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Bounded {
        bytes: Vec::new(),
        max,
    };
    serde_json::to_writer(&mut output, value).map_err(|e| e.to_string())?;
    Ok(output.bytes)
}
fn decode<T: for<'a> Deserialize<'a>>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("Invalid run history: {e}"))
}
fn artifact(bytes: &[u8]) -> Artifact {
    Artifact {
        bytes: bytes.len() as u64,
        sha256: hash(bytes),
    }
}
fn checked(path: &Path, expected: &Artifact, max: u64) -> Result<Vec<u8>, String> {
    if !valid_hash(&expected.sha256) || expected.bytes > max {
        return Err("Invalid run artifact receipt".into());
    }
    let bytes = read(path, max)?;
    if bytes.len() as u64 != expected.bytes || hash(&bytes) != expected.sha256 {
        return Err("Saved run artifact was changed; receipt does not match".into());
    }
    Ok(bytes)
}
// All callers hold history.lock. Exclusive creation prevents replacement;
// interrupted writes remain evidence and are never treated as valid receipts.
// Avoid staging hard-links, which can leave duplicate artifacts after a crash.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("Cannot create new run artifact: {e}"))?;
    single_link(&file)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("Incomplete run artifact preserved: {e}"))
}
fn lock_file(path: &Path, create: bool) -> Result<File, String> {
    if path.exists() {
        let _ = regular(path, 0)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(create);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(3).custom_flags(0x0020_0000);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || linked(&meta) || meta.len() != 0 {
        return Err("Invalid run lock file".into());
    }
    single_link(&file)?;
    Ok(file)
}
fn manifest_identity(manifest: &Path) -> Result<String, String> {
    Ok(hash(&read(manifest, MAX_DOCUMENT_BYTES)?))
}
fn root_path(manifest: &Path) -> Result<PathBuf, String> {
    Ok(absolute(manifest)?
        .parent()
        .ok_or("Case has no parent directory")?
        .join("regional-runs"))
}
fn open_root(manifest: &Path, create: bool) -> Result<(PathBuf, Vec<File>, File, String), String> {
    let manifest = absolute(manifest)?;
    let parent = manifest.parent().ok_or("Case has no parent directory")?;
    let mut guards = ancestors(parent)?;
    guards.push(regular(&manifest, MAX_DOCUMENT_BYTES)?);
    let identity = manifest_identity(&manifest)?;
    let root = root_path(&manifest)?;
    if create && !root.exists() {
        fs::create_dir(&root).map_err(|e| e.to_string())?;
    }
    guards.push(guard(&root)?);
    let lock = lock_file(&root.join("history.lock"), create)?;
    lock.try_lock()
        .map_err(|_| "Run history is busy; try again".to_string())?;
    let owner_path = root.join("owner.json");
    if !owner_path.exists() && create {
        // Never adopt a populated, unowned directory.
        if fs::read_dir(&root).map_err(|e| e.to_string())?.count() != 1 {
            return Err("Unowned run history directory is not empty".into());
        }
        write_new(
            &owner_path,
            &json(
                &Owner {
                    schema_version: 1,
                    case_manifest_sha256: identity.clone(),
                },
                MAX_METADATA_BYTES,
            )?,
        )?;
    }
    let owner: Owner = decode(&read(&owner_path, MAX_METADATA_BYTES)?)?;
    if owner.schema_version != 1 || owner.case_manifest_sha256 != identity {
        return Err("Run history belongs to another case manifest".into());
    }
    Ok((root, guards, lock, identity))
}
fn inventory(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut runs = Vec::new();
    for item in fs::read_dir(root).map_err(|e| e.to_string())? {
        let item = item.map_err(|e| e.to_string())?;
        let name = item.file_name();
        let name = name.to_str().ok_or("Invalid run history name")?;
        if matches!(name, "owner.json" | "history.lock") {
            let _ = regular(&item.path(), MAX_METADATA_BYTES)?;
        } else if valid_id(name) {
            let _ = guard(&item.path())?;
            runs.push(item.path());
        } else {
            return Err("Unrecognized file in owned run history; no files were removed".into());
        }
        if runs.len() > MAX_RUNS {
            return Err("Run history exceeds its count limit".into());
        }
    }
    Ok(runs)
}
fn run_size(directory: &Path) -> Result<u64, String> {
    let mut total = 0;
    for name in run_files(directory)? {
        total += regular(&directory.join(name), MAX_RUN_BYTES)?
            .metadata()
            .map_err(|e| e.to_string())?
            .len();
        if total > MAX_RUN_BYTES {
            return Err("Run exceeds its storage limit".into());
        }
    }
    Ok(total)
}
fn run_artifact_name(name: &str) -> bool {
    FILES.contains(&name)
        || ["workspace", "start", "result", "terminal"]
            .iter()
            .any(|stem| {
                name.strip_prefix(&format!("{stem}.pending-"))
                    .is_some_and(|tail| {
                        let parts: Vec<_> = tail.split('-').collect();
                        parts.len() == 2
                            && parts.iter().all(|part| {
                                !part.is_empty()
                                    && part.len() <= 20
                                    && part.bytes().all(|b| b.is_ascii_digit())
                            })
                    })
            })
}
fn run_files(directory: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    for item in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let item = item.map_err(|e| e.to_string())?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| "Invalid run artifact name")?;
        if !run_artifact_name(&name) && name != DELETE_RECEIPT {
            return Err("Unrecognized file in run directory; preserved without deletion".into());
        }
        files.push(name);
        if files.len() > MAX_RUN_FILES {
            return Err("Run artifact count exceeds its limit; files preserved".into());
        }
    }
    Ok(files)
}
fn active_lock(directory: &Path) -> Result<Option<File>, String> {
    let path = directory.join("active.lock");
    match fs::symlink_metadata(&path) {
        Ok(_) => lock_file(&path, false).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}
// Windows byte-range locks also reject reads through another handle to an
// empty file. Inspect the handle that owns the lock instead; no bytes exist to
// read, and the handle's no-delete sharing keeps its pathname pinned.
fn active_artifact(active: Option<&File>) -> Result<Artifact, String> {
    let active = active.ok_or("Run active lock disappeared; files preserved")?;
    let metadata = active.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || linked(&metadata) || metadata.len() != 0 {
        return Err("Run active lock changed; files preserved".into());
    }
    single_link(active)?;
    Ok(artifact(b""))
}
fn start_at(directory: &Path, identity: &str) -> Result<(Start, String), String> {
    let bytes = read(&directory.join("start.json"), MAX_METADATA_BYTES)?;
    let start: Start = decode(&bytes)?;
    if start.schema_version != 1
        || directory.file_name().and_then(|x| x.to_str()) != Some(&start.id)
        || !valid_id(&start.id)
        || start.case_manifest_sha256 != identity
        || !valid_hash(&start.inputs_sha256)
        || !valid_hash(&start.document_sha256)
        || start.version.len() > 128
        || start.workspace.bytes > MAX_DOCUMENT_BYTES
        || start.joint_request.as_ref().is_some_and(|artifact| {
            artifact.bytes > MAX_JOINT_REQUEST_BYTES || !valid_hash(&artifact.sha256)
        })
    {
        return Err("Invalid or foreign run start receipt".into());
    }
    Ok((start, hash(&bytes)))
}
fn summary_at(
    directory: &Path,
    identity: &str,
) -> Result<(RunSummary, Start, Option<Terminal>), String> {
    let _guard = guard(directory)?;
    run_size(directory)?;
    if directory.join(DELETE_RECEIPT).exists() {
        return Err("Deletion was interrupted. Retry Delete to finish removing this run; remaining files are preserved.".into());
    }
    let (start, start_hash) = start_at(directory, identity)?;
    let terminal_path = directory.join("terminal.json");
    let terminal: Option<Terminal> = if terminal_path.exists() {
        Some(decode(&read(&terminal_path, MAX_METADATA_BYTES)?)?)
    } else {
        None
    };
    let (status, has_result, error) = if let Some(end) = &terminal {
        if end.schema_version != 1
            || end.start_sha256 != start_hash
            || !matches!(
                end.status,
                RunStatus::Completed | RunStatus::Cancelled | RunStatus::Failed
            )
            || end.finished_unix_ms < start.created_unix_ms
            || end.error.as_ref().is_some_and(|s| s.len() > 8192)
            || (end.status == RunStatus::Completed && (end.result.is_none() || end.error.is_some()))
            || (end.status == RunStatus::Failed && (end.result.is_some() || end.error.is_none()))
            || (end.result.is_none() && end.error.is_none())
        {
            return Err("Invalid run terminal receipt".into());
        }
        (end.status, end.result.is_some(), end.error.clone())
    } else {
        let active = active_lock(directory)?;
        let running = active
            .as_ref()
            .is_some_and(|active| active.try_lock().is_err());
        (
            if running {
                RunStatus::Running
            } else {
                RunStatus::Interrupted
            },
            false,
            None,
        )
    };
    Ok((
        RunSummary {
            id: start.id.clone(),
            created_unix_ms: start.created_unix_ms,
            status,
            inputs_sha256: start.inputs_sha256.clone(),
            document_sha256: start.document_sha256.clone(),
            has_result,
            error,
        },
        start,
        terminal,
    ))
}
fn validate_document(manifest: &Path, document: &ReviewWorkspace) -> Result<(), String> {
    document.validate()?;
    let case = casework::read_case(manifest, false)?;
    if document.case_id != case.case_id
        || document.recording_sha256 != case.recording.sha256
        || document.channel >= case.recording.media.channels as usize
        || (document.duration_s - case.recording.media.duration_s).abs() > 0.001
    {
        return Err("Reviewed workspace belongs to another case".into());
    }
    Ok(())
}
fn report_identity(report: &ReportKind) -> (&str, &str, SearchStatus) {
    match report {
        ReportKind::Regional(r) => (&r.document_sha256, &r.inputs_sha256, r.status),
        ReportKind::Comparison(r) => (&r.document_sha256, &r.inputs_sha256, r.status),
        ReportKind::JointEnvelope(snapshot) => (
            &snapshot.report.document_sha256,
            &snapshot.report.inputs_sha256,
            match snapshot.report.status {
                joint_evidence::JointStatus::Completed => SearchStatus::Completed,
                joint_evidence::JointStatus::Cancelled => SearchStatus::Cancelled,
            },
        ),
    }
}
fn validate_report(document: &ReviewWorkspace, report: &ReportKind) -> Result<(), String> {
    match report {
        ReportKind::Regional(r) => investigation_report::validate(document, r),
        ReportKind::Comparison(r) => investigation_report::validate_comparison(document, r),
        ReportKind::JointEnvelope(snapshot) => {
            crate::investigation_archive::validate_joint_snapshot(document, snapshot)
        }
    }
}

fn validate_joint_request(
    document: &ReviewWorkspace,
    request: &JointRequest,
    inputs_sha256: &str,
) -> Result<(), String> {
    if document != &request.document || joint_evidence::inputs_sha256(request)? != inputs_sha256 {
        return Err("Joint request differs from the original workspace or source inputs".into());
    }
    crate::investigation_archive::validate_joint_request_inputs(request)?;
    joint_evidence::estimate(request)?;
    Ok(())
}

fn joint_request_at(
    directory: &Path,
    start: &Start,
    document: &ReviewWorkspace,
) -> Result<Option<JointRequest>, String> {
    if let Some(expected) = &start.joint_request {
        let request = decode(&checked(
            &directory.join("joint-request.json"),
            expected,
            MAX_JOINT_REQUEST_BYTES,
        )?)?;
        validate_joint_request(document, &request, &start.inputs_sha256)?;
        Ok(Some(request))
    } else if directory.join("joint-request.json").exists() {
        Err("Joint request is not bound by this run's start receipt; files preserved".into())
    } else {
        Ok(None)
    }
}

fn validate_joint_start_binding(
    start_request: &Option<JointRequest>,
    report: &ReportKind,
) -> Result<(), String> {
    match (start_request, report) {
        (Some(original), ReportKind::JointEnvelope(snapshot))
            if joint_evidence::request_sha256(original)?
                == joint_evidence::request_sha256(&snapshot.request)? =>
        {
            Ok(())
        }
        (None, ReportKind::Regional(_) | ReportKind::Comparison(_)) => Ok(()),
        _ => Err("Result type or frozen joint request differs from the start receipt".into()),
    }
}

/// Called before computation. Failure is explicit; the caller may continue only
/// with a visible unsaved-result warning. No datasets or recording are opened.
pub fn begin(
    manifest: &Path,
    document: &ReviewWorkspace,
    inputs_sha256: &str,
) -> Result<RunHandle, String> {
    begin_internal(manifest, document, inputs_sha256, None)
}

/// Journal the complete frozen joint request before any computation starts.
/// Callers must not run a joint search if this durable start fails.
pub fn begin_joint(
    manifest: &Path,
    document: &ReviewWorkspace,
    request: &JointRequest,
    inputs_sha256: &str,
) -> Result<RunHandle, String> {
    validate_joint_request(document, request, inputs_sha256)?;
    let request_bytes = json(request, MAX_JOINT_REQUEST_BYTES)?;
    begin_internal(manifest, document, inputs_sha256, Some(request_bytes))
}

fn begin_internal(
    manifest: &Path,
    document: &ReviewWorkspace,
    inputs_sha256: &str,
    joint_request: Option<Vec<u8>>,
) -> Result<RunHandle, String> {
    if !valid_hash(inputs_sha256) {
        return Err("Invalid computation input hash".into());
    }
    let manifest = absolute(manifest)?;
    let (root, root_guards, _lock, identity) = open_root(&manifest, true)?;
    validate_document(&manifest, document)?;
    let bytes = json(document, MAX_DOCUMENT_BYTES)?;
    let runs = inventory(&root)?;
    if runs.len() >= MAX_RUNS {
        return Err(
            "Run history is full (256 runs); delete a saved run explicitly or export a copy".into(),
        );
    }
    let mut used = 0_u64;
    for directory in &runs {
        let _guard = guard(directory)?;
        let actual = run_size(directory)?;
        let active = active_lock(directory)?;
        used = used.saturating_add(
            if active
                .as_ref()
                .is_some_and(|active| active.try_lock().is_err())
            {
                MAX_RUN_BYTES
            } else {
                actual
            },
        );
    }
    if used.saturating_add(MAX_RUN_BYTES) > MAX_HISTORY_BYTES {
        return Err(
            "Run history storage limit reached (1 GiB); remove a saved run explicitly".into(),
        );
    }
    let id = format!(
        "run-{}-{}-{}",
        now(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let start = Start {
        schema_version: 1,
        id,
        created_unix_ms: now(),
        version: env!("CARGO_PKG_VERSION").into(),
        case_manifest_sha256: identity,
        document_sha256: regional_search::document_sha256(document)?,
        inputs_sha256: inputs_sha256.into(),
        workspace: artifact(&bytes),
        joint_request: joint_request.as_deref().map(artifact),
    };
    let start_bytes = json(&start, MAX_METADATA_BYTES)?;
    let directory = root.join(&start.id);
    fs::create_dir(&directory).map_err(|e| e.to_string())?;
    let setup = (|| {
        let directory_guard = guard(&directory)?;
        let active = lock_file(&directory.join("active.lock"), true)?;
        active.try_lock().map_err(|e| e.to_string())?;
        write_new(&directory.join("workspace.json"), &bytes)?;
        if let Some(bytes) = &joint_request {
            write_new(&directory.join("joint-request.json"), bytes)?;
        }
        write_new(&directory.join("start.json"), &start_bytes)?;
        Ok::<_, String>((directory_guard, active))
    })();
    let (directory_guard, active) = match setup {
        Ok(value) => value,
        Err(error) => {
            // Remove only an empty directory exclusively created by this call.
            // Partial writes, locks and any unexpected files remain evidence.
            let _ = fs::remove_dir(&directory);
            return Err(format!(
                "Run {} could not start: {error}. Any partial artifacts were preserved for inspection.",
                start.id
            ));
        }
    };
    Ok(RunHandle {
        manifest,
        root,
        directory,
        start,
        start_sha256: hash(&start_bytes),
        _active: active,
        _directory: directory_guard,
        _ancestors: root_guards,
    })
}

/// Terminal publication is immutable. A failed write leaves the start receipt;
/// after the worker releases its OS lock this is visibly Interrupted.
pub fn finish(
    handle: RunHandle,
    outcome: &Result<ReportKind, String>,
    cancelled: bool,
) -> Result<RunSummary, String> {
    let (_root, _guard, _lock, identity) = open_root(&handle.manifest, false)?;
    if handle.root != root_path(&handle.manifest)? || identity != handle.start.case_manifest_sha256
    {
        return Err("Case changed during computation".into());
    }
    let (start, start_hash) = start_at(&handle.directory, &identity)?;
    if start_hash != handle.start_sha256 {
        return Err("Run start changed during computation".into());
    }
    let document: ReviewWorkspace = decode(&checked(
        &handle.directory.join("workspace.json"),
        &start.workspace,
        MAX_DOCUMENT_BYTES,
    )?)?;
    let joint_request = joint_request_at(&handle.directory, &start, &document)?;
    let (status, result, error) = match outcome {
        Ok(report) => {
            validate_report(&document, report)?;
            validate_joint_start_binding(&joint_request, report)?;
            let (doc_hash, input_hash, status) = report_identity(report);
            if doc_hash != start.document_sha256 || input_hash != start.inputs_sha256 {
                return Err(
                    "Result does not match the original reviewed workspace and inputs".into(),
                );
            }
            let bytes = json(report, MAX_RESULT_BYTES)?;
            write_new(&handle.directory.join("result.json"), &bytes)?;
            (
                if status == SearchStatus::Completed {
                    RunStatus::Completed
                } else {
                    RunStatus::Cancelled
                },
                Some(artifact(&bytes)),
                None,
            )
        }
        Err(message) => {
            let mut message = message.clone();
            while message.len() > 8192 {
                message.pop();
            }
            (
                if cancelled {
                    RunStatus::Cancelled
                } else {
                    RunStatus::Failed
                },
                None,
                Some(message),
            )
        }
    };
    let terminal = Terminal {
        schema_version: 1,
        start_sha256: start_hash,
        finished_unix_ms: now().max(start.created_unix_ms),
        status,
        result,
        error,
    };
    write_new(
        &handle.directory.join("terminal.json"),
        &json(&terminal, MAX_METADATA_BYTES)?,
    )?;
    Ok(summary_at(&handle.directory, &identity)?.0)
}

/// Explicit, bounded metadata read. No computation, source loading or networking.
pub fn list(manifest: &Path) -> Result<Vec<RunSummary>, String> {
    if !root_path(manifest)?.exists() {
        return Ok(Vec::new());
    }
    let (root, _guard, _lock, identity) = open_root(manifest, false)?;
    let mut rows = Vec::new();
    for directory in inventory(&root)? {
        match summary_at(&directory, &identity) {
            Ok(value) => rows.push(value.0),
            Err(error) => {
                let start = start_at(&directory, &identity).ok().map(|value| value.0);
                rows.push(RunSummary {
                    id: directory
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .into(),
                    created_unix_ms: start.as_ref().map_or(0, |start| start.created_unix_ms),
                    status: RunStatus::Interrupted,
                    inputs_sha256: start
                        .as_ref()
                        .map_or(String::new(), |start| start.inputs_sha256.clone()),
                    document_sha256: start
                        .as_ref()
                        .map_or(String::new(), |start| start.document_sha256.clone()),
                    has_result: false,
                    error: Some(format!(
                        "Run is incomplete or damaged; files preserved: {error}"
                    )),
                });
            }
        }
    }
    rows.sort_by(|a, b| {
        b.created_unix_ms
            .cmp(&a.created_unix_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(rows)
}
pub fn load(manifest: &Path, id: &str) -> Result<LoadedRun, String> {
    if !valid_id(id) {
        return Err("Invalid run id".into());
    }
    let (root, _guard, _lock, identity) = open_root(manifest, false)?;
    let directory = root.join(id);
    let _directory = guard(&directory)?;
    load_at(manifest, &directory, &identity)
}
fn load_at(manifest: &Path, directory: &Path, identity: &str) -> Result<LoadedRun, String> {
    let (summary, start, terminal) = summary_at(directory, identity)?;
    let document: ReviewWorkspace = decode(&checked(
        &directory.join("workspace.json"),
        &start.workspace,
        MAX_DOCUMENT_BYTES,
    )?)?;
    validate_document(manifest, &document)?;
    if regional_search::document_sha256(&document)? != start.document_sha256 {
        return Err("Workspace identity does not match run receipt".into());
    }
    let joint_request = joint_request_at(directory, &start, &document)?;
    let report = if let Some(artifact) = terminal.as_ref().and_then(|end| end.result.as_ref()) {
        let report: ReportKind = decode(&checked(
            &directory.join("result.json"),
            artifact,
            MAX_RESULT_BYTES,
        )?)?;
        validate_report(&document, &report)?;
        validate_joint_start_binding(&joint_request, &report)?;
        let (document_hash, inputs_hash, status) = report_identity(&report);
        if document_hash != start.document_sha256
            || inputs_hash != start.inputs_sha256
            || (status == SearchStatus::Completed) != (summary.status == RunStatus::Completed)
        {
            return Err(
                "Run report bindings or completion status do not match its receipts".into(),
            );
        }
        Some(report)
    } else {
        None
    };
    Ok(LoadedRun {
        summary,
        document,
        report,
        joint_request,
        result_sha256: terminal.and_then(|end| end.result.map(|artifact| artifact.sha256)),
    })
}
/// A basic result copy remains exportable after edits or loss of source files.
/// It deliberately contains no replay buffers and uses only the saved workspace.
pub fn export_copy(manifest: &Path, id: &str, destination: &Path) -> Result<(), String> {
    let loaded = load(manifest, id)?;
    match loaded
        .report
        .as_ref()
        .ok_or("This run has no result to export")?
    {
        ReportKind::Regional(report) => {
            investigation_report::export(destination, &loaded.document, report)?;
        }
        ReportKind::Comparison(report) => {
            investigation_report::export_modalities(destination, &loaded.document, report)?;
        }
        ReportKind::JointEnvelope(snapshot) => {
            investigation_report::export_joint(destination, &loaded.document, snapshot)?;
        }
    }
    Ok(())
}
/// Delete only one validated, inactive owned run. Never delete referenced data.
pub fn delete(manifest: &Path, id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err("Invalid run id".into());
    }
    let (root, _guard, _lock, identity) = open_root(manifest, false)?;
    let directory = root.join(id);
    let directory_guard = guard(&directory)?;
    let active = active_lock(&directory)?;
    if let Some(active) = &active {
        active
            .try_lock()
            .map_err(|_| "Cannot delete a running computation".to_string())?;
    }
    run_size(&directory)?;
    let marker_path = directory.join(DELETE_RECEIPT);
    let marker_bytes = if marker_path.exists() {
        read(&marker_path, MAX_METADATA_BYTES)?
    } else {
        // Full report validation and publication of the deletion inventory are
        // under the SAME history lock. No new workspace/source binding is used.
        let _ = load_at(manifest, &directory, &identity)?;
        let mut files = std::collections::BTreeMap::new();
        for name in run_files(&directory)? {
            let receipt = if name == "active.lock" {
                active_artifact(active.as_ref())?
            } else {
                artifact(&read(&directory.join(&name), MAX_RUN_BYTES)?)
            };
            files.insert(name, receipt);
        }
        let bytes = json(
            &Deletion {
                schema_version: 1,
                id: id.into(),
                case_manifest_sha256: identity.clone(),
                files,
            },
            MAX_METADATA_BYTES,
        )?;
        write_new(&marker_path, &bytes)?;
        bytes
    };
    let deletion: Deletion = decode(&marker_bytes)?;
    validate_deletion(&deletion, id, &identity)?;
    // Verify ALL surviving artifacts before deleting any of them. A damaged
    // receipt or unexpected/replaced file stops deletion without further loss.
    let present = run_files(&directory)?;
    for name in &present {
        if name == DELETE_RECEIPT {
            continue;
        }
        let expected = deletion
            .files
            .get(name)
            .ok_or("Unexpected file after deletion began; remaining artifacts preserved")?;
        if name == "active.lock" {
            let receipt = active_artifact(active.as_ref())?;
            if expected.bytes != receipt.bytes || expected.sha256 != receipt.sha256 {
                return Err("Run active lock receipt changed; files preserved".into());
            }
        } else {
            checked(&directory.join(name), expected, MAX_RUN_BYTES)?;
        }
    }
    for name in present
        .iter()
        .filter(|name| name.as_str() != "active.lock" && name.as_str() != DELETE_RECEIPT)
    {
        remove_checked(&directory.join(name), &deletion.files[name])?;
    }
    // Other GeoSonnar operations cannot acquire this root lock in the gap.
    drop(active);
    if present.iter().any(|name| name == "active.lock") {
        remove_checked(
            &directory.join("active.lock"),
            &deletion.files["active.lock"],
        )?;
    }
    remove_checked(&marker_path, &artifact(&marker_bytes))?;
    drop(directory_guard);
    fs::remove_dir(directory).map_err(|e| format!("Run deletion incomplete: {e}"))?;
    Ok(())
}

fn validate_deletion(deletion: &Deletion, id: &str, identity: &str) -> Result<(), String> {
    if deletion.schema_version != 1
        || deletion.id != id
        || !valid_id(id)
        || deletion.case_manifest_sha256 != identity
        || !valid_hash(identity)
        || deletion.files.len() > MAX_RUN_FILES
        || !["active.lock", "start.json", "workspace.json"]
            .iter()
            .all(|name| deletion.files.contains_key(*name))
        || deletion
            .files
            .get("active.lock")
            .is_none_or(|file| file.bytes != 0 || file.sha256 != hash(b""))
        || deletion.files.iter().any(|(name, file)| {
            !run_artifact_name(name) || !valid_hash(&file.sha256) || file.bytes > MAX_RUN_BYTES
        })
        || deletion.files.values().map(|file| file.bytes).sum::<u64>() > MAX_RUN_BYTES
    {
        return Err("Invalid deletion inventory; remaining run artifacts preserved".into());
    }
    Ok(())
}

fn remove_checked(path: &Path, expected: &Artifact) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // DELETE + GENERIC_READ, no write/delete sharing: inspect and delete the
        // same opened file rather than reopening a possibly replaced pathname.
        options
            .access_mode(0x8000_0000 | 0x0001_0000)
            .share_mode(1)
            .custom_flags(0x0020_0000);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("Run deletion incomplete; retry Delete: {e}"))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file()
        || linked(&meta)
        || meta.len() != expected.bytes
        || expected.bytes > MAX_RUN_BYTES
        || !valid_hash(&expected.sha256)
    {
        return Err("Run artifact changed before deletion; remaining files preserved".into());
    }
    single_link(&file)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > expected.bytes {
            return Err("Run artifact grew before deletion; files preserved".into());
        }
        digest.update(&buffer[..read]);
    }
    if total != expected.bytes || format!("{:x}", digest.finalize()) != expected.sha256 {
        return Err("Run artifact changed before deletion; files preserved".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetFileInformationByHandle(
                handle: *mut std::ffi::c_void,
                class: u32,
                info: *const std::ffi::c_void,
                size: u32,
            ) -> i32;
        }
        let delete_file = 1_u8;
        if unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                4,
                (&delete_file as *const u8).cast(),
                1,
            )
        } == 0
        {
            return Err(format!(
                "Run deletion incomplete; retry Delete: {}",
                std::io::Error::last_os_error()
            ));
        }
        // FILE_DISPOSITION_INFO keeps the inspected object pending deletion
        // until this handle closes; no pathname replacement window is opened.
        drop(file);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let current = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !current.is_file() || linked(&current) {
            return Err("Run artifact changed before deletion".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if current.dev() != meta.dev() || current.ino() != meta.ino() {
                return Err("Run artifact was replaced before deletion".into());
            }
        }
        fs::remove_file(path).map_err(|e| format!("Run deletion incomplete; retry Delete: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "geosonnar-history-unit-{}-{}-{}",
                std::process::id(),
                now(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn owned_root() -> (Scratch, PathBuf, PathBuf, String) {
        let scratch = Scratch::new();
        let manifest = scratch.0.join("case.json");
        fs::write(&manifest, b"{}").unwrap();
        let (root, guards, lock, identity) = open_root(&manifest, true).unwrap();
        drop(lock);
        drop(guards);
        (scratch, manifest, root, identity)
    }
    fn interrupted_delete(root: &Path, identity: &str) -> (PathBuf, Deletion) {
        let id = "run-1-2-3";
        let directory = root.join(id);
        fs::create_dir(&directory).unwrap();
        let mut files = std::collections::BTreeMap::new();
        // Simulate the inventory already validated before a process interruption.
        for (name, bytes) in [
            ("active.lock", &b""[..]),
            ("workspace.json", &b"original workspace"[..]),
            ("start.json", &b"original start"[..]),
            ("result.json", &b"original result"[..]),
        ] {
            fs::write(directory.join(name), bytes).unwrap();
            files.insert(name.into(), artifact(bytes));
        }
        let receipt = Deletion {
            schema_version: 1,
            id: id.into(),
            case_manifest_sha256: identity.into(),
            files,
        };
        write_new(
            &directory.join(DELETE_RECEIPT),
            &json(&receipt, MAX_METADATA_BYTES).unwrap(),
        )
        .unwrap();
        (directory, receipt)
    }

    #[test]
    fn serialization_is_bounded_and_publication_never_replaces() {
        assert!(json(&vec![0_u8; 1000], 100).is_err());
        let scratch = Scratch::new();
        let path = scratch.0.join("terminal.json");
        write_new(&path, b"first").unwrap();
        assert!(write_new(&path, b"second").is_err());
        assert_eq!(fs::read(path).unwrap(), b"first");
        assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 1);
    }

    #[test]
    fn empty_active_receipt_is_checked_through_the_locked_handle() {
        let scratch = Scratch::new();
        let active = lock_file(&scratch.0.join("active.lock"), true).unwrap();
        active.try_lock().unwrap();
        let receipt = active_artifact(Some(&active)).unwrap();
        assert_eq!(receipt.bytes, 0);
        assert_eq!(receipt.sha256, hash(b""));
        assert!(active_artifact(None).is_err());
    }

    #[test]
    fn hard_links_cannot_be_read_or_deleted_as_owned_artifacts() {
        let scratch = Scratch::new();
        let original = scratch.0.join("source-data");
        let alias = scratch.0.join("result.json");
        fs::write(&original, b"preserve source").unwrap();
        fs::hard_link(&original, &alias).unwrap();
        assert!(read(&alias, 1024).is_err());
        assert!(remove_checked(&alias, &artifact(b"preserve source")).is_err());
        assert_eq!(fs::read(&original).unwrap(), b"preserve source");
        assert!(alias.exists());
    }

    #[test]
    fn deletion_uses_checked_bytes_and_preserves_changed_files() {
        let scratch = Scratch::new();
        let path = scratch.0.join("result.json");
        fs::write(&path, b"changed").unwrap();
        assert!(remove_checked(&path, &artifact(b"original")).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"changed");
        remove_checked(&path, &artifact(b"changed")).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn damaged_rows_remain_visible_without_hiding_other_runs() {
        let (_scratch, manifest, root, identity) = owned_root();
        let directory = root.join("run-1-1-1");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("active.lock"), []).unwrap();
        let start = Start {
            schema_version: 1,
            id: "run-1-1-1".into(),
            created_unix_ms: 1,
            version: "test".into(),
            case_manifest_sha256: identity,
            document_sha256: hash(b"document"),
            inputs_sha256: hash(b"inputs"),
            workspace: artifact(b"{}"),
            joint_request: None,
        };
        fs::write(
            directory.join("start.json"),
            json(&start, MAX_METADATA_BYTES).unwrap(),
        )
        .unwrap();
        let damaged = root.join("run-2-2-2");
        fs::create_dir(&damaged).unwrap();
        fs::write(damaged.join("start.json"), b"{").unwrap();
        fs::write(damaged.join("workspace.pending-123-4"), b"partial").unwrap();
        assert_eq!(run_size(&damaged).unwrap(), 8);
        let rows = list(&manifest).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .any(|row| row.id == "run-1-1-1" && row.error.is_none())
        );
        assert!(rows.iter().any(|row| row.id == "run-2-2-2"
            && row.status == RunStatus::Interrupted
            && row.error.is_some()));
        assert!(damaged.join("workspace.pending-123-4").exists());
    }

    #[test]
    fn interrupted_deletion_resumes_after_original_start_was_removed() {
        let (scratch, manifest, root, identity) = owned_root();
        let source = scratch.0.join("original-recording");
        fs::write(&source, b"source stays").unwrap();
        let (directory, receipt) = interrupted_delete(&root, &identity);
        fs::remove_file(directory.join("start.json")).unwrap();
        let rows = list(&manifest).unwrap();
        assert_eq!(rows[0].status, RunStatus::Interrupted);
        delete(&manifest, &receipt.id).unwrap();
        assert!(!directory.exists());
        assert_eq!(fs::read(source).unwrap(), b"source stays");
    }

    #[test]
    fn resumed_deletion_validates_all_remaining_files_before_removal() {
        let (_scratch, manifest, root, identity) = owned_root();
        let (directory, receipt) = interrupted_delete(&root, &identity);
        fs::remove_file(directory.join("start.json")).unwrap();
        fs::write(directory.join("result.json"), b"changed result").unwrap();
        assert!(delete(&manifest, &receipt.id).is_err());
        assert_eq!(
            fs::read(directory.join("workspace.json")).unwrap(),
            b"original workspace"
        );
        assert!(directory.join("active.lock").exists());
        assert!(directory.join(DELETE_RECEIPT).exists());
    }

    #[test]
    fn unexpected_files_and_path_names_are_preserved() {
        let (_scratch, manifest, root, identity) = owned_root();
        let (directory, mut receipt) = interrupted_delete(&root, &identity);
        fs::write(directory.join("unrecognized.txt"), b"keep").unwrap();
        assert!(delete(&manifest, &receipt.id).is_err());
        assert!(directory.join("workspace.json").exists());
        assert_eq!(
            fs::read(directory.join("unrecognized.txt")).unwrap(),
            b"keep"
        );
        receipt
            .files
            .insert("../recording.wav".into(), artifact(b""));
        assert!(validate_deletion(&receipt, &receipt.id, &identity).is_err());
        assert!(!run_artifact_name("result.pending-1-2/elsewhere"));
    }

    #[cfg(windows)]
    #[test]
    fn held_case_ancestors_and_manifest_prevent_path_rebinding() {
        let (scratch, manifest, _root, _identity) = owned_root();
        let (_root, guards, lock, _) = open_root(&manifest, false).unwrap();
        assert!(fs::write(&manifest, b"replacement").is_err());
        let renamed = scratch.0.with_extension("moved");
        let rename = fs::rename(&scratch.0, &renamed);
        if rename.is_ok() {
            fs::rename(&renamed, &scratch.0).unwrap();
        }
        assert!(rename.is_err());
        drop(lock);
        drop(guards);
        fs::write(manifest, b"replacement").unwrap();
    }
}
