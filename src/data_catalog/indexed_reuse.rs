//! Metadata-only index discovery and explicit full-source verified reuse.
use super::{
    model::{CatalogEntry, CatalogFile, DataKind, IndexedReuseMatch, VerifiedIndexedReuse},
    storage::{
        catalog_checkpoint, checked_with_checkpoint, child, entry, item_dir, lock, read, regular,
        unchanged_case, validate_bounds,
    },
};
use crate::{
    adsblol::{self, TraceImportConfig},
    adsblol_acquisition as acquisition, detailed_data,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

fn reuse_hash(value: &impl Serialize) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|e| e.to_string())?)
    ))
}

fn reuse_index_header<T: serde::de::DeserializeOwned>(
    path: &Path,
    checkpoint: &mut impl FnMut() -> Result<(), String>,
) -> Result<T, String> {
    struct CheckedReader<'a, F> {
        input: std::io::Take<File>,
        checkpoint: &'a mut F,
        stopped: Option<String>,
    }
    impl<F: FnMut() -> Result<(), String>> Read for CheckedReader<'_, F> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            if let Err(error) = (self.checkpoint)() {
                self.stopped = Some(error.clone());
                return Err(std::io::Error::other(error));
            }
            self.input.read(bytes)
        }
    }
    checkpoint()?;
    regular(path, false)?;
    let file = File::open(path).map_err(|e| e.to_string())?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length > 64 * 1024 * 1024 {
        return Err("Native index metadata exceeds 0.0625 GiB".into());
    }
    // Stream ignored pass/page fields instead of allocating a second full index
    // image merely to extract its provenance. Check each bounded underlying read.
    let mut reader = std::io::BufReader::with_capacity(
        64 * 1024,
        CheckedReader {
            input: file.take(length + 1),
            checkpoint,
            stopped: None,
        },
    );
    let result = serde_json::from_reader(&mut reader).map_err(|e| e.to_string());
    if let Some(error) = reader.get_mut().stopped.take() {
        return Err(error);
    }
    (reader.get_mut().checkpoint)()?;
    if fs::metadata(path).map_err(|e| e.to_string())?.len() != length {
        return Err("Native index metadata changed length during reuse verification".into());
    }
    result
}

fn reuse_archive(
    directory: &Path,
    case_hash: &str,
    config: &TraceImportConfig,
    checkpoint: &mut impl FnMut() -> Result<(), String>,
) -> Result<(CatalogEntry, acquisition::ArchiveManifest, CatalogFile), String> {
    checkpoint()?;
    adsblol::validate_config(config)?;
    validate_bounds(config.bounds)?;
    if config.bounds.is_none() {
        return Err("Declare the exact bounded import region before finding an index".into());
    }
    if directory.join("deletion.json").exists() || directory.join("deleted.json").exists() {
        return Err("The selected archive is deleted or pending deletion".into());
    }
    let value = entry(directory, case_hash)?;
    if value.format != "adsblol_archive" || value.kind != DataKind::Adsb {
        return Err("Select a complete downloaded ADSB.lol archive".into());
    }
    let registered = value
        .files
        .iter()
        .find(|f| f.relative_path == value.primary_file)
        .ok_or("Archive manifest is not registered")?
        .clone();
    let path = checked_with_checkpoint(directory, &registered, checkpoint)?;
    let archive: acquisition::ArchiveManifest = read(&path)?;
    let midnight = detailed_data::utc_midnight(&archive.date_utc)?;
    if archive.schema_version != 1
        || !archive.complete
        || !archive.full_sha_verified
        || archive.parts.is_empty()
        || config.time_origin_utc_s != midnight
        || config.time_window_utc_s[0] < midnight
        || config.time_window_utc_s[1] > midnight + 86_400.
    {
        return Err(
            "Index reuse needs a complete verified archive and its exact UTC-day import selection"
                .into(),
        );
    }
    let mut seen = BTreeSet::new();
    for part in &archive.parts {
        checkpoint()?;
        let relative = value
            .primary_file
            .parent()
            .unwrap_or(Path::new(""))
            .join(&part.path);
        let actual = child(directory, &relative, true)?;
        if !seen.insert(relative.clone())
            || !value.files.iter().any(|file| {
                file.relative_path == relative
                    && file.bytes == part.bytes
                    && file.sha256 == part.sha256
            })
            || fs::metadata(actual).map_err(|e| e.to_string())?.len() != part.bytes
        {
            return Err("Archive part differs from its registered acquisition identity".into());
        }
    }
    checkpoint()?;
    Ok((value, archive, registered))
}

