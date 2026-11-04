//! Frontend lifecycle controls use only isolated fabricated case/review files.
//! No window, file picker, preferences file, media tool or job runner is launched.
use super::*;
use geosonnar::{
    casework::{CaseMedia, CaseRecording, SavedCase},
    jobs::{JobStatus, StepStatus},
    verification::write_json,
};
use serde_json::json;
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn startup_preferences_bound_bytes_and_recent_cases_without_changing_valid_choices() {
    let fixture = Fixture::new();
    let path = fixture.path.join("settings.json");
    let chosen = Preferences {
        dark_mode: true,
        detailed_controls: true,
        ffmpeg: "chosen-ffmpeg.exe".into(),
        ffprobe: "chosen-ffprobe.exe".into(),
        ffplay: "chosen-ffplay.exe".into(),
        recent_cases: (0..100)
            .map(|i| PathBuf::from(format!("case-{i}.json")))
            .collect(),
    };
    fs::write(&path, serde_json::to_vec(&chosen).unwrap()).unwrap();
    let restored = read_preferences(&path);
    assert!(restored.dark_mode);
    assert!(restored.detailed_controls);
    assert!(
        !serde_json::from_str::<Preferences>("{}")
            .unwrap()
            .detailed_controls
    );
    assert_eq!(restored.ffmpeg, chosen.ffmpeg);
    assert_eq!(restored.ffprobe, chosen.ffprobe);
    assert_eq!(restored.ffplay, chosen.ffplay);
    assert_eq!(
        restored.recent_cases,
        chosen.recent_cases[..MAX_RECENT_CASES]
    );

    // Valid JSON at the byte boundary remains supported; one extra byte rejects
    // the file without rewriting it or partially applying its settings.
    let mut boundary = serde_json::to_vec(&chosen).unwrap();
    boundary.resize(MAX_PREFERENCES_BYTES, b' ');
    fs::write(&path, &boundary).unwrap();
    assert!(read_preferences(&path).dark_mode);
    boundary.push(b' ');
    fs::write(&path, &boundary).unwrap();
    let rejected = read_preferences(&path);
    assert!(!rejected.dark_mode && rejected.ffmpeg.is_empty() && rejected.recent_cases.is_empty());
    assert_eq!(fs::read(&path).unwrap(), boundary);
    fs::write(&path, b"{malformed").unwrap();
    assert!(read_preferences(&path).recent_cases.is_empty());
    assert!(
        read_preferences(&fixture.path.join("missing.json"))
            .ffmpeg
            .is_empty()
    );
}

#[test]
fn empty_startup_settles_without_media_jobs_unbounded_rendering_or_immediate_repaints() {
    use eframe::App;
    let fixture = Fixture::new();
    let preferences_path = fixture.path.join("geosonnar-settings.json");
    let preferences = Preferences {
        dark_mode: false,
        detailed_controls: false,
        ffmpeg: "unused-ffmpeg.exe".into(),
        ffprobe: "unused-ffprobe.exe".into(),
        ffplay: "unused-ffplay.exe".into(),
        recent_cases: (0..8)
            .map(|i| fixture.path.join(format!("missing-case-{i}.json")))
            .collect(),
    };
    fs::write(&preferences_path, serde_json::to_vec(&preferences).unwrap()).unwrap();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    // Production construction with an isolated executable/settings location;
    // no native window or graphics adapter is initialized by this test.
    let mut app =
        Workbench::with_executable(&creation, fixture.path.join("geosonnar-gui.exe"), None);
    let mut native_frame = eframe::Frame::_new_kittest();
    let mut total_texture_pixels = 0usize;
    let mut last_metadata_check_frame = 0usize;
    for frame_index in 0..120 {
        // One local metadata check may request up to four completion repaints;
        // this is separate from decoding, processing jobs and a continuous loop.
        if app.media_setup.check_in_progress() {
            last_metadata_check_frame = frame_index;
        }
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 820.0),
                )),
                time: Some(frame_index as f64 * 0.5),
                ..Default::default()
            },
            |ui| {
                app.logic(ui.ctx(), &mut native_frame);
                app.ui(ui, &mut native_frame);
            },
        );
        assert!(app.case.is_none() && app.jobs.is_empty() && app.review.is_none());
        assert!(app.idle() && !app.polling && !app.canceling);
        assert!(app.receiver.try_recv().is_err());
        app.studio.assert_unloaded_idle();
        assert!(
            output.shapes.len() < 1500,
            "Unexpected empty-screen shape growth"
        );
        assert_eq!(output.viewport_output.len(), 1);
        for (id, delta) in &output.textures_delta.set {
            assert_eq!(
                *id,
                egui::TextureId::Managed(0),
                "Only the font atlas belongs on the initial screen"
            );
            let [width, height] = delta.image.size();
            total_texture_pixels += width * height;
        }
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        let vertices: usize = primitives
            .iter()
            .map(|primitive| match &primitive.primitive {
                egui::epaint::Primitive::Mesh(mesh) => mesh.vertices.len(),
                egui::epaint::Primitive::Callback(_) => {
                    panic!("Unexpected GPU callback on the empty screen")
                }
            })
            .sum();
        assert!(
            vertices < 100_000,
            "Unexpected empty-screen tessellation growth"
        );
        if frame_index >= 10 && frame_index > last_metadata_check_frame + 2 {
            let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            assert!(
                delay >= Duration::from_millis(250),
                "Idle repaint loop: {delay:?}"
            );
        }
    }
    assert!(
        total_texture_pixels <= 4 * 1024 * 1024,
        "Unexpected repeated font uploads"
    );
    assert!(matches!(app.page, Page::Intake));
    assert_eq!(app.preferences.recent_cases, preferences.recent_cases);
}

