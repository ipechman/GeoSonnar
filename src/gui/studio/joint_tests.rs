//! Headless controls only. No media, file picker, native worker or network opens.
//! One source-restoration control uses tiny owned synthetic lightning files.
use super::*;
use geosonnar::{
    casework::{CaseMedia, CaseRecording, SavedCase},
    thunder_envelope::ObservedEnvelope,
};

fn studio() -> Studio {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("output/never-opened-joint-ui-control");
    let case = CaseOverview {
        manifest: root.join("case.json"),
        recording_path: root.join("synthetic.raw"),
        recording_verified: Some(true),
        recording_problem: None,
        hints: vec![],
        case: SavedCase {
            schema_version: 1,
            case_id: "joint-ui-control".into(),
            title: "Fabricated headless UI control".into(),
            created_unix_s: 1,
            local_only: true,
            recording: CaseRecording {
                path: "synthetic.raw".into(),
                sha256: "a".repeat(64),
                bytes: 5,
                media: CaseMedia {
                    duration_s: 60.0,
                    sample_rate_hz: 4000,
                    channels: 2,
                    audio_codec: "fabricated".into(),
                },
            },
        },
    };
    let mut studio = Studio::default();
    studio.bind(&case);
    studio.data.refresh_requested = false;
    studio.next_progress = Instant::now() + Duration::from_secs(3600);
    studio.next_frame = Instant::now() + Duration::from_secs(3600);
    studio
}
fn selection() -> EnvelopeSelection {
    EnvelopeSelection {
        start_s: 0.0,
        end_s: 3.0,
        fft_size: 4096,
        hop_s: 0.5,
        band_hz: [20.0, 250.0],
        reviewed_selection_note:
            "Fabricated selection for message controls, not a real measurement".into(),
    }
}
fn observation() -> ObservedEnvelope {
    ObservedEnvelope {
        recording_sha256: "a".repeat(64),
        channel: 0,
        reviewed_selection_note: selection().reviewed_selection_note,
        envelope: thunder::ThunderEnvelope {
            samples: (0..4)
                .map(|i| thunder::EnvelopeSample {
                    time_s: 0.512 + i as f64 * 0.5,
                    rms: 1.0 + i as f64 * 0.1,
                })
                .collect(),
            band_low_hz: 20.5078125,
            band_high_hz: 250.0,
            method: "Fabricated PSD envelope for UI controls".into(),
        },
    }
}
fn selected_studio() -> Studio {
    let mut studio = studio();
    studio.document.as_mut().unwrap().envelope_review = Some(EnvelopeReview {
        selection: selection(),
        observation: None,
    });
    studio
}
#[derive(Clone, Copy)]
enum Pane {
    Audio,
    Method,
    Progress,
    Results,
}
fn frame(
    studio: &mut Studio,
    ctx: &egui::Context,
    pane: Pane,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 1000.0),
            )),
            events,
            ..Default::default()
        },
        |ui| match pane {
            Pane::Audio => studio.joint_audio_ui(ui),
            Pane::Method => studio.joint_method_ui(ui),
            Pane::Progress => studio.joint_progress_ui(ui),
            Pane::Results => {
                studio.joint_results_ui(ui);
            }
        },
    )
}
fn labels(output: &egui::FullOutput) -> Vec<(String, egui::Pos2)> {
    fn visit(shape: &egui::epaint::Shape, rows: &mut Vec<(String, egui::Pos2)>) {
        match shape {
            egui::epaint::Shape::Text(text) => rows.push((
                text.galley.text().to_string(),
                text.pos + text.galley.size() / 2.0,
            )),
            egui::epaint::Shape::Vec(values) => {
                for shape in values {
                    visit(shape, rows);
                }
            }
            _ => {}
        }
    }
    let mut rows = Vec::new();
    for item in &output.shapes {
        visit(&item.shape, &mut rows);
    }
    rows
}
fn click(studio: &mut Studio, ctx: &egui::Context, pane: Pane, label: &str) {
    let _ = frame(studio, ctx, pane, vec![]);
    let output = frame(studio, ctx, pane, vec![]);
    let point = labels(&output)
        .into_iter()
        .find(|(text, _)| text == label)
        .unwrap_or_else(|| panic!("Missing control {label}"))
        .1;
    for pressed in [true, false] {
        let _ = frame(
            studio,
            ctx,
            pane,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    for _ in 0..20 {
        let _ = frame(studio, ctx, pane, vec![]);
    }
}

#[test]
fn idle_joint_controls_never_measure_or_change_evidence() {
    let mut studio = studio();
    let before = studio.document.clone();
    let ctx = egui::Context::default();
    for _ in 0..4 {
        let _ = frame(&mut studio, &ctx, Pane::Audio, vec![]);
        let _ = frame(&mut studio, &ctx, Pane::Method, vec![]);
    }
    assert_eq!(studio.document, before);
    assert!(!studio.busy());
    assert_eq!(studio.joint.run_id, 0);
    assert!(!studio.joint.enabled && !studio.joint.running);
    assert!(studio.joint.snapshot.is_none());
}
#[test]
fn optional_method_and_presentation_toggle_preserve_all_selections() {
    let mut studio = selected_studio();
    let before = studio.document.clone();
    let ctx = egui::Context::default();
    click(
        &mut studio,
        &ctx,
        Pane::Method,
        "Doppler + envelope (experimental)",
    );
    assert!(studio.joint.enabled);
    studio.detailed_controls = true;
    let _ = frame(&mut studio, &ctx, Pane::Audio, vec![]);
    click(&mut studio, &ctx, Pane::Method, "Reviewed events");
    assert!(!studio.joint.enabled);
    assert_eq!(studio.document, before);
    assert!(!studio.busy());
}
#[test]
fn explicit_interval_stays_fixed_when_spectrogram_viewport_changes() {
    let mut studio = studio();
    let ctx = egui::Context::default();
    studio.view.time_min_s = 10.0;
    studio.view.time_max_s = 20.0;
    click(
        &mut studio,
        &ctx,
        Pane::Audio,
        "Thunder envelope — optional comparison interval",
    );
    click(
        &mut studio,
        &ctx,
        Pane::Audio,
        "Use displayed time interval",
    );
    let review = studio
        .document
        .as_ref()
        .unwrap()
        .envelope_review
        .clone()
        .unwrap();
    assert_eq!(
        (review.selection.start_s, review.selection.end_s),
        (10.0, 20.0)
    );
    assert!(review.observation.is_none());
    studio.view.time_min_s = 13.0;
    studio.view.time_max_s = 15.0;
    studio.view.frequency_max_hz = 500.0;
    let _ = frame(&mut studio, &ctx, Pane::Audio, vec![]);
    assert_eq!(
        studio.document.as_ref().unwrap().envelope_review,
        Some(review)
    );
    assert!(!studio.busy());
}
#[test]
fn current_measurement_is_undoable_and_does_not_select_scoring_terms() {
    let mut studio = selected_studio();
    let original = studio.document.clone().unwrap();
    studio.joint.run_id = 4;
    studio.accept_joint_measurement(MeasurementReply {
        run_id: 4,
        original: original.clone(),
        result: Ok(observation()),
    });
    assert!(
        studio
            .document
            .as_ref()
            .unwrap()
            .envelope_review
            .as_ref()
            .unwrap()
            .observation
            .is_some()
    );
    assert_eq!(studio.document.as_ref().unwrap().mix, original.mix);
    assert_eq!(studio.document.as_ref().unwrap().events, original.events);
    studio.undo();
    assert_eq!(studio.document, Some(original));
}
#[test]
fn changed_review_keeps_edits_and_finishes_its_own_busy_lane() {
    let mut studio = selected_studio();
    let original = studio.document.clone().unwrap();
    studio.joint.run_id = 8;
    studio.busy = Some("measurement".into());
    studio.document.as_mut().unwrap().notes = "New analyst note".into();
    let edited = studio.document.clone();
    studio
        .sender
        .send(Reply {
            generation: studio.generation,
            lane: WorkerLane::JointTask(8),
            result: Ok(ResultValue::JointMeasured(Box::new(MeasurementReply {
                run_id: 8,
                original,
                result: Ok(observation()),
            }))),
        })
        .unwrap();
    studio.tick(&egui::Context::default());
    assert_eq!(studio.document, edited);
    assert!(!studio.busy());
    assert!(studio.notice.as_ref().unwrap().1.contains("changed"));
}
#[test]
fn obsolete_task_cannot_clear_a_newer_jobs_busy_flag() {
    let mut studio = selected_studio();
    let original = studio.document.clone().unwrap();
    studio.joint.run_id = 9;
    studio.busy = Some("newer job".into());
    studio
        .sender
        .send(Reply {
            generation: studio.generation,
            lane: WorkerLane::JointTask(8),
            result: Ok(ResultValue::JointMeasured(Box::new(MeasurementReply {
                run_id: 8,
                original,
                result: Ok(observation()),
            }))),
        })
        .unwrap();
    studio.tick(&egui::Context::default());
    assert_eq!(studio.busy.as_deref(), Some("newer job"));
    assert!(
        studio
            .document
            .as_ref()
            .unwrap()
            .envelope_review
            .as_ref()
            .unwrap()
            .observation
            .is_none()
    );
}
#[test]
fn wrong_recording_or_channel_measurement_is_rejected() {
    for channel in [0, 1] {
        let mut studio = selected_studio();
        let original = studio.document.clone().unwrap();
        let mut measured = observation();
        if channel == 0 {
            measured.recording_sha256 = "b".repeat(64);
        } else {
            measured.channel = 1;
        }
        studio.accept_joint_measurement(MeasurementReply {
            run_id: 0,
            original: original.clone(),
            result: Ok(measured),
        });
        assert_eq!(studio.document, Some(original));
        assert!(studio.undo.is_empty());
        assert!(studio.notice.as_ref().unwrap().0);
    }
}
#[test]
fn cancellation_after_worker_completion_still_preserves_review() {
    let mut studio = selected_studio();
    let original = studio.document.clone().unwrap();
    studio.joint.cancel.store(true, Ordering::Relaxed);
    studio.accept_joint_measurement(MeasurementReply {
        run_id: 0,
        original: original.clone(),
        result: Ok(observation()),
    });
    assert_eq!(studio.document, Some(original));
    assert!(studio.undo.is_empty());
}
#[test]
fn cancel_button_changes_only_the_owned_cancellation_token() {
    let mut studio = selected_studio();
    studio.joint.running = true;
    let original = studio.document.clone();
    let ctx = egui::Context::default();
    click(
        &mut studio,
        &ctx,
        Pane::Progress,
        "Cancel shared-cell comparison",
    );
    assert!(studio.joint.cancel.load(Ordering::Relaxed));
    assert_eq!(studio.document, original);
    studio.accept_joint_stopped(0, "Stopped before computation".into());
    assert!(!studio.joint.running);
}
#[test]
fn picked_timing_is_blocked_without_automatically_changing_mix() {
    let mut studio = selected_studio();
    let doc = studio.document.as_mut().unwrap();
    doc.mix.doppler_frequency = true;
    doc.mix.thunder_timing = true;
    let before = doc.clone();
    assert!(joint_block_reason(doc).unwrap().contains("disable"));
    assert_eq!(*doc, before);
}
#[test]
fn utc_query_support_roundtrips_without_local_time_conversion() {
    let epoch = workspace::parse_utc("2025-07-19 16:00:00Z").unwrap();
    assert_eq!(workspace::parse_utc(&utc_input(epoch)).unwrap(), epoch);
}

fn display_snapshot() -> JointSnapshot {
    // Deliberately fabricated display-only records, never passed to native scoring.
    let mut document = selected_studio().document.unwrap();
    document.envelope_review.as_mut().unwrap().observation = Some(observation());
    let progress = JointProgress {
        stage: JointStage::Finished,
        completed_doppler_runs: 3,
        total_doppler_runs: 3,
        completed_envelope_runs: 1,
        total_envelope_runs: 1,
        completed_doppler_cells: 3,
        completed_envelope_cells: 2,
        total_cells: 3,
    };
    let estimate = JointEstimate {
        request_sha256: "b".repeat(64),
        request_bytes: 1,
        receivers: 1,
        clocks: 3,
        cells: 3,
        doppler_runs: 3,
        envelope_runs: 1,
        doppler_evaluations: 3,
        doppler_residual_evaluations: 12,
        doppler_trajectory_point_visits: 12,
        envelope_work_units: 10,
        peak_engine_memory_bytes: 100,
        working_memory_bound_bytes: 200,
        serialized_report_bound_bytes: 400,
        serialized_snapshot_bound_bytes: 800,
        ranking_work_bound: 20,
    };
    let receivers = vec![EnvelopeReceiver {
        id: "control-receiver".into(),
        location: evidence_score::ObserverLocation {
            latitude_deg: 45.0,
            longitude_deg: 0.0,
            ellipsoid_height_m: 0.0,
        },
    }];
    let cells = vec![
        joint_evidence::JointCell {
            receiver_index: 0,
            clock_index: 0,
            clock_offset_s: -1.0,
            clock_at_boundary: true,
            spatial_at_boundary: true,
            doppler: DopplerState::Available {
                score: 2.0,
                assignments: vec![],
                frequency_at_boundary: false,
            },
            envelope: EnvelopeState::Available {
                cosine_similarity: 0.7,
                contributing_source_rows: 1,
                distance_excluded_source_rows: 0,
            },
            ranks: None,
        },
        joint_evidence::JointCell {
            receiver_index: 0,
            clock_index: 1,
            clock_offset_s: 0.0,
            clock_at_boundary: false,
            spatial_at_boundary: true,
            doppler: DopplerState::Available {
                score: 1.0,
                assignments: vec![],
                frequency_at_boundary: false,
            },
            envelope: EnvelopeState::Unavailable {
                reason: "No predicted energy".into(),
            },
            ranks: None,
        },
        joint_evidence::JointCell {
            receiver_index: 0,
            clock_index: 2,
            clock_offset_s: 1.0,
            clock_at_boundary: true,
            spatial_at_boundary: true,
            doppler: DopplerState::NotStarted,
            envelope: EnvelopeState::NotStarted,
            ranks: None,
        },
    ];
    let map = map_data::MapData {
        source_path: PathBuf::new(),
        source_sha256: "c".repeat(64),
        time_origin_utc_s: None,
        tracks: vec![],
        candidates: vec![],
        observers: vec![],
        grid: None,
        terrain: None,
        lightning: vec![],
        lightning_provenance: None,
        notes: vec![],
        location_verified: false,
    };
    let lightning = evidence_score::LightningCatalog {
        schema_version: 1,
        input_path: PathBuf::new(),
        input_bytes: 1,
        input_sha256: "d".repeat(64),
        provenance: evidence_score::LightningProvenance {
            provider: "Fabricated".into(),
            source: "Headless UI control".into(),
            license: None,
            notes: None,
            altitude_datum: evidence_score::LightningAltitudeDatum::Unknown,
            source_metadata: None,
        },
        strikes: vec![],
    };
    let request = JointRequest {
        schema_version: 1,
        document: document.clone(),
        map,
        lightning,
        observation: observation(),
        regional_plan: geosonnar::regional_search::RegionalPlan::default(),
        envelope_plan: EnvelopeSearchPlan {
            schema_version: 1,
            catalog_sha256: "d".repeat(64),
            catalog_utc_interval: [1_700_000_000.0, 1_700_000_300.0],
            receivers: receivers.clone(),
            nominal_recording_start_utc_s: 1_700_000_100.0,
            min_clock_offset_s: -1.0,
            max_clock_offset_s: 1.0,
            model: EnvelopeModel {
                sound_speed_mps: 343.0,
                fallback_strike_ellipsoid_height_m: None,
                minimum_distance_m: 100.0,
                maximum_distance_m: 50_000.0,
                kernel_decay_s: 5.0,
                kernel_duration_s: 24.0,
            },
            source_policy: None,
            keep_best: 3,
            limits: EnvelopeLimits::default(),
        },
        limits: joint_evidence::JointLimits::default(),
    };
    JointSnapshot {
        request,
        report: joint_evidence::JointReport {
            schema_version: 1,
            request_sha256: "b".repeat(64),
            document_sha256: "e".repeat(64),
            inputs_sha256: "f".repeat(64),
            receivers,
            clock_offsets_s: vec![-1.0, 0.0, 1.0],
            nominal_recording_start_utc_s: 1_700_000_100.0,
            estimate,
            progress,
            status: JointStatus::Cancelled,
            all_cells_available: false,
            ranking_available: false,
            cells,
            location_verified: false,
            independent_field_validation: false,
            limitations: vec!["Fabricated display control".into()],
        },
    }
}
#[test]
fn unavailable_and_unfinished_cells_remain_in_every_sort() {
    let snapshot = display_snapshot();
    assert_eq!(
        sorted_cells(
            &snapshot.report,
            JointView::Doppler,
            JointColumn::Doppler,
            false
        ),
        vec![1, 0, 2]
    );
    assert_eq!(
        sorted_cells(
            &snapshot.report,
            JointView::Envelope,
            JointColumn::Envelope,
            true
        ),
        vec![0, 1, 2]
    );
    assert_eq!(
        sorted_cells(
            &snapshot.report,
            JointView::Combined,
            JointColumn::Rank,
            false
        ),
        vec![0, 1, 2]
    );
    assert_eq!(
        state_reason(&snapshot.report.cells[1]),
        "Envelope: No predicted energy"
    );
    assert!(state_reason(&snapshot.report.cells[2]).contains("not completed"));
}
#[test]
fn result_frames_reuse_surface_and_sort_cache_and_do_not_edit_review() {
    let mut studio = studio();
    let snapshot = display_snapshot();
    let document = snapshot.request.document.clone();
    studio.accept_joint_report(snapshot, document);
    let current = studio.document.clone();
    let ctx = egui::Context::default();
    let _ = frame(&mut studio, &ctx, Pane::Results, vec![]);
    let pointer = Arc::as_ptr(studio.joint.snapshot.as_ref().unwrap());
    let rows = studio.joint.rows.as_ptr();
    for _ in 0..3 {
        let _ = frame(&mut studio, &ctx, Pane::Results, vec![]);
    }
    assert_eq!(
        pointer,
        Arc::as_ptr(studio.joint.snapshot.as_ref().unwrap())
    );
    assert_eq!(rows, studio.joint.rows.as_ptr());
    assert_eq!(studio.document, current);
    assert_eq!(studio.joint.rows.len(), 3);
    assert!(!studio.busy());
}
#[test]
fn interrupted_request_reopen_keeps_sources_unloaded_and_does_not_restart() {
    let mut studio = studio();
    let snapshot = display_snapshot();
    let original = studio.document.clone();
    studio.restore_joint_request(snapshot.request);
    assert!(studio.has_joint_results());
    assert!(studio.joint.snapshot.is_none());
    assert!(studio.layers.is_empty() && studio.lightning.is_none());
    assert!(!studio.joint.running && !studio.busy());
    assert_eq!(studio.document, original);
    assert!(studio.joint.prepared.is_none());
}
#[test]
fn source_replacement_invalidates_estimate_even_when_review_is_unchanged() {
    let mut studio = studio();
    let snapshot = display_snapshot();
    studio.document = Some(snapshot.request.document.clone());
    let options = options_for(&snapshot.request);
    studio.joint.options = options.clone();
    studio.accept_joint_estimate(EstimateReply {
        run_id: 0,
        request: Arc::new(snapshot.request),
        options,
        source_revision: studio.loaded_data_revision,
        result: Ok(snapshot.report.estimate),
    });
    assert!(studio.joint_prepared_current());
    studio.loaded_data_revision += 1;
    assert!(!studio.joint_prepared_current());
    studio.run_joint(&egui::Context::default());
    assert!(!studio.busy());
    assert!(!studio.joint.running);
}
#[test]
fn stale_estimate_does_not_replace_current_review_or_enable_run() {
    let mut studio = studio();
    let snapshot = display_snapshot();
    let current = studio.document.clone();
    let options = options_for(&snapshot.request);
    studio.joint.options = options.clone();
    studio.accept_joint_estimate(EstimateReply {
        run_id: 0,
        request: Arc::new(snapshot.request),
        options,
        source_revision: studio.loaded_data_revision,
        result: Ok(snapshot.report.estimate),
    });
    assert!(studio.joint.prepared.is_none());
    assert_eq!(studio.document, current);
    assert!(!studio.busy());
}

#[test]
fn historical_catalog_metadata_is_available_only_for_the_exact_saved_review() {
    let mut studio = studio();
    let snapshot = display_snapshot();
    let original = snapshot.request.document.clone();
    studio.accept_joint_report(snapshot.clone(), original.clone());
    assert!(studio.joint_catalog_for_restore(&original).is_some());
    let mut changed = original.clone();
    changed.notes.push_str(" changed");
    assert!(studio.joint_catalog_for_restore(&changed).is_none());
    studio.restore_joint_request(snapshot.request);
    assert!(studio.joint_catalog_for_restore(&original).is_some());
    assert!(studio.layers.is_empty() && studio.lightning.is_none() && !studio.busy());
}

#[test]
fn explicit_joint_restore_preserves_legacy_provenance_but_rejects_different_data() {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
    fs::create_dir_all(&parent).unwrap();
    let root = parent.join(format!(
        "joint-catalog-restore-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    assert_eq!(
        root.parent(),
        Some(parent.canonicalize().unwrap().as_path())
    );
    let provenance = evidence_score::LightningProvenance {
        provider: "User-supplied lightning export".into(),
        source: "Original analyst attribution".into(),
        license: None,
        notes: Some("Original reviewed metadata".into()),
        altitude_datum: evidence_score::LightningAltitudeDatum::Wgs84Ellipsoid,
        source_metadata: None,
    };
    for (name, body) in [
        (
            "legacy.csv",
            "id,time_utc_s,latitude_deg,longitude_deg,altitude_m\none,1700000000,45,-73,0\n",
        ),
        (
            "legacy.json",
            r#"{"schema_version":1,"strikes":[{"id":"one","time_utc_s":1700000000,"latitude_deg":45,"longitude_deg":-73,"altitude_m":0}]}"#,
        ),
    ] {
        let path = root.join(name);
        fs::write(&path, body).unwrap();
        let expected = evidence_score::import_lightning(&path, &provenance).unwrap();
        let ordinary = super::super::map::restore_lightning_catalog(&path, true, None).unwrap();
        assert_eq!(ordinary.input_sha256, expected.input_sha256);
        assert_ne!(
            serde_json::to_value(&ordinary).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        let restored =
            super::super::map::restore_lightning_catalog(&path, true, Some(&expected)).unwrap();
        assert_eq!(
            serde_json::to_value(restored).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        assert!(
            super::super::map::restore_lightning_catalog(&path, false, Some(&expected)).is_err()
        );
        let mut changed = expected.clone();
        changed.strikes[0].latitude_deg += 0.01;
        assert!(super::super::map::restore_lightning_catalog(&path, true, Some(&changed)).is_err());
        fs::write(&path, format!("{body}\n")).unwrap();
        assert!(
            super::super::map::restore_lightning_catalog(&path, true, Some(&expected)).is_err()
        );
        fs::remove_file(&path).unwrap();
    }
    fs::remove_dir(&root).unwrap();
}
