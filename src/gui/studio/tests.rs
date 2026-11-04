//! Headless message/state controls. No media player, file picker or network work.
use super::*;
use geosonnar::{
    casework::{CaseMedia, CaseRecording, SavedCase},
    detailed_data::sha256,
};
use serde_json::json;
use std::{
    fs,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[test]
fn presentation_choice_survives_case_binding_without_changing_review_or_search() {
    let fixture = Fixture::new();
    let mut studio = fixture.studio();
    let before = studio.document.clone();
    let guided_search = studio.regional.guided;
    studio.detailed_controls = true;
    studio.bind(&fixture.case);
    assert!(studio.detailed_controls);
    assert_eq!(studio.document, before);
    assert_eq!(studio.regional.guided, guided_search);
    let second = Fixture::new();
    studio.bind(&second.case);
    assert!(studio.detailed_controls);
    assert_eq!(
        studio.document,
        Some(ReviewWorkspace::new(&second.case.case, 0))
    );
    assert!(!Studio::default().detailed_controls);
}

#[test]
fn confirmed_exit_cancels_owned_work_and_ignores_late_results_without_editing_review() {
    let fixture = Fixture::new();
    let mut studio = fixture.studio();
    let before = studio.document.clone();
    let regional_cancel = studio.regional.cancel.clone();
    let download_cancel = studio.download_cancel.clone();
    studio.frame_requested = true;
    studio.data.refresh_requested = true;
    studio.pending_action = Some(Action::ReviewResults);
    studio.cancel_for_exit();
    assert!(regional_cancel.load(Ordering::Relaxed));
    assert!(download_cancel.load(Ordering::Relaxed));
    assert!(!studio.frame_requested && !studio.data.refresh_requested);
    assert!(studio.take_pending_action().is_none());
    let ctx = egui::Context::default();
    studio.spawn(&ctx, "must not start", || {
        panic!("new work after confirmed exit")
    });
    studio
        .sender
        .send(Reply {
            generation: studio.generation,
            lane: WorkerLane::Task,
            result: Ok(ResultValue::DataReady("must not trigger refresh".into())),
        })
        .unwrap();
    studio.tick(&ctx);
    assert_eq!(studio.document, before);
    assert!(!studio.data.refresh_requested && studio.notice.is_none());
}

struct Fixture {
    root: PathBuf,
    case: CaseOverview,
}
impl Fixture {
    fn new() -> Self {
        let run = ROOT.get_or_init(|| {
            let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("output/benchmarks/studio-lifecycle")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            fs::create_dir_all(&p).unwrap();
            p.canonicalize().unwrap()
        });
        let id = format!("fabricated-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let root = run.join(&id);
        fs::create_dir(&root).unwrap();
        let recording = root.join("fabricated.raw");
        fs::write(
            &recording,
            b"Fabricated studio evidence, not a real recording",
        )
        .unwrap();
        let case = SavedCase {
            schema_version: 1,
            case_id: id,
            title: "Synthetic stereo case".into(),
            created_unix_s: 1,
            local_only: true,
            recording: CaseRecording {
                path: "fabricated.raw".into(),
                sha256: sha256(&recording).unwrap(),
                bytes: fs::metadata(&recording).unwrap().len(),
                media: CaseMedia {
                    duration_s: 60.0,
                    sample_rate_hz: 4000,
                    channels: 2,
                    audio_codec: "fabricated".into(),
                },
            },
        };
        let manifest = root.join("case.json");
        fs::write(&manifest, serde_json::to_vec_pretty(&case).unwrap()).unwrap();
        Self {
            root,
            case: CaseOverview {
                manifest,
                case,
                hints: vec![],
                recording_path: recording,
                recording_verified: Some(true),
                recording_problem: None,
            },
        }
    }
    fn studio(&self) -> Studio {
        // Existing detailed-control regressions continue to exercise that view;
        // guided presentation has separate controls without changing defaults.
        let mut studio = Studio {
            detailed_controls: true,
            ..Default::default()
        };
        studio.bind(&self.case);
        studio.data.refresh_requested = false;
        studio.next_progress = Instant::now() + Duration::from_secs(3600);
        studio.next_frame = Instant::now() + Duration::from_secs(3600);
        studio
    }
    fn window(&self, channel: usize) -> SpectrogramWindow {
        SpectrogramWindow {
            input: self.case.recording_path.clone(),
            source_sha256: self.case.case.recording.sha256.clone(),
            source_stamp: review_media::SourceStamp {
                path: self.case.recording_path.clone(),
                bytes: self.case.case.recording.bytes,
                modified_unix_ns: 1,
            },
            channel,
            source_duration_s: 60.0,
            requested_start_s: 0.0,
            requested_end_s: 20.0,
            start_s: 0.0,
            end_s: 20.0,
            decoded_samples: 80000,
            sample_rate_hz: 4000,
            fft_size: 16384,
            hop_samples: 400,
            times_s: vec![2.048, 2.148],
            frequencies_hz: vec![0.0, 100.0],
            power_db: vec![-100.0, -50.0, -100.0, -50.0],
            envelope: (0..201)
                .map(|i| review_media::EnvelopeSample {
                    time_s: i as f64 / 10.0,
                    rms: if (40..=60).contains(&i) { 5.0 } else { 1.0 },
                })
                .collect(),
            envelope_band_hz: [20.01953125, 250.0],
            envelope_method: "Fabricated spectral band energy".into(),
            time_reference: "Fabricated sample centers".into(),
        }
    }
    fn session(&self, channel: usize) -> ReviewSession {
        let directory = self.root.join(format!("discovery-{channel}"));
        fs::create_dir(&directory).unwrap();
        fs::write(
            directory.join("ridge-a.csv"),
            b"time_s,frequency_hz,weight\n10,120,1\n11,110,1\n12,100,1\n",
        )
        .unwrap();
        let report = json!({"schema_version":1,"status":"events_proposed","location_verified":false,
            "source":self.case.recording_path,"source_sha256":self.case.case.recording.sha256,
            "source_metadata":{"duration_s":60.0,"selected_channel_index":channel,"available_channels":2,"channels_averaged":false},
            "settings":{"min_frequency_hz":40.0,"max_frequency_hz":1600.0,"max_gap_s":2.0},"group_count":1,"ridge_count":1,
            "groups":[{"harmonic_group_id":"group-a","start_s":9.0,"end_s":14.0,"ridge_count":1,"review_status":"unreviewed","aircraft_identity_confirmed":false}],
            "events":[{"event_id":"ridge-a","harmonic_group_id":"group-a","start_s":9.0,"end_s":14.0,"band_hz":[95.0,125.0],"ridge":[{"time_s":10.0,"frequency_hz":120.0,"weight":1.0},{"time_s":11.0,"frequency_hz":110.0,"weight":1.0},{"time_s":12.0,"frequency_hz":100.0,"weight":1.0}],"quality":{"support_fraction":0.4,"fragment_count":1,"frequency_sigma_hz":0.5}}],"limitations":["Fabricated lifecycle control"]});
        let discovery = directory.join("discovery.json");
        fs::write(&discovery, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        geosonnar::event_review::load(
            &self.case.manifest,
            &discovery,
            &sha256(&discovery).unwrap(),
            channel,
        )
        .unwrap()
    }
    fn layer(&self) -> map_data::MapData {
        let path = self.root.join("tracks.json");
        fs::write(
            &path,
            b"{\"tracks\":[],\"config\":{\"time_origin_utc_s\":1788825600,\"max_gap_s\":30}}\n",
        )
        .unwrap();
        map_data::load(&path).unwrap()
    }
    fn lightning(&self) -> evidence_score::LightningCatalog {
        let path = self.root.join("lightning.json");
        fs::write(&path,b"{\"schema_version\":1,\"strikes\":[{\"id\":\"s1\",\"time_utc_s\":1788825600,\"latitude_deg\":0,\"longitude_deg\":0,\"altitude_m\":1000,\"intensity\":null}]}\n").unwrap();
        evidence_score::import_lightning(
            &path,
            &evidence_score::LightningProvenance {
                provider: "Synthetic".into(),
                source: "Fabricated data".into(),
                license: None,
                notes: None,
                altitude_datum: evidence_score::LightningAltitudeDatum::Wgs84Ellipsoid,
                source_metadata: None,
            },
        )
        .unwrap()
    }
}

fn send(studio: &Studio, generation: u64, lane: WorkerLane, result: Result<ResultValue, String>) {
    studio
        .sender
        .send(Reply {
            generation,
            lane,
            result,
        })
        .unwrap();
}
fn tick(studio: &mut Studio) {
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput::default());
    studio.tick(&ctx);
    let _ = ctx.end_pass();
}

/// Small real native search and export, rather than a report-shaped fake.
fn saved_investigation_fixture(
    f: &Fixture,
    comparison: bool,
    partial: bool,
    replay: bool,
) -> geosonnar::investigation_archive::LoadedInvestigation {
    use geosonnar::{investigation_archive as archive, investigation_report, regional_search::*};
    let mut doc = f.studio().document.unwrap();
    doc.recording_start_utc = "2026-09-08T00:00:00Z".into();
    doc.map.bounds = [-0.1, -0.1, 0.1, 0.1];
    doc.mix.doppler_frequency = true;
    doc.mix.thunder_timing = true;
    let mut doppler = EvidenceEvent::manual("recorded-tone".into(), EventKind::Doppler, 2.0, 4.0);
    doppler.review = ReviewState::Confirmed;
    doppler.acoustic_source = workspace::AcousticSourceAssessment::Compatible;
    doppler.selected_for_scoring = true;
    doppler.points = vec![
        workspace::TracePoint {
            time_s: 2.0,
            frequency_hz: 102.0,
        },
        workspace::TracePoint {
            time_s: 4.0,
            frequency_hz: 100.0,
        },
    ];
    let mut thunder =
        EvidenceEvent::manual("recorded-thunder".into(), EventKind::Thunder, 2.5, 4.0);
    thunder.arrival_s = 3.0;
    thunder.review = ReviewState::Confirmed;
    thunder.acoustic_source = workspace::AcousticSourceAssessment::Compatible;
    thunder.selected_for_scoring = true;
    doc.events = vec![doppler, thunder];
    let path = f.root.join("archive-tracks.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({"tracks":[{"flight_id":"test-flight","points":[
        {"t_s":-10.0,"xyz_m":[6379137.0,-1000.0,0.0]},
        {"t_s":0.0,"xyz_m":[6379137.0,0.0,0.0]},
        {"t_s":10.0,"xyz_m":[6379137.0,1000.0,0.0]}]}],
        "config":{"time_origin_utc_s":1788825600.0,"max_gap_s":15.0}}))
        .unwrap(),
    )
    .unwrap();
    let layers = vec![map_data::load(&path).unwrap()];
    let lightning = Some(f.lightning());
    doc.register_dataset(
        SourceBinding {
            path: layers[0].source_path.clone(),
            sha256: layers[0].source_sha256.clone(),
        },
        layers[0].time_origin_utc_s,
        false,
    )
    .unwrap();
    let catalog = lightning.as_ref().unwrap();
    doc.register_dataset(
        SourceBinding {
            path: catalog.input_path.clone(),
            sha256: catalog.input_sha256.clone(),
        },
        None,
        true,
    )
    .unwrap();
    let plan = RegionalPlan {
        events: vec![
            EventAlternatives {
                event_id: "recorded-tone".into(),
                source_ids: vec!["test-flight".into()],
                emitted_frequency_hz: Some([90.0, 110.0]),
            },
            EventAlternatives {
                event_id: "recorded-thunder".into(),
                source_ids: vec!["s1".into()],
                emitted_frequency_hz: None,
            },
        ],
        ..Default::default()
    };
    doc.regional_plan = Some(plan.clone());
    let observers = vec![evidence_score::ObserverLocation {
        latitude_deg: 0.0,
        longitude_deg: 0.0,
        ellipsoid_height_m: 0.0,
    }];
    let map = archive::merge_layers(&layers).unwrap();
    let buffers = archive::capture_replay_inputs(&doc, &layers, &lightning).unwrap();
    let directory = f.root.join("investigation");
    if comparison {
        let options = ComparisonOptions {
            modes: vec![
                SearchMode::DopplerOnly,
                SearchMode::ThunderOnly,
                SearchMode::Combined,
            ],
            max_total_evaluations: 1000,
        };
        doc.regional_comparison = Some(options.clone());
        let request = ModalityPlan {
            regional: plan,
            modes: options.modes,
            max_total_evaluations: 1000,
        };
        let cancel = AtomicBool::new(false);
        let report = run_modalities(
            &doc,
            &map,
            &lightning,
            &observers,
            &request,
            &cancel,
            |progress| {
                if partial && progress.completed_modes == 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        )
        .unwrap();
        if replay {
            investigation_report::export_modalities_with_replay_inputs(
                &directory, &doc, &report, &buffers,
            )
            .unwrap();
        } else {
            investigation_report::export_modalities(&directory, &doc, &report).unwrap();
        }
    } else {
        let report = run(
            &doc,
            &map,
            &lightning,
            &observers,
            &plan,
            &AtomicBool::new(partial),
            |_| {},
        )
        .unwrap();
        if replay {
            investigation_report::export_with_replay_inputs(&directory, &doc, &report, &buffers)
                .unwrap();
        } else {
            investigation_report::export(&directory, &doc, &report).unwrap();
        }
    }
    archive::load(&directory).unwrap()
}

fn report_inputs_reply(
    studio: &Studio,
    result: Result<Option<geosonnar::investigation_archive::ReplayInputs>, String>,
) -> investigation::InputsReply {
    let archive = studio.investigation.archive.as_ref().unwrap();
    investigation::InputsReply {
        token: studio.investigation.token,
        receipt_sha256: archive.receipt_sha256.clone(),
        document: studio.document.clone().unwrap(),
        previous_inputs: inspection::InputIdentity::capture(studio),
        result,
    }
}

fn finish_studio_task(studio: &mut Studio) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        tick(studio);
        if !studio.busy() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Headless studio task did not complete"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn automatic_library_refresh_preserves_the_operations_success_or_failure_notice() {
    let f = Fixture::new();
    for failed in [false, true] {
        let mut studio = f.studio();
        let message = if failed {
            "Original source is missing; archived Results remain viewable."
        } else {
            "Imported data is ready to review."
        };
        studio.busy = Some("Finishing operation".into());
        send(
            &studio,
            studio.generation,
            WorkerLane::Task,
            if failed {
                Err(message.into())
            } else {
                Ok(ResultValue::DataReady(message.into()))
            },
        );
        finish_studio_task(&mut studio);
        assert_eq!(studio.notice, Some((failed, message.into())));
        assert!(!studio.data.refresh_requested);
    }
}

#[test]
fn opening_saved_investigation_restores_exact_results_without_loading_or_applying() {
    for (comparison, partial) in [(false, false), (false, true), (true, false), (true, true)] {
        let f = Fixture::new();
        let loaded = saved_investigation_fixture(&f, comparison, partial, true);
        let original = loaded.document.clone();
        let expected = serde_json::to_value(&loaded.report).unwrap();
        let mut studio = f.studio();
        let previous = studio.document.clone().unwrap();
        studio.window = Some(f.window(0));
        studio.undo.push(previous.clone());
        studio.regional.running = true;
        studio.regional.estimate = Some(regional_estimate_fixture());
        let old_generation = studio.generation;
        send(
            &studio,
            old_generation,
            WorkerLane::Task,
            Ok(ResultValue::InvestigationLoaded(
                Box::new(loaded),
                Box::new(previous),
            )),
        );
        // A frame reply from the old review must not repaint the restored case.
        send(
            &studio,
            old_generation,
            WorkerLane::Frame,
            Err("Old frame result".into()),
        );
        tick(&mut studio);
        assert_eq!(studio.document.as_ref(), Some(&original));
        assert!(!studio.dirty());
        assert!(studio.undo.is_empty() && studio.redo.is_empty());
        assert!(studio.window.is_none() && studio.layers.is_empty() && studio.lightning.is_none());
        assert!(studio.score.is_none() && studio.inspection.active.is_none());
        assert!(
            !studio.regional.running
                && studio.regional.estimate.is_none()
                && studio.regional.comparison_estimate.is_none()
        );
        assert!(studio.regional.selection_key().is_none());
        assert!(!studio.report_inputs_ready());
        assert!(matches!(
            studio.take_pending_action(),
            Some(Action::ReviewResults)
        ));
        assert_eq!(
            serde_json::to_value(&studio.investigation.archive.as_ref().unwrap().report).unwrap(),
            expected
        );
        if comparison {
            let report = studio.regional.comparison.as_ref().unwrap();
            assert_eq!(report.modes.len(), 3);
            if partial {
                assert_eq!(
                    report.modes[1].status,
                    geosonnar::regional_search::ModeStatus::NotStarted
                );
            }
        }
    }
}

#[test]
fn saved_investigation_missing_sources_still_opens_and_explicit_restore_enables_archived_inspection()
 {
    use geosonnar::investigation_archive;
    let f = Fixture::new();
    let loaded = saved_investigation_fixture(&f, false, false, true);
    let original = loaded.document.clone();
    for binding in &original.datasets {
        fs::remove_file(&binding.path).unwrap();
    }
    let mut studio = f.studio();
    let previous = studio.document.clone().unwrap();
    studio.accept_investigation(loaded, previous);
    assert!(!studio.report_inputs_ready());
    studio.prepare_report_inputs(&egui::Context::default(), true);
    finish_studio_task(&mut studio);
    assert!(studio.report_inputs_ready(), "{:?}", studio.notice);
    assert_eq!(studio.document.as_ref(), Some(&original));
    assert!(
        workspace::verify(&f.case.manifest, &original).is_err(),
        "Raw source gate is deliberately unchanged"
    );
    let map = studio.merged_map().unwrap();
    investigation_archive::verify_archived_inspection(
        &f.case.manifest,
        studio.investigation.archive.as_ref().unwrap(),
        &original,
        &map,
        &studio.lightning,
    )
    .unwrap();
    let id = studio.regional.report.as_ref().unwrap().hypotheses[0]
        .id
        .clone();
    let score = studio.regional.report.as_ref().unwrap().hypotheses[0].score;
    studio.regional.select_hypothesis(&id);
    studio.inspect_regional(&egui::Context::default());
    finish_studio_task(&mut studio);
    let inspected = &studio
        .inspection
        .active
        .as_ref()
        .expect("Archived model inspection")
        .overlay;
    assert!((inspected.score_report.candidates[0].score.unwrap() - score).abs() < 1e-9);
    assert_eq!(studio.document.as_ref(), Some(&original));
    assert!(studio.undo.is_empty());
    studio.apply_regional(&egui::Context::default());
    finish_studio_task(&mut studio);
    assert_eq!(studio.document.as_ref(), Some(&original));
    assert!(studio.undo.is_empty());
    assert!(
        studio
            .notice
            .as_ref()
            .unwrap()
            .1
            .contains("original source files")
    );
}

