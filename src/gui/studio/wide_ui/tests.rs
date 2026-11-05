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
        let mut event = EvidenceEvent::manual(id.into(), EventKind::Doppler, start, start + 10.);
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
        maximum_timing_pairs_before_scan: request.budgets.work_limit_enabled.then_some(1),
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
fn gui_default_disables_only_work_limit_and_legacy_requests_keep_the_cap() {
    let mut studio = studio();
    let mut settings = reviewed(&mut studio);
    let before = studio.document.clone().unwrap();
    let source = source();
    let native = WideBudgets::default();
    assert!(native.work_limit_enabled);
    assert!(!settings.work_limit_enabled);
    let plan = request(&before, &source, &settings).unwrap();
    assert!(!plan.budgets.work_limit_enabled);
    assert_eq!(plan.budgets.max_work_units, native.max_work_units);
    assert_eq!(plan.budgets.max_duration_s, native.max_duration_s);
    assert_eq!(plan.budgets.max_output_bytes, native.max_output_bytes);
    assert_eq!(
        plan.budgets.max_index_memory_bytes,
        native.max_index_memory_bytes
    );
    assert!(plan.budgets.max_duration_s > 0);
    let mut uncapped = serde_json::to_value(&plan).unwrap();
    assert_eq!(uncapped["budgets"]["work_limit_enabled"], false);
    settings.work_limit_enabled = true;
    let legacy = request(&before, &source, &settings).unwrap();
    let encoded = serde_json::to_string(&legacy).unwrap();
    let restored: WidePairRequest = serde_json::from_str(&encoded).unwrap();
    assert!(restored.budgets.work_limit_enabled);
    assert_eq!(serde_json::to_string(&restored).unwrap(), encoded);
    assert_eq!(
        wide_area_pair::request_sha256(&restored).unwrap(),
        wide_area_pair::request_sha256(&legacy).unwrap()
    );
    uncapped["budgets"]
        .as_object_mut()
        .unwrap()
        .remove("work_limit_enabled");
    assert_eq!(serde_json::to_value(&legacy).unwrap(), uncapped);
    assert_ne!(
        wide_area_pair::request_sha256(&legacy).unwrap(),
        wide_area_pair::request_sha256(&plan).unwrap()
    );
    assert_eq!(studio.document.as_ref(), Some(&before));
    assert!(studio.undo.is_empty());
}
#[test]
fn work_limit_toggle_preserves_budget_and_invalidates_previous_estimate() {
    let mut studio = studio();
    studio.wide.settings = reviewed(&mut studio);
    studio.wide.settings.max_work = 7654321;
    let source = source();
    studio.wide.source = Some(source.clone());
    let before = studio.document.clone().unwrap();
    let original = studio.wide.settings.clone();
    let plan = request(&before, &source, &original).unwrap();
    let ctx = egui::Context::default();
    let mut position = egui::Pos2::ZERO;
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        position = work_limit_ui(ui, &mut studio.wide.settings).rect.center();
    });
    for enabled in [true, false] {
        for pressed in [true, false] {
            let _ = ctx.run_ui(
                egui::RawInput {
                    events: vec![
                        egui::Event::PointerMoved(position),
                        egui::Event::PointerButton {
                            pos: position,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    work_limit_ui(ui, &mut studio.wide.settings);
                },
            );
        }
        assert_eq!(studio.wide.settings.work_limit_enabled, enabled);
        assert_eq!(studio.wide.settings.max_work, original.max_work);
        if enabled {
            studio.accept_wide_reply(WideReply::Estimated(Box::new(Prepared {
                estimate: estimate_fixture(&plan, &source),
                request: plan.clone(),
                document: before.clone(),
                settings: original.clone(),
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
            let mut actual =
                serde_json::to_value(request(&before, &source, &studio.wide.settings).unwrap())
                    .unwrap();
            actual["budgets"]["work_limit_enabled"] = false.into();
            assert_eq!(actual, serde_json::to_value(&plan).unwrap());
        }
    }
    assert_eq!(studio.wide.settings, original);
    assert_eq!(studio.document.as_ref(), Some(&before));
    assert_eq!(studio.wide.source.as_ref().unwrap().binding, source.binding);
    assert!(studio.undo.is_empty());
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
    settings.work_limit_enabled = true;
    settings.max_work = 123456;
    let before = studio.document.clone().unwrap();
    let original = settings.clone();
    copy_reviewed_pair(&before, &mut settings).unwrap();
    assert_eq!(settings.speed, original.speed);
    assert_eq!(settings.distance, original.distance);
    assert_eq!(settings.work_limit_enabled, original.work_limit_enabled);
    assert_eq!(settings.max_work, original.max_work);
    assert_eq!(
        settings.cpa_s,
        [before.events[0].arrival_s, before.events[1].arrival_s]
    );
    assert_eq!(settings.tones[0], [118. * 0.9, 120. * 1.1, 1.]);
    let mut replacement = before.clone();
    replacement.events[0].id = "replacement-reviewed-event".into();
    settings.work_limit_enabled = false;
    copy_reviewed_pair(&replacement, &mut settings).unwrap();
    assert_eq!(settings.event_ids[0], "replacement-reviewed-event");
    assert_eq!(settings.speed, original.speed);
    assert_eq!(settings.distance, original.distance);
    assert!(!settings.work_limit_enabled);
    assert_eq!(settings.max_work, original.max_work);
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
        settings.work_limit_enabled = legacy;
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
        let sidecar: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.with_extension("review.json")).unwrap())
                .unwrap();
        assert_eq!(sidecar["schema_version"], 1);
        assert_eq!(
            sidecar.as_object().unwrap().len(),
            3,
            "Saved review format must remain unchanged"
        );
        let report = wide_area_pair::run(&plan, &output, &AtomicBool::new(true), |_| {}).unwrap();
        assert_eq!(report.status, wide_area_pair::WideStatus::Cancelled);
        assert_eq!(report.progress.completed_receivers, 0);
        let saved = std::fs::read(output.join("request.json")).unwrap();
        let saved_json: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        assert_eq!(
            saved_json["budgets"].get("work_limit_enabled"),
            if legacy {
                None
            } else {
                Some(&serde_json::Value::Bool(false))
            }
        );
        studio.wide.settings.speed = [[1., 2.], [3., 4.]];
        studio.wide.settings.distance = [[5., 6.], [7., 8.]];
        studio.wide.settings.work_limit_enabled = !legacy;
        let draft = studio.wide.settings.clone();
        studio.accept_wide_reply(load_saved_wide(&output).unwrap());
        assert_eq!(
            studio.wide.report.as_ref().unwrap().request_sha256,
            wide_area_pair::request_sha256(&plan).unwrap()
        );
        assert_eq!(studio.wide.report_document.as_ref(), Some(&document));
        assert_eq!(
            serde_json::to_value(studio.wide.report_budgets.as_ref().unwrap()).unwrap(),
            serde_json::to_value(&plan.budgets).unwrap()
        );
        assert_eq!(studio.document.as_ref(), Some(&document));
        assert_eq!(
            studio.wide.settings, draft,
            "Opening a result must not replace active choices"
        );
        assert!(studio.wide.source.is_none() && studio.wide.prepared.is_none());
        assert_eq!(std::fs::read(output.join("request.json")).unwrap(), saved);
        let reopened: WidePairRequest = serde_json::from_slice(&saved).unwrap();
        assert_eq!(reopened.budgets.work_limit_enabled, legacy);
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
fn issue_one_output_reservation_is_actionable_with_work_limit_disabled() {
    let mut settings = Settings {
        work_limit_enabled: false,
        max_work: 0,
        max_output: gib_to_bytes(1.598).unwrap(),
        ..Settings::default()
    };
    let before = settings.clone();
    let locations = 18_387_820;
    let required = wide_area_pair::preliminary_output_bytes(locations).unwrap();
    let text = painted_text(|ui| output_allowance_ui(ui, &mut settings, Some(locations)));
    for expected in [
        "Output allowance (GiB)",
        "Output allowance is too small",
        "1.660 GiB",
        "1.598 GiB",
        "Increase Output allowance",
        "grid are unchanged",
        "not a forecast",
        "Actual saved results may need more space",
    ] {
        assert!(text.contains(expected), "Missing {expected}: {text}");
    }
    assert!(!text.contains("work/output") && !text.contains("work-unit allowance is too small"));
    assert!(required > settings.max_output);
    assert_eq!(settings, before);
    settings.max_output = 8 * 1024 * 1024 * 1024;
    let changed = settings.clone();
    let text = painted_text(|ui| output_allowance_ui(ui, &mut settings, Some(locations)));
    assert!(!text.contains("is too small"));
    assert!(text.contains("8.000 GiB") && text.contains("does not guarantee completion"));
    assert_eq!(settings, changed);
}

#[test]
fn byte_close_gib_comparison_and_idle_controls_preserve_exact_budget_fields() {
    let locations = 18_387_820;
    let required = wide_area_pair::preliminary_output_bytes(locations).unwrap();
    let mut settings = Settings {
        max_output: required - 1,
        ..Settings::default()
    };
    let (required_text, allowed_text) = format_gib_pair(required, settings.max_output);
    assert_ne!(required_text, allowed_text);
    let before = settings.clone();
    let text = painted_text(|ui| output_allowance_ui(ui, &mut settings, Some(locations)));
    assert!(
        text.contains(&required_text)
            && text.contains(&allowed_text)
            && text.contains("is too small")
    );
    assert_eq!(settings, before);

    let mut studio = studio();
    studio.wide.settings = reviewed(&mut studio);
    studio.wide.settings.max_output += 1;
    studio.wide.settings.max_index_memory_bytes += 1;
    studio.wide.source = Some(source());
    studio.wide.section = PlanSection::Resources;
    let document = studio.document.clone();
    let settings = studio.wide.settings.clone();
    let text = painted_text(|ui| studio.wide_controls_ui(ui));
    for expected in [
        "Output allowance (GiB)",
        "Source/index memory allowance (GiB)",
        "0.250 GiB",
        "Retained ranking rows",
        "does not reduce the locations",
        "runtime limit can stop",
        "no automatic retry or resume",
    ] {
        assert!(text.contains(expected), "Missing {expected}: {text}");
    }
    assert!(!text.contains("MiB") && !text.contains("output bytes"));
    assert_eq!(studio.wide.settings, settings);
    assert_eq!(studio.document, document);
    assert!(studio.undo.is_empty() && !studio.busy() && studio.receiver.try_recv().is_err());
}

#[test]
fn section_navigation_wraps_when_narrow_and_never_changes_search_inputs() {
    for (width, scale) in [(240., 1.), (320., 1.5)] {
        let mut studio = studio();
        studio.wide.settings = reviewed(&mut studio);
        let settings = studio.wide.settings.clone();
        let document = studio.document.clone();
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| {
            for font in style.text_styles.values_mut() {
                font.size *= scale;
            }
        });
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 500.));
        let mut bounds = Vec::new();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                bounds = plan_section_tabs(ui, &mut studio.wide.section)
                    .into_iter()
                    .map(|r| r.rect)
                    .collect();
            },
        );
        assert_eq!(bounds.len(), 4);
        assert!(
            bounds
                .iter()
                .all(|rect| rect.left() >= screen.left() && rect.right() <= screen.right() + 0.5)
        );
        assert!(bounds[3].top() > bounds[0].top());
        for (index, section) in [
            PlanSection::Area,
            PlanSection::Evidence,
            PlanSection::Resources,
            PlanSection::Estimate,
        ]
        .into_iter()
        .enumerate()
        {
            let position = bounds[index].center();
            for pressed in [true, false] {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        events: vec![
                            egui::Event::PointerMoved(position),
                            egui::Event::PointerButton {
                                pos: position,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        ..Default::default()
                    },
                    |ui| {
                        plan_section_tabs(ui, &mut studio.wide.section);
                    },
                );
            }
            assert_eq!(studio.wide.section, section);
            assert_eq!(studio.wide.settings, settings);
            assert_eq!(studio.document, document);
        }
        assert!(studio.undo.is_empty() && !studio.busy());
    }
}

