//! Synthetic physical trajectory/clock controls; no known private location.
use geosonnar::{
    evidence_score::*,
    map_data,
    physics::{self, ModelConfig, PreparedTrack, Track, TrackPoint},
    workspace::{
        EventKind, EvidenceEvent, EvidenceMix, MapSelection, ReviewState, ReviewWorkspace,
    },
    workspace_scoring,
};
use std::path::PathBuf;

fn fixture() -> (
    ReviewWorkspace,
    map_data::MapData,
    Option<LightningCatalog>,
    ObserverLocation,
) {
    let origin = 1_788_825_600.0;
    let observer = ObserverLocation {
        latitude_deg: 0.0,
        longitude_deg: 0.0,
        ellipsoid_height_m: 0.0,
    };
    let xyz = physics::geodetic_to_ecef(0.0, 0.0, 0.0).unwrap();
    let track = Track {
        flight_id: "fabricated-aircraft".into(),
        points: (0..=10)
            .map(|i| TrackPoint {
                t_s: i as f64 * 10.0,
                xyz_m: [xyz[0] + 1000.0, (i as f64 * 10.0 - 50.0) * 200.0, 0.0],
            })
            .collect(),
    };
    let model = PreparedTrack::new(
        &track,
        xyz,
        ModelConfig {
            sound_speed_mps: 343.0,
            max_gap_s: 15.0,
        },
    )
    .unwrap();
    let mut aircraft =
        EvidenceEvent::manual("aircraft-event".into(), EventKind::Doppler, 30.0, 90.0);
    aircraft.flight_id = track.flight_id.clone();
    aircraft.emitted_frequency_hz = 110.0;
    aircraft.arrival_s = model.closest_approach().arrival_time_s;
    aircraft.points = (0..=10)
        .map(|i| {
            let time_s = 30.0 + i as f64 * 5.0;
            geosonnar::workspace::TracePoint {
                time_s,
                frequency_hz: 110.0 * model.factor_at_arrival(time_s).unwrap(),
            }
        })
        .collect();
    aircraft.review = ReviewState::Confirmed;
    // Fabricated physics control: source class is specified by its generator.
    aircraft.acoustic_source = geosonnar::workspace::AcousticSourceAssessment::Compatible;
    aircraft.selected_for_scoring = true;
    let strike = LightningStrike {
        id: "fabricated-strike".into(),
        time_utc_s: origin + 40.0,
        latitude_deg: 0.015,
        longitude_deg: 0.0,
        altitude_m: Some(1000.0),
        intensity: None,
        source_record: None,
    };
    let p = physics::geodetic_to_ecef(strike.latitude_deg, strike.longitude_deg, 1000.0).unwrap();
    let delay = (p[0] - xyz[0]).hypot(p[1] - xyz[1]).hypot(p[2] - xyz[2]) / 343.0;
    let mut thunder = EvidenceEvent::manual("thunder-event".into(), EventKind::Thunder, 40.0, 60.0);
    thunder.arrival_s = 40.0 + delay;
    thunder.strike_id = strike.id.clone();
    thunder.review = ReviewState::Confirmed;
    // Fabricated physics control: source class is specified by its generator.
    thunder.acoustic_source = geosonnar::workspace::AcousticSourceAssessment::Compatible;
    thunder.selected_for_scoring = true;
    let document = ReviewWorkspace {
        schema_version: 1,
        case_id: "fabricated-case".into(),
        recording_sha256: "0".repeat(64),
        channel: 0,
        duration_s: 140.0,
        recording_start_utc: "2026-09-08 00:00:00".into(),
        sound_speed_mps: 343.0,
        events: vec![aircraft, thunder],
        atmosphere: None,
        regional_plan: None,
        regional_comparison: None,
        envelope_review: None,
        receiver_terrain: None,
        notes: "Synthetic test".into(),
        mix: EvidenceMix {
            doppler_frequency: true,
            doppler_timing: true,
            thunder_timing: true,
            ..Default::default()
        },
        map: MapSelection {
            bounds: [-0.01, -0.01, 0.01, 0.01],
            grid_spacing_m: 2000.0,
            receiver_height_m: 0.0,
            clock_adjustment_s: 0.0,
        },
        datasets: Vec::new(),
        dataset_options: Vec::new(),
        strike_fallback_height_m: None,
    };
    let map=map_data::parse_value(&serde_json::json!({"config":{"time_origin_utc_s":origin,"max_gap_s":15.0},"tracks":[track]})).unwrap();
    let lightning = Some(LightningCatalog {
        schema_version: 1,
        input_path: PathBuf::from("fabricated-lightning.json"),
        input_bytes: 0,
        input_sha256: "0".repeat(64),
        provenance: LightningProvenance {
            provider: "Synthetic test".into(),
            source: "Fabricated point and clock".into(),
            license: None,
            notes: None,
            altitude_datum: LightningAltitudeDatum::Wgs84Ellipsoid,
            source_metadata: None,
        },
        strikes: vec![strike],
    });
    (document, map, lightning, observer)
}

