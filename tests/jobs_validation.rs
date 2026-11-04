//! Saved jobs use fabricated local evidence and the real GeoSonnar child CLI.
use geosonnar::{
    casework::{self, CaseMedia, CaseRecording, SavedCase},
    detailed_data::sha256,
    jobs,
    verification::{self, write_json},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
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
            "jobs-control-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&path).unwrap();
        let work = Self { root, path };
        fs::write(
            work.path.join("recording.wav"),
            b"fabricated source identity; not media measurements",
        )
        .unwrap();
        fs::create_dir(work.path.join("case")).unwrap();
        let recording = work.path.join("recording.wav");
        let case = SavedCase {
            schema_version: 1,
            case_id: "synthetic-control".into(),
            title: "Fabricated job case".into(),
            created_unix_s: 1,
            local_only: true,
            recording: CaseRecording {
                path: "../recording.wav".into(),
                bytes: fs::metadata(&recording).unwrap().len(),
                sha256: sha256(&recording).unwrap(),
                media: CaseMedia {
                    duration_s: 10.0,
                    sample_rate_hz: 8000,
                    channels: 1,
                    audio_codec: "pcm_s16le".into(),
                },
            },
        };
        write_json(&work.path.join("case/case.json"), &case).unwrap();
        write_json(
            &work.path.join("search.json"),
            &verification::demo_input().unwrap(),
        )
        .unwrap();
        work
    }
    fn plan(&self, steps: Value) -> PathBuf {
        let path = self
            .path
            .join(format!("plan-{}.json", NEXT.fetch_add(1, Ordering::SeqCst)));
        write_json(
            &path,
            &json!({"schema_version":1,"case_manifest":"case/case.json","steps":steps}),
        )
        .unwrap();
        path
    }
    fn initialize(&self, steps: Value) -> PathBuf {
        let plan = self.plan(steps);
        let directory = self
            .path
            .join(format!("job-{}", NEXT.fetch_add(1, Ordering::SeqCst)));
        let result = create_cli(&plan, &directory);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        directory
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        if let Ok(actual) = self.path.canonicalize() {
            assert!(actual.starts_with(&self.root) && actual != self.root);
            let _ = fs::remove_dir_all(actual);
        }
    }
}

fn create_cli(plan: &Path, out: &Path) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_geosonnar"));
    command
        .arg("job-create")
        .arg("--plan")
        .arg(plan)
        .arg("--out")
        .arg(out);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.output().unwrap()
}
fn rank_steps() -> Value {
    json!([{"id":"rank","action":{"kind":"rank","input":{"source":"file","path":"search.json"}}}])
}
fn read_value(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn successful_job_freezes_context_records_receipts_and_reuses_unchanged_stage() {
    let work = Workspace::new();
    let hint = work.path.join("hint.json");
    write_json(&hint,&json!({"kind":"region","text":"Broad synthetic region only", "source":"user","certainty":"uncertain"})).unwrap();
    casework::add_hint(&work.path.join("case/case.json"), &hint).unwrap();
    let job = work.initialize(rank_steps());
    assert_eq!(fs::read_dir(job.join("context/hints")).unwrap().count(), 1);
    let manifest = read_value(&job.join("job.json"));
    assert_eq!(manifest["location_verified"], false);
    assert!(manifest["original_sources"].as_array().unwrap().len() >= 4);
    assert!(
        manifest["frozen_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"].as_str().unwrap().contains("geosonnar"))
    );
    let state = jobs::run_job(&job, false).unwrap();
    assert_eq!(state.phase, "completed");
    assert!(!state.location_verified);
    let result_path = job.join("attempts/rank/000001/artifacts/results.json");
    let result = read_value(&result_path);
    assert_eq!(result["location_verified"], false);
    assert!(result["ranked_count"].as_u64().unwrap() > 0);
    let attempt = read_value(&job.join("attempts/rank/000001/result.json"));
    assert_eq!(attempt["inputs"].as_array().unwrap().len(), 1);
    assert_eq!(attempt["artifacts"].as_array().unwrap().len(), 1);
    let bytes = fs::read(&result_path).unwrap();
    // Appending a new hint does not silently revise an existing job's context.
    casework::add_hint(&work.path.join("case/case.json"), &hint).unwrap();
    let resumed = jobs::run_job(&job, true).unwrap();
    assert_eq!(resumed.phase, "completed");
    assert!(resumed.steps[0].reused);
    assert_eq!(resumed.steps[0].attempt, 1);
    assert!(!job.join("attempts/rank/000002").exists());
    assert_eq!(fs::read(&result_path).unwrap(), bytes);
    assert_eq!(fs::read_dir(job.join("context/hints")).unwrap().count(), 1);
    assert!(jobs::run_job(&job, false).is_err());
}

