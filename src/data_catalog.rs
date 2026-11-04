//! Case-local owned data copies. Sources and old case/job evidence are never moved.
//! Destructive operations remove only individually registered regular files.
#[path = "fmi_catalog.rs"]
pub mod fmi;
#[path = "dataset_requests.rs"]
pub mod requests;
use crate::{
    adsblol::{self, TraceImportConfig},
    adsblol_acquisition as acquisition, adsblol_history as history, casework,
    detailed_data::{self, sha256},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);
const MAX_CATALOG_FILES: usize = 250_010;
const MAX_METADATA_BYTES: u64 = 128 * 1024 * 1024;
fn nonce() -> Result<String, String> {
    Ok(format!(
        "{}-{}-{}",
        now()?.as_nanos(),
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    Adsb,
    Lightning,
    Terrain,
    Other,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub label: String,
    pub kind: DataKind,
    pub date_utc: Option<String>,
    pub bounds: Option<[f64; 4]>,
    pub notes: String,
    pub max_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogFile {
    pub relative_path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    pub schema_version: u32,
    pub id: String,
    pub case_manifest_sha256: String,
    pub label: String,
    pub kind: DataKind,
    pub format: String,
    pub primary_file: PathBuf,
    pub created_unix_s: u64,
    pub source: Value,
    pub files: Vec<CatalogFile>,
    pub date_utc: Option<String>,
    pub bounds: Option<[f64; 4]>,
    pub notes: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct CatalogItem {
    pub entry: CatalogEntry,
    pub status: String,
    pub available_bytes: u64,
    pub problem: Option<String>,
    /// Listing checks paths, lengths and bounded provider status metadata;
    /// use verified_files before reading data or claiming source integrity.
    pub integrity_verified: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeleteReceipt {
    pub id: String,
    pub deleted_files: usize,
    pub deleted_bytes: u64,
    pub originals_preserved: bool,
}
#[derive(Clone, Debug)]
pub struct AdsbPlan {
    pub catalog_id: String,
    pub plan_path: PathBuf,
    pub plan: acquisition::DownloadPlan,
}
#[derive(Clone, Debug)]
pub struct HistoryItem {
    pub entry: CatalogEntry,
    pub plan: history::HistoryPlan,
    pub report: Option<history::HistoryReport>,
}
#[derive(Clone, Debug, Serialize)]
pub struct DownloadProgress {
    pub planned_bytes: u64,
    pub downloaded_bytes: u64,
    pub complete: bool,
    pub notes: Vec<String>,
}

fn now() -> Result<std::time::Duration, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())
}
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn regular(path: &Path, directory: bool) -> Result<(), String> {
    let m = fs::symlink_metadata(path)
        .map_err(|e| format!("Managed path unavailable {}: {e}", path.display()))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return Err("Managed paths must not be Windows reparse points".into());
        }
    }
    if m.file_type().is_symlink() || (directory && !m.is_dir()) || (!directory && !m.is_file()) {
        return Err("Managed path has an unexpected file type".into());
    }
    Ok(())
}
fn child(root: &Path, relative: &Path, exists: bool) -> Result<PathBuf, String> {
    if relative.as_os_str().is_empty()
        || relative.components().any(|c| match c {
            Component::Normal(s) => s.to_str().is_none_or(|s| s.contains(':')),
            _ => true,
        })
    {
        return Err("Managed file path must be a confined relative path".into());
    }
    let mut path = root.to_path_buf();
    let count = relative.components().count();
    for (i, c) in relative.components().enumerate() {
        path.push(c.as_os_str());
        if path.symlink_metadata().is_ok() {
            regular(&path, i + 1 < count)?;
        } else if exists || i + 1 < count {
            return Err(format!("Managed file is missing: {}", path.display()));
        }
    }
    if exists
        && !path
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(root)
    {
        return Err("Managed path escaped its registered directory".into());
    }
    Ok(path)
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    regular(path, false)?;
    let file = File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_METADATA_BYTES {
        return Err("Catalog metadata exceeds 128 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err("Catalog metadata grew beyond its limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
fn write_new(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err("Catalog metadata exceeds 128 MiB".into());
    }
    let staged = path.with_file_name(format!(".pending-{}", nonce()?));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&staged)
        .map_err(|e| e.to_string())?;
    if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&staged);
        return Err(error.to_string());
    }
    drop(file);
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
        }
        let from: Vec<_> = staged.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x8) } == 0 {
            let error = std::io::Error::last_os_error().to_string();
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        if let Err(error) = fs::hard_link(&staged, path) {
            let _ = fs::remove_file(&staged);
            return Err(error.to_string());
        }
        fs::remove_file(staged).map_err(|e| e.to_string())
    }
}
fn base(manifest: &Path, create: bool) -> Result<Option<(PathBuf, String)>, String> {
    let manifest = manifest.canonicalize().map_err(|e| e.to_string())?;
    let before = sha256(&manifest)?;
    casework::read_case(&manifest, false)?;
    if sha256(&manifest)? != before {
        return Err("Case changed during catalog access".into());
    }
    let directory = manifest
        .parent()
        .ok_or("Case has no directory")?
        .join("managed-data");
    if !directory.exists() {
        if !create {
            return Ok(None);
        }
        fs::create_dir(&directory).map_err(|e| e.to_string())?;
        write_new(
            &directory.join("owner.json"),
            &json!({"schema_version":1,"case_manifest_sha256":before,"scope":"owned data copies"}),
        )?;
    }
    regular(&directory, true)?;
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    let owner: Value = read(&directory.join("owner.json"))?;
    if owner["schema_version"] != 1 || owner["case_manifest_sha256"] != before {
        return Err("Managed data directory belongs to a different case".into());
    }
    Ok(Some((directory, before)))
}
fn item_dir(manifest: &Path, id: &str) -> Result<(PathBuf, String), String> {
    if !safe_id(id) {
        return Err("Invalid catalog identity".into());
    }
    let (root, hash) = base(manifest, false)?.ok_or("Case has no managed data")?;
    let path = root.join(id);
    regular(&path, true)?;
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(root) {
        return Err("Catalog item escaped its case".into());
    }
    Ok((path, hash))
}
fn lock(directory: &Path) -> Result<File, String> {
    let path = directory.join(".busy");
    if path.exists() {
        regular(&path, false)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.try_lock()
        .map_err(|_| "This data item is busy; wait for its operation to stop".to_string())?;
    Ok(file)
}
fn entry(directory: &Path, case_hash: &str) -> Result<CatalogEntry, String> {
    let mut path = if directory.join("completed.json").exists() {
        directory.join("completed.json")
    } else {
        directory.join("entry.json")
    };
    let initial: CatalogEntry = read(&path)?;
    if matches!(
        initial.format.as_str(),
        "adsblol_history_plan" | "fmi_lightning_plan"
    ) {
        let prefix = if initial.format == "fmi_lightning_plan" {
            "fmi"
        } else {
            "history"
        };
        for revision in 1..=1024 {
            let next = directory.join(format!("{prefix}-registration-{revision:08}.json"));
            if !next.exists() {
                break;
            }
            regular(&next, false)?;
            path = next;
        }
    }
    let entry: CatalogEntry = if path.file_name().and_then(|s| s.to_str()).is_some_and(|s| {
        s.starts_with("history-registration-") || s.starts_with("fmi-registration-")
    }) {
        read(&path)?
    } else {
        initial
    };
    if entry.schema_version != 1
        || entry.case_manifest_sha256 != case_hash
        || directory.file_name().and_then(|p| p.to_str()) != Some(&entry.id)
        || !safe_id(&entry.id)
        || entry.files.len() > MAX_CATALOG_FILES
    {
        return Err("Invalid catalog ownership record".into());
    }
    let mut seen = BTreeSet::new();
    for file in &entry.files {
        child(directory, &file.relative_path, false)?;
        if !seen.insert(file.relative_path.clone())
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("Invalid catalog file inventory".into());
        }
    }
    if !seen.contains(&entry.primary_file) {
        return Err("Primary catalog file is not registered".into());
    }
    Ok(entry)
}
fn identity(directory: &Path, path: &Path) -> Result<CatalogFile, String> {
    regular(path, false)?;
    let relative = path
        .strip_prefix(directory)
        .map_err(|_| "File is not owned by this catalog item")?
        .to_path_buf();
    child(directory, &relative, true)?;
    Ok(CatalogFile {
        relative_path: relative,
        bytes: fs::metadata(path).map_err(|e| e.to_string())?.len(),
        sha256: sha256(path)?,
    })
}
fn checked(directory: &Path, file: &CatalogFile) -> Result<PathBuf, String> {
    checked_with_checkpoint(directory, file, &mut || Ok(()))
}
fn checked_with_checkpoint(
    directory: &Path,
    file: &CatalogFile,
    checkpoint: &mut impl FnMut() -> Result<(), String>,
) -> Result<PathBuf, String> {
    checkpoint()?;
    let path = child(directory, &file.relative_path, true)?;
    if fs::metadata(&path).map_err(|e| e.to_string())?.len() != file.bytes
        || detailed_data::sha256_with_checkpoint(&path, checkpoint)? != file.sha256
    {
        return Err(format!(
            "Managed copy changed: {}",
            file.relative_path.display()
        ));
    }
    Ok(path)
}

