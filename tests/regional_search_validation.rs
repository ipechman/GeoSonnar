//! Independent constant-velocity reception-time oracle plus production replay.
//! These are fabricated numerical controls, not field localization evidence.
use geosonnar::{
    evidence_score::{
        LightningAltitudeDatum, LightningCatalog, LightningProvenance, LightningStrike,
        ObserverLocation,
    },
    map_data::{self, MapData},
    physics::{Track, TrackPoint},
    regional_search::{
        self, ClockRange, EventAlternatives, RegionalPlan, RegionalReport, SearchStatus,
    },
    workspace::{
        EventKind, EvidenceEvent, EvidenceMix, MapSelection, ReviewState, ReviewWorkspace,
        TracePoint,
    },
    workspace_scoring,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

const ORIGIN: f64 = 1_788_825_600.0;
const C: f64 = 343.0;
const TRUE_OFFSET: f64 = 4.0;
const TONE: f64 = 117.35;

fn xyz(latitude: f64, longitude: f64, height: f64) -> [f64; 3] {
    let latitude = latitude.to_radians();
    let longitude = longitude.to_radians();
    let e2 = 6.6943799901413165e-3;
    let n = 6378137.0 / (1.0 - e2 * latitude.sin().powi(2)).sqrt();
    [
        (n + height) * latitude.cos() * longitude.cos(),
        (n + height) * latitude.cos() * longitude.sin(),
        (n * (1.0 - e2) + height) * latitude.sin(),
    ]
}
fn analytic_factor(track: &Track, observer: &ObserverLocation, arrival: f64) -> f64 {
    let receiver = xyz(
        observer.latitude_deg,
        observer.longitude_deg,
        observer.ellipsoid_height_m,
    );
    let a = &track.points[0];
    let b = &track.points[1];
    let velocity: Vec<_> = (0..3)
        .map(|i| (b.xyz_m[i] - a.xyz_m[i]) / (b.t_s - a.t_s))
        .collect();
    let r: Vec<_> = (0..3)
        .map(|i| a.xyz_m[i] - velocity[i] * a.t_s - receiver[i])
        .collect();
    let speed2 = velocity.iter().map(|v| v * v).sum::<f64>();
    let tc = -r.iter().zip(&velocity).map(|(a, b)| a * b).sum::<f64>() / speed2;
    let distance2 = r
        .iter()
        .zip(&velocity)
        .map(|(r, v)| (r + v * tc).powi(2))
        .sum::<f64>();
    let tau = arrival - tc;
    let relative = (C * C * tau
        - (C * C * speed2 * tau * tau + (C * C - speed2) * distance2).sqrt())
        / (C * C - speed2);
    1.0 / (1.0 + speed2 * relative / (C * (distance2 + speed2 * relative * relative).sqrt()))
}
fn track(id: &str, tc: f64) -> Track {
    Track {
        flight_id: id.into(),
        // Include enough observed trajectory before the recording for every
        // declared clock alternative; the separate short-track control below
        // deliberately removes this support and checks the exact exclusions.
        points: (-6..=15)
            .map(|i| TrackPoint {
                t_s: i as f64 * 10.0,
                xyz_m: [6378137.0 + 1500.0, 200.0 * (i as f64 * 10.0 - tc), 0.0],
            })
            .collect(),
    }
}
struct Fixture {
    doc: ReviewWorkspace,
    map: MapData,
    lightning: Option<LightningCatalog>,
    observers: Vec<ObserverLocation>,
    plan: RegionalPlan,
}
fn fixture() -> Fixture {
    let observer = ObserverLocation {
        latitude_deg: 0.0,
        longitude_deg: 0.0,
        ellipsoid_height_m: 0.0,
    };
    let aircraft = track("true-aircraft", 50.0);
    let wrong = track("wrong-aircraft", 62.0);
    let mut doppler = EvidenceEvent::manual("doppler".into(), EventKind::Doppler, 20.0, 90.0);
    doppler.review = ReviewState::Confirmed;
    // Fabricated physics control: source class is specified by its generator.
    doppler.acoustic_source = geosonnar::workspace::AcousticSourceAssessment::Compatible;
    doppler.selected_for_scoring = true;
    doppler.arrival_s = 50.0 + 1500.0 / C - TRUE_OFFSET;
    doppler.points = (0..=20)
        .map(|i| {
            let time_s = 30.0 + i as f64 * 2.0;
            TracePoint {
                time_s,
                frequency_hz: TONE * analytic_factor(&aircraft, &observer, time_s + TRUE_OFFSET),
            }
        })
        .collect();
    // Current fixed associations deliberately remain empty; a search may propose
    // them only from the separately approved plan, never from hidden defaults.
    let true_strike = LightningStrike {
        id: "true-strike".into(),
        time_utc_s: ORIGIN + 30.75,
        latitude_deg: 0.015,
        longitude_deg: 0.005,
        altitude_m: Some(600.0),
        intensity: None,
        source_record: None,
    };
    let p = xyz(true_strike.latitude_deg, true_strike.longitude_deg, 600.0);
    let delay = ((p[0] - 6378137.0).powi(2) + p[1].powi(2) + p[2].powi(2)).sqrt() / C;
    let mut thunder = EvidenceEvent::manual("thunder".into(), EventKind::Thunder, 25.0, 60.0);
    thunder.review = ReviewState::Confirmed;
    // Fabricated physics control: source class is specified by its generator.
    thunder.acoustic_source = geosonnar::workspace::AcousticSourceAssessment::Compatible;
    thunder.selected_for_scoring = true;
    thunder.arrival_s = 30.75 + delay - TRUE_OFFSET;
    let mut wrong_strike = true_strike.clone();
    wrong_strike.id = "wrong-strike".into();
    wrong_strike.time_utc_s += 17.0;
    let doc = ReviewWorkspace {
        schema_version: 1,
        case_id: "regional-control".into(),
        recording_sha256: "0".repeat(64),
        channel: 0,
        duration_s: 150.0,
        recording_start_utc: "2026-09-08 00:00:00".into(),
        sound_speed_mps: C,
        atmosphere: None,
        regional_plan: None,
        regional_comparison: None,
        envelope_review: None,
        receiver_terrain: None,
        events: vec![doppler, thunder],
        notes: "Fabricated oracle; not a field claim".into(),
        mix: EvidenceMix {
            doppler_frequency: true,
            doppler_timing: true,
            thunder_timing: true,
            frequency_weight: 2.0,
            doppler_timing_weight: 0.7,
            thunder_weight: 1.3,
            ..Default::default()
        },
        map: MapSelection {
            bounds: [-0.04, -0.04, 0.04, 0.04],
            grid_spacing_m: 2000.0,
            receiver_height_m: 0.0,
            clock_adjustment_s: 99.0,
        },
        datasets: Vec::new(),
        dataset_options: Vec::new(),
        strike_fallback_height_m: None,
    };
    let map=map_data::parse_value(&serde_json::json!({"config":{"time_origin_utc_s":ORIGIN,"max_gap_s":15.0},"tracks":[aircraft,wrong]})).unwrap();
    let lightning = Some(LightningCatalog {
        schema_version: 1,
        input_path: PathBuf::from("synthetic-lightning.json"),
        input_bytes: 0,
        input_sha256: "0".repeat(64),
        provenance: LightningProvenance {
            provider: "Independent synthetic control".into(),
            source: "Analytic point source".into(),
            license: None,
            notes: None,
            altitude_datum: LightningAltitudeDatum::Wgs84Ellipsoid,
            source_metadata: None,
        },
        strikes: vec![true_strike, wrong_strike],
    });
    let observers = vec![
        ObserverLocation {
            latitude_deg: 0.02,
            longitude_deg: 0.0,
            ellipsoid_height_m: 0.0,
        },
        observer,
        ObserverLocation {
            latitude_deg: 0.0,
            longitude_deg: -0.02,
            ellipsoid_height_m: 0.0,
        },
    ];
    let plan = RegionalPlan {
        schema_version: 1,
        clock: ClockRange {
            min_offset_s: -2.0,
            max_offset_s: 10.0,
            step_s: 1.0,
        },
        events: vec![
            EventAlternatives {
                event_id: "doppler".into(),
                source_ids: vec!["wrong-aircraft".into(), "true-aircraft".into()],
                emitted_frequency_hz: Some([100.0, 140.0]),
            },
            EventAlternatives {
                event_id: "thunder".into(),
                source_ids: vec!["wrong-strike".into(), "true-strike".into()],
                emitted_frequency_hz: None,
            },
        ],
        max_evaluations: 200_000,
        retain_hypotheses: 1000,
    };
    Fixture {
        doc,
        map,
        lightning,
        observers,
        plan,
    }
}
fn search(f: &Fixture) -> RegionalReport {
    regional_search::run(
        &f.doc,
        &f.map,
        &f.lightning,
        &f.observers,
        &f.plan,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap()
}

#[test]
fn regional_estimate_and_run_block_unassessed_sources_before_any_work() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    for assessment in [
        Assessment::Unassessed,
        Assessment::Incompatible,
        Assessment::UncertainHypothesis {
            rationale: " \n ".into(),
        },
    ] {
        let mut f = fixture();
        f.doc.events[0].acoustic_source = assessment;
        let before = serde_json::to_vec(&f.doc).unwrap();
        let error = regional_search::estimate(&f.doc, &f.map, &f.lightning, &f.observers, &f.plan)
            .unwrap_err();
        assert!(error.contains("Acoustic source assessment"));
        assert!(error.contains("doppler"));
        let mut callbacks = 0;
        assert!(
            regional_search::run(
                &f.doc,
                &f.map,
                &f.lightning,
                &f.observers,
                &f.plan,
                &AtomicBool::new(false),
                |_| callbacks += 1
            )
            .is_err()
        );
        assert_eq!(callbacks, 0);
        assert_eq!(serde_json::to_vec(&f.doc).unwrap(), before);
    }
}

#[test]
fn uncertain_source_assumptions_survive_regional_apply_and_fixed_replay() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    let mut f = fixture();
    let old = search(&f);
    f.doc.events[0].acoustic_source = Assessment::UncertainHypothesis {
        rationale:
            "Synthetic competing sound-class interpretation retained for sensitivity analysis"
                .into(),
    };
    let before = serde_json::to_vec(&f.doc).unwrap();
    assert!(
        regional_search::apply_hypothesis(&f.doc, &old, &old.hypotheses[0].id)
            .unwrap_err()
            .contains("changed")
    );
    let report = search(&f);
    assert_eq!(
        serde_json::to_value(&report.hypotheses).unwrap(),
        serde_json::to_value(&old.hypotheses).unwrap()
    );
    assert!(report.limitations[0].contains("UNCERTAIN ACOUSTIC SOURCE"));
    assert!(report.limitations[0].contains("sensitivity analysis"));
    let best = &report.hypotheses[0];
    let applied = regional_search::apply_hypothesis(&f.doc, &report, &best.id).unwrap();
    assert_eq!(
        applied.events[0].acoustic_source,
        f.doc.events[0].acoustic_source
    );
    let replay = workspace_scoring::score_observers(
        &applied,
        &f.map,
        &f.lightning,
        None,
        std::slice::from_ref(&best.observer),
    )
    .unwrap();
    assert!((replay.candidates[0].score.unwrap() - best.score).abs() < 1e-12);
    assert!(replay.limitations[0].contains("UNCERTAIN ACOUSTIC SOURCE"));
    assert!(!report.location_verified);
    assert_eq!(serde_json::to_vec(&f.doc).unwrap(), before);
}

