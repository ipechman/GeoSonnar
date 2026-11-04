//! Bounded, cached downloads from the public adsb.aero historical-flight API.
//! These simplified tracks are useful for a reference-area check; they are not
//! the raw ADS-B messages or the creator's original Flightradar24 dataset.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const BASE_URL: &str = "https://adsb.aero/api/v1";
const SCHEMA_URL: &str = "https://adsb.aero/api/openapi.json";
const PAGE_SIZE: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AcquisitionRequest {
    /// One UTC calendar date, YYYY-MM-DD. Local dates must be converted first.
    pub date_utc: String,
    /// Aircraft-coverage bounds: [south, west, north, east], degrees WGS84.
    /// Pad the observer search area by the desired acoustic range beforehand.
    pub bounds: [f64; 4],
    pub max_flights: usize,
    /// An explicit refresh creates a new run and preserves the previous files.
    #[serde(default)]
    pub refresh: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcquisitionReport {
    pub provider: String,
    pub source_url: String,
    pub request: AcquisitionRequest,
    pub fetched_at_unix_s: u64,
    pub observed_data_range: Value,
    pub cached: bool,
    /// True only if this bounded API query exhausted its cursor. It does not
    /// imply complete real-world ADS-B receiver coverage.
    pub complete: bool,
    pub flight_count: usize,
    pub point_count: usize,
    /// Paths are relative to the caller's output directory, for portability.
    pub flights_file: String,
    pub raw_pages: Vec<String>,
    /// Integrity against later local changes; these are not provider signatures.
    /// Older caches without hashes must be explicitly migrated or refreshed.
    #[serde(default)]
    pub flights_sha256: String,
    #[serde(default)]
    pub raw_page_sha256: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

/// Download the selected date and aircraft-coverage region. Requires Windows'
/// bundled curl.exe and certutil.exe (curl and sha256sum on other systems);
/// no Python, account, or paid API.
/// Every request and server response is retained. Existing matching results
/// are reused; a different request needs a different output directory or an
/// explicit refresh. Each response is capped at 32 MiB and 120 seconds.
pub fn fetch_adsb_aero(
    request: &AcquisitionRequest,
    output_dir: &Path,
) -> Result<AcquisitionReport, String> {
    validate_request(request)?;
    fs::create_dir_all(output_dir).map_err(|e| format!("Cannot create download directory: {e}"))?;
    let report_path = output_dir.join("acquisition.json");
    if report_path.exists() && !request.refresh {
        let mut report: AcquisitionReport = serde_json::from_slice(
            &fs::read(&report_path).map_err(|e| format!("Cannot read cached report: {e}"))?,
        )
        .map_err(|e| format!("Invalid cached acquisition report: {e}"))?;
        let mut stored = report.request.clone();
        stored.refresh = false;
        let mut incoming = request.clone();
        incoming.refresh = false;
        if stored != incoming {
            return Err("The download directory contains a different date, area, or limit. Choose another directory or explicitly request refresh.".into());
        }
        verify_cache(output_dir, &report)?;
        report.cached = true;
        return Ok(report);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    let run_name = format!("run-{}", now.as_nanos());
    let run_dir = output_dir.join(&run_name);
    fs::create_dir(&run_dir).map_err(|e| format!("Cannot create download run: {e}"))?;
    write_json(&run_dir.join("request.json"), request)?;

    let range_bytes = curl_json(&format!("{BASE_URL}/data-range"), None)?;
    fs::write(run_dir.join("data-range.json"), &range_bytes).map_err(|e| e.to_string())?;
    let range: Value = serde_json::from_slice(&range_bytes)
        .map_err(|e| format!("Invalid archive date response: {e}"))?;
    let first = range["first_date"]
        .as_str()
        .ok_or("Archive did not report an earliest date")?;
    let last = range["last_date"]
        .as_str()
        .ok_or("Archive did not report a latest date")?;
    parse_date(first)?;
    parse_date(last)?;
    if request.date_utc.as_str() < first || request.date_utc.as_str() > last {
        return Err(format!(
            "Requested UTC date {} is outside this archive's observed range {first} through {last}.",
            request.date_utc
        ));
    }
    let schema = curl_json(SCHEMA_URL, None)?;
    let _: Value =
        serde_json::from_slice(&schema).map_err(|e| format!("Invalid provider schema: {e}"))?;
    fs::write(run_dir.join("openapi.json"), schema).map_err(|e| e.to_string())?;

    let mut flights = Vec::new();
    let mut ids = HashSet::new();
    let mut cursor: Option<String> = None;
    let mut seen_cursors = HashSet::new();
    let mut raw_pages = Vec::new();
    let mut complete = false;
    let mut duplicate_count = 0usize;
    // Additional pages accommodate empty time windows, but cannot loop forever.
    let max_pages = request.max_flights.div_ceil(PAGE_SIZE) + 12;
    for page_index in 0..max_pages {
        let remaining = request.max_flights - flights.len();
        if remaining == 0 {
            break;
        }
        let body = query_payload(request, cursor.as_deref(), PAGE_SIZE.min(remaining))?;
        let prefix = format!("page-{:04}", page_index + 1);
        write_json(&run_dir.join(format!("{prefix}-request.json")), &body)?;
        eprintln!(
            "Downloading flight page {} ({} flights retained)...",
            page_index + 1,
            flights.len()
        );
        let bytes = curl_json(&format!("{BASE_URL}/query"), Some(&body))?;
        fs::write(run_dir.join(format!("{prefix}-response.json")), &bytes)
            .map_err(|e| e.to_string())?;
        raw_pages.push(format!("{run_name}/{prefix}-response.json"));
        let page: Value = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid flight page {}: {e}", page_index + 1))?;
        let entries = page["flights"]
            .as_array()
            .ok_or("API response is missing the flights array")?;
        if entries.len() > remaining || entries.len() > PAGE_SIZE {
            return Err("Provider returned more flights than the requested bound; raw response retained for inspection.".into());
        }
        for entry in entries {
            let id = entry["flight_id"]
                .as_str()
                .ok_or("Provider returned a flight without a flight_id")?;
            if ids.insert(id.to_owned()) {
                flights.push(entry.clone());
            } else {
                duplicate_count += 1;
            }
        }
        cursor = match page.get("cursor") {
            Some(Value::Null) => None,
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => return Err("Provider response has no valid continuation cursor".into()),
        };
        if let Some(ref token) = cursor {
            if !seen_cursors.insert(token.clone()) {
                return Err("Provider repeated a continuation cursor; stopped to avoid an unbounded download.".into());
            }
        } else {
            complete = true;
            break;
        }
    }
    let point_count = flights
        .iter()
        .filter_map(|f| f["point_count"].as_u64())
        .sum::<u64>() as usize;
    let flights_file = format!("{run_name}/flights.json");
    write_json(&output_dir.join(&flights_file), &json!({"flights":flights}))?;
    let flights_sha256 = sha256_file(&output_dir.join(&flights_file))?;
    let raw_page_sha256 = raw_pages
        .iter()
        .map(|p| Ok((p.clone(), sha256_file(&output_dir.join(p))?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let mut warnings = vec![
        "These are simplified adsb.aero trajectories, not the original video's Flightradar24 records or raw ADS-B messages.".into(),
        "Attribution: adsb.aero derives trajectory data from the adsb.lol archive (ODbL), with reference data from OpenAIP. Provider terms: https://adsb.aero/llms.txt .".into(),
        "Provider documents 50 m horizontal and 100 ft altitude simplification. Ground speed, track and vertical-rate series are simplified separately. Numerical derivatives require care.".into(),
        "Coordinates contain pressure altitude in feet; apply supplied QNH correction explicitly and record the vertical datum. No geometric altitude is provided by this endpoint.".into(),
        "Receiver gaps, unknown aircraft types, simplification, and archive coverage can omit relevant evidence. An exhausted query does not establish a complete sky or validate a silence filter.".into(),
        "The date is UTC. The query includes flights starting up to seven days earlier whose paths intersect the requested region on that day. Bounds describe aircraft coverage, not the observer grid.".into(),
    ];
    if !complete {
        warnings.push("Download stopped at the configured flight/page limit. This is a partial candidate dataset; increase the explicit limit and refresh to complete it.".into());
    }
    if duplicate_count > 0 {
        warnings.push(format!("Removed {duplicate_count} duplicate flight IDs across API pages; original responses are retained."));
    }
    let report = AcquisitionReport {
        provider: "adsb.aero".into(),
        source_url: format!("{BASE_URL}/query"),
        request: request.clone(),
        fetched_at_unix_s: now.as_secs(),
        observed_data_range: range,
        cached: false,
        complete,
        flight_count: flights.len(),
        point_count,
        flights_file,
        raw_pages,
        flights_sha256,
        raw_page_sha256,
        warnings,
    };
    write_json(&run_dir.join("acquisition.json"), &report)?;
    // Publish the current pointer only after the full local result exists.
    write_json(&report_path, &report)?;
    Ok(report)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}

fn cache_file(output_dir: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("Cache paths must be relative files inside the download directory".into());
    }
    let root = output_dir.canonicalize().map_err(|e| e.to_string())?;
    let file = root.join(path).canonicalize().map_err(|e| {
        format!(
            "Cached file {relative} is missing or inaccessible: {e}. Refresh or restore the cache."
        )
    })?;
    if !file.starts_with(&root) || !file.is_file() {
        return Err(format!(
            "Cached file {relative} is outside the download directory or is not a file"
        ));
    }
    Ok(file)
}

fn verify_cache(output_dir: &Path, report: &AcquisitionReport) -> Result<(), String> {
    if !valid_sha256(&report.flights_sha256)
        || report.raw_pages.is_empty()
        || report.raw_page_sha256.len() != report.raw_pages.len()
        || report.raw_pages.iter().any(|p| {
            report
                .raw_page_sha256
                .get(p)
                .is_none_or(|hash| !valid_sha256(hash))
        })
    {
        return Err("Cached data has no complete SHA256 integrity record. Explicitly migrate the old cache after checking its raw-page/aggregate consistency, or request refresh; it has not been verified.".into());
    }
    let mut files = vec![(&report.flights_file, &report.flights_sha256)];
    files.extend(
        report
            .raw_pages
            .iter()
            .map(|p| (p, &report.raw_page_sha256[p])),
    );
    for (relative, expected) in files {
        let path = cache_file(output_dir, relative)?;
        if !sha256_file(&path)?.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "Cached file {relative} failed SHA256 integrity verification. Restore the original file or explicitly refresh; cached counts and completeness are not trusted."
            ));
        }
    }
    Ok(())
}

/// Uses an OS-bundled hashing utility without a command shell or dependencies.
pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    let absolute = path
        .canonicalize()
        .map_err(|e| format!("Cannot hash {}: {e}", path.display()))?;
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("certutil.exe");
        c.arg("-hashfile").arg(&absolute).arg("SHA256");
        c.creation_flags(0x08000000);
        c
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut c = Command::new("sha256sum");
        c.arg("--").arg(&absolute);
        c
    };
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Cannot start the OS SHA256 utility: {e}"))?;
    if !output.status.success() {
        return Err(format!("OS SHA256 utility failed for {}", path.display()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    #[cfg(windows)]
    let digest = stdout
        .lines()
        .map(|line| line.split_whitespace().collect::<String>())
        .find(|line| valid_sha256(line));
    #[cfg(not(windows))]
    let digest = stdout
        .split_whitespace()
        .next()
        .filter(|s| valid_sha256(s))
        .map(str::to_owned);
    digest
        .map(|s| s.to_ascii_lowercase())
        .ok_or_else(|| "OS SHA256 utility did not return an identifiable digest".into())
}

/// Build a provider request without network access, useful for reviewing a job.
pub fn query_payload(
    request: &AcquisitionRequest,
    cursor: Option<&str>,
    limit: usize,
) -> Result<Value, String> {
    validate_request(request)?;
    if !(1..=PAGE_SIZE).contains(&limit) {
        return Err("Page limit must be between 1 and 100".into());
    }
    let day = parse_date(&request.date_utc)?;
    let tomorrow = date_string(add_days(day, 1));
    let earliest_start = date_string(add_days(day, -7));
    let [south, west, north, east] = request.bounds;
    let mut payload = json!({
        "end_date":format!("{tomorrow}T00:00:00Z"),
        "start_from":format!("{earliest_start}T00:00:00Z"),
        "window_days":7,
        "match":{"trajectory_intersects":{
            "geometry":{"type":"Polygon","coordinates":[[[west,south],[east,south],[east,north],[west,north],[west,south]]]},
            "time_from":format!("{}T00:00:00Z",request.date_utc),
            "time_to":format!("{tomorrow}T00:00:00Z")
        }},
        "limit":limit,"include_path":true
    });
    if let Some(c) = cursor {
        payload["cursor"] = Value::String(c.into());
    }
    Ok(payload)
}

fn validate_request(request: &AcquisitionRequest) -> Result<(), String> {
    parse_date(&request.date_utc)?;
    let [south, west, north, east] = request.bounds;
    if !request.bounds.iter().all(|x| x.is_finite())
        || south < -90.0
        || north > 90.0
        || west < -180.0
        || east > 180.0
        || south >= north
        || west >= east
    {
        return Err("Bounds must be finite [south,west,north,east] within WGS84 latitude/longitude and have positive extent. Split an antimeridian-crossing region into two requests.".into());
    }
    if !(1..=20_000).contains(&request.max_flights) {
        return Err("max_flights must be between 1 and 20000; partition larger searches into explicit regional jobs.".into());
    }
    Ok(())
}

fn curl_json(url: &str, body: Option<&Value>) -> Result<Vec<u8>, String> {
    let curl = if cfg!(windows) { "curl.exe" } else { "curl" };
    let mut command = Command::new(curl);
    command.args([
        "--fail-with-body",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https",
        "--connect-timeout",
        "20",
        "--max-time",
        "120",
        "--max-filesize",
        "33554432",
        "--user-agent",
        "GeoSonnar/0.1 (research replication)",
    ]);
    if body.is_some() {
        command.args([
            "--header",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
        ]);
    }
    command
        .arg(url)
        .stdin(if body.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot start {curl}; Windows curl is required for downloads: {e}"))?;
    if let Some(payload) = body {
        let bytes = serde_json::to_vec(payload).map_err(|e| e.to_string())?;
        if let Err(error) = child
            .stdin
            .take()
            .ok_or("Cannot open downloader input")?
            .write_all(&bytes)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Cannot send flight query: {error}"));
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("Flight download failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let response = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "Public flight archive request failed ({url}): {} {}",
            stderr.trim(),
            response.chars().take(800).collect::<String>()
        ));
    }
    if output.stdout.is_empty() {
        return Err("Public flight archive returned an empty response".into());
    }
    Ok(output.stdout)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    fs::write(path, bytes).map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

pub(crate) fn parse_date(value: &str) -> Result<(i32, u32, u32), String> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return Err("Date must use YYYY-MM-DD".into());
    }
    let year = value[..4].parse::<i32>().map_err(|e| e.to_string())?;
    let month = value[5..7].parse::<u32>().map_err(|e| e.to_string())?;
    let day = value[8..].parse::<u32>().map_err(|e| e.to_string())?;
    if !(1900..=2200).contains(&year)
        || !(1..=12).contains(&month)
        || day == 0
        || day > month_days(year, month)
    {
        return Err("Invalid calendar date (supported years 1900-2200)".into());
    }
    Ok((year, month, day))
}

fn month_days(y: i32, m: u32) -> u32 {
    match m {
        4 | 6 | 9 | 11 => 30,
        2 => {
            if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 31,
    }
}
fn add_days(mut date: (i32, u32, u32), days: i32) -> (i32, u32, u32) {
    for _ in 0..days.unsigned_abs() {
        if days > 0 {
            date.2 += 1;
            if date.2 > month_days(date.0, date.1) {
                date.2 = 1;
                date.1 += 1;
                if date.1 > 12 {
                    date.1 = 1;
                    date.0 += 1;
                }
            }
        } else if date.2 > 1 {
            date.2 -= 1;
        } else {
            if date.1 > 1 {
                date.1 -= 1;
            } else {
                date.1 = 12;
                date.0 -= 1;
            }
            date.2 = month_days(date.0, date.1);
        }
    }
    date
}
fn date_string((y, m, d): (i32, u32, u32)) -> String {
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn req(date: &str) -> AcquisitionRequest {
        AcquisitionRequest {
            date_utc: date.into(),
            bounds: [40.65, -73.8, 41.1, -73.25],
            max_flights: 1000,
            refresh: false,
        }
    }
    #[test]
    fn real_calendar_dates_and_year_boundaries() {
        assert!(parse_date("2026-02-29").is_err());
        assert!(parse_date("2024-02-29").is_ok());
        assert_eq!(date_string(add_days((2024, 2, 28), 1)), "2024-02-29");
        assert_eq!(date_string(add_days((2026, 1, 1), -7)), "2025-12-25");
        assert_eq!(date_string(add_days((2026, 12, 31), 1)), "2027-01-01");
    }
    #[test]
    fn temporal_query_keeps_cross_midnight_flights_and_bounds_coordinates() {
        let p = query_payload(&req("2026-09-08"), Some("opaque"), 100).unwrap();
        assert_eq!(p["start_from"], "2026-09-01T00:00:00Z");
        assert_eq!(p["end_date"], "2026-09-09T00:00:00Z");
        assert_eq!(
            p["match"]["trajectory_intersects"]["time_from"],
            "2026-09-08T00:00:00Z"
        );
        assert_eq!(
            p["match"]["trajectory_intersects"]["geometry"]["coordinates"][0][0],
            json!([-73.8, 40.65])
        );
        assert_eq!(p["cursor"], "opaque");
    }
    #[test]
    fn rejects_invalid_regions_and_unbounded_downloads() {
        let mut r = req("2026-09-08");
        r.bounds = [40.0, 179.0, 41.0, -179.0];
        assert!(validate_request(&r).is_err());
        r.bounds = [40.0, -74.0, 41.0, -73.0];
        r.max_flights = 0;
        assert!(validate_request(&r).is_err());
        r.max_flights = 20_001;
        assert!(validate_request(&r).is_err());
    }

    #[test]
    fn cached_content_changes_and_unsigned_legacy_reports_are_rejected() {
        let directory = std::env::temp_dir().join(format!(
            "geosonnar-cache-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let flights = directory.join("flights.json");
        let page = directory.join("page.json");
        fs::write(&flights, b"abc").unwrap();
        fs::write(&page, br#"{"flights":[],"cursor":null}"#).unwrap();
        let digest = sha256_file(&flights).unwrap();
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let request = req("2026-09-08");
        let mut report = AcquisitionReport {
            provider: "test".into(),
            source_url: "local fixture".into(),
            request: request.clone(),
            fetched_at_unix_s: 0,
            observed_data_range: Value::Null,
            cached: false,
            complete: true,
            flight_count: 0,
            point_count: 0,
            flights_file: "flights.json".into(),
            raw_pages: vec!["page.json".into()],
            flights_sha256: digest,
            raw_page_sha256: BTreeMap::from([("page.json".into(), sha256_file(&page).unwrap())]),
            warnings: vec![],
        };
        let manifest = directory.join("acquisition.json");
        write_json(&manifest, &report).unwrap();
        assert!(fetch_adsb_aero(&request, &directory).unwrap().cached);
        // Same-size replacement must fail, not merely a missing-file check.
        fs::write(&flights, b"xyz").unwrap();
        assert!(
            fetch_adsb_aero(&request, &directory)
                .unwrap_err()
                .contains("failed SHA256")
        );
        fs::write(&flights, b"abc").unwrap();
        fs::write(&page, br#"{"flights":[],"cursor":"changed"}"#).unwrap();
        assert!(
            fetch_adsb_aero(&request, &directory)
                .unwrap_err()
                .contains("failed SHA256")
        );
        report.flights_sha256.clear();
        report.raw_page_sha256.clear();
        write_json(&manifest, &report).unwrap();
        assert!(
            fetch_adsb_aero(&request, &directory)
                .unwrap_err()
                .contains("Explicitly migrate")
        );
        assert!(cache_file(&directory, "../outside.json").is_err());
        fs::remove_file(flights).unwrap();
        fs::remove_file(page).unwrap();
        fs::remove_file(manifest).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