#[test]
fn disabled_run_explains_changed_settings_and_keeps_old_estimate_separate() {
    let mut studio = studio();
    studio.wide.settings = reviewed(&mut studio);
    let source = source();
    let document = studio.document.clone().unwrap();
    let request = request(&document, &source, &studio.wide.settings).unwrap();
    let estimate = estimate_fixture(&request, &source);
    studio.wide.source = Some(source);
    studio.wide.prepared = Some(Box::new(Prepared {
        request,
        estimate,
        document: document.clone(),
        settings: studio.wide.settings.clone(),
    }));
    assert!(run_disabled_reason(&studio.wide, Some(&document), false).is_none());
    studio.wide.settings.max_output += 1;
    studio.wide.section = PlanSection::Estimate;
    let settings = studio.wide.settings.clone();
    assert_eq!(
        run_disabled_reason(&studio.wide, Some(&document), false),
        Some("Settings changed — estimate again.")
    );
    let text = painted_text(|ui| studio.wide_controls_ui(ui));
    for expected in [
        "Area/grid",
        "Evidence/assumptions",
        "Resource allowances",
        "Estimate/run",
        "Previous estimate",
        "Settings changed — estimate again",
        "Run reviewed wide-area plan",
        "Review Resource allowances",
    ] {
        assert!(text.contains(expected), "Missing {expected}: {text}");
    }
    assert_eq!(studio.wide.settings, settings);
    assert_eq!(studio.document.as_ref(), Some(&document));
    assert!(studio.wide.prepared.is_some() && studio.undo.is_empty() && !studio.busy());
}