#[test]
fn workbench_pages_follow_repeated_window_resizes_without_expanding_the_viewport() {
    use eframe::App;
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let mut case = fixture.case("resize");
    case.case.title = "A long recording title with detailed date and place clues ".repeat(12);
    app.studio.bind(&case);
    app.case = Some(case);
    app.companion = Err("A deliberately long tool path is unavailable: ".to_owned()
        + &r"\\?\C:\recordings\a long folder name\".repeat(8));
    app.preferences.ffmpeg = r"\\?\C:\long folder\tools\ffmpeg.exe".into();
    let ctx = egui::Context::default();
    theme(&ctx, false);
    let mut native_frame = eframe::Frame::_new_kittest();
    for page in [
        Page::Intake,
        Page::Review,
        Page::Data,
        Page::Map,
        Page::Results,
        Page::Jobs,
        Page::Settings,
    ] {
        app.page = page;
        for size in [
            egui::vec2(1600.0, 1000.0),
            egui::vec2(760.0, 560.0),
            egui::vec2(1100.0, 720.0),
            egui::vec2(760.0, 560.0),
        ] {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            // Reuse context/panel state: shrinking after widening is the regression.
            for _ in 0..3 {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        app.ui(ui, &mut native_frame);
                        let used = ui.min_rect();
                        assert!(
                            used.max.x <= screen.max.x + 1.0,
                            "page={} size={size:?} rect={used:?}",
                            page as u8
                        );
                        assert!(
                            used.max.y <= screen.max.y + 1.0,
                            "page={} size={size:?} rect={used:?}",
                            page as u8
                        );
                    },
                );
            }
        }
    }
    assert_eq!(
        app.preferences.ffmpeg,
        r"\\?\C:\long folder\tools\ffmpeg.exe"
    );
}

