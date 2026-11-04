//! Official ADSB.lol daily archives: explicit plan, bounded resumable download,
//! and native SHA-256 verification. Region/time filtering happens at import.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

const CHUNK_BYTES: u64 = 32 * 1024 * 1024;
const METADATA_LIMIT: u64 = 2 * 1024 * 1024;
fn default_budget() -> u64 {
    6_000_000_000
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AcquisitionRequest {
    pub date_utc: String,
    #[serde(default)]
    pub instance: Option<String>,
    #[serde(default = "default_budget")]
    pub max_download_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArchiveAsset {
    pub name: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetadataReceipt {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadPlan {
    pub schema_version: u32,
    pub provider: String,
    pub license: String,
    pub request: AcquisitionRequest,
    pub date_utc: String,
    pub instance: String,
    pub release_tag: String,
    pub release_url: String,
    pub assets: Vec<ArchiveAsset>,
    pub planned_bytes: u64,
    pub chunk_bytes: u64,
    pub metadata: Vec<MetadataReceipt>,
    pub coverage_complete: bool,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArchivePart {
    /// Relative to archive-manifest.json, so the cached case can be moved.
    pub path: PathBuf,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveManifest {
    pub schema_version: u32,
    pub provider: String,
    pub license: String,
    pub date_utc: String,
    pub release_tag: String,
    pub plan_sha256: String,
    pub parts: Vec<ArchivePart>,
    /// All planned archive bytes verified, not all real-world aircraft covered.
    pub complete: bool,
    pub full_sha_verified: bool,
    pub coverage_complete: bool,
}

fn require(ok: bool, message: impl Into<String>) -> Result<(), String> {
    if ok { Ok(()) } else { Err(message.into()) }
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}

fn date_parts(value: &str) -> Result<(u32, String), String> {
    let bytes = value.as_bytes();
    require(
        bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit()),
        "Use a UTC YYYY-MM-DD date",
    )?;
    let year: u32 = value[..4].parse().map_err(|_| "Invalid year")?;
    let month: usize = value[5..7].parse().map_err(|_| "Invalid month")?;
    let day: u32 = value[8..].parse().map_err(|_| "Invalid day")?;
    require(
        (2023..=2200).contains(&year) && (1..=12).contains(&month),
        "ADSB.lol archive date must be 2023 or later with a valid month",
    )?;
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
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
    require(
        day > 0 && day <= days[month - 1],
        "Invalid UTC calendar date",
    )?;
    Ok((year, value.replace('-', ".")))
}

fn validate_request(request: &AcquisitionRequest) -> Result<(), String> {
    date_parts(&request.date_utc)?;
    require(
        request.max_download_bytes > 0 && request.max_download_bytes <= 1_000_000_000_000,
        "Download budget must be positive and no greater than 1 TB",
    )?;
    if let Some(instance) = &request.instance {
        require(
            ["preferred", "prod-0", "prod-1", "staging-0"].contains(&instance.as_str()),
            "Instance must be preferred, prod-0, prod-1 or staging-0",
        )?;
    }
    Ok(())
}

/// Resolve the provider-maintained largest-archive preference, without guessing
/// a winner or interpreting file size as complete receiver coverage.
pub fn resolve_instance(request: &AcquisitionRequest, preferred: &str) -> Result<String, String> {
    validate_request(request)?;
    if let Some(instance) = &request.instance
        && instance != "preferred"
    {
        return Ok(instance.clone());
    }
    let (year, date) = date_parts(&request.date_utc)?;
    let prefix = format!(
        "https://github.com/adsblol/globe_history_{year}/releases/download/v{date}-planes-readsb-"
    );
    let mut found = Vec::new();
    for line in preferred
        .lines()
        .filter(|line| line.contains(&format!("v{date}-planes-readsb-")))
    {
        let urls: Vec<_> = line.trim().split(',').collect();
        require(!urls.is_empty(), "Empty preferred archive entry")?;
        let tail = urls[0]
            .strip_prefix(&prefix)
            .ok_or("Preferred archive URL is outside the expected official date/repository")?;
        let (instance, _) = tail
            .split_once('/')
            .ok_or("Malformed preferred archive URL")?;
        require(
            ["prod-0", "prod-1", "staging-0"].contains(&instance),
            "Unknown preferred archive instance",
        )?;
        let stem = format!("{prefix}{instance}/v{date}-planes-readsb-{instance}.tar");
        require(
            urls.iter().all(|url| {
                url.starts_with(&stem)
                    && (url.len() == stem.len()
                        || (url.len() == stem.len() + 3 && url[stem.len()..].starts_with('.')))
            }),
            "Preferred archive lists a different date, instance or unexpected URL",
        )?;
        found.push(instance.to_owned());
    }
    require(
        found.len() == 1,
        "The provider's preferred list has no unique entry for this date; choose a documented available instance",
    )?;
    Ok(found.remove(0))
}

fn validate_preferred_assets(plan: &DownloadPlan, preferred: &str) -> Result<(), String> {
    require(
        resolve_instance(&plan.request, preferred)? == plan.instance,
        "Preferred instance differs from the original provider selection",
    )?;
    let (_, date) = date_parts(&plan.date_utc)?;
    let line = preferred
        .lines()
        .find(|line| line.contains(&format!("v{date}-planes-readsb-")))
        .ok_or("Preferred archive entry is missing")?;
    let mut preferred_urls: Vec<_> = line.trim().split(',').collect();
    let mut release_urls: Vec<_> = plan.assets.iter().map(|asset| asset.url.as_str()).collect();
    preferred_urls.sort_unstable();
    release_urls.sort_unstable();
    require(
        preferred_urls == release_urls,
        "Preferred archive part list differs from the release assets; a part may be missing",
    )
}

/// Pure, offline-testable release parsing. Metadata must come from the official
/// date-tag endpoint; every returned download URL is reconstructed and checked.
pub fn parse_release_plan(
    request: &AcquisitionRequest,
    instance: &str,
    release: &Value,
) -> Result<DownloadPlan, String> {
    validate_request(request)?;
    require(
        ["prod-0", "prod-1", "staging-0"].contains(&instance),
        "Unknown archive instance",
    )?;
    if let Some(wanted) = &request.instance {
        require(
            wanted == "preferred" || wanted == instance,
            "Resolved instance differs from the request",
        )?;
    }
    let (year, date) = date_parts(&request.date_utc)?;
    let tag = format!("v{date}-planes-readsb-{instance}");
    let repo = format!("https://github.com/adsblol/globe_history_{year}");
    require(
        release["tag_name"].as_str() == Some(&tag)
            && release["draft"].as_bool() == Some(false)
            && release["prerelease"].as_bool() == Some(false),
        "Release is unavailable, draft, prerelease or for a different date/instance",
    )?;
    let release_url = format!("{repo}/releases/tag/{tag}");
    require(
        release["html_url"].as_str() == Some(&release_url),
        "Release metadata has a foreign URL",
    )?;
    let rows = release["assets"]
        .as_array()
        .ok_or("Release assets are missing")?;
    let stem = format!("{tag}.tar");
    let mut assets = Vec::new();
    for row in rows {
        let name = row["name"].as_str().ok_or("Asset name is missing")?;
        if !name.contains(".tar") {
            continue;
        }
        let suffix = name
            .strip_prefix(&stem)
            .ok_or("Archive asset name does not match the requested release")?;
        require(
            suffix.is_empty()
                || (suffix.len() == 3
                    && suffix.starts_with('.')
                    && suffix.as_bytes()[1..].iter().all(u8::is_ascii_lowercase)),
            "Unsupported or unsafe archive part name",
        )?;
        let bytes = row["size"]
            .as_u64()
            .ok_or("Archive asset size is missing")?;
        require(
            bytes > 0 && bytes <= 2_000_000_000,
            "Unexpected archive asset size",
        )?;
        let hash = row["digest"]
            .as_str()
            .and_then(|s| s.strip_prefix("sha256:"))
            .ok_or("Provider SHA-256 digest is missing; cannot verify this release")?;
        require(valid_hash(hash), "Invalid provider SHA-256")?;
        require(
            row["state"].as_str() == Some("uploaded"),
            "Archive asset is not completely uploaded",
        )?;
        let url = format!("{repo}/releases/download/{tag}/{name}");
        require(
            row["browser_download_url"].as_str() == Some(&url),
            "Archive asset download URL is outside the exact official release",
        )?;
        assets.push(ArchiveAsset {
            name: name.into(),
            url,
            bytes,
            sha256: hash.to_ascii_lowercase(),
        });
    }
    assets.sort_by(|a, b| a.name.cmp(&b.name));
    require(
        !assets.is_empty() && assets.len() <= 32,
        "Release has no archives or exceeds the part limit",
    )?;
    if assets.len() == 1 && assets[0].name == stem {
        // An unsplit daily tar is allowed.
    } else {
        for (index, asset) in assets.iter().enumerate() {
            let suffix = format!(
                ".{}{}",
                (b'a' + (index / 26) as u8) as char,
                (b'a' + (index % 26) as u8) as char
            );
            require(
                asset.name == format!("{stem}{suffix}"),
                "Archive parts must be unique and contiguous from .aa; a part is missing",
            )?;
            require(
                index + 1 == assets.len() || asset.bytes == 2_000_000_000,
                "A non-final split archive part has an unexpected length",
            )?;
        }
    }
    let total = assets
        .iter()
        .try_fold(0u64, |sum, a| sum.checked_add(a.bytes))
        .ok_or("Archive size overflow")?;
    require(
        total <= request.max_download_bytes,
        format!(
            "Archive requires {total} bytes, above the explicit {} byte download budget",
            request.max_download_bytes
        ),
    )?;
    Ok(DownloadPlan { schema_version:1, provider:"ADSB.lol".into(), license:"ODbL-1.0".into(), request:request.clone(),
        date_utc:request.date_utc.clone(), instance:instance.into(), release_tag:tag, release_url,
        assets, planned_bytes:total, chunk_bytes:CHUNK_BYTES, metadata:Vec::new(), coverage_complete:false,
        notes:vec!["One global daily archive is downloaded; region/time filtering happens locally during native import.".into(),
                   "Provider preference selects the largest archive for the date; size does not prove complete receiver coverage.".into(),
                   "These are retained readsb traces, not raw radio messages. Preserve gaps, source types and altitude datums.".into()] })
}

fn write_new(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_reader(File::open(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("Invalid {}: {e}", path.display()))
}

fn curl(args: &[String]) -> Result<(), String> {
    let mut command = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    command
        .args([
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-redirs",
            "5",
            "--location",
            "--fail",
            "--silent",
            "--show-error",
            "--connect-timeout",
            "15",
        ])
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let result = command
        .output()
        .map_err(|e| format!("Cannot start curl: {e}"))?;
    require(
        result.status.success(),
        format!(
            "Download failed; partial work was preserved: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ),
    )
}

fn metadata_download(url: &str, body: &Path, headers: &Path) -> Result<(), String> {
    require(
        !body.exists() && !headers.exists(),
        "Metadata output already exists",
    )?;
    curl(&[
        "--max-time".into(),
        "60".into(),
        "--max-filesize".into(),
        METADATA_LIMIT.to_string(),
        "--dump-header".into(),
        headers.to_string_lossy().into_owned(),
        "--output".into(),
        body.to_string_lossy().into_owned(),
        url.into(),
    ])?;
    require(
        fs::metadata(body).map_err(|e| e.to_string())?.len() <= METADATA_LIMIT,
        "Metadata exceeds the byte limit",
    )
}

fn metadata_receipt(path: &Path, base: &Path) -> Result<MetadataReceipt, String> {
    Ok(MetadataReceipt {
        path: path
            .strip_prefix(base)
            .map_err(|e| e.to_string())?
            .to_owned(),
        bytes: fs::metadata(path).map_err(|e| e.to_string())?.len(),
        sha256: sha256_file(path)?,
    })
}

pub fn plan(request: &AcquisitionRequest, output_dir: &Path) -> Result<DownloadPlan, String> {
    validate_request(request)?;
    require(
        !output_dir.exists(),
        "Plan output already exists; use a fresh directory",
    )?;
    fs::create_dir_all(output_dir).map_err(|e| e.to_string())?;
    write_new(&output_dir.join("request.json"), request)?;
    let (year, date) = date_parts(&request.date_utc)?;
    let mut metadata = vec![metadata_receipt(
        &output_dir.join("request.json"),
        output_dir,
    )?];
    let preferred = if request.instance.as_deref().is_none_or(|s| s == "preferred") {
        let body = output_dir.join("preferred-releases.txt");
        let headers = output_dir.join("preferred-headers.txt");
        metadata_download(
            &format!(
                "https://raw.githubusercontent.com/adsblol/globe_history_{year}/main/PREFERRED_RELEASES.txt"
            ),
            &body,
            &headers,
        )?;
        metadata.push(metadata_receipt(&body, output_dir)?);
        metadata.push(metadata_receipt(&headers, output_dir)?);
        fs::read_to_string(body).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    let instance = resolve_instance(request, &preferred)?;
    let release_path = output_dir.join("release.json");
    let headers = output_dir.join("release-headers.txt");
    metadata_download(
        &format!(
            "https://api.github.com/repos/adsblol/globe_history_{year}/releases/tags/v{date}-planes-readsb-{instance}"
        ),
        &release_path,
        &headers,
    )?;
    metadata.push(metadata_receipt(&release_path, output_dir)?);
    metadata.push(metadata_receipt(&headers, output_dir)?);
    let mut result = parse_release_plan(request, &instance, &read_json::<Value>(&release_path)?)?;
    if request.instance.as_deref().is_none_or(|s| s == "preferred") {
        validate_preferred_assets(&result, &preferred)?;
    }
    result.metadata = metadata;
    write_new(&output_dir.join("download-plan.json"), &result)?;
    Ok(result)
}

pub fn verify_asset(path: &Path, asset: &ArchiveAsset) -> Result<(), String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("Cannot read cached asset {}: {e}", path.display()))?;
    require(
        meta.is_file() && !meta.file_type().is_symlink(),
        "Cached archive asset must be a regular file",
    )?;
    require(
        meta.len() == asset.bytes,
        "Cached archive length differs from the provider manifest",
    )?;
    require(
        sha256_file(path)? == asset.sha256,
        "Cached archive SHA-256 differs from the provider manifest",
    )
}

/// Parse the last response in a curl redirect chain and reject ignored/mismatched
/// ranges before any bytes are appended to the resumable archive.
pub fn validate_range_response(
    headers: &str,
    start: u64,
    end: u64,
    total: u64,
    bytes: u64,
) -> Result<(), String> {
    require(start <= end && end < total, "Invalid planned byte range")?;
    let mut status = None;
    let mut range = None;
    let mut length = None;
    for line in headers.lines() {
        if line.starts_with("HTTP/") {
            status = line
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.parse::<u16>().ok());
            range = None;
            length = None;
        } else if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("content-range") {
                range = Some(value.trim().to_owned());
            }
            if key.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<u64>().ok();
            }
        }
    }
    let expected = end - start + 1;
    require(
        status == Some(206)
            && range.as_deref() == Some(&format!("bytes {start}-{end}/{total}"))
            && bytes == expected
            && length.is_none_or(|n| n == expected),
        "Server did not return the exact requested partial archive range",
    )
}

fn validate_plan(plan: &DownloadPlan, base: &Path) -> Result<(), String> {
    require(
        plan.schema_version == 1
            && plan.provider == "ADSB.lol"
            && plan.license == "ODbL-1.0"
            && !plan.coverage_complete
            && plan.chunk_bytes == CHUNK_BYTES,
        "Unsupported or altered archive plan",
    )?;
    require(
        !plan.metadata.is_empty(),
        "Plan is missing its raw provider metadata receipts",
    )?;
    for item in &plan.metadata {
        require(
            item.path
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
                && valid_hash(&item.sha256),
            "Unsafe metadata receipt",
        )?;
        let path = base.join(&item.path);
        require(
            fs::metadata(&path).map_err(|e| e.to_string())?.len() == item.bytes
                && sha256_file(&path)? == item.sha256,
            "Plan provider metadata changed; create a new plan",
        )?;
    }
    require(
        plan.metadata
            .iter()
            .any(|r| r.path == Path::new("release.json")),
        "Plan has no bound release metadata",
    )?;
    require(
        plan.metadata
            .iter()
            .any(|r| r.path == Path::new("request.json")),
        "Plan has no bound original request",
    )?;
    require(
        read_json::<AcquisitionRequest>(&base.join("request.json"))? == plan.request,
        "Plan request differs from its original request receipt",
    )?;
    let original = parse_release_plan(
        &plan.request,
        &plan.instance,
        &read_json::<Value>(&base.join("release.json"))?,
    )?;
    require(
        plan.date_utc == original.date_utc
            && plan.release_tag == original.release_tag
            && plan.release_url == original.release_url
            && plan.assets == original.assets
            && plan.planned_bytes == original.planned_bytes,
        "Plan differs from its provider manifest",
    )?;
    if plan
        .request
        .instance
        .as_deref()
        .is_none_or(|s| s == "preferred")
    {
        require(
            plan.metadata
                .iter()
                .any(|r| r.path == Path::new("preferred-releases.txt")),
            "Preferred selection is missing its bound provider list",
        )?;
        validate_preferred_assets(
            plan,
            &fs::read_to_string(base.join("preferred-releases.txt")).map_err(|e| e.to_string())?,
        )?;
    }
    Ok(())
}

fn fetch_asset(asset: &ArchiveAsset, directory: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let final_path = directory.join(&asset.name);
    if final_path.exists() {
        return verify_asset(&final_path, asset);
    }
    let partial = directory.join(format!("{}.part", asset.name));
    let owner = directory.join(format!("{}.part-identity.json", asset.name));
    if owner.exists() {
        require(
            read_json::<ArchiveAsset>(&owner)? == *asset,
            "Partial archive belongs to a different provider asset",
        )?;
    } else {
        require(
            !partial.exists(),
            "Partial archive has no identity receipt; preserve it and use a new cache or explicitly resolve the partial file",
        )?;
        write_new(&owner, asset)?;
    }
    let mut offset = if partial.exists() {
        let meta = fs::symlink_metadata(&partial).map_err(|e| e.to_string())?;
        require(
            meta.is_file() && !meta.file_type().is_symlink(),
            "Partial archive is not a regular file",
        )?;
        meta.len()
    } else {
        0
    };
    require(
        offset <= asset.bytes,
        "Partial archive exceeds expected size",
    )?;
    let chunk_path = directory.join(format!("{}.chunk", asset.name));
    let headers = directory.join(format!("{}.chunk-headers", asset.name));
    while offset < asset.bytes {
        if cancel.load(Ordering::Relaxed) {
            return Err(
                "Download canceled between chunks; verified partial data is retained for resume"
                    .into(),
            );
        }
        let end = offset.saturating_add(CHUNK_BYTES).min(asset.bytes) - 1;
        for path in [&chunk_path, &headers] {
            if path.exists() {
                require(
                    !fs::symlink_metadata(path)
                        .map_err(|e| e.to_string())?
                        .file_type()
                        .is_symlink(),
                    "Download temporary path must not be a symlink",
                )?;
            }
        }
        curl(&[
            "--max-time".into(),
            "120".into(),
            "--retry".into(),
            "2".into(),
            "--retry-delay".into(),
            "2".into(),
            "--max-filesize".into(),
            (end - offset + 1).to_string(),
            "--range".into(),
            format!("{offset}-{end}"),
            "--dump-header".into(),
            headers.to_string_lossy().into_owned(),
            "--output".into(),
            chunk_path.to_string_lossy().into_owned(),
            asset.url.clone(),
        ])?;
        validate_range_response(
            &fs::read_to_string(&headers).map_err(|e| e.to_string())?,
            offset,
            end,
            asset.bytes,
            fs::metadata(&chunk_path).map_err(|e| e.to_string())?.len(),
        )?;
        let mut destination = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&partial)
            .map_err(|e| e.to_string())?;
        require(
            destination.metadata().map_err(|e| e.to_string())?.len() == offset,
            "Partial archive changed during download",
        )?;
        std::io::copy(
            &mut File::open(&chunk_path).map_err(|e| e.to_string())?,
            &mut destination,
        )
        .map_err(|e| e.to_string())?;
        destination.sync_all().map_err(|e| e.to_string())?;
        offset = end + 1;
        eprintln!(
            "ADSB.lol {}: {} / {} bytes",
            asset.name, offset, asset.bytes
        );
    }
    verify_asset(&partial, asset)?;
    require(
        !final_path.exists(),
        "Completed archive path appeared during download; refusing to replace it",
    )?;
    fs::rename(&partial, &final_path).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn fetch(plan_path: &Path, cache_dir: &Path) -> Result<ArchiveManifest, String> {
    fetch_with_cancel(plan_path, cache_dir, &AtomicBool::new(false))
}

/// Cancellation is observed between bounded range chunks. The current curl
/// request/retry finishes first, preserving the existing verified-resume contract.
pub fn fetch_with_cancel(
    plan_path: &Path,
    cache_dir: &Path,
    cancel: &AtomicBool,
) -> Result<ArchiveManifest, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Download canceled before acquisition".into());
    }
    let plan_hash = sha256_file(plan_path)?;
    let plan: DownloadPlan = read_json(plan_path)?;
    let base = plan_path.parent().unwrap_or(Path::new("."));
    validate_plan(&plan, base)?;
    fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let manifest_path = cache_dir.join("archive-manifest.json");
    let parts: Vec<_> = plan
        .assets
        .iter()
        .map(|asset| ArchivePart {
            path: PathBuf::from(&asset.name),
            name: asset.name.clone(),
            bytes: asset.bytes,
            sha256: asset.sha256.clone(),
        })
        .collect();
    if manifest_path.exists() {
        let prior: ArchiveManifest = read_json(&manifest_path)?;
        require(
            prior.schema_version == 1
                && prior.provider == "ADSB.lol"
                && prior.license == "ODbL-1.0"
                && prior.date_utc == plan.date_utc
                && prior.release_tag == plan.release_tag
                && prior.parts == parts
                && valid_hash(&prior.plan_sha256)
                && prior.complete
                && prior.full_sha_verified
                && !prior.coverage_complete,
            "Cache manifest belongs to a different request or is incomplete; use a new cache directory",
        )?;
        for asset in &plan.assets {
            if cancel.load(Ordering::Relaxed) {
                return Err("Download verification canceled".into());
            }
            verify_asset(&cache_dir.join(&asset.name), asset)?;
        }
        require(
            sha256_file(plan_path)? == plan_hash,
            "Download plan changed during cache verification",
        )?;
        validate_plan(&plan, base)?;
        return Ok(prior);
    }
    for asset in &plan.assets {
        if cancel.load(Ordering::Relaxed) {
            return Err("Download canceled; partial data is retained for resume".into());
        }
        fetch_asset(asset, cache_dir, cancel)?;
    }
    require(
        sha256_file(plan_path)? == plan_hash,
        "Download plan changed during acquisition",
    )?;
    validate_plan(&plan, base)?;
    let result = ArchiveManifest {
        schema_version: 1,
        provider: "ADSB.lol".into(),
        license: "ODbL-1.0".into(),
        date_utc: plan.date_utc,
        release_tag: plan.release_tag,
        plan_sha256: plan_hash,
        parts,
        complete: true,
        full_sha_verified: true,
        coverage_complete: false,
    };
    let mut pending = cache_dir.join("archive-manifest.json.pending");
    if pending.exists() {
        // A crash may leave an incomplete receipt here. Preserve that evidence
        // while completing the newly reverified acquisition in a fresh file.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        pending = cache_dir.join(format!(
            "archive-manifest.json.pending.{}.{nonce}",
            std::process::id()
        ));
    }
    write_new(&pending, &result)?;
    require(
        !manifest_path.exists(),
        "Archive manifest appeared during acquisition; refusing to replace it",
    )?;
    fs::rename(&pending, &manifest_path).map_err(|e| e.to_string())?;
    Ok(result)
}
