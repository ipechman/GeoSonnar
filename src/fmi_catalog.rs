//! Explicit case-owned FMI acquisition. Planning, fetching, import and loading
//! are separate actions; none changes reviewed acoustic evidence.
use super::*;
use crate::fmi_lightning::{self as provider, FmiChunkData, FmiPlan, FmiRequest};
pub use crate::http_download::TransportResult;
use std::{collections::BTreeMap, time::Instant};

const MAX_ATTEMPTS: usize = 240;
const MAX_SESSION_S: u64 = 300;
const MAX_STATUS_BYTES: u64 = 1024;
// The native writer retains exactly one over-limit byte as evidence, then stops.
const MAX_BODY_OVERSHOOT: u64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FmiStatus {
    Complete,
    Partial,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FmiReport {
    pub schema_version: u32,
    pub plan_sha256: String,
    pub status: FmiStatus,
    pub chunks_total: usize,
    pub chunks_complete: usize,
    pub raw_rows: usize,
    pub owned_rows: usize,
    pub transferred_bytes: u64,
    pub attempts: usize,
    pub errors: Vec<String>,
    pub source_coverage_complete: bool,
    pub location_verified: bool,
}

#[derive(Clone, Debug)]
pub struct FmiItem {
    pub entry: CatalogEntry,
    pub plan: FmiPlan,
    pub report: Option<FmiReport>,
}

#[derive(Clone, Debug)]
pub struct FmiProgress {
    pub chunks_total: usize,
    pub chunks_complete: usize,
    pub transferred_bytes: u64,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptRequest {
    sequence: usize,
    plan_sha256: String,
    chunk_index: usize,
    url: String,
    byte_limit: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptReceipt {
    request: AttemptRequest,
    body: CatalogFile,
    http_status: Option<u16>,
    error: Option<String>,
    finished_unix_s: u64,
}

fn attempt_name(prefix: &str, sequence: usize, extension: &str) -> String {
    format!("{prefix}-{sequence:06}.{extension}")
}
fn sequence(name: &str, prefix: &str, extension: &str) -> Option<usize> {
    let middle = name
        .strip_prefix(prefix)?
        .strip_prefix('-')?
        .strip_suffix(extension)?
        .strip_suffix('.')?;
    if middle.len() != 6 || !middle.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    middle
        .parse::<usize>()
        .ok()
        .filter(|v| (1..=MAX_ATTEMPTS).contains(v))
}

/// Only this provider's flat, bounded cache layout can be adopted into ownership.
fn inventory(directory: &Path) -> Result<Vec<CatalogFile>, String> {
    let mut files = Vec::new();
    for item in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        regular(&path, false)?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("Invalid FMI cache filename")?;
        if matches!(name, "entry.json" | ".busy" | "deletion.json")
            || name
                .strip_prefix("fmi-registration-")
                .and_then(|s| s.strip_suffix(".json"))
                .is_some_and(|s| s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let allowed = name == "fmi-plan.json"
            || sequence(name, "request", "json").is_some()
            || sequence(name, "attempt", "json").is_some()
            || sequence(name, "response", "xml").is_some()
            || sequence(name, "response", "status").is_some()
            || sequence(name, "report", "json").is_some();
        if !allowed {
            return Err(format!(
                "Unexpected file in FMI cache: {name}; it will not be adopted or deleted"
            ));
        }
        let size = fs::metadata(&path).map_err(|e| e.to_string())?.len();
        let limit = if name.ends_with(".xml") {
            8 * 1024 * 1024 + MAX_BODY_OVERSHOOT
        } else if name.ends_with(".status") {
            MAX_STATUS_BYTES
        } else {
            1024 * 1024
        };
        if size > limit {
            return Err(format!("FMI cache file exceeds its bounded size: {name}"));
        }
        files.push(identity(directory, &path)?);
        if files.len() > MAX_ATTEMPTS * 5 + 2 {
            return Err("FMI cache file limit exceeded".into());
        }
    }
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}

pub fn plan(manifest: &Path, request: &FmiRequest) -> Result<FmiItem, String> {
    let plan = provider::plan(request)?;
    let (directory, id, case_hash) = new_item(manifest)?;
    let _lock = lock(&directory)?;
    write_new(&directory.join("fmi-plan.json"), &plan)?;
    let value=CatalogEntry {
        schema_version:1,id,case_manifest_sha256:case_hash,
        label:format!("FMI lightning {}",request.start_utc),kind:DataKind::Lightning,
        format:"fmi_lightning_plan".into(),primary_file:"fmi-plan.json".into(),
        created_unix_s:now()?.as_secs(),source:json!({"provider":"FMI/NORDLIS","license":"CC BY 4.0","license_url":"https://en.ilmatieteenlaitos.fi/open-data-licence","request":request,"network_requested":false,"source_coverage_complete":false}),
        files:vec![identity(&directory,&directory.join("fmi-plan.json"))?],
        date_utc:Some(request.start_utc[..10].into()),bounds:Some(request.bounds),
        notes:"Planned public Nordic lightning request. No network activity yet. Whole-second source times; ETRS89 coordinates; unknown source height and unresolved location-error units. Download, import, load and source interpretation are separate actions.".into(),
    };
    unchanged_case(manifest, &value.case_manifest_sha256)?;
    write_new(&directory.join("entry.json"), &value)?;
    Ok(FmiItem {
        entry: value,
        plan,
        report: None,
    })
}

fn item(directory: &Path, case_hash: &str) -> Result<FmiItem, String> {
    if directory.join("deletion.json").exists() {
        return Err("This FMI request has been deleted or is pending deletion".into());
    }
    let value = entry(directory, case_hash)?;
    if !matches!(
        value.format.as_str(),
        "fmi_lightning_plan" | "fmi_lightning_download"
    ) {
        return Err("Select a stored FMI lightning request".into());
    }
    for file in &value.files {
        checked(directory, file)?;
    }
    let plan_file = value
        .files
        .iter()
        .find(|f| f.relative_path == Path::new("fmi-plan.json"))
        .ok_or("Missing FMI plan inventory")?;
    let plan: FmiPlan = read(&checked(directory, plan_file)?)?;
    provider::validate_plan(&plan)?;
    let report = if value.format == "fmi_lightning_download" {
        let report: FmiReport = read(&child(directory, &value.primary_file, true)?)?;
        if report.schema_version != 1
            || report.plan_sha256 != provider::plan_sha256(&plan)?
            || report.source_coverage_complete
            || report.location_verified
        {
            return Err("FMI report differs from its plan or claims unsupported coverage".into());
        }
        Some(report)
    } else {
        None
    };
    Ok(FmiItem {
        entry: value,
        plan,
        report,
    })
}

struct Scan {
    receipts: Vec<AttemptReceipt>,
    chunks: BTreeMap<usize, FmiChunkData>,
    unfinished: Vec<usize>,
}

// Bound expansion into retained native strike records across the entire
// request, including represented-time boundary rows. The response remains in
// the attempt cache when acceptance fails; it is never relabeled as complete.
fn accept_chunk(
    chunks: &mut BTreeMap<usize, FmiChunkData>,
    parsed: FmiChunkData,
) -> Result<(), String> {
    let total = chunks.values().try_fold(parsed.raw_rows, |total, chunk| {
        total.checked_add(chunk.raw_rows)
    });
    if total.is_none_or(|total| total > provider::MAX_STRIKES) {
        return Err(format!(
            "Cumulative FMI row limit of {} exceeded; narrow the request. Received response retained as a failed attempt",
            provider::MAX_STRIKES
        ));
    }
    if chunks.contains_key(&parsed.chunk_index) {
        return Err("FMI parsed chunk is already complete".into());
    }
    chunks.insert(parsed.chunk_index, parsed);
    Ok(())
}

fn scan(directory: &Path, plan: &FmiPlan) -> Result<Scan, String> {
    let files = inventory(directory)?;
    let mut requests = Vec::new();
    for file in &files {
        let name = file.relative_path.to_str().ok_or("Invalid FMI filename")?;
        if let Some(index) = sequence(name, "request", "json") {
            requests.push(index);
        }
    }
    requests.sort_unstable();
    if requests.iter().copied().ne(1..=requests.len()) {
        return Err("FMI attempt sequence has missing requests".into());
    }
    for file in &files {
        let name = file.relative_path.to_str().ok_or("Invalid FMI filename")?;
        for (prefix, extension) in [
            ("response", "xml"),
            ("response", "status"),
            ("attempt", "json"),
        ] {
            if let Some(index) = sequence(name, prefix, extension)
                && index > requests.len()
            {
                return Err("FMI cache has an orphan response or receipt".into());
            }
        }
    }
    let plan_sha = provider::plan_sha256(plan)?;
    let mut result = Scan {
        receipts: Vec::new(),
        chunks: BTreeMap::new(),
        unfinished: Vec::new(),
    };
    let mut charged_bytes = 0u64;
    for index in requests {
        let request: AttemptRequest =
            read(&directory.join(attempt_name("request", index, "json")))?;
        let chunk = plan
            .chunks
            .get(request.chunk_index)
            .ok_or("FMI request chunk index is outside its plan")?;
        if request.sequence != index
            || request.plan_sha256 != plan_sha
            || request.url != chunk.url
            || request.byte_limit == 0
            || request.byte_limit > plan.max_response_bytes
            || request.byte_limit > plan.request.max_total_bytes
        {
            return Err("FMI attempt request differs from the bounded plan".into());
        }
        let remaining = plan.request.max_total_bytes.saturating_sub(charged_bytes);
        if remaining == 0
            || request.byte_limit != plan.max_response_bytes.min(remaining)
            || result.chunks.contains_key(&request.chunk_index)
        {
            return Err("FMI cached attempt bypasses the cumulative budget or redownloads an already complete chunk".into());
        }
        let body_path = directory.join(attempt_name("response", index, "xml"));
        let receipt_path = directory.join(attempt_name("attempt", index, "json"));
        // A request can be durably written just before the empty body is created.
        // Read-only reopening keeps this unfinished state; resume creates the body.
        let body = if body_path.exists() {
            identity(directory, &body_path)?
        } else {
            CatalogFile {
                relative_path: PathBuf::from(attempt_name("response", index, "xml")),
                bytes: 0,
                sha256: format!("{:x}", Sha256::digest([])),
            }
        };
        let receipt = if receipt_path.exists() {
            let saved: AttemptReceipt = read(&receipt_path)?;
            if saved.request != request
                || saved.body != body
                || !body_path.exists()
                || (saved.error.is_none() && saved.http_status != Some(200))
            {
                return Err("FMI cached receipt or body identity changed".into());
            }
            saved
        } else {
            result.unfinished.push(index);
            AttemptReceipt {
                request,
                body,
                http_status: None,
                error: Some(
                    "Interrupted attempt has no final receipt; its bytes count toward the budget"
                        .into(),
                ),
                finished_unix_s: 0,
            }
        };
        if receipt.error.is_none() {
            if receipt.body.bytes > receipt.request.byte_limit {
                return Err("Successful FMI receipt exceeds its request byte limit".into());
            }
            let bytes = fs::read(&body_path).map_err(|e| e.to_string())?;
            let parsed = provider::parse_chunk(plan, receipt.request.chunk_index, &bytes)?;
            accept_chunk(&mut result.chunks, parsed)?;
        }
        charged_bytes = charged_bytes
            .checked_add(receipt.body.bytes)
            .ok_or("FMI byte inventory overflow")?;
        result.receipts.push(receipt);
    }
    Ok(result)
}

fn report(
    plan: &FmiPlan,
    scan: &Scan,
    cancelled: bool,
    extra: Vec<String>,
) -> Result<FmiReport, String> {
    let complete = scan.chunks.len() == plan.chunks.len();
    let transferred_bytes = scan.receipts.iter().try_fold(0u64, |total, r| {
        total
            .checked_add(r.body.bytes)
            .ok_or("FMI byte counter overflow")
    })?;
    let mut errors = scan
        .receipts
        .iter()
        .filter_map(|r| {
            r.error
                .as_ref()
                .map(|e| format!("Attempt {}: {}", r.request.sequence, e))
        })
        .collect::<Vec<_>>();
    errors.extend(extra);
    Ok(FmiReport {
        schema_version: 1,
        plan_sha256: provider::plan_sha256(plan)?,
        status: if complete {
            FmiStatus::Complete
        } else if cancelled {
            FmiStatus::Cancelled
        } else {
            FmiStatus::Partial
        },
        chunks_total: plan.chunks.len(),
        chunks_complete: scan.chunks.len(),
        raw_rows: scan.chunks.values().map(|c| c.raw_rows).sum(),
        owned_rows: scan.chunks.values().map(|c| c.strikes.len()).sum(),
        transferred_bytes,
        attempts: scan.receipts.len(),
        errors,
        source_coverage_complete: false,
        location_verified: false,
    })
}

/// Reports may survive an interruption immediately before registration. Only
/// reports whose counters agree with their immutable attempt prefix are owned.
fn validate_reports(directory: &Path, plan: &FmiPlan, state: &Scan) -> Result<(), String> {
    let plan_hash = provider::plan_sha256(plan)?;
    for file in inventory(directory)? {
        if sequence(
            file.relative_path.to_str().ok_or("Invalid FMI filename")?,
            "report",
            "json",
        )
        .is_none()
        {
            continue;
        }
        let saved: FmiReport = read(&checked(directory, &file)?)?;
        if saved.schema_version != 1
            || saved.plan_sha256 != plan_hash
            || saved.source_coverage_complete
            || saved.location_verified
            || saved.attempts > state.receipts.len()
            || saved.errors.len() > MAX_ATTEMPTS + 2
            || saved.errors.iter().any(|s| s.len() > 5000)
            || state.unfinished.iter().any(|i| *i <= saved.attempts)
        {
            return Err("FMI report differs from its bounded attempt history".into());
        }
        let prefix = &state.receipts[..saved.attempts];
        let successful: Vec<_> = prefix.iter().filter(|r| r.error.is_none()).collect();
        let expected_bytes: u64 = prefix.iter().map(|r| r.body.bytes).sum();
        let expected_raw: usize = successful
            .iter()
            .map(|r| state.chunks[&r.request.chunk_index].raw_rows)
            .sum();
        let expected_owned: usize = successful
            .iter()
            .map(|r| state.chunks[&r.request.chunk_index].strikes.len())
            .sum();
        let expected_errors: Vec<_> = prefix
            .iter()
            .filter_map(|r| {
                r.error
                    .as_ref()
                    .map(|e| format!("Attempt {}: {}", r.request.sequence, e))
            })
            .collect();
        let complete = successful.len() == plan.chunks.len();
        if saved.chunks_total != plan.chunks.len()
            || saved.chunks_complete != successful.len()
            || saved.raw_rows != expected_raw
            || saved.owned_rows != expected_owned
            || saved.transferred_bytes != expected_bytes
            || (saved.status == FmiStatus::Complete) != complete
            || !saved.errors.starts_with(&expected_errors)
        {
            return Err("FMI report counters differ from its verified response history".into());
        }
    }
    Ok(())
}

/// Called under the catalog lock before recording an owned deletion. This
/// includes validated crash remnants without adopting unrelated files.
pub(super) fn recover_inventory(
    directory: &Path,
    case_hash: &str,
) -> Result<Vec<CatalogFile>, String> {
    let item = item(directory, case_hash)?;
    let state = scan(directory, &item.plan)?;
    validate_reports(directory, &item.plan, &state)?;
    inventory(directory)
}

pub fn load(manifest: &Path, id: &str) -> Result<FmiItem, String> {
    let (directory, hash) = item_dir(manifest, id)?;
    let _lock = lock(&directory)?;
    let mut item = item(&directory, &hash)?;
    let scan = scan(&directory, &item.plan)?;
    validate_reports(&directory, &item.plan, &scan)?;
    if !scan.receipts.is_empty() {
        let saved_cancelled = item
            .report
            .as_ref()
            .is_some_and(|r| r.status == FmiStatus::Cancelled);
        item.report = Some(report(&item.plan, &scan, saved_cancelled, vec![])?);
    }
    Ok(item)
}

fn publish(
    manifest: &Path,
    directory: &Path,
    mut item: FmiItem,
    report: FmiReport,
) -> Result<FmiItem, String> {
    let mut revision = 1usize;
    while directory
        .join(format!("fmi-registration-{revision:08}.json"))
        .exists()
    {
        revision += 1;
        if revision > MAX_ATTEMPTS {
            return Err("FMI request revision limit reached; plan a new request".into());
        }
    }
    let mut report_index = 1usize;
    while directory
        .join(attempt_name("report", report_index, "json"))
        .exists()
    {
        report_index += 1;
        if report_index > MAX_ATTEMPTS {
            return Err("FMI report limit reached; plan a new request".into());
        }
    }
    let report_file = directory.join(attempt_name("report", report_index, "json"));
    write_new(&report_file, &report)?;
    item.entry.files = inventory(directory)?;
    item.entry.format = "fmi_lightning_download".into();
    item.entry.primary_file = report_file
        .strip_prefix(directory)
        .map_err(|e| e.to_string())?
        .to_path_buf();
    item.entry.source = json!({"provider":"FMI/NORDLIS","license":"CC BY 4.0","license_url":"https://en.ilmatieteenlaitos.fi/open-data-licence","request":item.plan.request,"plan_sha256":report.plan_sha256,"status":report.status,"source_coverage_complete":false,"location_verified":false});
    item.entry.notes = format!(
        "FMI {:?}: {}/{} query chunks, {} strikes, {} cumulative response bytes. Query completion does not prove complete lightning coverage. Import and explicit source interpretation remain separate; heights are unknown.",
        report.status,
        report.chunks_complete,
        report.chunks_total,
        report.owned_rows,
        report.transferred_bytes
    );
    unchanged_case(manifest, &item.entry.case_manifest_sha256)?;
    write_new(
        &directory.join(format!("fmi-registration-{revision:08}.json")),
        &item.entry,
    )?;
    item.report = Some(report);
    Ok(item)
}

pub fn fetch<F>(
    manifest: &Path,
    id: &str,
    cancel: &AtomicBool,
    progress: F,
) -> Result<FmiItem, String>
where
    F: FnMut(FmiProgress),
{
    fetch_with_transport(
        manifest,
        id,
        cancel,
        progress,
        crate::http_download::download,
    )
}

/// Injectable transport permits complete offline checks of the production state machine.
pub fn fetch_with_transport<F, T>(
    manifest: &Path,
    id: &str,
    cancel: &AtomicBool,
    mut progress: F,
    mut transport: T,
) -> Result<FmiItem, String>
where
    F: FnMut(FmiProgress),
    T: FnMut(&str, u64, &Path, &AtomicBool) -> Result<TransportResult, String>,
{
    let (directory, hash) = item_dir(manifest, id)?;
    let _lock = lock(&directory)?;
    let item = item(&directory, &hash)?;
    let mut state = scan(&directory, &item.plan)?;
    validate_reports(&directory, &item.plan, &state)?;
    for index in &state.unfinished {
        let receipt = &state.receipts[index - 1];
        let body = directory.join(&receipt.body.relative_path);
        if !body.exists() {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&body)
                .map_err(|e| e.to_string())?;
        }
        write_new(
            &directory.join(attempt_name("attempt", *index, "json")),
            receipt,
        )?;
    }
    let start = Instant::now();
    let mut extra = Vec::new();
    for chunk in &item.plan.chunks {
        if state.chunks.contains_key(&chunk.index) {
            continue;
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if start.elapsed().as_secs() >= MAX_SESSION_S {
            extra.push("Five-minute download session budget reached; resume explicitly".into());
            break;
        }
        if state.receipts.len() >= MAX_ATTEMPTS {
            extra.push("Attempt limit reached; plan a new bounded request".into());
            break;
        }
        if state.receipts.last().is_some_and(|r| {
            matches!(r.http_status, Some(429 | 503))
                && now().is_ok_and(|t| t.as_secs() < r.finished_unix_s.saturating_add(60))
        }) {
            extra.push("Provider throttled or unavailable; allow at least one minute before explicit resume".into());
            break;
        }
        let current = report(&item.plan, &state, false, vec![])?;
        let remaining = item
            .plan
            .request
            .max_total_bytes
            .saturating_sub(current.transferred_bytes);
        if remaining == 0 {
            extra.push(
                "Cumulative response-byte budget exhausted, including failed attempts".into(),
            );
            break;
        }
        let limit = item.plan.max_response_bytes.min(remaining);
        let sequence = state.receipts.len() + 1;
        let request = AttemptRequest {
            sequence,
            plan_sha256: provider::plan_sha256(&item.plan)?,
            chunk_index: chunk.index,
            url: chunk.url.clone(),
            byte_limit: limit,
        };
        write_new(
            &directory.join(attempt_name("request", sequence, "json")),
            &request,
        )?;
        let body = directory.join(attempt_name("response", sequence, "xml"));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&body)
            .map_err(|e| e.to_string())?;
        progress(FmiProgress {
            chunks_total: item.plan.chunks.len(),
            chunks_complete: state.chunks.len(),
            transferred_bytes: current.transferred_bytes,
            message: format!(
                "Requesting FMI chunk {} of {}",
                chunk.index + 1,
                item.plan.chunks.len()
            ),
        });
        let outcome =
            transport(&chunk.url, limit, &body, cancel).unwrap_or_else(|e| TransportResult {
                http_status: None,
                error: Some(e),
            });
        regular(&body, false)?;
        let byte_count = fs::metadata(&body).map_err(|e| e.to_string())?.len();
        if byte_count > item.plan.max_response_bytes + MAX_BODY_OVERSHOOT {
            return Err(
                "FMI transport wrote beyond its absolute body limit; cache retained for inspection"
                    .into(),
            );
        }
        let mut error = outcome.error;
        if error.is_none() && outcome.http_status != Some(200) {
            error = Some(format!(
                "Expected HTTP 200, received {:?}",
                outcome.http_status
            ));
        }
        if byte_count > limit {
            error = Some("Response exceeded its declared byte budget".into());
        }
        if error.is_none() {
            let bytes = fs::read(&body).map_err(|e| e.to_string())?;
            match provider::parse_chunk(&item.plan, chunk.index, &bytes) {
                Ok(parsed) => {
                    if let Err(reason) = accept_chunk(&mut state.chunks, parsed) {
                        error = Some(reason);
                    }
                }
                Err(reason) => error = Some(reason),
            }
        }
        let receipt = AttemptReceipt {
            request,
            body: identity(&directory, &body)?,
            http_status: outcome.http_status,
            error: error.map(|s| s.chars().take(1200).collect()),
            finished_unix_s: now()?.as_secs(),
        };
        write_new(
            &directory.join(attempt_name("attempt", sequence, "json")),
            &receipt,
        )?;
        let failed = receipt.error.is_some();
        state.receipts.push(receipt);
        let current = report(&item.plan, &state, cancel.load(Ordering::Relaxed), vec![])?;
        progress(FmiProgress {
            chunks_total: current.chunks_total,
            chunks_complete: current.chunks_complete,
            transferred_bytes: current.transferred_bytes,
            message: format!(
                "{} of {} FMI chunks complete",
                current.chunks_complete, current.chunks_total
            ),
        });
        // No automatic retries. A failed response stops this run and remains
        // visible; an analyst may resume after inspecting the error.
        if failed {
            break;
        }
        for _ in 0..5 {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    // Recheck raw bytes and the original inventory after callbacks and before
    // registering completion; no changed file is silently given a new identity.
    for file in &item.entry.files {
        checked(&directory, file)?;
    }
    drop(state);
    let state = scan(&directory, &item.plan)?;
    validate_reports(&directory, &item.plan, &state)?;
    let report = report(&item.plan, &state, cancel.load(Ordering::Relaxed), extra)?;
    publish(manifest, &directory, item, report)
}

pub fn import(manifest: &Path, id: &str, cancel: &AtomicBool) -> Result<CatalogEntry, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("FMI import cancelled".into());
    }
    let (source_dir, hash) = item_dir(manifest, id)?;
    let _source_lock = lock(&source_dir)?;
    let item = item(&source_dir, &hash)?;
    let scan = scan(&source_dir, &item.plan)?;
    validate_reports(&source_dir, &item.plan, &scan)?;
    if !scan.unfinished.is_empty() || scan.chunks.len() != item.plan.chunks.len() {
        return Err(
            "FMI import requires every query chunk to be complete; resume the stored request first"
                .into(),
        );
    }
    let chunks: Vec<_> = scan.chunks.into_values().collect();
    let document=provider::document(&item.plan,&chunks)?.ok_or("Complete FMI response contains no strikes in the requested interval; no lightning layer was created")?;
    crate::evidence_score::validate_lightning_document(&document)?;
    if serde_json::to_vec_pretty(&document)
        .map_err(|e| e.to_string())?
        .len()
        > 64 * 1024 * 1024
    {
        return Err("Normalized FMI strikes exceed 64 MiB; narrow the request".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("FMI import cancelled; original responses retained".into());
    }
    let (directory, new_id, new_hash) = new_item(manifest)?;
    if hash != new_hash {
        return Err("Case changed during FMI import".into());
    }
    let _lock = lock(&directory)?;
    write_new(&directory.join("lightning.json"), &document)?;
    write_new(
        &directory.join("source.json"),
        &json!({"source_catalog_id":id,"plan":item.plan,"source_files":inventory(&source_dir)?,"provider":"FMI/NORDLIS","license":"CC BY 4.0","source_coverage_complete":false}),
    )?;
    let value=CatalogEntry {schema_version:1,id:new_id,case_manifest_sha256:hash,
        label:format!("FMI · {} imported strikes",document.strikes.len()),kind:DataKind::Lightning,
        format:"fmi_lightning".into(),primary_file:"lightning.json".into(),created_unix_s:now()?.as_secs(),
        source:json!({"source_catalog_id":id,"plan_sha256":provider::plan_sha256(&item.plan)?,"provider":"FMI/NORDLIS","license":"CC BY 4.0","source_coverage_complete":false,"location_verified":false}),
        files:vec![identity(&directory,&directory.join("lightning.json"))?,identity(&directory,&directory.join("source.json"))?],
        date_utc:Some(item.plan.request.start_utc[..10].into()),bounds:Some(item.plan.request.bounds),
        notes:"Native FMI/NORDLIS import, CC BY 4.0. Original responses remain separate. Whole-second times and ETRS89 coordinates retained; an explicit source interpretation is required before thunder scoring. Strike heights unknown; ellipse error units unresolved and never used for scoring.".into()};
    unchanged_case(manifest, &value.case_manifest_sha256)?;
    write_new(&directory.join("entry.json"), &value)?;
    Ok(value)
}

#[cfg(test)]
#[path = "data_catalog/fmi_tests.rs"]
mod acceptance_controls;