struct Fixture {
    root: PathBuf,
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let path = root.join(format!(
            "gui-lifecycle-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&path).unwrap();
        Self { root, path }
    }
    fn case(&self, id: &str) -> CaseOverview {
        let directory = self.path.join(id);
        fs::create_dir(&directory).unwrap();
        let recording = directory.join("fabricated.raw");
        fs::write(
            &recording,
            b"Fabricated lifecycle evidence; no real recording or location",
        )
        .unwrap();
        let case = SavedCase {
            schema_version: 1,
            case_id: id.into(),
            title: format!("Synthetic {id}"),
            created_unix_s: 1,
            local_only: true,
            recording: CaseRecording {
                path: "fabricated.raw".into(),
                bytes: fs::metadata(&recording).unwrap().len(),
                sha256: sha256(&recording).unwrap(),
                media: CaseMedia {
                    duration_s: 60.0,
                    sample_rate_hz: 8000,
                    channels: 1,
                    audio_codec: "synthetic".into(),
                },
            },
        };
        let manifest = directory.join("case.json");
        write_json(&manifest, &case).unwrap();
        CaseOverview {
            manifest,
            case,
            hints: Vec::new(),
            recording_path: recording,
            recording_verified: Some(true),
            recording_problem: None,
        }
    }
    fn app(&self) -> Workbench {
        let (sender, receiver) = mpsc::channel();
        Workbench {
            page: Page::Intake,
            companion: Err("No companion is invoked by lifecycle controls".into()),
            preferences: Preferences::default(),
            preferences_path: self.path.join("test-preferences.json"),
            media_setup: media_setup::MediaSetup::default(),
            case: None,
            jobs: Vec::new(),
            selected_job: None,
            review: None,
            studio: studio::Studio::default(),
            channel: 0,
            title: String::new(),
            recording: String::new(),
            case_parent: String::new(),
            hint_kind: 0,
            hint_text: String::new(),
            hint_reported: false,
            busy: None,
            running: None,
            canceling: false,
            // A fabricated in-flight poll prevents update_messages from launching
            // filesystem workers; explicit poll tests also keep the deadline ahead.
            polling: true,
            next_poll: Instant::now() + Duration::from_secs(3600),
            notice: None,
            close_requested: false,
            shutdown: shutdown::ExitState::default(),
            allow_close: false,
            sender,
            receiver,
        }
    }
    fn review(&self, case: &CaseOverview) -> ReviewView {
        let directory = self.path.join(unique_name("review"));
        fs::create_dir(&directory).unwrap();
        let event = |id: &str, group: &str, start: f64, frequency: f64| {
            json!({
            "event_id":id,"harmonic_group_id":group,"start_s":start-1.0,"end_s":start+4.0,
            "band_hz":[frequency-25.0,frequency+5.0],
            "ridge":[{"time_s":start,"frequency_hz":frequency,"weight":1.0},
                {"time_s":start+1.0,"frequency_hz":frequency-10.0,"weight":1.0},
                    {"time_s":start+2.0,"frequency_hz":frequency-20.0,"weight":1.0}],"quality":{"support_fraction":0.4,"fragment_count":1,"frequency_sigma_hz":0.5}})
        };
        let report = json!({"schema_version":1,"status":"events_proposed","location_verified":false,
            "source":case.recording_path,"source_sha256":case.case.recording.sha256,
            "source_metadata":{"duration_s":60.0,"selected_channel_index":0,"available_channels":1,"channels_averaged":false},
            "settings":{"min_frequency_hz":40.0,"max_frequency_hz":1600.0,"max_gap_s":2.0},"group_count":2,"ridge_count":2,
            "groups":[{"harmonic_group_id":"group-a","start_s":9.0,"end_s":14.0,"ridge_count":1,"review_status":"unreviewed","aircraft_identity_confirmed":false},
                {"harmonic_group_id":"group-b","start_s":19.0,"end_s":24.0,"ridge_count":1,"review_status":"unreviewed","aircraft_identity_confirmed":false}],
            "events":[event("ridge-a","group-a",10.0,120.0),event("ridge-b","group-b",20.0,180.0)],
            "limitations":["Fabricated GUI lifecycle fixture"]});
        for event in report["events"].as_array().unwrap() {
            let mut csv = String::from("time_s,frequency_hz,weight\n");
            for point in event["ridge"].as_array().unwrap() {
                csv.push_str(&format!(
                    "{},{},{}\n",
                    point["time_s"], point["frequency_hz"], point["weight"]
                ));
            }
            fs::write(
                directory.join(format!("{}.csv", event["event_id"].as_str().unwrap())),
                csv,
            )
            .unwrap();
        }
        let discovery = directory.join("discovery.json");
        write_json(&discovery, &report).unwrap();
        let session =
            event_review::load(&case.manifest, &discovery, &sha256(&discovery).unwrap(), 0)
                .unwrap();
        let choices = session.initial_choices();
        ReviewView {
            session: Arc::new(session),
            binding: None,
            choices: choices.clone(),
            saved_choices: choices,
            selected: 0,
            saved_path: None,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(path) = self.path.canonicalize() {
            assert!(path.starts_with(&self.root) && path != self.root);
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn job(case: &CaseOverview, name: &str, phase: &str) -> JobOverview {
    JobOverview {
        directory: case.manifest.parent().unwrap().join("jobs").join(name),
        status: Some(JobStatus {
            schema_version: 1,
            phase: phase.into(),
            updated_unix_s: 1,
            steps: vec![StepStatus {
                id: "discover".into(),
                state: phase.into(),
                attempt: 1,
                reused: false,
                receipt_sha256: Some("0".repeat(64)),
            }],
            message: None,
            cancel_requested: phase == "canceled",
            location_verified: false,
        }),
        discovery_artifacts: Vec::new(),
        problem: None,
    }
}

fn send(app: &Workbench, lane: Lane, case: &CaseOverview, result: Result<Payload, String>) {
    app.sender
        .send(Message {
            lane,
            case_manifest: Some(case.manifest.clone()),
            result,
        })
        .unwrap();
}

fn frame(app: &mut Workbench, ctx: &egui::Context, close_requested: bool) -> egui::FullOutput {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 700.0),
        )),
        ..Default::default()
    };
    if close_requested {
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
    }
    ctx.begin_pass(input);
    app.update_messages(ctx);
    app.closing(ctx);
    ctx.end_pass()
}

fn contains_close(output: &egui::FullOutput) -> bool {
    output.viewport_output.values().any(|v| {
        v.commands
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::Close))
    })
}
fn contains_cancel_close(output: &egui::FullOutput) -> bool {
    output.viewport_output.values().any(|v| {
        v.commands
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::CancelClose))
    })
}