#[test]
fn reopened_report_can_be_read_without_recording_but_audio_inspection_requires_its_hash() {
    let f = Fixture::new();
    let loaded = saved_investigation_fixture(&f, false, false, true);
    fs::remove_file(&f.case.recording_path).unwrap();
    let mut studio = f.studio();
    let previous = studio.document.clone().unwrap();
    studio.accept_investigation(loaded, previous);
    studio.prepare_report_inputs(&egui::Context::default(), true);
    finish_studio_task(&mut studio);
    assert!(studio.report_inputs_ready());
    let id = studio.regional.report.as_ref().unwrap().hypotheses[0]
        .id
        .clone();
    studio.regional.select_hypothesis(&id);
    studio.inspect_regional(&egui::Context::default());
    finish_studio_task(&mut studio);
    assert!(studio.inspection.active.is_none());
    assert!(studio.regional.report.is_some());
    assert!(studio.notice.as_ref().unwrap().0);
}

#[test]
fn investigation_open_refuses_unsaved_edits_and_late_or_foreign_case_replies() {
    let f = Fixture::new();
    let loaded = saved_investigation_fixture(&f, false, false, true);
    let mut studio = f.studio();
    let previous = studio.document.clone().unwrap();
    studio.document.as_mut().unwrap().notes = "Keep these edits".into();
    let changed = studio.document.clone();
    studio.open_investigation_path(&egui::Context::default(), loaded.directory.clone());
    assert!(!studio.busy());
    assert!(
        studio
            .notice
            .as_ref()
            .unwrap()
            .1
            .contains("Save or discard")
    );
    studio.accept_investigation(loaded.clone(), previous);
    assert_eq!(studio.document, changed);
    assert!(studio.investigation.archive.is_none());
    let other = Fixture::new();
    studio.bind(&other.case);
    let previous = studio.document.clone().unwrap();
    studio.accept_investigation(loaded, previous.clone());
    assert_eq!(studio.document.as_ref(), Some(&previous));
    assert!(studio.investigation.archive.is_none());
}

#[test]
fn report_input_replies_cannot_overwrite_review_or_data_changes() {
    for change in 0..3 {
        let f = Fixture::new();
        let loaded = saved_investigation_fixture(&f, false, false, true);
        let inputs = geosonnar::investigation_archive::load_replay_inputs(&loaded).unwrap();
        let mut studio = f.studio();
        let previous = studio.document.clone().unwrap();
        studio.accept_investigation(loaded, previous);
        let reply = report_inputs_reply(&studio, Ok(Some(inputs)));
        match change {
            0 => studio.document.as_mut().unwrap().notes = "New review".into(),
            1 => studio.report_data_changed(),
            _ => studio.investigation.token += 1,
        }
        studio.accept_report_inputs(reply);
        assert!(studio.layers.is_empty() && studio.lightning.is_none());
        assert!(!studio.report_inputs_ready());
        assert!(studio.notice.as_ref().unwrap().1.contains("changed"));
    }
}

#[test]
fn older_export_checks_independently_loaded_inputs_without_replacing_the_report() {
    let f = Fixture::new();
    let loaded = saved_investigation_fixture(&f, false, false, false);
    let original = loaded.document.clone();
    assert!(!loaded.has_replay_inputs);
    let mut studio = f.studio();
    let previous = studio.document.clone().unwrap();
    studio.accept_investigation(loaded, previous);
    studio.prepare_report_inputs(&egui::Context::default(), true);
    assert!(!studio.busy());
    studio.prepare_report_inputs(&egui::Context::default(), false);
    finish_studio_task(&mut studio);
    assert!(!studio.report_inputs_ready());
    assert!(
        studio
            .investigation
            .problem
            .as_ref()
            .unwrap()
            .contains("do not match")
    );
    studio.layers = vec![map_data::load(&f.root.join("archive-tracks.json")).unwrap()];
    studio.lightning = Some(f.lightning());
    studio.report_data_changed();
    studio.prepare_report_inputs(&egui::Context::default(), false);
    finish_studio_task(&mut studio);
    assert!(studio.report_inputs_ready(), "{:?}", studio.notice);
    assert_eq!(studio.document.as_ref(), Some(&original));
    studio.lightning.as_mut().unwrap().provenance.notes =
        Some("Changed source interpretation".into());
    studio.report_data_changed();
    assert!(!studio.report_inputs_ready());
}

#[test]
fn saved_investigation_readiness_is_visible_and_small_window_layouts_remain_bounded() {
    let f = Fixture::new();
    let loaded = saved_investigation_fixture(&f, true, true, true);
    let mut studio = f.studio();
    let previous = studio.document.clone().unwrap();
    studio.accept_investigation(loaded, previous);
    let ctx = egui::Context::default();
    for size in [[1600.0, 1100.0], [760.0, 560.0], [1200.0, 800.0]] {
        diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Results, size, vec![]);
        let (output, bounds) =
            diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Results, size, vec![]);
        assert!(bounds.right() <= size[0] + 1.0 && bounds.bottom() <= size[1] + 1.0);
        let text = diagnostic_text(&output)
            .into_iter()
            .map(|x| x.0)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Restore report data"));
        assert!(text.contains("Report data are not loaded"));
    }
    let before = studio.document.clone();
    studio.document.as_mut().unwrap().notes = "A new analyst note".into();
    studio.checkpoint(before.clone().unwrap());
    studio.undo();
    assert_eq!(studio.document, before);
    assert!(
        !studio.report_inputs_ready(),
        "Undo does not invent a data verification"
    );
}

#[test]
fn late_loaded_reply_preserves_edits_made_after_the_load_started() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let previous = studio.document.clone().unwrap();
    let saved_before = studio.saved.clone();
    let mut loaded = previous.clone();
    loaded.notes = "Old saved document".into();
    studio.document.as_mut().unwrap().notes = "My new unsaved choices".into();
    studio.window = Some(f.window(0));
    studio.busy = Some("Loading".into());
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::Loaded(loaded, Box::new(previous))),
    );
    tick(&mut studio);
    assert_eq!(
        studio.document.as_ref().unwrap().notes,
        "My new unsaved choices"
    );
    assert_eq!(studio.saved, saved_before);
    assert!(studio.dirty());
    assert!(studio.window.is_some());
    assert!(!studio.busy());
    assert!(
        studio
            .notice
            .as_ref()
            .unwrap()
            .1
            .contains("edits were kept")
    );
}

#[test]
fn matching_loaded_reply_resets_caches_and_preserves_all_loaded_choices() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let previous = studio.document.clone().unwrap();
    let mut loaded = previous.clone();
    loaded.notes = "Loaded notes".into();
    loaded.strike_fallback_height_m = Some(140.0);
    studio.window = Some(f.window(0));
    studio.layers.push(f.layer());
    studio.selected = Some(0);
    studio.play_start = Some((Instant::now(), 4.0));
    let token = studio.playback_generation;
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::Loaded(loaded.clone(), Box::new(previous))),
    );
    tick(&mut studio);
    assert_eq!(studio.document, Some(loaded.clone()));
    assert_eq!(studio.saved, Some(loaded));
    assert!(!studio.dirty());
    assert!(studio.window.is_none());
    assert!(studio.layers.is_empty());
    assert!(studio.play_start.is_none());
    assert_ne!(studio.playback_generation, token);
    assert!(studio.selected.is_none());
}

#[test]
fn saved_snapshot_reply_marks_only_that_snapshot_saved_and_keeps_newer_edits_dirty() {
    let f = Fixture::new();
    let mut studio = f.studio();
    studio.document.as_mut().unwrap().notes = "Version sent for save".into();
    let submitted = studio.document.clone().unwrap();
    studio.document.as_mut().unwrap().notes = "Typed while saving".into();
    studio.busy = Some("Saving".into());
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::Saved(
            f.root.join("saved.json"),
            submitted.clone(),
        )),
    );
    tick(&mut studio);
    assert_eq!(studio.saved, Some(submitted));
    assert_eq!(
        studio.document.as_ref().unwrap().notes,
        "Typed while saving"
    );
    assert!(studio.dirty());
    assert!(!studio.busy());
}

#[test]
fn stale_case_replies_cannot_replace_document_notice_or_current_worker_state() {
    let a = Fixture::new();
    let b = Fixture::new();
    let mut studio = a.studio();
    let old_generation = studio.generation;
    let old_doc = studio.document.clone().unwrap();
    studio.bind(&b.case);
    studio.data.refresh_requested = false;
    studio.busy = Some("Current case operation".into());
    studio.frame_busy = true;
    studio.notice = Some((false, "Current case notice".into()));
    let current = studio.document.clone();
    send(
        &studio,
        old_generation,
        WorkerLane::Task,
        Ok(ResultValue::Loaded(old_doc.clone(), Box::new(old_doc))),
    );
    send(
        &studio,
        old_generation,
        WorkerLane::Frame,
        Err("Old recording unavailable".into()),
    );
    tick(&mut studio);
    assert_eq!(studio.document, current);
    assert_eq!(studio.busy.as_deref(), Some("Current case operation"));
    assert!(studio.frame_busy);
    assert_eq!(studio.notice, Some((false, "Current case notice".into())));
}

#[test]
fn unrelated_worker_lanes_do_not_clear_foreground_activity() {
    let f = Fixture::new();
    let mut studio = f.studio();
    studio.busy = Some("Scoring".into());
    studio.frame_busy = true;
    studio.progress_busy = true;
    send(
        &studio,
        studio.generation,
        WorkerLane::Frame,
        Ok(ResultValue::Frame(None)),
    );
    tick(&mut studio);
    assert_eq!(studio.busy.as_deref(), Some("Scoring"));
    assert!(!studio.frame_busy);
    assert!(studio.progress_busy);
}

#[test]
fn a_spectrum_for_a_different_channel_cannot_replace_the_current_review_window() {
    let f = Fixture::new();
    let mut studio = f.studio();
    studio.window = Some(f.window(0));
    let mut other = f.window(1);
    other.start_s = 5.0;
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::Spectrum(other)),
    );
    tick(&mut studio);
    assert_eq!(studio.window.as_ref().unwrap().channel, 0);
    assert_eq!(studio.window.as_ref().unwrap().start_s, 0.0);
    assert!(studio.notice.as_ref().unwrap().0);
}

#[test]
fn thunder_proposals_remain_pending_and_redetection_preserves_reviewed_edits() {
    let f = Fixture::new();
    let mut studio = f.studio();
    studio.window = Some(f.window(0));
    studio.propose_thunder();
    let doc = studio.document.as_mut().unwrap();
    assert_eq!(doc.events.len(), 1);
    let e = &mut doc.events[0];
    assert_eq!(e.review, ReviewState::Pending);
    assert!(e.acoustic_source.is_unassessed());
    assert!(!e.selected_for_scoring);
    assert!(e.uncertainty_s >= 2.048);
    assert_eq!(e.kind, EventKind::Thunder);
    e.label = "My reviewed interpretation".into();
    e.review = ReviewState::Confirmed;
    e.acoustic_source = workspace::AcousticSourceAssessment::Compatible;
    e.selected_for_scoring = true;
    let expected = e.clone();
    studio.propose_thunder();
    assert_eq!(studio.document.as_ref().unwrap().events, vec![expected]);
}

#[test]
fn imported_channel_switch_clears_media_cache_and_does_not_auto_select_proposals() {
    let f = Fixture::new();
    let session = f.session(1);
    let mut studio = f.studio();
    studio.window = Some(f.window(0));
    studio.play_start = Some((Instant::now(), 2.0));
    let token = studio.playback_generation;
    studio.import_review(&session).unwrap();
    assert_eq!(studio.channel(), 1);
    assert!(studio.window.is_none());
    assert!(studio.play_start.is_none());
    assert_ne!(studio.playback_generation, token);
    let events = &studio.document.as_ref().unwrap().events;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].review, ReviewState::Pending);
    assert!(!events[0].selected_for_scoring);
}

#[test]
fn rejected_foreign_discovery_import_keeps_channel_and_existing_media_cache() {
    let f = Fixture::new();
    let foreign = Fixture::new();
    let session = foreign.session(1);
    let mut studio = f.studio();
    studio.window = Some(f.window(0));
    let previous = studio.document.clone();
    let token = studio.playback_generation;
    assert!(studio.import_review(&session).is_err());
    assert_eq!(studio.document, previous);
    assert!(studio.window.is_some());
    assert_eq!(studio.playback_generation, token);
}

#[test]
fn undoing_an_origin_change_blocks_cached_models_until_the_saved_assumption_matches() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let layer = f.layer();
    let binding = SourceBinding {
        path: layer.source_path.clone(),
        sha256: layer.source_sha256.clone(),
    };
    studio.add_layer(layer, binding);
    let original = studio.document.clone().unwrap();
    assert!(studio.cached_bindings_match(&original).is_ok());
    studio.document.as_mut().unwrap().dataset_options[0].time_origin_utc_s = Some(1_788_825_610.0);
    studio.layers[0].time_origin_utc_s = Some(1_788_825_610.0);
    assert!(
        studio
            .cached_bindings_match(studio.document.as_ref().unwrap())
            .is_ok()
    );
    studio.checkpoint(original.clone());
    studio.undo();
    assert_eq!(studio.document, Some(original));
    assert!(
        studio
            .cached_bindings_match(studio.document.as_ref().unwrap())
            .is_err()
    );
    studio.layers[0].time_origin_utc_s = Some(1_788_825_600.0);
    assert!(
        studio
            .cached_bindings_match(studio.document.as_ref().unwrap())
            .is_ok()
    );
}

#[test]
fn lightning_cache_requires_the_persisted_altitude_interpretation_and_source_binding() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let catalog = f.lightning();
    let doc = studio.document.as_mut().unwrap();
    doc.register_dataset(
        SourceBinding {
            path: catalog.input_path.clone(),
            sha256: catalog.input_sha256.clone(),
        },
        None,
        true,
    )
    .unwrap();
    studio.lightning = Some(catalog);
    assert!(
        studio
            .cached_bindings_match(studio.document.as_ref().unwrap())
            .is_ok()
    );
    studio.document.as_mut().unwrap().dataset_options[0].lightning_altitude_wgs84 = false;
    assert!(
        studio
            .cached_bindings_match(studio.document.as_ref().unwrap())
            .is_err()
    );
    studio.document.as_mut().unwrap().dataset_options[0].lightning_altitude_wgs84 = true;
    studio.document.as_mut().unwrap().datasets.clear();
    assert!(
        studio
            .cached_bindings_match(studio.document.as_ref().unwrap())
            .is_err()
    );
}

#[test]
fn late_restored_layers_cannot_replace_cache_after_assumptions_change() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let mut layer = f.layer();
    studio.add_layer(
        layer.clone(),
        SourceBinding {
            path: layer.source_path.clone(),
            sha256: layer.source_sha256.clone(),
        },
    );
    let previous = studio.document.clone().unwrap();
    studio.document.as_mut().unwrap().dataset_options[0].time_origin_utc_s = Some(1_788_825_610.0);
    studio.layers[0].time_origin_utc_s = Some(1_788_825_610.0);
    layer.time_origin_utc_s = Some(1_788_825_600.0);
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::RestoredLayers(vec![layer], None, previous)),
    );
    tick(&mut studio);
    assert_eq!(studio.layers[0].time_origin_utc_s, Some(1_788_825_610.0));
    assert_eq!(
        studio.document.as_ref().unwrap().dataset_options[0].time_origin_utc_s,
        Some(1_788_825_610.0)
    );
    assert!(studio.notice.as_ref().unwrap().0);
}

fn keyboard_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}
#[test]
fn selected_event_delete_and_keyboard_undo_redo_preserve_other_evidence() {
    let f = Fixture::new();
    let mut studio = f.studio();
    studio.add_event(EventKind::Doppler);
    studio.add_event(EventKind::Thunder);
    studio.selected = Some(0);
    let original = studio.document.clone().unwrap();
    let ctx = egui::Context::default();
    let key = |studio: &mut Studio, key, modifiers| {
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![keyboard_event(key, modifiers)],
                ..Default::default()
            },
            |ui| {
                studio.keyboard_shortcuts(ui.ctx());
            },
        );
    };
    key(&mut studio, egui::Key::Delete, egui::Modifiers::NONE);
    assert_eq!(
        studio.document.as_ref().unwrap().events,
        original.events[1..]
    );
    assert_eq!(studio.selected, None);
    assert_eq!(studio.undo.len(), 1);
    key(&mut studio, egui::Key::Z, egui::Modifiers::COMMAND);
    assert_eq!(studio.document.as_ref(), Some(&original));
    key(&mut studio, egui::Key::Y, egui::Modifiers::COMMAND);
    assert_eq!(
        studio.document.as_ref().unwrap().events,
        original.events[1..]
    );
    // A held/repeated Delete cannot remove another event once selection clears.
    key(&mut studio, egui::Key::Delete, egui::Modifiers::NONE);
    assert_eq!(studio.document.as_ref().unwrap().events.len(), 1);
}
#[test]
fn event_shortcuts_do_not_steal_delete_or_undo_from_text_fields() {
    let f = Fixture::new();
    let mut studio = f.studio();
    studio.add_event(EventKind::Doppler);
    let original = studio.document.clone();
    let ctx = egui::Context::default();
    let mut label = "Editable event label".to_owned();
    let id = egui::Id::new("typing_event_label");
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.add(egui::TextEdit::singleline(&mut label).id(id))
            .request_focus();
    });
    for key in [egui::Key::Delete, egui::Key::Z] {
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![keyboard_event(
                    key,
                    if key == egui::Key::Z {
                        egui::Modifiers::COMMAND
                    } else {
                        egui::Modifiers::NONE
                    },
                )],
                ..Default::default()
            },
            |ui| {
                assert!(ui.ctx().text_edit_focused());
                studio.keyboard_shortcuts(ui.ctx());
                // The text editor still receives the key; the review is unchanged.
                ui.add(egui::TextEdit::singleline(&mut label).id(id));
            },
        );
        assert_eq!(studio.document, original);
        assert!(studio.undo.is_empty());
    }
}
#[test]
fn a_multi_frame_trace_is_one_undo_step_including_release_frame_edits() {
    let f = Fixture::new();
    let mut studio = f.studio();
    studio.add_event(EventKind::Doppler);
    let original = studio.document.clone().unwrap();
    for (time, down) in [(1.0, true), (2.0, true), (3.0, false)] {
        let before = studio.document.clone().unwrap();
        studio.document.as_mut().unwrap().events[0]
            .points
            .push(workspace::TracePoint {
                time_s: time,
                frequency_hz: 100.0,
            });
        studio.finish_edit_frame(before, down);
    }
    assert_eq!(studio.undo.len(), 1);
    let drawn = studio.document.clone();
    studio.undo();
    assert_eq!(studio.document.as_ref(), Some(&original));
    studio.redo();
    assert_eq!(studio.document, drawn);
}
#[test]
fn audio_review_resizes_without_expanding_the_available_width() {
    let f = Fixture::new();
    let mut studio = f.studio();
    studio.add_event(EventKind::Doppler);
    let ctx = egui::Context::default();
    for (width, height) in [
        (1500.0, 900.0),
        (800.0, 550.0),
        (1200.0, 720.0),
        (660.0, 460.0),
    ] {
        for _ in 0..2 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, height),
                    )),
                    ..Default::default()
                },
                |ui| {
                    studio.analysis_ui(ui);
                    assert!(
                        ui.min_rect().right() <= width + 1.0,
                        "width={width}: {:?}",
                        ui.min_rect()
                    );
                    assert!(
                        ui.min_rect().bottom() <= height + 1.0,
                        "height={height}: {:?}",
                        ui.min_rect()
                    );
                },
            );
        }
    }
}