#[test]
fn preliminary_output_refusal_opens_allowances_without_loading_data_or_changing_grid() {
    let mut studio = studio();
    studio.wide.settings = reviewed(&mut studio);
    studio.wide.settings.work_limit_enabled = false;
    studio.wide.settings.max_work = 0;
    studio.wide.settings.max_output = 16 * 1024 * 1024;
    studio.wide.source = Some(source());
    studio.document.as_mut().unwrap().map.grid_spacing_m = 1000.;
    let document = studio.document.clone();
    let settings = studio.wide.settings.clone();
    studio.prepare_wide(&egui::Context::default(), false);
    assert_eq!(studio.wide.section, PlanSection::Resources);
    let (error, message) = studio.notice.as_ref().unwrap();
    assert!(*error && message.contains("Output allowance") && message.contains("GiB"));
    assert!(!message.contains("work/output") && !message.contains("work-unit allowance"));
    assert_eq!(studio.document, document);
    assert_eq!(studio.wide.settings, settings);
    assert!(studio.wide.prepared.is_none() && !studio.busy() && !studio.wide.running);
    assert!(studio.receiver.try_recv().is_err() && studio.undo.is_empty());
}

#[test]
fn saved_combined_limit_uses_verified_saved_allowances_without_rewriting_report() {
    let fixture = SavedRunFixture::new();
    let mut studio = studio();
    studio.wide.settings = reviewed(&mut studio);
    let document = studio.document.clone().unwrap();
    let plan = request(&document, &source(), &studio.wide.settings).unwrap();
    let mut report = wide_area_pair::run(
        &plan,
        &fixture.0.join("legacy-diagnostic"),
        &AtomicBool::new(true),
        |_| {},
    )
    .unwrap();
    report.status = wide_area_pair::WideStatus::BudgetExceeded;
    report.progress.total_receivers = 18_387_820;
    report.error = Some("The full declared grid cannot fit the explicit minimum work/output budget; no receivers were removed".into());
    let saved_report = serde_json::to_value(&report).unwrap();
    let mut saved_budgets = plan.budgets.clone();
    saved_budgets.work_limit_enabled = false;
    saved_budgets.max_work_units = 0;
    saved_budgets.max_output_bytes = gib_to_bytes(1.598).unwrap();
    studio.wide.settings.max_output = 8 * 1024 * 1024 * 1024;
    let active = studio.wide.settings.clone();
    studio.accept_wide_reply(WideReply::ReopenedReport(
        Box::new(report),
        None,
        Some(saved_budgets.clone()),
    ));
    let text = painted_text(|ui| {
        studio.wide_results_ui(ui);
    });
    assert!(
        text.contains("Output allowance")
            && text.contains("1.660 GiB")
            && text.contains("1.598 GiB"),
        "{text}"
    );
    assert!(text.contains("Copy original diagnostic"));
    assert!(
        !text.contains("minimum work/output budget") && !text.contains("Enabled work-unit limit")
    );
    assert_eq!(
        serde_json::to_value(studio.wide.report.as_ref().unwrap().as_ref()).unwrap(),
        saved_report
    );
    assert_eq!(studio.wide.settings, active);
    assert_eq!(studio.document, Some(document));
    assert!(studio.undo.is_empty());
    saved_budgets.work_limit_enabled = true;
    let separate = saved_diagnostic(
        saved_report["error"].as_str().unwrap(),
        18_387_820,
        Some(&saved_budgets),
    );
    assert!(
        separate.contains("Output allowance") && separate.contains("Enabled work-unit limit"),
        "{separate}"
    );
}

