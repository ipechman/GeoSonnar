//! Case-owned ADSB day-archive planning, downloading and progress.
use super::{
    model::{AdsbPlan, CatalogEntry, CatalogFile, DataKind, DownloadProgress},
    storage::{
        checked, entry, identity, item_dir, lock, new_item, now, read, regular, unchanged_case,
        validate_bounds, write_new,
    },
};
use crate::adsblol_acquisition as acquisition;
use serde_json::json;
use std::{collections::BTreeSet, fs, path::Path, sync::atomic::AtomicBool};

pub(super) fn plan_entry(
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

pub(super) fn cache_inventory(
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
