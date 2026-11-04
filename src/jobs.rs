//! Saved, local, explicitly bounded jobs. This is not a general command runner.
//! Every attempt and state transition is retained; resume never edits an attempt.
use crate::{casework, detailed_data, discovery, single, verification};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputRef {
    File { path: PathBuf },
    Step { step_id: String, artifact: PathBuf },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobAction {
    Inspect {
        input: InputRef,
    },
    Discover {
        input: InputRef,
        #[serde(default)]
        channel: usize,
        #[serde(default)]
        settings: Option<InputRef>,
    },
    TerrainGrid {
        config: InputRef,
    },
    Rank {
        input: InputRef,
    },
    RankSingle {
        input: InputRef,
    },
    Overlays {
        input: InputRef,
        results: InputRef,
        #[serde(default = "default_overlay_count")]
        top: usize,
    },
}

fn default_overlay_count() -> usize {
    3
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobStep {
    pub id: String,
    pub action: JobAction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobPlan {
    pub schema_version: u32,
    pub case_manifest: PathBuf,
    pub steps: Vec<JobStep>,
    /// Actual media executables, not package-manager launch shims. Required only
    /// by media stages and copied into the job's frozen runtime directory.
    #[serde(default)]
    pub media_tools: Option<MediaTools>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaTools {
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobManifest {
    schema_version: u32,
    created_unix_s: u64,
    plan: JobPlan,
    original_sources: Vec<FileIdentity>,
    frozen_files: Vec<FileIdentity>,
    executable: PathBuf,
    location_verified: bool,
    limitations: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepStatus {
    pub id: String,
    pub state: String,
    pub attempt: u32,
    pub reused: bool,
    #[serde(default)]
    pub receipt_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobStatus {
    pub schema_version: u32,
    pub phase: String,
    pub updated_unix_s: u64,
    pub steps: Vec<StepStatus>,
    pub message: Option<String>,
    pub cancel_requested: bool,
    pub location_verified: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    schema_version: u32,
    step_id: String,
    attempt: u32,
    state: String,
    started_unix_s: u64,
    finished_unix_s: u64,
    inputs: Vec<FileIdentity>,
    artifacts: Vec<FileIdentity>,
    execution_evidence: Vec<FileIdentity>,
    exit_code: Option<i32>,
    error: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessOwner {
    pid: u32,
    created_unix_s: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerPermit {
    nonce: String,
    parent_pid: u32,
}

/// Called by the CLI before parsing or running any stage. Normal invocations
/// have no gate. Job children cannot launch media helpers until the parent has
/// attached the complete process tree to its native Windows Job Object.
pub fn await_worker_gate() -> Result<(), String> {
    let Some(path) = std::env::var_os("GEOSONNAR_JOB_GATE") else {
        if std::env::var_os("GEOSONNAR_JOB_NONCE").is_some()
            || std::env::var_os("GEOSONNAR_JOB_PARENT").is_some()
        {
            return Err("Incomplete job child gate configuration".into());
        }
        return Ok(());
    };
    let nonce = std::env::var("GEOSONNAR_JOB_NONCE").map_err(|_| "Job child has no gate nonce")?;
    let parent_pid = std::env::var("GEOSONNAR_JOB_PARENT")
        .map_err(|_| "Job child has no parent identity")?
        .parse::<u32>()
        .map_err(|_| "Invalid job parent identity")?;
    if nonce.is_empty() || parent_pid == 0 {
        return Err("Invalid job child gate".into());
    }
    let path = PathBuf::from(path);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if !process_alive(parent_pid)? {
            return Err("Job runner exited before permitting stage execution".into());
        }
        if path.exists() {
            let permit: WorkerPermit = read(&path)?;
            if permit.nonce != nonce || permit.parent_pid != parent_pid {
                return Err("Job child gate identity does not match".into());
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("Job runner did not permit stage execution within ten seconds".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn unique() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    verification::read_json(path)
}

fn write_new(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing output parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = parent.join(format!(".pending-{}", unique()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    if path.exists() {
        return Err(format!("Refusing to replace {}", path.display()));
    }
    fs::rename(&temporary, path).map_err(|e| e.to_string())
}

fn regular(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Job files must be regular files, not links".into());
    }
    Ok(())
}

fn identity(path: &Path) -> Result<FileIdentity, String> {
    regular(path)?;
    Ok(FileIdentity {
        path: path.canonicalize().map_err(|e| e.to_string())?,
        bytes: fs::metadata(path).map_err(|e| e.to_string())?.len(),
        sha256: detailed_data::sha256(path)?,
    })
}

fn relative_identity(root: &Path, path: &Path) -> Result<FileIdentity, String> {
    let mut receipt = identity(path)?;
    receipt.path = receipt
        .path
        .strip_prefix(root)
        .map_err(|_| "Artifact escaped job directory")?
        .to_path_buf();
    Ok(receipt)
}

fn matches(root: &Path, expected: &FileIdentity) -> bool {
    identity(&root.join(&expected.path))
        .is_ok_and(|actual| actual.bytes == expected.bytes && actual.sha256 == expected.sha256)
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|c| matches!(c, Component::Normal(_)))
        && !path.to_string_lossy().contains([':', '\\'])
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn references(action: &JobAction) -> Vec<&InputRef> {
    match action {
        JobAction::Inspect { input }
        | JobAction::Rank { input }
        | JobAction::RankSingle { input } => vec![input],
        JobAction::Discover {
            input, settings, ..
        } => std::iter::once(input).chain(settings.iter()).collect(),
        JobAction::TerrainGrid { config } => vec![config],
        JobAction::Overlays { input, results, .. } => vec![input, results],
    }
}

fn validate_plan(plan: &JobPlan) -> Result<(), String> {
    if plan.schema_version != 1 || plan.steps.is_empty() || plan.steps.len() > 32 {
        return Err("Job schema 1 requires 1..32 explicit stages".into());
    }
    let mut seen = BTreeSet::new();
    for step in &plan.steps {
        if let JobAction::Overlays { top, .. } = &step.action
            && !(1..=10).contains(top)
        {
            return Err("Overlay jobs require top between 1 and 10".into());
        }
        if !valid_id(&step.id) || seen.contains(&step.id) {
            return Err("Stage IDs must be unique safe names of at most 48 characters".into());
        }
        for reference in references(&step.action) {
            if let InputRef::Step { step_id, artifact } = reference
                && (!seen.contains(step_id) || !safe_relative(artifact))
            {
                return Err(
                    "Artifact references require an earlier stage and a safe relative file path"
                        .into(),
                );
            }
        }
        seen.insert(step.id.clone());
    }
    Ok(())
}

fn add_source(sources: &mut Vec<FileIdentity>, path: &Path) -> Result<(), String> {
    let receipt = identity(path)?;
    if !sources.iter().any(|r| r.path == receipt.path) {
        sources.push(receipt);
    }
    Ok(())
}

fn copy_frozen(
    root: &Path,
    source: &Path,
    relative: &Path,
    frozen: &mut Vec<FileIdentity>,
) -> Result<(), String> {
    let before = identity(source)?;
    let destination = root.join(relative);
    fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
    if destination.exists() {
        return Err("Snapshot destination already exists".into());
    }
    fs::copy(source, &destination).map_err(|e| e.to_string())?;
    let after = identity(source)?;
    let copied = relative_identity(root, &destination)?;
    if before != after || before.bytes != copied.bytes || before.sha256 != copied.sha256 {
        return Err("Source changed while taking job snapshot".into());
    }
    frozen.push(copied);
    Ok(())
}

fn checked_json(path: &Path, kind: &str) -> Result<(), String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > 512 * 1024 * 1024 {
        return Err("Job JSON exceeds 512 MiB limit".into());
    }
    match kind {
        "rank" => {
            let input: verification::SearchInput = read(path)?;
            verification::validate_search_input(&input)?;
        }
        "rank_single" => {
            let _: single::SingleInput = read(path)?;
        }
        "settings" => {
            let settings: discovery::DiscoverySettings = read(path)?;
            settings.validate()?;
        }
        "terrain" => {
            let _: detailed_data::TerrainRequest = read(path)?;
        }
        "rank_results" => {
            let results: serde_json::Value = read(path)?;
            if !results
                .get("best_per_flight_pair")
                .or_else(|| results.get("candidates"))
                .is_some_and(|v| v.is_array())
            {
                return Err("Overlay results require a native ranked candidate array".into());
            }
        }
        _ => return Err("Unsupported JSON dependency kind".into()),
    }
    Ok(())
}

fn freeze_input(
    root: &Path,
    base: &Path,
    input: &mut InputRef,
    id: &str,
    kind: &str,
    sources: &mut Vec<FileIdentity>,
    frozen: &mut Vec<FileIdentity>,
) -> Result<(), String> {
    let InputRef::File { path } = input else {
        return Ok(());
    };
    let original = base
        .join(&*path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    add_source(sources, &original)?;
    if kind == "media" {
        *path = original;
        return Ok(());
    }
    checked_json(&original, kind)?;
    let relative = PathBuf::from(format!("inputs/{id}/{kind}.json"));
    if kind == "terrain" {
        copy_frozen(
            root,
            &original,
            &PathBuf::from(format!("context/{id}-terrain-original.json")),
            frozen,
        )?;
        let mut config: detailed_data::TerrainRequest = read(&original)?;
        let parent = original.parent().unwrap();
        config.dem = parent
            .join(config.dem)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        config.geoid = parent
            .join(config.geoid)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        add_source(sources, &config.dem)?;
        add_source(sources, &config.geoid)?;
        write_new(&root.join(&relative), &config)?;
        frozen.push(relative_identity(root, &root.join(&relative))?);
    } else {
        copy_frozen(root, &original, &relative, frozen)?;
    }
    *path = relative;
    Ok(())
}

fn explicit_tool(plan: &JobPlan, base: &Path, name: &str) -> Result<PathBuf, String> {
    let tools = plan.media_tools.as_ref().ok_or(
        "Media jobs require explicit media_tools paths to the actual ffprobe/ffmpeg executables",
    )?;
    let path = match name {
        "ffmpeg" => &tools.ffmpeg,
        "ffprobe" => &tools.ffprobe,
        _ => return Err("Unknown media helper".into()),
    }
    .as_ref()
    .ok_or_else(|| format!("media_tools.{name} is required"))?;
    let path = base.join(path).canonicalize().map_err(|e| e.to_string())?;
    regular(&path)?;
    let mut sample = Vec::new();
    std::fs::File::open(&path)
        .map_err(|e| e.to_string())?
        .take(2 * 1024 * 1024)
        .read_to_end(&mut sample)
        .map_err(|e| e.to_string())?;
    let flattened = sample
        .into_iter()
        .filter(|b| *b != 0)
        .map(|b| b.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if flattened.windows(7).any(|w| w == b"shimgen")
        || flattened.windows(13).any(|w| w == b"chocolateyshim")
    {
        return Err(
            "Media helper is a package-manager shim; specify its actual executable target".into(),
        );
    }
    Ok(path)
}

/// Capture an immutable plan and tool context. Invoke from the GeoSonnar CLI;
/// the current GeoSonnar executable is copied for subsequent stage execution.
pub fn init_job(plan_path: &Path, output: &Path) -> Result<JobStatus, String> {
    if output.exists() {
        return Err("Use a new job directory".into());
    }
    let original_plan = identity(plan_path)?;
    let mut plan: JobPlan = read(plan_path)?;
    validate_plan(&plan)?;
    let base = plan_path.parent().unwrap_or(Path::new("."));
    let case_path = base
        .join(&plan.case_manifest)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let original_case = identity(&case_path)?;
    let case = casework::read_case(&case_path, true)?;
    let original_hints =
        serde_json::to_value(casework::hints(&case_path)?).map_err(|e| e.to_string())?;
    let recording = casework::recording_path(&case_path, &case)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    fs::create_dir_all(output.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    fs::create_dir(output).map_err(|e| e.to_string())?;
    let root = output.canonicalize().map_err(|e| e.to_string())?;
    let result = (|| {
        let mut sources = Vec::new();
        let mut frozen = Vec::new();
        add_source(&mut sources, &case_path)?;
        add_source(&mut sources, &recording)?;
        copy_frozen(
            &root,
            plan_path,
            Path::new("context/original-plan.json"),
            &mut frozen,
        )?;
        copy_frozen(
            &root,
            &case_path,
            Path::new("context/case.json"),
            &mut frozen,
        )?;
        let hint_folder = case_path.parent().unwrap().join("hints");
        if hint_folder.is_dir() {
            let mut hints = fs::read_dir(&hint_folder)
                .map_err(|e| e.to_string())?
                .map(|e| e.map(|v| v.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            hints.sort();
            if hints.len() > 1000 {
                return Err("A saved job supports at most 1000 existing hints".into());
            }
            for hint in hints {
                if hint.extension().and_then(|s| s.to_str()) == Some("json") {
                    add_source(&mut sources, &hint)?;
                    copy_frozen(
                        &root,
                        &hint,
                        &PathBuf::from("context/hints").join(hint.file_name().unwrap()),
                        &mut frozen,
                    )?;
                }
            }
        }
        let mut tools = BTreeSet::new();
        for step in &mut plan.steps {
            match &mut step.action {
                JobAction::Inspect { input } => {
                    freeze_input(
                        &root,
                        base,
                        input,
                        &step.id,
                        "media",
                        &mut sources,
                        &mut frozen,
                    )?;
                    if !matches!(input,InputRef::File {path} if path==&recording) {
                        return Err("Media stage must use this saved case's original recording; no earlier stage produces media".into());
                    }
                    tools.insert("ffprobe");
                }
                JobAction::Discover {
                    input, settings, ..
                } => {
                    freeze_input(
                        &root,
                        base,
                        input,
                        &step.id,
                        "media",
                        &mut sources,
                        &mut frozen,
                    )?;
                    if !matches!(input,InputRef::File {path} if path==&recording) {
                        return Err("Media stage must use this saved case's original recording; no earlier stage produces media".into());
                    }
                    if let Some(settings) = settings {
                        freeze_input(
                            &root,
                            base,
                            settings,
                            &step.id,
                            "settings",
                            &mut sources,
                            &mut frozen,
                        )?;
                    }
                    tools.extend(["ffprobe", "ffmpeg"]);
                }
                JobAction::TerrainGrid { config } => freeze_input(
                    &root,
                    base,
                    config,
                    &step.id,
                    "terrain",
                    &mut sources,
                    &mut frozen,
                )?,
                JobAction::Rank { input } => freeze_input(
                    &root,
                    base,
                    input,
                    &step.id,
                    "rank",
                    &mut sources,
                    &mut frozen,
                )?,
                JobAction::RankSingle { input } => freeze_input(
                    &root,
                    base,
                    input,
                    &step.id,
                    "rank_single",
                    &mut sources,
                    &mut frozen,
                )?,
                JobAction::Overlays { input, results, .. } => {
                    freeze_input(
                        &root,
                        base,
                        input,
                        &step.id,
                        "rank",
                        &mut sources,
                        &mut frozen,
                    )?;
                    freeze_input(
                        &root,
                        base,
                        results,
                        &step.id,
                        "rank_results",
                        &mut sources,
                        &mut frozen,
                    )?;
                }
            }
        }
        let executable = PathBuf::from(if cfg!(windows) {
            "bin/geosonnar.exe"
        } else {
            "bin/geosonnar"
        });
        copy_frozen(
            &root,
            &std::env::current_exe().map_err(|e| e.to_string())?,
            &executable,
            &mut frozen,
        )?;
        for tool in tools {
            let source = explicit_tool(&plan, base, tool)?;
            let filename = if cfg!(windows) {
                format!("{tool}.exe")
            } else {
                tool.to_owned()
            };
            let relative = PathBuf::from("bin/tools").join(filename);
            copy_frozen(&root, &source, &relative, &mut frozen)?;
            let mut command = Command::new(root.join(relative));
            command.arg("-version").stdin(Stdio::null());
            hide(&mut command);
            let output = command
                .output()
                .map_err(|e| format!("Copied {tool} could not start: {e}"))?;
            if !output.status.success()
                || !String::from_utf8_lossy(&output.stdout).starts_with(&format!("{tool} version "))
            {
                return Err(format!(
                    "Copied {tool} did not identify itself successfully; a standalone actual executable is required"
                ));
            }
            let version = root.join(format!("context/{tool}-version.json"));
            write_new(
                &version,
                &serde_json::json!({"source":source,"stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr)}),
            )?;
            frozen.push(relative_identity(&root, &version)?);
        }
        plan.case_manifest = PathBuf::from("context/case.json");
        for source in &sources {
            if !matches(&root, source) {
                return Err("A job source changed during initialization".into());
            }
        }
        if original_plan != identity(plan_path)?
            || original_case != identity(&case_path)?
            || original_hints
                != serde_json::to_value(casework::hints(&case_path)?).map_err(|e| e.to_string())?
        {
            return Err("Plan or case context changed while the job was being initialized".into());
        }
        let manifest = JobManifest {
            schema_version: 1, created_unix_s: now(), plan, original_sources: sources, frozen_files: frozen, executable,
            location_verified: false,
            limitations: vec!["Only inspect, discover, terrain-grid, rank, rank-single and overlays are job stages; downloads, reviewed-event selection and refinement are not automated here.".into(),
                "Recorded hints are prior information, never verification truth; later case hints are not incorporated into this job.".into(),
                "Attaching a self-contained search JSON to a case does not prove that its measured events came from that recording; search-source provenance still requires separate review.".into(),
                "Changing original recording/configuration/terrain inputs requires a new job; missing or changed output artifacts require a new attempt.".into(),
                "Operating-system components and dynamic runtime libraries are not packaged; media helper executables are copied and hashed.".into()],
        };
        write_new(&root.join("job.json"), &manifest)?;
        write_new(
            &root.join("job-integrity.json"),
            &relative_identity(&root, &root.join("job.json"))?,
        )?;
        let state = JobStatus {
            schema_version: 1,
            phase: "pending".into(),
            updated_unix_s: now(),
            steps: manifest
                .plan
                .steps
                .iter()
                .map(|s| StepStatus {
                    id: s.id.clone(),
                    state: "pending".into(),
                    attempt: 0,
                    reused: false,
                    receipt_sha256: None,
                })
                .collect(),
            message: None,
            cancel_requested: false,
            location_verified: false,
        };
        append_state(&root, &state)?;
        Ok(state)
    })();
    if let Err(error) = &result {
        let _ = write_new(
            &root.join("initialization-failure.json"),
            &serde_json::json!({"status":"failed","error":error,"location_verified":false}),
        );
    }
    result
}

fn state_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    let directory = root.join("state");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut files = fs::read_dir(directory)
        .map_err(|e| e.to_string())?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    files.sort();
    if files.len() > 10000 {
        return Err("Job state history exceeds 10000 transitions".into());
    }
    for (index, file) in files.iter().enumerate() {
        if file.file_name().and_then(|s| s.to_str()) != Some(&format!("{:010}.json", index + 1)) {
            return Err("Job state journal has a missing or unexpected transition".into());
        }
    }
    Ok(files)
}

fn append_state(root: &Path, state: &JobStatus) -> Result<(), String> {
    let index = state_paths(root)?.len() + 1;
    write_new(&root.join(format!("state/{index:010}.json")), state)
}

pub fn status(job_dir: &Path) -> Result<JobStatus, String> {
    let paths = state_paths(job_dir)?;
    let mut state: JobStatus = read(paths.last().ok_or("Job has no durable state")?)?;
    if state.schema_version != 1
        || state.location_verified
        || state.steps.iter().any(|s| !valid_id(&s.id))
    {
        return Err("Invalid durable job status".into());
    }
    state.cancel_requested = job_dir.join("cancel.json").exists();
    Ok(state)
}

fn manifest_metadata(root: &Path) -> Result<JobManifest, String> {
    let integrity: FileIdentity = read(&root.join("job-integrity.json"))?;
    if integrity.path != Path::new("job.json") || !matches(root, &integrity) {
        return Err("Saved job manifest changed".into());
    }
    let manifest: JobManifest = read(&root.join("job.json"))?;
    validate_plan(&manifest.plan)?;
    if manifest.schema_version != 1 || manifest.location_verified {
        return Err("Unsupported saved job".into());
    }
    Ok(manifest)
}

fn load_manifest(root: &Path) -> Result<JobManifest, String> {
    let manifest = manifest_metadata(root)?;
    for expected in manifest
        .original_sources
        .iter()
        .chain(&manifest.frozen_files)
    {
        if !matches(root, expected) {
            return Err(format!(
                "Bound input or runtime changed: {}; create a new job",
                expected.path.display()
            ));
        }
    }
    Ok(manifest)
}

/// Cheap case association for a GUI listing. This checks saved metadata only;
/// it does not claim that recording, terrain, runtime or output bytes still match.
pub fn case_plan(job_dir: &Path, case_manifest: &Path) -> Result<JobPlan, String> {
    let root = job_dir.canonicalize().map_err(|e| e.to_string())?;
    let manifest = manifest_metadata(&root)?;
    let selected = identity(case_manifest)?;
    let frozen = manifest
        .frozen_files
        .iter()
        .find(|f| f.path == Path::new("context/case.json"))
        .ok_or("Job has no frozen case identity")?;
    if selected.bytes != frozen.bytes
        || selected.sha256 != frozen.sha256
        || !matches(&root, frozen)
        || !manifest
            .original_sources
            .iter()
            .any(|source| source == &selected)
    {
        return Err("Job does not belong to the selected immutable saved case".into());
    }
    Ok(manifest.plan)
}

/// Verify an artifact and the completed pipeline leading to it without running
/// or altering the job. Expensive source hashing belongs on a worker thread.
pub fn verified_artifact(
    job_dir: &Path,
    step_id: &str,
    artifact: &Path,
) -> Result<PathBuf, String> {
    if !safe_relative(artifact) {
        return Err("Artifact path must stay inside its completed stage".into());
    }
    let root = job_dir.canonicalize().map_err(|e| e.to_string())?;
    let manifest = load_manifest(&root)?;
    let state = status(&root)?;
    if state.steps.len() != manifest.plan.steps.len()
        || state
            .steps
            .iter()
            .zip(&manifest.plan.steps)
            .any(|(a, b)| a.id != b.id)
    {
        return Err("Job status does not match its saved stage plan".into());
    }
    let target = manifest
        .plan
        .steps
        .iter()
        .position(|s| s.id == step_id)
        .ok_or("Unknown job stage")?;
    let mut selected = None;
    for (index, step) in manifest.plan.steps.iter().enumerate().take(target + 1) {
        let current = &state.steps[index];
        if current.state != "completed" || current.attempt == 0 {
            return Err("The requested artifact has an incomplete stage or predecessor".into());
        }
        let directory = attempt_directory(&root, &step.id, current.attempt);
        let receipt = directory.join("result.json");
        if current.receipt_sha256.as_ref() != Some(&detailed_data::sha256(&receipt)?) {
            return Err("Completed attempt receipt changed".into());
        }
        let attempt: Attempt = read(&receipt)?;
        if attempt.schema_version != 1
            || attempt.step_id != step.id
            || attempt.attempt != current.attempt
            || attempt.state != "completed"
            || attempt.artifacts.is_empty()
            || execution_inventory(&root, &directory)? != attempt.execution_evidence
            || artifact_inventory(&root, &directory.join("artifacts"))? != attempt.artifacts
            || invocation(&root, step, &state, &directory.join("artifacts"))?.1 != attempt.inputs
        {
            return Err("Completed stage evidence or its dependencies changed; resume to create a fresh attempt".into());
        }
        if index == target {
            let path = directory.join("artifacts").join(artifact);
            let receipt = relative_identity(&root, &path)?;
            if !attempt.artifacts.contains(&receipt) {
                return Err("Artifact is not listed in the completed stage receipt".into());
            }
            selected = Some(path.canonicalize().map_err(|e| e.to_string())?);
        }
    }
    selected.ok_or_else(|| "No verified artifact was found".into())
}

struct RunnerLock {
    path: PathBuf,
}
impl Drop for RunnerLock {
    fn drop(&mut self) {
        if self.path.join("child.json").exists()
            && !read::<ProcessOwner>(&self.path.join("child.json"))
                .is_ok_and(|child| process_alive(child.pid) == Ok(false))
        {
            // Keep the ownership evidence if a child may still be alive.
            return;
        }
        // Only our own, fixed lock files are removed; attempt data is never erased.
        let _ = fs::remove_file(self.path.join("owner.json"));
        let _ = fs::remove_file(self.path.join("child.json"));
        let _ = fs::remove_dir(&self.path);
    }
}

fn acquire_lock(root: &Path) -> Result<RunnerLock, String> {
    if root.join("recovery.lock").exists() {
        return Err("Job lock recovery is in progress".into());
    }
    let path = root.join("runner.lock");
    fs::create_dir(&path).map_err(|_| {
        "Job already has a runner lock; inspect status or explicitly recover a dead runner"
            .to_owned()
    })?;
    let guard = RunnerLock { path };
    if root.join("recovery.lock").exists() {
        return Err("Job lock recovery is in progress".into());
    }
    write_new(
        &guard.path.join("owner.json"),
        &ProcessOwner {
            pid: std::process::id(),
            created_unix_s: now(),
        },
    )?;
    Ok(guard)
}

#[cfg(windows)]
fn process_alive(pid: u32) -> Result<bool, String> {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(handle: *mut c_void, exit: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }
    // Query-only access. A reused live PID conservatively blocks recovery.
    unsafe {
        let handle = OpenProcess(0x1000, 0, pid);
        if handle.is_null() {
            return match GetLastError() {
                87 => Ok(false),
                5 => Ok(true),
                code => Err(format!(
                    "Cannot prove process {pid} is dead: Windows error {code}"
                )),
            };
        }
        let mut exit = 0;
        let success = GetExitCodeProcess(handle, &mut exit);
        CloseHandle(handle);
        if success == 0 {
            Err("Cannot inspect runner process".into())
        } else {
            Ok(exit == 259)
        }
    }
}

#[cfg(not(windows))]
fn process_alive(pid: u32) -> Result<bool, String> {
    if Path::new("/proc").is_dir() {
        return Ok(Path::new("/proc").join(pid.to_string()).exists());
    }
    let status = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(true)
    } else {
        Err("Cannot prove process death on this platform".into())
    }
}

/// Explicit stale-lock recovery. Live or unidentifiable owners are never displaced.
pub fn recover_job(job_dir: &Path) -> Result<JobStatus, String> {
    let root = job_dir.canonicalize().map_err(|e| e.to_string())?;
    let recovery = root.join("recovery.lock");
    fs::create_dir(&recovery).map_err(|_| "Another recovery is in progress".to_owned())?;
    let result = (|| {
        let lock = root.join("runner.lock");
        let owner: ProcessOwner = read(&lock.join("owner.json"))?;
        if owner.pid == 0 {
            return Err("Invalid runner PID; cannot prove ownership for recovery".into());
        }
        if process_alive(owner.pid)? {
            return Err("Recorded runner PID is still alive; recovery refused (including possible PID reuse)".into());
        }
        if lock.join("child.json").exists() {
            let child: ProcessOwner = read(&lock.join("child.json"))?;
            if child.pid == 0 {
                return Err("Invalid stage PID; cannot prove ownership for recovery".into());
            }
            if process_alive(child.pid)? {
                return Err("The prior stage process is still alive; recovery refused".into());
            }
        }
        fs::create_dir_all(root.join("history")).map_err(|e| e.to_string())?;
        fs::rename(
            &lock,
            root.join("history")
                .join(format!("recovered-lock-{}", unique())),
        )
        .map_err(|e| e.to_string())?;
        let mut state = status(&root)?;
        state.phase = "interrupted".into();
        state.updated_unix_s = now();
        state.message=Some("Dead runner lock retained in history; resume starts a new attempt for unfinished stages".into());
        for step in &mut state.steps {
            if step.state == "running" {
                step.state = "interrupted".into();
            }
        }
        append_state(&root, &state)?;
        Ok(state)
    })();
    let _ = fs::remove_dir(recovery);
    result
}

pub fn cancel_job(job_dir: &Path) -> Result<JobStatus, String> {
    let mut state = status(job_dir)?;
    if state.phase == "completed" {
        return Err("Completed jobs have no active work to cancel".into());
    }
    let path = job_dir.join("cancel.json");
    if !path.exists() {
        write_new(
            &path,
            &serde_json::json!({"requested_unix_s":now(),"requester_pid":std::process::id()}),
        )?;
    }
    state.cancel_requested = true;
    Ok(state)
}

fn attempt_directory(root: &Path, id: &str, attempt: u32) -> PathBuf {
    root.join("attempts").join(id).join(format!("{attempt:06}"))
}

fn resolve_input(root: &Path, input: &InputRef, state: &JobStatus) -> Result<PathBuf, String> {
    match input {
        InputRef::File { path } => Ok(root.join(path)),
        InputRef::Step { step_id, artifact } => {
            let previous = state
                .steps
                .iter()
                .find(|s| &s.id == step_id)
                .ok_or("Unknown dependency stage")?;
            if previous.state != "completed" || !safe_relative(artifact) {
                return Err("Dependency stage is not complete or artifact path is unsafe".into());
            }
            let directory = attempt_directory(root, step_id, previous.attempt).join("artifacts");
            let file = directory
                .join(artifact)
                .canonicalize()
                .map_err(|e| e.to_string())?;
            if !file.starts_with(directory.canonicalize().map_err(|e| e.to_string())?) {
                return Err("Dependency artifact escaped its stage".into());
            }
            Ok(file)
        }
    }
}

fn invocation(
    root: &Path,
    step: &JobStep,
    state: &JobStatus,
    artifacts: &Path,
) -> Result<(Vec<String>, Vec<FileIdentity>), String> {
    let mut args = Vec::new();
    let mut dependencies = Vec::new();
    let mut input_path = |reference: &InputRef| -> Result<PathBuf, String> {
        let path = resolve_input(root, reference, state)?;
        add_source(&mut dependencies, &path)?;
        Ok(path)
    };
    let text = |path: &Path| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| "Job paths must be Unicode".to_owned())
    };
    match &step.action {
        JobAction::Inspect { input } => args.extend([
            "inspect".into(),
            "--input".into(),
            text(&input_path(input)?)?,
            "--out".into(),
            text(&artifacts.join("report.json"))?,
        ]),
        JobAction::Discover {
            input,
            channel,
            settings,
        } => {
            args.extend([
                "discover".into(),
                "--input".into(),
                text(&input_path(input)?)?,
                "--out".into(),
                text(artifacts)?,
                "--channel".into(),
                channel.to_string(),
            ]);
            if let Some(settings) = settings {
                let path = input_path(settings)?;
                checked_json(&path, "settings")?;
                args.extend(["--settings".into(), text(&path)?]);
            }
        }
        JobAction::TerrainGrid { config } => {
            let path = input_path(config)?;
            checked_json(&path, "terrain")?;
            let terrain: detailed_data::TerrainRequest = read(&path)?;
            let base = path.parent().unwrap();
            add_source(&mut dependencies, &base.join(terrain.dem))?;
            add_source(&mut dependencies, &base.join(terrain.geoid))?;
            args.extend([
                "terrain-grid".into(),
                "--config".into(),
                text(&path)?,
                "--out".into(),
                text(artifacts)?,
            ]);
        }
        JobAction::Rank { input } | JobAction::RankSingle { input } => {
            let (command, kind) = if matches!(step.action, JobAction::Rank { .. }) {
                ("rank", "rank")
            } else {
                ("rank-single", "rank_single")
            };
            let path = input_path(input)?;
            checked_json(&path, kind)?;
            args.extend([
                command.into(),
                "--input".into(),
                text(&path)?,
                "--out".into(),
                text(&artifacts.join("results.json"))?,
            ]);
        }
        JobAction::Overlays {
            input,
            results,
            top,
        } => {
            let input = input_path(input)?;
            checked_json(&input, "rank")?;
            let results = input_path(results)?;
            checked_json(&results, "rank_results")?;
            args.extend([
                "overlays".into(),
                "--input".into(),
                text(&input)?,
                "--results".into(),
                text(&results)?,
                "--out".into(),
                text(&artifacts.join("overlays.json"))?,
                "--top".into(),
                top.to_string(),
            ]);
        }
    }
    dependencies.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((args, dependencies))
}

fn artifact_inventory(root: &Path, directory: &Path) -> Result<Vec<FileIdentity>, String> {
    let mut pending = vec![directory.to_path_buf()];
    let mut files = Vec::new();
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(&folder).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if metadata.file_type().is_symlink() {
                return Err("Stage artifacts cannot contain links".into());
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                files.push(relative_identity(root, &path)?);
            } else {
                return Err("Unsupported stage artifact type".into());
            }
            if files.len() + pending.len() > 10000 {
                return Err("Stage output exceeds 10000 artifact entries".into());
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn execution_inventory(root: &Path, directory: &Path) -> Result<Vec<FileIdentity>, String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name() == "result.json" || entry.file_name() == "artifacts" {
            continue;
        }
        files.push(relative_identity(root, &entry.path())?);
        if files.len() > 100 {
            return Err("Unexpectedly many attempt evidence files".into());
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn hide(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

#[cfg(windows)]
mod windows_tree {
    use std::{ffi::c_void, os::windows::io::AsRawHandle, process::Child};
    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        per_process_user_time: i64,
        per_job_user_time: i64,
        flags: u32,
        minimum_working_set: usize,
        maximum_working_set: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io_counters: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            info: *const c_void,
            size: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn TerminateJobObject(job: *mut c_void, exit_code: u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }
    pub struct Tree(*mut c_void);
    impl Tree {
        pub fn attach(child: &Child) -> Result<Self, String> {
            // https://learn.microsoft.com/windows/win32/procthread/job-objects
            // The noninherited owned handle also kills all assigned descendants
            // if the runner exits unexpectedly. No PID-based external killer.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(format!(
                        "Cannot create stage process group: {}",
                        GetLastError()
                    ));
                }
                let tree = Self(handle);
                let limits = ExtendedLimits {
                    basic: BasicLimits {
                        flags: 0x2000,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                if SetInformationJobObject(
                    handle,
                    9,
                    (&limits as *const ExtendedLimits).cast(),
                    std::mem::size_of::<ExtendedLimits>() as u32,
                ) == 0
                {
                    return Err(format!(
                        "Cannot configure stage process group: {}",
                        GetLastError()
                    ));
                }
                if AssignProcessToJobObject(handle, child.as_raw_handle().cast()) == 0 {
                    return Err(format!("Cannot own stage process tree: {}", GetLastError()));
                }
                Ok(tree)
            }
        }
        pub fn terminate(&self) -> Result<(), String> {
            unsafe {
                if TerminateJobObject(self.0, 1) == 0 {
                    Err(format!(
                        "Cannot terminate stage process tree: {}",
                        GetLastError()
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

fn terminate(child: &mut GuardedChild) -> Result<(), String> {
    #[cfg(windows)]
    {
        child.tree.terminate()?;
    }
    #[cfg(not(windows))]
    {
        child.kill().map_err(|e| e.to_string())?;
    }
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}

struct GuardedChild {
    process: Child,
    #[cfg(windows)]
    tree: windows_tree::Tree,
}
impl GuardedChild {
    fn spawn(command: &mut Command) -> Result<Self, String> {
        let mut process = command.spawn().map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let tree = match windows_tree::Tree::attach(&process) {
            Ok(tree) => tree,
            Err(error) => {
                let _ = process.kill();
                let _ = process.wait();
                return Err(error);
            }
        };
        #[cfg(not(windows))]
        let _ = &mut process;
        Ok(Self {
            process,
            #[cfg(windows)]
            tree,
        })
    }
}
impl std::ops::Deref for GuardedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.process
    }
}
impl std::ops::DerefMut for GuardedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.process
    }
}
impl Drop for GuardedChild {
    fn drop(&mut self) {
        if !self.process.try_wait().is_ok_and(|exit| exit.is_some()) {
            let _ = terminate(self);
        }
    }
}

fn next_attempt(root: &Path, id: &str) -> Result<u32, String> {
    let directory = root.join("attempts").join(id);
    if !directory.exists() {
        return Ok(1);
    }
    let mut max = 0;
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let n = name
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or("Unexpected attempt directory")?;
        max = max.max(n);
    }
    if max >= 1000 {
        return Err("Stage has reached the 1000-attempt limit".into());
    }
    Ok(max + 1)
}

fn execute_attempt(
    root: &Path,
    manifest: &JobManifest,
    step: &JobStep,
    state: &mut JobStatus,
    index: usize,
    lock: &RunnerLock,
) -> Result<Attempt, String> {
    let number = next_attempt(root, &step.id)?;
    let directory = attempt_directory(root, &step.id, number);
    fs::create_dir_all(directory.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::create_dir(&directory).map_err(|e| e.to_string())?;
    let artifacts = directory.join("artifacts");
    let mut attempt = Attempt {
        schema_version: 1,
        step_id: step.id.clone(),
        attempt: number,
        state: "running".into(),
        started_unix_s: now(),
        finished_unix_s: 0,
        inputs: Vec::new(),
        artifacts: Vec::new(),
        execution_evidence: Vec::new(),
        exit_code: None,
        error: None,
    };
    state.steps[index] = StepStatus {
        id: step.id.clone(),
        state: "running".into(),
        attempt: number,
        reused: false,
        receipt_sha256: None,
    };
    state.updated_unix_s = now();
    append_state(root, state)?;
    let result = (|| {
        let (args, inputs) = invocation(root, step, state, &artifacts)?;
        attempt.inputs = inputs;
        write_new(&directory.join("start.json"), &attempt)?;
        write_new(
            &directory.join("command.json"),
            &serde_json::json!({"executable":manifest.executable,"arguments":args,"shell":false}),
        )?;
        if !matches!(
            step.action,
            JobAction::Discover { .. } | JobAction::TerrainGrid { .. }
        ) {
            fs::create_dir(&artifacts).map_err(|e| e.to_string())?;
        }
        let stdout = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("stdout.log"))
            .map_err(|e| e.to_string())?;
        let stderr = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("stderr.log"))
            .map_err(|e| e.to_string())?;
        let mut command = Command::new(root.join(&manifest.executable));
        let gate = directory.join("permit.json");
        let nonce = unique();
        command
            .args(&args)
            .env("GEOSONNAR_JOB_GATE", &gate)
            .env("GEOSONNAR_JOB_NONCE", &nonce)
            .env("GEOSONNAR_JOB_PARENT", std::process::id().to_string())
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        hide(&mut command);
        let mut child = GuardedChild::spawn(&mut command)?;
        if let Err(error) = write_new(
            &lock.path.join("child.json"),
            &ProcessOwner {
                pid: child.id(),
                created_unix_s: now(),
            },
        ) {
            let _ = terminate(&mut child);
            return Err(error);
        }
        write_new(
            &directory.join("child.json"),
            &ProcessOwner {
                pid: child.id(),
                created_unix_s: now(),
            },
        )?;
        write_new(
            &gate,
            &WorkerPermit {
                nonce,
                parent_pid: std::process::id(),
            },
        )?;
        let exit = loop {
            if root.join("cancel.json").exists() {
                terminate(&mut child)?;
                attempt.state = "canceled".into();
                break None;
            }
            if let Some(exit) = child.try_wait().map_err(|e| e.to_string())? {
                break Some(exit);
            }
            thread::sleep(Duration::from_millis(50));
        };
        fs::remove_file(lock.path.join("child.json")).map_err(|e| e.to_string())?;
        attempt.exit_code = exit.and_then(|s| s.code());
        if attempt.state == "canceled" {
            return Ok(());
        }
        let no_candidate = attempt.exit_code == Some(3)
            && matches!(
                step.action,
                JobAction::Rank { .. } | JobAction::RankSingle { .. }
            )
            && read::<serde_json::Value>(&artifacts.join("results.json"))
                .is_ok_and(|v| v["status"] == "no_candidate");
        if attempt.exit_code != Some(0) && !no_candidate {
            return Err(format!(
                "Stage exited with code {:?}; see stderr.log",
                attempt.exit_code
            ));
        }
        for input in &attempt.inputs {
            if !matches(root, input) {
                return Err("A stage input changed while it was running".into());
            }
        }
        load_manifest(root)?;
        attempt.artifacts = artifact_inventory(root, &artifacts)?;
        if attempt.artifacts.is_empty() {
            return Err("Successful stage did not leave any artifacts".into());
        }
        attempt.state = "completed".into();
        Ok(())
    })();
    if let Err(error) = result {
        attempt.state = "failed".into();
        attempt.error = Some(error);
    }
    attempt.finished_unix_s = now();
    attempt.execution_evidence = execution_inventory(root, &directory)?;
    write_new(&directory.join("result.json"), &attempt)?;
    Ok(attempt)
}

/// Run synchronously; another CLI invocation may request cancellation. Resume
/// validates all dependencies before reusing an already completed stage.
pub fn run_job(job_dir: &Path, resume: bool) -> Result<JobStatus, String> {
    let root = job_dir.canonicalize().map_err(|e| e.to_string())?;
    let lock = acquire_lock(&root)?;
    let mut state = status(&root)?;
    if !resume && state.phase != "pending" {
        return Err("Job was already started; use resume explicitly".into());
    }
    if resume && root.join("cancel.json").exists() {
        fs::create_dir_all(root.join("history")).map_err(|e| e.to_string())?;
        fs::rename(
            root.join("cancel.json"),
            root.join("history")
                .join(format!("cancel-{}.json", unique())),
        )
        .map_err(|e| e.to_string())?;
    }
    let result = (|| {
        let manifest = load_manifest(&root)?;
        if state.steps.len() != manifest.plan.steps.len()
            || state
                .steps
                .iter()
                .zip(&manifest.plan.steps)
                .any(|(a, b)| a.id != b.id)
        {
            return Err("Job state stages no longer match the immutable plan".into());
        }
        state.phase = "running".into();
        state.message = None;
        state.cancel_requested = false;
        state.updated_unix_s = now();
        append_state(&root, &state)?;
        let mut invalidated = false;
        for (index, step) in manifest.plan.steps.iter().enumerate() {
            if root.join("cancel.json").exists() {
                state.phase = "canceled".into();
                state.cancel_requested = true;
                break;
            }
            let previous = &state.steps[index];
            let reusable = if resume && !invalidated && previous.state == "completed" {
                let directory = attempt_directory(&root, &step.id, previous.attempt);
                let old = read::<Attempt>(&directory.join("result.json"));
                old.is_ok_and(|old| {
                    old.state == "completed"
                        && previous.receipt_sha256.as_ref().is_some_and(|expected| {
                            detailed_data::sha256(&directory.join("result.json"))
                                .is_ok_and(|actual| &actual == expected)
                        })
                        && execution_inventory(&root, &directory)
                            .is_ok_and(|current| current == old.execution_evidence)
                        && !old.artifacts.is_empty()
                        && artifact_inventory(&root, &directory.join("artifacts"))
                            .is_ok_and(|current| current == old.artifacts)
                        && invocation(&root, step, &state, &directory.join("artifacts"))
                            .is_ok_and(|(_, inputs)| inputs == old.inputs)
                })
            } else {
                false
            };
            if reusable {
                state.steps[index].reused = true;
                state.updated_unix_s = now();
                append_state(&root, &state)?;
                continue;
            }
            if previous.attempt > 0 {
                write_new(
                    &root.join("history").join(format!(
                        "invalidation-{}-{}.json",
                        step.id,
                        unique()
                    )),
                    &serde_json::json!({"stage":step.id,"previous_attempt":previous.attempt,
                        "previous_state":previous.state,"upstream_invalidated":invalidated,
                        "reason":"Previous attempt was unfinished, unsuccessful, or its inputs/artifact inventory changed; all previous files retained",
                        "created_unix_s":now()}),
                )?;
            }
            invalidated = true;
            let attempt = execute_attempt(&root, &manifest, step, &mut state, index, &lock)?;
            state.steps[index].state = attempt.state.clone();
            state.steps[index].receipt_sha256 = Some(detailed_data::sha256(
                &attempt_directory(&root, &step.id, attempt.attempt).join("result.json"),
            )?);
            state.updated_unix_s = now();
            if attempt.state != "completed" {
                state.phase = attempt.state;
                state.message = attempt.error;
                state.cancel_requested = root.join("cancel.json").exists();
                append_state(&root, &state)?;
                return Ok(());
            }
            append_state(&root, &state)?;
        }
        if state.phase == "running" {
            state.phase = "completed".into();
        }
        state.updated_unix_s = now();
        append_state(&root, &state)?;
        Ok(())
    })();
    if let Err(error) = result {
        state.phase = "failed".into();
        state.message = Some(error);
        state.updated_unix_s = now();
        append_state(&root, &state)?;
    }
    Ok(state)
}