#[test]
fn saved_memory_diagnostic_shows_gib_and_copies_exact_original_only_on_click() {
    let errors = [
        "Combined source/spatial-index/local-CPA memory needs 1024 bytes, exceeding the declared 512 bytes; no source pass or receiver was removed",
        "Track-store index memory budget exceeded: need at least 1024 bytes; declared 512",
        "One continuous pass exceeds the 256 MiB track-cache bound; it was not truncated",
    ];
    for error in errors {
        let original = error.to_owned();
        let text = painted_text(|ui| {
            saved_diagnostic_ui(ui, error, 50, None);
        });
        assert!(text.contains("GiB") && text.contains("Copy original diagnostic"));
        assert!(
            !text.contains("MiB") && !text.contains("1024 bytes") && !text.contains("512 bytes")
        );
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760., 400.));
        let mut rect = egui::Rect::NOTHING;
        let initial = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                rect = saved_diagnostic_ui(ui, error, 50, None).rect;
            },
        );
        assert!(
            !initial
                .platform_output
                .commands
                .iter()
                .any(|command| matches!(command, egui::OutputCommand::CopyText(_)))
        );
        for pressed in [true, false] {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![
                        egui::Event::PointerMoved(rect.center()),
                        egui::Event::PointerButton {
                            pos: rect.center(),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    saved_diagnostic_ui(ui, error, 50, None);
                },
            );
            if !pressed {
                assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(value) if value == &original)));
            }
        }
        assert_eq!(error, original);
    }
    let current = "Combined source/spatial-index/local-CPA memory needs 0.600 GiB, exceeding the declared 0.500 GiB; increase the source/index memory allowance and estimate again. No source pass or receiver was removed";
    assert_eq!(saved_diagnostic(current, 50, None), current);
    let path_error = "Source missing: C:/case/1024 bytes/example-MiB.raw";
    assert_eq!(saved_diagnostic(path_error, 50, None), path_error);
}