#[test]
fn source_assessment_blocks_selected_active_events_without_dropping_them() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    for blocked in [
        Assessment::Unassessed,
        Assessment::Incompatible,
        Assessment::UncertainHypothesis {
            rationale: " \n\t ".into(),
        },
    ] {
        let (mut doc, map, lightning, observer) = fixture();
        doc.events[0].acoustic_source = blocked;
        let before = serde_json::to_vec(&doc).unwrap();
        let error = workspace_scoring::score_observers(&doc, &map, &lightning, None, &[observer])
            .unwrap_err();
        assert!(
            error.contains("Acoustic source assessment blocks scoring"),
            "{error}"
        );
        assert!(error.contains("aircraft-event"));
        assert_eq!(serde_json::to_vec(&doc).unwrap(), before);
        assert_eq!(doc.active_scoring_events().count(), 2);
        assert!(doc.events[0].selected_for_scoring);
    }
}

#[test]
fn source_gate_respects_disabled_terms_and_unselected_events_both_directions() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    for disabled_kind in [EventKind::Doppler, EventKind::Thunder] {
        let (mut doc, map, lightning, observer) = fixture();
        let index = if disabled_kind == EventKind::Doppler {
            0
        } else {
            1
        };
        doc.events[index].acoustic_source = Assessment::Incompatible;
        if disabled_kind == EventKind::Doppler {
            doc.mix.doppler_frequency = false;
            doc.mix.doppler_timing = false;
        } else {
            doc.mix.thunder_timing = false;
        }
        let report = workspace_scoring::score_observers(
            &doc,
            &map,
            &lightning,
            None,
            std::slice::from_ref(&observer),
        )
        .unwrap();
        assert!(report.candidates[0].score.is_some());
        assert!(doc.events[index].selected_for_scoring);
        // Explicitly deselecting the same row also permits other active evidence.
        doc.mix.doppler_frequency = true;
        doc.mix.doppler_timing = true;
        doc.mix.thunder_timing = true;
        // Keep every enabled term supplied by a different explicit synthetic
        // observation; deselecting the only event must not fabricate support.
        let mut replacement = doc.events[index].clone();
        replacement.id = format!("replacement-{}", replacement.id);
        replacement.acoustic_source = Assessment::Compatible;
        doc.events.push(replacement);
        doc.events[index].selected_for_scoring = false;
        let report =
            workspace_scoring::score_observers(&doc, &map, &lightning, None, &[observer]).unwrap();
        assert!(report.candidates[0].score.is_some());
    }
}

#[test]
fn explicit_uncertain_source_preserves_numerical_score_and_records_assumption() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    let (mut doc, map, lightning, observer) = fixture();
    let baseline = workspace_scoring::score_observers(
        &doc,
        &map,
        &lightning,
        None,
        std::slice::from_ref(&observer),
    )
    .unwrap();
    doc.events[0].acoustic_source = Assessment::UncertainHypothesis {
        rationale: "Synthetic source-label uncertainty control; allow a competing non-aircraft interpretation".into(),
    };
    let before = serde_json::to_vec(&doc).unwrap();
    let report =
        workspace_scoring::score_observers(&doc, &map, &lightning, None, &[observer]).unwrap();
    assert_eq!(
        serde_json::to_value(&report.candidates).unwrap(),
        serde_json::to_value(&baseline.candidates).unwrap()
    );
    assert!(report.limitations[0].contains("UNCERTAIN ACOUSTIC SOURCE"));
    assert!(report.limitations[0].contains("competing non-aircraft interpretation"));
    assert!(!report.location_verified);
    assert_eq!(serde_json::to_vec(&doc).unwrap(), before);
}

#[test]
fn actual_aircraft_predictions_and_thunder_timing_recover_the_synthetic_location() {
    let (doc, map, lightning, observer) = fixture();
    let wrong = ObserverLocation {
        latitude_deg: 0.02,
        longitude_deg: -0.01,
        ellipsoid_height_m: 0.0,
    };
    let r = workspace_scoring::score_observers(&doc, &map, &lightning, None, &[wrong, observer])
        .unwrap();
    assert_eq!(r.ranked_candidate_ids[0], "receiver-00001");
    assert!(r.candidates[1].score.unwrap() < 1e-10);
    assert!(r.candidates[0].score.unwrap() > 1.0);
    assert_eq!(r.candidates[1].terms.len(), 3);
    assert!(!r.location_verified);
    assert!(r.candidates[1].unavailable_reasons.is_empty());
}

