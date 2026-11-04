//! GUI service boundary. Mutations run through the packaged companion CLI so
//! saved jobs always freeze geosonnar.exe, never the graphical application.
//! All APIs are synchronous and belong on UI worker threads when doing I/O.
use crate::{
    casework::{self, CaseHint, HintRequest, SavedCase},
    detailed_data::sha256,
    jobs::{self, InputRef, JobAction, JobPlan, JobStatus, JobStep, MediaTools},
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const MAX_PLAN_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CASE_JOBS: usize = 1000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaseOverview {
    pub manifest: PathBuf,
    pub case: SavedCase,
    pub hints: Vec<CaseHint>,
    pub recording_path: PathBuf,
    /// None means the overview deliberately did not hash the recording.
    pub recording_verified: Option<bool>,
    pub recording_problem: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactCandidate {
    pub step_id: String,
    pub path: PathBuf,
    /// Listing a completed result never substitutes for opening verification.
    pub verified: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobOverview {
    pub directory: PathBuf,
    pub status: Option<JobStatus>,
    pub discovery_artifacts: Vec<ArtifactCandidate>,
    pub problem: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifiedDiscoveryArtifact {
    pub path: PathBuf,
    pub sha256: String,
    pub channel: usize,
    pub step_id: String,
    pub job_directory: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Companion {
    path: PathBuf,
}

#[derive(Serialize)]
struct CliReceipt {
    schema_version: u32,
    executable: PathBuf,
    executable_sha256: String,
    arguments: Vec<String>,
    started_unix_s: u64,
    finished_unix_s: u64,
    exit_code: Option<i32>,
    error: Option<String>,
    stdout_sha256: String,
    stderr_sha256: String,
}

struct CliResult {
    directory: PathBuf,
    code: Option<i32>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    )
}

fn regular(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Expected a regular local file, not a link".into());
    }
    Ok(())
}

fn manifest_path(path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_dir() {
        path.join("case.json")
    } else {
        path.to_path_buf()
    };
    regular(&path)?;
    path.canonicalize().map_err(|e| e.to_string())
}

fn new_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

fn managed_directory(case_manifest: &Path, name: &str, create: bool) -> Result<PathBuf, String> {
    let base = case_manifest.parent().ok_or("Case has no directory")?;
    let directory = base.join(name);
    if !directory.exists() {
        if !create {
            return Ok(directory);
        }
        fs::create_dir(&directory).map_err(|e| e.to_string())?;
    }
    let metadata = fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Case-managed directories cannot be links or files".into());
    }
    let canonical = directory.canonicalize().map_err(|e| e.to_string())?;
    if canonical.parent() != Some(base) {
        return Err("Case-managed directory escapes the selected case".into());
    }
    Ok(canonical)
}

fn managed_job(case_manifest: &Path, job_dir: &Path) -> Result<PathBuf, String> {
    let parent = managed_directory(case_manifest, "jobs", false)?;
    let metadata = fs::symlink_metadata(job_dir).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Select a regular saved-job directory".into());
    }
    let job = job_dir.canonicalize().map_err(|e| e.to_string())?;
    if job.parent() != Some(parent.as_path()) {
        return Err("Job is outside the selected case's jobs directory".into());
    }
    Ok(job)
}

fn operation(case_manifest: &Path, kind: &str) -> Result<PathBuf, String> {
    let parent = managed_directory(case_manifest, "gui-operations", true)?;
    let directory = parent.join(unique(kind));
    fs::create_dir(&directory).map_err(|e| e.to_string())?;
    Ok(directory)
}

fn tail(path: &Path) -> String {
    let result = (|| -> std::io::Result<String> {
        let mut file = File::open(path)?;
        let size = file.metadata()?.len();
        file.seek(SeekFrom::Start(size.saturating_sub(16 * 1024)))?;
        let mut bytes = Vec::new();
        file.take(16 * 1024).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
    })();
    result.unwrap_or_default()
}

impl CliResult {
    fn failure(&self) -> String {
        let stderr = tail(&self.directory.join("stderr.log"));
        let detail = if stderr.is_empty() {
            tail(&self.directory.join("stdout.log"))
        } else {
            stderr
        };
        format!(
            "GeoSonnar command did not complete successfully (exit {:?}). {}\nDetails: {}",
            self.code,
            detail,
            self.directory.display()
        )
    }
    fn require_success(&self) -> Result<(), String> {
        if self.code == Some(0) {
            Ok(())
        } else {
            Err(self.failure())
        }
    }
}

/// Read lightweight case metadata by default. Use verify_recording=true only on
/// an explicit worker action; it hashes the potentially large recording.
pub fn case_overview(case_manifest: &Path, verify_recording: bool) -> Result<CaseOverview, String> {
    let manifest = manifest_path(case_manifest)?;
    let before = sha256(&manifest)?;
    let case = casework::read_case(&manifest, false)?;
    let hints = casework::hints(&manifest)?;
    let recording_path = casework::recording_path(&manifest, &case);
    let verification = verify_recording.then(|| casework::read_case(&manifest, true));
    let recording_verified = verification.as_ref().map(Result::is_ok);
    let recording_problem = verification.and_then(Result::err);
    if sha256(&manifest)? != before {
        return Err("Case changed while its overview was being prepared".into());
    }
    Ok(CaseOverview {
        manifest,
        case,
        hints,
        recording_path,
        recording_verified,
        recording_problem,
    })
}

/// This metadata view does not hash recording/terrain/runtime/artifacts and
/// therefore must never authorize review directly from a candidate path.
pub fn job_overview(case_manifest: &Path, job_dir: &Path) -> Result<JobOverview, String> {
    let case_manifest = manifest_path(case_manifest)?;
    let directory = managed_job(&case_manifest, job_dir)?;
    let plan = jobs::case_plan(&directory, &case_manifest)?;
    let status = jobs::status(&directory)?;
    if status.steps.len() != plan.steps.len()
        || status
            .steps
            .iter()
            .zip(&plan.steps)
            .any(|(a, b)| a.id != b.id)
    {
        return Err("Job status and saved plan disagree".into());
    }
    let mut discovery_artifacts = Vec::new();
    let mut problem = None;
    for (step, state) in plan.steps.iter().zip(&status.steps) {
        if matches!(step.action, JobAction::Discover { .. }) && state.state == "completed" {
            let path = directory
                .join("attempts")
                .join(&step.id)
                .join(format!("{:06}", state.attempt))
                .join("artifacts/discovery.json");
            if path.is_file() {
                discovery_artifacts.push(ArtifactCandidate {
                    step_id: step.id.clone(),
                    path,
                    verified: false,
                });
            } else {
                problem=Some("A completed discovery result is missing; resume the job to create a fresh attempt".into());
            }
        }
    }
    Ok(JobOverview {
        directory,
        status: Some(status),
        discovery_artifacts,
        problem,
    })
}

pub fn list_jobs(case_manifest: &Path) -> Result<Vec<JobOverview>, String> {
    let case_manifest = manifest_path(case_manifest)?;
    casework::read_case(&case_manifest, false)?;
    let directory = managed_directory(&case_manifest, "jobs", false)?;
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        paths.push(entry.map_err(|e| e.to_string())?.path());
        if paths.len() > MAX_CASE_JOBS {
            return Err("Case has more than 1000 job entries; narrow the case workspace".into());
        }
    }
    paths.sort();
    paths.reverse();
    Ok(paths
        .into_iter()
        .map(|path| match job_overview(&case_manifest, &path) {
            Ok(overview) => overview,
            Err(error) => JobOverview {
                directory: path,
                status: None,
                discovery_artifacts: Vec::new(),
                problem: Some(error),
            },
        })
        .collect())
}

