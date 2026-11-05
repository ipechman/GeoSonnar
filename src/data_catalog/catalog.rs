//! Catalog listing, explicit verification and resumable owned-copy deletion.
use super::{
    adsb_downloads::{cache_inventory, plan_entry},
    fmi,
    history::history_inventory,
    model::{CatalogEntry, CatalogFile, CatalogItem, DataKind, DeleteReceipt},
    storage::{
        base, catalog_checkpoint, checked, checked_with_checkpoint, child, entry, item_dir, lock,
        read, regular, remove_registered, unchanged_case, write_new,
    },
};
use crate::adsblol_history as history;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

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