#[cfg(windows)]
fn remove_registered(directory: &Path, entry: &CatalogFile) -> Result<(), String> {
    use std::ffi::{OsString, c_void};
    use std::os::windows::{
        ffi::OsStringExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFinalPathNameByHandleW(
            file: *mut c_void,
            buffer: *mut u16,
            size: u32,
            flags: u32,
        ) -> u32;
        fn SetFileInformationByHandle(
            file: *mut c_void,
            class: u32,
            data: *const c_void,
            size: u32,
        ) -> i32;
    }
    let path = child(directory, &entry.relative_path, true)?;
    // DELETE access, no write/delete sharing, and OPEN_REPARSE_POINT prevent
    // a replacement file or final symlink being substituted after this open.
    let mut file = OpenOptions::new()
        .read(true)
        .access_mode(0x80000000 | 0x00010000)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.file_attributes() & 0x400 != 0
        || metadata.len() != entry.bytes
    {
        return Err("Managed deletion handle has changed type or size".into());
    }
    let mut buffer = vec![0u16; 32768];
    let n = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            0,
        )
    };
    if n == 0 || n as usize >= buffer.len() {
        return Err("Cannot resolve the owned deletion handle".into());
    }
    let final_path = PathBuf::from(OsString::from_wide(&buffer[..n as usize]));
    if !final_path.starts_with(directory)
        || final_path != path.canonicalize().map_err(|e| e.to_string())?
    {
        return Err("Deletion handle resolved outside its registered owned path".into());
    }
    let mut digest = Sha256::new();
    let mut data = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut data).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&data[..n]);
    }
    if format!("{:x}", digest.finalize()) != entry.sha256 {
        return Err("Managed copy changed before deletion".into());
    }
    let delete: u8 = 1;
    if unsafe {
        SetFileInformationByHandle(file.as_raw_handle(), 4, (&delete as *const u8).cast(), 1)
    } == 0
    {
        return Err(format!(
            "Cannot delete registered copy: {}",
            std::io::Error::last_os_error()
        ));
    }
    drop(file);
    Ok(())
}
#[cfg(not(windows))]
fn remove_registered(directory: &Path, entry: &CatalogFile) -> Result<(), String> {
    let path = checked(directory, entry)?;
    fs::remove_file(path).map_err(|e| e.to_string())
}
fn validate_bounds(bounds: Option<[f64; 4]>) -> Result<(), String> {
    if let Some([s, w, n, e]) = bounds
        && (![s, w, n, e].iter().all(|x| x.is_finite())
            || s < -90.
            || n > 90.
            || s >= n
            || !(-180. ..=180.).contains(&w)
            || !(-180. ..=180.).contains(&e)
            || w == e)
    {
        return Err(
            "Invalid south,west,north,east region; date-line crossing uses west > east".into(),
        );
    }
    Ok(())
}
fn new_item(manifest: &Path) -> Result<(PathBuf, String, String), String> {
    let (root, hash) = base(manifest, true)?.unwrap();
    let id = format!("data-{}", nonce()?);
    let path = root.join(&id);
    fs::create_dir(&path).map_err(|e| e.to_string())?;
    Ok((path, id, hash))
}
fn unchanged_case(manifest: &Path, expected: &str) -> Result<(), String> {
    if sha256(manifest)? != expected {
        Err("Case changed during its data operation".into())
    } else {
        Ok(())
    }
}
pub fn import_file(
    manifest: &Path,
    source: &Path,
    request: &ImportRequest,
) -> Result<CatalogEntry, String> {
    import_file_with_cancel(manifest, source, request, &AtomicBool::new(false))
}
pub fn import_file_with_cancel(
    manifest: &Path,
    source: &Path,
    request: &ImportRequest,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Import canceled before reading the source".into());
    }
    validate_bounds(request.bounds)?;
    if request.label.trim().is_empty()
        || request.label.len() > 240
        || request.notes.len() > 20_000
        || request.max_bytes == 0
        || request.max_bytes > 1_000_000_000_000
    {
        return Err("Use a label and a positive import limit of at most 1 TB".into());
    }
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    regular(&source, false)?;
    let bytes = fs::metadata(&source).map_err(|e| e.to_string())?.len();
    if bytes > request.max_bytes {
        return Err("Selected file exceeds the declared import size limit".into());
    }
    let mut reader = File::open(&source).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Import canceled while checking the source".into());
        }
        let n = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    let hash = format!("{:x}", digest.finalize());
    let (directory, id, case_hash) = new_item(manifest)?;
    let _lock = lock(&directory)?;
    let extension = source
        .extension()
        .and_then(|p| p.to_str())
        .filter(|p| p.len() <= 12 && p.bytes().all(|b| b.is_ascii_alphanumeric()))
        .unwrap_or("bin");
    let name = PathBuf::from(format!("payload.{extension}"));
    let destination = directory.join(&name);
    let mut target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|e| e.to_string())?;
    let mut input = File::open(&source)
        .map_err(|e| e.to_string())?
        .take(request.max_bytes + 1);
    let mut copied = 0u64;
    let mut canceled = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            canceled = true;
            break;
        }
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        target.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        copied += n as u64;
    }
    target.sync_all().map_err(|e| e.to_string())?;
    drop(target);
    let complete =
        !canceled && copied == bytes && sha256(&source)? == hash && sha256(&destination)? == hash;
    let value = CatalogEntry {
        schema_version: 1,
        id,
        case_manifest_sha256: case_hash,
        label: request.label.clone(),
        kind: request.kind,
        format: if complete {
            "local_file"
        } else {
            "incomplete_import"
        }
        .into(),
        primary_file: name,
        created_unix_s: now()?.as_secs(),
        source: json!({"type":"local_import","path":source,"bytes":bytes,"sha256":hash,"original_preserved":true,"copy_complete":complete}),
        files: vec![identity(&directory, &destination)?],
        date_utc: request.date_utc.clone(),
        bounds: request.bounds,
        notes: request.notes.clone(),
    };
    unchanged_case(manifest, &value.case_manifest_sha256)?;
    write_new(&directory.join("entry.json"), &value)?;
    if !complete {
        return Err(format!(
            "Import canceled or source changed; incomplete managed copy {} can be deleted in Data",
            value.id
        ));
    }
    Ok(value)
}
/// Summarize the registered FMI state without decoding response bodies or
/// treating a generic catalog lock as proof of an active download. Live
/// transfer progress belongs to the worker; this is the last published state.
fn fmi_library_status(directory: &Path, entry: &CatalogEntry) -> Result<&'static str, String> {
    if entry.kind != DataKind::Lightning {
        return Err("FMI catalog entry has an inconsistent data kind".into());
    }
    match entry.format.as_str() {
        "fmi_lightning_plan" => return Ok("planned"),
        "fmi_lightning" => return Ok("imported"),
        "fmi_lightning_download" => {}
        _ => return Err("Unrecognized FMI catalog format".into()),
    }
    // Metadata only: bound and hash the exact report buffer before using its
    // counters. This does not certify the underlying response inventory, so
    // CatalogItem::integrity_verified remains false until explicit verification.
    const MAX_REPORT_BYTES: u64 = 1024 * 1024;
    let report_file = entry
        .files
        .iter()
        .find(|file| file.relative_path == entry.primary_file)
        .ok_or("FMI report is not registered")?;
    if report_file.bytes == 0 || report_file.bytes > MAX_REPORT_BYTES {
        return Err("FMI report exceeds the bounded metadata size".into());
    }
    let path = child(directory, &report_file.relative_path, true)?;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_REPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != report_file.bytes
        || format!("{:x}", Sha256::digest(&bytes)) != report_file.sha256
    {
        return Err("FMI report changed; reopen and verify the stored request".into());
    }
    let report: fmi::FmiReport = serde_json::from_slice(&bytes)
        .map_err(|_| "FMI report has an unsupported or invalid schema")?;
    let request: crate::fmi_lightning::FmiRequest =
        serde_json::from_value(entry.source["request"].clone())
            .map_err(|_| "FMI request metadata is missing or invalid")?;
    let plan = crate::fmi_lightning::plan(&request)?;
    let plan_hash = crate::fmi_lightning::plan_sha256(&plan)?;
    if report.schema_version != 1
        || report.plan_sha256 != plan_hash
        || entry.source["plan_sha256"].as_str() != Some(plan_hash.as_str())
        || entry.source["status"]
            != serde_json::to_value(report.status).map_err(|e| e.to_string())?
        || report.chunks_total != plan.chunks.len()
        || report.chunks_complete > report.chunks_total
        || report.chunks_complete > report.attempts
        || (report.status == fmi::FmiStatus::Complete)
            != (report.chunks_complete == report.chunks_total)
        || report.owned_rows > report.raw_rows
        || (report.chunks_complete == 0 && (report.raw_rows != 0 || report.owned_rows != 0))
        || report.owned_rows > crate::fmi_lightning::MAX_STRIKES
        || report.transferred_bytes > plan.request.max_total_bytes.saturating_add(1)
        || report.attempts > 240
        || report.errors.len() > 242
        || report.errors.iter().any(|error| error.len() > 5000)
        || report.source_coverage_complete
        || report.location_verified
    {
        return Err("FMI report state differs from its registered request or counters".into());
    }
    Ok(match report.status {
        fmi::FmiStatus::Complete if report.owned_rows == 0 => "complete_empty",
        fmi::FmiStatus::Complete => "complete",
        fmi::FmiStatus::Cancelled => "cancelled",
        fmi::FmiStatus::Partial if report.chunks_complete == 0 && !report.errors.is_empty() => {
            "failed"
        }
        fmi::FmiStatus::Partial => "partial",
    })
}

