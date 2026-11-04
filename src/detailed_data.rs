//! Streaming detailed-source ingestion and terrain preparation for saved cases.
//! These stages do not choose acoustic events or infer a camera location.

use crate::{
    adsblol::{self, TraceImportConfig, TraceImportReport},
    terrain::{GeographicRaster, observer_grid},
    verification::{read_json, write_json},
};
use flate2::bufread::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, VecDeque},
    fs::{self, File},
    io::{BufReader, Cursor, Read},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_COMPRESSED_MEMBER: u64 = 32 * 1024 * 1024;
const MAX_DECODED_MEMBER: u64 = 128 * 1024 * 1024;
const MAX_TRACE_MEMBERS: usize = 250_000;
const MAX_SELECTED_POINTS: usize = 2_000_000;

pub fn sha256(path: &Path) -> Result<String, String> {
    sha256_with_checkpoint(path, || Ok(()))
}

/// Hash exact bytes while letting a caller stop between bounded reads.
pub fn sha256_with_checkpoint(
    path: &Path,
    mut checkpoint: impl FnMut() -> Result<(), String>,
) -> Result<String, String> {
    checkpoint()?;
    let mut reader = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        checkpoint()?;
        let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    checkpoint()?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn receipt(path: &Path) -> Result<Value, String> {
    Ok(
        json!({"path":path.canonicalize().map_err(|e|e.to_string())?,
        "bytes":fs::metadata(path).map_err(|e|e.to_string())?.len(),"sha256":sha256(path)?}),
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivePart {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveImportRequest {
    /// Exact ordered parts, relative to this request. They form one complete TAR.
    pub parts: Vec<ArchivePart>,
    /// A local acquisition manifest binding provider metadata and these parts.
    pub acquisition_manifest: PathBuf,
    pub config: TraceImportConfig,
}

/// Connect a verified cache manifest to a saved regional/time selection. The
/// generated request snapshots configuration values and uses absolute inputs.
pub fn prepare_import(
    manifest_path: &Path,
    config_path: &Path,
    output: &Path,
) -> Result<(), String> {
    if output.exists() {
        return Err("Use a new import request file".into());
    }
    let manifest_before = receipt(manifest_path)?;
    let config_before = receipt(config_path)?;
    let manifest: crate::adsblol_acquisition::ArchiveManifest = read_json(manifest_path)?;
    if !manifest.complete || !manifest.full_sha_verified {
        return Err("Import requires a complete SHA-verified acquisition manifest".into());
    }
    let config: TraceImportConfig = read_json(config_path)?;
    adsblol::validate_config(&config)?;
    let base = manifest_path.parent().unwrap_or(Path::new("."));
    let parts = manifest
        .parts
        .iter()
        .map(|p| {
            Ok(ArchivePart {
                path: base
                    .join(&p.path)
                    .canonicalize()
                    .map_err(|e| e.to_string())?,
                bytes: p.bytes,
                sha256: p.sha256.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let request = ArchiveImportRequest {
        parts,
        acquisition_manifest: manifest_path.canonicalize().map_err(|e| e.to_string())?,
        config,
    };
    if manifest_before != receipt(manifest_path)? || config_before != receipt(config_path)? {
        return Err("Import planning inputs changed".into());
    }
    write_json(output, &request)
}

pub struct PartReader {
    paths: VecDeque<PathBuf>,
    current: Option<BufReader<File>>,
}

impl PartReader {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths: paths.into(),
            current: None,
        }
    }
}

impl Read for PartReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if self.current.is_none() {
                let Some(path) = self.paths.pop_front() else {
                    return Ok(0);
                };
                self.current = Some(BufReader::new(File::open(path)?));
            }
            let count = self.current.as_mut().unwrap().read(buffer)?;
            if count > 0 {
                return Ok(count);
            }
            self.current = None;
        }
    }
}

pub fn trace_member_name(path: &Path) -> Result<Option<String>, String> {
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("Archive contains an unsafe member path".into());
    }
    let text = path.to_str().ok_or("Non-UTF8 archive member path")?;
    if text.contains('\\') || text.contains(':') {
        return Err("Unsupported archive path separators".into());
    }
    let pieces: Vec<_> = text.trim_start_matches("./").split('/').collect();
    if pieces.len() != 3 || pieces[0] != "traces" {
        return Ok(None);
    }
    let Some(hex) = pieces[2]
        .strip_prefix("trace_full_")
        .and_then(|s| s.strip_suffix(".json"))
    else {
        return Ok(None);
    };
    let digits = hex.strip_prefix('~').unwrap_or(hex);
    if digits.len() != 6
        || !digits.bytes().all(|b| b.is_ascii_hexdigit())
        || pieces[1] != &digits[4..]
    {
        return Err("Malformed historical trace member identity".into());
    }
    Ok(Some(hex.to_ascii_lowercase()))
}

pub fn decode_member(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() as u64 > MAX_COMPRESSED_MEMBER {
        return Err("Trace member exceeds compressed byte limit".into());
    }
    let mut output = Vec::new();
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = GzDecoder::new(Cursor::new(bytes));
        decoder
            .by_ref()
            .take(MAX_DECODED_MEMBER + 1)
            .read_to_end(&mut output)
            .map_err(|e| format!("Corrupt/incomplete trace gzip: {e}"))?;
        if decoder.into_inner().position() != bytes.len() as u64 {
            return Err("Trace gzip has trailing data or multiple members".into());
        }
    } else {
        output.extend_from_slice(bytes);
    }
    if output.len() as u64 > MAX_DECODED_MEMBER {
        return Err("Trace exceeds decoded byte limit".into());
    }
    String::from_utf8(output).map_err(|_| "Trace JSON is not UTF-8".into())
}

pub fn utc_midnight(date: &str) -> Result<f64, String> {
    let (year, month, day) = crate::acquisition::parse_date(date)?;
    let leap = |y: i32| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let year_days = |y: i32| if leap(y) { 366i64 } else { 365i64 };
    let mut days = if year >= 1970 {
        (1970..year).map(year_days).sum::<i64>()
    } else {
        -(year..1970).map(year_days).sum::<i64>()
    };
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    days += months[..month as usize - 1].iter().sum::<i64>() + i64::from(day) - 1;
    Ok((days * 86_400) as f64)
}

fn check_parts(parts: &[ArchivePart], base: &Path) -> Result<Vec<PathBuf>, String> {
    check_parts_with_cancel(parts, base, &AtomicBool::new(false))
}

fn check_parts_with_cancel(
    parts: &[ArchivePart],
    base: &Path,
    cancel: &AtomicBool,
) -> Result<Vec<PathBuf>, String> {
    if parts.is_empty() || parts.len() > 32 {
        return Err("Provide 1..32 complete ordered archive parts".into());
    }
    let mut seen = BTreeSet::new();
    let mut paths = Vec::new();
    for part in parts {
        let path = base
            .join(&part.path)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if !seen.insert(path.clone())
            || part.bytes == 0
            || fs::metadata(&path).map_err(|e| e.to_string())?.len() != part.bytes
        {
            return Err(format!(
                "Missing, duplicated, changed or incomplete archive part: {}",
                path.display()
            ));
        }
        import_checkpoint(cancel)?;
        let mut reader = BufReader::new(File::open(&path).map_err(|e| e.to_string())?);
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            import_checkpoint(cancel)?;
            let n = reader.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        if format!("{:x}", hasher.finalize()) != part.sha256 {
            return Err(format!(
                "Missing, duplicated, changed or incomplete archive part: {}",
                path.display()
            ));
        }
        paths.push(path);
    }
    Ok(paths)
}

/// Ingest a complete verified daily archive without unpacking the global TAR.
/// Malformed traces fail this run; they never silently disappear from coverage.
pub fn import_archive(request_path: &Path, output: &Path) -> Result<Value, String> {
    import_archive_with_cancel(request_path, output, &AtomicBool::new(false))
}

/// Cancellation is checked between archive members and integrity-verification
/// phases. Partial outputs retain a failure receipt and are never complete.
pub fn import_archive_with_cancel(
    request_path: &Path,
    output: &Path,
    cancel: &AtomicBool,
) -> Result<Value, String> {
    import_archive_mode(request_path, output, cancel, None)
}

/// Preserve native precision in bounded disk blocks instead of accumulating a
/// whole-day normalized JSON/map buffer. This does not change selection policy.
pub fn import_archive_indexed_with_cancel(
    request_path: &Path,
    output: &Path,
    max_disk_bytes: u64,
    cancel: &AtomicBool,
) -> Result<Value, String> {
    import_archive_indexed_with_limits(
        request_path,
        output,
        max_disk_bytes,
        crate::track_store::DEFAULT_INDEX_MEMORY_BYTES,
        cancel,
    )
}

/// Declare the disk and resident index-memory budgets independently. Neither
/// budget changes the requested time, region, source samples or gap policy.
pub fn import_archive_indexed_with_limits(
    request_path: &Path,
    output: &Path,
    max_disk_bytes: u64,
    max_index_memory_bytes: u64,
    cancel: &AtomicBool,
) -> Result<Value, String> {
    crate::track_store::validate_index_memory_limit(max_index_memory_bytes)?;
    import_archive_mode(
        request_path,
        output,
        cancel,
        Some((max_disk_bytes, max_index_memory_bytes)),
    )
}

fn import_archive_mode(
    request_path: &Path,
    output: &Path,
    cancel: &AtomicBool,
    indexed_budgets: Option<(u64, u64)>,
) -> Result<Value, String> {
    import_checkpoint(cancel)?;
    if output.exists() {
        return Err("Use a new detailed-import output directory".into());
    }
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let result = import_archive_inner(request_path, output, cancel, indexed_budgets);
    if let Err(error) = &result {
        let _ = write_json(
            &output.join("failure.json"),
            &json!({"status":"failed","error":error,
            "location_verified":false,"archive_import_complete":false}),
        );
    }
    result
}

fn import_checkpoint(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Detailed archive import canceled; partial output is not usable".into())
    } else {
        Ok(())
    }
}

