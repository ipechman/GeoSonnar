//! Explicit disk-backed two-pass search; map previews never supply physics.
use super::*;
use geosonnar::{
    receiver_grid::GridLayout,
    track_store::{DEFAULT_INDEX_MEMORY_BYTES, TrackStore, TrackStoreBinding},
    wide_area_pair::{
        self, WideBudgets, WideCandidate, WideEstimate, WidePairReport, WidePairRequest,
        WideProgress,
    },
};

#[derive(Clone, Debug, PartialEq)]
struct Settings {
    event_ids: [String; 2],
    cpa_s: [f64; 2],
    tones: [[f64; 3]; 2],
    speed: [[f64; 2]; 2],
    distance: [[f64; 2]; 2],
    separation_tolerance_s: f64,
    shift: [f64; 3],
    utc_start: String,
    utc_end: String,
    max_work: u64,
    max_output: u64,
    max_seconds: u64,
    max_index_memory_bytes: u64,
    retain: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use geosonnar::casework::{CaseMedia, CaseRecording, SavedCase};

    fn studio() -> Studio {
        let directory =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("output/never-opened-wide-ui-control");
        let case = CaseOverview {
            manifest: directory.join("case.json"),
            recording_path: directory.join("synthetic.raw"),
            recording_verified: Some(true),
            recording_problem: None,
            hints: vec![],
            case: SavedCase {
                schema_version: 1,
                case_id: "wide-ui-control".into(),
                title: "Fabricated UI control".into(),
                created_unix_s: 1,
                local_only: true,
                recording: CaseRecording {
                    path: "synthetic.raw".into(),
                    sha256: "a".repeat(64),
                    bytes: 5,
                    media: CaseMedia {
                        duration_s: 120.,
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
    fn source() -> IndexedSource {
        let origin = geosonnar::detailed_data::utc_midnight("2026-09-08").unwrap();
        IndexedSource {
            binding: TrackStoreBinding {
                manifest_path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("output/never-opened-wide-ui-control/native/index.json"),
                manifest_sha256: "b".repeat(64),
            },
            label: "Synthetic index metadata only".into(),
            passes: 50_000,
            points: 3_000_000,
            origin,
            utc_window: [origin, origin + 86400.],
            max_gap_s: 45.,
        }
    }
    fn reviewed(studio: &mut Studio) -> Settings {
        let source = source();
        let doc = studio.document.as_mut().unwrap();
        doc.map.bounds = [24., -125., 49., -66.];
        doc.map.grid_spacing_m = 10_000.;
        doc.datasets.push(SourceBinding {
            path: source.binding.manifest_path.clone(),
            sha256: source.binding.manifest_sha256.clone(),
        });
        for (id, start) in [("one", 10.), ("two", 80.)] {
            let mut event =
                EvidenceEvent::manual(id.into(), EventKind::Doppler, start, start + 10.);
            event.review = ReviewState::Confirmed;
            event.selected_for_scoring = true;
            event.acoustic_source = workspace::AcousticSourceAssessment::Compatible;
            event.points = (0..3)
                .map(|i| workspace::TracePoint {
                    time_s: start + i as f64,
                    frequency_hz: 120. - i as f64,
                })
                .collect();
            doc.events.push(event);
        }
        Settings {
            event_ids: ["one".into(), "two".into()],
            cpa_s: [15., 85.],
            utc_start: utc_input(source.origin),
            utc_end: utc_input(source.origin + 86400.),
            ..Settings::default()
        }
    }
    fn estimate_fixture(request: &WidePairRequest, source: &IndexedSource) -> WideEstimate {
        WideEstimate {
            receiver_count: request.grid.count,
            source_passes: source.passes as u64,
            source_points: source.points,
            unpruned_pass_checks: 1,
            worst_scan_point_visits: 1,
            minimum_output_bytes: 1,
            source_time_origin_utc_s: source.origin,
            minimum_work_units_per_timing_pair: 1,
            maximum_timing_pairs_before_scan: 1,
            source_metadata_bytes: 1,
            spatial_index_memory_bytes: 1,
            local_cpa_memory_reserve_bytes: 1,
            required_index_memory_bytes: 3,
            cost_preview: None,
            limitations: vec![],
        }
    }
    #[test]
    fn wide_defaults_are_idle_and_do_not_measure_or_select_sources() {
        let mut studio = studio();
        let before = studio.document.clone();
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| studio.wide_controls_ui(ui));
        assert_eq!(studio.document, before);
        assert!(studio.wide.source.is_none() && studio.wide.prepared.is_none());
        assert!(!studio.wide.running && !studio.busy() && studio.receiver.try_recv().is_err());
    }
    #[test]
    fn wide_national_plan_preserves_all_receivers_exact_traces_and_source_gap() {
        let mut studio = studio();
        let settings = reviewed(&mut studio);
        let before = studio.document.clone().unwrap();
        let plan = request(&before, &source(), &settings).unwrap();
        assert!(plan.grid.count > 10_000);
        assert_eq!(
            plan.grid
                .observer(plan.grid.count - 1)
                .unwrap()
                .longitude_deg,
            -66.
        );
        assert_eq!(plan.events[0].points.len(), before.events[0].points.len());
        for (actual, original) in plan.events[0].points.iter().zip(&before.events[0].points) {
            assert_eq!(actual.t_rec_s, original.time_s);
            assert_eq!(actual.frequency_hz, original.frequency_hz);
        }
        assert_eq!(plan.pair_config.model.max_gap_s, 45.);
        assert_eq!(plan.pair_config.clock_offset_bounds_s, Some([0., 86400.]));
        assert_eq!(studio.document.as_ref(), Some(&before));
        assert!(studio.undo.is_empty());
    }
    #[test]
    fn independent_event_bounds_bind_request_and_validate_each_event() {
        let mut studio = studio();
        let mut settings = reviewed(&mut studio);
        let document = studio.document.clone().unwrap();
        settings.speed = [[60., 110.], [140., 250.]];
        settings.distance = [[1000., 6000.], [8000., 30000.]];
        let expected = request(&document, &source(), &settings).unwrap();
        for i in 0..2 {
            assert_eq!(expected.coarse_bounds[i].speed_mps, settings.speed[i]);
            assert_eq!(
                expected.coarse_bounds[i].slant_distance_m,
                settings.distance[i]
            );
            for speed in [true, false] {
                let mut changed = settings.clone();
                let bounds = if speed {
                    &mut changed.speed[i]
                } else {
                    &mut changed.distance[i]
                };
                bounds[0] += 1.;
                let actual = request(&document, &source(), &changed).unwrap();
                assert_ne!(
                    wide_area_pair::request_sha256(&actual).unwrap(),
                    wide_area_pair::request_sha256(&expected).unwrap()
                );
                assert_eq!(
                    actual.coarse_bounds[1 - i].speed_mps,
                    expected.coarse_bounds[1 - i].speed_mps
                );
                assert_eq!(
                    actual.coarse_bounds[1 - i].slant_distance_m,
                    expected.coarse_bounds[1 - i].slant_distance_m
                );
                for invalid in [[-1., 10.], [20., 10.], [f64::NAN, 10.], [1., f64::INFINITY]] {
                    let mut invalid_settings = settings.clone();
                    if speed {
                        invalid_settings.speed[i] = invalid;
                    } else {
                        invalid_settings.distance[i] = invalid;
                    }
                    assert!(
                        request(&document, &source(), &invalid_settings)
                            .unwrap_err()
                            .contains(&format!("Event {}", i + 1))
                    );
                }
            }
        }
        assert_eq!(studio.document.as_ref(), Some(&document));
        assert!(studio.undo.is_empty());
    }
    #[test]
    fn copying_reviewed_pair_preserves_independent_physical_bounds() {
        let mut studio = studio();
        let mut settings = reviewed(&mut studio);
        settings.speed = [[55., 100.], [160., 290.]];
        settings.distance = [[2000., 7000.], [15000., 40000.]];
        let before = studio.document.clone().unwrap();
        let original = settings.clone();
        copy_reviewed_pair(&before, &mut settings).unwrap();
        assert_eq!(settings.speed, original.speed);
        assert_eq!(settings.distance, original.distance);
        assert_eq!(
            settings.cpa_s,
            [before.events[0].arrival_s, before.events[1].arrival_s]
        );
        assert_eq!(settings.tones[0], [118. * 0.9, 120. * 1.1, 1.]);
        let mut replacement = before.clone();
        replacement.events[0].id = "replacement-reviewed-event".into();
        copy_reviewed_pair(&replacement, &mut settings).unwrap();
        assert_eq!(settings.event_ids[0], "replacement-reviewed-event");
        assert_eq!(settings.speed, original.speed);
        assert_eq!(settings.distance, original.distance);
        replacement.events[1].review = ReviewState::Pending;
        let unchanged = settings.clone();
        assert!(copy_reviewed_pair(&replacement, &mut settings).is_err());
        assert_eq!(settings, unchanged);
        assert_eq!(studio.document.as_ref(), Some(&before));
        assert!(studio.undo.is_empty());
    }
    #[test]
    fn either_event_physical_bound_change_refuses_an_older_estimate() {
        for i in 0..2 {
            for speed in [true, false] {
                let mut studio = studio();
                let settings = reviewed(&mut studio);
                let document = studio.document.clone().unwrap();
                let source = source();
                let request = request(&document, &source, &settings).unwrap();
                let estimate = estimate_fixture(&request, &source);
                studio.wide.source = Some(source);
                studio.wide.settings = settings.clone();
                if speed {
                    studio.wide.settings.speed[i][0] += 1.;
                } else {
                    studio.wide.settings.distance[i][1] += 100.;
                }
                studio.accept_wide_reply(WideReply::Estimated(Box::new(Prepared {
                    request,
                    estimate,
                    document: document.clone(),
                    settings,
                })));
                assert!(studio.wide.prepared.is_none());
                assert!(
                    studio
                        .notice
                        .as_ref()
                        .unwrap()
                        .1
                        .contains("options changed")
                );
                assert_eq!(studio.document.as_ref(), Some(&document));
                assert!(studio.undo.is_empty());
            }
        }
    }
    struct SavedRunFixture(PathBuf);
    impl SavedRunFixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("output/wide-ui-saved-run-controls")
                .join(format!(
                    "{}-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for SavedRunFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn legacy_shared_and_new_independent_bounds_reopen_without_rewriting_request() {
        let fixture = SavedRunFixture::new();
        for legacy in [true, false] {
            let mut studio = studio();
            let mut settings = reviewed(&mut studio);
            if !legacy {
                settings.speed = [[60., 110.], [140., 250.]];
                settings.distance = [[1000., 6000.], [8000., 30000.]];
            }
            let document = studio.document.clone().unwrap();
            let plan = request(&document, &source(), &settings).unwrap();
            let output = fixture.0.join(if legacy {
                "legacy-shared"
            } else {
                "independent"
            });
            save_review_snapshot(&output, &plan, &document).unwrap();
            let sidecar: serde_json::Value = serde_json::from_slice(
                &std::fs::read(output.with_extension("review.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(sidecar["schema_version"], 1);
            assert_eq!(
                sidecar.as_object().unwrap().len(),
                3,
                "Saved review format must remain unchanged"
            );
            let report =
                wide_area_pair::run(&plan, &output, &AtomicBool::new(true), |_| {}).unwrap();
            assert_eq!(report.status, wide_area_pair::WideStatus::Cancelled);
            assert_eq!(report.progress.completed_receivers, 0);
            let saved = std::fs::read(output.join("request.json")).unwrap();
            studio.wide.settings.speed = [[1., 2.], [3., 4.]];
            studio.wide.settings.distance = [[5., 6.], [7., 8.]];
            let draft = studio.wide.settings.clone();
            studio.accept_wide_reply(load_saved_wide(&output).unwrap());
            assert_eq!(
                studio.wide.report.as_ref().unwrap().request_sha256,
                wide_area_pair::request_sha256(&plan).unwrap()
            );
            assert_eq!(studio.wide.report_document.as_ref(), Some(&document));
            assert_eq!(studio.document.as_ref(), Some(&document));
            assert_eq!(
                studio.wide.settings, draft,
                "Opening a result must not replace active choices"
            );
            assert!(studio.wide.source.is_none() && studio.wide.prepared.is_none());
            assert_eq!(std::fs::read(output.join("request.json")).unwrap(), saved);
            let reopened: WidePairRequest = serde_json::from_slice(&saved).unwrap();
            for i in 0..2 {
                assert_eq!(reopened.coarse_bounds[i].speed_mps, settings.speed[i]);
                assert_eq!(
                    reopened.coarse_bounds[i].slant_distance_m,
                    settings.distance[i]
                );
            }
        }
    }
    fn painted_text(mut paint: impl FnMut(&mut egui::Ui)) -> String {
        let context = egui::Context::default();
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(760., 1200.),
                )),
                ..Default::default()
            },
            |ui| paint(ui),
        );
        fn collect(shape: &egui::epaint::Shape, text: &mut Vec<String>) {
            match shape {
                egui::epaint::Shape::Text(value) => text.push(value.galley.job.text.clone()),
                egui::epaint::Shape::Vec(values) => {
                    values.iter().for_each(|value| collect(value, text))
                }
                _ => {}
            }
        }
        let mut text = Vec::new();
        for shape in &output.shapes {
            collect(&shape.shape, &mut text);
        }
        text.join("\n")
    }
    #[test]
    fn cost_preview_shows_declared_counts_and_necessary_only_budget_warnings() {
        let mut studio = studio();
        let settings = reviewed(&mut studio);
        let document = studio.document.clone().unwrap();
        let source = source();
        let plan = request(&document, &source, &settings).unwrap();
        let mut estimate = estimate_fixture(&plan, &source);
        let old = painted_text(|ui| cost_preview_ui(ui, &estimate));
        assert!(old.contains("unavailable for this older estimate"));
        assert!(!old.contains("0 trace samples"));
        estimate.cost_preview = Some(wide_area_pair::WideCostPreview {
            event_trace_samples: [3, 5],
            event_tone_counts: [61, 126],
            shared_shift_count: 11,
            trace_fit_work_units_per_pair: 20207,
            minimum_track_work_units_per_pair: 320,
            retention_work_units_per_pair: 800,
            index_build_work_units: 600,
            minimum_grid_work_units: 700,
            maximum_timing_pairs_after_minimum_grid_work: 0,
            can_afford_minimum_first_receiver: false,
            can_afford_minimum_first_pair: false,
        });
        let first = painted_text(|ui| cost_preview_ui(ui, &estimate));
        for expected in [
            "Event 1: 3 trace samples",
            "61 tones",
            "Event 2: 5 trace samples",
            "126 tones",
            "11 shared shifts",
            "Detailed work breakdown",
            "minimum setup for the first receiver",
        ] {
            assert!(first.contains(expected), "Missing {expected}: {first}");
        }
        assert!(
            !first.contains("20207 trace-fit"),
            "Detailed accounting should start collapsed"
        );
        let details =
            painted_text(|ui| cost_details_ui(ui, estimate.cost_preview.as_ref().unwrap()));
        assert!(details.contains("20207 trace-fit"));
        assert!(details.contains("not a completion or runtime estimate"));
        estimate
            .cost_preview
            .as_mut()
            .unwrap()
            .can_afford_minimum_first_receiver = true;
        let pair = painted_text(|ui| cost_preview_ui(ui, &estimate));
        assert!(pair.contains("minimum first timing-pair charge"));
        estimate
            .cost_preview
            .as_mut()
            .unwrap()
            .can_afford_minimum_first_pair = true;
        estimate
            .cost_preview
            .as_mut()
            .unwrap()
            .maximum_timing_pairs_after_minimum_grid_work = 3;
        let permitted = painted_text(|ui| cost_preview_ui(ui, &estimate));
        assert!(!permitted.contains("cannot cover"));
        assert!(permitted.contains("Detailed work breakdown"));
        assert_eq!(studio.document.as_ref(), Some(&document));
        assert!(studio.undo.is_empty());
    }
    #[test]
    fn zero_receiver_work_stop_shows_guidance_without_changing_evidence_or_budgets() {
        let fixture = SavedRunFixture::new();
        let mut studio = studio();
        studio.wide.settings = reviewed(&mut studio);
        let document = studio.document.clone().unwrap();
        let settings = studio.wide.settings.clone();
        let plan = request(&document, &source(), &settings).unwrap();
        let mut report = wide_area_pair::run(
            &plan,
            &fixture.0.join("stopped"),
            &AtomicBool::new(true),
            |_| {},
        )
        .unwrap();
        report.status = wide_area_pair::WideStatus::BudgetExceeded;
        report.error = Some("Global wide-area work budget exhausted".into());
        report.progress.scored_candidates = 102;
        studio.accept_wide_reply(WideReply::Report(
            Box::new(report.clone()),
            Some(Box::new(document.clone())),
        ));
        let text = painted_text(|ui| {
            studio.wide_results_ui(ui);
        });
        for expected in [
            "No receiver finished",
            "not a no-match result",
            "102 provisional pair scores",
            "raw integer",
            "Raising only the time limit",
            "re-estimate",
        ] {
            assert!(text.contains(expected), "Missing {expected}: {text}");
        }
        assert_eq!(studio.wide.settings, settings);
        assert_eq!(studio.document.as_ref(), Some(&document));
        assert!(studio.undo.is_empty() && studio.wide.source.is_none());
        for (error, expected) in [
            ("Global wide-area wall budget exhausted", "runtime budget"),
            (
                "Global wide-area output budget exhausted",
                "output-byte budget",
            ),
            (
                "Combined source/spatial-index/local-CPA memory needs 1024 bytes, exceeding the declared 512 bytes; no source pass or receiver was removed",
                "Advanced memory limit",
            ),
            ("Synthetic unknown limit", "reported limit"),
        ] {
            report.error = Some(error.into());
            let guidance = stop_guidance(&report).unwrap();
            assert!(guidance.contains(expected));
            assert!(!guidance.contains("Raising only the time limit"));
        }
        report.status = wide_area_pair::WideStatus::Completed;
        assert!(stop_guidance(&report).is_none());
    }
    #[test]
    fn wide_index_memory_budget_binds_plan_without_changing_review_or_source() {
        let mut studio = studio();
        let mut settings = reviewed(&mut studio);
        assert_eq!(settings.max_index_memory_bytes, 512 * 1024 * 1024);
        assert_eq!(settings.max_index_memory_bytes, DEFAULT_INDEX_MEMORY_BYTES);
        let document = studio.document.clone().unwrap();
        let source = source();
        let original = request(&document, &source, &settings).unwrap();
        let original_json = serde_json::to_value(&original).unwrap();
        assert!(
            original_json["budgets"]
                .get("max_index_memory_bytes")
                .is_none()
        );
        let restored: WidePairRequest = serde_json::from_value(original_json.clone()).unwrap();
        assert_eq!(restored.budgets.max_index_memory_bytes, 512 * 1024 * 1024);
        assert_eq!(serde_json::to_value(restored).unwrap(), original_json);
        for mib in [64, 768, 4096] {
            settings.max_index_memory_bytes = mib * 1024 * 1024;
            let changed = request(&document, &source, &settings).unwrap();
            assert_eq!(changed.budgets.max_index_memory_bytes, mib * 1024 * 1024);
            let mut actual = serde_json::to_value(changed).unwrap();
            assert_eq!(
                actual["budgets"]["max_index_memory_bytes"],
                mib * 1024 * 1024
            );
            actual["budgets"]
                .as_object_mut()
                .unwrap()
                .remove("max_index_memory_bytes");
            assert_eq!(actual, original_json);
        }
        assert_eq!(studio.document.as_ref(), Some(&document));
        assert!(studio.undo.is_empty());
        assert!(studio.wide.source.is_none());
    }
    #[test]
    fn wide_index_memory_change_refuses_an_older_estimate() {
        let mut studio = studio();
        let settings = reviewed(&mut studio);
        let document = studio.document.clone().unwrap();
        let source = source();
        let request = request(&document, &source, &settings).unwrap();
        let estimate = estimate_fixture(&request, &source);
        studio.wide.source = Some(source);
        studio.wide.settings = settings.clone();
        studio.wide.settings.max_index_memory_bytes = 64 * 1024 * 1024;
        studio.accept_wide_reply(WideReply::Estimated(Box::new(Prepared {
            request,
            estimate,
            document: document.clone(),
            settings,
        })));
        assert!(studio.wide.prepared.is_none());
        assert!(studio.wide.source.is_some());
        assert_eq!(studio.document.as_ref(), Some(&document));
        assert!(studio.undo.is_empty());
        assert!(
            studio
                .notice
                .as_ref()
                .unwrap()
                .1
                .contains("options changed")
        );
    }
    #[test]
    fn wide_pair_needs_explicit_confirmed_selection_and_current_source() {
        let mut studio = studio();
        let settings = reviewed(&mut studio);
        let doc = studio.document.as_mut().unwrap();
        doc.events[0].review = ReviewState::Pending;
        assert!(
            request(doc, &source(), &settings)
                .unwrap_err()
                .contains("confirmed")
        );
        doc.events[0].review = ReviewState::Confirmed;
        doc.events[1].selected_for_scoring = false;
        assert!(
            request(doc, &source(), &settings)
                .unwrap_err()
                .contains("exactly two")
        );
        doc.events[1].selected_for_scoring = true;
        doc.datasets.clear();
        assert!(
            request(doc, &source(), &settings)
                .unwrap_err()
                .contains("source")
        );
    }
    #[test]
    fn wide_explicit_grids_reject_overflow_without_allocating() {
        assert_eq!(
            samples([100., 102., 1.], true).unwrap(),
            vec![100., 101., 102.]
        );
        assert!(samples([0., f64::MAX, f64::MIN_POSITIVE], false).is_err());
        assert!(samples([100., 1200., 1.], true).is_err());
        assert!(samples([f64::NAN, 2., 1.], false).is_err());
        assert!(samples([0., 2., 1.], true).is_err());
    }
    #[test]
    fn wide_stale_reply_cannot_clear_new_job_and_exit_cancels_owner() {
        let mut studio = studio();
        studio.wide.run_id = 2;
        studio.wide.running = true;
        studio.busy = Some("new wide job".into());
        studio
            .sender
            .send(Reply {
                generation: studio.generation,
                lane: WorkerLane::WideTask(1),
                result: Err("old failure".into()),
            })
            .unwrap();
        studio.tick(&egui::Context::default());
        assert!(studio.wide.running && studio.busy());
        assert!(studio.notice.is_none());
        let cancel = studio.wide.cancel.clone();
        studio.cancel_for_exit();
        assert!(cancel.load(Ordering::Relaxed));
    }
    #[test]
    fn wide_case_replacement_cancels_old_source_job_and_resets_binding() {
        let mut studio = studio();
        studio.wide.source = Some(source());
        studio.wide.settings.speed = [[60., 100.], [150., 200.]];
        studio.wide.settings.distance = [[1000., 2000.], [5000., 10000.]];
        studio.wide.running = true;
        let cancel = studio.wide.cancel.clone();
        let mut case = studio.case.clone().unwrap();
        case.manifest = case.manifest.with_file_name("second-case.json");
        studio.bind(&case);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(studio.wide.source.is_none() && !studio.wide.running);
        assert_eq!(studio.wide.settings.speed, [[40., 330.]; 2]);
        assert_eq!(studio.wide.settings.distance, [[100., 100_000.]; 2]);
    }
    #[test]
    fn wide_source_deletion_is_scoped_and_retains_historical_result() {
        let mut studio = studio();
        let source = source();
        studio.wide.source = Some(source.clone());
        let report = WidePairReport {
            schema_version: 1,
            request_sha256: "c".repeat(64),
            source: source.binding.clone(),
            source_time_origin_utc_s: Some(source.origin),
            status: wide_area_pair::WideStatus::Cancelled,
            progress: WideProgress::default(),
            best_hypotheses: vec![],
            best_flight_pairs: vec![],
            retain: 200,
            retained_truncated: false,
            pair_retention_truncated: false,
            local_cpa_count: 0,
            boundary_cpa_rejections: 0,
            model_rejections: 0,
            scoring_rejections: 0,
            output_directory: PathBuf::from("unused"),
            pages: vec![],
            error: None,
            location_verified: false,
            source_verified_at_end: true,
            limitations: vec![],
        };
        studio.wide.report = Some(Arc::new(report));
        studio.invalidate_wide_source(|_| false);
        assert!(studio.wide.source.is_some() && !studio.wide.report_source_missing);
        studio.invalidate_wide_source(|path| path == source.binding.manifest_path);
        assert!(studio.wide.source.is_none() && studio.wide.report_source_missing);
        assert!(studio.wide.report.is_some());
    }
    #[test]
    fn indexed_import_defaults_to_entire_day_without_changing_review() {
        let mut studio = studio();
        let _ = reviewed(&mut studio);
        let before = studio.document.clone();
        let entry = data_catalog::CatalogEntry {
            schema_version: 1,
            id: "synthetic".into(),
            case_manifest_sha256: "d".repeat(64),
            label: "Synthetic day archive".into(),
            kind: data_catalog::DataKind::Adsb,
            format: "adsblol_archive".into(),
            primary_file: "archive.tar".into(),
            created_unix_s: 0,
            source: serde_json::Value::Null,
            files: vec![],
            date_utc: Some("2026-09-08".into()),
            bounds: None,
            notes: String::new(),
        };
        let config = studio.indexed_import_config(&entry, "", "", 100.).unwrap();
        assert_eq!(
            config.time_window_utc_s,
            [source().origin, source().origin + 86400.]
        );
        assert!(config.bounds.unwrap()[0] < 24. && config.bounds.unwrap()[1] < -125.);
        assert_eq!(studio.document, before);
        assert!(
            studio
                .indexed_import_config(&entry, "2026-09-07 23:59:59", "", 0.)
                .is_err()
        );
    }

    #[test]
    fn cancelled_source_reply_does_not_register_or_select_source() {
        let mut studio = studio();
        let before = studio.document.clone().unwrap();
        studio.wide.run_id = 7;
        studio.wide.running = true;
        studio.busy = Some("Verifying source".into());
        studio.wide.cancel.store(true, Ordering::Relaxed);
        studio
            .sender
            .send(Reply {
                generation: studio.generation,
                lane: WorkerLane::WideTask(7),
                result: Ok(ResultValue::WideReady(Box::new(WideReply::Source(
                    source(),
                    Box::new(before.clone()),
                )))),
            })
            .unwrap();
        studio.tick(&egui::Context::default());
        assert!(!studio.busy() && !studio.wide.running);
        assert_eq!(studio.document, Some(before));
        assert!(studio.undo.is_empty());
        assert!(studio.wide.source.is_none());
        let notice = studio.notice.as_ref().unwrap();
        assert!(!notice.0 && notice.1.contains("cancelled"));
    }

    #[test]
    fn cancelled_estimate_reply_preserves_review_and_never_enables_run() {
        let mut studio = studio();
        let settings = reviewed(&mut studio);
        let document = studio.document.clone().unwrap();
        let source = source();
        let request = request(&document, &source, &settings).unwrap();
        let estimate = estimate_fixture(&request, &source);
        studio.wide.source = Some(source);
        studio.wide.settings = settings.clone();
        studio.wide.running = true;
        studio.wide.cancel.store(true, Ordering::Relaxed);
        studio.accept_wide_reply(WideReply::Estimated(Box::new(Prepared {
            request,
            estimate,
            document: document.clone(),
            settings,
        })));
        assert!(studio.wide.prepared.is_none() && !studio.wide.running);
        assert!(studio.wide.source.is_some());
        assert_eq!(studio.document, Some(document));
        assert!(studio.undo.is_empty());
        assert!(studio.notice.as_ref().unwrap().1.contains("cancelled"));
    }

    #[test]
    fn cancelled_reopen_keeps_current_report_but_live_cancelled_run_is_retained() {
        let mut studio = studio();
        let source = source();
        let report = WidePairReport {
            schema_version: 1,
            request_sha256: "c".repeat(64),
            source: source.binding,
            source_time_origin_utc_s: None,
            status: wide_area_pair::WideStatus::Cancelled,
            progress: WideProgress::default(),
            best_hypotheses: vec![],
            best_flight_pairs: vec![],
            retain: 200,
            retained_truncated: false,
            pair_retention_truncated: false,
            local_cpa_count: 0,
            boundary_cpa_rejections: 0,
            model_rejections: 0,
            scoring_rejections: 0,
            output_directory: PathBuf::from("unused"),
            pages: vec![],
            error: Some("Cancelled before source verification".into()),
            location_verified: false,
            source_verified_at_end: false,
            limitations: vec![],
        };
        studio.wide.report = Some(Arc::new(report.clone()));
        let before = studio.document.clone();
        let mut incoming = report;
        incoming.request_sha256 = "d".repeat(64);
        studio.wide.running = true;
        studio.wide.cancel.store(true, Ordering::Relaxed);
        studio.accept_wide_reply(WideReply::ReopenedReport(Box::new(incoming.clone()), None));
        assert_eq!(
            studio.wide.report.as_ref().unwrap().request_sha256,
            "c".repeat(64)
        );
        assert!(studio.pending_action.is_none());
        assert_eq!(studio.document, before);
        assert!(studio.notice.as_ref().unwrap().1.contains("cancelled"));
        studio.accept_wide_reply(WideReply::Report(Box::new(incoming), None));
        assert_eq!(
            studio.wide.report.as_ref().unwrap().request_sha256,
            "d".repeat(64)
        );
        assert_eq!(
            studio.wide.report.as_ref().unwrap().status,
            wide_area_pair::WideStatus::Cancelled
        );
        assert!(
            studio
                .wide
                .report
                .as_ref()
                .unwrap()
                .source_time_origin_utc_s
                .is_none()
        );
        assert!(matches!(studio.pending_action, Some(Action::ReviewResults)));
        assert_eq!(studio.document, before);
    }

    #[test]
    fn wide_result_rows_keep_exact_stride_and_reach_last_row_when_narrow_or_scaled() {
        for (width, font_scale) in [(240., 1.), (480., 1.), (240., 1.8), (900., 1.8)] {
            for at_end in [false, true] {
                let ctx = egui::Context::default();
                for theme in [egui::Theme::Light, egui::Theme::Dark] {
                    let mut style = (*ctx.style_of(theme)).clone();
                    for font in style.text_styles.values_mut() {
                        font.size *= font_scale;
                    }
                    ctx.set_style_of(theme, style);
                }
                let mut observed = Vec::new();
                let mut expected_height = 0.;
                let mut expected_stride = 0.;
                let output=ctx.run_ui(egui::RawInput {screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(width,320.))),..Default::default()},|ui| {
                    expected_height=wide_result_row_height(ui);
                    expected_stride=expected_height+ui.spacing().item_spacing.y;
                    egui::ScrollArea::vertical().max_height(180.)
                        .vertical_scroll_offset(if at_end { (expected_stride*100.-160.).max(0.) } else { 0. })
                        .show_rows(ui,expected_height,100,|ui,rows| {
                            for index in rows {
                                let summary=format!("wide-row-control-{index}: 0.1234 Hz · 45.123456, -71.123456 · 2026-09-08 20:30:00 UTC · {} + {}", "long-flight-id-".repeat(18),"other-flight-id-".repeat(17));
                                let id=ui.make_persistent_id(("control",index));
                                let (button,row)=wide_result_row(ui,id,&summary,true);
                                observed.push((index,row,button.rect));
                            }
                        });
                });
                assert!(!observed.is_empty());
                if at_end {
                    assert_eq!(observed.last().unwrap().0, 99);
                } else {
                    assert_eq!(observed.first().unwrap().0, 0);
                }
                for (_, row, button) in &observed {
                    assert!((row.height() - expected_height).abs() < 0.1);
                    assert!(
                        row.expand(0.1).contains_rect(*button),
                        "button escaped its row at width {width}, scale {font_scale}"
                    );
                }
                for pair in observed.windows(2) {
                    assert!((pair[1].1.top() - pair[0].1.top() - expected_stride).abs() < 0.1);
                    assert!(pair[1].1.top() >= pair[0].1.bottom());
                }
                fn summaries(shape: &egui::epaint::Shape, count: &mut usize) {
                    match shape {
                        egui::epaint::Shape::Text(text)
                            if text.galley.job.text.starts_with("wide-row-control-") =>
                        {
                            assert_eq!(text.galley.rows.len(), 1, "a virtual row summary wrapped");
                            *count += 1;
                        }
                        egui::epaint::Shape::Vec(shapes) => {
                            for shape in shapes {
                                summaries(shape, count);
                            }
                        }
                        _ => {}
                    }
                }
                let mut summary_count = 0;
                for clipped in &output.shapes {
                    summaries(&clipped.shape, &mut summary_count);
                }
                assert!(summary_count > 0, "expected real rendered summaries");
            }
        }
    }
}
impl Default for Settings {
    fn default() -> Self {
        let budget = WideBudgets::default();
        Self {
            event_ids: Default::default(),
            cpa_s: [0., 0.],
            tones: [[80., 200., 1.]; 2],
            speed: [[40., 330.]; 2],
            distance: [[100., 100_000.]; 2],
            separation_tolerance_s: 30.,
            shift: [-5., 5., 1.],
            utc_start: String::new(),
            utc_end: String::new(),
            max_work: budget.max_work_units,
            max_output: budget.max_output_bytes,
            max_seconds: budget.max_duration_s,
            max_index_memory_bytes: DEFAULT_INDEX_MEMORY_BYTES,
            retain: 200,
        }
    }
}
#[derive(Clone)]
pub(super) struct IndexedSource {
    pub(super) binding: TrackStoreBinding,
    pub(super) label: String,
    pub(super) passes: usize,
    pub(super) points: u64,
    pub(super) origin: f64,
    pub(super) utc_window: [f64; 2],
    pub(super) max_gap_s: f64,
}
pub(super) struct Prepared {
    request: WidePairRequest,
    estimate: WideEstimate,
    document: ReviewWorkspace,
    settings: Settings,
}
pub(super) enum WideReply {
    Imported(Box<data_catalog::CatalogEntry>),
    Source(IndexedSource, Box<ReviewWorkspace>),
    Estimated(Box<Prepared>),
    Report(Box<WidePairReport>, Option<Box<ReviewWorkspace>>),
    ReopenedReport(Box<WidePairReport>, Option<Box<ReviewWorkspace>>),
}
pub(super) struct WideState {
    pub enabled: bool,
    pub running: bool,
    pub run_id: u64,
    pub cancel: Arc<AtomicBool>,
    pub progress: Option<WideProgress>,
    source: Option<IndexedSource>,
    settings: Settings,
    prepared: Option<Box<Prepared>>,
    report: Option<Arc<WidePairReport>>,
    report_document: Option<ReviewWorkspace>,
    report_source_missing: bool,
    show_results: bool,
    sort_column: usize,
    sort_reverse: bool,
    row_order: Vec<usize>,
    started: Option<Instant>,
    import_start: String,
    import_end: String,
    import_padding_km: f64,
    import_disk_gib: f64,
}
impl Default for WideState {
    fn default() -> Self {
        Self {
            enabled: false,
            running: false,
            run_id: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
            source: None,
            settings: Settings::default(),
            prepared: None,
            report: None,
            report_document: None,
            report_source_missing: false,
            show_results: false,
            sort_column: 0,
            sort_reverse: false,
            row_order: vec![],
            started: None,
            import_start: String::new(),
            import_end: String::new(),
            import_padding_km: 100.,
            import_disk_gib: 8.,
        }
    }
}

fn utc_input(seconds: f64) -> String {
    regional::utc_label(seconds).chars().take(19).collect()
}

fn wide_checkpoint(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Wide-area work cancelled".into())
    } else {
        Ok(())
    }
}

fn samples(bounds: [f64; 3], positive: bool) -> Result<Vec<f64>, String> {
    let [low, high, step] = bounds;
    if !bounds.iter().all(|v| v.is_finite()) || low > high || step <= 0. || positive && low <= 0. {
        return Err("Grid ranges need finite ordered bounds and a positive step".into());
    }
    let steps = ((high - low) / step).floor();
    if !steps.is_finite() || steps >= 1001. {
        return Err("A tone/shared-shift grid is limited to 1,001 values".into());
    }
    let count = steps as usize + 1;
    Ok((0..count).map(|i| low + i as f64 * step).collect())
}
fn reviewed_pair(doc: &ReviewWorkspace) -> Result<[&EvidenceEvent; 2], String> {
    let mut selected: Vec<_> = doc
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Doppler && e.selected_for_scoring)
        .collect();
    if selected.len() != 2 {
        return Err("Select exactly two Doppler events in Audio review for this method".into());
    }
    selected.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
    for event in &selected {
        if event.review != ReviewState::Confirmed || event.points.len() < 3 {
            return Err(format!(
                "{} needs confirmed measurements and at least three trace points",
                event.label
            ));
        }
        if let Some(reason) = event.acoustic_source_block_reason() {
            return Err(reason);
        }
    }
    if selected[0].end_s >= selected[1].start_s {
        return Err("The two reviewed event windows must be ordered and nonoverlapping".into());
    }
    Ok([selected[0], selected[1]])
}
fn copy_reviewed_pair(doc: &ReviewWorkspace, settings: &mut Settings) -> Result<(), String> {
    let pair = reviewed_pair(doc)?;
    for (i, event) in pair.into_iter().enumerate() {
        settings.event_ids[i] = event.id.clone();
        settings.cpa_s[i] = event.arrival_s;
        let low = event
            .points
            .iter()
            .map(|p| p.frequency_hz)
            .fold(f64::INFINITY, f64::min);
        let high = event
            .points
            .iter()
            .map(|p| p.frequency_hz)
            .fold(0., f64::max);
        settings.tones[i] = [(low * 0.9).max(0.1), high * 1.1, 1.];
    }
    // Copying measurements never changes the analyst's physical limits.
    Ok(())
}
fn physical_range_ui(ui: &mut egui::Ui, label: &str, range: &mut [f64; 2], unit: &str, speed: f64) {
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        ui.label("Min");
        ui.add(
            egui::DragValue::new(&mut range[0])
                .suffix(unit)
                .speed(speed),
        );
        ui.label("Max");
        ui.add(
            egui::DragValue::new(&mut range[1])
                .suffix(unit)
                .speed(speed),
        );
    });
}
fn stop_guidance(report: &WidePairReport) -> Option<&'static str> {
    use wide_area_pair::WideStatus;
    match report.status {
        WideStatus::Completed => None,
        WideStatus::BudgetExceeded => Some(match report.error.as_deref() {
            Some("Global wide-area work budget exhausted") => {
                "The work-unit limit stopped this run. Review each event's evidence-supported tone and physical ranges and the work-unit budget (a raw integer), then re-estimate. Raising only the time limit does not remove this limit; a larger work budget is not a completion guarantee."
            }
            Some("Global wide-area wall budget exhausted") => {
                "The runtime limit stopped this run. Review the runtime budget and re-estimate before starting a new run. Extra time does not change the work or memory limits."
            }
            Some(
                "Global wide-area output budget exhausted"
                | "Wide-area output budget exhausted"
                | "Wide-area checkpoint budget exhausted"
                | "Wide-area final report exceeds its reserved output budget",
            ) => {
                "The output limit stopped this run. Review the output-byte budget and available disk space, then re-estimate before starting a new run."
            }
            Some(error)
                if geosonnar::track_store::is_memory_limit_error(error)
                    || error
                        .starts_with("Combined source/spatial-index/local-CPA memory needs ") =>
            {
                "The source/index memory limit stopped this run. Review Advanced memory limit and available system memory, then re-estimate. This allowance is separate from the track cache and total application memory."
            }
            _ => {
                "Review the reported limit and the applicable search setting, then re-estimate before starting a new run. The saved result remains incomplete; increasing an unrelated budget may not resolve the stop."
            }
        }),
        WideStatus::Cancelled => Some(
            "The saved run remains incomplete. Review the event ranges and budgets and re-estimate before starting a new run; reopening does not resume it.",
        ),
        WideStatus::Failed => Some(
            "Review the saved error and source state before re-estimating a new run. An incomplete search cannot establish that no matching source exists.",
        ),
    }
}
fn cost_preview_ui(ui: &mut egui::Ui, estimate: &WideEstimate) {
    let Some(cost) = &estimate.cost_preview else {
        muted(
            ui,
            "Detailed work breakdown is unavailable for this older estimate; re-estimate to obtain it.",
        );
        return;
    };
    ui.label(format!("Event 1: {} trace samples × {} tones · Event 2: {} trace samples × {} tones · {} shared shifts",
        cost.event_trace_samples[0], cost.event_tone_counts[0], cost.event_trace_samples[1], cost.event_tone_counts[1], cost.shared_shift_count));
    egui::CollapsingHeader::new("Detailed work breakdown")
        .id_salt("wide_cost_details")
        .show(ui, |ui| cost_details_ui(ui, cost));
    if !cost.can_afford_minimum_first_receiver {
        ui.colored_label(tone(ui, AMBER), "The work budget cannot cover even the minimum setup for the first receiver. Review the work budget and re-estimate.");
    } else if !cost.can_afford_minimum_first_pair {
        ui.colored_label(tone(ui, AMBER), "The work budget cannot cover even the minimum first timing-pair charge. Review the work budget and re-estimate.");
    }
}
fn cost_details_ui(ui: &mut egui::Ui, cost: &wide_area_pair::WideCostPreview) {
    ui.label(format!("Minimum work per timing pair: {} trace-fit + {} track + {} retention units. Minimum spatial-index construction: {} units.",
        cost.trace_fit_work_units_per_pair, cost.minimum_track_work_units_per_pair, cost.retention_work_units_per_pair, cost.index_build_work_units));
    ui.label(format!("Full-grid work floor: {} units; at most {} timing pairs after that floor. Traversal, source census and joining cost more; this is not a completion or runtime estimate.",
        cost.minimum_grid_work_units, cost.maximum_timing_pairs_after_minimum_grid_work));
}
fn request(
    doc: &ReviewWorkspace,
    source: &IndexedSource,
    settings: &Settings,
) -> Result<WidePairRequest, String> {
    if !doc.datasets.iter().any(|b| {
        b.path == source.binding.manifest_path && b.sha256 == source.binding.manifest_sha256
    }) {
        return Err("Select the indexed source for this review again".into());
    }
    let events = reviewed_pair(doc)?;
    if settings.event_ids != [events[0].id.clone(), events[1].id.clone()] {
        return Err(
            "Copy the current reviewed pair into this plan, then inspect its timing and tone grids"
                .into(),
        );
    }
    for i in 0..2 {
        for (name, [low, high]) in [
            ("speed", settings.speed[i]),
            ("slant distance", settings.distance[i]),
        ] {
            if !low.is_finite() || !high.is_finite() || low < 0. || low > high {
                return Err(format!(
                    "Event {} needs finite, nonnegative, ordered {name} bounds",
                    i + 1
                ));
            }
        }
    }
    if settings
        .cpa_s
        .iter()
        .any(|v| !v.is_finite() || *v < 0. || *v > doc.duration_s)
        || settings.cpa_s[0] >= settings.cpa_s[1]
    {
        return Err("Review two ordered CPA anchors inside the recording".into());
    }
    let start = workspace::parse_utc(&settings.utc_start)? - source.origin;
    let end = workspace::parse_utc(&settings.utc_end)? - source.origin;
    if start >= end {
        return Err("Recording-start UTC bounds must be ordered".into());
    }
    let event = |i: usize| -> Result<geosonnar::physics::EventTrace, String> {
        Ok(geosonnar::physics::EventTrace {
            points: events[i]
                .points
                .iter()
                .map(|p| geosonnar::physics::TracePoint {
                    t_rec_s: p.time_s,
                    frequency_hz: p.frequency_hz,
                })
                .collect(),
            f0_hz: samples(settings.tones[i], true)?,
        })
    };
    Ok(WidePairRequest {
        schema_version: 1,
        document_sha256: geosonnar::regional_search::document_sha256(doc)?,
        recording_sha256: doc.recording_sha256.clone(),
        channel: doc.channel,
        grid: GridLayout::new(&doc.map)?,
        source: source.binding.clone(),
        events: [event(0)?, event(1)?],
        pair_config: geosonnar::physics::PairConfig {
            event1_cpa_recording_s: settings.cpa_s[0],
            shared_shifts_s: samples(settings.shift, false)?,
            clock_offset_bounds_s: Some([start, end]),
            model: geosonnar::physics::ModelConfig {
                sound_speed_mps: doc.sound_speed_mps,
                max_gap_s: source.max_gap_s,
            },
        },
        coarse_bounds: std::array::from_fn(|i| geosonnar::verification::CoarseBounds {
            speed_mps: settings.speed[i],
            slant_distance_m: settings.distance[i],
        }),
        measured_separation_s: settings.cpa_s[1] - settings.cpa_s[0],
        separation_tolerance_s: settings.separation_tolerance_s,
        budgets: WideBudgets {
            max_work_units: settings.max_work,
            max_output_bytes: settings.max_output,
            max_duration_s: settings.max_seconds,
            max_index_memory_bytes: settings.max_index_memory_bytes,
        },
        retain: settings.retain,
    })
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewSnapshot {
    schema_version: u32,
    request_sha256: String,
    document: ReviewWorkspace,
}
fn ordinary_result_parents(path: &std::path::Path) -> Result<(), String> {
    for parent in path.ancestors() {
        if parent.as_os_str().is_empty() {
            continue;
        }
        let metadata = std::fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("Wide-area output parents must be ordinary directories".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("Wide-area output parents cannot be reparse points".into());
            }
        }
    }
    Ok(())
}
fn save_review_snapshot(
    output: &std::path::Path,
    request: &WidePairRequest,
    document: &ReviewWorkspace,
) -> Result<(), String> {
    use std::io::Write;
    let snapshot = ReviewSnapshot {
        schema_version: 1,
        request_sha256: wide_area_pair::request_sha256(request)?,
        document: document.clone(),
    };
    let bytes = serde_json::to_vec(&snapshot).map_err(|e| e.to_string())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("Wide-area review snapshot exceeds 8 MiB; nothing was scored".into());
    }
    ordinary_result_parents(output.parent().ok_or("Missing output parent")?)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.with_extension("review.json"))
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn load_saved_wide(path: &std::path::Path) -> Result<WideReply, String> {
    use std::io::Read;
    let report = wide_area_pair::load_report(path)?;
    let sidecar = path.with_extension("review.json");
    let document = if sidecar.try_exists().map_err(|e| e.to_string())? {
        let metadata = std::fs::symlink_metadata(&sidecar).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("Saved review snapshot is not an ordinary file".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("Saved review snapshot is a reparse point".into());
            }
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&sidecar)
            .map_err(|e| e.to_string())?
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Saved review snapshot exceeds its bound".into());
        }
        let snapshot: ReviewSnapshot = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let mut request_bytes = Vec::new();
        std::fs::File::open(path.join("request.json"))
            .map_err(|e| e.to_string())?
            .take(wide_area_pair::MAX_REQUEST_BYTES as u64 + 1)
            .read_to_end(&mut request_bytes)
            .map_err(|e| e.to_string())?;
        if request_bytes.len() > wide_area_pair::MAX_REQUEST_BYTES {
            return Err("Saved request exceeds its bound".into());
        }
        let request: WidePairRequest =
            serde_json::from_slice(&request_bytes).map_err(|e| e.to_string())?;
        snapshot.document.validate()?;
        if snapshot.schema_version != 1
            || snapshot.request_sha256 != report.request_sha256
            || wide_area_pair::request_sha256(&request)? != report.request_sha256
            || geosonnar::regional_search::document_sha256(&snapshot.document)?
                != request.document_sha256
        {
            return Err("Saved review/request identity differs".into());
        }
        Some(Box::new(snapshot.document))
    } else {
        None
    };
    Ok(WideReply::ReopenedReport(Box::new(report), document))
}
fn range_ui(ui: &mut egui::Ui, label: &str, values: &mut [f64; 3], suffix: &str) {
    ui.label(label);
    ui.horizontal_wrapped(|ui| {
        for (name, value) in ["From", "To", "Step"].into_iter().zip(values) {
            ui.add(
                egui::DragValue::new(value)
                    .prefix(format!("{name} "))
                    .suffix(suffix)
                    .speed(0.5),
            );
        }
    });
}

