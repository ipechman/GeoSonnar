//! Explicit local-copy and ADSB archive/index import coordination.
use super::{
    model::{CatalogEntry, DataKind, ImportRequest},
    storage::{
        MAX_CATALOG_FILES, catalog_checkpoint, checked_with_checkpoint, child, entry, identity,
        item_dir, lock, new_item, now, read, regular, unchanged_case, validate_bounds, write_new,
    },
};
use crate::{
    adsblol::{self, TraceImportConfig},
    adsblol_acquisition as acquisition,
    detailed_data::{self, sha256},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

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
        return Err("Use a label and a positive import limit of at most 931.3225746155 GiB".into());
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
        return Err(
            "Indexed import needs a declared storage allowance from 0.0625 GiB to 1024.000 GiB"
                .into(),
        );
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