#[test]
fn stale_poll_rows_cannot_replace_jobs_or_selection_for_another_case() {
    let fixture = Fixture::new();
    let old = fixture.case("old");
    let selected = fixture.case("selected");
    let mut app = fixture.app();
    let current_job = job(&selected, "current", "running");
    app.case = Some(selected.clone());
    app.jobs = vec![current_job.clone()];
    app.selected_job = Some(current_job.directory.clone());
    app.notice = Some((false, "Selected case is open".into()));
    send(
        &app,
        Lane::Poll,
        &old,
        Ok(Payload::Jobs(vec![job(&old, "obsolete", "completed")])),
    );
    let _ = frame(&mut app, &egui::Context::default(), false);
    assert_eq!(app.case.as_ref().unwrap().manifest, selected.manifest);
    assert_eq!(app.jobs.len(), 1);
    assert_eq!(app.jobs[0].directory, current_job.directory);
    assert_eq!(app.selected_job, Some(current_job.directory));
    assert_eq!(app.notice, Some((false, "Selected case is open".into())));
}

#[test]
fn stale_poll_error_cannot_overwrite_the_new_cases_notice_or_work_lane() {
    let fixture = Fixture::new();
    let old = fixture.case("old");
    let selected = fixture.case("selected");
    let mut app = fixture.app();
    app.case = Some(selected.clone());
    app.busy = Some("Saving this case's clue".into());
    app.notice = Some((false, "Current case notice".into()));
    send(&app, Lane::Poll, &old, Err("Old case was moved".into()));
    let _ = frame(&mut app, &egui::Context::default(), false);
    assert_eq!(app.notice, Some((false, "Current case notice".into())));
    assert_eq!(app.busy.as_deref(), Some("Saving this case's clue"));
    assert_eq!(app.case.as_ref().unwrap().manifest, selected.manifest);
}

#[test]
fn matching_case_poll_updates_progress_without_clearing_foreground_work() {
    let fixture = Fixture::new();
    let case = fixture.case("current-poll");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.jobs = vec![job(&case, "same-job", "pending")];
    app.busy = Some("Saving a clue".into());
    app.notice = Some((false, "Current notice".into()));
    send(
        &app,
        Lane::Poll,
        &case,
        Ok(Payload::Jobs(vec![job(&case, "same-job", "running")])),
    );
    let _ = frame(&mut app, &egui::Context::default(), false);
    assert_eq!(app.jobs[0].status.as_ref().unwrap().phase, "running");
    assert_eq!(app.busy.as_deref(), Some("Saving a clue"));
    assert_eq!(app.notice, Some((false, "Current notice".into())));
}