fn wide_result_line_height(ui: &egui::Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Body)
        .max(ui.text_style_height(&egui::TextStyle::Button) + 2. * ui.spacing().button_padding.y)
        .max(ui.spacing().interact_size.y)
}

fn wide_result_row_height(ui: &egui::Ui) -> f32 {
    2. * wide_result_line_height(ui) + ui.spacing().item_spacing.y
}

/// `show_rows` requires every row to consume exactly the declared height.
/// Long source identifiers remain available in the tooltip, never extra lines
/// that change the virtual scroll offsets when the window is narrowed.
fn wide_result_row(
    ui: &mut egui::Ui,
    id: egui::Id,
    summary: &str,
    can_refine: bool,
) -> (egui::Response, egui::Rect) {
    let line_height = wide_result_line_height(ui);
    let width = ui.available_width().max(1.);
    let (row, _) = ui.allocate_exact_size(
        egui::vec2(width, wide_result_row_height(ui)),
        egui::Sense::hover(),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().id(id).max_rect(row));
    child.set_clip_rect(ui.clip_rect().intersect(row));
    let summary_rect = egui::Rect::from_min_size(row.min, egui::vec2(width, line_height));
    child
        .place(summary_rect, egui::Label::new(summary).truncate())
        .on_hover_text(summary);
    let button_width = (ui
        .painter()
        .layout_no_wrap(
            "Prepare local area".into(),
            egui::TextStyle::Button.resolve(ui.style()),
            Color32::WHITE,
        )
        .size()
        .x
        + 2. * ui.spacing().button_padding.x)
        .min(width);
    let button_rect = egui::Rect::from_min_size(
        egui::pos2(row.left(), row.bottom() - line_height),
        egui::vec2(button_width, line_height),
    );
    if !can_refine {
        child.disable();
    }
    let response = child.place(
        button_rect,
        egui::Button::new("Prepare local area").truncate(),
    );
    (response, row)
}

