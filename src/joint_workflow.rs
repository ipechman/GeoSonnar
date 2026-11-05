//! Explicit file-bound shared-cell comparisons using saved measured evidence.
use crate::{
    investigation_archive::JointSnapshot,
    investigation_report,
    joint_evidence::{self, JointProgress, JointRequest, JointStatus},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct InputReceipt {
    path: PathBuf,
    bytes: u64,
    sha256: String,
}

fn file_receipt(path: &Path) -> Result<InputReceipt, String> {
    file_receipt_with_cancel(path, None)
}
fn file_receipt_with_cancel(
    path: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<InputReceipt, String> {
    if let Some(cancel) = cancel {
        check_cancel(cancel)?;
    }
    let path = path
        .canonicalize()
        .map_err(|e| format!("Input unavailable: {e}"))?;
    let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err("Input must be a nonempty regular file".into());
    }
    let mut file = fs::File::open(&path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if let Some(cancel) = cancel {
            check_cancel(cancel)?;
        }
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or("Source byte count overflow")?;
        hash.update(&buffer[..read]);
    }
    if bytes != metadata.len() {
        return Err("Input size changed during verification".into());
    }
    Ok(InputReceipt {
        sha256: format!("{:x}", hash.finalize()),
        bytes,
        path,
    })
}
fn read_request(path: &Path) -> Result<(JointRequest, InputReceipt), String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let file = fs::File::open(&path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Request must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(joint_evidence::MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > joint_evidence::MAX_REQUEST_BYTES {
        return Err(format!(
            "Joint request exceeds {}",
            crate::data_size::format_gib(16 * 1024 * 1024)
        ));
    }
    let value = serde_json::from_str(
        std::str::from_utf8(&bytes)
            .map_err(|_| "Request must be UTF-8")?
            .trim_start_matches('\u{feff}'),
    )
    .map_err(|e| format!("Invalid joint request: {e}"))?;
    let receipt = InputReceipt {
        path,
        bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    };
    Ok((value, receipt))
}
fn resolve(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.into()
    } else {
        base.join(path)
    }
}
fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Joint comparison cancelled before source verification".into())
    } else {
        Ok(())
    }
}
fn verify_sources(
    request: &JointRequest,
    base: &Path,
    recording: &Path,
    cancel: &AtomicBool,
) -> Result<Vec<InputReceipt>, String> {
    check_cancel(cancel)?;
    let media = file_receipt_with_cancel(recording, Some(cancel))?;
    if media.sha256 != request.document.recording_sha256
        || media.sha256 != request.observation.recording_sha256
    {
        return Err("Selected recording differs from the reviewed evidence".into());
    }
    let mut inputs = vec![media];
    for binding in &request.document.datasets {
        check_cancel(cancel)?;
        let source = file_receipt_with_cancel(&resolve(base, &binding.path), Some(cancel))?;
        if source.sha256 != binding.sha256 {
            return Err(format!("Dataset changed: {}", source.path.display()));
        }
        if !inputs.iter().any(|old| old.path == source.path) {
            inputs.push(source);
        }
    }
    if !request.map.source_path.as_os_str().is_empty() {
        let source =
            file_receipt_with_cancel(&resolve(base, &request.map.source_path), Some(cancel))?;
        if source.sha256 != request.map.source_sha256 {
            return Err("Aircraft source changed".into());
        }
        if !inputs.iter().any(|old| old.path == source.path) {
            inputs.push(source);
        }
    }
    check_cancel(cancel)?;
    let catalog =
        file_receipt_with_cancel(&resolve(base, &request.lightning.input_path), Some(cancel))?;
    if catalog.sha256 != request.lightning.input_sha256
        || catalog.bytes != request.lightning.input_bytes
    {
        return Err("Lightning source changed".into());
    }
    if !inputs.iter().any(|old| old.path == catalog.path) {
        inputs.push(catalog);
    }
    check_cancel(cancel)?;
    Ok(inputs)
}
fn unchanged(inputs: &[InputReceipt]) -> Result<(), String> {
    for input in inputs {
        let current = file_receipt(&input.path)?;
        if current.sha256 != input.sha256 || current.bytes != input.bytes {
            return Err("Inputs changed during comparison".into());
        }
    }
    Ok(())
}
fn fresh_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > 64 * 1024 * 1024 {
        return Err(format!(
            "Joint artifact exceeds {}",
            crate::data_size::format_gib(64 * 1024 * 1024)
        ));
    }
    bytes.push(b'\n');
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Use a new output: {e}"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