fn atmospheric_fixture() -> geosonnar::atmosphere::AtmosphereProfile {
    let mut profile: geosonnar::atmosphere::AtmosphereProfile = serde_json::from_str(include_str!(
        "../../../examples/synthetic-atmosphere-profile.json"
    ))
    .unwrap();
    profile.name = "Fabricated GUI atmospheric profile".into();
    profile.valid_from_utc_s = workspace::parse_utc("2026-09-08 00:00:00").unwrap();
    profile.valid_to_utc_s = profile.valid_from_utc_s + 3600.0;
    profile.validate().unwrap();
    profile
}

/// A fabricated two-modality score surface, not an audio/location experiment.
fn modality_surface(overlap: bool) -> evidence_score::ScoreReport {
    use evidence_score::{
        CandidateEvidence, CandidateScore, EvidenceTerm, ObserverLocation, ScoreConfig,
        TermContribution, TermSettings,
    };
    let means = if overlap {
        [[0.0, 3.0], [0.6, 0.6], [3.0, 0.0]]
    } else {
        [[0.0, 10.0], [6.0, 6.0], [10.0, 0.0]]
    };
    let config = ScoreConfig {
        thunder_timing: TermSettings {
            enabled: true,
            weight: 1.0,
            residual_scale: 1.0,
        },
        ..Default::default()
    };
    let selected_terms = vec![EvidenceTerm::DopplerFrequency, EvidenceTerm::ThunderTiming];
    let mut candidates: Vec<_> = means
        .into_iter()
        .enumerate()
        .map(|(i, means)| {
            let terms = selected_terms
                .iter()
                .copied()
                .zip(means)
                .map(|(term, mean)| TermContribution {
                    term,
                    weight: 1.0,
                    residual_scale: 1.0,
                    available: true,
                    unavailable_reason: None,
                    residuals: vec![],
                    event_mean_squared_residuals: std::collections::BTreeMap::from([(
                        "fabricated".into(),
                        mean,
                    )]),
                    mean_normalized_squared_residual: Some(mean),
                    weighted_contribution: Some(mean / 2.0),
                })
                .collect();
            CandidateScore {
                candidate: CandidateEvidence {
                    id: format!("receiver-{i:05}"),
                    observer: ObserverLocation {
                        latitude_deg: 0.01,
                        longitude_deg: i as f64 * 0.01,
                        ellipsoid_height_m: 80.0,
                    },
                    recording_start_utc_s: 1_788_825_600.0,
                    frequency: vec![],
                    doppler_timing: vec![],
                },
                rank: None,
                score: Some((means[0] + means[1]) / 2.0),
                terms,
                unavailable_reasons: vec![],
                location_verified: false,
            }
        })
        .collect();
    let mut ranking: Vec<_> = (0..candidates.len()).collect();
    ranking.sort_by(|&a, &b| {
        candidates[a]
            .score
            .unwrap()
            .total_cmp(&candidates[b].score.unwrap())
            .then_with(|| candidates[a].candidate.id.cmp(&candidates[b].candidate.id))
    });
    for (i, &index) in ranking.iter().enumerate() {
        candidates[index].rank = Some(i + 1);
    }
    let mut report = evidence_score::ScoreReport {
        schema_version: 1,
        config,
        selected_terms,
        evidence_sha256: "0".repeat(64),
        ranked_candidate_ids: ranking
            .iter()
            .map(|&i| candidates[i].candidate.id.clone())
            .collect(),
        candidates,
        calibration: "Uncalibrated fabricated UI surface".into(),
        location_verified: false,
        modalities: None,
        limitations: vec!["Fabricated UI control, not a measured recording".into()],
    };
    report.modalities = Some(
        geosonnar::modality_diagnostics::summarize(
            &report,
            Some([0.0, 0.0, 0.02, 0.02]),
            &geosonnar::modality_diagnostics::DiagnosticOptions {
                near_best_score_delta: 0.75,
            },
        )
        .unwrap(),
    );
    report
}

fn prediction_fixture(doc: &ReviewWorkspace) -> geosonnar::prediction_overlay::PredictionOverlay {
    use geosonnar::prediction_overlay::*;
    let sample = |time, hz, measured| FrequencySample {
        time_s: time,
        arrival_utc_s: 1_788_825_605.0 + time,
        frequency_hz: hz,
        measured_point_index: measured,
        observed_frequency_hz: measured.map(|_| hz + 10.0),
        normalized_residual: measured.map(|_| 2.0),
        effective_residual_scale_hz: measured.map(|_| 5.0),
    };
    PredictionOverlay {
        schema_version: 1,
        document_sha256: geosonnar::regional_search::document_sha256(doc).unwrap(),
        inputs_sha256: "f".repeat(64),
        observer: evidence_score::ObserverLocation {
            latitude_deg: 45.0,
            longitude_deg: -73.0,
            ellipsoid_height_m: 50.0,
        },
        clock_adjustment_s: 5.0,
        adjusted_recording_start_utc_s: 1_788_825_605.0,
        model_label: "Fabricated constant-speed display control".into(),
        settings: OverlaySettings::default(),
        frequency: vec![FrequencyEventOverlay {
            event_id: "inspected-doppler".into(),
            source_id: "hypothetical-aircraft".into(),
            event_label: "Synthetic Doppler".into(),
            emitted_frequency_hz: 100.0,
            start_s: 1.0,
            end_s: 5.0,
            sampling_step_s: 1.0,
            fit_diagnostics: None,
            segments: vec![
                FrequencySegment {
                    support_segment_index: 0,
                    trajectory_edge_index: 0,
                    samples: vec![sample(1.0, 100.0, Some(0)), sample(2.0, 100.0, None)],
                },
                FrequencySegment {
                    support_segment_index: 1,
                    trajectory_edge_index: 0,
                    samples: vec![sample(4.0, 90.0, None), sample(5.0, 90.0, Some(1))],
                },
            ],
        }],
        timing: vec![TimingMarker {
            event_id: "inspected-thunder".into(),
            source_id: "hypothetical-strike".into(),
            kind: EventKind::Thunder,
            predicted_arrival_s: 7.0,
            predicted_arrival_utc_s: 1_788_825_612.0,
            observed_arrival_s: 8.0,
            observed_uncertainty_s: 0.5,
            effective_residual_scale_s: 0.5,
            normalized_residual: 2.0,
            strike_height_assumed: Some(false),
        }],
        unavailable: vec![OverlayUnavailable {
            event_id: "inspected-doppler".into(),
            source_id: "hypothetical-aircraft".into(),
            start_s: Some(2.0),
            end_s: Some(4.0),
            measured_point_index: None,
            reason: "Fabricated source support gap".into(),
        }],
        frequency_sample_count: 4,
        sampling_coarsened: false,
        score_report: modality_surface(true),
        location_verified: false,
        limitations: vec!["Fabricated display control, not location validation".into()],
    }
}

fn inspection_fixture(f: &Fixture) -> (Studio, inspection::PreparedInspection) {
    let mut studio = f.studio();
    let doc = studio.document.as_mut().unwrap();
    doc.recording_start_utc = "2026-09-08T00:00:00Z".into();
    let mut doppler =
        EvidenceEvent::manual("inspected-doppler".into(), EventKind::Doppler, 1.0, 5.0);
    doppler.review = ReviewState::Confirmed;
    doppler.acoustic_source = workspace::AcousticSourceAssessment::Compatible;
    doppler.selected_for_scoring = true;
    doppler.points = vec![
        workspace::TracePoint {
            time_s: 1.0,
            frequency_hz: 110.0,
        },
        workspace::TracePoint {
            time_s: 5.0,
            frequency_hz: 100.0,
        },
    ];
    let mut thunder =
        EvidenceEvent::manual("inspected-thunder".into(), EventKind::Thunder, 8.0, 9.0);
    thunder.review = ReviewState::Confirmed;
    thunder.acoustic_source = workspace::AcousticSourceAssessment::Compatible;
    thunder.selected_for_scoring = true;
    doc.events = vec![doppler, thunder];
    let original = doc.clone();
    studio.regional.report = Some(regional_report_fixture(&original, false));
    studio.regional.report_document = Some(original.clone());
    studio.regional.select_hypothesis("fabricated-hypothesis-a");
    let prepared = inspection::PreparedInspection {
        inputs: inspection::InputIdentity::capture(&studio),
        origin: inspection::InspectionOrigin::Regional {
            key: studio.regional.selection_key().unwrap(),
            run_id: studio.regional.run_id,
        },
        overlay: prediction_fixture(&original),
        original,
    };
    (studio, prepared)
}

#[test]
fn inspecting_regional_fit_focuses_audio_without_applying_or_selecting_evidence() {
    let f = Fixture::new();
    let (mut studio, prepared) = inspection_fixture(&f);
    let before = prepared.original.clone();
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::PredictionInspected(Box::new(prepared))),
    );
    tick(&mut studio);
    assert_eq!(studio.document.as_ref(), Some(&before));
    assert!(studio.undo.is_empty());
    assert!(studio.score.is_none());
    assert!(studio.inspection.active.is_some());
    assert_eq!(
        studio.regional.detail.as_ref().unwrap().0,
        "fabricated-hypothesis-a"
    );
    assert_eq!(studio.selected, Some(0));
    assert!(studio.view.time_min_s <= 1.0 && studio.view.time_max_s >= 5.0);
    assert!(matches!(
        studio.take_pending_action(),
        Some(Action::ReviewInspection)
    ));
    assert!(studio.take_pending_action().is_none());
    assert_eq!(studio.tool, EditTool::Inspect);
}

#[test]
fn late_prediction_replies_reject_changed_evidence_data_candidate_and_run() {
    for change in 0..5 {
        let f = Fixture::new();
        let (mut studio, prepared) = inspection_fixture(&f);
        match change {
            0 => studio.document.as_mut().unwrap().events[0].points[0].frequency_hz += 1.0,
            1 => studio.layers.push(f.layer()),
            2 => studio.lightning = Some(f.lightning()),
            3 => studio.regional.select_hypothesis("different-hypothesis"),
            _ => studio.regional.run_id += 1,
        }
        let before = studio.document.clone();
        send(
            &studio,
            studio.generation,
            WorkerLane::Task,
            Ok(ResultValue::PredictionInspected(Box::new(prepared))),
        );
        tick(&mut studio);
        assert_eq!(studio.document, before, "change {change}");
        assert!(studio.inspection.active.is_none(), "change {change}");
        assert!(studio.regional.detail.is_none());
        assert!(studio.take_pending_action().is_none());
        assert!(studio.notice.as_ref().unwrap().1.contains("changed"));
    }
}

#[test]
fn inspected_prediction_is_removed_when_loaded_source_identity_changes() {
    for change in 0..4 {
        let f = Fixture::new();
        let (mut studio, mut prepared) = inspection_fixture(&f);
        studio.layers.push(f.layer());
        studio.lightning = Some(f.lightning());
        prepared.inputs = inspection::InputIdentity::capture(&studio);
        studio.accept_inspection(prepared, &egui::Context::default());
        assert!(studio.inspection.active.is_some());
        match change {
            0 => studio.layers[0].source_sha256 = "e".repeat(64),
            1 => studio.layers[0].time_origin_utc_s = Some(42.0),
            2 => studio.lightning.as_mut().unwrap().input_sha256 = "e".repeat(64),
            _ => studio.document.as_mut().unwrap().map.clock_adjustment_s += 1.0,
        }
        studio.invalidate_changed_inspection();
        assert!(studio.inspection.active.is_none(), "change {change}");
        assert!(studio.inspection.invalidated.is_some());
        assert!(studio.take_pending_action().is_none());
    }
}

#[test]
fn switching_candidate_or_independent_mode_discards_the_inspected_overlay() {
    use geosonnar::regional_search::SearchMode;
    for change_mode in [false, true] {
        let f = Fixture::new();
        let (mut studio, mut prepared) = inspection_fixture(&f);
        studio.regional.accept_comparison(
            regional_comparison_fixture(&prepared.original, false),
            prepared.original.clone(),
        );
        studio.regional.mode = SearchMode::Combined;
        studio.regional.select_hypothesis("fabricated-hypothesis-a");
        prepared.origin = inspection::InspectionOrigin::Regional {
            key: studio.regional.selection_key().unwrap(),
            run_id: studio.regional.run_id,
        };
        let before = studio.document.clone();
        studio.accept_inspection(prepared, &egui::Context::default());
        if change_mode {
            studio.regional.mode = SearchMode::DopplerOnly;
        } else {
            studio.regional.select_location(9);
        }
        studio.invalidate_changed_inspection();
        assert!(studio.inspection.active.is_none());
        assert_eq!(studio.document, before);
    }
}

#[test]
fn prediction_controls_hide_and_clear_without_editing_and_fit_small_layouts() {
    let f = Fixture::new();
    let (mut studio, prepared) = inspection_fixture(&f);
    let before = prepared.original.clone();
    let ctx = egui::Context::default();
    studio.accept_inspection(prepared, &ctx);
    let size = [1600.0, 1400.0];
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Audio,
        size,
        "Show inspected hypothesis",
    );
    assert!(!studio.inspection.visible);
    assert!(studio.inspection.active.is_some());
    for size in [[760.0, 560.0], [1100.0, 720.0], [1600.0, 1400.0]] {
        diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
        let (output, bounds) =
            diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
        assert!(bounds.right() <= size[0] + 1.0, "{size:?}: {bounds:?}");
        assert!(bounds.bottom() <= size[1] + 1.0, "{size:?}: {bounds:?}");
        assert!(
            diagnostic_text(&output)
                .iter()
                .any(|(text, _)| text.contains("not confidence"))
        );
    }
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Audio,
        size,
        "Clear inspection",
    );
    assert!(studio.inspection.active.is_none());
    assert_eq!(studio.document.as_ref(), Some(&before));
    assert!(studio.undo.is_empty());
}

#[test]
fn fit_comparison_stays_read_only_respects_focus_and_fits_resized_audio_review() {
    use geosonnar::fit_diagnostics::{ErrorSummary, FitDiagnostics, ToneComparison};
    let f = Fixture::new();
    let (mut studio, mut prepared) = inspection_fixture(&f);
    // Presentation-only numbers: native arithmetic has independent controls.
    prepared.overlay.frequency[0].fit_diagnostics = Some(FitDiagnostics {
        observed_points: 3,
        supported_points: 3,
        span_s: Some(4.0),
        comparison: Some(ToneComparison {
            doppler: ErrorSummary {
                rmse_hz: 1.25,
                normalized_rmse: 0.25,
            },
            flat: ErrorSummary {
                rmse_hz: 7.5,
                normalized_rmse: 1.5,
            },
            linear: ErrorSummary {
                rmse_hz: 0.5,
                normalized_rmse: 0.1,
            },
            flat_frequency_hz: 105.0,
            linear_reference_time_s: 3.0,
            linear_frequency_at_reference_hz: 105.0,
            linear_slope_hz_per_s: -2.5,
        }),
        unavailable_reason: None,
        limitations: vec!["Fabricated display control; samples may be correlated.".into()],
    });
    let ctx = egui::Context::default();
    let size = [1600.0, 1400.0];
    studio.accept_inspection(prepared, &ctx);
    let before_document = studio.document.clone();
    let before_report = serde_json::to_value(&studio.regional.report).unwrap();
    let before_overlay =
        serde_json::to_value(&studio.inspection.active.as_ref().unwrap().overlay).unwrap();
    diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
    let (collapsed, _) = diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
    assert!(
        diagnostic_text(&collapsed)
            .iter()
            .any(|(t, _)| t == "Fit comparison")
    );
    assert!(
        !diagnostic_text(&collapsed)
            .iter()
            .any(|(t, _)| t == "Fitted flat tone")
    );
    let output = click_settled_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Audio,
        size,
        "Fit comparison",
    );
    let text = diagnostic_text(&output);
    for expected in [
        "Inspected Doppler",
        "Fitted flat tone",
        "Fitted linear drift",
        "1.25000",
        "7.50000",
        "0.50000",
        "Scaled RMSE",
    ] {
        assert!(
            text.iter().any(|(t, _)| t == expected),
            "Missing {expected}"
        );
    }
    assert!(
        text.iter()
            .any(|(t, _)| t.contains("3 of 3 measured points"))
    );
    assert!(
        text.iter()
            .any(|(t, _)| t.contains("does not identify a source"))
    );
    let output = click_settled_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Audio,
        size,
        "Fitted alternatives and interpretation",
    );
    assert!(
        diagnostic_text(&output)
            .iter()
            .any(|(t, _)| t.contains("samples may be correlated"))
    );
    studio.selected = Some(1);
    diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
    let (focused_thunder, _) =
        diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
    let text = diagnostic_text(&focused_thunder);
    assert!(
        text.iter()
            .any(|(t, _)| t.starts_with("Select an inspected Doppler"))
    );
    assert!(!text.iter().any(|(t, _)| t == "Inspected Doppler"));
    let output = click_settled_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Audio,
        size,
        "All inspected events",
    );
    assert!(
        diagnostic_text(&output)
            .iter()
            .any(|(t, _)| t == "Inspected Doppler")
    );
    for size in [[760.0, 560.0], [1100.0, 720.0], [1600.0, 1400.0]] {
        diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
        let (_, bounds) = diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
        assert!(bounds.right() <= size[0] + 1.0, "{size:?}: {bounds:?}");
        assert!(bounds.bottom() <= size[1] + 1.0, "{size:?}: {bounds:?}");
    }
    assert_eq!(studio.document, before_document);
    assert_eq!(
        serde_json::to_value(&studio.regional.report).unwrap(),
        before_report
    );
    assert_eq!(
        serde_json::to_value(&studio.inspection.active.as_ref().unwrap().overlay).unwrap(),
        before_overlay
    );
    assert!(studio.undo.is_empty() && studio.score.is_none());
}