fn indexed_reuse_match(
    directory: &Path,
    value: &CatalogEntry,
    source_id: &str,
    acquisition_sha256: &str,
    config: &TraceImportConfig,
    checkpoint: &mut impl FnMut() -> Result<(), String>,
) -> Result<Option<IndexedReuseMatch>, String> {
    checkpoint()?;
    if value.format != "indexed_tracks"
        || value.kind != DataKind::Adsb
        || directory.join("deletion.json").exists()
        || directory.join("deleted.json").exists()
        || value.source["type"] != "native_adsblol_indexed_archive_import"
        || value.source["source_catalog_id"].as_str() != Some(source_id)
        || value.source["acquisition_manifest_sha256"].as_str() != Some(acquisition_sha256)
        || value.source.get("import_error") != Some(&Value::Null)
    {
        return Ok(None);
    }
    let stored_config: TraceImportConfig =
        serde_json::from_value(value.source["config"].clone())
            .map_err(|e| format!("Saved index has invalid import settings: {e}"))?;
    if stored_config != *config || reuse_hash(&stored_config)? != reuse_hash(config)? {
        return Ok(None);
    }
    let mut registered_bytes = 0_u64;
    for file in &value.files {
        checkpoint()?;
        let path = match child(directory, &file.relative_path, true) {
            Ok(path) => path,
            Err(_) => return Ok(None),
        };
        if fs::metadata(path).map_err(|e| e.to_string())?.len() != file.bytes {
            return Ok(None);
        }
        registered_bytes = registered_bytes
            .checked_add(file.bytes)
            .ok_or("Saved index size total overflow")?;
    }
    let primary = value
        .files
        .iter()
        .find(|f| f.relative_path == value.primary_file)
        .ok_or("Saved index primary file is not registered")?;
    let index = crate::track_store::TrackStoreBinding {
        manifest_path: child(directory, &primary.relative_path, true)?,
        manifest_sha256: primary.sha256.clone(),
    };
    Ok(Some(IndexedReuseMatch {
        catalog_id: value.id.clone(),
        label: value.label.clone(),
        index,
        source_catalog_id: source_id.into(),
        acquisition_manifest_sha256: acquisition_sha256.into(),
        config_sha256: reuse_hash(config)?,
        catalog_entry_sha256: reuse_hash(value)?,
        registered_bytes,
    }))
}

/// Lightweight discovery only: checks registered identities, metadata and file
/// presence/lengths. It does NOT hash the archive parts or open each native index.
/// Same-size corruption can therefore be detected only by explicit verification.
pub fn find_completed_indexed_with_cancel(
    manifest: &Path,
    source_catalog_id: &str,
    config: &TraceImportConfig,
    cancel: &AtomicBool,
) -> Result<Vec<IndexedReuseMatch>, String> {
    find_completed_indexed_with_checkpoint(manifest, source_catalog_id, config, || {
        catalog_checkpoint(cancel)
    })
}

pub fn find_completed_indexed_with_checkpoint(
    manifest: &Path,
    source_catalog_id: &str,
    config: &TraceImportConfig,
    mut checkpoint: impl FnMut() -> Result<(), String>,
) -> Result<Vec<IndexedReuseMatch>, String> {
    checkpoint()?;
    let (source_dir, case_hash) = item_dir(manifest, source_catalog_id)?;
    let _source_lock = lock(&source_dir)?;
    let (source, _, acquisition_file) =
        reuse_archive(&source_dir, &case_hash, config, &mut checkpoint)?;
    let source_identity = reuse_hash(&source)?;
    let root = source_dir.parent().ok_or("Archive has no catalog parent")?;
    let mut matches = Vec::new();
    let mut count = 0;
    for item in fs::read_dir(root).map_err(|e| e.to_string())? {
        checkpoint()?;
        let path = item.map_err(|e| e.to_string())?.path();
        if path
            .file_name()
            .and_then(|p| p.to_str())
            .is_none_or(|p| !p.starts_with("data-"))
        {
            continue;
        }
        count += 1;
        if count > 10_000 {
            return Err("Catalog exceeds 10,000 entries".into());
        }
        regular(&path, true)?;
        if !path.join("entry.json").exists() {
            continue;
        }
        let value = entry(&path, &case_hash)?;
        if let Some(candidate) = indexed_reuse_match(
            &path,
            &value,
            source_catalog_id,
            &acquisition_file.sha256,
            config,
            &mut checkpoint,
        )? {
            matches.push(candidate);
        }
    }
    matches.sort_by(|a, b| a.catalog_id.cmp(&b.catalog_id));
    checked_with_checkpoint(&source_dir, &acquisition_file, &mut checkpoint)?;
    if reuse_hash(&entry(&source_dir, &case_hash)?)? != source_identity {
        return Err("Archive catalog identity changed during index discovery".into());
    }
    unchanged_case(manifest, &case_hash)?;
    checkpoint()?;
    Ok(matches)
}

