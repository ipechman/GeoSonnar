//! Small, explicit ADSB.lol map-history requests. Replay supplies candidates,
//! never complete-sky coverage or an acoustic association. Raw HTTP bodies and
//! native-import-compatible full traces are retained with local integrity hashes.
use crate::{adsblol, detailed_data, workspace};
use flate2::bufread::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_TRACES: usize = 256;
pub const MAX_DOWNLOAD_BYTES: u64 = 250_000_000;
pub const REPLAY_BYTE_LIMIT: u64 = 20_000_000;
pub const TRACE_BYTE_LIMIT: u64 = 2_000_000;
const MAX_DECODED_REPLAY: usize = 128_000_000;
const MAX_DECODED_TRACE: usize = 8_000_000;
const MAX_INDEX_ROWS: usize = 250_000;
const MAX_ATTEMPTS: usize = 1024;
// Shared native transport retains one byte beyond the requested limit as
// evidence of overflow. It is charged, never parsed or accepted as success.
const RESPONSE_LIMIT_SENTINEL_BYTES: u64 = 1;
const MAGIC: u32 = 0x0e7f7c9d;
pub const FORMAT_SOURCE_URL: &str = "https://adsb.lol/script_22b772ac55265ba79eeeca99ab34165c.js";
pub const FORMAT_SOURCE_SHA256: &str =
    "8cce1dff88b156e5323dca610ed643b10b60759ec3eff5c0206e9e82089d82dd";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryRequest {
    /// Explicit UTC, YYYY-MM-DDTHH:MM:SSZ; at most one hour in one calendar day.
    pub utc_start: String,
    pub utc_end: String,
    /// Aircraft coverage: south, west, north, east. Pad the receiver region.
    /// West > east denotes a date-line crossing. Each span is at most 20°.
    pub bounds: [f64; 4],
    pub max_traces: usize,
    /// Cumulative HTTP payload bytes for this cache, including failed attempts.
    pub max_download_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayChunk {
    pub index: u8,
    pub url: String,
    pub start_utc_s: f64,
    pub end_utc_s: f64,
    pub max_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryPlan {
    pub schema_version: u32,
    pub provider: String,
    pub license: String,
    pub source_docs: String,
    /// The inspected provider implementation defining this decoder, not a
    /// claim that a newly downloaded response has a provider-signed checksum.
    pub format_source_url: String,
    pub format_source_sha256: String,
    pub request: HistoryRequest,
    pub date_utc: String,
    pub start_utc_s: f64,
    pub end_utc_s: f64,
    pub replay_chunks: Vec<ReplayChunk>,
    pub trace_max_bytes: u64,
    pub maximum_files: usize,
    pub coverage_complete: bool,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayRecord {
    pub icao24: String,
    pub timestamp: f64,
    pub latitude: f64,
    pub longitude: f64,
    pub barometric_altitude_ft: Option<i32>,
    pub on_ground: bool,
    pub groundspeed_kt: Option<f64>,
    pub position_source_code: u8,
    pub position_source: String,
    pub replay_interval_s: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplaySummary {
    pub slices: usize,
    pub first_utc_s: f64,
    pub last_utc_s: f64,
    pub min_interval_s: f64,
    pub max_interval_s: f64,
    /// Missing nominal samples, not a guarantee that the remaining sky is complete.
    pub missing_slice_gaps: usize,
    pub leading_or_trailing_gap: bool,
    pub legacy_index_rows: usize,
    pub metadata_rows: usize,
    pub position_rows: usize,
    pub selected_rows: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayIndex {
    pub records: Vec<ReplayRecord>,
    pub summary: ReplaySummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileReceipt {
    pub attempt: usize,
    pub url: String,
    pub byte_limit: u64,
    pub body: Artifact,
    pub http_status: Option<u16>,
    pub error: Option<String>,
    pub fetched_at_unix_s: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayResult {
    pub chunk: ReplayChunk,
    pub receipt: Option<FileReceipt>,
    pub summary: Option<ReplaySummary>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub icao24: String,
    pub observations: usize,
    pub first_utc_s: f64,
    pub last_utc_s: f64,
    pub source_codes: Vec<u8>,
    pub source_types: Vec<String>,
    pub selected: bool,
    pub omission: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceResult {
    pub icao24: String,
    pub url: String,
    pub receipt: Option<FileReceipt>,
    pub decoded: Option<Artifact>,
    pub trace_points: Option<usize>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStatus {
    Complete,
    Partial,
    Cancelled,
    BudgetExhausted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryReport {
    pub schema_version: u32,
    pub plan: HistoryPlan,
    pub plan_sha256: String,
    pub status: HistoryStatus,
    pub replay: Vec<ReplayResult>,
    pub index: Artifact,
    pub candidates: Vec<Candidate>,
    pub traces: Vec<TraceResult>,
    pub attempts: Vec<FileReceipt>,
    pub http_payload_bytes: u64,
    pub new_http_payload_bytes: u64,
    pub reused_files: usize,
    pub non_icao_omissions: usize,
    pub trace_cap_omissions: usize,
    pub replay_files_complete: bool,
    pub replay_timing_gaps: bool,
    pub selected_traces_complete: bool,
    /// Always false: replay sampling and receiver availability cannot establish this.
    pub coverage_complete: bool,
    pub location_verified: bool,
    /// This immutable snapshot can be passed to the native importer separately.
    pub report_path: PathBuf,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct HistoryProgress {
    pub stage: String,
    pub completed_files: usize,
    pub selected_files: usize,
    pub http_payload_bytes: u64,
    pub max_download_bytes: u64,
}

pub use crate::http_download::TransportResult;
use crate::http_download::download as native_transport;

fn require(ok: bool, message: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec_pretty(value).map_err(|e| e.to_string())
}

pub fn plan(request: &HistoryRequest) -> Result<HistoryPlan, String> {
    for text in [&request.utc_start, &request.utc_end] {
        require(
            text.len() == 20 && text.ends_with('Z') && text.as_bytes().get(10) == Some(&b'T'),
            "Use explicit UTC YYYY-MM-DDTHH:MM:SSZ start and end",
        )?;
    }
    let start = workspace::parse_utc(&request.utc_start)?;
    let end = workspace::parse_utc(&request.utc_end)?;
    require(
        request.utc_start[..10] == request.utc_end[..10] && start < end && end - start <= 3600.0,
        "A small historical request must span at most one hour in one UTC calendar day",
    )?;
    let date = &request.utc_start[..10];
    require(
        (2023..=2200).contains(&date[..4].parse::<u32>().map_err(|_| "Invalid year")?),
        "ADSB.lol history dates must be between 2023 and 2200",
    )?;
    let [south, west, north, east] = request.bounds;
    let longitude_span = if east > west {
        east - west
    } else {
        east + 360.0 - west
    };
    require(
        request.bounds.iter().all(|v| v.is_finite())
            && south >= -90.0
            && north <= 90.0
            && north > south
            && north - south <= 20.0
            && (-180.0..=180.0).contains(&west)
            && (-180.0..=180.0).contains(&east)
            && longitude_span > 0.0
            && longitude_span <= 20.0,
        "Use a nonempty aircraft-coverage rectangle with latitude and longitude spans at most 20 degrees",
    )?;
    require(
        (1..=MAX_TRACES).contains(&request.max_traces),
        "Choose 1..256 full traces",
    )?;
    require(
        request.max_download_bytes > 0 && request.max_download_bytes <= MAX_DOWNLOAD_BYTES,
        "Use a positive cumulative payload allowance of at most 0.2328306437 GiB",
    )?;
    let midnight = detailed_data::utc_midnight(date)?;
    let first = ((start - midnight) / 1800.0) as u8;
    let last = ((end - midnight) / 1800.0) as u8;
    let day_path = date.replace('-', "/");
    let replay_chunks: Vec<_> = (first..=last)
        .map(|index| ReplayChunk {
            index,
            url: format!("https://adsb.lol/globe_history/{day_path}/heatmap/{index:02}.bin.ttf"),
            start_utc_s: midnight + f64::from(index) * 1800.0,
            end_utc_s: midnight + f64::from(index + 1) * 1800.0,
            max_bytes: REPLAY_BYTE_LIMIT,
        })
        .collect();
    Ok(HistoryPlan {
        schema_version: 1, provider: "ADSB.lol public map history".into(), license: "ODbL-1.0".into(),
        source_docs: "https://www.adsb.lol/docs/open-data/historical".into(),
        format_source_url: FORMAT_SOURCE_URL.into(), format_source_sha256: FORMAT_SOURCE_SHA256.into(),
        request: request.clone(), date_utc: date.into(), start_utc_s: start, end_utc_s: end,
        maximum_files: replay_chunks.len()+request.max_traces, replay_chunks,
        trace_max_bytes: TRACE_BYTE_LIMIT, coverage_complete: false,
        limitations: vec![
            "Replay is a sampled candidate index, not exhaustive sky coverage or an aircraft absence test.".into(),
            "The rectangle covers aircraft positions; pad the receiver region for acoustic travel and aircraft motion before planning.".into(),
            "Candidates crossing the rectangle between replay samples, missing positions, untracked aircraft and receiver gaps can be omitted.".into(),
            "Non-ICAO identities remain in the index but are explicitly omitted from this full-trace downloader.".into(),
            "All source codes, including unknown and MLAT, remain in the index. Native import applies a separate explicit position-source and altitude policy.".into(),
            "Trace-cap selection is lexicographic ICAO order, not relevance, acoustic evidence or aircraft-type ranking.".into(),
            "SHA-256 receipts detect later local changes; they are not provider signatures or independent timestamp certification.".into(),
            "Index altitude is quantized barometric altitude. Raw full traces retain their original altitude/source fields for the native importer.".into(),
            "A downloaded flight is not a confirmed acoustic association or verified recording location.".into(),
        ],
    })
}

pub fn validate_plan(value: &HistoryPlan) -> Result<(), String> {
    require(
        *value == plan(&value.request)?,
        "Historical plan differs from its reconstructed bounded request; regenerate the plan",
    )
}

/// Strict single-member gzip decoding, bounded even for compressed bombs.
pub fn decode_payload(bytes: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    if !bytes.starts_with(&[0x1f, 0x8b]) {
        require(
            bytes.len() <= limit,
            "Response exceeds its decoded-data allowance",
        )?;
        return Ok(bytes.to_vec());
    }
    let mut decoder = GzDecoder::new(Cursor::new(bytes));
    let mut output = Vec::new();
    decoder
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut output)
        .map_err(|e| format!("Invalid gzip response: {e}"))?;
    require(
        output.len() <= limit,
        "Response exceeds its decoded-data allowance",
    )?;
    require(
        decoder.into_inner().position() == bytes.len() as u64,
        "Gzip response contains trailing data or multiple members",
    )?;
    Ok(output)
}

fn source_name(code: u8) -> &'static str {
    match code {
        0 => "adsb_icao",
        1 => "adsb_icao_nt",
        2 => "adsr_icao",
        3 => "tisb_icao",
        4 => "adsc",
        5 => "mlat",
        6 => "other",
        7 => "mode_s",
        8 => "adsb_other",
        9 => "adsr_other",
        10 => "tisb_trackfile",
        11 => "tisb_other",
        12 => "mode_ac",
        _ => "unknown",
    }
}

/// Decode one official half-hour file. Validate every coordinate and timestamp,
/// including positions outside the selected rectangle, before filtering.
pub fn decode_replay(
    bytes: &[u8],
    plan: &HistoryPlan,
    chunk: &ReplayChunk,
) -> Result<ReplayIndex, String> {
    validate_plan(plan)?;
    require(
        plan.replay_chunks.contains(chunk),
        "Replay chunk is not part of this plan",
    )?;
    require(
        bytes.len() as u64 <= chunk.max_bytes,
        "Replay HTTP body exceeds its transfer allowance",
    )?;
    let raw = decode_payload(bytes, MAX_DECODED_REPLAY)?;
    require(
        raw.len().is_multiple_of(16),
        "Replay byte length must be a multiple of 16",
    )?;
    let words: Vec<[u32; 4]> = raw
        .chunks_exact(16)
        .map(|row| {
            std::array::from_fn(|i| u32::from_le_bytes(row[4 * i..4 * i + 4].try_into().unwrap()))
        })
        .collect();
    let first = words
        .iter()
        .position(|r| r[0] == MAGIC)
        .ok_or("Replay timestamp markers missing")?;
    let mut legacy = Vec::new();
    for row in &words[..first] {
        require(
            row[1..] == [0, 0, 0]
                && (row[0] as usize) < words.len()
                && words[row[0] as usize][0] == MAGIC,
            "Invalid legacy replay slice index",
        )?;
        legacy.push(row[0] as usize);
    }
    let mut markers = Vec::new();
    let mut records = Vec::new();
    let mut summary = ReplaySummary {
        slices: 0,
        first_utc_s: 0.0,
        last_utc_s: 0.0,
        min_interval_s: f64::INFINITY,
        max_interval_s: 0.0,
        missing_slice_gaps: 0,
        leading_or_trailing_gap: false,
        legacy_index_rows: first,
        metadata_rows: 0,
        position_rows: 0,
        selected_rows: 0,
    };
    let mut timestamp = 0.0;
    let mut interval: f64 = 0.0;
    for (offset, row) in words.iter().enumerate().skip(first) {
        let [a, b, c, d] = *row;
        if a == MAGIC {
            let next = (((b as u64) << 32) | c as u64) as f64 / 1000.0;
            let next_interval = f64::from(d & 65535) / 1000.0;
            require(
                next_interval > 0.0 && next_interval <= 60.0,
                "Invalid replay sampling interval",
            )?;
            require(
                next >= chunk.start_utc_s && next <= chunk.end_utc_s,
                "Replay timestamp does not belong to its requested date/half-hour",
            )?;
            if summary.slices > 0 {
                require(
                    next > timestamp,
                    "Replay slices must be strictly increasing",
                )?;
                if next - timestamp > interval.max(next_interval) + 0.001 {
                    summary.missing_slice_gaps += 1;
                }
            } else {
                summary.first_utc_s = next;
            }
            timestamp = next;
            interval = next_interval;
            summary.slices += 1;
            summary.last_utc_s = timestamp;
            summary.min_interval_s = summary.min_interval_s.min(interval);
            summary.max_interval_s = summary.max_interval_s.max(interval);
            markers.push(offset);
            continue;
        }
        let lat_raw = b as i32;
        if lat_raw >= (1 << 30) {
            summary.metadata_rows += 1;
            continue;
        }
        let latitude = f64::from(lat_raw) / 1e6;
        let longitude = f64::from(c as i32) / 1e6;
        require(
            (-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude),
            "Replay coordinate outside geographic range",
        )?;
        let altitude = (d & 65535) as i16;
        let speed = (d >> 16) as i16;
        require(speed >= -1, "Replay contains invalid negative ground speed")?;
        summary.position_rows += 1;
        if timestamp < plan.start_utc_s
            || timestamp > plan.end_utc_s
            || !adsblol::point_in_bounds(latitude, longitude, plan.request.bounds)
        {
            continue;
        }
        require(
            records.len() < MAX_INDEX_ROWS,
            "Too many replay positions; reduce the region or time window",
        )?;
        let code = ((a >> 27) & 31) as u8;
        records.push(ReplayRecord {
            icao24: format!(
                "{}{:06x}",
                if a & 0x1000000 != 0 { "~" } else { "" },
                a & 0xffffff
            ),
            timestamp,
            latitude,
            longitude,
            barometric_altitude_ft: if altitude == -123 || altitude == -124 {
                None
            } else {
                Some(i32::from(altitude) * 25)
            },
            on_ground: altitude == -123,
            groundspeed_kt: if speed == -1 {
                None
            } else {
                Some(f64::from(speed) / 10.0)
            },
            position_source_code: code,
            position_source: source_name(code).into(),
            replay_interval_s: interval,
        });
    }
    require(
        first == 0 || legacy == markers,
        "Legacy replay index does not enumerate every slice exactly once in order",
    )?;
    summary.leading_or_trailing_gap = summary.first_utc_s - chunk.start_utc_s
        > summary.max_interval_s + 0.001
        || chunk.end_utc_s - summary.last_utc_s > interval + 0.001;
    summary.selected_rows = records.len();
    Ok(ReplayIndex { records, summary })
}

fn trace_url(plan: &HistoryPlan, identity: &str) -> String {
    format!(
        "https://adsb.lol/globe_history/{}/traces/{}/trace_full_{}.json",
        plan.date_utc.replace('-', "/"),
        &identity[4..],
        identity
    )
}

fn validate_trace(
    bytes: &[u8],
    identity: &str,
    plan: &HistoryPlan,
) -> Result<(Vec<u8>, usize), String> {
    let decoded = decode_payload(bytes, MAX_DECODED_TRACE)?;
    let value: Value =
        serde_json::from_slice(&decoded).map_err(|e| format!("Invalid trace JSON: {e}"))?;
    let id = value
        .get("icao")
        .and_then(Value::as_str)
        .ok_or("Documented readsb trace ICAO identity missing; a hex-only payload is not native-import-compatible")?;
    require(
        id == identity,
        "Trace identity differs from the replay candidate",
    )?;
    let rows = value
        .get("trace")
        .and_then(Value::as_array)
        .ok_or("Full trace array missing")?;
    let base = value
        .get("timestamp")
        .and_then(Value::as_f64)
        .ok_or("Trace timestamp missing")?;
    let midnight = detailed_data::utc_midnight(&plan.date_utc)?;
    require(
        base.is_finite() && base >= midnight && base < midnight + 86400.0,
        "Full trace base timestamp differs from requested UTC day",
    )?;
    adsblol::validate_trace_rows(rows, base)?;
    // The detailed native importer applies the separate source/altitude/continuity
    // policy. Acquisition preserves those raw rows, including rejected rows.
    Ok((decoded, rows.len()))
}

fn ordinary(path: &Path, directory: bool) -> Result<(), String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("Cannot inspect {}: {e}", path.display()))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        require(
            meta.file_attributes() & 0x400 == 0,
            "History cache must not contain Windows reparse points",
        )?;
    }
    require(
        !meta.file_type().is_symlink()
            && if directory {
                meta.is_dir()
            } else {
                meta.is_file()
            },
        "History cache path has an unexpected file type",
    )
}
fn safe_directory(path: &Path) -> Result<(), String> {
    require(
        !path.as_os_str().is_empty() && !path.components().any(|c| c == Component::ParentDir),
        "History cache path cannot contain parent traversal",
    )?;
    let mut walked = PathBuf::new();
    for component in path.components() {
        walked.push(component);
        if walked.exists() {
            ordinary(&walked, true)?;
        }
    }
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    ordinary(path, true)
}
fn confined(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    require(
        !relative.as_os_str().is_empty()
            && relative.components().all(
                |c| matches!(c,Component::Normal(s) if s.to_str().is_some_and(|s|!s.contains(':'))),
            ),
        "History artifact path must be confined and relative",
    )?;
    let mut path = root.to_path_buf();
    for c in relative.components() {
        path.push(c);
        if path.exists() {
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            ordinary(&path, meta.is_dir())?;
        }
    }
    Ok(path)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn write_json_new<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    write_new(path, &json_bytes(value)?)
}
fn artifact(root: &Path, path: &Path, bytes: &[u8]) -> Result<Artifact, String> {
    Ok(Artifact {
        path: path.strip_prefix(root).map_err(|e| e.to_string())?.into(),
        bytes: bytes.len() as u64,
        sha256: hash(bytes),
    })
}
fn verified_artifact(root: &Path, file: &Artifact, max: u64) -> Result<Vec<u8>, String> {
    let path = confined(root, &file.path)?;
    ordinary(&path, false)?;
    require(
        file.bytes <= max && fs::metadata(&path).map_err(|e| e.to_string())?.len() == file.bytes,
        "Cached history artifact length differs from receipt",
    )?;
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    require(
        hash(&bytes) == file.sha256,
        "Cached history artifact SHA-256 differs from its acquisition receipt",
    )?;
    Ok(bytes)
}

/// Persistent control file; ownership belongs to the OS lock, not its existence
/// or a saved PID. Dropping the handle or process death releases the lock.
pub(crate) fn try_lock_cache(root: &Path) -> Result<File, String> {
    safe_directory(root)?;
    let path = root.join("download.lock");
    if path.exists() {
        ordinary(&path, false)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| format!("Cannot open history cache lock: {e}"))?;
    file.try_lock()
        .map_err(|_| "History cache is locked by another active operation".to_string())?;
    // Migrate the old PID sentinel only after obtaining the actual OS lock.
    file.set_len(0).map_err(|e| e.to_string())?;
    Ok(file)
}

#[derive(Serialize, Deserialize)]
struct AttemptRequest {
    url: String,
    byte_limit: u64,
}

struct Cache<'a> {
    root: &'a Path,
    plan: &'a HistoryPlan,
    attempts: Vec<FileReceipt>,
    total: u64,
    initial: u64,
    reused: usize,
}
type FileOutcome = (Option<FileReceipt>, Option<Vec<u8>>, Option<String>);
impl<'a> Cache<'a> {
    fn load(root: &'a Path, plan: &'a HistoryPlan) -> Result<Self, String> {
        let mut directories = Vec::new();
        for entry in fs::read_dir(root.join("attempts")).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "Invalid history attempt name")?;
            require(
                name.len() == 8 && name.bytes().all(|b| b.is_ascii_digit()),
                "Unexpected history cache entry",
            )?;
            ordinary(&entry.path(), true)?;
            directories.push((name, entry.path()));
        }
        directories.sort();
        require(
            directories.len() <= MAX_ATTEMPTS,
            "History retry limit reached; use a fresh bounded request",
        )?;
        let mut attempts = Vec::new();
        let mut total = 0u64;
        for (i, (name, dir)) in directories.into_iter().enumerate() {
            require(
                name == format!("{:08}", i + 1),
                "History attempt sequence has gaps",
            )?;
            let request_path = dir.join("request.json");
            ordinary(&request_path, false)?;
            require(
                fs::metadata(&request_path)
                    .map_err(|e| e.to_string())?
                    .len()
                    < 16384,
                "Oversized attempt request",
            )?;
            let request: AttemptRequest =
                serde_json::from_slice(&fs::read(&request_path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            let cap = if plan.replay_chunks.iter().any(|c| c.url == request.url) {
                REPLAY_BYTE_LIMIT
            } else if valid_trace_url(plan, &request.url) {
                TRACE_BYTE_LIMIT
            } else {
                return Err("Cached request URL differs from this plan".into());
            };
            let remaining = plan.request.max_download_bytes.saturating_sub(total);
            require(
                request.byte_limit > 0 && request.byte_limit <= cap.min(remaining),
                "Cached request exceeds its file or cumulative budget",
            )?;
            let body_path = dir.join("response.bin");
            if !body_path.exists() {
                write_new(&body_path, &[])?;
            }
            ordinary(&body_path, false)?;
            require(
                fs::metadata(&body_path).map_err(|e| e.to_string())?.len()
                    <= request.byte_limit + RESPONSE_LIMIT_SENTINEL_BYTES,
                "Interrupted history response exceeded its limit",
            )?;
            let receipt_path = dir.join("receipt.json");
            let receipt: FileReceipt = if receipt_path.exists() {
                ordinary(&receipt_path, false)?;
                require(
                    fs::metadata(&receipt_path)
                        .map_err(|e| e.to_string())?
                        .len()
                        < 32768,
                    "Oversized history receipt",
                )?;
                serde_json::from_slice(&fs::read(&receipt_path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?
            } else {
                let bytes = fs::read(&body_path).map_err(|e| e.to_string())?;
                let value=FileReceipt {attempt:i+1,url:request.url.clone(),byte_limit:request.byte_limit,body:artifact(root,&body_path,&bytes)?,http_status:None,
                    error:Some("Previous process stopped before completing this attempt; retained partial payload is not reused".into()),fetched_at_unix_s:now()?};
                write_json_new(&receipt_path, &value)?;
                value
            };
            require(
                receipt.attempt == i + 1
                    && receipt.url == request.url
                    && receipt.byte_limit == request.byte_limit
                    && receipt.body.path == body_path.strip_prefix(root).unwrap(),
                "History attempt receipt differs from its request",
            )?;
            require(
                receipt.error.is_some()
                    || (receipt.http_status == Some(200)
                        && receipt.body.bytes <= request.byte_limit),
                "Cached successful response lacks HTTP 200 status or exceeds its transfer allowance",
            )?;
            verified_artifact(
                root,
                &receipt.body,
                request.byte_limit + RESPONSE_LIMIT_SENTINEL_BYTES,
            )?;
            total = total
                .checked_add(receipt.body.bytes)
                .ok_or("History transfer-size total overflow")?;
            attempts.push(receipt);
        }
        require(
            total <= plan.request.max_download_bytes + RESPONSE_LIMIT_SENTINEL_BYTES,
            "Cached attempts already exceed the cumulative request budget",
        )?;
        Ok(Cache {
            root,
            plan,
            attempts,
            total,
            initial: total,
            reused: 0,
        })
    }
    fn obtain<T, F>(
        &mut self,
        url: &str,
        cap: u64,
        cancel: &AtomicBool,
        transport: &mut T,
        validate: F,
    ) -> Result<FileOutcome, String>
    where
        T: FnMut(&str, u64, &Path, &AtomicBool) -> Result<TransportResult, String>,
        F: Fn(&[u8]) -> Result<(), String>,
    {
        if let Some(cached) = self
            .attempts
            .iter()
            .rev()
            .find(|r| r.url == url && r.error.is_none())
            .cloned()
        {
            let bytes = verified_artifact(self.root, &cached.body, cap)?;
            validate(&bytes).map_err(|e| format!("Cached response fails validation: {e}"))?;
            self.reused += 1;
            return Ok((Some(cached), Some(bytes), None));
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok((
                None,
                None,
                Some("Download cancelled before this file".into()),
            ));
        }
        let remaining = self
            .plan
            .request
            .max_download_bytes
            .saturating_sub(self.total);
        if remaining == 0 {
            return Ok((
                None,
                None,
                Some("Cumulative HTTP payload budget exhausted".into()),
            ));
        }
        require(
            self.attempts.len() < MAX_ATTEMPTS,
            "History retry limit reached",
        )?;
        let sequence = self.attempts.len() + 1;
        let dir = self.root.join("attempts").join(format!("{sequence:08}"));
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let limit = cap.min(remaining);
        write_json_new(
            &dir.join("request.json"),
            &AttemptRequest {
                url: url.into(),
                byte_limit: limit,
            },
        )?;
        let body = dir.join("response.bin");
        write_new(&body, &[])?;
        let result = transport(url, limit, &body, cancel).unwrap_or_else(|error| TransportResult {
            http_status: None,
            error: Some(error),
        });
        ordinary(&body, false)?;
        let bytes_count = fs::metadata(&body).map_err(|e| e.to_string())?.len();
        require(
            bytes_count <= limit + RESPONSE_LIMIT_SENTINEL_BYTES,
            "Downloader exceeded its bounded response size; partial attempt retained",
        )?;
        let bytes = fs::read(&body).map_err(|e| e.to_string())?;
        self.total = self
            .total
            .checked_add(bytes_count)
            .ok_or("History transfer-size total overflow")?;
        let error = if bytes_count > limit {
            Some(
                "Response exceeded its transfer allowance; overflow sentinel is retained and charged"
                    .into(),
            )
        } else {
            result.error.or_else(|| {
                if result.http_status != Some(200) {
                    Some("Expected HTTP 200 for a complete history file".into())
                } else {
                    validate(&bytes).err()
                }
            })
        };
        let receipt = FileReceipt {
            attempt: sequence,
            url: url.into(),
            byte_limit: limit,
            body: artifact(self.root, &body, &bytes)?,
            http_status: result.http_status,
            error: error.clone(),
            fetched_at_unix_s: now()?,
        };
        write_json_new(&dir.join("receipt.json"), &receipt)?;
        self.attempts.push(receipt.clone());
        Ok((
            Some(receipt),
            if error.is_none() { Some(bytes) } else { None },
            error,
        ))
    }
}
fn now() -> Result<u64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs())
}
fn valid_trace_url(plan: &HistoryPlan, url: &str) -> bool {
    let Some(tail) = url
        .rsplit('/')
        .next()
        .and_then(|s| s.strip_prefix("trace_full_"))
        .and_then(|s| s.strip_suffix(".json"))
    else {
        return false;
    };
    tail.len() == 6
        && tail
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        && trace_url(plan, tail) == url
}

pub fn fetch<F>(
    plan: &HistoryPlan,
    cache: &Path,
    cancel: &AtomicBool,
    progress: F,
) -> Result<HistoryReport, String>
where
    F: FnMut(HistoryProgress),
{
    fetch_with_transport(plan, cache, cancel, progress, native_transport)
}

/// Injectable bounded transport for offline replay tests or a verified local
/// cache adapter. It follows the same validation, accounting and receipt path.
pub fn fetch_with_transport<F, T>(
    plan: &HistoryPlan,
    root: &Path,
    cancel: &AtomicBool,
    progress: F,
    transport: T,
) -> Result<HistoryReport, String>
where
    F: FnMut(HistoryProgress),
    T: FnMut(&str, u64, &Path, &AtomicBool) -> Result<TransportResult, String>,
{
    validate_plan(plan)?;
    let _lock = try_lock_cache(root)?;
    fetch_with_transport_in_locked_cache(plan, root, cancel, progress, transport)
}

/// Internal managed-catalog entry: caller retains try_lock_cache through both
/// acquisition and immutable catalog registration, avoiding a race in between.
pub(crate) fn fetch_in_locked_cache<F>(
    plan: &HistoryPlan,
    root: &Path,
    cancel: &AtomicBool,
    progress: F,
) -> Result<HistoryReport, String>
where
    F: FnMut(HistoryProgress),
{
    fetch_with_transport_in_locked_cache(plan, root, cancel, progress, native_transport)
}

pub(crate) fn fetch_with_transport_in_locked_cache<F, T>(
    plan: &HistoryPlan,
    root: &Path,
    cancel: &AtomicBool,
    mut progress: F,
    mut transport: T,
) -> Result<HistoryReport, String>
where
    F: FnMut(HistoryProgress),
    T: FnMut(&str, u64, &Path, &AtomicBool) -> Result<TransportResult, String>,
{
    validate_plan(plan)?;
    safe_directory(root)?;
    let plan_path = root.join("plan.json");
    let encoded = json_bytes(plan)?;
    let plan_hash = hash(&encoded);
    if plan_path.exists() {
        ordinary(&plan_path, false)?;
        require(
            fs::metadata(&plan_path).map_err(|e| e.to_string())?.len() < 65536,
            "Oversized cached plan",
        )?;
        require(
            fs::read(&plan_path).map_err(|e| e.to_string())? == encoded,
            "Cache belongs to a different historical request; choose a new directory",
        )?;
    } else {
        write_new(&plan_path, &encoded)?;
    }
    for name in ["attempts", "traces", "reports"] {
        safe_directory(&root.join(name))?;
    }
    let mut cache = Cache::load(root, plan)?;
    let mut replay = Vec::new();
    let mut records = Vec::new();
    let mut completed = 0;
    for chunk in &plan.replay_chunks {
        let (receipt, bytes, error) = cache.obtain(
            &chunk.url,
            chunk.max_bytes,
            cancel,
            &mut transport,
            |bytes| decode_replay(bytes, plan, chunk).map(|_| ()),
        )?;
        let summary = if let Some(bytes) = bytes {
            let index = decode_replay(&bytes, plan, chunk)?;
            require(
                records.len() + index.records.len() <= MAX_INDEX_ROWS,
                "Combined replay index exceeds row bound; reduce request",
            )?;
            records.extend(index.records);
            completed += 1;
            Some(index.summary)
        } else {
            None
        };
        replay.push(ReplayResult {
            chunk: chunk.clone(),
            receipt,
            summary,
            error,
        });
        progress(HistoryProgress {
            stage: "replay_index".into(),
            completed_files: completed,
            selected_files: plan.replay_chunks.len(),
            http_payload_bytes: cache.total,
            max_download_bytes: plan.request.max_download_bytes,
        });
    }
    let mut by_identity: BTreeMap<String, Vec<&ReplayRecord>> = BTreeMap::new();
    for row in &records {
        by_identity.entry(row.icao24.clone()).or_default().push(row);
    }
    let mut selected = 0;
    let mut non_icao = 0;
    let mut capped = 0;
    let mut candidates = Vec::new();
    let mut traces = Vec::new();
    for (identity, rows) in by_identity {
        let omission = if identity.starts_with('~') {
            non_icao += 1;
            Some(
                "Non-ICAO identity: full-trace acquisition is not supported by this downloader"
                    .into(),
            )
        } else if selected >= plan.request.max_traces {
            capped += 1;
            Some("Explicit trace cap reached; lexicographic ICAO selection".into())
        } else {
            selected += 1;
            None
        };
        let source_codes: BTreeSet<_> = rows.iter().map(|r| r.position_source_code).collect();
        candidates.push(Candidate {
            icao24: identity,
            observations: rows.len(),
            first_utc_s: rows
                .iter()
                .map(|r| r.timestamp)
                .fold(f64::INFINITY, f64::min),
            last_utc_s: rows
                .iter()
                .map(|r| r.timestamp)
                .fold(f64::NEG_INFINITY, f64::max),
            source_types: source_codes
                .iter()
                .map(|&c| source_name(c).into())
                .collect(),
            source_codes: source_codes.into_iter().collect(),
            selected: omission.is_none(),
            omission,
        });
    }
    for candidate in candidates.iter().filter(|c| c.selected) {
        let url = trace_url(plan, &candidate.icao24);
        let (receipt, bytes, error) =
            cache.obtain(&url, TRACE_BYTE_LIMIT, cancel, &mut transport, |bytes| {
                validate_trace(bytes, &candidate.icao24, plan).map(|_| ())
            })?;
        let (decoded, trace_points) = if let Some(bytes) = bytes {
            let (decoded, count) = validate_trace(&bytes, &candidate.icao24, plan)?;
            let path = root
                .join("traces")
                .join(format!("trace_full_{}.json", candidate.icao24));
            let info = artifact(root, &path, &decoded)?;
            if path.exists() {
                verified_artifact(root, &info, MAX_DECODED_TRACE as u64)?;
            } else {
                write_new(&path, &decoded)?;
            }
            completed += 1;
            (Some(info), Some(count))
        } else {
            (None, None)
        };
        traces.push(TraceResult {
            icao24: candidate.icao24.clone(),
            url,
            receipt,
            decoded,
            trace_points,
            error,
        });
        progress(HistoryProgress {
            stage: "full_traces".into(),
            completed_files: completed,
            selected_files: plan.replay_chunks.len() + selected,
            http_payload_bytes: cache.total,
            max_download_bytes: plan.request.max_download_bytes,
        });
    }
    let replay_files_complete = replay.iter().all(|r| r.summary.is_some());
    let replay_timing_gaps = replay
        .iter()
        .filter_map(|r| r.summary.as_ref())
        .any(|s| s.missing_slice_gaps > 0 || s.leading_or_trailing_gap);
    let selected_traces_complete = traces.iter().all(|r| r.decoded.is_some());
    let status = if cancel.load(Ordering::Relaxed) {
        HistoryStatus::Cancelled
    } else if (!replay_files_complete || !selected_traces_complete)
        && cache.total >= plan.request.max_download_bytes
    {
        HistoryStatus::BudgetExhausted
    } else if !replay_files_complete
        || !selected_traces_complete
        || replay_timing_gaps
        || non_icao > 0
        || capped > 0
    {
        HistoryStatus::Partial
    } else {
        HistoryStatus::Complete
    };
    // Immutable numbered snapshots preserve failed/cancelled runs and retries.
    let mut revision = 1;
    while root
        .join("reports")
        .join(format!("{revision:08}.json"))
        .exists()
        || root
            .join("reports")
            .join(format!("{revision:08}-index.json"))
            .exists()
    {
        revision += 1;
        require(revision <= MAX_ATTEMPTS, "Too many report snapshots")?;
    }
    let index_path = root
        .join("reports")
        .join(format!("{revision:08}-index.json"));
    let index_bytes = json_bytes(&records)?;
    write_new(&index_path, &index_bytes)?;
    let report_path = PathBuf::from("reports").join(format!("{revision:08}.json"));
    let report = HistoryReport {
        schema_version: 1,
        plan: plan.clone(),
        plan_sha256: plan_hash,
        status,
        replay,
        index: artifact(root, &index_path, &index_bytes)?,
        candidates,
        traces,
        http_payload_bytes: cache.total,
        new_http_payload_bytes: cache.total - cache.initial,
        reused_files: cache.reused,
        attempts: cache.attempts,
        non_icao_omissions: non_icao,
        trace_cap_omissions: capped,
        replay_files_complete,
        replay_timing_gaps,
        selected_traces_complete,
        coverage_complete: false,
        location_verified: false,
        report_path: report_path.clone(),
        limitations: plan.limitations.clone(),
    };
    write_json_new(&root.join(report_path), &report)?;
    Ok(report)
}

/// Explicit native normalization of downloaded traces. Partial acquisition is
/// allowed only when the caller opts in; its limitations remain in quality notes.
pub fn import_traces(
    root: &Path,
    report: &HistoryReport,
    config: &adsblol::TraceImportConfig,
    allow_partial: bool,
) -> Result<adsblol::TraceImportReport, String> {
    import_traces_with_cancel(root, report, config, allow_partial, &AtomicBool::new(false))
}

pub fn import_traces_with_cancel(
    root: &Path,
    report: &HistoryReport,
    config: &adsblol::TraceImportConfig,
    allow_partial: bool,
    cancel: &AtomicBool,
) -> Result<adsblol::TraceImportReport, String> {
    require(
        !cancel.load(Ordering::Relaxed),
        "Historical trace import cancelled",
    )?;
    validate_plan(&report.plan)?;
    require(
        report.plan_sha256 == hash(&json_bytes(&report.plan)?),
        "Historical report plan fingerprint differs",
    )?;
    require(
        !report.coverage_complete && !report.location_verified,
        "Historical report cannot claim sky coverage or a verified location",
    )?;
    require(
        report.status == HistoryStatus::Complete || allow_partial,
        "Historical acquisition is partial; explicitly allow partial import after reviewing its omissions",
    )?;
    adsblol::validate_config(config)?;
    require(
        config.time_window_utc_s[0] >= report.plan.start_utc_s
            && config.time_window_utc_s[1] <= report.plan.end_utc_s,
        "Import window must stay inside the replay candidate-selection interval; acquire a wider interval first",
    )?;
    // Bind the complete snapshot, not editable in-memory lists of selected files.
    let saved = confined(root, &report.report_path)?;
    ordinary(&saved, false)?;
    require(
        fs::metadata(&saved).map_err(|e| e.to_string())?.len() <= 64_000_000,
        "Oversized historical report",
    )?;
    let stored: HistoryReport =
        serde_json::from_slice(&fs::read(&saved).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    require(
        json_bytes(&stored)? == json_bytes(report)?,
        "Historical report differs from its saved acquisition snapshot",
    )?;
    let plan_path = confined(root, Path::new("plan.json"))?;
    ordinary(&plan_path, false)?;
    require(
        fs::metadata(&plan_path).map_err(|e| e.to_string())?.len() < 65536
            && fs::read(&plan_path).map_err(|e| e.to_string())? == json_bytes(&report.plan)?,
        "Cached plan differs from historical report",
    )?;
    let mut payload_total = 0u64;
    for attempt in &report.attempts {
        require(
            !cancel.load(Ordering::Relaxed),
            "Historical trace import cancelled during source verification",
        )?;
        let limit = if report
            .plan
            .replay_chunks
            .iter()
            .any(|c| c.url == attempt.url)
        {
            REPLAY_BYTE_LIMIT
        } else if valid_trace_url(&report.plan, &attempt.url) {
            TRACE_BYTE_LIMIT
        } else {
            return Err("Historical attempt URL outside declared provider request".into());
        };
        let remaining = report
            .plan
            .request
            .max_download_bytes
            .saturating_sub(payload_total);
        require(
            attempt.byte_limit > 0
                && attempt.byte_limit <= limit.min(remaining)
                && (attempt.error.is_some()
                    || (attempt.http_status == Some(200)
                        && attempt.body.bytes <= attempt.byte_limit)),
            "Historical attempt exceeds its budget or claims oversized success",
        )?;
        verified_artifact(
            root,
            &attempt.body,
            attempt.byte_limit + RESPONSE_LIMIT_SENTINEL_BYTES,
        )?;
        payload_total = payload_total
            .checked_add(attempt.body.bytes)
            .ok_or("Historical payload count overflow")?;
    }
    require(
        payload_total == report.http_payload_bytes
            && payload_total
                <= report.plan.request.max_download_bytes + RESPONSE_LIMIT_SENTINEL_BYTES,
        "Historical report transfer accounting differs",
    )?;
    verified_artifact(root, &report.index, 128_000_000)?;
    let mut reports = Vec::new();
    let mut identities = BTreeSet::new();
    for trace in &report.traces {
        require(
            !cancel.load(Ordering::Relaxed),
            "Historical trace import cancelled",
        )?;
        if let Some(file) = &trace.decoded {
            require(
                identities.insert(&trace.icao24)
                    && report
                        .candidates
                        .iter()
                        .any(|c| c.icao24 == trace.icao24 && c.selected),
                "Unexpected or duplicate trace in acquisition report",
            )?;
            let bytes = verified_artifact(root, file, MAX_DECODED_TRACE as u64)?;
            let receipt = trace
                .receipt
                .as_ref()
                .ok_or("Downloaded trace has no source receipt")?;
            require(
                report.attempts.contains(receipt)
                    && receipt.url == trace_url(&report.plan, &trace.icao24)
                    && receipt.error.is_none(),
                "Decoded trace is not bound to a successful source response",
            )?;
            let raw = verified_artifact(root, &receipt.body, TRACE_BYTE_LIMIT)?;
            require(
                decode_payload(&raw, MAX_DECODED_TRACE)? == bytes,
                "Decoded trace differs from preserved HTTP response",
            )?;
            validate_trace(&bytes, &trace.icao24, &report.plan)?;
            let text = String::from_utf8(bytes).map_err(|_| "Trace JSON is not UTF-8")?;
            reports.push(adsblol::normalize_trace(&text, config)?);
        }
    }
    require(
        !reports.is_empty(),
        "No downloaded full traces available for native import",
    )?;
    require(
        !cancel.load(Ordering::Relaxed),
        "Historical trace import cancelled",
    )?;
    let mut result = adsblol::merge_reports(reports, config)?;
    result.quality_notes.push(format!("Regional ADSB.lol replay acquisition status {:?}; {} non-ICAO and {} trace-cap omissions; coverage_complete=false. Snapshot {} SHA-256 {}.",report.status,report.non_icao_omissions,report.trace_cap_omissions,report.report_path.display(),hash(&json_bytes(report)?)));
    result.quality_notes.extend(report.limitations.clone());
    Ok(result)
}