#[test]
fn cancel_work_and_run_completions_clear_only_their_own_activity_lane() {
    let fixture = Fixture::new();
    let case = fixture.case("lanes");
    let mut app = fixture.app();
    let running = job(&case, "job", "running");
    app.case = Some(case.clone());
    app.running = Some(running.directory.clone());
    app.busy = Some("Another foreground operation".into());
    app.canceling = true;
    let ctx = egui::Context::default();
    send(
        &app,
        Lane::Cancel,
        &case,
        Ok(Payload::Job(job(&case, "job", "canceled"))),
    );
    let _ = frame(&mut app, &ctx, false);
    assert!(!app.canceling);
    assert_eq!(app.running, Some(running.directory.clone()));
    assert_eq!(app.busy.as_deref(), Some("Another foreground operation"));
    assert!(!app.idle());
    app.canceling = true;
    send(
        &app,
        Lane::Cancel,
        &case,
        Err("Cancellation could not be requested".into()),
    );
    let _ = frame(&mut app, &ctx, false);
    assert!(!app.canceling);
    assert_eq!(app.running, Some(running.directory.clone()));
    assert!(!app.shutdown.requested());
    send(
        &app,
        Lane::Work,
        &case,
        Err("Foreground operation failed independently".into()),
    );
    let _ = frame(&mut app, &ctx, false);
    assert!(app.busy.is_none());
    assert_eq!(app.running, Some(running.directory));
    assert!(!app.idle());
    send(
        &app,
        Lane::Run,
        &case,
        Ok(Payload::Job(job(&case, "job", "canceled"))),
    );
    let _ = frame(&mut app, &ctx, false);
    assert!(app.running.is_none());
    assert!(app.idle());
}

#[test]
fn dirty_review_blocks_switching_until_matching_saved_choices_arrive() {
    let fixture = Fixture::new();
    let case = fixture.case("review");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.review = Some(fixture.review(&case));
    assert!(app.can_switch());
    let review = app.review.as_mut().unwrap();
    review.choices[0].decision = ReviewDecision::KeepForMatching;
    review.choices[0].representative_event_id = Some("ridge-a".into());
    let edited = review.choices.clone();
    assert!(app.dirty());
    assert!(!app.can_switch());
    app.busy = Some("Saving review".into());
    send(
        &app,
        Lane::Work,
        &case,
        Ok(Payload::Saved(
            fixture.path.join("saved-review"),
            edited.clone(),
        )),
    );
    let _ = frame(&mut app, &egui::Context::default(), false);
    assert!(!app.dirty());
    assert!(app.can_switch());
    assert_eq!(app.review.as_ref().unwrap().choices, edited);
    assert_eq!(app.review.as_ref().unwrap().saved_choices, edited);
}

#[test]
fn late_case_open_cannot_replace_new_edits_active_jobs_or_a_different_current_case() {
    for variant in 0..3 {
        let fixture = Fixture::new();
        let old = fixture.case("previous");
        let destination = fixture.case("destination");
        let mut app = fixture.app();
        app.case = Some(old.clone());
        app.studio.bind(&old);
        app.busy = Some("Checking another case".into());
        let origin = if variant == 2 {
            fixture.case("stale-origin")
        } else {
            old.clone()
        };
        if variant == 0 {
            app.studio.document.as_mut().unwrap().recording_start_utc =
                "2026-05-12T12:34:56Z".into();
        } else if variant == 1 {
            app.running = Some(old.manifest.parent().unwrap().join("active-job"));
        }
        let before = app.studio.document.clone();
        send(&app, Lane::Work, &origin, Ok(Payload::Case(destination)));
        let _ = frame(&mut app, &egui::Context::default(), false);
        assert_eq!(app.case.as_ref().unwrap().manifest, old.manifest);
        assert_eq!(app.studio.document, before);
        assert!(
            app.notice
                .as_ref()
                .unwrap()
                .1
                .contains("Your current work was kept")
        );
        assert!(app.preferences.recent_cases.is_empty());
    }
}