fn import_archive_inner(
    request_path: &Path,
    output: &Path,
    cancel: &AtomicBool,
    indexed_budgets: Option<(u64, u64)>,
) -> Result<Value, String> {
    import_checkpoint(cancel)?;
    let request_receipt = receipt(request_path)?;
    let request: ArchiveImportRequest = read_json(request_path)?;
    adsblol::validate_config(&request.config)?;
    let base = request_path.parent().unwrap_or(Path::new("."));
    let acquisition_path = base.join(&request.acquisition_manifest);
    let acquisition_receipt = receipt(&acquisition_path)?;
    let acquisition: Value = read_json(&acquisition_path)?;
    if acquisition["schema_version"] != 1
        || acquisition["complete"] != true
        || acquisition["full_sha_verified"] != true
    {
        return Err("Import requires a complete SHA-verified acquisition manifest".into());
    }
    let midnight = utc_midnight(
        acquisition["date_utc"]
            .as_str()
            .ok_or("Acquisition needs its UTC date")?,
    )?;
    if request.config.time_origin_utc_s != midnight
        || request.config.time_window_utc_s[0] < midnight
        || request.config.time_window_utc_s[1] > midnight + 86_400.0
    {
        return Err("Import UTC origin/window must lie within the acquired archive day; use separate requests for additional days".into());
    }
    let acquired = acquisition["parts"]
        .as_array()
        .ok_or("Acquisition manifest needs ordered parts")?;
    if acquired.len() != request.parts.len() {
        return Err("Requested part count differs from acquisition".into());
    }
    for (actual, expected) in request.parts.iter().zip(acquired) {
        if expected["sha256"].as_str() != Some(&actual.sha256)
            || expected["bytes"].as_u64() != Some(actual.bytes)
        {
            return Err("Requested part identity/order differs from acquisition".into());
        }
    }
    let paths = check_parts_with_cancel(&request.parts, base, cancel)?;
    import_checkpoint(cancel)?;
    let total_bytes = request
        .parts
        .iter()
        .try_fold(0u64, |sum, part| sum.checked_add(part.bytes))
        .ok_or("Archive byte count overflow")?;
    if !total_bytes.is_multiple_of(512) {
        return Err("Archive is not a complete sequence of TAR blocks".into());
    }
    let mut plan = json!({"schema_version":1,"request":request_receipt,
        "acquisition_manifest":acquisition_receipt,"parts":request.parts,"config":request.config,
        "limits":{"trace_members":MAX_TRACE_MEMBERS,"selected_points":MAX_SELECTED_POINTS,
        "compressed_member_bytes":MAX_COMPRESSED_MEMBER,"decoded_member_bytes":MAX_DECODED_MEMBER},
        "failure_policy":"Stop on corruption, malformed trace or budget exhaustion; never claim complete partial import",
        "no_reference_coordinate_input":true,"location_verified":false});
    if let Some((limit, memory)) = indexed_budgets {
        plan["limits"]["selected_points"] = Value::Null;
        plan["limits"]["indexed_disk_bytes"] = json!(limit);
        plan["limits"]["index_memory_bytes"] = json!(memory);
        plan["storage"] = json!(
            "lossless native point blocks and compressed source provenance; no aggregate selected-point cap"
        );
    }
    write_json(&output.join("plan.json"), &plan)?;
    let mut indexed = indexed_budgets.map(|(limit, memory)| crate::track_store::TrackStoreWriter::new_with_memory_limit(
        output, &request.config,
        json!({"request":request_receipt,"acquisition_manifest":acquisition_receipt,"parts":request.parts,
            "selection":"Identical native time/quality/whole-aircraft regional selection; no physics decimation",
            "block_boundaries_are_not_pass_boundaries":true}), limit, memory)).transpose()?;
    let mut archive = tar::Archive::new(PartReader::new(paths));
    let mut reports = Vec::<TraceImportReport>::new();
    let mut selected_members = Vec::<Value>::new();
    let mut member_count = 0usize;
    let mut selected_points = 0usize;
    let mut traces = 0usize;
    let mut seen_hex = BTreeSet::new();
    for item in archive.entries().map_err(|e| e.to_string())? {
        import_checkpoint(cancel)?;
        let mut entry = item.map_err(|e| format!("Archive is incomplete or corrupt: {e}"))?;
        member_count += 1;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let Some(hex) = trace_member_name(&path)? else {
            continue;
        };
        if !seen_hex.insert(hex.clone()) {
            return Err("Duplicate full trace identity in one daily archive".into());
        }
        if !entry.header().entry_type().is_file() {
            return Err("Trace member is not a regular file".into());
        }
        traces += 1;
        if traces > MAX_TRACE_MEMBERS || entry.size() > MAX_COMPRESSED_MEMBER {
            return Err("Archive exceeds declared trace-count/member budget".into());
        }
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(MAX_COMPRESSED_MEMBER + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let raw = decode_member(&bytes)?;
        let report = adsblol::normalize_trace(&raw, &request.config)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if report.sources.len() != 1 || report.sources[0].icao24 != hex {
            return Err(format!(
                "Trace content/path identity mismatch: {}",
                path.display()
            ));
        }
        selected_points += report.stats.output_points;
        if indexed.is_none() && selected_points > MAX_SELECTED_POINTS {
            return Err("Selected data exceeds two-million-point budget; narrow the requested time/region explicitly".into());
        }
        if indexed.is_none() && report.stats.selected_traces > 0 {
            let directory = output.join("selected-traces");
            fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
            let filename = if bytes.starts_with(&[0x1f, 0x8b]) {
                format!("{hex}.json.gz")
            } else {
                format!("{hex}.json")
            };
            fs::write(directory.join(&filename), &bytes).map_err(|e| e.to_string())?;
            selected_members.push(
                json!({"archive_member":path,"retained_file":format!("selected-traces/{filename}"),
                "compressed_bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(&bytes)),
                "points":report.stats.output_points,"passes":report.stats.output_passes}),
            );
        }
        if let Some(store) = &mut indexed {
            store.push(
                report,
                json!({"path":path,"compressed_bytes":bytes.len(),
                "sha256":format!("{:x}",Sha256::digest(&bytes))}),
                cancel,
            )?;
        } else {
            reports.push(report);
        }
        if traces.is_multiple_of(5000) {
            eprintln!("Scanned {traces} aircraft traces; retained {selected_points} points");
        }
    }
    // tar stops at the first zero header. A complete TAR needs a second zero
    // block, and only zero padding may follow; never ignore appended payloads.
    let mut remainder = archive.into_inner();
    let mut padding = 0u64;
    let mut buffer = [0u8; 8192];
    loop {
        import_checkpoint(cancel)?;
        let n = remainder.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if buffer[..n].iter().any(|b| *b != 0) {
            return Err("Archive has nonzero data after its TAR terminator".into());
        }
        padding += n as u64;
    }
    if padding < 512 {
        return Err("Archive is missing the complete two-block TAR terminator".into());
    }
    if traces == 0 {
        return Err("Archive contains no documented full aircraft traces".into());
    }
    import_checkpoint(cancel)?;
    if let Some(store) = indexed {
        check_parts_with_cancel(&request.parts, base, cancel)?;
        import_checkpoint(cancel)?;
        if request_receipt != receipt(request_path)?
            || acquisition_receipt != receipt(&acquisition_path)?
        {
            return Err("Indexed import input changed during processing".into());
        }
        let mut result = store.finish(cancel)?;
        result["archive_members_scanned"] = json!(member_count);
        result["traces_scanned"] = json!(traces);
        result["inputs_unchanged"] = json!(true);
        write_json(&output.join("receipt.json"), &result)?;
        return Ok(result);
    }
    let normalized = adsblol::merge_reports(reports, &request.config)?;
    let stats = serde_json::to_value(&normalized.stats).map_err(|e| e.to_string())?;
    let normalized_path = output.join("normalized.json");
    write_json(&normalized_path, &normalized)?;
    check_parts(&request.parts, base)?;
    import_checkpoint(cancel)?;
    if request_receipt != receipt(request_path)?
        || acquisition_receipt != receipt(&acquisition_path)?
    {
        return Err("Import input changed during processing".into());
    }
    let report = json!({"schema_version":1,"status":"detailed_tracks_imported","archive_import_complete":true,
        "real_world_coverage_complete":false,"location_verified":false,"request":request_receipt,
        "acquisition_manifest":acquisition_receipt,"normalized":receipt(&normalized_path)?,
        "archive_members_scanned":member_count,"traces_scanned":traces,"stats":stats,
        "selected_members":selected_members,"inputs_unchanged":true,
        "limitations":["One selected receiver-instance archive; unavailable aircraft are not inferred absent.",
        "Date/time/region selection remains supplied prior information; geometric-only defaults may split or exclude traces.",
        "No historical flight identity or evaluation coordinate is used to select individual aircraft."]});
    write_json(&output.join("receipt.json"), &report)?;
    Ok(report)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerrainRequest {
    pub dem: PathBuf,
    pub geoid: PathBuf,
    pub observer_bounds: [f64; 4],
    pub grid_spacing_m: f64,
    pub microphone_height_m: f64,
    pub vertical_datum_assumption: String,
}

pub fn terrain_grid(request_path: &Path, output: &Path) -> Result<Value, String> {
    if output.exists() {
        return Err("Use a new terrain output directory".into());
    }
    let before = receipt(request_path)?;
    let request: TerrainRequest = read_json(request_path)?;
    if request.vertical_datum_assumption.trim().is_empty() {
        return Err("Declare the DEM/geoid vertical-datum assumption".into());
    }
    let base = request_path.parent().unwrap_or(Path::new("."));
    let mut dem = GeographicRaster::open(&base.join(&request.dem))?;
    let mut geoid = GeographicRaster::open(&base.join(&request.geoid))?;
    let grid = observer_grid(
        &mut dem,
        &mut geoid,
        request.observer_bounds,
        request.grid_spacing_m,
        request.microphone_height_m,
    )?;
    if before != receipt(request_path)? {
        return Err("Terrain request changed during processing".into());
    }
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let observers = output.join("observers.json");
    write_json(&observers, &grid.observers)?;
    write_json(&output.join("height-evidence.json"), &grid)?;
    let result = json!({"schema_version":1,"status":"terrain_grid_ready","location_verified":false,
        "request":before,"observer_count":grid.observers.len(),"observers":receipt(&observers)?,
        "vertical_datum_assumption":request.vertical_datum_assumption,"dem":grid.dem,"geoid":grid.geoid,
        "inputs_unchanged":true});
    write_json(&output.join("receipt.json"), &result)?;
    Ok(result)
}
