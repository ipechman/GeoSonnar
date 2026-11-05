//! Case-owned regional ADSB replay acquisition and explicit trace import.
use super::{
    model::{CatalogEntry, CatalogFile, DataKind, HistoryItem},
    storage::{
        checked, child, entry, identity, item_dir, lock, new_item, now, read, regular,
        unchanged_case, write_new,
    },
};
use crate::{
    adsblol::{self, TraceImportConfig},
    adsblol_history as history,
    detailed_data::sha256,
};
use serde_json::json;
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

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

pub(super) fn history_inventory(directory: &Path) -> Result<Vec<CatalogFile>, String> {
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