#[test]
fn fit_comparison_explains_legacy_and_incomplete_support_without_showing_numeric_rows() {
    use geosonnar::fit_diagnostics::FitDiagnostics;
    let f = Fixture::new();
    for legacy in [true, false] {
        let (mut studio, mut prepared) = inspection_fixture(&f);
        if !legacy {
            prepared.overlay.frequency[0].fit_diagnostics = Some(FitDiagnostics {
                observed_points: 3,
                supported_points: 2,
                span_s: Some(4.0),
                comparison: None,
                unavailable_reason: Some(
                    "A measured point has no prediction; comparison withheld.".into(),
                ),
                limitations: vec![],
            });
        }
        let before = prepared.original.clone();
        let ctx = egui::Context::default();
        studio.accept_inspection(prepared, &ctx);
        let output = click_settled_diagnostic_label(
            &mut studio,
            &ctx,
            DiagnosticPane::Audio,
            [1600.0, 1400.0],
            "Fit comparison",
        );
        let text = diagnostic_text(&output);
        assert!(text.iter().any(|(t, _)| t.contains(if legacy {
            "unavailable in this saved inspection"
        } else {
            "comparison withheld"
        })));
        assert!(!text.iter().any(|(t, _)| t == "Inspected Doppler"));
        assert_eq!(studio.document.as_ref(), Some(&before));
        assert!(studio.undo.is_empty() && studio.score.is_none());
    }
}

#[test]
fn candidate_predictions_are_painted_on_the_measured_spectrum_and_video_overlay() {
    let f = Fixture::new();
    let (mut studio, prepared) = inspection_fixture(&f);
    let ctx = egui::Context::default();
    studio.accept_inspection(prepared, &ctx);
    let window = f.window(0);
    studio.texture = Some(spectrogram::texture(&ctx, &window, -100.0, 0.0));
    studio.window = Some(window);
    studio.video = Some(ctx.load_texture(
        "fabricated-video-frame",
        egui::ColorImage::filled([2, 2], Color32::BLACK),
        egui::TextureOptions::LINEAR,
    ));
    studio.video_overlay = true;
    studio.view.frequency_max_hz = 200.0;
    studio.inspection.all_events = true;
    let size = [1600.0, 1400.0];
    diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
    let (output, _) = diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
    assert_eq!(
        diagnostic_text(&output)
            .iter()
            .filter(|(text, _)| text.starts_with("HYPOTHESIS"))
            .count(),
        2,
        "The measured spectrogram and its video overlay must both carry candidate identity"
    );
    studio.inspection.visible = false;
    let (output, _) = diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
    assert!(
        !diagnostic_text(&output)
            .iter()
            .any(|(text, _)| text.starts_with("HYPOTHESIS"))
    );
    assert!(studio.inspection.active.is_some());
}

#[test]
fn inspect_frequency_pan_in_audio_workspace_keeps_saved_review_score_and_undo_unchanged() {
    let f = Fixture::new();
    let mut studio = f.studio();
    studio.add_event(EventKind::Doppler);
    studio.saved = studio.document.clone();
    studio.undo.clear();
    studio.redo.clear();
    studio.score = Some(modality_surface(true));
    studio.score_document = studio.document.clone();
    let before = studio.document.clone();
    let score = serde_json::to_value(&studio.score).unwrap();
    let ctx = egui::Context::default();
    let window = f.window(0);
    let image = spectrogram::texture(&ctx, &window, -100.0, 0.0);
    let image_id = image.id();
    studio.texture = Some(image);
    studio.window = Some(window);
    studio.tool = EditTool::Inspect;
    studio.view.frequency_min_hz = 20.0;
    studio.view.frequency_max_hz = 60.0;
    let size = [1600.0, 1400.0];
    diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
    let (output, _) = diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, vec![]);
    let spectrum_rects: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Mesh(mesh) if mesh.texture_id == image_id => Some(shape.clip_rect),
            _ => None,
        })
        .collect();
    // The texture is first painted in the read-only video overlay, then in the
    // interactive editor. Keep both visible and drag the actual editor below.
    assert_eq!(spectrum_rects.len(), 2);
    let rect = spectrum_rects[1];
    assert!(rect.top() > spectrum_rects[0].bottom());
    assert!(rect.height() > spectrum_rects[0].height());
    let start = rect.center();
    let end = start + egui::vec2(0.0, 30.0);
    for (point, pressed) in [(start, Some(true)), (end, None), (end, Some(false))] {
        let mut events = vec![egui::Event::PointerMoved(point)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Audio, size, events);
    }
    assert!(studio.view.frequency_min_hz > 20.0);
    assert!((studio.view.frequency_max_hz - studio.view.frequency_min_hz - 40.0).abs() < 1e-8);
    assert_eq!(studio.document, before);
    assert_eq!(serde_json::to_value(&studio.score).unwrap(), score);
    assert_eq!(studio.score_document, before);
    assert!(!studio.dirty());
    assert!(studio.undo.is_empty() && studio.redo.is_empty() && studio.pending_undo.is_none());
}

#[test]
fn fixed_receiver_inspection_keeps_the_fixed_report_and_requires_the_same_selected_receiver() {
    let f = Fixture::new();
    let (mut studio, mut prepared) = inspection_fixture(&f);
    studio.regional.reset_results();
    studio.score = Some(modality_surface(true));
    studio.score_document = studio.document.clone();
    let before_report = serde_json::to_value(studio.score.as_ref().unwrap()).unwrap();
    let ctx = egui::Context::default();
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Results,
        [1600.0, 1400.0],
        "1",
    );
    let (id, observer) = studio.fixed_selected_receiver().unwrap();
    prepared.origin = inspection::InspectionOrigin::fixed(id, &observer);
    let before_doc = studio.document.clone();
    studio.accept_inspection(prepared, &ctx);
    assert!(studio.inspection.active.is_some());
    assert_eq!(studio.document, before_doc);
    assert_eq!(
        serde_json::to_value(studio.score.as_ref().unwrap()).unwrap(),
        before_report
    );
    assert!(studio.regional.detail.is_none());
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Results,
        [1600.0, 1400.0],
        "2",
    );
    studio.invalidate_changed_inspection();
    assert!(studio.inspection.active.is_none());
}

#[test]
fn fixed_inspection_rejects_reused_candidate_id_at_different_receiver_coordinates() {
    let f = Fixture::new();
    let (mut studio, mut prepared) = inspection_fixture(&f);
    studio.regional.reset_results();
    studio.score = Some(modality_surface(true));
    studio.score_document = studio.document.clone();
    let ctx = egui::Context::default();
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Results,
        [1600.0, 1400.0],
        "1",
    );
    let (id, observer) = studio.fixed_selected_receiver().unwrap();
    prepared.origin = inspection::InspectionOrigin::fixed(id.clone(), &observer);
    studio
        .score
        .as_mut()
        .unwrap()
        .candidates
        .iter_mut()
        .find(|row| row.candidate.id == id)
        .unwrap()
        .candidate
        .observer
        .latitude_deg += 1.0;
    studio.accept_inspection(prepared, &ctx);
    assert!(studio.inspection.active.is_none());
    assert!(studio.notice.as_ref().unwrap().1.contains("changed"));
    let prepared = inspection::PreparedInspection {
        original: studio.document.clone().unwrap(),
        inputs: inspection::InputIdentity::capture(&studio),
        origin: {
            let (id, observer) = studio.fixed_selected_receiver().unwrap();
            inspection::InspectionOrigin::fixed(id, &observer)
        },
        overlay: prediction_fixture(studio.document.as_ref().unwrap()),
    };
    studio.accept_inspection(prepared, &ctx);
    assert!(studio.inspection.active.is_some());
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::Scored(
            Box::new(modality_surface(true)),
            studio.document.clone().unwrap(),
        )),
    );
    tick(&mut studio);
    assert!(studio.inspection.active.is_none());
    assert!(studio.take_pending_action().is_none());
}

fn prediction_paint_frame(
    overlay: &geosonnar::prediction_overlay::PredictionOverlay,
    events: &[EvidenceEvent],
    filter: Option<&str>,
) -> (egui::FullOutput, super::super::prediction_plot::DrawStats) {
    let ctx = egui::Context::default();
    let mut stats = super::super::prediction_plot::DrawStats::default();
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 400.0),
            )),
            ..Default::default()
        },
        |ui| {
            stats = super::super::prediction_plot::paint(
                ui.painter(),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1000.0, 400.0)),
                &PlotView {
                    time_min_s: 0.0,
                    time_max_s: 10.0,
                    frequency_min_hz: 0.0,
                    frequency_max_hz: 200.0,
                    db_min: -100.0,
                    db_max: 0.0,
                },
                overlay,
                filter,
                events,
            );
        },
    );
    (output, stats)
}

#[test]
fn prediction_geometry_uses_measured_time_residuals_and_never_bridges_source_gaps() {
    use super::super::prediction_plot::{PREDICTED_FREQUENCY, PREDICTED_TIMING};
    let f = Fixture::new();
    let (_, prepared) = inspection_fixture(&f);
    let (output, stats) =
        prediction_paint_frame(&prepared.overlay, &prepared.original.events, None);
    assert_eq!(stats.frequency_segments, 2);
    assert_eq!(stats.frequency_markers, 2);
    assert_eq!(stats.timing_markers, 1);
    assert_eq!(stats.unavailable_spans, 1);
    let lines: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::LineSegment { points, stroke } => Some((*points, *stroke)),
            _ => None,
        })
        .collect();
    let curves: Vec<_> = lines
        .iter()
        .filter(|(_, stroke)| stroke.color == PREDICTED_FREQUENCY)
        .collect();
    assert_eq!(curves.len(), 2);
    assert_eq!(
        curves[0].0,
        [egui::pos2(100.0, 200.0), egui::pos2(200.0, 200.0)]
    );
    assert_eq!(
        curves[1].0,
        [egui::pos2(400.0, 220.0), egui::pos2(500.0, 220.0)]
    );
    let sample = &prepared.overlay.frequency[0].segments[0].samples[0];
    assert_eq!(
        (sample.observed_frequency_hz.unwrap() - sample.frequency_hz)
            / sample.effective_residual_scale_hz.unwrap(),
        sample.normalized_residual.unwrap()
    );
    assert!(lines.iter().any(|(points, stroke)| stroke.color
        == PREDICTED_FREQUENCY.gamma_multiply(0.65)
        && *points == [egui::pos2(100.0, 180.0), egui::pos2(100.0, 200.0)]));
    assert!(
        lines
            .iter()
            .any(|(points, stroke)| stroke.color == PREDICTED_TIMING
                && points.iter().all(|point| point.x == 700.0)),
        "Expected arrival uses t=7, not observed t=8"
    );
    assert!(
        diagnostic_text(&output)
            .iter()
            .any(|(text, _)| text.contains("HYPOTHESIS") && text.contains("clock +5.000s"))
    );
    let (_, only_thunder) = prediction_paint_frame(
        &prepared.overlay,
        &prepared.original.events,
        Some("inspected-thunder"),
    );
    assert_eq!(only_thunder.frequency_segments, 0);
    assert_eq!(only_thunder.frequency_markers, 0);
    assert_eq!(only_thunder.timing_markers, 1);
}

#[test]
fn prediction_painter_has_a_deterministic_work_limit_and_preserves_missing_data_marks() {
    let f = Fixture::new();
    let (_, mut prepared) = inspection_fixture(&f);
    let mut sample = prepared.overlay.frequency[0].segments[0].samples[0].clone();
    sample.measured_point_index = None;
    prepared.overlay.frequency[0].segments[0].samples = (0..30_000)
        .map(|i| {
            let mut p = sample.clone();
            p.time_s = i as f64 / 3000.0;
            p
        })
        .collect();
    let (_, stats) = prediction_paint_frame(&prepared.overlay, &prepared.original.events, None);
    assert_eq!(
        stats.examined,
        super::super::prediction_plot::MAX_DRAW_ITEMS
    );
    assert!(stats.budget_exhausted);
    assert_eq!(stats.timing_markers, 1);
    assert_eq!(stats.unavailable_spans, 1);
}

#[test]
fn manual_and_detected_events_never_receive_an_automatic_source_assessment() {
    let f = Fixture::new();
    let mut studio = f.studio();
    studio.add_event(EventKind::Doppler);
    studio.add_event(EventKind::Thunder);
    studio.import_review(&f.session(0)).unwrap();
    studio.window = Some(f.window(0));
    studio.propose_thunder();
    let events = &studio.document.as_ref().unwrap().events;
    assert!(events.len() >= 3);
    for event in events {
        assert!(event.acoustic_source.is_unassessed());
        assert_eq!(event.review, ReviewState::Pending);
        assert!(!event.selected_for_scoring);
        assert!(event.flight_id.is_empty() && event.strike_id.is_empty());
    }
}

#[test]
fn measured_trace_timing_type_and_channel_edits_reset_source_review_and_undo_restores_it() {
    for change in 0..8 {
        let f = Fixture::new();
        let (mut studio, _) = inspection_fixture(&f);
        let before = studio.document.clone().unwrap();
        let doc = studio.document.as_mut().unwrap();
        let event = &mut doc.events[0];
        match change {
            0 => event.points[0].frequency_hz += 1.0,
            1 => event.start_s -= 0.1,
            2 => event.end_s += 0.1,
            3 => event.arrival_s += 0.1,
            4 => event.kind = EventKind::Thunder,
            5 => event.uncertainty_s += 1.0,
            6 => event.frequency_sigma_hz += 1.0,
            _ => doc.channel = 1,
        }
        studio.finish_edit_frame(before.clone(), false);
        let current = studio.document.clone().unwrap();
        assert!(
            current.events[0].acoustic_source.is_unassessed(),
            "change {change}"
        );
        assert_eq!(current.events[0].review, ReviewState::Pending);
        assert!(!current.events[0].selected_for_scoring);
        assert_eq!(studio.undo.len(), 1);
        studio.undo();
        assert_eq!(studio.document.as_ref(), Some(&before));
        studio.redo();
        assert_eq!(studio.document.as_ref(), Some(&current));
    }
}

#[test]
fn association_tone_and_source_assessment_edits_preserve_measurement_review() {
    use workspace::AcousticSourceAssessment;
    let f = Fixture::new();
    let (mut studio, prepared) = inspection_fixture(&f);
    studio.accept_inspection(prepared, &egui::Context::default());
    studio.score = Some(modality_surface(true));
    let before = studio.document.clone().unwrap();
    let event = &mut studio.document.as_mut().unwrap().events[0];
    event.flight_id = "new-hypothesis".into();
    event.emitted_frequency_hz = 111.0;
    event.acoustic_source = AcousticSourceAssessment::UncertainHypothesis {
        rationale: "The moving tone could also be a road vehicle".into(),
    };
    studio.finish_edit_frame(before.clone(), false);
    let event = &studio.document.as_ref().unwrap().events[0];
    assert_eq!(event.review, ReviewState::Confirmed);
    assert!(event.selected_for_scoring);
    assert!(matches!(
        event.acoustic_source,
        AcousticSourceAssessment::UncertainHypothesis { .. }
    ));
    assert!(studio.inspection.active.is_none());
    assert!(studio.score.is_none());
    studio.undo();
    assert_eq!(studio.document.as_ref(), Some(&before));
}

#[test]
fn source_assessment_controls_allow_uncertainty_with_a_reason_without_approving_a_source() {
    use workspace::AcousticSourceAssessment;
    let f = Fixture::new();
    let (mut studio, _) = inspection_fixture(&f);
    studio.selected = Some(0);
    studio.document.as_mut().unwrap().mix.doppler_frequency = true;
    studio.document.as_mut().unwrap().events[0].acoustic_source =
        AcousticSourceAssessment::Unassessed;
    let ctx = egui::Context::default();
    let size = [480.0, 1400.0];
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Inspector,
        size,
        "Not assessed",
    );
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Inspector,
        size,
        "Uncertain hypothesis",
    );
    assert!(acoustic_review::first_block(studio.document.as_ref().unwrap(), false).is_some());
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Inspector,
        size,
        "What makes it plausible, and what else could explain it?",
    );
    let reason = "A sustained moving tone; road traffic remains possible.";
    diagnostic_frame(
        &mut studio,
        &ctx,
        DiagnosticPane::Inspector,
        size,
        vec![egui::Event::Text(reason.into())],
    );
    let event = &studio.document.as_ref().unwrap().events[0];
    assert_eq!(event.acoustic_source.rationale(), Some(reason));
    assert_eq!(event.review, ReviewState::Confirmed);
    assert!(event.selected_for_scoring);
    assert!(event.flight_id.is_empty() && event.strike_id.is_empty());
    assert!(acoustic_review::first_block(studio.document.as_ref().unwrap(), false).is_none());
    assert_eq!(
        studio
            .document
            .as_ref()
            .unwrap()
            .acoustic_source_warnings()
            .len(),
        1
    );
    // Choosing "incompatible" keeps the observation visibly selected and blocks scoring.
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Inspector,
        size,
        "Uncertain hypothesis",
    );
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Inspector,
        size,
        "Does not fit this source",
    );
    let event = &studio.document.as_ref().unwrap().events[0];
    assert!(event.selected_for_scoring);
    assert_eq!(event.review, ReviewState::Confirmed);
    assert!(acoustic_review::first_block(studio.document.as_ref().unwrap(), false).is_some());
    studio.undo();
    assert_eq!(
        studio.document.as_ref().unwrap().events[0]
            .acoustic_source
            .rationale(),
        Some(reason)
    );
}