#[test]
fn pure_frequency_uses_selected_f0_without_silent_fitting() {
    let (mut doc, map, lightning, observer) = fixture();
    doc.mix.doppler_timing = false;
    doc.mix.thunder_timing = false;
    let r = workspace_scoring::score_observers(
        &doc,
        &map,
        &lightning,
        None,
        std::slice::from_ref(&observer),
    )
    .unwrap();
    assert!(r.candidates[0].score.unwrap() < 1e-20);
    doc.events[0].emitted_frequency_hz = 120.0;
    let r = workspace_scoring::score_observers(&doc, &map, &lightning, None, &[observer]).unwrap();
    assert!(r.candidates[0].score.unwrap() > 20.0);
    let predicted = &r.candidates[0].candidate.frequency;
    for (p, o) in predicted.iter().zip(&doc.events[0].points) {
        assert!((p.predicted_hz / o.frequency_hz - 120.0 / 110.0).abs() < 1e-12);
    }
}

#[test]
fn shared_clock_is_applied_to_aircraft_and_thunder_without_per_event_adjustment() {
    let (mut doc, map, lightning, observer) = fixture();
    doc.map.clock_adjustment_s = 3.0;
    let r = workspace_scoring::score_observers(&doc, &map, &lightning, None, &[observer]).unwrap();
    let terms = &r.candidates[0].terms;
    assert!((terms[1].residuals[0].residual + 3.0).abs() < 1e-6);
    assert!((terms[2].residuals[0].residual + 3.0).abs() < 1e-10);
    assert!(terms[0].mean_normalized_squared_residual.unwrap() > 0.1);
}

#[test]
fn explicit_utc_and_gap_contracts_are_required_for_aircraft_scoring() {
    let (doc, mut map, lightning, observer) = fixture();
    map.tracks[0].time_origin_utc_s = None;
    assert!(
        workspace_scoring::score_observers(
            &doc,
            &map,
            &lightning,
            None,
            std::slice::from_ref(&observer)
        )
        .unwrap_err()
        .contains("UTC origin")
    );
    map.tracks[0].time_origin_utc_s = Some(1_788_825_600.0);
    map.tracks[0].max_gap_s = None;
    assert!(
        workspace_scoring::score_observers(&doc, &map, &lightning, None, &[observer])
            .unwrap_err()
            .contains("interpolation gap")
    );
}

#[test]
fn missing_track_support_makes_candidate_unrankable_without_clipping_observations() {
    let (doc, mut map, lightning, observer) = fixture();
    map.tracks[0].physics_track.points.drain(4..7);
    map.tracks[0].segments = vec![[0, 4], [4, 8]];
    let r = workspace_scoring::score_observers(&doc, &map, &lightning, None, &[observer]).unwrap();
    assert_eq!(r.candidates[0].score, None);
    assert!(r.ranked_candidate_ids.is_empty());
    assert!(
        r.candidates[0]
            .unavailable_reasons
            .iter()
            .any(|x| x.contains("coverage"))
    );
    assert!(!r.candidates[0].terms[0].available);
}

#[test]
fn automatic_or_unselected_events_do_not_enter_scoring() {
    let (mut doc, map, lightning, observer) = fixture();
    doc.events[0].review = ReviewState::Pending;
    assert!(
        workspace_scoring::score_observers(
            &doc,
            &map,
            &lightning,
            None,
            std::slice::from_ref(&observer)
        )
        .is_err()
    );
    doc.events[0].selected_for_scoring = false;
    doc.mix.doppler_frequency = false;
    doc.mix.doppler_timing = false;
    let r = workspace_scoring::score_observers(&doc, &map, &lightning, None, &[observer]).unwrap();
    assert_eq!(r.selected_terms, vec![EvidenceTerm::ThunderTiming]);
    assert!(r.candidates[0].score.unwrap() < 1e-20);
}

#[test]
fn bounded_grid_covers_edges_and_runs_real_scoring() {
    let (doc, map, lightning, _) = fixture();
    let grid = workspace_scoring::grid_observers(&doc.map).unwrap();
    assert_eq!(grid.len(), 9);
    assert_eq!(grid[0].latitude_deg, -0.01);
    assert_eq!(grid[8].longitude_deg, 0.01);
    let r = workspace_scoring::score_grid(&doc, &map, &lightning, None).unwrap();
    assert_eq!(r.candidates.len(), 9);
    assert_eq!(r.ranked_candidate_ids[0], "receiver-00004");
    let huge = MapSelection {
        bounds: [-40.0, -120.0, 40.0, 120.0],
        grid_spacing_m: 10.0,
        ..Default::default()
    };
    assert!(
        workspace_scoring::grid_observers(&huge)
            .unwrap_err()
            .contains("exceeds")
    );
}