#[test]
fn same_case_recording_refresh_preserves_new_review_edits() {
    let fixture = Fixture::new();
    let case = fixture.case("refresh");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.studio.bind(&case);
    app.studio.document.as_mut().unwrap().recording_start_utc = "2026-05-12T12:34:56Z".into();
    let before = app.studio.document.clone();
    send(&app, Lane::Work, &case, Ok(Payload::Case(case.clone())));
    let _ = frame(&mut app, &egui::Context::default(), false);
    assert_eq!(app.studio.document, before);
    assert!(app.dirty());
    assert_eq!(app.case.as_ref().unwrap().manifest, case.manifest);
}

#[test]
fn failed_save_preserves_unsaved_choices_and_does_not_grant_exit() {
    let fixture = Fixture::new();
    let case = fixture.case("failed-save");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.review = Some(fixture.review(&case));
    app.review.as_mut().unwrap().choices[1].decision = ReviewDecision::Uncertain;
    let edited = app.review.as_ref().unwrap().choices.clone();
    let baseline = app.review.as_ref().unwrap().saved_choices.clone();
    app.busy = Some("Saving review".into());
    app.close_requested = true;
    send(
        &app,
        Lane::Work,
        &case,
        Err("Recording changed; review was not saved".into()),
    );
    let output = frame(&mut app, &egui::Context::default(), false);
    assert_eq!(app.review.as_ref().unwrap().choices, edited);
    assert_eq!(app.review.as_ref().unwrap().saved_choices, baseline);
    assert!(app.dirty());
    assert!(!app.shutdown.requested());
    assert!(!app.allow_close);
    assert!(!contains_close(&output));
    assert_eq!(
        app.notice,
        Some((true, "Recording changed; review was not saved".into()))
    );
}

#[test]
fn closing_without_discard_confirmation_preserves_unsaved_review_when_run_stops() {
    let fixture = Fixture::new();
    let case = fixture.case("close-dirty");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.review = Some(fixture.review(&case));
    app.review.as_mut().unwrap().choices[0].decision = ReviewDecision::Reject;
    let edited = app.review.as_ref().unwrap().choices.clone();
    let baseline = app.review.as_ref().unwrap().saved_choices.clone();
    app.running = Some(job(&case, "active", "running").directory);
    app.canceling = true;
    app.close_requested = true;
    send(
        &app,
        Lane::Cancel,
        &case,
        Ok(Payload::Job(job(&case, "active", "canceled"))),
    );
    send(
        &app,
        Lane::Run,
        &case,
        Ok(Payload::Job(job(&case, "active", "canceled"))),
    );
    let output = frame(&mut app, &egui::Context::default(), false);
    assert!(app.idle());
    assert!(app.dirty());
    assert!(app.close_requested);
    assert!(!app.allow_close);
    assert!(!contains_close(&output));
    assert_eq!(app.review.as_ref().unwrap().choices, edited);
    assert_eq!(app.review.as_ref().unwrap().saved_choices, baseline);
}

#[test]
fn native_close_request_protects_dirty_review_and_finished_work_still_needs_confirmation() {
    let fixture = Fixture::new();
    let case = fixture.case("closing");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.review = Some(fixture.review(&case));
    app.review.as_mut().unwrap().choices[1].decision = ReviewDecision::Reject;
    let ctx = egui::Context::default();
    let output = frame(&mut app, &ctx, true);
    assert!(contains_cancel_close(&output));
    assert!(!contains_close(&output));
    assert!(app.close_requested);
    let review = app.review.as_mut().unwrap();
    review.choices = review.saved_choices.clone();
    app.running = Some(job(&case, "active", "running").directory);
    let output = frame(&mut app, &ctx, false);
    assert!(!app.allow_close);
    assert!(!contains_close(&output));
    send(
        &app,
        Lane::Run,
        &case,
        Ok(Payload::Job(job(&case, "active", "canceled"))),
    );
    let output = frame(&mut app, &ctx, false);
    assert!(!app.allow_close);
    assert!(!contains_close(&output));
    assert!(!app.shutdown.requested());
    app.begin_cancel_and_exit(&ctx);
    assert!(app.shutdown.requested());
    assert!(!app.allow_close);
}