#[test]
fn source_readiness_only_gates_requested_terms_and_modes_and_keeps_blocked_events() {
    use geosonnar::regional_search::{ComparisonOptions, SearchMode};
    use workspace::AcousticSourceAssessment;
    let f = Fixture::new();
    let (mut studio, _) = inspection_fixture(&f);
    let doc = studio.document.as_mut().unwrap();
    doc.mix.doppler_frequency = true;
    doc.events[1].acoustic_source = AcousticSourceAssessment::Unassessed;
    assert!(
        acoustic_review::first_block(doc, false).is_none(),
        "Inactive thunder cannot block Doppler"
    );
    doc.mix.thunder_timing = true;
    doc.regional_comparison = Some(ComparisonOptions {
        modes: vec![SearchMode::DopplerOnly],
        ..Default::default()
    });
    assert!(acoustic_review::first_block(doc, true).is_none());
    assert!(acoustic_review::first_block(doc, false).is_some());
    doc.regional_comparison
        .as_mut()
        .unwrap()
        .modes
        .push(SearchMode::Combined);
    assert!(acoustic_review::first_block(doc, true).is_some());
    assert!(doc.events.iter().all(|event| event.selected_for_scoring));
}

#[test]
fn search_readiness_opens_the_blocked_event_without_changing_selection_or_associations() {
    let f = Fixture::new();
    let (mut studio, _) = inspection_fixture(&f);
    let doc = studio.document.as_mut().unwrap();
    doc.mix.doppler_frequency = true;
    doc.events[0].acoustic_source = workspace::AcousticSourceAssessment::Unassessed;
    let ctx = egui::Context::default();
    let size = [1600.0, 1400.0];
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Search,
        size,
        "Set up regional search",
    );
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Search,
        size,
        "2 Evidence",
    );
    let before = studio.document.clone();
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Search,
        size,
        "Review source in Audio",
    );
    assert_eq!(studio.document, before);
    assert_eq!(studio.selected, Some(0));
    assert!(matches!(
        studio.take_pending_action(),
        Some(Action::ReviewEvidence)
    ));
    assert!(!studio.busy());
}

#[test]
fn legacy_review_reopens_with_unassessed_sources_and_requires_explicit_reassessment() {
    let f = Fixture::new();
    let (mut studio, _) = inspection_fixture(&f);
    studio.document.as_mut().unwrap().mix.doppler_frequency = true;
    let current = studio.document.clone().unwrap();
    let mut legacy = serde_json::to_value(&current).unwrap();
    for event in legacy["events"].as_array_mut().unwrap() {
        event.as_object_mut().unwrap().remove("acoustic_source");
    }
    let path = f.root.join("legacy-workspace.json");
    fs::write(&path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
    let loaded = workspace::load(&f.case.manifest, &path).unwrap();
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::Loaded(loaded, Box::new(current))),
    );
    tick(&mut studio);
    let doc = studio.document.as_ref().unwrap();
    assert!(
        doc.events
            .iter()
            .all(|event| event.acoustic_source.is_unassessed())
    );
    assert!(
        doc.events
            .iter()
            .all(|event| event.review == ReviewState::Confirmed && event.selected_for_scoring)
    );
    assert!(acoustic_review::first_block(doc, false).is_some());
    let saved = workspace::save(&f.case.manifest, doc).unwrap();
    let roundtrip = workspace::load(&f.case.manifest, &saved).unwrap();
    assert_eq!(roundtrip.events, doc.events);
}

#[test]
fn uncertain_source_reason_remains_visible_in_results_audio_and_prediction_caption() {
    use workspace::AcousticSourceAssessment;
    let f = Fixture::new();
    let (mut studio, mut prepared) = inspection_fixture(&f);
    let reason = "Traffic is still a plausible explanation.";
    let doc = studio.document.as_mut().unwrap();
    doc.mix.doppler_frequency = true;
    doc.events[0].acoustic_source = AcousticSourceAssessment::UncertainHypothesis {
        rationale: reason.into(),
    };
    prepared.original = doc.clone();
    studio.regional.report_document = Some(doc.clone());
    studio.regional.report.as_mut().unwrap().plan.events.push(
        geosonnar::regional_search::EventAlternatives {
            event_id: doc.events[0].id.clone(),
            source_ids: vec!["hypothetical-aircraft".into()],
            emitted_frequency_hz: Some([90.0, 110.0]),
        },
    );
    let ctx = egui::Context::default();
    studio.accept_inspection(prepared, &ctx);
    let size = [1600.0, 1400.0];
    for pane in [DiagnosticPane::Results, DiagnosticPane::Audio] {
        diagnostic_frame(&mut studio, &ctx, pane, size, vec![]);
        let (output, _) = diagnostic_frame(&mut studio, &ctx, pane, size, vec![]);
        let text = diagnostic_text(&output);
        assert!(
            text.iter()
                .any(|(text, _)| text.contains("Uncertain acoustic source"))
        );
        assert!(
            text.iter().any(|(text, _)| text.contains(reason)),
            "Reason must not hide inside collapsed limitations"
        );
    }
    let active = studio.inspection.active.as_ref().unwrap();
    let (output, _) = prediction_paint_frame(
        &active.overlay,
        &studio.document.as_ref().unwrap().events,
        Some("inspected-doppler"),
    );
    assert!(
        diagnostic_text(&output)
            .iter()
            .any(|(text, _)| text.starts_with("UNCERTAIN SOURCE"))
    );
    let (output, _) = prediction_paint_frame(
        &active.overlay,
        &studio.document.as_ref().unwrap().events,
        Some("inspected-thunder"),
    );
    assert!(
        !diagnostic_text(&output)
            .iter()
            .any(|(text, _)| text.starts_with("UNCERTAIN SOURCE"))
    );
}

#[derive(Clone, Copy)]
enum DiagnosticPane {
    Inspector,
    Audio,
    Results,
    Data,
    Search,
}

fn diagnostic_frame(
    studio: &mut Studio,
    ctx: &egui::Context,
    pane: DiagnosticPane,
    size: [f32; 2],
    events: Vec<egui::Event>,
) -> (egui::FullOutput, egui::Rect) {
    let mut bounds = egui::Rect::NOTHING;
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            match pane {
                DiagnosticPane::Inspector => {
                    let before = studio.document.clone().unwrap();
                    studio.inspector(ui);
                    studio
                        .finish_edit_frame(before, ui.input(|input| input.pointer.primary_down()));
                }
                DiagnosticPane::Audio => {
                    studio.analysis_ui(ui);
                }
                DiagnosticPane::Results => studio.results_ui(ui),
                DiagnosticPane::Data => studio.library_ui(ui),
                DiagnosticPane::Search => studio.map_ui(ui),
            }
            bounds = ui.min_rect();
        },
    );
    (output, bounds)
}

fn diagnostic_text(output: &egui::FullOutput) -> Vec<(String, egui::Pos2)> {
    fn visit(shape: &egui::epaint::Shape, found: &mut Vec<(String, egui::Pos2)>) {
        match shape {
            egui::epaint::Shape::Text(text) => found.push((
                text.galley.text().to_owned(),
                text.pos + text.galley.size() / 2.0,
            )),
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    visit(shape, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for shape in &output.shapes {
        visit(&shape.shape, &mut found);
    }
    found
}

fn click_diagnostic_label(
    studio: &mut Studio,
    ctx: &egui::Context,
    pane: DiagnosticPane,
    size: [f32; 2],
    label: &str,
) -> egui::FullOutput {
    let _ = diagnostic_frame(studio, ctx, pane, size, vec![]);
    let (output, _) = diagnostic_frame(studio, ctx, pane, size, vec![]);
    let position = diagnostic_text(&output)
        .into_iter()
        .find(|(text, _)| text == label)
        .unwrap_or_else(|| panic!("Button not painted: {label}"))
        .1;
    for pressed in [true, false] {
        let _ = diagnostic_frame(
            studio,
            ctx,
            pane,
            size,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    diagnostic_frame(studio, ctx, pane, size, vec![]).0
}

fn click_settled_diagnostic_label(
    studio: &mut Studio,
    ctx: &egui::Context,
    pane: DiagnosticPane,
    size: [f32; 2],
    label: &str,
) -> egui::FullOutput {
    let mut output = click_diagnostic_label(studio, ctx, pane, size, label);
    // CollapsingHeader deliberately clips its body during expansion. RawInput
    // without a timestamp advances egui's clock by predicted_dt each frame;
    // advance the configured animation plus two layout frames before checking
    // final contents. This is simulated frame time, never a wall-clock sleep.
    let frames = (ctx.global_style().animation_time / egui::RawInput::default().predicted_dt).ceil()
        as usize
        + 2;
    assert!(frames <= 62, "Unexpected diagnostic animation duration");
    for _ in 0..frames {
        output = diagnostic_frame(studio, ctx, pane, size, vec![]).0;
    }
    output
}

#[test]
fn late_atmospheric_profile_cannot_overwrite_edits_made_during_loading() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let previous = studio.document.clone().unwrap();
    studio.document.as_mut().unwrap().notes = "Retain this newer review edit".into();
    studio.busy = Some("Loading atmospheric profile".into());
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::Atmosphere(atmospheric_fixture(), previous)),
    );
    tick(&mut studio);
    let current = studio.document.as_ref().unwrap();
    assert_eq!(current.notes, "Retain this newer review edit");
    assert!(current.atmosphere.is_none());
    assert!(studio.undo.is_empty());
    assert!(!studio.busy());
    assert!(
        studio
            .notice
            .as_ref()
            .unwrap()
            .1
            .contains("Your edits were kept")
    );
}

#[test]
fn imported_atmosphere_is_one_undo_step_and_survives_saved_and_loaded_replies() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let before = studio.document.clone().unwrap();
    studio.score = Some(modality_surface(true));
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::Atmosphere(
            atmospheric_fixture(),
            before.clone(),
        )),
    );
    tick(&mut studio);
    let imported = studio.document.clone().unwrap();
    assert!(imported.atmosphere.is_some());
    assert!(studio.score.is_none());
    assert_eq!(studio.undo.len(), 1);
    studio.undo();
    assert_eq!(studio.document.as_ref(), Some(&before));
    studio.redo();
    assert_eq!(studio.document.as_ref(), Some(&imported));
    let path = workspace::save(&f.case.manifest, &imported).unwrap();
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::Saved(path.clone(), imported.clone())),
    );
    tick(&mut studio);
    assert!(!studio.dirty());
    let loaded = workspace::load(&f.case.manifest, &path).unwrap();
    let mut reopened = f.studio();
    let previous = reopened.document.clone().unwrap();
    send(
        &reopened,
        reopened.generation,
        WorkerLane::Task,
        Ok(ResultValue::Loaded(loaded, Box::new(previous))),
    );
    tick(&mut reopened);
    assert_eq!(
        reopened.document.as_ref().unwrap().atmosphere,
        imported.atmosphere
    );
    assert!(!reopened.dirty());
}

#[test]
fn regional_setup_keeps_source_choices_empty_and_observations_unchanged() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let ctx = egui::Context::default();
    let doc = studio.document.as_mut().unwrap();
    doc.mix.thunder_timing = true;
    let mut reviewed =
        EvidenceEvent::manual("reviewed-thunder".into(), EventKind::Thunder, 10.0, 12.0);
    reviewed.review = ReviewState::Confirmed;
    reviewed.acoustic_source = workspace::AcousticSourceAssessment::Compatible;
    reviewed.selected_for_scoring = true;
    reviewed.strike_id = "previous-explicit-source".into();
    doc.events.push(reviewed);
    doc.events.push(EvidenceEvent::manual(
        "pending-thunder".into(),
        EventKind::Thunder,
        20.0,
        23.0,
    ));
    let observations = doc.events.clone();
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Search,
        [1500.0, 1100.0],
        "Set up regional search",
    );
    let doc = studio.document.as_ref().unwrap();
    assert_eq!(doc.events, observations);
    let plan = doc.regional_plan.as_ref().unwrap();
    assert_eq!(plan.events.len(), 1);
    assert_eq!(plan.events[0].event_id, "reviewed-thunder");
    assert!(
        plan.events[0].source_ids.is_empty(),
        "Even an existing association needs explicit inclusion in the regional alternatives"
    );
    assert_eq!(studio.undo.len(), 1);
    studio.undo();
    assert!(studio.document.as_ref().unwrap().regional_plan.is_none());
    assert_eq!(studio.document.as_ref().unwrap().events, observations);
    studio.redo();
    assert!(studio.document.as_ref().unwrap().regional_plan.is_some());
}

#[test]
fn draft_regional_plan_saves_without_forcing_unknown_associations() {
    let f = Fixture::new();
    let mut doc = f.studio().document.unwrap();
    let mut plan = geosonnar::regional_search::RegionalPlan::default();
    plan.clock.min_offset_s = -123.0;
    plan.clock.max_offset_s = 456.0;
    plan.events
        .push(geosonnar::regional_search::EventAlternatives {
            event_id: "not-yet-reviewed".into(),
            source_ids: vec![],
            emitted_frequency_hz: None,
        });
    doc.regional_plan = Some(plan);
    let path = workspace::save(&f.case.manifest, &doc).unwrap();
    let loaded = workspace::load(&f.case.manifest, &path).unwrap();
    assert_eq!(loaded.regional_plan, doc.regional_plan);
    assert!(loaded.events.is_empty());
}

#[test]
fn stale_regional_apply_cannot_replace_newer_review_edits() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let previous = studio.document.clone().unwrap();
    let mut applied = previous.clone();
    applied.map.clock_adjustment_s = 17.0;
    studio.document.as_mut().unwrap().notes = "New independent note".into();
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::RegionalApplied(
            Box::new(applied),
            Box::new(previous),
        )),
    );
    tick(&mut studio);
    assert_eq!(
        studio.document.as_ref().unwrap().notes,
        "New independent note"
    );
    assert_eq!(
        studio.document.as_ref().unwrap().map.clock_adjustment_s,
        0.0
    );
    assert!(studio.undo.is_empty());
    assert!(
        studio
            .notice
            .as_ref()
            .unwrap()
            .1
            .contains("newer edits were kept")
    );
}

#[test]
fn regional_apply_is_one_undo_step_and_does_not_confirm_events() {
    let f = Fixture::new();
    let mut studio = f.studio();
    studio
        .document
        .as_mut()
        .unwrap()
        .events
        .push(EvidenceEvent::manual(
            "pending".into(),
            EventKind::Thunder,
            2.0,
            4.0,
        ));
    let previous = studio.document.clone().unwrap();
    let mut next = previous.clone();
    next.map.clock_adjustment_s = 9.5;
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::RegionalApplied(
            Box::new(next.clone()),
            Box::new(previous.clone()),
        )),
    );
    tick(&mut studio);
    assert_eq!(studio.document.as_ref(), Some(&next));
    assert_eq!(studio.undo.len(), 1);
    assert_eq!(
        studio.document.as_ref().unwrap().events[0].review,
        ReviewState::Pending
    );
    studio.undo();
    assert_eq!(studio.document.as_ref(), Some(&previous));
    studio.redo();
    assert_eq!(studio.document.as_ref(), Some(&next));
}

#[test]
fn regional_cancellation_is_explicit_and_case_switch_cancels_old_worker() {
    let f = Fixture::new();
    let g = Fixture::new();
    let mut studio = f.studio();
    let ctx = egui::Context::default();
    studio.regional.running = true;
    let cancel = studio.regional.cancel.clone();
    assert!(!cancel.load(Ordering::Relaxed));
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Search,
        [1500.0, 1100.0],
        "Cancel regional search",
    );
    assert!(cancel.load(Ordering::Relaxed));
    studio.regional.cancel = Arc::new(AtomicBool::new(false));
    let second = studio.regional.cancel.clone();
    studio.bind(&g.case);
    assert!(second.load(Ordering::Relaxed));
    assert!(!studio.regional.running);
}

fn regional_estimate_fixture() -> geosonnar::regional_search::SearchEstimate {
    geosonnar::regional_search::SearchEstimate {
        observer_count: 2,
        clock_count: 3,
        event_count: 1,
        source_alternatives: 2,
        evaluations: 12,
        residual_evaluations: 12,
        trajectory_point_visits: 0,
    }
}

fn history_request_fixture() -> geosonnar::adsblol_history::HistoryRequest {
    geosonnar::adsblol_history::HistoryRequest {
        utc_start: "2026-05-12T12:25:00Z".into(),
        utc_end: "2026-05-12T12:45:00Z".into(),
        bounds: [44.0, -74.0, 46.0, -72.0],
        max_traces: 4,
        max_download_bytes: 20_000_000,
    }
}

fn fmi_request_fixture() -> geosonnar::fmi_lightning::FmiRequest {
    geosonnar::fmi_lightning::FmiRequest {
        start_utc: "2025-07-19T16:00:00Z".into(),
        end_utc: "2025-07-19T16:01:00Z".into(),
        bounds: [65.0, 13.0, 66.0, 15.0],
        max_total_bytes: 8 * 1024 * 1024,
    }
}

fn fmi_item_fixture(
    status: data_catalog::fmi::FmiStatus,
    owned_rows: usize,
) -> data_catalog::fmi::FmiItem {
    let plan = geosonnar::fmi_lightning::plan(&fmi_request_fixture()).unwrap();
    data_catalog::fmi::FmiItem {
        entry: data_catalog::CatalogEntry {
            schema_version: 1,
            id: "fabricated-fmi".into(),
            case_manifest_sha256: "c".repeat(64),
            label: "Fabricated FMI report".into(),
            kind: data_catalog::DataKind::Lightning,
            format: "fmi_lightning_download".into(),
            primary_file: "fabricated-report.json".into(),
            created_unix_s: 0,
            source: json!({"fabricated":true}),
            files: vec![],
            date_utc: Some("2025-07-19".into()),
            bounds: Some(plan.request.bounds),
            notes: "Offline presentation fixture".into(),
        },
        report: Some(data_catalog::fmi::FmiReport {
            schema_version: 1,
            plan_sha256: "a".repeat(64),
            status,
            chunks_total: 1,
            chunks_complete: usize::from(matches!(status, data_catalog::fmi::FmiStatus::Complete)),
            raw_rows: owned_rows,
            owned_rows,
            transferred_bytes: 123,
            attempts: 1,
            errors: vec!["Fabricated request diagnostic".into()],
            source_coverage_complete: false,
            location_verified: false,
        }),
        plan,
    }
}

