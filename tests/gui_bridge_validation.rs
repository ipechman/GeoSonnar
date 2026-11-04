//! The graphical bridge exercises the real companion CLI with fabricated data.
use geosonnar::{
    casework::{
        CaseMedia, CaseRecording, HintCertainty, HintKind, HintRequest, HintSource, SavedCase,
    },
    detailed_data::sha256,
    gui_bridge::{self, Companion},
    jobs::{self, MediaTools},
    verification::{self, write_json},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Workspace {
    root: PathBuf,
    path: PathBuf,
}
impl Workspace {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let path = root.join(format!(
            "gui bridge controls $; {}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&path).unwrap();
        Self { root, path }
    }
    fn case(&self, name: &str) -> PathBuf {
        let directory = self.path.join(name);
        fs::create_dir(&directory).unwrap();
        let source = directory.join("recording & source.wav");
        fs::write(
            &source,
            b"fabricated recording identity; not acoustic measurements",
        )
        .unwrap();
        let case = SavedCase {
            schema_version: 1,
            case_id: name.into(),
            title: format!("Case {name}"),
            created_unix_s: 1,
            local_only: true,
            recording: CaseRecording {
                path: "recording & source.wav".into(),
                bytes: fs::metadata(&source).unwrap().len(),
                sha256: sha256(&source).unwrap(),
                media: CaseMedia {
                    duration_s: 10.0,
                    sample_rate_hz: 8000,
                    channels: 1,
                    audio_codec: "pcm_s16le".into(),
                },
            },
        };
        let manifest = directory.join("case.json");
        write_json(&manifest, &case).unwrap();
        manifest
    }
    fn rank_plan(&self, case: &Path, heavy: bool) -> PathBuf {
        let directory = self.path.join(format!(
            "outside plan {}",
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&directory).unwrap();
        let mut input = verification::demo_input().unwrap();
        if heavy {
            let first = input.observers[0].clone();
            input.observers = (0..5000)
                .map(|n| {
                    let mut v = first.clone();
                    v.id = format!("synthetic-{n}");
                    v
                })
                .collect();
        }
        write_json(&directory.join("search input.json"), &input).unwrap();
        let path = directory.join("plan & input.json");
        write_json(&path,&json!({"schema_version":1,"case_manifest":case,"steps":[{"id":"rank","action":{"kind":"rank","input":{"source":"file","path":"search input.json"}}},{"id":"curves","action":{"kind":"overlays","input":{"source":"file","path":"search input.json"},"results":{"source":"step","step_id":"rank","artifact":"results.json"},"top":1}}]})).unwrap();
        path
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        if let Ok(path) = self.path.canonicalize() {
            assert!(path.starts_with(&self.root) && path != self.root);
            let _ = fs::remove_dir_all(path);
        }
    }
}
fn companion() -> Companion {
    Companion::from_path(Path::new(env!("CARGO_BIN_EXE_geosonnar"))).unwrap()
}
fn value(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn companion_is_resolved_next_to_gui_and_never_falls_back_to_an_unrelated_path_program() {
    let cli = Path::new(env!("CARGO_BIN_EXE_geosonnar"));
    let gui = cli.parent().unwrap().join("geosonnar-gui.exe");
    assert_eq!(
        Companion::locate(&gui).unwrap().path(),
        cli.canonicalize().unwrap()
    );
    let work = Workspace::new();
    assert!(Companion::locate(&work.path.join("geosonnar-gui.exe")).is_err());
    let other = work.path.join("other.exe");
    fs::write(&other, b"not the companion").unwrap();
    assert!(Companion::from_path(&other).is_err());
}

#[test]
fn lightweight_case_view_does_not_claim_a_source_hash_check_and_worker_verification_reports_missing_media()
 {
    let work = Workspace::new();
    let case = work.case("case-a");
    let quick = gui_bridge::case_overview(&case, false).unwrap();
    assert_eq!(quick.recording_verified, None);
    assert_eq!(quick.hints.len(), 0);
    let checked = gui_bridge::case_overview(&case, true).unwrap();
    assert_eq!(checked.recording_verified, Some(true));
    fs::remove_file(checked.recording_path).unwrap();
    assert_eq!(
        gui_bridge::case_overview(&case, false)
            .unwrap()
            .recording_verified,
        None
    );
    let missing = gui_bridge::case_overview(&case, true).unwrap();
    assert_eq!(missing.recording_verified, Some(false));
    assert!(missing.recording_problem.unwrap().contains("unavailable"));
}

#[test]
fn clue_registration_uses_real_cli_and_preserves_literal_text_and_operation_logs() {
    let work = Workspace::new();
    let case = work.case("clues");
    let before = sha256(&case).unwrap();
    let hint = HintRequest {
        kind: HintKind::Region,
        text: "Possible region; $(not a command) & quotation: \"unverified\"".into(),
        source: HintSource::User,
        certainty: HintCertainty::Uncertain,
    };
    let overview = companion().add_hint(&case, &hint).unwrap();
    assert_eq!(overview.hints.len(), 1);
    assert_eq!(overview.hints[0].hint.text, hint.text);
    assert_eq!(sha256(&case).unwrap(), before);
    let operations = fs::read_dir(case.parent().unwrap().join("gui-operations"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(operations.len(), 1);
    let operation = &operations[0];
    for file in [
        "command.json",
        "hint-request.json",
        "result.json",
        "stdout.log",
        "stderr.log",
    ] {
        assert!(operation.join(file).is_file());
    }
    let command = value(&operation.join("command.json"));
    assert_eq!(command["shell"], false);
    assert_eq!(command["arguments"][0], "case-add-hint");
    let result = value(&operation.join("result.json"));
    assert_eq!(result["exit_code"], 0);
    assert_eq!(
        result["stdout_sha256"],
        sha256(&operation.join("stdout.log")).unwrap()
    );
}

#[test]
fn imported_plan_preserves_relative_bindings_and_companion_executable_then_runs_and_reuses() {
    let work = Workspace::new();
    let case = work.case("import");
    let plan = work.rank_plan(&case, false);
    let original = fs::read(&plan).unwrap();
    let cli = companion();
    let job = cli.import_job_plan(&case, &plan).unwrap();
    assert_eq!(job.status.as_ref().unwrap().phase, "pending");
    assert_eq!(
        job.directory.parent().unwrap(),
        case.parent().unwrap().join("jobs")
    );
    let manifest = value(&job.directory.join("job.json"));
    let original_input = plan
        .parent()
        .unwrap()
        .join("search input.json")
        .canonicalize()
        .unwrap();
    assert!(
        manifest["original_sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["path"] == serde_json::to_value(&original_input).unwrap())
    );
    let frozen = job.directory.join(if cfg!(windows) {
        "bin/geosonnar.exe"
    } else {
        "bin/geosonnar"
    });
    assert_eq!(sha256(&frozen).unwrap(), sha256(cli.path()).unwrap());
    assert_eq!(fs::read(&plan).unwrap(), original);
    let completed = cli.run_job(&case, &job.directory, false).unwrap();
    assert_eq!(completed.status.as_ref().unwrap().phase, "completed");
    let curves =
        jobs::verified_artifact(&job.directory, "curves", Path::new("overlays.json")).unwrap();
    assert!(curves.is_file());
    let resumed = cli.run_job(&case, &job.directory, true).unwrap();
    assert!(resumed.status.unwrap().steps.iter().all(|s| s.reused));
    let listed = gui_bridge::list_jobs(&case).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].problem.is_none());
    assert!(listed[0].discovery_artifacts.is_empty());
    assert!(gui_bridge::verified_discovery_artifact(&case, &job.directory, "rank").is_err());
}

#[test]
fn foreign_plans_jobs_and_outside_paths_cannot_be_started_cancelled_or_recovered_for_selected_case()
{
    let work = Workspace::new();
    let first = work.case("first");
    let second = work.case("second");
    let cli = companion();
    let plan = work.rank_plan(&second, false);
    assert!(
        cli.import_job_plan(&first, &plan)
            .unwrap_err()
            .contains("different")
    );
    assert!(!first.parent().unwrap().join("jobs").exists());
    let foreign = cli.import_job_plan(&second, &plan).unwrap();
    assert!(cli.run_job(&first, &foreign.directory, false).is_err());
    assert!(cli.cancel_job(&first, &foreign.directory).is_err());
    assert!(cli.recover_job(&first, &foreign.directory).is_err());
    assert!(!foreign.directory.join("cancel.json").exists());
    let parent = first.parent().unwrap().join("jobs");
    fs::create_dir(&parent).unwrap();
    let misplaced = parent.join("misplaced-job");
    fs::rename(&foreign.directory, &misplaced).unwrap();
    assert!(
        gui_bridge::job_overview(&first, &misplaced)
            .unwrap_err()
            .contains("does not belong")
    );
    let listed = gui_bridge::list_jobs(&first).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].status.is_none());
    assert!(
        listed[0]
            .problem
            .as_ref()
            .unwrap()
            .contains("does not belong")
    );
    assert!(cli.cancel_job(&first, &misplaced).is_err());
    assert!(!misplaced.join("cancel.json").exists());
}

#[test]
fn verified_artifact_rejects_stale_sources_outputs_logs_and_path_escape_without_running_anything() {
    for changed in ["source", "artifact", "log", "receipt"] {
        let work = Workspace::new();
        let case = work.case("verify");
        let plan = work.rank_plan(&case, false);
        let cli = companion();
        let job = cli.import_job_plan(&case, &plan).unwrap();
        cli.run_job(&case, &job.directory, false).unwrap();
        assert!(
            jobs::verified_artifact(&job.directory, "curves", Path::new("overlays.json")).is_ok()
        );
        assert!(
            jobs::verified_artifact(&job.directory, "rank", Path::new("../result.json")).is_err()
        );
        let target = match changed {
            "source" => case.parent().unwrap().join("recording & source.wav"),
            "artifact" => job
                .directory
                .join("attempts/rank/000001/artifacts/results.json"),
            "log" => job.directory.join("attempts/rank/000001/stderr.log"),
            "receipt" => job.directory.join("attempts/rank/000001/result.json"),
            _ => unreachable!(),
        };
        let mut bytes = fs::read(&target).unwrap();
        bytes.extend(b" \n");
        fs::write(&target, bytes).unwrap();
        let state_before = fs::read_dir(job.directory.join("state")).unwrap().count();
        assert!(
            jobs::verified_artifact(&job.directory, "curves", Path::new("overlays.json")).is_err(),
            "{changed}"
        );
        assert_eq!(
            fs::read_dir(job.directory.join("state")).unwrap().count(),
            state_before
        );
        assert!(!job.directory.join("attempts/rank/000002").exists());
        // Cheap overview is metadata only, even when the scientific bytes changed.
        assert!(gui_bridge::job_overview(&case, &job.directory).is_ok());
    }
}

#[test]
fn bridge_cancels_and_resumes_pending_jobs_without_blocking_the_ui_worker_contract() {
    let work = Workspace::new();
    let case = work.case("cancel");
    let cli = companion();
    let job = cli
        .import_job_plan(&case, &work.rank_plan(&case, false))
        .unwrap();
    assert!(
        cli.cancel_job(&case, &job.directory)
            .unwrap()
            .status
            .unwrap()
            .cancel_requested
    );
    let canceled = cli.run_job(&case, &job.directory, false).unwrap();
    assert_eq!(canceled.status.unwrap().phase, "canceled");
    let resumed = cli.run_job(&case, &job.directory, true).unwrap();
    assert_eq!(resumed.status.unwrap().phase, "completed");
}

#[test]
fn bridge_cancel_command_reaches_a_running_companion_and_native_stage_tree() {
    let work = Workspace::new();
    let case = work.case("active");
    let cli = companion();
    let job = cli
        .import_job_plan(&case, &work.rank_plan(&case, true))
        .unwrap();
    let directory = job.directory.clone();
    let run_case = case.clone();
    let runner = cli.clone();
    let handle = thread::spawn(move || runner.run_job(&run_case, &directory, false));
    let deadline = Instant::now() + Duration::from_secs(15);
    while !job.directory.join("runner.lock/child.json").exists()
        && !handle.is_finished()
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(20));
    }
    let started = job.directory.join("runner.lock/child.json").exists();
    let canceled = cli.cancel_job(&case, &job.directory);
    let finished = handle.join().unwrap();
    assert!(started);
    assert!(canceled.is_ok());
    let result = finished.unwrap();
    assert_eq!(result.status.unwrap().phase, "canceled");
    assert!(!job.directory.join("runner.lock").exists());
}

#[test]
fn bridge_recovery_refuses_live_owner_and_recovers_proven_dead_owner() {
    let work = Workspace::new();
    let case = work.case("recovery");
    let cli = companion();
    let job = cli
        .import_job_plan(&case, &work.rank_plan(&case, false))
        .unwrap();
    let lock = job.directory.join("runner.lock");
    fs::create_dir(&lock).unwrap();
    write_json(
        &lock.join("owner.json"),
        &json!({"pid":std::process::id(),"created_unix_s":1}),
    )
    .unwrap();
    assert!(cli.recover_job(&case, &job.directory).is_err());
    assert!(lock.exists());
    write_json(
        &lock.join("owner.json"),
        &json!({"pid":u32::MAX,"created_unix_s":1}),
    )
    .unwrap();
    let recovered = cli.recover_job(&case, &job.directory).unwrap();
    assert_eq!(recovered.status.unwrap().phase, "interrupted");
    assert!(!lock.exists());
    assert_eq!(
        cli.run_job(&case, &job.directory, true)
            .unwrap()
            .status
            .unwrap()
            .phase,
        "completed"
    );
}

#[test]
fn discovery_preparation_rejects_invalid_channel_and_missing_actual_helpers_before_spawning() {
    let work = Workspace::new();
    let case = work.case("channels");
    let cli = companion();
    let tools = MediaTools {
        ffmpeg: None,
        ffprobe: None,
    };
    assert!(
        cli.prepare_discovery_job(&case, 1, &tools)
            .unwrap_err()
            .contains("channel")
    );
    assert!(
        cli.prepare_discovery_job(&case, 0, &tools)
            .unwrap_err()
            .contains("actual FFmpeg")
    );
    assert!(!case.parent().unwrap().join("jobs").exists());
}

fn write_wave(path: &Path) {
    let rate = 8000u32;
    let samples = rate * 24;
    let size = samples * 2;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + size).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(size.to_le_bytes());
    for n in 0..samples {
        let t = f64::from(n) / f64::from(rate);
        // A downward acoustic sweep with enough measured support for the
        // detector. This is synthetic sound, not evidence of an aircraft.
        let sample =
            (8000.0 * (std::f64::consts::TAU * (280.0 * t - 2.5 * t * t)).sin()).round() as i16;
        bytes.extend(sample.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

#[test]
fn invalid_discovery_sensitivity_is_rejected_before_writing_any_job_or_operation() {
    let work = Workspace::new();
    let case = work.case("invalid-settings");
    let settings = geosonnar::discovery::DiscoverySettings {
        min_contrast: 0.0,
        ..Default::default()
    };
    let error = companion()
        .prepare_discovery_job_with_settings(
            &case,
            0,
            &MediaTools {
                ffmpeg: None,
                ffprobe: None,
            },
            Some(&settings),
        )
        .unwrap_err();
    assert!(error.contains("settings"));
    assert!(!case.parent().unwrap().join("gui-operations").exists());
    assert!(!case.parent().unwrap().join("jobs").exists());
}

#[test]
#[ignore = "Requires installed actual FFmpeg/FFprobe binaries; synthetic audio only"]
fn actual_companion_creates_case_and_discovery_job_then_verifies_review_artifact() {
    let work = Workspace::new();
    let recording = work.path.join("synthetic tone.wav");
    write_wave(&recording);
    let ffmpeg = std::env::var_os("GEOSONNAR_TEST_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("C:/ProgramData/chocolatey/lib/ffmpeg-full/tools/ffmpeg/bin/ffmpeg.exe")
        });
    let probe = std::env::var_os("GEOSONNAR_TEST_FFPROBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("C:/ProgramData/chocolatey/lib/ffmpeg-full/tools/ffmpeg/bin/ffprobe.exe")
        });
    let cli = companion();
    let case = cli
        .create_case_with_probe(
            &recording,
            &work.path.join("real intake"),
            "Synthetic GUI bridge intake",
            Some(&probe),
        )
        .unwrap();
    assert_eq!(case.recording_verified, Some(true));
    let settings = geosonnar::discovery::DiscoverySettings {
        chunk_s: 60.0,
        overlap_s: 30.0,
        min_frequency_hz: 80.0,
        max_frequency_hz: 600.0,
        min_contrast: 3.0,
        min_duration_s: 8.0,
        min_support: 0.6,
        ..Default::default()
    };
    let expected_settings = serde_json::to_value(&settings).unwrap();
    let job = cli
        .prepare_discovery_job_with_settings(
            &case.manifest,
            0,
            &MediaTools {
                ffmpeg: Some(ffmpeg),
                ffprobe: Some(probe),
            },
            Some(&settings),
        )
        .unwrap();
    assert_eq!(job.status.as_ref().unwrap().phase, "pending");
    let manifest = value(&job.directory.join("job.json"));
    let frozen_settings = job.directory.join(
        manifest["plan"]["steps"][0]["action"]["settings"]["path"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(value(&frozen_settings), expected_settings);
    let original_plan = value(&job.directory.join("context/original-plan.json"));
    let original_settings = PathBuf::from(
        original_plan["steps"][0]["action"]["settings"]["path"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(value(&original_settings), expected_settings);
    assert_eq!(
        sha256(&frozen_settings).unwrap(),
        sha256(&original_settings).unwrap()
    );
    let completed = cli.run_job(&case.manifest, &job.directory, false).unwrap();
    assert_eq!(completed.status.unwrap().phase, "completed");
    assert_eq!(completed.discovery_artifacts.len(), 1);
    assert!(!completed.discovery_artifacts[0].verified);
    let artifact =
        gui_bridge::verified_discovery_artifact(&case.manifest, &job.directory, "discover")
            .unwrap();
    assert_eq!(artifact.channel, 0);
    assert_eq!(artifact.sha256, sha256(&artifact.path).unwrap());
    let report = value(&artifact.path);
    assert_eq!(
        report["settings"], expected_settings,
        "GUI sensitivity settings must reach actual discovery unchanged"
    );
    assert_eq!(report["location_verified"], false);
    assert!(
        report["group_count"].as_u64().unwrap() >= 1,
        "Positive acoustic control must produce reviewable events"
    );
    let session = geosonnar::event_review::load(
        &case.manifest,
        &artifact.path,
        &artifact.sha256,
        artifact.channel,
    )
    .unwrap();
    assert!(!session.groups().is_empty());
    assert!(session.overview().measured_point_count >= 3);
    let mut choices = session.initial_choices();
    choices[0].decision = geosonnar::event_review::ReviewDecision::KeepForMatching;
    choices[0].representative_event_id = Some(session.groups()[0].ridges[0].event_id.clone());
    let review = work.path.join("native event review");
    geosonnar::event_review::save(
        &session,
        &choices,
        geosonnar::event_review::ReviewActor::AutomatedTest,
        &review,
    )
    .unwrap();
    assert_eq!(
        geosonnar::event_review::load_choices(&session, &review).unwrap(),
        choices
    );
    fs::write(
        job.directory.join("attempts/discover/000001/stderr.log"),
        b"changed execution log",
    )
    .unwrap();
    assert!(
        gui_bridge::verified_discovery_artifact(&case.manifest, &job.directory, "discover")
            .is_err()
    );
}