#[test]
fn changed_or_added_artifacts_create_new_attempts_and_invalidate_downstream() {
    for addition in [false, true] {
        let work = Workspace::new();
        let job = work.initialize(json!([
            {"id":"first","action":{"kind":"rank","input":{"source":"file","path":"search.json"}}},
            {"id":"second","action":{"kind":"rank","input":{"source":"file","path":"search.json"}}}
        ]));
        assert_eq!(jobs::run_job(&job, false).unwrap().phase, "completed");
        let first = job.join("attempts/first/000001/artifacts/results.json");
        if addition {
            fs::write(
                first.parent().unwrap().join("added.txt"),
                b"unexpected artifact",
            )
            .unwrap();
        } else {
            fs::write(&first, b"externally changed result").unwrap();
        }
        let old = fs::read(&first).unwrap();
        let second_original =
            fs::read(job.join("attempts/second/000001/artifacts/results.json")).unwrap();
        let state = jobs::run_job(&job, true).unwrap();
        assert_eq!(state.phase, "completed");
        assert_eq!(
            state.steps.iter().map(|s| s.attempt).collect::<Vec<_>>(),
            [2, 2]
        );
        assert!(state.steps.iter().all(|s| !s.reused));
        assert_eq!(fs::read(&first).unwrap(), old);
        assert_eq!(
            fs::read(job.join("attempts/second/000001/artifacts/results.json")).unwrap(),
            second_original
        );
        let invalidations = fs::read_dir(job.join("history"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(invalidations.len(), 2);
        assert!(
            invalidations
                .iter()
                .any(|p| read_value(p)["upstream_invalidated"] == true)
        );
    }
}

#[test]
fn changed_source_configuration_runtime_and_frozen_context_block_resume() {
    for kind in ["recording", "configuration", "runtime", "context"] {
        let work = Workspace::new();
        let job = work.initialize(rank_steps());
        assert_eq!(jobs::run_job(&job, false).unwrap().phase, "completed");
        let path = match kind {
            "recording" => work.path.join("recording.wav"),
            "configuration" => work.path.join("search.json"),
            "runtime" => job.join(if cfg!(windows) {
                "bin/geosonnar.exe"
            } else {
                "bin/geosonnar"
            }),
            "context" => job.join("context/case.json"),
            _ => unreachable!(),
        };
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 1;
        fs::write(&path, bytes).unwrap();
        let state = jobs::run_job(&job, true).unwrap();
        assert_eq!(state.phase, "failed", "{kind}");
        assert!(state.message.unwrap().contains("changed"));
        assert!(!job.join("attempts/rank/000002").exists());
        assert_eq!(
            read_value(&job.join("attempts/rank/000001/result.json"))["state"],
            "completed"
        );
    }
}

#[test]
fn terrain_dependencies_are_bound_even_before_the_terrain_stage_runs() {
    let work = Workspace::new();
    fs::write(
        work.path.join("dem.tif"),
        b"fake DEM bytes for identity binding only",
    )
    .unwrap();
    fs::write(
        work.path.join("geoid.tif"),
        b"fake geoid bytes for identity binding only",
    )
    .unwrap();
    write_json(&work.path.join("terrain.json"),&json!({"dem":"dem.tif","geoid":"geoid.tif","observer_bounds":[0.0,0.0,0.01,0.01],"grid_spacing_m":100.0,"microphone_height_m":1.0,"vertical_datum_assumption":"Synthetic unit contract only"})).unwrap();
    let job=work.initialize(json!([{"id":"terrain","action":{"kind":"terrain_grid","config":{"source":"file","path":"terrain.json"}}}]));
    let manifest = read_value(&job.join("job.json"));
    for name in ["dem.tif", "geoid.tif"] {
        assert!(
            manifest["original_sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["path"].as_str().unwrap().ends_with(name))
        );
    }
    fs::write(work.path.join("geoid.tif"), b"changed geoid").unwrap();
    let state = jobs::run_job(&job, false).unwrap();
    assert_eq!(state.phase, "failed");
    assert!(state.message.unwrap().contains("geoid.tif"));
    assert!(!job.join("attempts").exists());
}

#[test]
fn failed_stage_retries_without_overwriting_attempt_evidence() {
    let work = Workspace::new();
    let demo = serde_json::to_value(verification::demo_input().unwrap()).unwrap();
    let mut input = json!({"schema_version":1,"evidence_kind":"synthetic","provenance":["fabricated failed-stage control"],"tracks":demo["tracks"],"observers":demo["observers"],"event":demo["events"][0],"model":demo["pair_config"]["model"],"clock_offsets_s":[0.0],"coarse_bounds":null});
    input["event"]["f0_hz"] = json!([]);
    write_json(&work.path.join("single.json"), &input).unwrap();
    let job=work.initialize(json!([{"id":"bad","action":{"kind":"rank_single","input":{"source":"file","path":"single.json"}}}]));
    let failed = jobs::run_job(&job, false).unwrap();
    assert_eq!(failed.phase, "failed");
    let first = job.join("attempts/bad/000001");
    let receipt = fs::read(first.join("result.json")).unwrap();
    assert!(!fs::read(first.join("stderr.log")).unwrap().is_empty());
    assert_eq!(read_value(&first.join("result.json"))["exit_code"], 2);
    let retry = jobs::run_job(&job, true).unwrap();
    assert_eq!(retry.phase, "failed");
    assert_eq!(retry.steps[0].attempt, 2);
    assert_eq!(fs::read(first.join("result.json")).unwrap(), receipt);
    assert!(job.join("attempts/bad/000002/stderr.log").exists());
}

#[test]
fn cancellation_before_start_is_durable_and_resume_explicitly_starts_work() {
    let work = Workspace::new();
    let job = work.initialize(rank_steps());
    assert!(jobs::cancel_job(&job).unwrap().cancel_requested);
    assert_eq!(jobs::run_job(&job, false).unwrap().phase, "canceled");
    assert!(!job.join("attempts").exists());
    assert_eq!(jobs::run_job(&job, true).unwrap().phase, "completed");
    assert!(!job.join("cancel.json").exists());
    assert!(fs::read_dir(job.join("history")).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("cancel-")
    }));
    assert!(jobs::cancel_job(&job).is_err());

    let pending = work.initialize(rank_steps());
    jobs::cancel_job(&pending).unwrap();
    assert_eq!(jobs::run_job(&pending, true).unwrap().phase, "completed");
    assert!(!pending.join("cancel.json").exists());
}

#[test]
fn cancellation_stops_a_live_child_and_preserves_partial_attempt() {
    let work = Workspace::new();
    let mut input = verification::demo_input().unwrap();
    let first = input.observers[0].clone();
    input.observers = (0..5000)
        .map(|n| {
            let mut observer = first.clone();
            observer.id = format!("control-{n}");
            observer
        })
        .collect();
    write_json(&work.path.join("search.json"), &input).unwrap();
    let job = work.initialize(rank_steps());
    let running = job.clone();
    let handle = thread::spawn(move || jobs::run_job(&running, false));
    let deadline = Instant::now() + Duration::from_secs(15);
    while !job.join("runner.lock/child.json").exists()
        && !handle.is_finished()
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        job.join("runner.lock/child.json").exists(),
        "Child did not start in time"
    );
    assert!(
        jobs::run_job(&job, true).is_err(),
        "A second runner must be rejected"
    );
    assert!(
        jobs::recover_job(&job).is_err(),
        "A live owner cannot be recovered"
    );
    jobs::cancel_job(&job).unwrap();
    let state = handle.join().unwrap().unwrap();
    assert_eq!(state.phase, "canceled", "{state:?}");
    assert_eq!(state.steps[0].attempt, 1);
    assert!(!job.join("runner.lock").exists());
    let attempt = job.join("attempts/rank/000001");
    assert_eq!(
        read_value(&attempt.join("result.json"))["state"],
        "canceled"
    );
    for file in [
        "start.json",
        "child.json",
        "command.json",
        "stdout.log",
        "stderr.log",
    ] {
        assert!(attempt.join(file).exists());
    }
}