impl Studio {
    pub(super) fn wide_index_memory_ui(&mut self, ui: &mut egui::Ui) {
        const MIB: u64 = 1024 * 1024;
        let mib = self.wide.settings.max_index_memory_bytes / MIB;
        egui::CollapsingHeader::new(format!("Advanced memory limit · {mib} MiB"))
            .id_salt("wide_index_memory")
            .show(ui, |ui| {
                ui.add_enabled_ui(!self.busy(), |ui| {
                    let mut value = self.wide.settings.max_index_memory_bytes / MIB;
                    if ui.add(egui::DragValue::new(&mut value).range(64..=4096).suffix(" MiB source/index memory")).changed() {
                        self.wide.settings.max_index_memory_bytes = value * MIB;
                    }
                    muted(ui, "Shared by indexed import, source loading and wide-area search. The search track cache has a separate 256 MiB limit. This is not a total application memory cap; exceeding a budget stops the operation without reducing the data scope.");
                });
            });
    }

    pub(super) fn wide_dependency_paths(&self) -> impl Iterator<Item = &std::path::Path> {
        self.wide
            .source
            .iter()
            .map(|source| source.binding.manifest_path.as_path())
            .chain(
                self.wide
                    .report
                    .iter()
                    .map(|report| report.source.manifest_path.as_path()),
            )
    }
    pub(super) fn wide_report_document(&self) -> Option<&ReviewWorkspace> {
        self.wide.report_document.as_ref()
    }
    pub(super) fn invalidate_wide_source(&mut self, affected: impl Fn(&std::path::Path) -> bool) {
        if self
            .wide
            .source
            .as_ref()
            .is_some_and(|s| affected(&s.binding.manifest_path))
        {
            self.wide.source = None;
            self.wide.prepared = None;
        }
        if self
            .wide
            .report
            .as_ref()
            .is_some_and(|r| affected(&r.source.manifest_path))
        {
            self.wide.report_source_missing = true;
        }
    }
    fn start_wide_task<F>(&mut self, ctx: &egui::Context, label: &str, task: F)
    where
        F: FnOnce() -> Result<ResultValue, String> + Send + 'static,
    {
        self.wide.run_id = self.wide.run_id.wrapping_add(1);
        self.wide.running = true;
        self.wide.started = Some(Instant::now());
        self.wide.progress = None;
        self.busy = Some(label.into());
        self.notice = None;
        self.spawn_lane(ctx, WorkerLane::WideTask(self.wide.run_id), task);
    }