/// Full source/runtime/attempt verification for a worker before review. Call
/// again before saving a job-backed review, then let event_review bind its bytes.
pub fn verified_discovery_artifact(
    case_manifest: &Path,
    job_dir: &Path,
    step_id: &str,
) -> Result<VerifiedDiscoveryArtifact, String> {
    let case_manifest = manifest_path(case_manifest)?;
    let directory = managed_job(&case_manifest, job_dir)?;
    let plan = jobs::case_plan(&directory, &case_manifest)?;
    let channel = plan
        .steps
        .iter()
        .find_map(|step| {
            if step.id == step_id
                && let JobAction::Discover { channel, .. } = step.action
            {
                Some(channel)
            } else {
                None
            }
        })
        .ok_or("Select a completed discovery stage, not another job artifact")?;
    let path = jobs::verified_artifact(&directory, step_id, Path::new("discovery.json"))?;
    let digest = sha256(&path)?;
    // The small case binding is checked again after expensive verification.
    jobs::case_plan(&directory, &case_manifest)?;
    Ok(VerifiedDiscoveryArtifact {
        path,
        sha256: digest,
        channel,
        step_id: step_id.into(),
        job_directory: directory,
    })
}

impl Companion {
    pub fn locate(gui_executable: &Path) -> Result<Self, String> {
        let folder = gui_executable
            .parent()
            .ok_or("The graphical application has no installation directory")?;
        Self::from_path(&folder.join(if cfg!(windows) {
            "geosonnar.exe"
        } else {
            "geosonnar"
        }))
    }
    pub fn current() -> Result<Self, String> {
        Self::locate(&std::env::current_exe().map_err(|e| e.to_string())?)
    }
    pub fn from_path(path: &Path) -> Result<Self, String> {
        regular(path).map_err(|error| {
            format!("The companion GeoSonnar program is required next to the GUI: {error}")
        })?;
        let expected = if cfg!(windows) {
            "geosonnar.exe"
        } else {
            "geosonnar"
        };
        if path.file_name().and_then(|s| s.to_str()) != Some(expected) {
            return Err("Select the packaged geosonnar companion executable".into());
        }
        Ok(Self {
            path: path.canonicalize().map_err(|e| e.to_string())?,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn invoke(&self, args: Vec<OsString>, directory: &Path) -> Result<CliResult, String> {
        let arguments = args
            .iter()
            .map(|s| {
                s.to_str()
                    .map(str::to_owned)
                    .ok_or("CLI arguments must be Unicode")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let executable_sha256 = sha256(&self.path)?;
        let started_unix_s = now();
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
        new_json(
            &directory.join("command.json"),
            &serde_json::json!({"executable":self.path,"arguments":arguments,"shell":false,"executable_sha256":executable_sha256}),
        )?;
        let mut command = Command::new(&self.path);
        command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let completed = command.status();
        let error = completed.as_ref().err().map(ToString::to_string);
        let code = completed.ok().and_then(|s| s.code());
        let receipt = CliReceipt {
            schema_version: 1,
            executable: self.path.clone(),
            executable_sha256,
            arguments,
            started_unix_s,
            finished_unix_s: now(),
            exit_code: code,
            error: error.clone(),
            stdout_sha256: sha256(&directory.join("stdout.log"))?,
            stderr_sha256: sha256(&directory.join("stderr.log"))?,
        };
        new_json(&directory.join("result.json"), &receipt)?;
        if let Some(error) = error {
            return Err(format!(
                "Could not start the companion: {error}; logs: {}",
                directory.display()
            ));
        }
        Ok(CliResult {
            directory: directory.to_path_buf(),
            code,
        })
    }

    pub fn create_case(
        &self,
        recording: &Path,
        output: &Path,
        title: &str,
    ) -> Result<CaseOverview, String> {
        self.create_case_with_probe(recording, output, title, None)
    }
    pub fn create_case_with_probe(
        &self,
        recording: &Path,
        output: &Path,
        title: &str,
        probe: Option<&Path>,
    ) -> Result<CaseOverview, String> {
        if output.exists() {
            return Err("Choose a new case directory".into());
        }
        let recording = recording.canonicalize().map_err(|e| e.to_string())?;
        let output = if output.is_absolute() {
            output.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| e.to_string())?
                .join(output)
        };
        let parent = output.parent().ok_or("New case directory has no parent")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let logs = parent.join(".geosonnar-gui-operations");
        if logs.exists() {
            let metadata = fs::symlink_metadata(&logs).map_err(|e| e.to_string())?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("Intake log directory must be a regular local directory".into());
            }
        } else {
            fs::create_dir(&logs).map_err(|e| e.to_string())?;
        }
        let operation = logs.join(unique("case-create"));
        fs::create_dir(&operation).map_err(|e| e.to_string())?;
        let mut args = vec![
            "case-create".into(),
            "--input".into(),
            recording.into_os_string(),
            "--title".into(),
            title.into(),
            "--out".into(),
            output.clone().into_os_string(),
        ];
        if let Some(probe) = probe {
            args.extend([
                "--ffprobe".into(),
                probe
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .into_os_string(),
            ]);
        }
        self.invoke(args, &operation)?.require_success()?;
        case_overview(&output.join("case.json"), true)
    }

    pub fn add_hint(
        &self,
        case_manifest: &Path,
        hint: &HintRequest,
    ) -> Result<CaseOverview, String> {
        let manifest = manifest_path(case_manifest)?;
        casework::read_case(&manifest, false)?;
        let directory = operation(&manifest, "add-hint")?;
        let request = directory.join("hint-request.json");
        new_json(&request, hint)?;
        self.invoke(
            vec![
                "case-add-hint".into(),
                "--case".into(),
                manifest.clone().into_os_string(),
                "--hint".into(),
                request.into_os_string(),
            ],
            &directory,
        )?
        .require_success()?;
        case_overview(&manifest, false)
    }

    pub fn prepare_discovery_job(
        &self,
        case_manifest: &Path,
        channel: usize,
        tools: &MediaTools,
    ) -> Result<JobOverview, String> {
        self.prepare_discovery_job_with_settings(case_manifest, channel, tools, None)
    }

    pub fn prepare_discovery_job_with_settings(
        &self,
        case_manifest: &Path,
        channel: usize,
        tools: &MediaTools,
        settings: Option<&crate::discovery::DiscoverySettings>,
    ) -> Result<JobOverview, String> {
        if let Some(settings) = settings {
            settings.validate()?;
        }
        let manifest = manifest_path(case_manifest)?;
        let case = casework::read_case(&manifest, false)?;
        if channel >= case.recording.media.channels as usize {
            return Err(
                "Selected audio channel is outside the recording's saved channel count".into(),
            );
        }
        let mut tools = tools.clone();
        for path in [&mut tools.ffmpeg, &mut tools.ffprobe] {
            let selected = path.as_ref().ok_or(
                "Choose the actual FFmpeg and FFprobe executables before preparing discovery",
            )?;
            regular(selected)?;
            *path = Some(selected.canonicalize().map_err(|e| e.to_string())?);
        }
        let directory = operation(&manifest, "prepare-discovery")?;
        let settings = if let Some(settings) = settings {
            let path = directory.join("discovery-settings.json");
            new_json(&path, settings)?;
            Some(InputRef::File { path })
        } else {
            None
        };
        let plan = JobPlan {
            schema_version: 1,
            case_manifest: manifest.clone(),
            media_tools: Some(tools),
            steps: vec![JobStep {
                id: "discover".into(),
                action: JobAction::Discover {
                    input: InputRef::File {
                        path: casework::recording_path(&manifest, &case)
                            .canonicalize()
                            .map_err(|e| e.to_string())?,
                    },
                    channel,
                    settings,
                },
            }],
        };
        let path = directory.join("plan.json");
        new_json(&path, &plan)?;
        self.create_job_from_plan(&manifest, &path, &directory)
    }

    /// Snapshot the selected plan before normalizing its top-level file paths.
    /// Relative nested dependencies remain relative to their original config.
    pub fn import_job_plan(
        &self,
        case_manifest: &Path,
        plan_path: &Path,
    ) -> Result<JobOverview, String> {
        let manifest = manifest_path(case_manifest)?;
        casework::read_case(&manifest, false)?;
        regular(plan_path)?;
        if fs::metadata(plan_path).map_err(|e| e.to_string())?.len() > MAX_PLAN_BYTES {
            return Err("Imported plan exceeds the 4 MiB GUI plan limit".into());
        }
        let original = fs::read(plan_path).map_err(|e| e.to_string())?;
        let mut plan: JobPlan = serde_json::from_slice(&original).map_err(|e| e.to_string())?;
        let base = plan_path.parent().unwrap_or(Path::new("."));
        if base
            .join(&plan.case_manifest)
            .canonicalize()
            .map_err(|e| e.to_string())?
            != manifest
        {
            return Err("Imported job plan belongs to a different saved case".into());
        }
        plan.case_manifest = manifest.clone();
        for step in &mut plan.steps {
            normalize_action_paths(base, &mut step.action)?;
        }
        if let Some(tools) = &mut plan.media_tools {
            for value in [&mut tools.ffmpeg, &mut tools.ffprobe]
                .into_iter()
                .flatten()
            {
                *value = base
                    .join(&*value)
                    .canonicalize()
                    .map_err(|e| e.to_string())?;
            }
        }
        let directory = operation(&manifest, "import-plan")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("original-plan.json"))
            .map_err(|e| e.to_string())?;
        file.write_all(&original)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        let normalized = directory.join("plan.json");
        new_json(&normalized, &plan)?;
        self.create_job_from_plan(&manifest, &normalized, &directory)
    }

    fn create_job_from_plan(
        &self,
        case_manifest: &Path,
        plan_path: &Path,
        operation: &Path,
    ) -> Result<JobOverview, String> {
        let jobs = managed_directory(case_manifest, "jobs", true)?;
        let output = jobs.join(unique("job"));
        let result = self.invoke(
            vec![
                "job-create".into(),
                "--plan".into(),
                plan_path.as_os_str().to_owned(),
                "--out".into(),
                output.clone().into_os_string(),
            ],
            operation,
        )?;
        result.require_success()?;
        job_overview(case_manifest, &output)
    }

    pub fn run_job(
        &self,
        case_manifest: &Path,
        job_dir: &Path,
        resume: bool,
    ) -> Result<JobOverview, String> {
        self.job_command(case_manifest, job_dir, "job-run", Some(resume))
    }
    pub fn cancel_job(&self, case_manifest: &Path, job_dir: &Path) -> Result<JobOverview, String> {
        self.job_command(case_manifest, job_dir, "job-cancel", None)
    }
    pub fn recover_job(&self, case_manifest: &Path, job_dir: &Path) -> Result<JobOverview, String> {
        self.job_command(case_manifest, job_dir, "job-recover", None)
    }
    fn job_command(
        &self,
        case_manifest: &Path,
        job_dir: &Path,
        command: &str,
        resume: Option<bool>,
    ) -> Result<JobOverview, String> {
        let manifest = manifest_path(case_manifest)?;
        let job = managed_job(&manifest, job_dir)?;
        jobs::case_plan(&job, &manifest)?;
        let directory = operation(&manifest, command)?;
        let mut args = vec![command.into(), "--job".into(), job.clone().into_os_string()];
        if let Some(resume) = resume {
            args.extend(["--resume".into(), resume.to_string().into()]);
        }
        let result = self.invoke(args, &directory)?;
        let mut overview = job_overview(&manifest, &job)?;
        if result.code != Some(0) && !(command == "job-run" && result.code == Some(4)) {
            if command == "job-run"
                && overview
                    .status
                    .as_ref()
                    .is_some_and(|s| s.phase == "failed")
            {
                overview.problem = Some(result.failure());
            } else {
                return Err(result.failure());
            }
        }
        Ok(overview)
    }
}

fn normalize_action_paths(base: &Path, action: &mut JobAction) -> Result<(), String> {
    fn normalize(base: &Path, input: &mut InputRef) -> Result<(), String> {
        if let InputRef::File { path } = input {
            *path = base
                .join(&*path)
                .canonicalize()
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    match action {
        JobAction::Inspect { input }
        | JobAction::Rank { input }
        | JobAction::RankSingle { input } => normalize(base, input)?,
        JobAction::Discover {
            input, settings, ..
        } => {
            normalize(base, input)?;
            if let Some(settings) = settings {
                normalize(base, settings)?;
            }
        }
        JobAction::TerrainGrid { config } => normalize(base, config)?,
        JobAction::Overlays { input, results, .. } => {
            normalize(base, input)?;
            normalize(base, results)?;
        }
    }
    Ok(())
}