#[test]
fn explicit_dead_owner_recovery_preserves_interrupted_attempt_and_live_owners_block() {
    let work = Workspace::new();
    let job = work.initialize(rank_steps());
    let lock = job.join("runner.lock");
    fs::create_dir(&lock).unwrap();
    write_json(
        &lock.join("owner.json"),
        &json!({"pid":std::process::id(),"created_unix_s":1}),
    )
    .unwrap();
    let owner = fs::read(lock.join("owner.json")).unwrap();
    assert!(jobs::recover_job(&job).unwrap_err().contains("alive"));
    assert_eq!(fs::read(lock.join("owner.json")).unwrap(), owner);
    assert!(jobs::run_job(&job, false).is_err());
    write_json(
        &lock.join("owner.json"),
        &json!({"pid":0,"created_unix_s":1}),
    )
    .unwrap();
    assert!(
        jobs::recover_job(&job)
            .unwrap_err()
            .contains("Invalid runner PID")
    );
    write_json(
        &lock.join("owner.json"),
        &json!({"pid":u32::MAX,"created_unix_s":1}),
    )
    .unwrap();
    let partial = job.join("attempts/rank/000001");
    fs::create_dir_all(&partial).unwrap();
    fs::write(partial.join("partial.txt"), b"interrupted evidence").unwrap();
    let mut state = jobs::status(&job).unwrap();
    state.phase = "running".into();
    state.steps[0].state = "running".into();
    state.steps[0].attempt = 1;
    write_json(&job.join("state/0000000002.json"), &state).unwrap();
    assert_eq!(jobs::recover_job(&job).unwrap().phase, "interrupted");
    assert!(!lock.exists());
    let resumed = jobs::run_job(&job, true).unwrap();
    assert_eq!(resumed.phase, "completed");
    assert_eq!(resumed.steps[0].attempt, 2);
    assert_eq!(
        fs::read(partial.join("partial.txt")).unwrap(),
        b"interrupted evidence"
    );
    assert!(fs::read_dir(job.join("history")).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("recovered-lock-")
    }));
}