#[test]
fn large_residual_product_is_rejected_before_computation() {
    let (doc, map, lightning, observer) = fixture();
    let observers = vec![observer; workspace_scoring::MAX_OBSERVERS];
    let mut doc = doc;
    let original = doc.events[0].points.clone();
    doc.events[0].points = (0..100)
        .map(|i| geosonnar::workspace::TracePoint {
            time_s: 30.0 + i as f64 * 0.5,
            frequency_hz: original[0].frequency_hz,
        })
        .collect();
    assert!(
        workspace_scoring::score_observers(&doc, &map, &lightning, None, &observers)
            .unwrap_err()
            .contains("residuals")
    );
}

fn uniform_profile(doc: &ReviewWorkspace) -> geosonnar::atmosphere::AtmosphereProfile {
    use geosonnar::atmosphere::*;
    let mut profile: AtmosphereProfile = serde_json::from_str(include_str!(
        "../examples/synthetic-atmosphere-profile.json"
    ))
    .unwrap();
    let origin = geosonnar::workspace::parse_utc(&doc.recording_start_utc).unwrap();
    profile.valid_from_utc_s = origin - 100.0;
    profile.valid_to_utc_s = origin + 1000.0;
    let gas = 8.314_462_618_153_24;
    let gamma = 29.12 / (29.12 - gas);
    let temperature = doc.sound_speed_mps.powi(2) * 0.028_965_46 / (gamma * gas);
    profile.levels = [-100.0, 10_000.0]
        .into_iter()
        .map(|height| AtmosphereLevel {
            ellipsoid_height_m: height,
            temperature_k: temperature,
            pressure_pa: 101_325.0,
            relative_humidity_fraction: 0.0,
            eastward_wind_mps: 0.0,
            northward_wind_mps: 0.0,
        })
        .collect();
    profile
}

#[test]
fn optional_uniform_atmosphere_preserves_joint_physics_and_diagnostics() {
    let (mut doc, map, lightning, observer) = fixture();
    let baseline = workspace_scoring::score_observers(
        &doc,
        &map,
        &lightning,
        None,
        std::slice::from_ref(&observer),
    )
    .unwrap();
    doc.atmosphere = Some(uniform_profile(&doc));
    let result =
        workspace_scoring::score_observers(&doc, &map, &lightning, None, &[observer]).unwrap();
    assert!(result.config.atmosphere.is_some());
    assert!(result.candidates[0].score.unwrap() < 1e-8);
    let diagnostic = result.modalities.as_ref().unwrap();
    assert_eq!(diagnostic.bounds, Some(doc.map.bounds));
    assert_eq!(
        diagnostic.comparison.status,
        geosonnar::modality_diagnostics::ComparisonStatus::SharedBest
    );
    for (current, prior) in result.candidates[0]
        .candidate
        .frequency
        .iter()
        .zip(&baseline.candidates[0].candidate.frequency)
    {
        assert!((current.predicted_hz - prior.predicted_hz).abs() < 0.0003);
    }
    let serialized = serde_json::to_vec(&doc).unwrap();
    assert_eq!(
        serde_json::from_slice::<ReviewWorkspace>(&serialized).unwrap(),
        doc
    );
}

#[test]
fn uncovered_atmosphere_is_unavailable_for_both_modalities_without_fallback() {
    let (mut doc, map, lightning, observer) = fixture();
    let mut profile = uniform_profile(&doc);
    profile.valid_from_utc_s += 86_400.0;
    profile.valid_to_utc_s += 86_400.0;
    doc.atmosphere = Some(profile);
    let result =
        workspace_scoring::score_observers(&doc, &map, &lightning, None, &[observer]).unwrap();
    assert!(result.ranked_candidate_ids.is_empty());
    let candidate = &result.candidates[0];
    assert!(candidate.score.is_none());
    assert!(candidate.terms.iter().all(|term| !term.available));
    assert!(
        candidate
            .unavailable_reasons
            .iter()
            .any(|message| message.contains("coverage"))
    );
    assert_eq!(
        result.modalities.unwrap().comparison.status,
        geosonnar::modality_diagnostics::ComparisonStatus::NotComparable
    );
}

#[test]
fn profiles_have_a_smaller_explicit_work_budget_and_old_reviews_still_load() {
    let (mut doc, map, lightning, observer) = fixture();
    let json = serde_json::to_value(&doc).unwrap();
    assert!(json.get("atmosphere").is_none());
    assert!(
        serde_json::from_value::<ReviewWorkspace>(json)
            .unwrap()
            .atmosphere
            .is_none()
    );
    doc.atmosphere = Some(uniform_profile(&doc));
    let observers = vec![observer; workspace_scoring::MAX_ATMOSPHERE_OBSERVERS + 1];
    assert!(
        workspace_scoring::score_observers(&doc, &map, &lightning, None, &observers)
            .unwrap_err()
            .contains("400 receivers")
    );
}