/// Preview validates the full saved request without reading source recordings.
pub fn preview(request_path: &Path) -> Result<Value, String> {
    let (request, input) = read_request(request_path)?;
    let estimate = joint_evidence::estimate(&request)?;
    unchanged(std::slice::from_ref(&input))?;
    Ok(
        json!({"schema_version":1,"status":"ready_for_explicit_run","request":input,
        "estimate":estimate,"original_sources_verified":false,"location_verified":false}),
    )
}

/// Source byte verification, one explicit run, immutable request/result export.
pub fn run(
    request_path: &Path,
    recording_path: &Path,
    output: &Path,
    cancel: &AtomicBool,
    progress: impl FnMut(&JointProgress),
) -> Result<Value, String> {
    let (request, input) = read_request(request_path)?;
    let estimate = joint_evidence::estimate(&request)?;
    if output.exists() {
        return Err("Choose a new joint output directory".into());
    }
    let mut inputs = vec![input.clone()];
    fs::create_dir(output)
        .map_err(|e| format!("Use a new output directory with an existing parent: {e}"))?;
    let output = output.canonicalize().map_err(|e| e.to_string())?;
    let result = (|| {
        // Publish the complete request before any original-source scan or score.
        // A failed or cancelled verification must leave an explicit receipt too.
        fresh_json(&output.join("request.json"), &request)?;
        fresh_json(
            &output.join("started.json"),
            &json!({"schema_version":1,"status":"verifying_original_sources",
            "version":env!("CARGO_PKG_VERSION"),"request_input":input,"estimate":estimate,
            "original_sources_verified":false,"location_verified":false}),
        )?;
        let mut sources_verified = false;
        if !cancel.load(Ordering::Relaxed) {
            match verify_sources(
                &request,
                input.path.parent().ok_or("Request has no parent")?,
                recording_path,
                cancel,
            ) {
                Ok(sources) => {
                    inputs.extend(sources);
                    sources_verified = true;
                }
                Err(_) if cancel.load(Ordering::Relaxed) => {}
                Err(error) => return Err(error),
            }
        }
        fresh_json(
            &output.join("sources.json"),
            &json!({"schema_version":1,"inputs":inputs,
            "original_sources_verified":sources_verified,"cancelled":cancel.load(Ordering::Relaxed)}),
        )?;
        let report = joint_evidence::run(&request, cancel, progress)?;
        joint_evidence::validate_report(&request, &report)?;
        unchanged(&inputs)?;
        fresh_json(&output.join("results.json"), &report)?;
        let cancelled = report.status == JointStatus::Cancelled;
        let rankable = report.ranking_available;
        let snapshot = JointSnapshot { request, report };
        let export = investigation_report::export_joint(
            &output.join("report"),
            &snapshot.request.document,
            &snapshot,
        )?;
        let artifacts = [
            "request.json",
            "started.json",
            "sources.json",
            "results.json",
        ]
        .into_iter()
        .map(|name| file_receipt(&output.join(name)))
        .collect::<Result<Vec<_>, _>>()?;
        let terminal = json!({"schema_version":1,"status":if cancelled{"cancelled"}else{"completed"},
            "complete":!cancelled,"cancelled":cancelled,"ranking_available":rankable,
            "original_sources_verified":sources_verified,"location_verified":false,
            "artifacts":artifacts,"export_receipt":export});
        fresh_json(&output.join("completed.json"), &terminal)?;
        Ok(terminal)
    })();
    if let Err(error) = &result {
        let _ = fresh_json(
            &output.join("failed.json"),
            &json!({"schema_version":1,"status":"failed","error":error}),
        );
    }
    result
}