#[test]
fn independent_mode_gate_ignores_excluded_modality_but_combined_requires_both() {
    use geosonnar::{
        regional_search::SearchMode, workspace::AcousticSourceAssessment as Assessment,
    };
    for mode in [SearchMode::DopplerOnly, SearchMode::ThunderOnly] {
        let mut f = fixture();
        let inactive = if mode == SearchMode::DopplerOnly {
            1
        } else {
            0
        };
        f.doc.events[inactive].acoustic_source = Assessment::Unassessed;
        let before = serde_json::to_vec(&f.doc).unwrap();
        let mut request = comparison_plan(&f);
        request.modes = vec![mode];
        let report = regional_search::run_modalities(
            &f.doc,
            &f.map,
            &f.lightning,
            &f.observers,
            &request,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(report.status, SearchStatus::Completed);
        assert_eq!(report.modes.len(), 1);
        assert!(
            report.modes[0].report.as_ref().unwrap().hypotheses[0]
                .assignments
                .iter()
                .all(|a| a.event_id != f.doc.events[inactive].id)
        );
        request.modes.push(SearchMode::Combined);
        assert!(
            regional_search::estimate_modalities(
                &f.doc,
                &f.map,
                &f.lightning,
                &f.observers,
                &request
            )
            .unwrap_err()
            .contains("unassessed")
        );
        // Even a permitted uncertainty in an excluded modality must not appear
        // as an assumption supporting the requested mode's result.
        f.doc.events[inactive].acoustic_source = Assessment::UncertainHypothesis {
            rationale: "Excluded-mode assumption".into(),
        };
        request.modes = vec![mode];
        let report = regional_search::run_modalities(
            &f.doc,
            &f.map,
            &f.lightning,
            &f.observers,
            &request,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert!(
            report
                .limitations
                .iter()
                .all(|text| !text.contains("Excluded-mode assumption"))
        );
        f.doc.events[inactive].acoustic_source = Assessment::Unassessed;
        assert_eq!(serde_json::to_vec(&f.doc).unwrap(), before);
    }
}

#[test]
fn unknown_shared_clock_tone_and_sources_recover_independent_analytic_scene() {
    let f = fixture();
    let before = serde_json::to_value(&f.doc).unwrap();
    let report = search(&f);
    assert_eq!(report.status, SearchStatus::Completed);
    assert_eq!(report.progress.completed_evaluations, 156);
    assert_eq!(report.progress.completed_cells, 39);
    assert_eq!(report.hypotheses.len(), 156);
    let best = &report.hypotheses[0];
    assert_eq!(best.clock_offset_s, TRUE_OFFSET);
    assert_eq!(best.observer.latitude_deg, 0.0);
    assert_eq!(best.observer.longitude_deg, 0.0);
    assert!(best.score < 1e-15, "{}", best.score);
    assert_eq!(best.assignments[0].source_id, "true-aircraft");
    assert_eq!(best.assignments[1].source_id, "true-strike");
    assert!((best.assignments[0].emitted_frequency_hz.unwrap() - TONE).abs() < 1e-10);
    assert_eq!(serde_json::to_value(&f.doc).unwrap(), before);
    assert!(!report.location_verified);
    assert!(!report.retained_truncated);
    assert_eq!(report.supported_hypotheses, Some(156));
}

#[test]
fn every_joint_score_replays_through_unchanged_fixed_scorer() {
    let f = fixture();
    let report = search(&f);
    for hypothesis in &report.hypotheses {
        let applied = regional_search::apply_hypothesis(&f.doc, &report, &hypothesis.id).unwrap();
        let fixed = workspace_scoring::score_observers(
            &applied,
            &f.map,
            &f.lightning,
            None,
            std::slice::from_ref(&hypothesis.observer),
        )
        .unwrap();
        assert!((fixed.candidates[0].score.unwrap() - hypothesis.score).abs() < 1e-8);
        assert_eq!(applied.events[0].points, f.doc.events[0].points);
        assert_eq!(applied.events[0].review, ReviewState::Confirmed);
        assert_eq!(applied.events[1].review, ReviewState::Confirmed);
        assert!(
            applied
                .events
                .iter()
                .all(|event| event.selected_for_scoring)
        );
        assert_eq!(applied.map.clock_adjustment_s, hypothesis.clock_offset_s);
    }
}

#[test]
fn top_k_matches_exhaustive_fixed_scorer_with_independent_tone_fit() {
    let mut f = fixture();
    f.plan.retain_hypotheses = 7;
    let report = search(&f);
    let mut exhaustive = Vec::new();
    for clock in f.plan.clock.values().unwrap() {
        for observer in &f.observers {
            for aircraft in &f.map.tracks {
                for strike in &f.lightning.as_ref().unwrap().strikes {
                    let mut doc = f.doc.clone();
                    doc.map.clock_adjustment_s = clock;
                    let event = &mut doc.events[0];
                    event.flight_id = aircraft.id.clone();
                    let factors: Vec<_> = event
                        .points
                        .iter()
                        .map(|p| {
                            analytic_factor(&aircraft.physics_track, observer, p.time_s + clock)
                        })
                        .collect();
                    event.emitted_frequency_hz = (factors
                        .iter()
                        .zip(&event.points)
                        .map(|(factor, p)| factor * p.frequency_hz)
                        .sum::<f64>()
                        / factors.iter().map(|factor| factor * factor).sum::<f64>())
                    .clamp(100.0, 140.0);
                    doc.events[1].strike_id = strike.id.clone();
                    let fixed = workspace_scoring::score_observers(
                        &doc,
                        &f.map,
                        &f.lightning,
                        None,
                        std::slice::from_ref(observer),
                    )
                    .unwrap();
                    exhaustive.push(fixed.candidates[0].score.unwrap());
                }
            }
        }
    }
    exhaustive.sort_by(f64::total_cmp);
    for (actual, expected) in report.hypotheses.iter().zip(exhaustive) {
        assert!((actual.score - expected).abs() < 1e-8);
    }
    assert!(report.retained_truncated);
    assert_eq!(report.hypotheses.len(), 7);
}

#[test]
fn all_event_weights_remain_equal_within_each_selected_term() {
    let mut f = fixture();
    let mut second = f.doc.events[0].clone();
    second.id = "doppler-second".into();
    second.points.truncate(3);
    f.doc.events.push(second);
    let mut alternatives = f.plan.events[0].clone();
    alternatives.event_id = "doppler-second".into();
    f.plan.events.push(alternatives);
    f.plan.retain_hypotheses = 20;
    let report = search(&f);
    for h in &report.hypotheses {
        let applied = regional_search::apply_hypothesis(&f.doc, &report, &h.id).unwrap();
        let fixed = workspace_scoring::score_observers(
            &applied,
            &f.map,
            &f.lightning,
            None,
            std::slice::from_ref(&h.observer),
        )
        .unwrap();
        assert!((h.score - fixed.candidates[0].score.unwrap()).abs() < 1e-8);
    }
}

#[test]
fn pure_doppler_and_pure_thunder_do_not_need_disabled_modality_sources() {
    let mut f = fixture();
    f.doc.mix.thunder_timing = false;
    f.plan.events.pop();
    f.lightning = None;
    let report = search(&f);
    assert!(!report.hypotheses.is_empty());
    assert!(report.hypotheses.iter().all(|h| h.assignments.len() == 1));
    let mut f = fixture();
    f.doc.mix.doppler_frequency = false;
    f.doc.mix.doppler_timing = false;
    f.plan.events.remove(0);
    f.map.tracks.clear();
    let report = search(&f);
    assert!(!report.hypotheses.is_empty());
    assert!(
        report
            .hypotheses
            .iter()
            .all(|h| h.assignments[0].emitted_frequency_hz.is_none())
    );
}

#[test]
fn missing_selected_support_never_becomes_a_partial_combined_score() {
    let mut f = fixture();
    for track in &mut f.map.tracks {
        track.time_origin_utc_s = None;
    }
    let report = search(&f);
    assert!(report.hypotheses.is_empty());
    assert_eq!(report.rankable_cells, 0);
    assert_eq!(report.unavailable_cells, 39);
    assert_eq!(report.unavailable_count, 78);
    assert!(
        report
            .unavailable
            .iter()
            .all(|u| u.reasons.iter().any(|r| r.contains("UTC origin")))
    );
    assert_eq!(report.status, SearchStatus::Completed);
}

#[test]
fn one_unavailable_alternative_does_not_remove_a_reviewed_valid_alternative() {
    let mut f = fixture();
    f.map
        .tracks
        .iter_mut()
        .find(|t| t.id == "wrong-aircraft")
        .unwrap()
        .time_origin_utc_s = None;
    let report = search(&f);
    assert_eq!(report.unavailable_count, 39);
    assert!(
        report
            .hypotheses
            .iter()
            .all(|h| h.assignments[0].source_id == "true-aircraft")
    );
    assert_eq!(report.hypotheses.len(), 78);
}

#[test]
fn cancellation_retains_only_finished_cells_and_cannot_apply_partial_results() {
    let f = fixture();
    let cancel = AtomicBool::new(false);
    let mut samples = Vec::new();
    let report = regional_search::run(
        &f.doc,
        &f.map,
        &f.lightning,
        &f.observers,
        &f.plan,
        &cancel,
        |p| {
            if p.completed_cells >= 3 {
                cancel.store(true, Ordering::Relaxed);
            }
            samples.push(p.completed_evaluations);
        },
    )
    .unwrap();
    assert_eq!(report.status, SearchStatus::Cancelled);
    assert_eq!(report.progress.completed_cells, 3);
    assert!(samples.windows(2).all(|p| p[0] <= p[1]));
    assert!(!report.hypotheses.is_empty());
    assert!(
        regional_search::apply_hypothesis(&f.doc, &report, &report.hypotheses[0].id)
            .unwrap_err()
            .contains("completed")
    );
    let report = regional_search::run(
        &f.doc,
        &f.map,
        &f.lightning,
        &f.observers,
        &f.plan,
        &AtomicBool::new(true),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.progress.completed_evaluations, 0);
    assert!(report.hypotheses.is_empty());
}

#[test]
fn interrupted_source_batch_is_not_mislabeled_as_a_complete_cell() {
    let f = fixture();
    let cancel = AtomicBool::new(false);
    let report = regional_search::run(
        &f.doc,
        &f.map,
        &f.lightning,
        &f.observers,
        &f.plan,
        &cancel,
        |p| {
            if p.completed_evaluations > 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    )
    .unwrap();
    assert_eq!(report.status, SearchStatus::Cancelled);
    assert!(report.progress.completed_evaluations > 0);
    assert_eq!(report.progress.completed_cells, 0);
    assert!(report.hypotheses.is_empty());
}

#[test]
fn plan_and_work_limits_are_checked_before_any_search_progress() {
    let mut f = fixture();
    f.plan.max_evaluations = 155;
    let mut callbacks = 0;
    let error = regional_search::run(
        &f.doc,
        &f.map,
        &f.lightning,
        &f.observers,
        &f.plan,
        &AtomicBool::new(false),
        |_| callbacks += 1,
    )
    .unwrap_err();
    assert!(error.contains("156"));
    assert_eq!(callbacks, 0);
    f.plan.max_evaluations = 200_000;
    f.plan.clock.step_s = 0.00001;
    assert!(
        regional_search::estimate(&f.doc, &f.map, &f.lightning, &f.observers, &f.plan).is_err()
    );
}

#[test]
fn drafts_are_saveable_but_no_source_is_approved_implicitly() {
    let f = fixture();
    let draft = RegionalPlan::default();
    assert!(draft.validate_draft().is_ok());
    assert!(draft.validate().is_err());
    let mut plan = f.plan.clone();
    plan.events[0].source_ids.clear();
    assert!(plan.validate_draft().is_ok());
    assert!(regional_search::estimate(&f.doc, &f.map, &f.lightning, &f.observers, &plan).is_err());
    let mut doc = f.doc.clone();
    doc.events[0].review = ReviewState::Pending;
    assert!(regional_search::estimate(&doc, &f.map, &f.lightning, &f.observers, &f.plan).is_err());
}

#[test]
fn duplicate_unknown_and_omitted_event_sources_are_rejected() {
    let f = fixture();
    let mut plan = f.plan.clone();
    plan.events[0].source_ids.push("true-aircraft".into());
    assert!(plan.validate().is_err());
    let mut plan = f.plan.clone();
    plan.events[0].source_ids = vec!["not-loaded".into()];
    assert!(regional_search::estimate(&f.doc, &f.map, &f.lightning, &f.observers, &plan).is_err());
    let mut plan = f.plan.clone();
    plan.events.pop();
    assert!(regional_search::estimate(&f.doc, &f.map, &f.lightning, &f.observers, &plan).is_err());
    let mut map = f.map.clone();
    map.tracks.push(map.tracks[0].clone());
    assert!(regional_search::estimate(&f.doc, &map, &f.lightning, &f.observers, &f.plan).is_err());
}

#[test]
fn fitted_tone_and_clock_edges_remain_visible() {
    let mut f = fixture();
    f.plan.events[0].emitted_frequency_hz = Some([100.0, 110.0]);
    f.plan.clock = ClockRange {
        min_offset_s: TRUE_OFFSET,
        max_offset_s: TRUE_OFFSET + 1.0,
        step_s: 1.0,
    };
    let report = search(&f);
    assert!(report.hypotheses.iter().all(|h| h.clock_at_boundary));
    assert!(report.hypotheses.iter().any(|h| h.frequency_at_boundary));
    assert!(
        report
            .hypotheses
            .iter()
            .flat_map(|h| &h.assignments)
            .filter_map(|a| a.emitted_frequency_hz)
            .all(|tone| (100.0..=110.0).contains(&tone))
    );
}

#[test]
fn explicit_fixed_tone_is_not_marked_as_a_fitting_boundary() {
    let mut f = fixture();
    f.plan.events[0].emitted_frequency_hz = Some([TONE, TONE]);
    f.plan.clock = ClockRange {
        min_offset_s: TRUE_OFFSET,
        max_offset_s: TRUE_OFFSET,
        step_s: 1.0,
    };
    let report = search(&f);
    assert!(
        report
            .hypotheses
            .iter()
            .all(|h| !h.frequency_at_boundary && !h.clock_at_boundary)
    );
}

#[test]
fn ordering_is_deterministic_and_hashes_detect_changed_loaded_inputs() {
    let mut f = fixture();
    let initial = search(&f);
    f.plan.events.reverse();
    for e in &mut f.plan.events {
        e.source_ids.reverse();
    }
    f.map.tracks.reverse();
    let reversed = search(&f);
    assert_eq!(
        serde_json::to_value(&initial.hypotheses).unwrap(),
        serde_json::to_value(&reversed.hypotheses).unwrap()
    );
    let old = regional_search::inputs_sha256(&f.map, &f.lightning).unwrap();
    f.map.tracks[0].physics_track.points[0].xyz_m[0] += 1.0;
    assert_ne!(
        old,
        regional_search::inputs_sha256(&f.map, &f.lightning).unwrap()
    );
}

#[test]
fn changed_evidence_and_unapproved_report_associations_cannot_be_applied() {
    let f = fixture();
    let mut report = search(&f);
    let id = report.hypotheses[0].id.clone();
    let mut doc = f.doc.clone();
    doc.notes.push_str(" changed");
    assert!(regional_search::apply_hypothesis(&doc, &report, &id).is_err());
    report.hypotheses[0].assignments[0].source_id = "not-approved".into();
    assert!(regional_search::apply_hypothesis(&f.doc, &report, &id).is_err());
}

#[test]
fn shared_clock_endpoints_are_included_without_an_extra_hidden_offset() {
    assert_eq!(
        ClockRange {
            min_offset_s: -1.0,
            max_offset_s: 1.0,
            step_s: 0.7
        }
        .values()
        .unwrap(),
        vec![-1.0, -0.30000000000000004, 0.3999999999999999, 1.0]
    );
    let mut f = fixture();
    f.plan.clock = ClockRange {
        min_offset_s: TRUE_OFFSET,
        max_offset_s: TRUE_OFFSET,
        step_s: 1.0,
    };
    let report = search(&f);
    assert_eq!(
        report.hypotheses[0].recording_start_utc_s,
        ORIGIN + TRUE_OFFSET
    );
    // Existing fixed adjustment is 99 s; adding it again would destroy the match.
    assert!(report.hypotheses[0].score < 1e-15);
}

#[test]
fn regional_bounds_and_joint_weight_overflow_fail_before_scoring() {
    let mut f = fixture();
    f.observers[0].latitude_deg = 0.1;
    assert!(
        regional_search::estimate(&f.doc, &f.map, &f.lightning, &f.observers, &f.plan)
            .unwrap_err()
            .contains("rectangle")
    );
    f.observers[0].latitude_deg = 0.02;
    f.doc.mix.doppler_timing = false;
    f.doc.mix.frequency_weight = 1e308;
    f.doc.mix.thunder_weight = 1e308;
    assert!(
        regional_search::estimate(&f.doc, &f.map, &f.lightning, &f.observers, &f.plan)
            .unwrap_err()
            .contains("finite positive sum")
    );
}

#[test]
fn track_endpoint_reception_coverage_matches_independent_exclusion_count() {
    let mut f = fixture();
    for track in &mut f.map.tracks {
        track
            .physics_track
            .points
            .retain(|p| p.t_s >= 0.0 && p.t_s <= 120.0);
        track.segments = vec![[0, track.physics_track.points.len()]];
    }
    let mut unsupported = 0;
    for clock in f.plan.clock.values().unwrap() {
        for observer in &f.observers {
            for track in &f.map.tracks {
                let receiver = xyz(
                    observer.latitude_deg,
                    observer.longitude_deg,
                    observer.ellipsoid_height_m,
                );
                let arrival = |p: &TrackPoint| {
                    p.t_s
                        + (0..3)
                            .map(|i| (p.xyz_m[i] - receiver[i]).powi(2))
                            .sum::<f64>()
                            .sqrt()
                            / C
                };
                let first = arrival(&track.physics_track.points[0]);
                let last = arrival(track.physics_track.points.last().unwrap());
                if f.doc.events[0]
                    .points
                    .iter()
                    .any(|p| p.time_s + clock < first || p.time_s + clock > last)
                {
                    unsupported += 1;
                }
            }
        }
    }
    assert_eq!(unsupported, 25);
    let report = search(&f);
    assert_eq!(report.unavailable_count, unsupported);
    assert_eq!(report.hypotheses.len(), 156 - 2 * unsupported as usize);
    assert!(report.unavailable.iter().all(|u| {
        u.reasons
            .iter()
            .any(|reason| reason.contains("coverage") || reason.contains("predictions"))
    }));
}

#[test]
fn geographic_summary_retains_and_replays_locations_outside_the_global_top_k() {
    let mut f = fixture();
    f.plan.retain_hypotheses = 1;
    let report = search(&f);
    assert_eq!(report.hypotheses.len(), 1);
    assert_eq!(report.location_summaries.len(), 3);
    for (index, row) in report.location_summaries.iter().enumerate() {
        assert_eq!(row.observer_index, index);
        assert_eq!(row.completed_clocks, 13);
        assert_eq!(row.rankable_clocks, 13);
        assert_eq!(row.unavailable_clocks, 0);
        let applied = regional_search::apply_location_hypothesis(&f.doc, &report, index).unwrap();
        let fixed = workspace_scoring::score_observers(
            &applied,
            &f.map,
            &f.lightning,
            None,
            std::slice::from_ref(&row.observer),
        )
        .unwrap();
        assert!((fixed.candidates[0].score.unwrap() - row.best_score.unwrap()).abs() < 1e-8);
    }
    assert_ne!(
        report.location_summaries[0]
            .best_hypothesis_id
            .as_ref()
            .unwrap(),
        &report.hypotheses[0].id
    );
}

#[test]
fn unavailable_geographic_locations_are_preserved_with_explicit_clock_counts() {
    let mut f = fixture();
    for track in &mut f.map.tracks {
        track.time_origin_utc_s = None;
    }
    let report = search(&f);
    assert_eq!(report.location_summaries.len(), 3);
    for row in &report.location_summaries {
        assert_eq!(row.completed_clocks, 13);
        assert_eq!(row.unavailable_clocks, 13);
        assert_eq!(row.rankable_clocks, 0);
        assert!(row.best_score.is_none());
        assert!(row.best_assignments.is_empty());
    }
    assert!(regional_search::apply_location_hypothesis(&f.doc, &report, 0).is_err());
}

fn comparison_plan(f: &Fixture) -> regional_search::ModalityPlan {
    regional_search::ModalityPlan {
        regional: f.plan.clone(),
        modes: vec![
            regional_search::SearchMode::DopplerOnly,
            regional_search::SearchMode::ThunderOnly,
            regional_search::SearchMode::Combined,
        ],
        max_total_evaluations: 1000,
    }
}

#[test]
fn independently_searched_modes_match_individual_runs_and_preserve_all_priors() {
    let f = fixture();
    let request = comparison_plan(&f);
    let original = serde_json::to_value(&f.doc).unwrap();
    let report = regional_search::run_modalities(
        &f.doc,
        &f.map,
        &f.lightning,
        &f.observers,
        &request,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.status, SearchStatus::Completed);
    assert_eq!(report.progress.completed_evaluations, 312);
    assert_eq!(report.progress.completed_modes, 3);
    for entry in &report.modes {
        assert_eq!(entry.status, regional_search::ModeStatus::Completed);
        let derived =
            regional_search::mode_document(&f.doc, &request.regional, entry.mode).unwrap();
        assert_eq!(derived.events, f.doc.events);
        assert_eq!(derived.map, f.doc.map);
        assert_eq!(derived.mix.frequency_weight, f.doc.mix.frequency_weight);
        assert_eq!(derived.mix.thunder_weight, f.doc.mix.thunder_weight);
        assert_eq!(derived.regional_plan.as_ref().unwrap().clock, f.plan.clock);
        let direct = regional_search::run(
            &derived,
            &f.map,
            &f.lightning,
            &f.observers,
            derived.regional_plan.as_ref().unwrap(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&direct.hypotheses).unwrap(),
            serde_json::to_value(&entry.report.as_ref().unwrap().hypotheses).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&direct.location_summaries).unwrap(),
            serde_json::to_value(&entry.report.as_ref().unwrap().location_summaries).unwrap()
        );
    }
    assert_eq!(serde_json::to_value(&f.doc).unwrap(), original);
}

#[test]
fn modality_total_budget_and_duplicate_requests_fail_before_any_work() {
    let f = fixture();
    let mut request = comparison_plan(&f);
    request.max_total_evaluations = 311;
    let mut callbacks = 0;
    let error = regional_search::run_modalities(
        &f.doc,
        &f.map,
        &f.lightning,
        &f.observers,
        &request,
        &AtomicBool::new(false),
        |_| callbacks += 1,
    )
    .unwrap_err();
    assert!(error.contains("312"));
    assert_eq!(callbacks, 0);
    request.max_total_evaluations = 1000;
    request.modes.push(regional_search::SearchMode::Combined);
    assert!(
        regional_search::estimate_modalities(&f.doc, &f.map, &f.lightning, &f.observers, &request)
            .is_err()
    );
    request.modes = vec![regional_search::SearchMode::ThunderOnly];
    let mut doc = f.doc.clone();
    doc.mix.thunder_timing = false;
    assert!(
        regional_search::estimate_modalities(&doc, &f.map, &f.lightning, &f.observers, &request)
            .is_err()
    );
}

#[test]
fn cancellation_between_modes_keeps_completed_mode_and_explicitly_unstarted_modes() {
    let f = fixture();
    let request = comparison_plan(&f);
    let cancel = AtomicBool::new(false);
    let report = regional_search::run_modalities(
        &f.doc,
        &f.map,
        &f.lightning,
        &f.observers,
        &request,
        &cancel,
        |p| {
            if p.completed_modes == 1 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    )
    .unwrap();
    assert_eq!(report.status, SearchStatus::Cancelled);
    assert_eq!(report.progress.completed_modes, 1);
    assert_eq!(report.progress.completed_evaluations, 78);
    assert_eq!(
        report.modes[0].status,
        regional_search::ModeStatus::Completed
    );
    assert!(report.modes[1..].iter().all(|entry| entry.status
        == regional_search::ModeStatus::NotStarted
        && entry.report.is_none()));
    let id = &report.modes[0].report.as_ref().unwrap().hypotheses[0].id;
    assert!(
        regional_search::apply_modality_hypothesis(
            &f.doc,
            &report,
            regional_search::SearchMode::DopplerOnly,
            id
        )
        .is_err()
    );
}

#[test]
fn cancellation_inside_a_mode_reports_partial_geographic_coverage() {
    let f = fixture();
    let request = comparison_plan(&f);
    let cancel = AtomicBool::new(false);
    let report = regional_search::run_modalities(
        &f.doc,
        &f.map,
        &f.lightning,
        &f.observers,
        &request,
        &cancel,
        |p| {
            if p.completed_evaluations > 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    )
    .unwrap();
    assert_eq!(
        report.modes[0].status,
        regional_search::ModeStatus::Cancelled
    );
    let partial = report.modes[0].report.as_ref().unwrap();
    assert_eq!(partial.location_summaries.len(), 3);
    assert!(
        partial
            .location_summaries
            .iter()
            .all(|row| row.completed_clocks == 0 && row.best_score.is_none())
    );
    assert_eq!(report.progress.completed_modes, 0);
    assert_eq!(report.progress.completed_evaluations, 3);
}

#[test]
fn explicit_mode_location_application_switches_terms_without_altering_observations() {
    let mut f = fixture();
    f.plan.retain_hypotheses = 1;
    let request = comparison_plan(&f);
    let report = regional_search::run_modalities(
        &f.doc,
        &f.map,
        &f.lightning,
        &f.observers,
        &request,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let applied = regional_search::apply_modality_location(
        &f.doc,
        &report,
        regional_search::SearchMode::ThunderOnly,
        0,
    )
    .unwrap();
    assert!(
        !applied.mix.doppler_frequency && !applied.mix.doppler_timing && applied.mix.thunder_timing
    );
    assert_eq!(applied.events[0].points, f.doc.events[0].points);
    assert_eq!(applied.events[0].review, f.doc.events[0].review);
    assert_eq!(applied.mix.thunder_weight, f.doc.mix.thunder_weight);
    let mut changed = f.doc.clone();
    changed.notes.push_str("changed");
    assert!(
        regional_search::apply_modality_location(
            &changed,
            &report,
            regional_search::SearchMode::ThunderOnly,
            0
        )
        .is_err()
    );
}

#[test]
fn older_reports_deserialize_without_geographic_summaries_but_cannot_apply_missing_locations() {
    let f = fixture();
    let mut value = serde_json::to_value(search(&f)).unwrap();
    value.as_object_mut().unwrap().remove("location_summaries");
    let report: RegionalReport = serde_json::from_value(value).unwrap();
    assert!(report.location_summaries.is_empty());
    assert!(regional_search::apply_location_hypothesis(&f.doc, &report, 0).is_err());
}