#[test]
fn busy_wide_page_stays_quiet_without_input_or_worker_updates() {
    let mut studio = studio();
    studio.wide.settings = reviewed(&mut studio);
    studio.busy = Some("Checking indexed flight data".into());
    studio.wide.running = true;
    studio.wide.started = Some(Instant::now());
    studio.wide.progress = Some(WideProgress {
        stage: "verifying_source".into(),
        total_receivers: 50,
        ..Default::default()
    });
    let document = studio.document.clone();
    let settings = studio.wide.settings.clone();
    let progress = serde_json::to_value(&studio.wide.progress).unwrap();
    let ctx = egui::Context::default();
    for frame in 0..16 {
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000., 1600.),
                )),
                time: Some(frame as f64 * 0.25),
                ..Default::default()
            },
            |ui| {
                studio.toolbar(ui);
                studio.wide_controls_ui(ui);
                studio.wide_progress_ui(ui);
            },
        );
        if frame >= 8 {
            assert!(
                output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
                    >= Duration::from_millis(100),
                "unexpected busy repaint at frame {frame}"
            );
        }
    }
    assert_eq!(studio.document, document);
    assert_eq!(studio.wide.settings, settings);
    assert_eq!(
        serde_json::to_value(&studio.wide.progress).unwrap(),
        progress
    );
    assert!(!studio.wide.cancel.load(Ordering::Relaxed));
    assert!(studio.undo.is_empty() && studio.wide.source.is_none());
}
#[test]
fn wide_progress_distinguishes_pair_work_from_finished_locations() {
    let mut studio = studio();
    studio.wide.settings = reviewed(&mut studio);
    let document = studio.document.clone();
    let settings = studio.wide.settings.clone();
    let undo_count = studio.undo.len();
    studio.wide.running = true;
    studio.wide.progress = Some(WideProgress {
        stage: "shared_clock_pair_scoring".into(),
        total_receivers: 50,
        completed_receivers: 0,
        scanned_passes: 1234,
        timing_candidates: 1248,
        scored_candidates: 765,
        score_bound_pruned_candidates: 123,
        ..Default::default()
    });
    let before = serde_json::to_value(&studio.wide.progress).unwrap();
    let text = painted_text(|ui| studio.wide_progress_ui(ui));
    for expected in [
        "Comparing Doppler pairs",
        "Finished locations: 0 / 50",
        "50 remaining",
        "Timing-compatible pairs tried: 1248",
        "Valid pair scores: 765",
        "Pairs excluded by score bound: 123",
        "Flight passes examined: 1234",
        "unfinished location",
        "only after all its eligible pairs are examined",
    ] {
        assert!(text.contains(expected), "Missing {expected}: {text}");
    }
    for absent in [
        "shared_clock_pair_scoring",
        "matches found",
        "ETA",
        "%",
        "Location 1",
    ] {
        assert!(!text.contains(absent), "Unexpected {absent}: {text}");
    }
    assert_eq!(serde_json::to_value(&studio.wide.progress).unwrap(), before);
    assert_eq!(studio.document, document);
    assert_eq!(studio.wide.settings, settings);
    assert_eq!(studio.undo.len(), undo_count);
    assert!(studio.wide.running && !studio.wide.cancel.load(Ordering::Relaxed));
}
#[test]
fn wide_progress_shows_source_verification_without_claiming_search_completion() {
    let mut studio = studio();
    studio.wide.settings = reviewed(&mut studio);
    let document = studio.document.clone();
    let settings = studio.wide.settings.clone();
    studio.wide.running = true;
    for (stage, label, completed) in [
        ("verifying_source", "Verifying flight data", 0),
        ("preparing_index", "Preparing the location search", 0),
        ("verifying_source_final", "Checking source files again", 50),
    ] {
        studio.wide.progress = Some(WideProgress {
            stage: stage.into(),
            total_receivers: 50,
            completed_receivers: completed,
            ..Default::default()
        });
        let before = serde_json::to_value(&studio.wide.progress).unwrap();
        let text = painted_text(|ui| studio.wide_progress_ui(ui));
        assert!(text.contains(label), "Missing {label}: {text}");
        assert!(text.contains(&format!("Finished locations: {completed} / 50")));
        assert!(text.contains("Timing-compatible pairs tried: 0"));
        assert!(text.contains("Valid pair scores: 0"));
        for absent in [
            stage,
            "Comparing Doppler pairs",
            "Search complete",
            "matches found",
            "ETA",
            "%",
            "Location 1",
        ] {
            assert!(!text.contains(absent), "Unexpected {absent}: {text}");
        }
        assert_eq!(serde_json::to_value(&studio.wide.progress).unwrap(), before);
        assert_eq!(studio.document, document);
        assert_eq!(studio.wide.settings, settings);
        assert!(studio.undo.is_empty());
        assert!(studio.wide.running && !studio.wide.cancel.load(Ordering::Relaxed));
    }
}
#[test]
fn wide_progress_displays_known_pair_total_without_finishing_the_location() {
    let mut studio = studio();
    studio.wide.settings = reviewed(&mut studio);
    let document = studio.document.clone();
    let settings = studio.wide.settings.clone();
    studio.wide.running = true;
    for (total, attempted) in [(None, 0), (Some(2000), 1248), (Some(1248), 1248)] {
        studio.wide.progress = Some(WideProgress {
            stage: if total.is_none() {
                "counting_timing_pairs"
            } else {
                "shared_clock_pair_scoring"
            }
            .into(),
            total_receivers: 50,
            completed_receivers: 0,
            timing_candidates: attempted,
            active_receiver: Some(wide_area_pair::WideActiveReceiverProgress {
                receiver_index: 0,
                eligible_pair_total: total,
                attempted_pairs: attempted,
            }),
            ..Default::default()
        });
        let before = serde_json::to_value(&studio.wide.progress).unwrap();
        let text = painted_text(|ui| studio.wide_progress_ui(ui));
        assert!(text.contains("Finished locations: 0 / 50"));
        assert!(text.contains("Current location: 1 / 50"));
        if let Some(total) = total {
            assert!(text.contains(&format!(
                "Pairs tried at this location: {attempted} / {total}"
            )));
            assert!(text.contains("unfinished location"));
        } else {
            assert!(text.contains("Counting timing-compatible pairs"));
            assert!(text.contains("Finding and counting flight pairs at this location"));
            assert!(!text.contains("Pairs tried at this location:"));
        }
        for absent in ["Finished locations: 1 /", "Search complete", "ETA", "%"] {
            assert!(!text.contains(absent), "Unexpected {absent}: {text}");
        }
        assert_eq!(serde_json::to_value(&studio.wide.progress).unwrap(), before);
        assert_eq!(studio.document, document);
        assert_eq!(studio.wide.settings, settings);
        assert!(studio.undo.is_empty());
        assert!(studio.wide.running && !studio.wide.cancel.load(Ordering::Relaxed));
    }
}
#[test]
fn cost_preview_shows_declared_counts_and_necessary_only_budget_warnings() {
    let mut studio = studio();
    let mut settings = reviewed(&mut studio);
    settings.work_limit_enabled = true;
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
        maximum_timing_pairs_after_minimum_grid_work: Some(0),
        can_afford_minimum_first_receiver: Some(false),
        can_afford_minimum_first_pair: Some(false),
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
    let details = painted_text(|ui| cost_details_ui(ui, estimate.cost_preview.as_ref().unwrap()));
    assert!(details.contains("20207 trace-fit"));
    assert!(details.contains("not a completion or runtime estimate"));
    estimate
        .cost_preview
        .as_mut()
        .unwrap()
        .can_afford_minimum_first_receiver = Some(true);
    let pair = painted_text(|ui| cost_preview_ui(ui, &estimate));
    assert!(pair.contains("minimum first timing-pair charge"));
    estimate
        .cost_preview
        .as_mut()
        .unwrap()
        .can_afford_minimum_first_pair = Some(true);
    estimate
        .cost_preview
        .as_mut()
        .unwrap()
        .maximum_timing_pairs_after_minimum_grid_work = Some(3);
    let permitted = painted_text(|ui| cost_preview_ui(ui, &estimate));
    assert!(!permitted.contains("cannot cover"));
    assert!(permitted.contains("Detailed work breakdown"));
    assert_eq!(studio.document.as_ref(), Some(&document));
    assert!(studio.undo.is_empty());
}
#[test]
fn uncapped_cost_preview_preserves_counts_without_false_affordability_claims() {
    let mut studio = studio();
    let mut settings = reviewed(&mut studio);
    let before = studio.document.clone().unwrap();
    let source = source();
    let plan = request(&before, &source, &settings).unwrap();
    let mut estimate = estimate_fixture(&plan, &source);
    estimate.cost_preview = Some(wide_area_pair::WideCostPreview {
        event_trace_samples: [3, 5],
        event_tone_counts: [61, 126],
        shared_shift_count: 11,
        trace_fit_work_units_per_pair: 20207,
        minimum_track_work_units_per_pair: 320,
        retention_work_units_per_pair: 800,
        index_build_work_units: 600,
        minimum_grid_work_units: 700,
        maximum_timing_pairs_after_minimum_grid_work: None,
        can_afford_minimum_first_receiver: None,
        can_afford_minimum_first_pair: None,
    });
    let text = painted_text(|ui| {
        runtime_and_work_ui(ui, &mut settings);
        cost_preview_ui(ui, &estimate);
    });
    for expected in [
        "Stop after",
        "No work-unit limit; time/memory/output limits still apply",
        "Advanced work limit",
        "Event 1: 3 trace samples",
        "126 tones",
        "11 shared shifts",
    ] {
        assert!(text.contains(expected), "Missing {expected}: {text}");
    }
    for absent in [
        "Budget upper bound",
        "at most",
        "cannot cover",
        "minimum first timing-pair charge",
    ] {
        assert!(!text.contains(absent), "Unexpected {absent}: {text}");
    }
    let details = painted_text(|ui| cost_details_ui(ui, estimate.cost_preview.as_ref().unwrap()));
    assert!(details.contains("20207 trace-fit") && details.contains("700 units"));
    assert!(details.contains("not a completion or runtime estimate"));
    assert!(!details.contains("at most"));
    for stored in [0, u64::MAX] {
        settings.max_work = stored;
        let _ = painted_text(|ui| {
            work_limit_ui(ui, &mut settings);
        });
        assert_eq!(
            settings.max_work, stored,
            "A disabled limit must not normalize its stored value"
        );
        assert!(!settings.work_limit_enabled);
        assert_eq!(
            request(&before, &source, &settings)
                .unwrap()
                .budgets
                .max_work_units,
            stored
        );
    }
    assert_eq!(studio.document.as_ref(), Some(&before));
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
        Some(plan.budgets.clone()),
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
            "Output allowance (GiB)",
        ),
        (
            "Combined source/spatial-index/local-CPA memory needs 1024 bytes, exceeding the declared 512 bytes; no source pass or receiver was removed",
            "Source/index memory allowance (GiB)",
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
    studio.wide.settings.work_limit_enabled = true;
    studio.wide.settings.max_work = 123456;
    studio.wide.running = true;
    let cancel = studio.wide.cancel.clone();
    let mut case = studio.case.clone().unwrap();
    case.manifest = case.manifest.with_file_name("second-case.json");
    studio.bind(&case);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(studio.wide.source.is_none() && !studio.wide.running);
    assert_eq!(studio.wide.settings.speed, [[40., 330.]; 2]);
    assert_eq!(studio.wide.settings.distance, [[100., 100_000.]; 2]);
    assert!(!studio.wide.settings.work_limit_enabled);
    assert_eq!(
        studio.wide.settings.max_work,
        WideBudgets::default().max_work_units
    );
}
#[test]
fn wide_source_deletion_is_scoped_and_retains_historical_result() {
    let mut studio = studio();
    let source = source();
    studio.wide.source = Some(source.clone());
    let report = WidePairReport {
        scoring_implementation: String::new(),
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

fn reuse_fixture(studio: &mut Studio) -> (IndexReuseScope, data_catalog::IndexedReuseMatch) {
    let entry = data_catalog::CatalogEntry {
        schema_version: 1,
        id: "synthetic-archive".into(),
        case_manifest_sha256: "d".repeat(64),
        label: "Synthetic day archive".into(),
        kind: data_catalog::DataKind::Adsb,
        format: "adsblol_archive".into(),
        primary_file: "archive.json".into(),
        created_unix_s: 0,
        source: serde_json::Value::Null,
        files: vec![data_catalog::CatalogFile {
            relative_path: "archive.json".into(),
            bytes: 1,
            sha256: "e".repeat(64),
        }],
        date_utc: Some("2026-09-08".into()),
        bounds: None,
        notes: String::new(),
    };
    studio.data.select_entry(entry.id.clone());
    studio.data.items = vec![Arc::new(data_catalog::CatalogItem {
        entry,
        status: "available".into(),
        available_bytes: 1,
        problem: None,
        integrity_verified: false,
    })];
    let scope = studio
        .index_reuse_scope(&studio.data.items[0].entry)
        .unwrap();
    let matched = data_catalog::IndexedReuseMatch {
        catalog_id: "synthetic-index".into(),
        label: "Synthetic saved index".into(),
        index: source().binding,
        source_catalog_id: scope.source_catalog_id.clone(),
        acquisition_manifest_sha256: scope.acquisition_manifest_sha256.clone(),
        config_sha256: "f".repeat(64),
        catalog_entry_sha256: "a".repeat(64),
        registered_bytes: 1024,
    };
    (scope, matched)
}

#[test]
fn matching_index_lookup_is_tentative_and_idle_paint_never_loads_or_builds() {
    let mut studio = studio();
    let (scope, matched) = reuse_fixture(&mut studio);
    let before = studio.document.clone();
    studio.accept_wide_reply(WideReply::IndexMatches(Box::new(IndexReuseMatches {
        scope: scope.clone(),
        matches: vec![matched],
    })));
    let ctx = egui::Context::default();
    let output = ctx.run_ui(egui::RawInput::default(), |ui| {
        studio.index_matches_ui(ui, &scope)
    });
    fn labels(shape: &egui::epaint::Shape, out: &mut String) {
        match shape {
            egui::epaint::Shape::Text(text) => out.push_str(&text.galley.job.text),
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    labels(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut text = String::new();
    for shape in output.shapes {
        labels(&shape.shape, &mut text);
    }
    assert!(text.contains("not yet verified") && text.contains("Use existing index"));
    assert_eq!(studio.document, before);
    assert!(studio.wide.source.is_none() && studio.wide.prepared.is_none());
    assert!(studio.undo.is_empty() && !studio.busy() && !studio.wide.running);
    assert!(studio.receiver.try_recv().is_err());
}

#[test]
fn matching_index_scope_rejects_changed_import_settings_and_source_identity() {
    let mutations: [fn(&mut Studio); 10] = [
        |s| s.data.set_import_gap_for_test(31.),
        |s| s.data.set_import_speed_for_test(331.),
        |s| s.data.set_baro_import_for_test(0.),
        |s| s.document.as_mut().unwrap().map.bounds[0] += 0.1,
        |s| s.wide.import_start = "2026-09-08 12:00:00".into(),
        |s| s.wide.import_padding_km += 1.,
        |s| Arc::make_mut(&mut s.data.items[0]).entry.files[0].sha256 = "c".repeat(64),
        |s| Arc::make_mut(&mut s.data.items[0]).entry.date_utc = Some("2026-09-09".into()),
        |s| Arc::make_mut(&mut s.data.items[0]).status = "deleted".into(),
        |s| s.data.select_entry("another-archive".into()),
    ];
    for mutate in mutations {
        let mut studio = studio();
        let (scope, matched) = reuse_fixture(&mut studio);
        assert!(studio.index_reuse_scope_current(&scope));
        mutate(&mut studio);
        assert!(!studio.index_reuse_scope_current(&scope));
        studio.accept_wide_reply(WideReply::IndexMatches(Box::new(IndexReuseMatches {
            scope,
            matches: vec![matched],
        })));
        assert!(studio.wide.index_matches.is_none() && studio.wide.source.is_none());
    }
    let mut studio = studio();
    let (scope, _) = reuse_fixture(&mut studio);
    studio.wide.import_disk_gib *= 2.;
    studio.wide.settings.max_index_memory_bytes *= 2;
    assert!(
        studio.index_reuse_scope_current(&scope),
        "operational limits do not change exact import identity"
    );
    let (_, mut changed_source) = reuse_fixture(&mut studio);
    changed_source.acquisition_manifest_sha256 = "0".repeat(64);
    studio.accept_wide_reply(WideReply::IndexMatches(Box::new(IndexReuseMatches {
        scope,
        matches: vec![changed_source],
    })));
    assert!(
        studio.wide.index_matches.is_none(),
        "a disk catalog change during Find is not accepted under an old source identity"
    );
    studio.data.set_baro_import_for_test(-0.);
    let negative_zero = studio
        .index_reuse_scope(&studio.data.items[0].entry)
        .unwrap();
    studio.data.set_baro_import_for_test(0.);
    let positive_zero = studio
        .index_reuse_scope(&studio.data.items[0].entry)
        .unwrap();
    assert_eq!(negative_zero.config, positive_zero.config);
    assert_ne!(negative_zero, positive_zero);
    assert!(!studio.index_reuse_scope_current(&negative_zero));
    assert!(studio.index_reuse_scope_current(&positive_zero));
}

#[test]
fn verified_index_reuse_applies_only_to_unchanged_scope_and_review() {
    let fixture = SavedRunFixture::new();
    let index_path = fixture.0.join("synthetic-index.json");
    // This test starts at the verified worker reply. Registration still
    // requires an actual ordinary file, unlike metadata-only UI fixtures.
    std::fs::write(
        &index_path,
        b"{\"synthetic\":\"verified-reply registration fixture\"}\n",
    )
    .unwrap();
    let mut verified_source = source();
    verified_source.binding = TrackStoreBinding {
        manifest_path: index_path.canonicalize().unwrap(),
        manifest_sha256: geosonnar::detailed_data::sha256(&index_path).unwrap(),
    };
    for stale in [0, 1, 2] {
        let mut studio = studio();
        let (scope, _) = reuse_fixture(&mut studio);
        let previous = studio.document.clone().unwrap();
        if stale == 1 {
            studio.data.set_import_gap_for_test(31.);
        }
        if stale == 2 {
            studio.document.as_mut().unwrap().channel = 1;
        }
        let before = studio.document.clone().unwrap();
        studio.accept_wide_reply(WideReply::ReusedSource(Box::new(ReusedIndexSource {
            scope,
            source: verified_source.clone(),
            document: Box::new(previous),
        })));
        if stale == 0 {
            assert!(studio.wide.source.is_some(), "{:?}", studio.notice);
            assert!(
                studio
                    .document
                    .as_ref()
                    .unwrap()
                    .datasets
                    .iter()
                    .any(|binding| binding
                        == &SourceBinding {
                            path: verified_source.binding.manifest_path.clone(),
                            sha256: verified_source.binding.manifest_sha256.clone(),
                        })
            );
            assert_eq!(studio.document.as_ref().unwrap().events, before.events);
            assert_eq!(studio.undo.len(), 1);
        } else {
            assert_eq!(studio.document, Some(before));
            assert!(studio.wide.source.is_none() && studio.undo.is_empty());
        }
    }
}

#[test]
fn cancelled_and_stale_index_reuse_replies_preserve_current_source() {
    for reused in [false, true] {
        for stale in [0, 1, 2] {
            let mut studio = studio();
            let (scope, matched) = reuse_fixture(&mut studio);
            let before = studio.document.clone().unwrap();
            let existing = source();
            studio.wide.source = Some(existing.clone());
            studio.wide.run_id = 5;
            studio.wide.running = true;
            studio.busy = Some("Checking saved index".into());
            if stale == 0 {
                studio.wide.cancel.store(true, Ordering::Relaxed);
            }
            let reply = if reused {
                WideReply::ReusedSource(Box::new(ReusedIndexSource {
                    scope,
                    source: source(),
                    document: Box::new(before.clone()),
                }))
            } else {
                WideReply::IndexMatches(Box::new(IndexReuseMatches {
                    scope,
                    matches: vec![matched],
                }))
            };
            studio
                .sender
                .send(Reply {
                    generation: if stale == 2 {
                        studio.generation.wrapping_sub(1)
                    } else {
                        studio.generation
                    },
                    lane: WorkerLane::WideTask(if stale == 1 { 4 } else { 5 }),
                    result: Ok(ResultValue::WideReady(Box::new(reply))),
                })
                .unwrap();
            studio.tick(&egui::Context::default());
            assert_eq!(studio.document, Some(before));
            assert_eq!(
                studio.wide.source.as_ref().unwrap().binding,
                existing.binding
            );
            assert!(studio.wide.index_matches.is_none() && studio.undo.is_empty());
            assert_eq!(
                studio.busy(),
                stale != 0,
                "stale reply must not clear newer activity"
            );
        }
    }
}

#[test]
fn deleting_one_saved_index_removes_only_its_tentative_match() {
    let mut studio = studio();
    let (scope, first) = reuse_fixture(&mut studio);
    let mut second = first.clone();
    second.catalog_id = "other-index".into();
    second.index.manifest_path = second
        .index
        .manifest_path
        .with_file_name("other-index.json");
    let before = studio.document.clone();
    studio.wide.index_matches = Some(Box::new(IndexReuseMatches {
        scope,
        matches: vec![first.clone(), second.clone()],
    }));
    studio.invalidate_wide_source(|path| path == first.index.manifest_path);
    assert_eq!(
        studio.wide.index_matches.as_ref().unwrap().matches,
        vec![second]
    );
    assert_eq!(studio.document, before);
    assert!(studio.undo.is_empty());
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
        scoring_implementation: String::new(),
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
    studio.accept_wide_reply(WideReply::ReopenedReport(
        Box::new(incoming.clone()),
        None,
        None,
    ));
    assert_eq!(
        studio.wide.report.as_ref().unwrap().request_sha256,
        "c".repeat(64)
    );
    assert!(studio.pending_action.is_none());
    assert_eq!(studio.document, before);
    assert!(studio.notice.as_ref().unwrap().1.contains("cancelled"));
    studio.accept_wide_reply(WideReply::Report(Box::new(incoming), None, None));
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