#[test]
fn unsupported_commands_forward_references_and_unsafe_artifact_paths_are_rejected() {
    let work = Workspace::new();
    for action in [
        json!({"kind":"shell","command":"anything"}),
        json!({"kind":"rank","input":{"source":"step","step_id":"later","artifact":"results.json"}}),
    ] {
        let plan = work.plan(json!([{"id":"first","action":action}]));
        let out = work
            .path
            .join(format!("invalid-{}", NEXT.fetch_add(1, Ordering::SeqCst)));
        assert!(!create_cli(&plan, &out).status.success());
        assert!(!out.exists());
    }
    for path in ["../outside.json", "/outside.json", "C:/outside.json"] {
        let plan=work.plan(json!([
            {"id":"first","action":{"kind":"rank","input":{"source":"file","path":"search.json"}}},
            {"id":"second","action":{"kind":"rank","input":{"source":"step","step_id":"first","artifact":path}}}
        ]));
        let out = work
            .path
            .join(format!("invalid-{}", NEXT.fetch_add(1, Ordering::SeqCst)));
        assert!(!create_cli(&plan, &out).status.success());
        assert!(!out.exists());
    }
    let job = work.initialize(rank_steps());
    let original = fs::read(job.join("job.json")).unwrap();
    assert!(!create_cli(&work.plan(rank_steps()), &job).status.success());
    assert_eq!(fs::read(job.join("job.json")).unwrap(), original);
}