    pub(super) fn indexed_archive_ui(
        &mut self,
        ui: &mut egui::Ui,
        entry: &data_catalog::CatalogEntry,
    ) {
        ui.separator();
        ui.strong("Wide-area search data — keep all selected points on disk");
        muted(
            ui,
            "This indexed copy can exceed two million points. It stays separate from the display map and preserves the requested full-day scope. Source, altitude and gap settings above also apply.",
        );
        self.wide_index_memory_ui(ui);
        ui.add_enabled_ui(!self.busy(), |ui| {
            if ui.button("Use complete archive UTC day").clicked() && let Some(date) = &entry.date_utc
                && let Ok(start) = geosonnar::detailed_data::utc_midnight(date) {
                self.wide.import_start = utc_input(start);
                self.wide.import_end = utc_input(start + 86400.);
            }
            ui.label("UTC source interval (blank fields use the complete archive day)");
            accessible_text(ui, "wide_import_start", &mut self.wide.import_start, "YYYY-MM-DD HH:MM:SS", false, ui.available_width());
            accessible_text(ui, "wide_import_end", &mut self.wide.import_end, "YYYY-MM-DD HH:MM:SS", false, ui.available_width());
            ui.add(egui::DragValue::new(&mut self.wide.import_padding_km).range(0.0..=500.0).suffix(" km aircraft-area padding"));
            ui.add(egui::DragValue::new(&mut self.wide.import_disk_gib).range(0.1..=1024.0).suffix(" GiB disk budget"));
            muted(ui, "The UTC interval is explicit: allow propagation/interpolation look-back when narrowing it. Adjacent-day data require their own archive selection; this action never silently clips or expands time.");
            if ui.button("Build indexed flight data").clicked() {
                let prepared = self.indexed_import_config(entry, &self.wide.import_start, &self.wide.import_end, self.wide.import_padding_km);
                match prepared {
                    Err(error) => self.notice = Some((true, error)),
                    Ok(config) => {
                        let manifest = self.case.as_ref().unwrap().manifest.clone(); let id = entry.id.clone();
                        let bytes = (self.wide.import_disk_gib * 1024_f64.powi(3)) as u64;
                        let memory = self.wide.settings.max_index_memory_bytes;
                        self.wide.cancel = Arc::new(AtomicBool::new(false));
                        self.download_cancel = self.wide.cancel.clone(); let cancel = self.wide.cancel.clone();
                        self.start_wide_task(ui.ctx(), "Scanning archive and building a bounded disk index…", move || {
                            data_catalog::import_adsblol_indexed_with_limits(&manifest, &id, &config, bytes, memory, &cancel)
                                .map(|entry| ResultValue::WideReady(Box::new(WideReply::Imported(Box::new(entry)))))
                        });
                    }
                }
            }
        });
    }

