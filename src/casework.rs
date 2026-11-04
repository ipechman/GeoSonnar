//! Local saved cases and append-only narrative priors. A clue is never a truth
//! label or an automatic numerical search restriction.
use crate::{detailed_data::sha256, media, verification::read_json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseMedia {
    pub duration_s: f64,
    pub sample_rate_hz: u32,
    pub channels: u32,
    pub audio_codec: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseRecording {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    pub media: CaseMedia,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedCase {
    pub schema_version: u32,
    pub case_id: String,
    pub title: String,
    pub created_unix_s: u64,
    pub recording: CaseRecording,
    pub local_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HintKind {
    Region,
    DateTime,
    Other,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HintSource {
    User,
    EmbeddedMetadata,
    Analyst,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HintCertainty {
    Uncertain,
    Reported,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HintRequest {
    pub kind: HintKind,
    pub text: String,
    pub source: HintSource,
    pub certainty: HintCertainty,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseHint {
    pub schema_version: u32,
    pub case_manifest_sha256: String,
    pub recording_sha256: String,
    pub created_unix_s: u64,
    pub hint: HintRequest,
}

fn now() -> Result<std::time::Duration, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())
}

#[cfg(windows)]
fn publish_new(staged: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let from: Vec<_> = staged.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // No REPLACE_EXISTING: publication must fail if a final evidence file exists.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x8) } == 0 {
        return Err(format!(
            "Cannot publish new case evidence: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn publish_new(staged: &Path, destination: &Path) -> Result<(), String> {
    fs::hard_link(staged, destination).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(staged);
    Ok(())
}

fn new_json(path: &Path, value: &impl Serialize, case_directory: &Path) -> Result<(), String> {
    if path.exists() {
        return Err("Case evidence already exists".into());
    }
    let staging = case_directory.join(".case-pending");
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let temporary = staging.join(format!("{}-{}.json", std::process::id(), now()?.as_nanos()));
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|e| format!("Cannot create new case evidence {}: {e}", path.display()))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    publish_new(&temporary, path)
}

fn text_valid(text: &str, maximum: usize) -> bool {
    !text.trim().is_empty() && text.len() <= maximum && !text.chars().any(|c| c == '\0')
}

fn valid_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Preserve portable relative references where both locations share a root.
/// Outside-case source references are read-only; media is never copied or moved.
fn relative_source(source: &Path, base: &Path) -> PathBuf {
    let left: Vec<_> = source.components().collect();
    let right: Vec<_> = base.components().collect();
    let common = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
    if common == 0 || left.first() != right.first() {
        return source.into();
    }
    let mut path = PathBuf::new();
    for component in &right[common..] {
        if matches!(component, Component::Normal(_)) {
            path.push("..");
        }
    }
    for component in &left[common..] {
        path.push(component.as_os_str());
    }
    path
}

pub fn recording_path(manifest: &Path, case: &SavedCase) -> PathBuf {
    manifest
        .parent()
        .unwrap_or(Path::new("."))
        .join(&case.recording.path)
}

pub fn read_case(manifest: &Path, verify_recording: bool) -> Result<SavedCase, String> {
    read_case_with_checkpoint(manifest, verify_recording, || Ok(()))
}

pub fn read_case_with_checkpoint(
    manifest: &Path,
    verify_recording: bool,
    mut checkpoint: impl FnMut() -> Result<(), String>,
) -> Result<SavedCase, String> {
    let before = crate::detailed_data::sha256_with_checkpoint(manifest, &mut checkpoint)?;
    let case: SavedCase = read_json(manifest)?;
    let media = &case.recording.media;
    if case.schema_version != 1
        || !case.local_only
        || !text_valid(&case.title, 200)
        || case.case_id.is_empty()
        || case.case_id.len() > 80
        || !case
            .case_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || case.recording.path.as_os_str().is_empty()
        || case.recording.bytes == 0
        || !valid_hash(&case.recording.sha256)
        || !media.duration_s.is_finite()
        || media.duration_s <= 0.0
        || media.sample_rate_hz == 0
        || media.channels == 0
        || !text_valid(&media.audio_codec, 100)
    {
        return Err("Invalid local case identity, recording receipt or audio metadata".into());
    }
    if verify_recording {
        verify_source_with_checkpoint(manifest, &case, &mut checkpoint)?;
    }
    if crate::detailed_data::sha256_with_checkpoint(manifest, &mut checkpoint)? != before {
        return Err("Case changed while reading its manifest".into());
    }
    Ok(case)
}

fn verify_source(manifest: &Path, case: &SavedCase) -> Result<(), String> {
    verify_source_with_checkpoint(manifest, case, || Ok(()))
}

fn verify_source_with_checkpoint(
    manifest: &Path,
    case: &SavedCase,
    mut checkpoint: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    checkpoint()?;
    let input = recording_path(manifest, case);
    if fs::metadata(&input)
        .map_err(|e| format!("Recording is unavailable: {e}"))?
        .len()
        != case.recording.bytes
        || crate::detailed_data::sha256_with_checkpoint(&input, &mut checkpoint)?
            != case.recording.sha256
    {
        return Err("Saved recording has changed; earlier results cannot be reused".into());
    }
    Ok(())
}

pub fn create(input: &Path, output: &Path, title: &str) -> Result<Value, String> {
    create_with_probe(input, output, title, None)
}

pub fn create_with_probe(
    input: &Path,
    output: &Path,
    title: &str,
    probe: Option<&Path>,
) -> Result<Value, String> {
    if !text_valid(title, 200) {
        return Err("Supply a case title of 1..200 bytes".into());
    }
    if output.exists() {
        return Err("Use a new saved-case directory".into());
    }
    let source = input.canonicalize().map_err(|e| e.to_string())?;
    let before = sha256(&source)?;
    let info = media::inspect_with_probe(&source, probe)?;
    if before != sha256(&source)?
        || fs::metadata(&source).map_err(|e| e.to_string())?.len() != info.bytes
    {
        return Err("Recording changed during case intake".into());
    }
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::create_dir(output).map_err(|e| format!("Use a new saved-case directory: {e}"))?;
    let base = output.canonicalize().map_err(|e| e.to_string())?;
    let timestamp = now()?;
    let case = SavedCase {
        schema_version: 1,
        case_id: format!("case-{:x}-{}", timestamp.as_nanos(), std::process::id()),
        title: title.into(),
        created_unix_s: timestamp.as_secs(),
        local_only: true,
        recording: CaseRecording {
            path: relative_source(&source, &base),
            sha256: before,
            bytes: info.bytes,
            media: CaseMedia {
                duration_s: info.duration_s,
                sample_rate_hz: info.sample_rate_hz,
                channels: info.channels,
                audio_codec: info.audio_codec.clone(),
            },
        },
    };
    new_json(&output.join("intake.json"), &info, output)?;
    new_json(&output.join("case.json"), &case, output)?;
    status(&output.join("case.json"))
}

pub fn hints(manifest: &Path) -> Result<Vec<CaseHint>, String> {
    let before = sha256(manifest)?;
    let case = read_case(manifest, false)?;
    let directory = manifest.parent().unwrap_or(Path::new(".")).join("hints");
    if !directory.exists() {
        if sha256(manifest)? != before {
            return Err("Case changed while reading clues".into());
        }
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file()
            || entry.path().extension().and_then(|x| x.to_str()) != Some("json")
        {
            return Err("Case hints directory contains an unsupported entry".into());
        }
        paths.push(entry.path());
        if paths.len() > 10_000 {
            return Err("Case hint count exceeds the supported budget".into());
        }
    }
    paths.sort();
    let mut result = Vec::new();
    for path in paths {
        let hint: CaseHint = read_json(&path)?;
        if hint.schema_version != 1
            || hint.case_manifest_sha256 != before
            || hint.recording_sha256 != case.recording.sha256
            || !text_valid(&hint.hint.text, 16_000)
        {
            return Err(format!(
                "Hint does not belong to the immutable saved case: {}",
                path.display()
            ));
        }
        result.push(hint);
    }
    if sha256(manifest)? != before {
        return Err("Case changed while reading clues".into());
    }
    Ok(result)
}

pub fn add_hint(manifest: &Path, request_path: &Path) -> Result<PathBuf, String> {
    let manifest_before = sha256(manifest)?;
    let request_before = sha256(request_path)?;
    let case = read_case(manifest, false)?;
    let request: HintRequest = read_json(request_path)?;
    if !text_valid(&request.text, 16_000) {
        return Err("Clue text must contain 1..16000 bytes".into());
    }
    hints(manifest)?;
    let timestamp = now()?;
    let hint = CaseHint {
        schema_version: 1,
        case_manifest_sha256: manifest_before.clone(),
        recording_sha256: case.recording.sha256,
        created_unix_s: timestamp.as_secs(),
        hint: request,
    };
    let directory = manifest.parent().unwrap_or(Path::new(".")).join("hints");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    if sha256(manifest)? != manifest_before || sha256(request_path)? != request_before {
        return Err("Case/clue input changed during registration".into());
    }
    let path = directory.join(format!(
        "hint-{}-{}.json",
        timestamp.as_nanos(),
        std::process::id()
    ));
    new_json(&path, &hint, manifest.parent().unwrap_or(Path::new(".")))?;
    Ok(path)
}

pub fn status(manifest: &Path) -> Result<Value, String> {
    let before = sha256(manifest)?;
    let case = read_case(manifest, false)?;
    let clues = hints(manifest)?;
    let input = recording_path(manifest, &case);
    let source_check = verify_source(manifest, &case);
    if sha256(manifest)? != before {
        return Err("Case changed while preparing status".into());
    }
    Ok(
        json!({"schema_version":1,"case":case,"case_manifest":manifest,
        "recording_path":input,"recording_matches":source_check.is_ok(),
        "recording_problem":source_check.err(),"hints":clues,"location_verified":false,
        "notes":["Clues are narrative prior information; they do not create numerical search bounds or establish capture time.",
        "Media remains at its source path. Keep the relative file layout when moving the case, or create a new case referencing the same verified recording.",
        "Completed jobs retain their own frozen clue snapshot; later hints require a new job to affect its declared context.",
        "Interrupted writes remain under .case-pending and are not treated as published clues."]}),
    )
}