fn fmi_catalog_fixture(f: &Fixture) -> evidence_score::LightningCatalog {
    use geosonnar::lightning_source::{FmiRecord, FmiSourceMetadata};
    let provenance = evidence_score::LightningProvenance {
        provider: "FMI / NORDLIS fabricated GUI control".into(),
        source: "Fabricated source metadata; no network request".into(),
        license: Some("CC BY 4.0".into()),
        notes: Some("Synthetic source for lifecycle testing only".into()),
        altitude_datum: evidence_score::LightningAltitudeDatum::Unknown,
        source_metadata: Some(FmiSourceMetadata::default()),
    };
    let document = evidence_score::LightningDocument {
        schema_version: 1,
        provenance: Some(provenance.clone()),
        strikes: vec![evidence_score::LightningStrike {
            id: "fmi-fabricated-0".into(),
            time_utc_s: 1_752_940_800.0,
            latitude_deg: 65.5,
            longitude_deg: 14.0,
            altitude_m: None,
            intensity: None,
            source_record: Some(FmiRecord {
                response_sha256: "d".repeat(64),
                row_ordinal: 0,
                multiplicity: 1,
                peak_current_ka: -12.5,
                cloud_indicator: 0,
                ellipse_major_raw: 1.5,
            }),
        }],
    };
    let path = f.root.join("fabricated-fmi.json");
    fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    evidence_score::import_lightning(&path, &provenance).unwrap()
}

#[test]
fn fmi_plan_is_local_and_request_edits_disable_download_without_mutating_search() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let before = studio.document.clone();
    let ctx = egui::Context::default();
    studio.data.show_fmi();
    studio.data.fmi.request = fmi_request_fixture();
    let size = [1400.0, 1100.0];
    let frame = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Data,
        size,
        "Prepare FMI request",
    );
    let text = diagnostic_text(&frame)
        .into_iter()
        .map(|v| v.0)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(studio.data.fmi.plan_current());
    assert!(studio.data.fmi.catalog_id.is_none());
    assert!(text.contains("does not certify availability"), "{text}");
    assert!(text.contains("Quebec"), "{text}");
    assert!(!studio.busy() && studio.data.downloading.is_none());
    studio.data.fmi.request.max_total_bytes += 1;
    let frame = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Data,
        size,
        "Download / resume reviewed FMI request",
    );
    assert!(
        diagnostic_text(&frame)
            .iter()
            .any(|(s, _)| s.contains("Request changed"))
    );
    assert!(!studio.busy() && studio.data.downloading.is_none());
    assert_eq!(studio.document, before);
    assert!(studio.undo.is_empty() && studio.lightning.is_none());
}

#[test]
fn fmi_partial_and_empty_reports_never_enable_import_and_panels_resize() {
    use data_catalog::fmi::FmiStatus;
    let f = Fixture::new();
    for (status, count, message) in [
        (FmiStatus::Partial, 3, "Partial request"),
        (FmiStatus::Cancelled, 3, "Cancelled request"),
        (FmiStatus::Complete, 0, "No rows to import"),
    ] {
        let mut studio = f.studio();
        let before = studio.document.clone();
        studio.data.show_fmi();
        studio.data.fmi.accept_item(fmi_item_fixture(status, count));
        let ctx = egui::Context::default();
        let frame = click_diagnostic_label(
            &mut studio,
            &ctx,
            DiagnosticPane::Data,
            [1400.0, 1200.0],
            "Import verified FMI strikes into library",
        );
        assert!(
            diagnostic_text(&frame)
                .iter()
                .any(|(text, _)| text.contains(message))
        );
        assert!(!studio.busy() && studio.lightning.is_none() && studio.layers.is_empty());
        assert_eq!(studio.document, before);
        for size in [[760.0, 560.0], [1400.0, 1000.0], [760.0, 560.0]] {
            diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Data, size, vec![]);
            let (_, bounds) =
                diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Data, size, vec![]);
            assert!(
                bounds.right() <= size[0] + 1.0 && bounds.bottom() <= size[1] + 1.0,
                "{size:?}: {bounds:?}"
            );
        }
    }
}

#[test]
fn fmi_progress_and_terminal_replies_cannot_clear_a_newer_run_or_case() {
    let f = Fixture::new();
    let other = Fixture::new();
    let mut studio = f.studio();
    studio.data.progress = Some(data_catalog::DownloadProgress {
        planned_bytes: 1024,
        downloaded_bytes: 1024,
        complete: true,
        notes: vec!["Unrelated completed archive".into()],
    });
    // Reopening is a local-only FMI operation; even its busy row must not reuse
    // the progress left behind by an unrelated archive transfer.
    studio.reopen_fmi(&egui::Context::default(), "missing-fmi-fixture".into());
    assert!(studio.data.progress.is_none());
    finish_studio_task(&mut studio);
    studio.notice = None;
    studio.data.fmi.run_id = 4;
    studio.data.downloading = Some("fmi".into());
    studio.busy = Some("Newer FMI request".into());
    send(
        &studio,
        studio.generation,
        WorkerLane::FmiTask(3),
        Err("old failure".into()),
    );
    let progress = data_catalog::fmi::FmiProgress {
        chunks_total: 2,
        chunks_complete: 1,
        transferred_bytes: 123,
        message: "Fabricated progress".into(),
    };
    send(
        &studio,
        studio.generation,
        WorkerLane::TransferProgress,
        Ok(ResultValue::FmiProgress(3, progress.clone())),
    );
    tick(&mut studio);
    assert_eq!(studio.busy.as_deref(), Some("Newer FMI request"));
    assert!(studio.notice.is_none() && studio.data.fmi.progress.is_none());
    send(
        &studio,
        studio.generation,
        WorkerLane::TransferProgress,
        Ok(ResultValue::FmiProgress(4, progress)),
    );
    tick(&mut studio);
    assert_eq!(
        studio.data.fmi.progress.as_ref().unwrap().chunks_complete,
        1
    );
    let cancel = studio.download_cancel.clone();
    let old_generation = studio.generation;
    studio.bind(&other.case);
    studio.data.refresh_requested = false;
    send(
        &studio,
        old_generation,
        WorkerLane::FmiTask(4),
        Err("old case failure".into()),
    );
    tick(&mut studio);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(!studio.busy() && studio.notice.is_none() && studio.data.fmi.progress.is_none());
}

#[test]
fn fmi_completion_preserves_changed_draft_and_import_requires_a_separate_load() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let before = studio.document.clone();
    let initial = fmi_request_fixture();
    studio.data.fmi.request = initial.clone();
    studio.data.fmi.request.end_utc = "2025-07-19T16:02:00Z".into();
    let newer = studio.data.fmi.request.clone();
    let item = fmi_item_fixture(data_catalog::fmi::FmiStatus::Complete, 3);
    studio.accept_fmi_reply(fmi_ui::FmiReply {
        request_at_start: initial,
        outcome: fmi_ui::FmiOutcome::Stored(Box::new(item.clone())),
    });
    assert_eq!(studio.data.fmi.request, newer);
    assert!(studio.data.fmi.plan.is_none() && studio.notice.as_ref().unwrap().0);
    studio.accept_fmi_reply(fmi_ui::FmiReply {
        request_at_start: newer.clone(),
        outcome: fmi_ui::FmiOutcome::Imported(Box::new(item.entry)),
    });
    assert!(studio.data.fmi.imported.is_some());
    assert_eq!(studio.data.fmi.step, 3);
    assert!(studio.lightning.is_none() && studio.layers.is_empty());
    assert_eq!(studio.document, before);
    assert!(studio.undo.is_empty());
    let ctx = egui::Context::default();
    studio.data.show_fmi();
    let (frame, _) = diagnostic_frame(
        &mut studio,
        &ctx,
        DiagnosticPane::Data,
        [1400.0, 1200.0],
        vec![],
    );
    assert!(
        diagnostic_text(&frame)
            .iter()
            .any(|(s, _)| s == "Load imported FMI strikes")
    );
}

#[test]
fn fmi_source_interpretation_is_explicit_source_bound_and_does_not_adjust_scales_or_events() {
    let f = Fixture::new();
    let mut studio = f.studio();
    studio.lightning = Some(fmi_catalog_fixture(&f));
    studio.document.as_mut().unwrap().mix.thunder_scale_s = 0.5;
    let event = EvidenceEvent::manual("unchanged-thunder".into(), EventKind::Thunder, 1.0, 4.0);
    studio.document.as_mut().unwrap().events.push(event);
    let ctx = egui::Context::default();
    let size = [1400.0, 1600.0];
    diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Data, size, vec![]);
    studio.data.fmi.approximation_accepted = true;
    studio.data.fmi.rationale =
        "Synthetic explicit regional approximation; no surveyed datum transform".into();
    studio.data.fmi.allowance_s = 1.0;
    let before = studio.document.clone().unwrap();
    let frame = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Data,
        size,
        "Apply declared lightning source assumptions",
    );
    assert!(
        diagnostic_text(&frame)
            .iter()
            .any(|(s, _)| s.contains("smaller than the declared FMI"))
    );
    assert_eq!(studio.document.as_ref(), Some(&before));
    assert!(studio.undo.is_empty());
    studio.document.as_mut().unwrap().mix.thunder_scale_s = 2.0;
    let before = studio.document.clone().unwrap();
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Data,
        size,
        "Apply declared lightning source assumptions",
    );
    let after = studio.document.as_ref().unwrap();
    let policy = after.mix.lightning_source_policy.as_ref().unwrap();
    assert_eq!(
        policy.catalog_sha256,
        studio.lightning.as_ref().unwrap().input_sha256
    );
    assert_eq!(policy.time_representation_allowance_s, 1.0);
    let mut restored = after.clone();
    restored.mix.lightning_source_policy = None;
    assert_eq!(restored, before);
    assert_eq!(studio.undo.len(), 1);
    studio.undo();
    assert_eq!(studio.document.as_ref(), Some(&before));
}

#[test]
fn fmi_copy_uses_shared_clock_uncertainty_and_refuses_to_shorten_large_requests() {
    let f = Fixture::new();
    let mut doc = f.studio().document.unwrap();
    doc.recording_start_utc = "2025-07-19T16:00:00Z".into();
    doc.map.bounds = [65.0, 13.0, 66.0, 15.0];
    doc.map.clock_adjustment_s = 30.0;
    let request = fmi_ui::copy_fmi_search_window(&doc, &fmi_request_fixture(), 0.0, 120.0).unwrap();
    assert_eq!(request.start_utc, "2025-07-19T15:58:30Z");
    assert_eq!(request.end_utc, "2025-07-19T16:01:30Z");
    assert_eq!(request.bounds, doc.map.bounds);
    let mut plan = geosonnar::regional_search::RegionalPlan::default();
    plan.clock.min_offset_s = -30.0;
    plan.clock.max_offset_s = 60.0;
    doc.regional_plan = Some(plan);
    let uncertain =
        fmi_ui::copy_fmi_search_window(&doc, &fmi_request_fixture(), 0.0, 120.0).unwrap();
    assert_eq!(uncertain.start_utc, "2025-07-19T15:57:30Z");
    assert_eq!(uncertain.end_utc, "2025-07-19T16:02:00Z");
    doc.duration_s = 7200.0;
    assert!(fmi_ui::copy_fmi_search_window(&doc, &fmi_request_fixture(), 0.0, 120.0).is_err());
}

fn history_item_fixture() -> data_catalog::HistoryItem {
    use geosonnar::adsblol_history::*;
    let plan = plan(&history_request_fixture()).unwrap();
    let artifact = Artifact {
        path: "fabricated.json".into(),
        bytes: 1,
        sha256: "a".repeat(64),
    };
    let report = HistoryReport {
        schema_version: 1,
        plan: plan.clone(),
        plan_sha256: "b".repeat(64),
        status: HistoryStatus::Partial,
        replay: vec![],
        index: artifact.clone(),
        candidates: vec![Candidate {
            icao24: "abcdef".into(),
            observations: 2,
            first_utc_s: plan.start_utc_s,
            last_utc_s: plan.end_utc_s,
            source_codes: vec![2],
            source_types: vec!["mlat".into()],
            selected: true,
            omission: None,
        }],
        traces: vec![TraceResult {
            icao24: "abcdef".into(),
            url: "https://example.invalid/fabricated".into(),
            receipt: None,
            decoded: Some(artifact),
            trace_points: Some(2),
            error: None,
        }],
        attempts: vec![],
        http_payload_bytes: 123,
        new_http_payload_bytes: 123,
        reused_files: 0,
        non_icao_omissions: 2,
        trace_cap_omissions: 3,
        replay_files_complete: false,
        replay_timing_gaps: true,
        selected_traces_complete: true,
        coverage_complete: false,
        location_verified: false,
        report_path: "fabricated-report.json".into(),
        limitations: vec!["Fabricated offline presentation fixture".into()],
    };
    data_catalog::HistoryItem {
        entry: data_catalog::CatalogEntry {
            schema_version: 1,
            id: "fabricated-history".into(),
            case_manifest_sha256: "c".repeat(64),
            label: "Fabricated history".into(),
            kind: data_catalog::DataKind::Adsb,
            format: "adsblol_history".into(),
            primary_file: "fabricated-report.json".into(),
            created_unix_s: 0,
            source: json!({"fabricated":true}),
            files: vec![],
            date_utc: Some(plan.date_utc.clone()),
            bounds: Some(plan.request.bounds),
            notes: "Offline fixture".into(),
        },
        plan,
        report: Some(report),
    }
}

#[test]
fn history_planning_is_local_explicit_and_edits_disable_the_old_fetch() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let ctx = egui::Context::default();
    let before = studio.document.clone();
    studio.data.show_history();
    studio.data.history.request = history_request_fixture();
    let size = [1400.0, 1000.0];
    let output = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Data,
        size,
        "Prepare bounded history request",
    );
    let text = diagnostic_text(&output)
        .into_iter()
        .map(|v| v.0)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(studio.data.history.plan_current());
    assert!(text.contains("not an availability check"), "{text}");
    assert!(text.contains("never complete sky coverage"), "{text}");
    assert!(!studio.busy());
    assert!(studio.data.history.catalog_id.is_none());
    assert_eq!(studio.document, before);
    studio.data.history.request.max_traces += 1;
    let output = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Data,
        size,
        "Fetch reviewed history request",
    );
    let text = diagnostic_text(&output)
        .into_iter()
        .map(|v| v.0)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Request changed"), "{text}");
    assert!(
        !studio.busy(),
        "An obsolete plan cannot initiate a transfer"
    );
    assert!(studio.data.downloading.is_none());
    assert_eq!(studio.document, before);
}

#[test]
fn partial_history_receipt_requires_explicit_import_and_preserves_evidence() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let ctx = egui::Context::default();
    let before = studio.document.clone();
    studio.data.show_history();
    studio.data.history.accept_item(history_item_fixture());
    let output = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Data,
        [1500.0, 1200.0],
        "Import reviewed historical traces into library",
    );
    let text = diagnostic_text(&output)
        .into_iter()
        .map(|v| v.0)
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Partial request",
        "3 omitted by cap",
        "2 non-ICAO omissions",
        "Missing aircraft are not proof of absence",
        "allow importing available traces",
    ] {
        assert!(text.contains(expected), "Missing {expected}: {text}");
    }
    assert!(
        !studio.busy(),
        "Partial import remains disabled until reviewed"
    );
    assert!(studio.layers.is_empty());
    assert_eq!(studio.document, before);
    assert!(studio.undo.is_empty());
    for size in [[760.0, 560.0], [1400.0, 900.0], [760.0, 560.0]] {
        for _ in 0..2 {
            let (_, bounds) =
                diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Data, size, vec![]);
            assert!(
                bounds.right() <= size[0] + 1.0,
                "width {bounds:?} at {size:?}"
            );
            assert!(
                bounds.bottom() <= size[1] + 1.0,
                "height {bounds:?} at {size:?}"
            );
        }
    }
}

#[test]
fn historical_progress_stays_in_its_run_and_case_switch_requests_cancellation() {
    use geosonnar::adsblol_history::HistoryProgress;
    let f = Fixture::new();
    let other = Fixture::new();
    let mut studio = f.studio();
    studio.data.history.run_id = 2;
    studio.busy = Some("Fetching history".into());
    let progress = HistoryProgress {
        stage: "traces".into(),
        completed_files: 1,
        selected_files: 4,
        http_payload_bytes: 100,
        max_download_bytes: 1000,
    };
    send(
        &studio,
        studio.generation,
        WorkerLane::TransferProgress,
        Ok(ResultValue::HistoryProgress(1, progress.clone())),
    );
    tick(&mut studio);
    assert!(studio.data.history.progress.is_none());
    assert!(studio.busy());
    send(
        &studio,
        studio.generation,
        WorkerLane::TransferProgress,
        Ok(ResultValue::HistoryProgress(2, progress)),
    );
    tick(&mut studio);
    assert_eq!(
        studio
            .data
            .history
            .progress
            .as_ref()
            .unwrap()
            .completed_files,
        1
    );
    assert!(studio.busy());
    let cancel = studio.download_cancel.clone();
    studio.bind(&other.case);
    assert!(cancel.load(Ordering::Relaxed));
}