#[test]
fn previous_step_artifacts_still_have_to_satisfy_the_next_native_input_contract() {
    let work = Workspace::new();
    let job=work.initialize(json!([
        {"id":"first","action":{"kind":"rank","input":{"source":"file","path":"search.json"}}},
        {"id":"second","action":{"kind":"rank","input":{"source":"step","step_id":"first","artifact":"results.json"}}}
    ]));
    let state = jobs::run_job(&job, false).unwrap();
    assert_eq!(state.phase, "failed");
    assert_eq!(state.steps[0].state, "completed");
    assert_eq!(state.steps[1].state, "failed");
    assert!(
        read_value(&job.join("attempts/second/000001/result.json"))["error"]
            .as_str()
            .unwrap()
            .contains("unknown field")
    );
    let retry = jobs::run_job(&job, true).unwrap();
    assert!(retry.steps[0].reused);
    assert_eq!(retry.steps[1].attempt, 2);
}

#[test]
fn rank_to_overlays_records_actual_dependency_and_rebuilds_both_after_changed_result() {
    let work = Workspace::new();
    let job=work.initialize(json!([
        {"id":"rank","action":{"kind":"rank","input":{"source":"file","path":"search.json"}}},
        {"id":"curves","action":{"kind":"overlays","input":{"source":"file","path":"search.json"},"results":{"source":"step","step_id":"rank","artifact":"results.json"},"top":2}}
    ]));
    let completed = jobs::run_job(&job, false).unwrap();
    assert_eq!(completed.phase, "completed");
    let curves = job.join("attempts/curves/000001/artifacts/overlays.json");
    let original = fs::read(&curves).unwrap();
    let attempt = read_value(&job.join("attempts/curves/000001/result.json"));
    assert!(attempt["inputs"].as_array().unwrap().iter().any(|input| {
        input["path"]
            .as_str()
            .unwrap()
            .replace('\\', "/")
            .ends_with("attempts/rank/000001/artifacts/results.json")
    }));
    let reused = jobs::run_job(&job, true).unwrap();
    assert!(reused.steps.iter().all(|s| s.reused));
    let first_result = job.join("attempts/rank/000001/artifacts/results.json");
    fs::write(&first_result, b"externally changed ranked report").unwrap();
    let rebuilt = jobs::run_job(&job, true).unwrap();
    assert_eq!(rebuilt.phase, "completed");
    assert!(rebuilt.steps.iter().all(|s| s.attempt == 2 && !s.reused));
    assert_eq!(fs::read(&curves).unwrap(), original);
    assert_eq!(
        fs::read(first_result).unwrap(),
        b"externally changed ranked report"
    );
    assert!(
        job.join("attempts/curves/000002/artifacts/overlays.json")
            .exists()
    );
    let invalid=work.plan(json!([{"id":"bad","action":{"kind":"overlays","input":{"source":"file","path":"search.json"},"results":{"source":"file","path":"unused.json"},"top":11}}]));
    assert!(
        !create_cli(&invalid, &work.path.join("too-many-overlays"))
            .status
            .success()
    );
}

#[test]
fn changed_attempt_logs_or_receipt_cannot_be_reused() {
    for filename in ["stderr.log", "result.json"] {
        let work = Workspace::new();
        let job = work.initialize(rank_steps());
        assert_eq!(jobs::run_job(&job, false).unwrap().phase, "completed");
        let path = job.join("attempts/rank/000001").join(filename);
        let mut changed = fs::read(&path).unwrap();
        changed.extend(b" \n");
        fs::write(&path, &changed).unwrap();
        let state = jobs::run_job(&job, true).unwrap();
        assert_eq!(state.phase, "completed");
        assert_eq!(state.steps[0].attempt, 2);
        assert!(!state.steps[0].reused);
        assert_eq!(fs::read(&path).unwrap(), changed);
    }
}

