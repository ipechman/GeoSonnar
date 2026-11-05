//! Explicit bounded HTTPS downloads for authorized external datasets.
use super::{
    model::{CatalogEntry, DataKind, ImportRequest},
    storage::{identity, lock, new_item, now, regular, unchanged_case, validate_bounds, write_new},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// Explicit user action only. URLs are passed directly to curl, without a shell,
/// cookies or curl configuration. Query strings are hashed, not saved in receipts.
pub fn download_url_with_cancel(
    manifest: &Path,
    url: &str,
    request: &ImportRequest,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, String> {
    download_url_as_with_cancel(manifest, url, request, cancel, None)
}

pub(super) fn download_url_as_with_cancel(
    manifest: &Path,
    url: &str,
    request: &ImportRequest,
    cancel: &AtomicBool,
    extension_override: Option<&str>,
) -> Result<CatalogEntry, String> {
    validate_bounds(request.bounds)?;
    if request.kind == DataKind::Adsb {
        return Err("Use the official ADSB.lol acquisition planner for flight archives".into());
    }
    if request.label.trim().is_empty()
        || request.label.len() > 240
        || request.notes.len() > 20_000
        || request.max_bytes == 0
        || request.max_bytes > 1_000_000_000_000
    {
        return Err(
            "Use a label and an explicit download limit of at most 931.3225746155 GiB".into(),
        );
    }
    let rest = url
        .strip_prefix("https://")
        .ok_or("Dataset download requires an HTTPS URL")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty()
        || authority.contains('@')
        || url.len() > 8192
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(
            "Use an HTTPS export URL without embedded user/password credentials or whitespace"
                .into(),
        );
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Download canceled before the request".into());
    }
    let public_url = url.split(['?', '#']).next().unwrap_or("https://");
    let url_hash = format!("{:x}", Sha256::digest(url.as_bytes()));
    let (directory, id, hash) = new_item(manifest)?;
    let _lock = lock(&directory)?;
    let temporary = directory.join("payload.part");
    let curl = if cfg!(windows) {
        PathBuf::from(std::env::var_os("SystemRoot").ok_or("Windows system directory unavailable")?)
            .join("System32/curl.exe")
    } else {
        PathBuf::from("curl")
    };
    let mut command = Command::new(curl);
    command
        .args([
            "-q",
            "--fail",
            "--location",
            "--silent",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "30",
            "--max-time",
            "1800",
            "--speed-limit",
            "1",
            "--speed-time",
            "60",
            "--max-filesize",
            &request.max_bytes.to_string(),
            "--output",
        ])
        .arg(&temporary)
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start the system HTTPS downloader")?;
    let failure = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            break Some("Dataset download canceled");
        }
        if temporary
            .metadata()
            .is_ok_and(|m| m.len() > request.max_bytes)
        {
            let _ = child.kill();
            let _ = child.wait();
            break Some("Dataset download exceeded its size limit");
        }
        match child.try_wait() {
            Ok(Some(status)) => break (!status.success()).then_some(
                "Dataset download failed; check the export URL, access permission and size limit",
            ),
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break Some("Could not observe the dataset download");
            }
        }
    };
    if !temporary.exists() {
        return Err(failure.unwrap_or("Download produced no data").into());
    }
    regular(&temporary, false)?;
    let failure = if temporary.metadata().map_err(|e| e.to_string())?.len() > request.max_bytes {
        Some("Dataset download exceeded its size limit")
    } else {
        failure
    };
    let extension = extension_override
        .or_else(|| {
            Path::new(public_url)
                .extension()
                .and_then(|s| s.to_str())
                .filter(|s| s.len() <= 12 && s.bytes().all(|b| b.is_ascii_alphanumeric()))
        })
        .unwrap_or("bin");
    let filename = if failure.is_none() {
        format!("payload.{extension}")
    } else {
        "payload.part".into()
    };
    if failure.is_none() {
        fs::rename(&temporary, directory.join(&filename)).map_err(|e| e.to_string())?;
    }
    let value = CatalogEntry {
        schema_version: 1,
        id,
        case_manifest_sha256: hash,
        label: request.label.clone(),
        kind: request.kind,
        format: if failure.is_none() {
            "local_file"
        } else {
            "incomplete_download"
        }
        .into(),
        primary_file: filename.clone().into(),
        created_unix_s: now()?.as_secs(),
        source: json!({"type":"authorized_https_export","url_without_query":public_url,"request_url_sha256":url_hash,"query_parameters_redacted":url.contains('?'),"download_error":failure,"provider_coverage_verified":false}),
        files: vec![identity(&directory, &directory.join(filename))?],
        date_utc: request.date_utc.clone(),
        bounds: request.bounds,
        notes: request.notes.clone(),
    };
    unchanged_case(manifest, &value.case_manifest_sha256)?;
    write_new(&directory.join("entry.json"), &value)?;
    if let Some(error) = failure {
        return Err(format!(
            "{error}; incomplete managed copy {} can be deleted in Data",
            value.id
        ));
    }
    Ok(value)
}
