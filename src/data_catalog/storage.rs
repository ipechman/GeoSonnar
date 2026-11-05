//! Owned paths, catalog records, publication, identity checks and deletion primitives.
use super::model::{CatalogEntry, CatalogFile};
use crate::{
    casework,
    detailed_data::{self, sha256},
};
use serde::Serialize;
use serde_json::{Value, json};
#[cfg(windows)]
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);
pub(super) const MAX_CATALOG_FILES: usize = 250_010;
const MAX_METADATA_BYTES: u64 = 128 * 1024 * 1024;

fn nonce() -> Result<String, String> {
    Ok(format!(
        "{}-{}-{}",
        now()?.as_nanos(),
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

pub(super) fn now() -> Result<std::time::Duration, String> {
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

pub(super) fn regular(path: &Path, directory: bool) -> Result<(), String> {
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

pub(super) fn child(root: &Path, relative: &Path, exists: bool) -> Result<PathBuf, String> {
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

pub(super) fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
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

pub(super) fn write_new(path: &Path, value: &impl Serialize) -> Result<(), String> {
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

pub(super) fn base(manifest: &Path, create: bool) -> Result<Option<(PathBuf, String)>, String> {
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

pub(super) fn item_dir(manifest: &Path, id: &str) -> Result<(PathBuf, String), String> {
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

pub(super) fn lock(directory: &Path) -> Result<File, String> {
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

pub(super) fn entry(directory: &Path, case_hash: &str) -> Result<CatalogEntry, String> {
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

pub(super) fn identity(directory: &Path, path: &Path) -> Result<CatalogFile, String> {
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

pub(super) fn checked(directory: &Path, file: &CatalogFile) -> Result<PathBuf, String> {
    checked_with_checkpoint(directory, file, &mut || Ok(()))
}

pub(super) fn checked_with_checkpoint(
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
pub(super) fn remove_registered(directory: &Path, entry: &CatalogFile) -> Result<(), String> {
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
pub(super) fn remove_registered(directory: &Path, entry: &CatalogFile) -> Result<(), String> {
    let path = checked(directory, entry)?;
    fs::remove_file(path).map_err(|e| e.to_string())
}

pub(super) fn validate_bounds(bounds: Option<[f64; 4]>) -> Result<(), String> {
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

pub(super) fn new_item(manifest: &Path) -> Result<(PathBuf, String, String), String> {
    let (root, hash) = base(manifest, true)?.unwrap();
    let id = format!("data-{}", nonce()?);
    let path = root.join(&id);
    fs::create_dir(&path).map_err(|e| e.to_string())?;
    Ok((path, id, hash))
}

pub(super) fn unchanged_case(manifest: &Path, expected: &str) -> Result<(), String> {
    if sha256(manifest)? != expected {
        Err("Case changed during its data operation".into())
    } else {
        Ok(())
    }
}

pub(super) fn catalog_checkpoint(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Catalog verification cancelled".into())
    } else {
        Ok(())
    }
}