pub fn list(manifest: &Path) -> Result<Vec<CatalogItem>, String> {
    let Some((root, hash)) = base(manifest, false)? else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for item in fs::read_dir(&root).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        if path
            .file_name()
            .and_then(|s| s.to_str())
            .is_none_or(|s| !s.starts_with("data-"))
        {
            continue;
        }
        regular(&path, true)?;
        if !path.join("entry.json").exists() {
            continue;
        }
        let entry = entry(&path, &hash)?;
        let mut available_bytes = 0u64;
        let mut problem = None;
        for file in &entry.files {
            match child(&path, &file.relative_path, true)
                .and_then(|p| fs::metadata(p).map_err(|e| e.to_string()))
            {
                Ok(m) => {
                    available_bytes = available_bytes.saturating_add(m.len());
                    if m.len() != file.bytes {
                        problem = Some("Managed copy size changed; verify before use".into());
                    }
                }
                Err(e) => problem = Some(e),
            }
        }
        let status = if path.join("deleted.json").exists() {
            "deleted"
        } else if path.join("deletion.json").exists() {
            "deletion_pending"
        } else if problem.is_some() {
            "unavailable"
        } else if entry.format.starts_with("fmi_lightning") {
            match fmi_library_status(&path, &entry) {
                Ok(status) => status,
                Err(error) => {
                    problem = Some(error);
                    "unavailable"
                }
            }
        } else if entry.format.starts_with("incomplete_") {
            "incomplete"
        } else if entry.format == "adsblol_plan" || entry.format == "adsblol_history_plan" {
            "planned"
        } else if entry.format == "adsblol_history"
            && entry.source.get("history_status").and_then(Value::as_str) != Some("complete")
        {
            "partial"
        } else {
            "available"
        };
        result.push(CatalogItem {
            entry,
            status: status.into(),
            available_bytes,
            problem,
            integrity_verified: false,
        });
        if result.len() > 10_000 {
            return Err("Catalog exceeds 10,000 entries".into());
        }
    }
    result.sort_by(|a, b| {
        b.entry
            .created_unix_s
            .cmp(&a.entry.created_unix_s)
            .then(a.entry.id.cmp(&b.entry.id))
    });
    Ok(result)
}
pub fn verified_files(manifest: &Path, id: &str) -> Result<Vec<PathBuf>, String> {
    verified_files_with_checkpoint(manifest, id, || Ok(()))
}
pub fn verified_files_with_cancel(
    manifest: &Path,
    id: &str,
    cancel: &AtomicBool,
) -> Result<Vec<PathBuf>, String> {
    verified_files_with_checkpoint(manifest, id, || catalog_checkpoint(cancel))
}
fn catalog_checkpoint(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Catalog verification cancelled".into())
    } else {
        Ok(())
    }
}
pub fn verified_files_with_checkpoint(
    manifest: &Path,
    id: &str,
    mut checkpoint: impl FnMut() -> Result<(), String>,
) -> Result<Vec<PathBuf>, String> {
    checkpoint()?;
    let (directory, hash) = item_dir(manifest, id)?;
    let _lock = lock(&directory)?;
    if directory.join("deletion.json").exists() {
        return Err("This managed copy has been deleted or is pending deletion".into());
    }
    let entry = entry(&directory, &hash)?;
    if entry.format.starts_with("incomplete_") {
        return Err("This managed copy is incomplete and cannot be used as data".into());
    }
    let files = entry
        .files
        .iter()
        .map(|f| checked_with_checkpoint(&directory, f, &mut checkpoint))
        .collect::<Result<Vec<_>, _>>()?;
    checkpoint()?;
    Ok(files)
}
fn plan_entry(
    directory: &Path,
    hash: &str,
) -> Result<(CatalogEntry, acquisition::DownloadPlan), String> {
    let entry = entry(directory, hash)?;
    if entry.format != "adsblol_plan" && entry.format != "adsblol_archive" {
        return Err("Select an ADSB.lol acquisition plan".into());
    }
    let file = entry
        .files
        .iter()
        .find(|f| f.relative_path == Path::new("plan/download-plan.json"))
        .ok_or("Catalog has no registered archive plan")?;
    let plan = read(&checked(directory, file)?)?;
    Ok((entry, plan))
}
fn cache_inventory(
    directory: &Path,
    plan: &acquisition::DownloadPlan,
) -> Result<Vec<CatalogFile>, String> {
    let cache = directory.join("cache");
    if !cache.exists() {
        return Ok(Vec::new());
    }
    regular(&cache, true)?;
    let mut names = BTreeSet::from([
        "archive-manifest.json".to_string(),
        "archive-manifest.json.pending".to_string(),
    ]);
    for asset in &plan.assets {
        for suffix in [
            "",
            ".part",
            ".part-identity.json",
            ".chunk",
            ".chunk-headers",
        ] {
            names.insert(format!("{}{suffix}", asset.name));
        }
    }
    let mut result = Vec::new();
    for value in fs::read_dir(cache).map_err(|e| e.to_string())? {
        let path = value.map_err(|e| e.to_string())?.path();
        let name = path
            .file_name()
            .and_then(|p| p.to_str())
            .ok_or("Invalid cache filename")?;
        let pending = name
            .strip_prefix("archive-manifest.json.pending.")
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit() || b == b'.'));
        if !names.contains(name) && !pending {
            return Err(
                "Unregistered file in acquisition cache; refusing to manage or delete it".into(),
            );
        }
        result.push(identity(directory, &path)?);
    }
    result.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(result)
}
pub fn plan_adsblol(
    manifest: &Path,
    request: &acquisition::AcquisitionRequest,
    bounds: Option<[f64; 4]>,
) -> Result<AdsbPlan, String> {
    validate_bounds(bounds)?;
    let (directory, id, hash) = new_item(manifest)?;
    let _lock = lock(&directory)?;
    let plan_directory = directory.join("plan");
    let plan = acquisition::plan(request, &plan_directory)?;
    let mut files = Vec::new();
    for f in fs::read_dir(&plan_directory).map_err(|e| e.to_string())? {
        files.push(identity(&directory, &f.map_err(|e| e.to_string())?.path())?);
    }
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let value=CatalogEntry{schema_version:1,id:id.clone(),case_manifest_sha256:hash,label:format!("ADSB.lol {}",request.date_utc),kind:DataKind::Adsb,format:"adsblol_plan".into(),primary_file:"plan/download-plan.json".into(),created_unix_s:now()?.as_secs(),source:json!({"provider":"ADSB.lol","license":"ODbL-1.0","request":request,"requested_import_bounds":bounds}),files,date_utc:Some(request.date_utc.clone()),bounds,notes:"The provider downloads complete global day archives. Region/time selection applies during native import, not during download. Aircraft coverage is incomplete.".into()};
    unchanged_case(manifest, &value.case_manifest_sha256)?;
    write_new(&directory.join("entry.json"), &value)?;
    Ok(AdsbPlan {
        catalog_id: id,
        plan_path: plan_directory.join("download-plan.json"),
        plan,
    })
}
pub fn fetch_adsblol(manifest: &Path, id: &str) -> Result<CatalogEntry, String> {
    fetch_adsblol_with_cancel(manifest, id, &AtomicBool::new(false))
}
pub fn fetch_adsblol_with_cancel(
    manifest: &Path,
    id: &str,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, String> {
    let (directory, hash) = item_dir(manifest, id)?;
    let _lock = lock(&directory)?;
    if directory.join("deletion.json").exists() {
        return Err("Cannot download into a deleted catalog item".into());
    }
    let (mut entry, plan) = plan_entry(&directory, &hash)?;
    for file in &entry.files {
        checked(&directory, file)?;
    }
    // Refuse unexpected files/reparse points before the downloader opens any path.
    cache_inventory(&directory, &plan)?;
    acquisition::fetch_with_cancel(
        &directory.join("plan/download-plan.json"),
        &directory.join("cache"),
        cancel,
    )?;
    unchanged_case(manifest, &hash)?;
    if entry.format == "adsblol_archive" {
        return Ok(entry);
    }
    entry.files.extend(cache_inventory(&directory, &plan)?);
    entry.format = "adsblol_archive".into();
    entry.primary_file = "cache/archive-manifest.json".into();
    write_new(&directory.join("completed.json"), &entry)?;
    Ok(entry)
}
pub fn adsblol_progress(manifest: &Path, id: &str) -> Result<DownloadProgress, String> {
    let (directory, hash) = item_dir(manifest, id)?;
    let (entry, plan) = plan_entry(&directory, &hash)?;
    let cache = directory.join("cache");
    let mut downloaded = 0u64;
    if cache.exists() {
        regular(&cache, true)?;
        for asset in &plan.assets {
            for suffix in ["", ".part"] {
                let p = cache.join(format!("{}{suffix}", asset.name));
                if p.exists() {
                    regular(&p, false)?;
                    downloaded = downloaded.saturating_add(
                        fs::metadata(p)
                            .map_err(|e| e.to_string())?
                            .len()
                            .min(asset.bytes),
                    );
                    break;
                }
            }
        }
    }
    Ok(DownloadProgress{planned_bytes:plan.planned_bytes,downloaded_bytes:downloaded.min(plan.planned_bytes),complete:entry.format=="adsblol_archive" && downloaded==plan.planned_bytes && !directory.join("deletion.json").exists(),notes:vec!["Byte counts are progress, not a checksum verdict. Full SHA-256 verification completes before registration.".into(),entry.notes]})
}
pub fn delete_owned(manifest: &Path, id: &str) -> Result<DeleteReceipt, String> {
    let (directory, hash) = item_dir(manifest, id)?;
    let _lock = lock(&directory)?;
    let entry = entry(&directory, &hash)?;
    let _history_lock = if matches!(
        entry.format.as_str(),
        "adsblol_history_plan" | "adsblol_history"
    ) && directory.join("cache").exists()
    {
        Some(history::try_lock_cache(&directory.join("cache"))?)
    } else {
        None
    };
    if directory.join("deleted.json").exists() {
        return read(&directory.join("deleted.json"));
    }
    let deletion = directory.join("deletion.json");
    let files: Vec<CatalogFile> = if deletion.exists() {
        read(&deletion)?
    } else {
        for file in &entry.files {
            checked(&directory, file)?;
        }
        let mut files = entry.files.clone();
        if entry.format == "adsblol_plan" {
            let (_, plan) = plan_entry(&directory, &hash)?;
            files.extend(cache_inventory(&directory, &plan)?);
        }
        if entry.format == "adsblol_history_plan" || entry.format == "adsblol_history" {
            let registered: BTreeSet<_> = files.iter().map(|f| f.relative_path.clone()).collect();
            files.extend(
                history_inventory(&directory)?
                    .into_iter()
                    .filter(|f| !registered.contains(&f.relative_path)),
            );
        }
        if matches!(
            entry.format.as_str(),
            "fmi_lightning_plan" | "fmi_lightning_download"
        ) {
            let registered: BTreeSet<_> = files.iter().map(|f| f.relative_path.clone()).collect();
            files.extend(
                fmi::recover_inventory(&directory, &hash)?
                    .into_iter()
                    .filter(|f| !registered.contains(&f.relative_path)),
            );
        }
        write_new(&deletion, &files)?;
        files
    };
    // A prior deletion may have stopped between files. Remaining paths are still
    // checked against the immutable pre-deletion inventory, never recursively removed.
    for file in &files {
        let p = child(&directory, &file.relative_path, false)?;
        if p.exists() {
            checked(&directory, file)?;
        }
    }
    let mut bytes = 0;
    unchanged_case(manifest, &hash)?;
    for file in &files {
        let p = child(&directory, &file.relative_path, false)?;
        if p.exists() {
            remove_registered(&directory, file)?;
        }
        bytes += file.bytes;
    }
    let receipt = DeleteReceipt {
        id: id.into(),
        deleted_files: files.len(),
        deleted_bytes: bytes,
        originals_preserved: true,
    };
    write_new(&directory.join("deleted.json"), &receipt)?;
    Ok(receipt)
}

/// Explicit user action only. URLs are passed directly to curl, without a shell,
/// cookies or curl configuration. Query strings are hashed, not saved in receipts.
pub fn download_url_with_cancel(
    manifest: &Path,
    url: &str,
    request: &ImportRequest,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, String> {
    download_url_as_with_cancel(manifest, url, request, cancel, None)
}

fn download_url_as_with_cancel(
    manifest: &Path,
    url: &str,
    request: &ImportRequest,
    cancel: &AtomicBool,
    extension_override: Option<&str>,
) -> Result<CatalogEntry, String> {
    validate_bounds(request.bounds)?;
    if request.kind == DataKind::Adsb {
        return Err("Use the official ADSB.lol acquisition planner for flight archives".into());
    }
    if request.label.trim().is_empty()
        || request.label.len() > 240
        || request.notes.len() > 20_000
        || request.max_bytes == 0
        || request.max_bytes > 1_000_000_000_000
    {
        return Err("Use a label and an explicit download limit of at most 1 TB".into());
    }
    let rest = url
        .strip_prefix("https://")
        .ok_or("Dataset download requires an HTTPS URL")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty()
        || authority.contains('@')
        || url.len() > 8192
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(
            "Use an HTTPS export URL without embedded user/password credentials or whitespace"
                .into(),
        );
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Download canceled before the request".into());
    }
    let public_url = url.split(['?', '#']).next().unwrap_or("https://");
    let url_hash = format!("{:x}", Sha256::digest(url.as_bytes()));
    let (directory, id, hash) = new_item(manifest)?;
    let _lock = lock(&directory)?;
    let temporary = directory.join("payload.part");
    let curl = if cfg!(windows) {
        PathBuf::from(std::env::var_os("SystemRoot").ok_or("Windows system directory unavailable")?)
            .join("System32/curl.exe")
    } else {
        PathBuf::from("curl")
    };
    let mut command = Command::new(curl);
    command
        .args([
            "-q",
            "--fail",
            "--location",
            "--silent",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "30",
            "--max-time",
            "1800",
            "--speed-limit",
            "1",
            "--speed-time",
            "60",
            "--max-filesize",
            &request.max_bytes.to_string(),
            "--output",
        ])
        .arg(&temporary)
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start the system HTTPS downloader")?;
    let failure = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            break Some("Dataset download canceled");
        }
        if temporary
            .metadata()
            .is_ok_and(|m| m.len() > request.max_bytes)
        {
            let _ = child.kill();
            let _ = child.wait();
            break Some("Dataset download exceeded its size limit");
        }
        match child.try_wait() {
            Ok(Some(status)) => break (!status.success()).then_some(
                "Dataset download failed; check the export URL, access permission and size limit",
            ),
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break Some("Could not observe the dataset download");
            }
        }
    };
    if !temporary.exists() {
        return Err(failure.unwrap_or("Download produced no data").into());
    }
    regular(&temporary, false)?;
    let failure = if temporary.metadata().map_err(|e| e.to_string())?.len() > request.max_bytes {
        Some("Dataset download exceeded its size limit")
    } else {
        failure
    };
    let extension = extension_override
        .or_else(|| {
            Path::new(public_url)
                .extension()
                .and_then(|s| s.to_str())
                .filter(|s| s.len() <= 12 && s.bytes().all(|b| b.is_ascii_alphanumeric()))
        })
        .unwrap_or("bin");
    let filename = if failure.is_none() {
        format!("payload.{extension}")
    } else {
        "payload.part".into()
    };
    if failure.is_none() {
        fs::rename(&temporary, directory.join(&filename)).map_err(|e| e.to_string())?;
    }
    let value = CatalogEntry {
        schema_version: 1,
        id,
        case_manifest_sha256: hash,
        label: request.label.clone(),
        kind: request.kind,
        format: if failure.is_none() {
            "local_file"
        } else {
            "incomplete_download"
        }
        .into(),
        primary_file: filename.clone().into(),
        created_unix_s: now()?.as_secs(),
        source: json!({"type":"authorized_https_export","url_without_query":public_url,"request_url_sha256":url_hash,"query_parameters_redacted":url.contains('?'),"download_error":failure,"provider_coverage_verified":false}),
        files: vec![identity(&directory, &directory.join(filename))?],
        date_utc: request.date_utc.clone(),
        bounds: request.bounds,
        notes: request.notes.clone(),
    };
    unchanged_case(manifest, &value.case_manifest_sha256)?;
    write_new(&directory.join("entry.json"), &value)?;
    if let Some(error) = failure {
        return Err(format!(
            "{error}; incomplete managed copy {} can be deleted in Data",
            value.id
        ));
    }
    Ok(value)
}