#[test]
fn completed_stage_and_kept_audio_proposal_never_become_a_verified_location() {
    let fixture = Fixture::new();
    let case = fixture.case("not-truth");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.review = Some(fixture.review(&case));
    app.review.as_mut().unwrap().choices[0].decision = ReviewDecision::KeepForMatching;
    app.review.as_mut().unwrap().choices[0].representative_event_id = Some("ridge-a".into());
    app.running = Some(job(&case, "completed", "running").directory);
    send(
        &app,
        Lane::Run,
        &case,
        Ok(Payload::Job(job(&case, "completed", "completed"))),
    );
    let _ = frame(&mut app, &egui::Context::default(), false);
    assert_eq!(phase_name("completed"), "Completed");
    assert!(!app.jobs[0].status.as_ref().unwrap().location_verified);
    assert!(
        !app.review
            .as_ref()
            .unwrap()
            .session
            .overview()
            .location_verified
    );
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .contains("does not verify a location")
    );
    assert_eq!(app.case.as_ref().unwrap().recording_verified, Some(true));
}

#[test]
fn reopened_reversed_decisions_are_ordered_by_group_identity_before_ui_indexing() {
    let fixture = Fixture::new();
    let case = fixture.case("ordering");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.review = Some(fixture.review(&case));
    let mut ordered = app.review.as_ref().unwrap().choices.clone();
    ordered[0].decision = ReviewDecision::KeepForMatching;
    ordered[0].representative_event_id = Some("ridge-a".into());
    ordered[1].decision = ReviewDecision::Reject;
    let mut reversed = ordered.clone();
    reversed.reverse();
    app.busy = Some("Reopening decisions".into());
    send(
        &app,
        Lane::Work,
        &case,
        Ok(Payload::Choices(
            reversed,
            fixture.path.join("saved-reversed"),
        )),
    );
    let _ = frame(&mut app, &egui::Context::default(), false);
    let review = app.review.as_ref().unwrap();
    assert_eq!(review.choices, ordered);
    assert_eq!(review.saved_choices, ordered);
    assert!(!review.dirty());
    for (group, choice) in review.session.groups().iter().zip(&review.choices) {
        assert_eq!(group.harmonic_group_id, choice.harmonic_group_id);
    }
}

#[test]
fn malformed_reopened_group_choices_leave_current_review_intact() {
    let fixture = Fixture::new();
    let case = fixture.case("invalid-ordering");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.review = Some(fixture.review(&case));
    let baseline = app.review.as_ref().unwrap().choices.clone();
    let mut duplicate = baseline.clone();
    duplicate[1] = duplicate[0].clone();
    let mut foreign = baseline.clone();
    foreign[1].harmonic_group_id = "foreign-group".into();
    for choices in [vec![baseline[0].clone()], duplicate, foreign] {
        app.busy = Some("Reopening decisions".into());
        send(
            &app,
            Lane::Work,
            &case,
            Ok(Payload::Choices(
                choices,
                fixture.path.join("invalid-snapshot"),
            )),
        );
        let _ = frame(&mut app, &egui::Context::default(), false);
        let review = app.review.as_ref().unwrap();
        assert_eq!(review.choices, baseline);
        assert_eq!(review.saved_choices, baseline);
        assert!(review.saved_path.is_none());
        assert!(!review.dirty());
        assert!(app.notice.as_ref().unwrap().0);
    }
}

fn hint_response(case: &CaseOverview, text: &str) -> CaseOverview {
    let mut response = case.clone();
    response.recording_verified = None;
    response.recording_problem = None;
    response.hints.push(geosonnar::casework::CaseHint {
        schema_version: 1,
        case_manifest_sha256: sha256(&case.manifest).unwrap(),
        recording_sha256: case.case.recording.sha256.clone(),
        created_unix_s: 1,
        hint: HintRequest {
            kind: HintKind::Region,
            text: text.into(),
            source: HintSource::User,
            certainty: HintCertainty::Uncertain,
        },
    });
    response
}