#[test]
fn history_copy_uses_clock_uncertainty_and_padding_without_shrinking_unsupported_requests() {
    let f = Fixture::new();
    let mut doc = f.studio().document.unwrap();
    doc.recording_start_utc = "2026-05-12T12:34:56Z".into();
    doc.map.bounds = [45.0, -73.0, 45.1, -72.9];
    let mut plan = geosonnar::regional_search::RegionalPlan::default();
    plan.clock.min_offset_s = -10.0;
    plan.clock.max_offset_s = 20.0;
    doc.regional_plan = Some(plan);
    let request =
        history_ui::copy_search_request(&doc, 100.0, 600.0, &history_request_fixture()).unwrap();
    assert_eq!(request.utc_start, "2026-05-12T12:24:46Z");
    assert_eq!(request.utc_end, "2026-05-12T12:36:16Z");
    assert!(request.bounds[0] < doc.map.bounds[0] && request.bounds[1] < doc.map.bounds[1]);
    doc.duration_s = 7200.0;
    assert!(
        history_ui::copy_search_request(&doc, 100.0, 600.0, &history_request_fixture()).is_err(),
        "Oversized windows must be split explicitly, not silently truncated"
    );
}

fn regional_report_fixture(
    doc: &ReviewWorkspace,
    cancelled: bool,
) -> geosonnar::regional_search::RegionalReport {
    use geosonnar::regional_search::*;
    RegionalReport {
        schema_version: 1,
        document_sha256: document_sha256(doc).unwrap(),
        inputs_sha256: "a".repeat(64),
        observers_sha256: "b".repeat(64),
        plan: RegionalPlan::default(),
        status: if cancelled {
            SearchStatus::Cancelled
        } else {
            SearchStatus::Completed
        },
        estimate: regional_estimate_fixture(),
        progress: SearchProgress {
            completed_evaluations: if cancelled { 4 } else { 12 },
            total_evaluations: 12,
            completed_cells: if cancelled { 2 } else { 6 },
            total_cells: 6,
        },
        hypotheses: vec![RegionalHypothesis {
            id: "fabricated-hypothesis-a".into(),
            rank: 1,
            score: 0.25,
            observer: evidence_score::ObserverLocation {
                latitude_deg: 45.0,
                longitude_deg: -73.0,
                ellipsoid_height_m: 50.0,
            },
            clock_offset_s: 5.0,
            recording_start_utc_s: 1_778_589_296.0,
            assignments: vec![EventAssignment {
                event_id: "fabricated-event".into(),
                kind: EventKind::Thunder,
                source_id: "candidate-a".into(),
                emitted_frequency_hz: None,
                frequency_at_boundary: false,
                weighted_contribution: 0.25,
                terms: vec![],
            }],
            clock_at_boundary: true,
            frequency_at_boundary: false,
            spatial_at_boundary: true,
        }],
        location_summaries: vec![],
        unavailable: vec![],
        unavailable_count: 0,
        rankable_cells: if cancelled { 2 } else { 6 },
        unavailable_cells: 0,
        supported_hypotheses: Some(12),
        retained_truncated: true,
        location_verified: false,
        limitations: vec!["Fabricated presentation fixture; no location accuracy claim".into()],
    }
}

#[test]
fn regional_source_checkboxes_only_change_the_approved_search_pool() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let ctx = egui::Context::default();
    let doc = studio.document.as_mut().unwrap();
    doc.mix.thunder_timing = true;
    let mut event = EvidenceEvent::manual("event-a".into(), EventKind::Thunder, 10.0, 15.0);
    event.review = ReviewState::Confirmed;
    event.acoustic_source = workspace::AcousticSourceAssessment::Compatible;
    event.selected_for_scoring = true;
    doc.events.push(event);
    let events = doc.events.clone();
    studio.lightning=Some(serde_json::from_value(json!({
        "schema_version":1,"input_path":"fabricated-lightning.json","input_bytes":1,"input_sha256":"a".repeat(64),
        "provenance":{"provider":"Fabricated GUI control","source":"Test","license":null,"notes":null,"altitude_datum":"wgs84_ellipsoid"},
        "strikes":[{"id":"candidate-a","time_utc_s":1778589296.0,"latitude_deg":45.0,"longitude_deg":-73.0,"altitude_m":0.0,"intensity":null},
        {"id":"candidate-b","time_utc_s":1778589306.0,"latitude_deg":45.1,"longitude_deg":-73.0,"altitude_m":0.0,"intensity":null}]
    })).unwrap());
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Search,
        [1600.0, 1400.0],
        "Set up regional search",
    );
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Search,
        [1600.0, 1400.0],
        "2 Evidence",
    );
    assert!(
        studio
            .document
            .as_ref()
            .unwrap()
            .regional_plan
            .as_ref()
            .unwrap()
            .events[0]
            .source_ids
            .is_empty()
    );
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Search,
        [1600.0, 1400.0],
        "candidate-a",
    );
    assert_eq!(
        studio
            .document
            .as_ref()
            .unwrap()
            .regional_plan
            .as_ref()
            .unwrap()
            .events[0]
            .source_ids,
        ["candidate-a"]
    );
    assert_eq!(studio.document.as_ref().unwrap().events, events);
    studio.undo();
    assert!(
        studio
            .document
            .as_ref()
            .unwrap()
            .regional_plan
            .as_ref()
            .unwrap()
            .events[0]
            .source_ids
            .is_empty()
    );
    assert_eq!(studio.document.as_ref().unwrap().events, events);
}

#[test]
fn regional_estimate_from_old_document_is_not_accepted() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let previous = studio.document.clone().unwrap();
    studio.document.as_mut().unwrap().map.grid_spacing_m = 500.0;
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::RegionalEstimated(
            regional_estimate_fixture(),
            Box::new(previous),
        )),
    );
    tick(&mut studio);
    assert!(studio.regional.estimate.is_none());
    assert!(studio.notice.as_ref().unwrap().1.contains("Plan changed"));
}

#[test]
fn regional_results_are_reviewable_without_applying_and_resize_headlessly() {
    for cancelled in [false, true] {
        let f = Fixture::new();
        let mut studio = f.studio();
        let ctx = egui::Context::default();
        let previous = studio.document.clone().unwrap();
        let report = regional_report_fixture(&previous, cancelled);
        send(
            &studio,
            studio.generation,
            WorkerLane::Task,
            Ok(ResultValue::RegionalSearched(
                Box::new(report),
                Box::new(previous.clone()),
            )),
        );
        tick(&mut studio);
        for size in [[1500.0, 1100.0], [760.0, 560.0], [1000.0, 700.0]] {
            for _ in 0..2 {
                diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Results, size, vec![]);
            }
            let (output, bounds) =
                diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Results, size, vec![]);
            let text = diagnostic_text(&output)
                .into_iter()
                .map(|(text, _)| text)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                text.contains(if cancelled {
                    "Cancelled — incomplete search"
                } else {
                    "Completed declared search"
                }),
                "{text}"
            );
            assert!(
                bounds.width() <= size[0] + 2.0,
                "width {bounds:?} at {size:?}"
            );
            assert!(
                bounds.height() <= size[1] + 2.0,
                "height {bounds:?} at {size:?}"
            );
            assert_eq!(studio.document.as_ref(), Some(&previous));
        }
    }
}

fn regional_comparison_fixture(
    doc: &ReviewWorkspace,
    incomplete: bool,
) -> geosonnar::regional_search::ModalityReport {
    use geosonnar::regional_search::*;
    let modes = [
        SearchMode::DopplerOnly,
        SearchMode::ThunderOnly,
        SearchMode::Combined,
    ];
    ModalityReport {
        schema_version: 1,
        document_sha256: document_sha256(doc).unwrap(),
        inputs_sha256: "a".repeat(64),
        request: ModalityPlan {
            regional: RegionalPlan::default(),
            modes: modes.to_vec(),
            max_total_evaluations: 1000,
        },
        status: if incomplete {
            SearchStatus::Cancelled
        } else {
            SearchStatus::Completed
        },
        estimate: ModalityEstimate {
            modes: modes
                .iter()
                .map(|&mode| ModeEstimate {
                    mode,
                    estimate: regional_estimate_fixture(),
                })
                .collect(),
            evaluations: 36,
            residual_evaluations: 36,
            trajectory_point_visits: 0,
        },
        progress: ModalityProgress {
            mode: SearchMode::ThunderOnly,
            mode_progress: SearchProgress::default(),
            completed_evaluations: if incomplete { 16 } else { 36 },
            total_evaluations: 36,
            completed_modes: if incomplete { 1 } else { 3 },
            total_modes: 3,
        },
        modes: modes
            .iter()
            .enumerate()
            .map(|(index, &mode)| {
                let mut report = regional_report_fixture(doc, incomplete && index == 1);
                report.hypotheses[0].observer.latitude_deg += index as f64 * 0.1;
                report.hypotheses[0].clock_offset_s += index as f64 * 10.0;
                let best = &report.hypotheses[0];
                report.location_summaries = vec![LocationSummary {
                    observer_index: index,
                    observer: best.observer.clone(),
                    completed_clocks: 3,
                    rankable_clocks: 3,
                    unavailable_clocks: 0,
                    best_score: Some(best.score),
                    best_clock_offset_s: Some(best.clock_offset_s),
                    best_hypothesis_id: Some(best.id.clone()),
                    best_assignments: best
                        .assignments
                        .iter()
                        .map(LocationAssignment::from)
                        .collect(),
                    spatial_at_boundary: false,
                }];
                ModeResult {
                    mode,
                    status: if incomplete && index == 2 {
                        ModeStatus::NotStarted
                    } else if incomplete && index == 1 {
                        ModeStatus::Cancelled
                    } else {
                        ModeStatus::Completed
                    },
                    report: if incomplete && index == 2 {
                        None
                    } else {
                        Some(report)
                    },
                }
            })
            .collect(),
        location_verified: false,
        limitations: vec!["Fabricated display fixture".into()],
    }
}

#[test]
fn independent_mode_review_shows_clock_disagreement_without_changing_evidence() {
    use geosonnar::regional_search::SearchMode;
    let f = Fixture::new();
    let mut studio = f.studio();
    let ctx = egui::Context::default();
    let before = studio.document.clone().unwrap();
    studio
        .regional
        .accept_comparison(regional_comparison_fixture(&before, false), before.clone());
    let output = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Results,
        [1600.0, 1300.0],
        "Doppler only",
    );
    let text = diagnostic_text(&output)
        .into_iter()
        .map(|v| v.0)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(studio.regional.mode, SearchMode::DopplerOnly);
    assert!(text.contains("clock difference 10.000s"), "{text}");
    assert!(text.contains("shared cells"), "{text}");
    assert!(
        text.contains("spatial overlap does not establish a shared time"),
        "{text}"
    );
    assert_eq!(studio.document.as_ref(), Some(&before));
    assert!(studio.undo.is_empty());
    for size in [[760.0, 560.0], [1400.0, 900.0], [760.0, 560.0]] {
        for _ in 0..2 {
            let (_, bounds) =
                diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Results, size, vec![]);
            assert!(
                bounds.right() <= size[0] + 1.0,
                "width {bounds:?} at {size:?}"
            );
            assert!(
                bounds.bottom() <= size[1] + 1.0,
                "height {bounds:?} at {size:?}"
            );
        }
    }
}

#[test]
fn not_started_mode_is_reviewed_as_missing_evidence_and_preserves_partial_comparison() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let ctx = egui::Context::default();
    let before = studio.document.clone().unwrap();
    studio
        .regional
        .accept_comparison(regional_comparison_fixture(&before, true), before.clone());
    let output = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Results,
        [1600.0, 1300.0],
        "Combined",
    );
    let text = diagnostic_text(&output)
        .into_iter()
        .map(|v| v.0)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Comparison incomplete"), "{text}");
    assert!(text.contains("absence is not negative evidence"), "{text}");
    assert!(studio.regional.report.is_none());
    assert!(studio.regional.comparison.is_some());
    assert_eq!(studio.document.as_ref(), Some(&before));
}

#[test]
fn all_receiver_rows_remain_selectable_outside_the_retained_combination_shortlist() {
    use geosonnar::regional_search::*;
    let f = Fixture::new();
    let mut studio = f.studio();
    let ctx = egui::Context::default();
    let before = studio.document.clone().unwrap();
    let mut report = regional_report_fixture(&before, false);
    report.location_summaries.push(LocationSummary {
        observer_index: 7,
        observer: evidence_score::ObserverLocation {
            latitude_deg: 46.0,
            longitude_deg: -72.0,
            ellipsoid_height_m: 80.0,
        },
        completed_clocks: 3,
        rankable_clocks: 3,
        unavailable_clocks: 0,
        best_score: Some(0.7),
        best_clock_offset_s: Some(9.0),
        best_hypothesis_id: Some("outside-shortlist".into()),
        best_assignments: vec![LocationAssignment {
            event_id: "fabricated-event".into(),
            kind: EventKind::Thunder,
            source_id: "candidate-b".into(),
            emitted_frequency_hz: None,
        }],
        spatial_at_boundary: false,
    });
    studio.regional.report = Some(report);
    studio.regional.report_document = Some(before.clone());
    studio.regional.show_results = true;
    let size = [1600.0, 1300.0];
    diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Results, size, vec![]);
    let (output, _) = diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Results, size, vec![]);
    let row = diagnostic_text(&output)
        .into_iter()
        .find(|(text, _)| text.contains("Cell 00007"))
        .unwrap()
        .0;
    let output = click_diagnostic_label(&mut studio, &ctx, DiagnosticPane::Results, size, &row);
    let text = diagnostic_text(&output)
        .into_iter()
        .map(|v| v.0)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Receiver 7 · 46.000000, -72.000000"),
        "{text}"
    );
    assert!(text.contains("candidate-b"), "{text}");
    assert_eq!(studio.document.as_ref(), Some(&before));
    assert!(studio.undo.is_empty());
}

#[test]
fn old_height_preview_cannot_replace_newer_draft() {
    use geosonnar::receiver_heights::{ReceiverHeightReport, ReceiverTerrain, SurfaceDatum};
    let f = Fixture::new();
    let mut studio = f.studio();
    let previous = studio.document.clone().unwrap();
    let old = ReceiverTerrain {
        surface_sha256s: vec!["a".repeat(64)],
        surface_datum: SurfaceDatum::Wgs84Ellipsoid,
        geoid: None,
        above_surface_m: 2.0,
        notes: "Old draft".into(),
    };
    let mut current = old.clone();
    current.above_surface_m = 4.0;
    studio.heights.draft = Some(current.clone());
    let report = ReceiverHeightReport {
        schema_version: 1,
        settings: Some(old),
        observers: vec![],
        samples: vec![],
        unavailable: vec![],
        sources: vec![],
        limitations: vec![],
    };
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::HeightPreview(
            Box::new(report),
            Box::new(previous),
        )),
    );
    tick(&mut studio);
    assert!(studio.heights.preview.is_none());
    assert_eq!(studio.heights.draft, Some(current));
    assert!(studio.document.as_ref().unwrap().receiver_terrain.is_none());
}

#[test]
fn refinement_is_an_explicit_undoable_plan_and_keeps_all_approved_sources() {
    use geosonnar::regional_search::{EventAlternatives, RegionalPlan};
    let f = Fixture::new();
    let mut studio = f.studio();
    let ctx = egui::Context::default();
    let mut plan = RegionalPlan::default();
    plan.events.push(EventAlternatives {
        event_id: "event-a".into(),
        source_ids: vec!["candidate-a".into(), "candidate-b".into()],
        emitted_frequency_hz: None,
    });
    studio.document.as_mut().unwrap().regional_plan = Some(plan.clone());
    let previous = studio.document.clone().unwrap();
    let report = regional_report_fixture(&previous, false);
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::RegionalSearched(
            Box::new(report),
            Box::new(previous.clone()),
        )),
    );
    tick(&mut studio);
    let size = [1600.0, 1400.0];
    diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Results, size, vec![]);
    let (frame, _) = diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Results, size, vec![]);
    let row = diagnostic_text(&frame)
        .into_iter()
        .find(|(text, _)| text.contains("#") && text.contains("candidate-a"))
        .unwrap()
        .0;
    click_diagnostic_label(&mut studio, &ctx, DiagnosticPane::Results, size, &row);
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Results,
        size,
        "Prepare a finer search around the selected hypothesis",
    );
    // Finish the opening animation before targeting a control inside the
    // expanding body. Painted text can precede its final interactive bounds.
    for _ in 0..20 {
        diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Results, size, vec![]);
    }
    click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Results,
        size,
        "Prepare refinement plan",
    );
    let next = studio.document.as_ref().unwrap();
    assert_eq!(next.regional_plan.as_ref().unwrap().events, plan.events);
    assert_eq!(
        next.regional_plan.as_ref().unwrap().clock.min_offset_s,
        -5.0,
        "Refinement notice: {:?}",
        studio.notice
    );
    assert_eq!(
        next.regional_plan.as_ref().unwrap().clock.max_offset_s,
        15.0
    );
    assert!(next.map.bounds[0] < 45.0 && next.map.bounds[2] > 45.0);
    assert!(next.map.bounds[1] < -73.0 && next.map.bounds[3] > -73.0);
    assert_eq!(next.events, previous.events);
    assert!(!studio.busy());
    assert_eq!(studio.undo.len(), 1);
    studio.undo();
    assert_eq!(studio.document.as_ref(), Some(&previous));
}

#[test]
fn results_render_both_disagreement_and_complementary_overlap_without_hiding_scores() {
    for overlap in [false, true] {
        let f = Fixture::new();
        let mut studio = f.studio();
        let ctx = egui::Context::default();
        studio.score = Some(modality_surface(overlap));
        studio.score_document = studio.document.clone();
        let before = serde_json::to_vec(studio.score.as_ref().unwrap()).unwrap();
        let _ = diagnostic_frame(
            &mut studio,
            &ctx,
            DiagnosticPane::Results,
            [1500.0, 1100.0],
            vec![],
        );
        let (output, _) = diagnostic_frame(
            &mut studio,
            &ctx,
            DiagnosticPane::Results,
            [1500.0, 1100.0],
            vec![],
        );
        let text = diagnostic_text(&output)
            .into_iter()
            .map(|x| x.0)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains(if overlap {
                "their low-score areas overlap"
            } else {
                "low-score areas do not overlap"
            }),
            "{text}"
        );
        for expected in [
            "Doppler",
            "Thunder",
            "Combined",
            "apart",
            "shared low-score cells",
            "Uncalibrated comparison",
        ] {
            assert!(text.contains(expected), "Missing {expected}: {text}");
        }
        assert_eq!(studio.score.as_ref().unwrap().ranked_candidate_ids.len(), 3);
        assert_eq!(
            serde_json::to_vec(studio.score.as_ref().unwrap()).unwrap(),
            before,
            "Rendering must not mutate score choices"
        );
    }
}