/// Convert one verified daily archive to actual tracks within an explicitly
/// supplied UTC window and region. Altitude treatment remains in the config.
pub fn import_adsblol_archive(
    manifest: &Path,
    id: &str,
    config: &TraceImportConfig,
) -> Result<CatalogEntry, String> {
    import_adsblol_archive_with_cancel(manifest, id, config, &AtomicBool::new(false))
}
pub fn import_adsblol_archive_with_cancel(
    manifest: &Path,
    id: &str,
    config: &TraceImportConfig,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, String> {
    import_adsblol_archive_inner(manifest, id, config, None, cancel)
}

/// Stream a complete day archive into exact, indexed disk-backed tracks.
/// The declared disk budget replaces the legacy whole-result point budget.
pub fn import_adsblol_indexed_with_cancel(
    manifest: &Path,
    id: &str,
    config: &TraceImportConfig,
    max_disk_bytes: u64,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, String> {
    import_adsblol_indexed_with_limits(
        manifest,
        id,
        config,
        max_disk_bytes,
        crate::track_store::DEFAULT_INDEX_MEMORY_BYTES,
        cancel,
    )
}

pub fn import_adsblol_indexed_with_limits(
    manifest: &Path,
    id: &str,
    config: &TraceImportConfig,
    max_disk_bytes: u64,
    max_index_memory_bytes: u64,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, String> {
    if !(64 * 1024 * 1024..=1024_u64.pow(4)).contains(&max_disk_bytes) {
        return Err("Indexed import needs a declared disk budget from 64 MiB to 1 TiB".into());
    }
    crate::track_store::validate_index_memory_limit(max_index_memory_bytes)?;
    import_adsblol_archive_inner(
        manifest,
        id,
        config,
        Some((max_disk_bytes, max_index_memory_bytes)),
        cancel,
    )
}

fn import_adsblol_archive_inner(
    manifest: &Path,
    id: &str,
    config: &TraceImportConfig,
    indexed_budgets: Option<(u64, u64)>,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, String> {
    adsblol::validate_config(config)?;
    validate_bounds(config.bounds)?;
    if config.bounds.is_none() {
        return Err("Declare a bounded map region before importing a daily archive".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Archive import canceled before reading the source".into());
    }
    let (source_dir, case_hash) = item_dir(manifest, id)?;
    let _source_lock = lock(&source_dir)?;
    if source_dir.join("deletion.json").exists() {
        return Err("Cannot import a deleted archive".into());
    }
    let source = entry(&source_dir, &case_hash)?;
    if source.format != "adsblol_archive" {
        return Err("Select a complete downloaded ADSB.lol archive".into());
    }
    for file in &source.files {
        if cancel.load(Ordering::Relaxed) {
            return Err("Archive import canceled during verification".into());
        }
        checked_with_checkpoint(&source_dir, file, &mut || catalog_checkpoint(cancel))?;
    }
    let acquisition_path = child(&source_dir, &source.primary_file, true)?;
    let acquisition: acquisition::ArchiveManifest = read(&acquisition_path)?;
    let midnight = detailed_data::utc_midnight(&acquisition.date_utc)?;
    if config.time_origin_utc_s != midnight
        || config.time_window_utc_s[0] < midnight
        || config.time_window_utc_s[1] > midnight + 86_400.0
    {
        return Err(
            "Import origin must be archive UTC midnight; the time window must stay inside that day"
                .into(),
        );
    }
    // The manifest may only refer to the exact owned files already registered.
    for part in &acquisition.parts {
        let path = acquisition_path
            .parent()
            .ok_or("Archive manifest has no directory")?
            .join(&part.path)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let relative = path
            .strip_prefix(&source_dir)
            .map_err(|_| "Archive part is outside the managed item")?;
        child(&source_dir, relative, true)?;
        if !source.files.iter().any(|f| {
            f.relative_path == relative && f.bytes == part.bytes && f.sha256 == part.sha256
        }) {
            return Err("Archive part is not registered with its declared identity".into());
        }
    }
    let (directory, new_id, new_case_hash) = new_item(manifest)?;
    if new_case_hash != case_hash {
        return Err("Case changed while preparing import".into());
    }
    let _lock = lock(&directory)?;
    let config_path = directory.join("config.json");
    write_new(&config_path, config)?;
    let request_path = directory.join("request.json");
    let result = detailed_data::prepare_import(&acquisition_path, &config_path, &request_path)
        .and_then(|()| match indexed_budgets {
            Some((limit, memory)) => detailed_data::import_archive_indexed_with_limits(
                &request_path,
                &directory.join("native"),
                limit,
                memory,
                cancel,
            ),
            None => detailed_data::import_archive_with_cancel(
                &request_path,
                &directory.join("native"),
                cancel,
            ),
        });
    let mut files = vec![identity(&directory, &config_path)?];
    if request_path.exists() {
        files.push(identity(&directory, &request_path)?);
    }
    let native = directory.join("native");
    if native.exists() {
        let mut pending = vec![native];
        while let Some(folder) = pending.pop() {
            regular(&folder, true)?;
            for child_item in fs::read_dir(&folder).map_err(|e| e.to_string())? {
                let path = child_item.map_err(|e| e.to_string())?.path();
                let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
                if metadata.is_dir() {
                    regular(&path, true)?;
                    pending.push(path);
                } else {
                    files.push(identity(&directory, &path)?);
                }
                if files.len() + pending.len() > MAX_CATALOG_FILES {
                    return Err("Imported archive exceeds the catalog file budget".into());
                }
            }
        }
    }
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let format = if result.is_err() {
        "incomplete_import"
    } else if indexed_budgets.is_some() {
        "indexed_tracks"
    } else {
        "normalized_tracks"
    };
    let primary = if result.is_err() {
        "config.json"
    } else if indexed_budgets.is_some() {
        "native/index.json"
    } else {
        "native/normalized.json"
    };
    let mut source_metadata = json!({"type":if indexed_budgets.is_some(){"native_adsblol_indexed_archive_import"}else{"native_adsblol_archive_import"},"source_catalog_id":id,"acquisition_manifest_sha256":sha256(&acquisition_path)?,"config":config,"import_error":result.as_ref().err(),"location_verified":false});
    if let Some((limit, memory)) = indexed_budgets {
        source_metadata["indexed_disk_budget_bytes"] = json!(limit);
        source_metadata["index_memory_budget_bytes"] = json!(memory);
    }
    let notes = if indexed_budgets.is_some() {
        "Exact indexed tracks retained on disk for wide-area search; the complete point set is not loaded as a map preview. Explicit altitude policy and original archive preserved; aircraft coverage remains incomplete."
    } else {
        "Native region/time selection from one provider archive. Explicit altitude policy retained; missing aircraft are not inferred absent. Original archive preserved."
    };
    let value = CatalogEntry {
        schema_version: 1,
        id: new_id,
        case_manifest_sha256: case_hash,
        label: format!(
            "{} tracks {}",
            if indexed_budgets.is_some() {
                "Indexed"
            } else {
                "Regional"
            },
            acquisition.date_utc
        ),
        kind: DataKind::Adsb,
        format: format.into(),
        primary_file: primary.into(),
        created_unix_s: now()?.as_secs(),
        source: source_metadata,
        files,
        date_utc: Some(acquisition.date_utc),
        bounds: config.bounds,
        notes: notes.into(),
    };
    unchanged_case(manifest, &value.case_manifest_sha256)?;
    write_new(&directory.join("entry.json"), &value)?;
    if let Err(error) = result {
        return Err(format!(
            "{error}; incomplete managed import {} can be deleted in Data",
            value.id
        ));
    }
    Ok(value)
}

/// Register an explicit small regional request. Planning performs no network I/O.
pub fn plan_history(
    manifest: &Path,
    request: &history::HistoryRequest,
) -> Result<HistoryItem, String> {
    let plan = history::plan(request)?;
    let (directory, id, case_hash) = new_item(manifest)?;
    let _lock = lock(&directory)?;
    let path = directory.join("history-plan.json");
    write_new(&path, &plan)?;
    let value=CatalogEntry{schema_version:1,id,case_manifest_sha256:case_hash,label:format!("Regional ADSB.lol {}",request.utc_start),kind:DataKind::Adsb,
        format:"adsblol_history_plan".into(),primary_file:"history-plan.json".into(),created_unix_s:now()?.as_secs(),
        source:json!({"provider":plan.provider,"license":plan.license,"request":request,"history_status":"planned","coverage_complete":false}),
        files:vec![identity(&directory,&path)?],date_utc:Some(plan.date_utc.clone()),bounds:Some(request.bounds),
        notes:"Sampled replay chooses aircraft candidates; raw full traces require a separate native import and analyst association review. No complete-sky coverage claim.".into()};
    unchanged_case(manifest, &value.case_manifest_sha256)?;
    write_new(&directory.join("entry.json"), &value)?;
    Ok(HistoryItem {
        entry: value,
        plan,
        report: None,
    })
}

fn history_item(directory: &Path, case_hash: &str) -> Result<HistoryItem, String> {
    if directory.join("deletion.json").exists() {
        return Err("This history cache has been deleted or is pending deletion".into());
    }
    let value = entry(directory, case_hash)?;
    if value.format != "adsblol_history_plan" && value.format != "adsblol_history" {
        return Err("Select a regional ADSB.lol history request".into());
    }
    let file = value
        .files
        .iter()
        .find(|f| f.relative_path == Path::new("history-plan.json"))
        .ok_or("Registered history plan missing")?;
    let plan: history::HistoryPlan = read(&checked(directory, file)?)?;
    history::validate_plan(&plan)?;
    let report = if value.format == "adsblol_history" {
        let file = value
            .files
            .iter()
            .find(|f| f.relative_path == value.primary_file)
            .ok_or("Registered history receipt missing")?;
        let report: history::HistoryReport = read(&checked(directory, file)?)?;
        if report.plan != plan
            || Path::new("cache").join(&report.report_path) != value.primary_file
            || report.coverage_complete
            || report.location_verified
        {
            return Err(
                "History report differs from its managed request or claims unsupported coverage"
                    .into(),
            );
        }
        Some(report)
    } else {
        None
    };
    Ok(HistoryItem {
        entry: value,
        plan,
        report,
    })
}

/// Reopen a planned, partial, cancelled or complete request after restart. Every
/// registered byte is reverified; list() remains the cheaper size-only overview.
pub fn read_history(manifest: &Path, id: &str) -> Result<HistoryItem, String> {
    let (directory, hash) = item_dir(manifest, id)?;
    let _lock = lock(&directory)?;
    let item = history_item(&directory, &hash)?;
    let _history_lock = if directory.join("cache").exists() {
        Some(history::try_lock_cache(&directory.join("cache"))?)
    } else {
        None
    };
    for file in &item.entry.files {
        checked(&directory, file)?;
    }
    Ok(item)
}

fn history_inventory(directory: &Path) -> Result<Vec<CatalogFile>, String> {
    let cache = directory.join("cache");
    if !cache.exists() {
        return Ok(Vec::new());
    }
    regular(&cache, true)?;
    fn numeric(s: &str) -> bool {
        s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit())
    }
    fn hex(s: &str) -> bool {
        s.len() == 6
            && s.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    }
    let mut pending = vec![cache];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        regular(&dir, true)?;
        for child_item in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let path = child_item.map_err(|e| e.to_string())?.path();
            let relative = path.strip_prefix(directory).map_err(|e| e.to_string())?;
            let names: Vec<_> = relative
                .components()
                .map(|c| c.as_os_str().to_str().ok_or("Invalid history cache path"))
                .collect::<Result<_, _>>()?;
            if names.as_slice() == ["cache", "download.lock"] {
                regular(&path, false)?;
                if fs::metadata(&path).map_err(|e| e.to_string())?.len() != 0 {
                    return Err("History OS lock control file must be empty".into());
                }
                // Retain the empty control file as metadata, like the catalog's
                // .busy lock. Deleting it would let another handle bypass a lock.
                continue;
            }
            let is_directory = match names.as_slice() {
                ["cache", "attempts" | "traces" | "reports"] => true,
                ["cache", "attempts", n] if numeric(n) => true,
                _ => false,
            };
            let is_file = match names.as_slice() {
                ["cache", "plan.json"] => true,
                [
                    "cache",
                    "attempts",
                    n,
                    "request.json" | "response.bin" | "response.status" | "receipt.json",
                ] if numeric(n) => true,
                ["cache", "traces", name] => name
                    .strip_prefix("trace_full_")
                    .and_then(|s| s.strip_suffix(".json"))
                    .is_some_and(hex),
                ["cache", "reports", name] => name
                    .strip_suffix(".json")
                    .map(|s| s.strip_suffix("-index").unwrap_or(s))
                    .is_some_and(numeric),
                _ => false,
            };
            if is_directory {
                regular(&path, true)?;
                pending.push(path);
            } else if is_file {
                files.push(identity(directory, &path)?);
            } else {
                return Err(format!(
                    "Unexpected history cache path {}; refusing to manage or delete it",
                    relative.display()
                ));
            }
            if files.len() + pending.len() > 10_000 {
                return Err("History cache exceeds its file budget".into());
            }
        }
    }
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}

fn register_history_result(
    manifest: &Path,
    directory: &Path,
    item: HistoryItem,
    result: &Result<history::HistoryReport, String>,
) -> Result<HistoryItem, String> {
    let mut value = item.entry;
    let plan_file = value
        .files
        .iter()
        .find(|f| f.relative_path == Path::new("history-plan.json"))
        .ok_or("Missing registered history plan")?
        .clone();
    value.files = vec![plan_file];
    value.files.extend(history_inventory(directory)?);
    value
        .files
        .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let report = result.as_ref().ok().cloned();
    value.primary_file = report
        .as_ref()
        .map(|r| Path::new("cache").join(&r.report_path))
        .unwrap_or_else(|| "history-plan.json".into());
    value.format = if report.is_some() {
        "adsblol_history"
    } else {
        "adsblol_history_plan"
    }
    .into();
    value.source = json!({"provider":item.plan.provider,"license":item.plan.license,"request":item.plan.request,"history_status":report.as_ref().map(|r|serde_json::to_value(r.status).unwrap()).unwrap_or(json!("error")),
        "last_error":result.as_ref().err(),"coverage_complete":false,"location_verified":false,
        "report_path":report.as_ref().map(|r|&r.report_path),"http_payload_bytes":report.as_ref().map(|r|r.http_payload_bytes),
        "non_icao_omissions":report.as_ref().map(|r|r.non_icao_omissions),"trace_cap_omissions":report.as_ref().map(|r|r.trace_cap_omissions)});
    value.notes = if let Some(r) = &report {
        format!(
            "Regional request {:?}: {} downloaded of {} selected traces; {} non-ICAO omissions and {} trace-cap omissions; replay timing gaps={}. Aircraft coverage remains incomplete. Raw sources and earlier attempts retained.",
            r.status,
            r.traces.iter().filter(|t| t.decoded.is_some()).count(),
            r.traces.len(),
            r.non_icao_omissions,
            r.trace_cap_omissions,
            r.replay_timing_gaps
        )
    } else {
        format!(
            "Regional download stopped: {}. Bounded partial cache retained for inspection/resume; no successful acquisition claim.",
            result.as_ref().err().unwrap()
        )
    };
    unchanged_case(manifest, &value.case_manifest_sha256)?;
    let mut revision = 1;
    while directory
        .join(format!("history-registration-{revision:08}.json"))
        .exists()
    {
        revision += 1;
        if revision > 1024 {
            return Err("History registration revision limit reached".into());
        }
    }
    write_new(
        &directory.join(format!("history-registration-{revision:08}.json")),
        &value,
    )?;
    Ok(HistoryItem {
        entry: value,
        plan: item.plan,
        report,
    })
}

pub fn fetch_history<F>(
    manifest: &Path,
    id: &str,
    cancel: &AtomicBool,
    progress: F,
) -> Result<HistoryItem, String>
where
    F: FnMut(history::HistoryProgress),
{
    let (directory, hash) = item_dir(manifest, id)?;
    let _lock = lock(&directory)?;
    let item = history_item(&directory, &hash)?;
    let _history_lock = history::try_lock_cache(&directory.join("cache"))?;
    for file in &item.entry.files {
        checked(&directory, file)?;
    }
    history_inventory(&directory)?;
    let result =
        history::fetch_in_locked_cache(&item.plan, &directory.join("cache"), cancel, progress);
    let saved = register_history_result(manifest, &directory, item, &result)?;
    result.map(|_| saved)
}

/// Same managed operation with an injectable transport for offline controls.
pub fn fetch_history_with_transport<F, T>(
    manifest: &Path,
    id: &str,
    cancel: &AtomicBool,
    progress: F,
    transport: T,
) -> Result<HistoryItem, String>
where
    F: FnMut(history::HistoryProgress),
    T: FnMut(&str, u64, &Path, &AtomicBool) -> Result<history::TransportResult, String>,
{
    let (directory, hash) = item_dir(manifest, id)?;
    let _lock = lock(&directory)?;
    let item = history_item(&directory, &hash)?;
    let _history_lock = history::try_lock_cache(&directory.join("cache"))?;
    for file in &item.entry.files {
        checked(&directory, file)?;
    }
    history_inventory(&directory)?;
    let result = history::fetch_with_transport_in_locked_cache(
        &item.plan,
        &directory.join("cache"),
        cancel,
        progress,
        transport,
    );
    let saved = register_history_result(manifest, &directory, item, &result)?;
    result.map(|_| saved)
}

/// Separate explicit import; a partial source requires an affirmative flag.
/// Neither import nor acquisition edits reviewed observations or associations.
pub fn import_history(
    manifest: &Path,
    id: &str,
    config: &TraceImportConfig,
    allow_partial: bool,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, String> {
    adsblol::validate_config(config)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("History import cancelled before source verification".into());
    }
    let (source_dir, hash) = item_dir(manifest, id)?;
    let _source_lock = lock(&source_dir)?;
    let item = history_item(&source_dir, &hash)?;
    let _history_lock = history::try_lock_cache(&source_dir.join("cache"))?;
    let report = item
        .report
        .as_ref()
        .ok_or("Fetch the selected history plan before importing tracks")?;
    for file in &item.entry.files {
        if cancel.load(Ordering::Relaxed) {
            return Err("History import cancelled during verification".into());
        }
        checked(&source_dir, file)?;
    }
    let normalized = history::import_traces_with_cancel(
        &source_dir.join("cache"),
        report,
        config,
        allow_partial,
        cancel,
    )?;
    let (directory, new_id, new_hash) = new_item(manifest)?;
    if new_hash != hash {
        return Err("Case changed during history import".into());
    }
    let _lock = lock(&directory)?;
    write_new(&directory.join("config.json"), config)?;
    write_new(&directory.join("normalized.json"), &normalized)?;
    let report_path = child(&source_dir, &item.entry.primary_file, true)?;
    let value = CatalogEntry {
        schema_version: 1,
        id: new_id,
        case_manifest_sha256: hash,
        label: format!("Regional history tracks {}", item.plan.date_utc),
        kind: DataKind::Adsb,
        format: "normalized_tracks".into(),
        primary_file: "normalized.json".into(),
        created_unix_s: now()?.as_secs(),
        source: json!({"type":"native_adsblol_history_import","source_catalog_id":id,"acquisition_report_sha256":sha256(&report_path)?,"config":config,"allow_partial":allow_partial,"history_status":report.status,"coverage_complete":false,"location_verified":false}),
        files: vec![
            identity(&directory, &directory.join("config.json"))?,
            identity(&directory, &directory.join("normalized.json"))?,
        ],
        date_utc: Some(item.plan.date_utc),
        bounds: config.bounds.or(Some(item.plan.request.bounds)),
        notes: format!(
            "Native full-trace import from a {:?} regional replay acquisition. Source/altitude policies and omission warnings retained; aircraft association remains unreviewed.",
            report.status
        ),
    };
    unchanged_case(manifest, &value.case_manifest_sha256)?;
    write_new(&directory.join("entry.json"), &value)?;
    Ok(value)
}