/// Explicit reuse verifies current raw archive bytes and every owned index
/// file, then opens the native source with its existing schema/memory checks.
/// max_disk_bytes limits ALREADY retained bytes; no new copy or free-space
/// reservation is required. Neither a mismatch nor a verification error builds
/// replacement data. Ordinary stored-index loading remains a separate operation.
pub fn verify_indexed_reuse_with_limits(
    manifest: &Path,
    source_catalog_id: &str,
    matched: &IndexedReuseMatch,
    config: &TraceImportConfig,
    max_disk_bytes: u64,
    max_index_memory_bytes: u64,
    cancel: &AtomicBool,
) -> Result<VerifiedIndexedReuse, String> {
    verify_indexed_reuse_with_checkpoint(
        manifest,
        source_catalog_id,
        matched,
        config,
        max_disk_bytes,
        max_index_memory_bytes,
        || catalog_checkpoint(cancel),
    )
}

pub fn verify_indexed_reuse_with_checkpoint(
    manifest: &Path,
    source_catalog_id: &str,
    matched: &IndexedReuseMatch,
    config: &TraceImportConfig,
    max_disk_bytes: u64,
    max_index_memory_bytes: u64,
    mut checkpoint: impl FnMut() -> Result<(), String>,
) -> Result<VerifiedIndexedReuse, String> {
    checkpoint()?;
    if !(64 * 1024 * 1024..=1024_u64.pow(4)).contains(&max_disk_bytes) {
        return Err(
            "Saved indexed-data storage allowance must be 0.0625 GiB..1024.000 GiB; reuse creates no new copy".into(),
        );
    }
    crate::track_store::validate_index_memory_limit(max_index_memory_bytes)?;
    let (source_dir, case_hash) = item_dir(manifest, source_catalog_id)?;
    let _source_lock = lock(&source_dir)?;
    let (source, archive, acquisition_file) =
        reuse_archive(&source_dir, &case_hash, config, &mut checkpoint)?;
    let source_identity = reuse_hash(&source)?;
    let (directory, index_case_hash) = item_dir(manifest, &matched.catalog_id)?;
    if index_case_hash != case_hash {
        return Err("Case changed before saved-index verification".into());
    }
    let _index_lock = lock(&directory)?;
    let value = entry(&directory, &case_hash)?;
    let current = indexed_reuse_match(
        &directory,
        &value,
        source_catalog_id,
        &acquisition_file.sha256,
        config,
        &mut checkpoint,
    )?
    .ok_or("Saved index no longer matches this archive and exact import selection")?;
    if current != *matched {
        return Err("Saved index identity changed; find matching indexes again".into());
    }
    if current.registered_bytes > max_disk_bytes {
        return Err("Existing saved index exceeds the selected retained-data size limit; no copy was attempted".into());
    }
    for file in &source.files {
        checked_with_checkpoint(&source_dir, file, &mut checkpoint)?;
    }
    for file in &value.files {
        checked_with_checkpoint(&directory, file, &mut checkpoint)?;
    }

    let metadata_path = |relative: &str| -> Result<PathBuf, String> {
        let relative = Path::new(relative);
        let registered = value
            .files
            .iter()
            .find(|f| f.relative_path == relative)
            .ok_or_else(|| format!("Saved import is missing registered {}", relative.display()))?;
        child(&directory, &registered.relative_path, true)
    };
    let saved_config: TraceImportConfig = read(&metadata_path("config.json")?)?;
    let request: detailed_data::ArchiveImportRequest = read(&metadata_path("request.json")?)?;
    let plan: Value = read(&metadata_path("native/plan.json")?)?;
    let receipt: Value = read(&metadata_path("native/receipt.json")?)?;
    if saved_config != *config
        || request.config != *config
        || reuse_hash(&saved_config)? != current.config_sha256
        || reuse_hash(&request.config)? != current.config_sha256
        || request.parts.len() != archive.parts.len()
        || request
            .parts
            .iter()
            .zip(&archive.parts)
            .any(|(actual, expected)| {
                actual.bytes != expected.bytes || actual.sha256 != expected.sha256
            })
        || plan["config"] != serde_json::to_value(config).map_err(|e| e.to_string())?
        || plan["acquisition_manifest"]["sha256"].as_str() != Some(acquisition_file.sha256.as_str())
        || plan["acquisition_manifest"]["bytes"].as_u64() != Some(acquisition_file.bytes)
        || receipt["status"] != "indexed_tracks_imported"
        || receipt["archive_import_complete"] != true
        || receipt["inputs_unchanged"] != true
        || receipt["location_verified"] != false
        || receipt["manifest"]["sha256"].as_str() != Some(current.index.manifest_sha256.as_str())
    {
        return Err("Saved indexed-import provenance differs from this acquisition, configuration or completed index".into());
    }
    // Both supported index versions carry the same source/config fields. Ignore
    // their large pass/page payload here; the native reader verifies it below.
    #[derive(Deserialize)]
    struct IndexSource {
        source: Value,
        config: TraceImportConfig,
    }
    let header: IndexSource = reuse_index_header(&current.index.manifest_path, &mut checkpoint)?;
    let registered_request = value
        .files
        .iter()
        .find(|f| f.relative_path == Path::new("request.json"))
        .ok_or("Saved request is not registered")?;
    let primary = value
        .files
        .iter()
        .find(|f| f.relative_path == value.primary_file)
        .ok_or("Missing index registration")?;
    if header.config != *config
        || reuse_hash(&header.config)? != current.config_sha256
        || header.source["acquisition_manifest"] != plan["acquisition_manifest"]
        || header.source["request"] != plan["request"]
        || header.source["parts"] != plan["parts"]
        || plan["parts"] != serde_json::to_value(&request.parts).map_err(|e| e.to_string())?
        || plan["request"]["sha256"].as_str() != Some(registered_request.sha256.as_str())
        || plan["request"]["bytes"].as_u64() != Some(registered_request.bytes)
        || receipt["manifest"]["bytes"].as_u64() != Some(primary.bytes)
    {
        return Err(
            "Native saved-index source binding differs from the registered completed import".into(),
        );
    }
    drop(header);
    drop(plan);
    checkpoint()?;
    let store = crate::track_store::TrackStore::open_with_memory_limit(
        &current.index.manifest_path,
        max_index_memory_bytes,
        &mut checkpoint,
    )?;
    if store.binding() != &current.index
        || store.import_config() != config
        || reuse_hash(store.import_config())? != current.config_sha256
        || receipt["pass_count"].as_u64() != Some(store.passes().len() as u64)
        || receipt["stats"]["output_points"].as_u64() != Some(store.total_points())
    {
        return Err("Verified native index differs from its saved completion receipt".into());
    }
    // Index verification can be lengthy. Match import's before/after raw-byte
    // checks rather than asserting current archive identity from metadata alone.
    for file in &source.files {
        checked_with_checkpoint(&source_dir, file, &mut checkpoint)?;
    }
    if reuse_hash(&entry(&source_dir, &case_hash)?)? != source_identity
        || reuse_hash(&entry(&directory, &case_hash)?)? != current.catalog_entry_sha256
        || source_dir.join("deletion.json").exists()
        || directory.join("deletion.json").exists()
    {
        return Err("Saved data ownership changed during explicit reuse verification".into());
    }
    checked_with_checkpoint(&source_dir, &acquisition_file, &mut checkpoint)?;
    unchanged_case(manifest, &case_hash)?;
    checkpoint()?;
    Ok(VerifiedIndexedReuse {
        matched: current,
        binding: store.binding().clone(),
        source_passes: store.passes().len() as u64,
        source_points: store.total_points(),
        time_origin_utc_s: store.time_origin_utc_s(),
        index_memory_bytes: store.resident_memory_bytes(),
        config: store.import_config().clone(),
    })
}