    pub(super) fn load_indexed_source(
        &mut self,
        ctx: &egui::Context,
        entry: &data_catalog::CatalogEntry,
    ) {
        if self.busy() {
            return;
        }
        let (Some(case), Some(previous)) = (&self.case, self.document.clone()) else {
            return;
        };
        let manifest = case.manifest.clone();
        let id = entry.id.clone();
        let primary = entry.primary_file.clone();
        let label = entry.label.clone();
        let memory = self.wide.settings.max_index_memory_bytes;
        self.wide.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.wide.cancel.clone();
        self.start_wide_task(
            ctx,
            "Verifying the complete indexed flight source…",
            move || {
                wide_checkpoint(&cancel)?;
                let path = data_catalog::verified_files_with_cancel(&manifest, &id, &cancel)?
                    .into_iter()
                    .find(|p| p.ends_with(primary.as_path()))
                    .ok_or("Index primary file is missing")?;
                let store =
                    TrackStore::open_with_memory_limit(&path, memory, || wide_checkpoint(&cancel))?;
                let source = IndexedSource {
                    binding: store.binding().clone(),
                    label,
                    passes: store.passes().len(),
                    points: store.total_points(),
                    origin: store.time_origin_utc_s(),
                    utc_window: store.import_config().time_window_utc_s,
                    max_gap_s: store.import_config().max_gap_s,
                };
                wide_checkpoint(&cancel)?;
                Ok(ResultValue::WideReady(Box::new(WideReply::Source(
                    source,
                    Box::new(previous),
                ))))
            },
        );
    }