#[test]
fn results_with_modality_diagnostics_remain_bounded_through_small_window_resizing() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let ctx = egui::Context::default();
    studio.score = Some(modality_surface(true));
    studio.score_document = studio.document.clone();
    for size in [
        [1500.0, 900.0],
        [760.0, 560.0],
        [1200.0, 720.0],
        [760.0, 560.0],
    ] {
        for _ in 0..2 {
            let (_, bounds) =
                diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Results, size, vec![]);
            assert!(
                bounds.right() <= size[0] + 1.0,
                "Results overflow width at {size:?}: {bounds:?}"
            );
            assert!(
                bounds.bottom() <= size[1] + 1.0,
                "Results overflow height at {size:?}: {bounds:?}"
            );
        }
    }
}

#[test]
fn data_atmosphere_panel_is_reachable_and_shows_model_and_coverage_limits() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let ctx = egui::Context::default();
    let output = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Data,
        [1200.0, 1000.0],
        "Atmospheric profile",
    );
    let text = diagnostic_text(&output)
        .into_iter()
        .map(|x| x.0)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("No atmospheric profile loaded"));
    assert!(text.contains("Import profile JSON"));
    assert!(!studio.busy());
    let mut profile = atmospheric_fixture();
    profile.valid_to_utc_s = profile.valid_from_utc_s + 30.0;
    let document = studio.document.as_mut().unwrap();
    document.recording_start_utc = "2026-09-08 00:00:00".into();
    document.atmosphere = Some(profile);
    let (output, _) = diagnostic_frame(
        &mut studio,
        &ctx,
        DiagnosticPane::Data,
        [1200.0, 1000.0],
        vec![],
    );
    let text = diagnostic_text(&output)
        .into_iter()
        .map(|x| x.0)
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Fabricated GUI atmospheric profile",
        "does not cover the entire recording",
        "no ray bending",
        "Weather-provider download",
    ] {
        assert!(text.contains(expected), "Missing {expected}: {text}");
    }
    for size in [[760.0, 560.0], [1200.0, 900.0], [760.0, 560.0]] {
        let (_, bounds) = diagnostic_frame(&mut studio, &ctx, DiagnosticPane::Data, size, vec![]);
        assert!(
            bounds.right() <= size[0] + 1.0,
            "Data width overflow at {size:?}: {bounds:?}"
        );
        assert!(
            bounds.bottom() <= size[1] + 1.0,
            "Data height overflow at {size:?}: {bounds:?}"
        );
    }
}

#[test]
fn removing_profile_through_data_panel_invalidates_scores_and_is_undoable() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let ctx = egui::Context::default();
    studio.document.as_mut().unwrap().atmosphere = Some(atmospheric_fixture());
    let before = studio.document.clone().unwrap();
    studio.score = Some(modality_surface(true));
    let _ = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Data,
        [1200.0, 1000.0],
        "Atmospheric profile",
    );
    let _ = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Data,
        [1200.0, 1000.0],
        "Remove profile · use fixed sound speed",
    );
    assert!(studio.document.as_ref().unwrap().atmosphere.is_none());
    assert!(studio.score.is_none());
    assert_eq!(studio.undo.len(), 1);
    studio.undo();
    assert_eq!(studio.document.as_ref(), Some(&before));
    studio.redo();
    assert!(studio.document.as_ref().unwrap().atmosphere.is_none());
}

#[test]
fn regional_history_completed_edit_restart_reopen_uses_original_snapshot_without_autoload() {
    use geosonnar::regional_history as history;
    let f = Fixture::new();
    let archive = saved_investigation_fixture(&f, false, false, false);
    let handle =
        history::begin(&f.case.manifest, &archive.document, &archive.inputs_sha256).unwrap();
    let id = handle.id().to_owned();
    let outcome = Ok(archive.report.clone());
    let summary = history::finish(handle, &outcome, false).unwrap();
    let mut studio = f.studio();
    studio.document = Some(archive.document.clone());
    studio.saved = studio.document.clone();
    studio.accept_run_completion(history_runs::RunCompletion {
        original_manifest: f.case.manifest.clone(),
        joint_request: None,
        document: archive.document.clone(),
        inputs_sha256: archive.inputs_sha256.clone(),
        outcome,
        cancelled: false,
        persistence: Ok(summary),
    });
    assert!(!studio.has_unsaved_results());
    studio.document.as_mut().unwrap().notes = "Later analyst edits".into();
    assert!(studio.dirty());
    let copy = f.root.join("history-copy-after-edit");
    history::export_copy(&f.case.manifest, &id, &copy).unwrap();
    assert_eq!(
        geosonnar::investigation_archive::load(&copy)
            .unwrap()
            .document,
        archive.document
    );
    drop(studio);
    let mut restarted = f.studio();
    restarted.open_run_history(&egui::Context::default(), id);
    finish_studio_task(&mut restarted);
    assert_eq!(restarted.document.as_ref(), Some(&archive.document));
    assert!(!restarted.dirty() && !restarted.has_unsaved_results());
    assert!(restarted.layers.is_empty() && restarted.lightning.is_none());
    assert!(restarted.window.is_none() && restarted.score.is_none());
    assert!(restarted.regional.report.is_some());
    assert!(restarted.investigation.archive.is_none());
    assert!(!restarted.report_inputs_ready());
    assert!(
        restarted
            .run_history
            .status
            .as_ref()
            .unwrap()
            .1
            .contains("replay requires original data")
    );
    restarted.check_history_inputs(&egui::Context::default());
    finish_studio_task(&mut restarted);
    assert!(!restarted.report_inputs_ready());
    assert!(restarted.notice.as_ref().unwrap().0);
    assert!(restarted.layers.is_empty() && restarted.lightning.is_none());
}

#[test]
fn regional_history_unsaved_result_is_visible_and_review_save_does_not_hide_it() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let document = studio.document.clone().unwrap();
    let outcome = Ok(geosonnar::investigation_archive::ReportKind::Regional(
        regional_report_fixture(&document, false),
    ));
    studio.accept_run_completion(history_runs::RunCompletion {
        original_manifest: f.case.manifest.clone(),
        joint_request: None,
        document: document.clone(),
        inputs_sha256: "a".repeat(64),
        outcome,
        cancelled: false,
        persistence: Err("Disk write denied (synthetic control)".into()),
    });
    assert!(studio.has_unsaved_results());
    assert!(
        !studio.dirty(),
        "Review edits and unsaved computation have separate ownership"
    );
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::Saved(
            f.root.join("review-only"),
            document.clone(),
        )),
    );
    tick(&mut studio);
    assert!(
        studio.has_unsaved_results(),
        "Ctrl+S only saved review, not numerical results"
    );
    let ctx = egui::Context::default();
    let (output, _) = diagnostic_frame(
        &mut studio,
        &ctx,
        DiagnosticPane::Results,
        [1000.0, 900.0],
        vec![],
    );
    let text = diagnostic_text(&output)
        .into_iter()
        .map(|x| x.0)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("NOT saved") && text.contains("Retry saving original result"),
        "{text}"
    );
    let _ = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Results,
        [1000.0, 900.0],
        "Discard unsaved result…",
    );
    assert!(
        studio.has_unsaved_results(),
        "First click must only ask for explicit discard"
    );
    let _ = click_diagnostic_label(
        &mut studio,
        &ctx,
        DiagnosticPane::Results,
        [1000.0, 900.0],
        "Discard result",
    );
    assert!(!studio.has_unsaved_results());
    assert_eq!(studio.document.as_ref(), Some(&document));
}

#[test]
fn regional_history_late_success_preserves_new_edits_and_keeps_original_saved_result() {
    use geosonnar::regional_history as history;
    let f = Fixture::new();
    let archive = saved_investigation_fixture(&f, true, true, false);
    let handle =
        history::begin(&f.case.manifest, &archive.document, &archive.inputs_sha256).unwrap();
    let id = handle.id().to_owned();
    let outcome = Ok(archive.report.clone());
    let summary = history::finish(handle, &outcome, true).unwrap();
    let mut studio = f.studio();
    studio.document = Some(archive.document.clone());
    studio.document.as_mut().unwrap().notes = "New review must survive late reply".into();
    let changed = studio.document.clone();
    studio.accept_run_completion(history_runs::RunCompletion {
        original_manifest: f.case.manifest.clone(),
        joint_request: None,
        document: archive.document.clone(),
        inputs_sha256: archive.inputs_sha256,
        outcome,
        cancelled: true,
        persistence: Ok(summary),
    });
    assert_eq!(studio.document, changed);
    assert!(studio.regional.report.is_none() && studio.regional.comparison.is_none());
    assert!(!studio.has_unsaved_results());
    let loaded = history::load(&f.case.manifest, &id).unwrap();
    assert_eq!(loaded.document, archive.document);
    assert_eq!(loaded.summary.status, history::RunStatus::Cancelled);
    assert!(loaded.report.is_some());
}

#[test]
fn regional_history_failed_run_can_reopen_without_scoring_or_event_approval() {
    use geosonnar::regional_history as history;
    let f = Fixture::new();
    let mut studio = f.studio();
    let document = studio.document.clone().unwrap();
    let handle = history::begin(&f.case.manifest, &document, &"a".repeat(64)).unwrap();
    let id = handle.id().to_owned();
    history::finish(handle, &Err("Original source disappeared".into()), false).unwrap();
    studio.open_run_history(&egui::Context::default(), id);
    finish_studio_task(&mut studio);
    assert_eq!(studio.document.as_ref(), Some(&document));
    assert!(studio.regional.report.is_none() && studio.regional.comparison.is_none());
    assert!(studio.layers.is_empty() && studio.lightning.is_none() && studio.score.is_none());
    assert!(!studio.has_unsaved_results());
    let ctx = egui::Context::default();
    let (output, _) = diagnostic_frame(
        &mut studio,
        &ctx,
        DiagnosticPane::Results,
        [1000.0, 700.0],
        vec![],
    );
    let text = diagnostic_text(&output)
        .into_iter()
        .map(|x| x.0)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Original source disappeared"), "{text}");
}

#[test]
fn regional_history_unsaved_outcome_blocks_replacing_it_with_an_archive() {
    let f = Fixture::new();
    let mut studio = f.studio();
    let document = studio.document.clone().unwrap();
    studio.accept_run_completion(history_runs::RunCompletion {
        original_manifest: f.case.manifest.clone(),
        joint_request: None,
        document: document.clone(),
        inputs_sha256: "a".repeat(64),
        outcome: Err("Synthetic failed computation".into()),
        cancelled: false,
        persistence: Err("Storage unavailable".into()),
    });
    studio.open_investigation_path(&egui::Context::default(), f.root.join("not-opened"));
    assert!(!studio.busy() && studio.has_unsaved_results());
    assert_eq!(studio.document.as_ref(), Some(&document));
    assert!(
        studio
            .notice
            .as_ref()
            .unwrap()
            .1
            .contains("Save or discard")
    );
}

#[test]
fn regional_worker_journals_before_source_verification_and_retains_failed_write_warning() {
    use geosonnar::{investigation_archive as archive, regional_history as history};
    for storage_available in [true, false] {
        let f = Fixture::new();
        let saved = saved_investigation_fixture(&f, false, false, true);
        let replay = archive::load_replay_inputs(&saved).unwrap();
        let mut studio = f.studio();
        studio.document = Some(saved.document.clone());
        studio.saved = studio.document.clone();
        studio.layers = replay.layers;
        studio.lightning = replay.lightning;
        let ctx = egui::Context::default();
        studio.prepare_regional(&ctx, false);
        finish_studio_task(&mut studio);
        assert!(studio.regional.estimate.is_some(), "{:?}", studio.notice);
        if !storage_available {
            fs::write(
                f.root.join("regional-runs"),
                b"Synthetic storage obstruction",
            )
            .unwrap();
        }
        // Fail between the explicit estimate and run. The immutable start must
        // exist before the worker discovers this missing required source.
        fs::remove_file(&saved.document.datasets[0].path).unwrap();
        studio.prepare_regional(&ctx, true);
        finish_studio_task(&mut studio);
        assert!(!studio.regional.running);
        assert!(studio.notice.as_ref().unwrap().0);
        assert_eq!(studio.document.as_ref(), Some(&saved.document));
        assert_eq!(studio.has_unsaved_results(), !storage_available);
        if storage_available {
            let rows = history::list(&f.case.manifest).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].status, history::RunStatus::Failed);
            assert_eq!(
                history::load(&f.case.manifest, &rows[0].id)
                    .unwrap()
                    .document,
                saved.document
            );
        } else {
            assert!(
                studio
                    .run_history
                    .status
                    .as_ref()
                    .unwrap()
                    .1
                    .contains("NOT saved")
            );
            studio.prepare_regional(&ctx, true);
            assert!(
                !studio.busy(),
                "An unsaved original run must not be replaced by another run"
            );
            assert!(
                studio
                    .notice
                    .as_ref()
                    .unwrap()
                    .1
                    .contains("previous result")
            );
        }
    }
}

#[test]
fn exporting_an_older_result_of_the_same_review_cannot_clear_a_different_unsaved_run() {
    use geosonnar::investigation_archive::ReportKind;
    let f = Fixture::new();
    let mut studio = f.studio();
    let document = studio.document.clone().unwrap();
    studio
        .regional
        .accept_saved_report(regional_report_fixture(&document, false), document.clone());
    studio.document.as_mut().unwrap().notes = "Edited while the next run was computing".into();
    studio.accept_run_completion(history_runs::RunCompletion {
        original_manifest: f.case.manifest.clone(),
        joint_request: None,
        document: document.clone(),
        inputs_sha256: "a".repeat(64),
        outcome: Ok(ReportKind::Regional(regional_report_fixture(
            &document, true,
        ))),
        cancelled: true,
        persistence: Err("Synthetic storage failure for the newer cancelled result".into()),
    });
    assert_eq!(
        studio.regional.report.as_ref().unwrap().status,
        geosonnar::regional_search::SearchStatus::Completed
    );
    studio.document = Some(document.clone()); // Undo back to the same reviewed inputs.
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::RegionalSaved(
            f.root.join("older-complete-export"),
            Box::new(document),
        )),
    );
    tick(&mut studio);
    assert!(
        studio.has_unsaved_results(),
        "Saving old completed A must not erase unsaved partial B with the same document"
    );
    let ReportKind::Regional(original) = studio
        .run_history
        .unsaved
        .as_ref()
        .unwrap()
        .outcome
        .as_ref()
        .unwrap()
    else {
        panic!("expected regional result");
    };
    assert_eq!(
        original.status,
        geosonnar::regional_search::SearchStatus::Cancelled
    );
}

#[test]
fn deleting_open_history_explicitly_removes_saved_claim_and_input_verification() {
    use geosonnar::regional_history as history;
    let f = Fixture::new();
    let archive = saved_investigation_fixture(&f, false, false, false);
    let handle =
        history::begin(&f.case.manifest, &archive.document, &archive.inputs_sha256).unwrap();
    let id = handle.id().to_owned();
    history::finish(handle, &Ok(archive.report), false).unwrap();
    let loaded = history::load(&f.case.manifest, &id).unwrap();
    let mut studio = f.studio();
    studio.accept_history_run(loaded, studio.document.clone().unwrap());
    studio.run_history.verified_inputs = Some(inspection::InputIdentity::capture(&studio));
    assert!(studio.report_inputs_ready());
    history::delete(&f.case.manifest, &id).unwrap();
    send(
        &studio,
        studio.generation,
        WorkerLane::Task,
        Ok(ResultValue::RunHistoryDeleted(id)),
    );
    tick(&mut studio);
    assert!(
        studio.regional.report.is_some(),
        "An intentional deletion may leave a clearly labelled in-memory view"
    );
    assert!(!studio.report_inputs_ready() && studio.run_history.verified_inputs.is_none());
    assert!(studio.run_history.deleted);
    let (output, _) = diagnostic_frame(
        &mut studio,
        &egui::Context::default(),
        DiagnosticPane::Results,
        [1000.0, 900.0],
        vec![],
    );
    let text = diagnostic_text(&output)
        .into_iter()
        .map(|x| x.0)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("memory only")
            && !text.contains("Saved result; replay requires original data"),
        "{text}"
    );
}

#[test]
fn late_history_completion_never_rebinds_the_older_displayed_result() {
    use geosonnar::regional_history as history;
    let f = Fixture::new();
    let archive = saved_investigation_fixture(&f, false, false, false);
    let first =
        history::begin(&f.case.manifest, &archive.document, &archive.inputs_sha256).unwrap();
    let first_id = first.id().to_owned();
    history::finish(first, &Ok(archive.report.clone()), false).unwrap();
    let mut studio = f.studio();
    studio.accept_history_run(
        history::load(&f.case.manifest, &first_id).unwrap(),
        studio.document.clone().unwrap(),
    );
    studio.run_history.verified_inputs = Some(inspection::InputIdentity::capture(&studio));
    let verified = studio.run_history.verified_inputs.clone();
    studio.document.as_mut().unwrap().notes = "Edits made during a later run".into();
    let second =
        history::begin(&f.case.manifest, &archive.document, &archive.inputs_sha256).unwrap();
    let second_id = second.id().to_owned();
    let outcome = Ok(archive.report.clone());
    let summary = history::finish(second, &outcome, false).unwrap();
    studio.accept_run_completion(history_runs::RunCompletion {
        original_manifest: f.case.manifest.clone(),
        joint_request: None,
        document: archive.document.clone(),
        inputs_sha256: archive.inputs_sha256.clone(),
        outcome,
        cancelled: false,
        persistence: Ok(summary),
    });
    assert_eq!(
        studio.run_history.opened.as_ref().unwrap().summary.id,
        first_id
    );
    assert_eq!(studio.run_history.verified_inputs, verified);
    studio.accept_history_deletion(&second_id);
    assert!(
        !studio.run_history.deleted,
        "Deleting a later undisplayed run must not relabel the older view"
    );
    studio.accept_history_deletion(&first_id);
    assert!(studio.run_history.deleted);
    studio.accept_investigation(archive, studio.document.clone().unwrap());
    assert!(!studio.run_history.deleted && studio.run_history.opened.is_none());
    studio.accept_history_deletion(&first_id);
    assert!(
        !studio.run_history.deleted,
        "Opening an independent exported archive releases the prior history binding"
    );
}