#[test]
fn failed_clue_save_preserves_the_exact_draft_and_recording_check() {
    let fixture = Fixture::new();
    let case = fixture.case("clue-failure");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.hint_text = "  Possibly near a lake; still uncertain.\n".into();
    let draft = app.hint_text.clone();
    app.busy = Some("Saving the clue".into());
    send(
        &app,
        Lane::Work,
        &case,
        Err("Could not publish clue receipt".into()),
    );
    let _ = frame(&mut app, &egui::Context::default(), false);
    assert_eq!(app.hint_text, draft);
    assert!(app.busy.is_none());
    assert_eq!(app.case.as_ref().unwrap().recording_verified, Some(true));
    assert!(app.case.as_ref().unwrap().hints.is_empty());
    assert!(app.notice.as_ref().unwrap().0);
}

#[test]
fn successful_clue_save_clears_only_the_submitted_draft_and_keeps_review_edits() {
    let fixture = Fixture::new();
    let case = fixture.case("clue-success");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.review = Some(fixture.review(&case));
    app.review.as_mut().unwrap().choices[0].decision = ReviewDecision::Uncertain;
    let edits = app.review.as_ref().unwrap().choices.clone();
    app.hint_text = "  Possibly near a lake  \n".into();
    app.busy = Some("Saving the clue".into());
    send(
        &app,
        Lane::Work,
        &case,
        Ok(Payload::HintAdded(
            hint_response(&case, "Possibly near a lake"),
            "Possibly near a lake".into(),
        )),
    );
    let _ = frame(&mut app, &egui::Context::default(), false);
    assert!(app.hint_text.is_empty());
    assert_eq!(app.case.as_ref().unwrap().hints.len(), 1);
    assert_eq!(app.case.as_ref().unwrap().recording_verified, Some(true));
    assert_eq!(app.review.as_ref().unwrap().choices, edits);
    assert!(app.dirty());
    assert!(!app.notice.as_ref().unwrap().0);
}

#[test]
fn clue_save_completion_preserves_a_new_draft_typed_while_the_worker_was_busy() {
    let fixture = Fixture::new();
    let case = fixture.case("clue-new-draft");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.hint_text = "A second clue is still being written".into();
    app.busy = Some("Saving the first clue".into());
    send(
        &app,
        Lane::Work,
        &case,
        Ok(Payload::HintAdded(
            hint_response(&case, "First submitted clue"),
            "First submitted clue".into(),
        )),
    );
    let _ = frame(&mut app, &egui::Context::default(), false);
    assert_eq!(app.hint_text, "A second clue is still being written");
    assert_eq!(
        app.case.as_ref().unwrap().hints[0].hint.text,
        "First submitted clue"
    );
    assert_eq!(app.case.as_ref().unwrap().recording_verified, Some(true));
}

#[test]
fn clue_response_with_changed_recording_identity_cannot_inherit_a_checked_badge() {
    let fixture = Fixture::new();
    let case = fixture.case("clue-changed-source");
    let mut app = fixture.app();
    app.case = Some(case.clone());
    app.hint_text = "Clue".into();
    let mut changed = hint_response(&case, "Clue");
    changed.case.recording.sha256 = "f".repeat(64);
    send(
        &app,
        Lane::Work,
        &case,
        Ok(Payload::HintAdded(changed, "Clue".into())),
    );
    let _ = frame(&mut app, &egui::Context::default(), false);
    assert_eq!(app.case.as_ref().unwrap().recording_verified, None);
}

#[test]
fn appearance_preferences_migrate_and_persist_without_changing_tools() {
    let legacy: Preferences =
        serde_json::from_str(r#"{"ffmpeg":"chosen-ffmpeg.exe","recent_cases":[]}"#).unwrap();
    assert!(!legacy.dark_mode);
    let mut chosen = legacy;
    chosen.dark_mode = true;
    let restored: Preferences =
        serde_json::from_slice(&serde_json::to_vec(&chosen).unwrap()).unwrap();
    assert!(restored.dark_mode);
    assert_eq!(restored.ffmpeg, "chosen-ffmpeg.exe");
}