    pub(super) fn accept_wide_reply(&mut self, reply: WideReply) {
        self.wide.running = false;
        if self.wide.cancel.load(Ordering::Relaxed)
            && matches!(
                &reply,
                WideReply::Source(..) | WideReply::Estimated(..) | WideReply::ReopenedReport(..)
            )
        {
            self.notice = Some((false, "Wide-area verification cancelled; no source selection, new estimate or reopened report was applied.".into()));
            return;
        }
        match reply {
            WideReply::Imported(entry) => {
                self.data.select_entry(entry.id.clone());
                self.data.refresh_requested = true;
                self.notice = Some((
                    false,
                    format!(
                        "{} is ready. Select Use indexed flight data to include it in a wide-area search; no map points or events were selected automatically.",
                        entry.label
                    ),
                ));
            }
            WideReply::Source(source, previous) => {
                if self.document.as_ref() != Some(previous.as_ref()) {
                    self.notice = Some((
                        true,
                        "Review changed while checking indexed data; select the source again."
                            .into(),
                    ));
                    return;
                }
                let doc = self.document.as_mut().unwrap();
                if let Err(error) = doc.register_dataset(
                    SourceBinding {
                        path: source.binding.manifest_path.clone(),
                        sha256: source.binding.manifest_sha256.clone(),
                    },
                    Some(source.origin),
                    false,
                ) {
                    self.notice = Some((true, error));
                    return;
                }
                self.checkpoint(*previous);
                self.restore_deleted_source(&SourceBinding {
                    path: source.binding.manifest_path.clone(),
                    sha256: source.binding.manifest_sha256.clone(),
                });
                self.wide.settings.utc_start = utc_input(source.utc_window[0]);
                self.wide.settings.utc_end = utc_input(source.utc_window[1]);
                self.wide.source = Some(source);
                self.wide.prepared = None;
                self.notice = Some((false, "Indexed flight data selected. Open Search → Wide-area two-pass Doppler. Map previews are not used for scoring.".into()));
            }
            WideReply::Estimated(prepared) => {
                if self.document.as_ref() == Some(&prepared.document)
                    && self.wide.settings == prepared.settings
                    && self
                        .wide
                        .source
                        .as_ref()
                        .is_some_and(|s| s.binding == prepared.request.source)
                {
                    self.wide.prepared = Some(prepared);
                } else {
                    self.notice = Some((
                        true,
                        "Review or search options changed; estimate the current plan again.".into(),
                    ));
                }
            }
            WideReply::Report(report, document) | WideReply::ReopenedReport(report, document) => {
                self.wide.row_order = (0..report.best_hypotheses.len()).collect();
                self.wide.sort_column = 0;
                self.wide.sort_reverse = false;
                self.wide.report_source_missing = document
                    .as_deref()
                    .is_some_and(|doc| !self.deleted_sources_ready(doc));
                self.wide.report = Some(Arc::new(*report));
                self.wide.report_document = document.map(|d| *d);
                self.wide.show_results = true;
                self.pending_action = Some(Action::ReviewResults);
                self.notice = Some((false, "Wide-area result saved. Review its completion status and alternatives; nothing was applied to the evidence.".into()));
            }
        }
    }