#[test]
fn invalid_hints_and_unbound_media_helpers_cannot_enter_a_saved_job() {
    let work = Workspace::new();
    fs::create_dir(work.path.join("case/hints")).unwrap();
    fs::write(work.path.join("case/hints/foreign.json"), b"{}").unwrap();
    let plan = work.plan(rank_steps());
    let out = work.path.join("invalid-hints");
    assert!(!create_cli(&plan, &out).status.success());
    assert!(!out.exists());
    fs::remove_file(work.path.join("case/hints/foreign.json")).unwrap();
    let media=work.plan(json!([{"id":"inspect","action":{"kind":"inspect","input":{"source":"file","path":"recording.wav"}}}]));
    let missing = work.path.join("missing-helper");
    let result = create_cli(&media, &missing);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("media_tools"));
    fs::write(
        work.path.join("shim.exe"),
        b"synthetic Chocolatey ShimGen wrapper signature",
    )
    .unwrap();
    let mut configured = read_value(&media);
    configured["media_tools"] = json!({"ffprobe":"shim.exe","ffmpeg":null});
    write_json(&media, &configured).unwrap();
    let shim = work.path.join("rejected-shim");
    let result = create_cli(&media, &shim);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("shim"));
}

fn gated_command(output: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_geosonnar"));
    command
        .args(["demo", "--out"])
        .arg(output)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}

#[test]
fn child_gate_rejects_partial_mismatched_and_dead_parent_context_before_any_work() {
    let work = Workspace::new();
    for mode in ["missing_path", "missing_nonce", "mismatch", "dead_parent"] {
        let output = work.path.join(mode);
        let permit = work.path.join(format!("{mode}-permit.json"));
        let mut command = gated_command(&output);
        if mode != "missing_path" {
            command.env("GEOSONNAR_JOB_GATE", &permit);
        }
        if mode != "missing_nonce" {
            command.env("GEOSONNAR_JOB_NONCE", "expected");
        }
        command.env(
            "GEOSONNAR_JOB_PARENT",
            if mode == "dead_parent" {
                u32::MAX
            } else {
                std::process::id()
            }
            .to_string(),
        );
        if mode == "mismatch" {
            write_json(
                &permit,
                &json!({"nonce":"different","parent_pid":std::process::id()}),
            )
            .unwrap();
        }
        let start = Instant::now();
        let result = command.output().unwrap();
        assert!(!result.status.success(), "{mode}");
        assert!(!output.exists());
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "Invalid gate should fail promptly: {mode}"
        );
    }
}

#[test]
fn child_gate_waits_for_parent_permit_before_stage_execution() {
    let work = Workspace::new();
    let output = work.path.join("gated-demo");
    let permit = work.path.join("permit.json");
    let mut command = gated_command(&output);
    command
        .env("GEOSONNAR_JOB_GATE", &permit)
        .env("GEOSONNAR_JOB_NONCE", "specific-test-permit")
        .env("GEOSONNAR_JOB_PARENT", std::process::id().to_string());
    let mut child = command.spawn().unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(!output.exists());
    assert!(child.try_wait().unwrap().is_none());
    // Publish atomically, just as the runner does after process-tree assignment.
    let temporary = work.path.join("pending-permit.json");
    write_json(
        &temporary,
        &json!({"nonce":"specific-test-permit","parent_pid":std::process::id()}),
    )
    .unwrap();
    fs::rename(temporary, &permit).unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.join("synthetic_results.json").exists());
}

#[test]
fn child_gate_times_out_without_running_an_unpermitted_stage() {
    let work = Workspace::new();
    let output = work.path.join("never-permitted");
    let mut command = gated_command(&output);
    command
        .env("GEOSONNAR_JOB_GATE", work.path.join("absent-permit.json"))
        .env("GEOSONNAR_JOB_NONCE", "never")
        .env("GEOSONNAR_JOB_PARENT", std::process::id().to_string());
    let result = command.output().unwrap();
    assert!(!result.status.success());
    assert!(!output.exists());
    assert!(String::from_utf8_lossy(&result.stderr).contains("within ten seconds"));
}