    pub(super) fn wide_progress_ui(&mut self, ui: &mut egui::Ui) {
        if !self.wide.running {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Wide-area work · {:.0}s",
                self.wide.started.map_or(0., |s| s.elapsed().as_secs_f64())
            ));
            if self.wide.cancel.load(Ordering::Relaxed) {
                ui.label("Cancellation requested; waiting for the current validation checkpoint. Saved-page validation may finish before cancellation is applied.");
            } else if ui.button("Cancel wide-area work").clicked() {
                self.wide.cancel.store(true, Ordering::Relaxed);
            }
        });
        if let Some(progress) = &self.wide.progress {
            ui.label(format!("{} · {} / {} receivers · {} remaining · {} pass checks · {} timing pairs · {} scored", progress.stage,
                progress.completed_receivers, progress.total_receivers, progress.total_receivers.saturating_sub(progress.completed_receivers), progress.scanned_passes, progress.timing_candidates, progress.scored_candidates));
        }
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }

    pub(super) fn wide_controls_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Wide-area two-pass Doppler");
        muted(
            ui,
            "Full indexed trajectories and all local CPA alternatives. This separate method uses two reviewed frequency traces and their time separation; it does not combine thunder or change your evidence selections.",
        );
        if let Some(source) = &self.wide.source {
            ui.label(format!(
                "{} · {} passes · {} points on disk",
                source.label, source.passes, source.points
            ));
            ui.label(format!(
                "Source interval: {} — {} · maximum interpolation gap {} s",
                regional::utc_label(source.utc_window[0]),
                regional::utc_label(source.utc_window[1]),
                source.max_gap_s
            ));
        } else {
            ui.colored_label(tone(ui, AMBER), "Data → select a complete day archive → Build indexed flight data → Use indexed flight data.");
        }
        ui.add_enabled_ui(!self.busy(), |ui| {
            let Some(doc) = &mut self.document else { return; };
            ui.strong("1 · Area, grid and recording-start interval");
            for (label, value) in ["South", "West", "North", "East"].into_iter().zip(&mut doc.map.bounds) {
                ui.horizontal(|ui| { ui.label(label); ui.add(egui::DragValue::new(value).speed(0.01)); });
            }
            egui::ComboBox::from_id_salt("wide_spacing").selected_text(format!("{} m", doc.map.grid_spacing_m)).show_ui(ui, |ui| {
                for value in [100_000., 50_000., 25_000., 10_000., 2000., 500., 100., 25.] {
                    ui.selectable_value(&mut doc.map.grid_spacing_m, value, format!("{value} m"));
                }
            });
            ui.add(egui::DragValue::new(&mut doc.map.grid_spacing_m).range(10.0..=100_000.0).suffix(" m maximum spacing"));
            match GridLayout::new(&doc.map) {
                Ok(grid) => { ui.label(format!("{} receivers · {} rows × {} columns · streamed, not a 10,000-point map layer", grid.count, grid.rows, grid.columns)); }
                Err(error) => { ui.colored_label(tone(ui, RED), error); }
            }
            accessible_text(ui, "wide_clock_start", &mut self.wide.settings.utc_start, "Earliest recording start UTC", false, ui.available_width());
            accessible_text(ui, "wide_clock_end", &mut self.wide.settings.utc_end, "Latest recording start UTC", false, ui.available_width());
            ui.add(egui::DragValue::new(&mut doc.map.receiver_height_m).suffix(" m receiver ellipsoid height"));
            ui.add(egui::DragValue::new(&mut doc.sound_speed_mps).suffix(" m/s sound speed"));
            muted(ui, "First wide-area pass uses this explicit constant receiver height and sound speed. Terrain/atmospheric refinement remains a separate reviewed comparison.");
            ui.separator(); ui.strong("2 · Reviewed pair and explicit physical assumptions");
            if ui.button("Copy the two selected reviewed Doppler events").clicked()
                && let Err(error) = copy_reviewed_pair(doc, &mut self.wide.settings) {
                self.notice = Some((true, error));
            }
            muted(ui, "Copy updates timing and tone suggestions. Each event's physical limits stay as entered; review them when changing the selected pair.");
            for i in 0..2 {
                ui.push_id(("wide_event", i), |ui| {
                    ui.strong(format!("Event {}: {}", i + 1, self.wide.settings.event_ids[i]));
                    ui.add(egui::DragValue::new(&mut self.wide.settings.cpa_s[i]).suffix(" s CPA anchor").speed(0.1));
                    range_ui(ui, "Emitted tone grid — review the suggested range", &mut self.wide.settings.tones[i], " Hz");
                    physical_range_ui(ui, &format!("Event {} speed", i + 1), &mut self.wide.settings.speed[i], " m/s", 1.);
                    physical_range_ui(ui, &format!("Event {} slant distance", i + 1), &mut self.wide.settings.distance[i], " m", 100.);
                });
            }
            range_ui(ui, "Shared shift around CPA-derived clock", &mut self.wide.settings.shift, " s");
            ui.add(egui::DragValue::new(&mut self.wide.settings.separation_tolerance_s).range(0.0..=3600.0).suffix(" s pair-separation tolerance"));
            ui.separator(); ui.strong("3 · Estimate, budgets and run");
            ui.add(egui::DragValue::new(&mut self.wide.settings.max_work).suffix(" work-unit budget").speed(100_000.));
            muted(ui, "Work units are a raw integer budget. Runtime and memory have separate limits; increasing time alone does not resolve a work-unit stop.");
            ui.add(egui::DragValue::new(&mut self.wide.settings.max_seconds).range(1..=86400).suffix(" s runtime budget"));
            ui.add(egui::DragValue::new(&mut self.wide.settings.max_output).suffix(" output bytes").speed(1_048_576.));
            ui.add(egui::DragValue::new(&mut self.wide.settings.retain).range(1..=1000).suffix(" retained rows"));
            self.wide_index_memory_ui(ui);
            if ui.button("Estimate complete wide-area plan").clicked() { self.prepare_wide(ui.ctx(), false); }
            if let Some(prepared) = &self.wide.prepared {
                let e = &prepared.estimate;
                ui.label(format!("{} receivers · {} source passes · {} source points", e.receiver_count, e.source_passes, e.source_points));
                ui.label(format!("Unpruned ceiling: {} pass checks / {} point visits. Filtering determines actual pair work.", e.unpruned_pass_checks, e.worst_scan_point_visits));
                ui.label(format!("Minimum output reservation: {} bytes", e.minimum_output_bytes));
                ui.label(format!("Source/index memory reservation: {:.1} MiB total · {:.1} MiB source/verification · {:.1} MiB spatial index · {:.1} MiB local CPA reserve. Separate track cache: 256 MiB.", e.required_index_memory_bytes as f64 / 1_048_576., e.source_metadata_bytes as f64 / 1_048_576., e.spatial_index_memory_bytes as f64 / 1_048_576., e.local_cpa_memory_reserve_bytes as f64 / 1_048_576.));
                ui.colored_label(tone(ui, AMBER),format!("Budget upper bound before scanning: at most {} timing-pair attempts globally (at least {} work units each). Source scanning and index work reduce this ceiling; it is not a completion estimate.",e.maximum_timing_pairs_before_scan,e.minimum_work_units_per_timing_pair));
                if self.document.as_ref() == Some(&prepared.document) && prepared.settings == self.wide.settings {
                    cost_preview_ui(ui, e);
                } else {
                    ui.colored_label(tone(ui, AMBER), "Search settings changed. Estimate again before using these previous cost figures or running.");
                }
            }
            let ready = self.wide.prepared.as_ref().is_some_and(|p| self.document.as_ref() == Some(&p.document) && p.settings == self.wide.settings);
            if ui.add_enabled(ready, egui::Button::new("Run reviewed wide-area plan")).clicked() { self.prepare_wide(ui.ctx(), true); }
            muted(ui, "A budget stop or cancellation remains incomplete. Results keep every processed receiver and source identity on disk; there is no automatic retry or resume in this first wide-area engine. A separately bounded review snapshot (up to 8 MiB) is saved beside the run before computation.");
        });
    }

    fn prepare_wide(&mut self, ctx: &egui::Context, run: bool) {
        if self.busy() || self.exiting {
            return;
        }
        let (Some(case), Some(doc), Some(source)) =
            (&self.case, self.document.clone(), self.wide.source.clone())
        else {
            self.notice = Some((true, "Select indexed flight data and a case first".into()));
            return;
        };
        if !self.deleted_sources_ready(&doc) {
            self.notice = Some((
                true,
                "Restore the deleted original source before running this review".into(),
            ));
            return;
        }
        if doc.receiver_terrain.is_some() || doc.atmosphere.is_some() {
            self.notice = Some((true, "This wide-area method uses constant receiver height and sound speed. Keep your current terrain/atmospheric review and prepare a separate plain-height review explicitly.".into()));
            return;
        }
        let settings = self.wide.settings.clone();
        let manifest = case.manifest.clone();
        let request = match request(&doc, &source, &settings) {
            Ok(v) => v,
            Err(e) => {
                self.notice = Some((true, e));
                return;
            }
        };
        self.wide.cancel = Arc::new(AtomicBool::new(false));
        if !run {
            let cancel = self.wide.cancel.clone();
            let sender = self.sender.clone();
            let generation = self.generation;
            let run_id = self.wide.run_id.wrapping_add(1);
            let repaint = ctx.clone();
            self.start_wide_task(
                ctx,
                "Checking the entire indexed source and wide-area plan…",
                move || {
                    workspace::verify_with_cancel(&manifest, &doc, &cancel)?;
                    let mut last = Instant::now() - Duration::from_secs(1);
                    let estimate =
                        wide_area_pair::estimate_with_cancel(&request, &cancel, |progress| {
                            if last.elapsed() >= Duration::from_millis(150) {
                                let _ = sender.send(Reply {
                                    generation,
                                    lane: WorkerLane::SearchProgress,
                                    result: Ok(ResultValue::WideProgress(run_id, progress)),
                                });
                                repaint.request_repaint();
                                last = Instant::now();
                            }
                        })?;
                    workspace::verify_with_cancel(&manifest, &doc, &cancel)?;
                    wide_checkpoint(&cancel)?;
                    Ok(ResultValue::WideReady(Box::new(WideReply::Estimated(
                        Box::new(Prepared {
                            request,
                            estimate,
                            document: doc,
                            settings,
                        }),
                    ))))
                },
            );
            return;
        }
        if self.wide.prepared.as_ref().is_none_or(|p| {
            p.document != doc || p.settings != settings || p.request.source != source.binding
        }) {
            self.notice = Some((true, "Estimate the current plan before running".into()));
            return;
        }
        let cancel = self.wide.cancel.clone();
        let sender = self.sender.clone();
        let generation = self.generation;
        let run_id = self.wide.run_id.wrapping_add(1);
        let repaint = ctx.clone();
        self.start_wide_task(
            ctx,
            "Searching full-resolution indexed trajectories…",
            move || {
                workspace::verify_with_cancel(&manifest, &doc, &cancel)?;
                let root = manifest
                    .parent()
                    .ok_or("Case has no directory")?
                    .join("wide-searches");
                ordinary_result_parents(manifest.parent().ok_or("Case has no directory")?)?;
                if !root.try_exists().map_err(|e| e.to_string())? {
                    std::fs::create_dir(&root).map_err(|e| e.to_string())?;
                }
                ordinary_result_parents(&root)?;
                let name = format!(
                    "run-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|e| e.to_string())?
                        .as_nanos()
                );
                let output = root.join(&name);
                save_review_snapshot(&output, &request, &doc)?;
                let mut last = Instant::now() - Duration::from_secs(1);
                let report = wide_area_pair::run(&request, &output, &cancel, |progress| {
                    if last.elapsed() >= Duration::from_millis(150) || progress.stage == "finished"
                    {
                        let _ = sender.send(Reply {
                            generation,
                            lane: WorkerLane::SearchProgress,
                            result: Ok(ResultValue::WideProgress(run_id, progress)),
                        });
                        repaint.request_repaint();
                        last = Instant::now();
                    }
                })
                .map_err(|error| {
                    format!(
                        "{error}; preserved wide-area run: {}",
                        display_path(&output)
                    )
                })?;
                Ok(ResultValue::WideReady(Box::new(WideReply::Report(
                    Box::new(report),
                    Some(Box::new(doc)),
                ))))
            },
        );
    }

    pub(super) fn wide_results_ui(&mut self, ui: &mut egui::Ui) -> bool {
        if self.wide.report.is_some() {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.wide.show_results, true, "Wide-area results");
                ui.selectable_value(&mut self.wide.show_results, false, "Other results");
            });
        }
        if ui
            .add_enabled(!self.busy(), egui::Button::new("Open saved wide-area run…"))
            .clicked()
            && let Some(path) = rfd::FileDialog::new().pick_folder()
        {
            self.wide.cancel = Arc::new(AtomicBool::new(false));
            let cancel = self.wide.cancel.clone();
            self.start_wide_task(
                ui.ctx(),
                "Opening the saved wide-area result…",
                move || {
                    wide_checkpoint(&cancel)?;
                    let reply = load_saved_wide(&path)?;
                    wide_checkpoint(&cancel)?;
                    Ok(ResultValue::WideReady(Box::new(reply)))
                },
            );
        }
        if !self.wide.show_results {
            return false;
        }
        let Some(report) = self.wide.report.clone() else {
            return false;
        };
        ui.heading("Wide-area two-pass results");
        if self.wide.report_source_missing {
            ui.colored_label(tone(ui, AMBER), "Historical result: its indexed source was deleted or became unavailable. Scores are retained; source verification and refinement are unavailable.");
        }
        ui.label(format!(
            "{:?} · {} / {} receivers · {} remaining",
            report.status,
            report.progress.completed_receivers,
            report.progress.total_receivers,
            report
                .progress
                .total_receivers
                .saturating_sub(report.progress.completed_receivers)
        ));
        ui.label(format!("Saved: {}", display_path(&report.output_directory)));
        if let Some(error) = &report.error {
            ui.colored_label(tone(ui, AMBER), error);
        }
        if let Some(guidance) = stop_guidance(&report) {
            if report.progress.completed_receivers == 0 {
                ui.colored_label(tone(ui, AMBER), "No receiver finished. This is not a no-match result; unfinished receiver scores are excluded from the completed results.");
                if report.progress.scored_candidates > 0 {
                    ui.label(format!("{} provisional pair scores were attempted before stopping; no completed receiver ranking is available.", report.progress.scored_candidates));
                }
            }
            ui.label(guidance);
        }
        muted(
            ui,
            "Partial runs show provisional alternatives, not a finished winner. Frequency RMSE is uncalibrated; source coverage and location are not verified. Map/display sampling never supplies these scores.",
        );
        if !report.source_verified_at_end {
            ui.colored_label(tone(ui, AMBER), "The source did not pass final verification; treat this result as provisional and inspect the saved limitations.");
        }
        if report.retained_truncated || report.pair_retention_truncated {
            muted(
                ui,
                "The compact table is truncated; the run folder retains the processed-receiver records. These rows do not establish uniqueness.",
            );
        }
        egui::CollapsingHeader::new(format!("Best retained flight pairs ({})", report.best_flight_pairs.len())).show(ui, |ui| {
            if report.best_flight_pairs.len() > 100 { muted(ui, "Showing the first 100 retained pairs; the saved report contains the complete retained list."); }
            for candidate in report.best_flight_pairs.iter().take(100) { ui.label(format!("{} + {} · {:.4} Hz · {:.5}, {:.5}", candidate.score.flight_ids[0], candidate.score.flight_ids[1], candidate.score.combined_rmse_hz, candidate.observer.latitude_deg, candidate.observer.longitude_deg)); }
        });
        ui.horizontal_wrapped(|ui| {
            for (column, name) in ["RMSE", "Latitude", "Longitude", "Clock"]
                .into_iter()
                .enumerate()
            {
                if ui.button(name).clicked() {
                    self.wide.sort_reverse =
                        self.wide.sort_column == column && !self.wide.sort_reverse;
                    self.wide.sort_column = column;
                    let value = |c: &WideCandidate| match column {
                        1 => c.observer.latitude_deg,
                        2 => c.observer.longitude_deg,
                        3 => c.score.clock_offset_s,
                        _ => c.score.combined_rmse_hz,
                    };
                    self.wide.row_order.sort_by(|a, b| {
                        value(&report.best_hypotheses[*a])
                            .total_cmp(&value(&report.best_hypotheses[*b]))
                    });
                    if self.wide.sort_reverse {
                        self.wide.row_order.reverse();
                    }
                }
            }
        });
        let mut refine = None;
        let row_height = wide_result_row_height(ui);
        egui::ScrollArea::vertical()
            .id_salt("wide_result_rows")
            .show_rows(ui, row_height, self.wide.row_order.len(), |ui, rows| {
                for row in rows {
                    let candidate = &report.best_hypotheses[self.wide.row_order[row]];
                    let summary = format!(
                        "{:.4} Hz · {:.6}, {:.6} · {} · {} + {}",
                        candidate.score.combined_rmse_hz,
                        candidate.observer.latitude_deg,
                        candidate.observer.longitude_deg,
                        report
                            .source_time_origin_utc_s
                            .map(|origin| regional::utc_label(
                                origin + candidate.score.clock_offset_s
                            ))
                            .unwrap_or_else(
                                || "UTC unavailable: source origin was not verified".into()
                            ),
                        candidate.score.flight_ids[0],
                        candidate.score.flight_ids[1],
                    );
                    let can_refine = !self.busy()
                        && !self.wide.report_source_missing
                        && report.source_verified_at_end
                        && self.wide.report_document.is_some()
                        && self.wide.report_document.as_ref() == self.document.as_ref()
                        && self
                            .wide
                            .source
                            .as_ref()
                            .is_some_and(|source| source.binding == report.source);
                    let id = ui.make_persistent_id((
                        "wide_result",
                        candidate.receiver_index,
                        candidate.encounter_index,
                    ));
                    let (button, _) = wide_result_row(ui, id, &summary, can_refine);
                    if button.clicked() {
                        refine = Some(candidate.observer.clone());
                    }
                }
            });
        if let Some(observer) = refine
            && let Some(before) = self.document.clone()
        {
            let dlat = 1000. / 111_320.;
            let dlon = dlat / observer.latitude_deg.to_radians().cos().abs().max(0.01);
            let doc = self.document.as_mut().unwrap();
            doc.map.bounds = [
                observer.latitude_deg - dlat,
                observer.longitude_deg - dlon,
                observer.latitude_deg + dlat,
                observer.longitude_deg + dlon,
            ];
            doc.map.grid_spacing_m = 100.;
            self.checkpoint(before);
            self.wide.prepared = None;
            self.wide.enabled = true;
            self.notice = Some((false, "Prepared a 1 km half-width, 100 m local grid. Review it in Search and estimate again. The full source pool and event approvals were preserved.".into()));
        }
        true
    }
}
