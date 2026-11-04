//! Independently calculated moving-source observations and inspection controls.
use geosonnar::{
    atmosphere::{
        AtmosphereHeightDatum, AtmosphereLevel, AtmosphereProfile, AtmosphereProvenance,
        HumidityReference, IntegrationSettings,
    },
    evidence_score::{
        EvidenceTerm, LightningAltitudeDatum, LightningCatalog, LightningProvenance,
        LightningStrike, ObserverLocation,
    },
    map_data::{self, MapData},
    physics::{self, Track, TrackPoint},
    prediction_overlay::{self as overlay, OverlaySettings},
    workspace::{
        EventKind, EvidenceEvent, EvidenceMix, MapSelection, ReviewState, ReviewWorkspace,
    },
    workspace_scoring,
};
use serde_json::json;
use std::path::PathBuf;

const ORIGIN: f64 = 1_704_067_200.0;
const F0: f64 = 120.0;
const SPEED: f64 = 100.0;
const RANGE: f64 = 1000.0;

/// Retarded-time quadratic for x=v(t-50), offset range=1000; no production
/// propagation calls are used to generate observed frequencies.
fn oracle(arrival: f64, c: f64) -> f64 {
    let x = arrival - 50.0;
    let c2 = c * c;
    let v2 = SPEED * SPEED;
    let emission_from_cpa =
        (c2 * x - (c2 * v2 * x * x + (c2 - v2) * RANGE * RANGE).sqrt()) / (c2 - v2);
    let range = (RANGE * RANGE + v2 * emission_from_cpa * emission_from_cpa).sqrt();
    F0 / (1.0 + v2 * emission_from_cpa / (c * range))
}

fn fixture() -> (
    ReviewWorkspace,
    MapData,
    Option<LightningCatalog>,
    ObserverLocation,
) {
    let observer = ObserverLocation {
        latitude_deg: 0.0,
        longitude_deg: 0.0,
        ellipsoid_height_m: 0.0,
    };
    let receiver = physics::geodetic_to_ecef(0., 0., 0.).unwrap();
    let track = Track {
        flight_id: "test-aircraft".into(),
        points: [0., 120.]
            .into_iter()
            .map(|t| TrackPoint {
                t_s: t,
                xyz_m: [receiver[0] + RANGE, SPEED * (t - 50.), 0.],
            })
            .collect(),
    };
    let map = map_data::parse_value(
        &json!({"config":{"time_origin_utc_s":ORIGIN,"max_gap_s":130.0},"tracks":[track]}),
    )
    .unwrap();
    let mut event = EvidenceEvent::manual("tone".into(), EventKind::Doppler, 30., 100.);
    event.selected_for_scoring = true;
    event.review = ReviewState::Confirmed;
    // Fabricated physics control: source class is specified by its generator.
    event.acoustic_source = geosonnar::workspace::AcousticSourceAssessment::Compatible;
    event.flight_id = "test-aircraft".into();
    event.emitted_frequency_hz = F0;
    event.arrival_s = 50. + RANGE / 343.;
    event.frequency_sigma_hz = 1.5;
    event.points = [30., 40.123456, 55., 80., 100.]
        .into_iter()
        .map(|time_s| geosonnar::workspace::TracePoint {
            time_s,
            frequency_hz: oracle(time_s, 343.) + 2.0,
        })
        .collect();
    let strike = LightningStrike {
        id: "test-strike".into(),
        time_utc_s: ORIGIN + 40.,
        latitude_deg: 0.,
        longitude_deg: 0.,
        altitude_m: Some(3430.),
        intensity: None,
        source_record: None,
    };
    let lightning = Some(LightningCatalog {
        schema_version: 1,
        input_path: PathBuf::from("synthetic-strikes.json"),
        input_bytes: 0,
        input_sha256: "a".repeat(64),
        provenance: LightningProvenance {
            provider: "synthetic fixture".into(),
            source: "independent ten-second vertical travel".into(),
            license: None,
            notes: None,
            altitude_datum: LightningAltitudeDatum::Wgs84Ellipsoid,
            source_metadata: None,
        },
        strikes: vec![strike],
    });
    let mut thunder = EvidenceEvent::manual("thunder".into(), EventKind::Thunder, 40., 60.);
    thunder.review = ReviewState::Confirmed;
    // Fabricated physics control: source class is specified by its generator.
    thunder.acoustic_source = geosonnar::workspace::AcousticSourceAssessment::Compatible;
    thunder.selected_for_scoring = true;
    thunder.arrival_s = 51.;
    thunder.uncertainty_s = 2.;
    thunder.strike_id = "test-strike".into();
    let document = ReviewWorkspace {
        schema_version: 1,
        case_id: "overlay-fixture".into(),
        recording_sha256: "0".repeat(64),
        channel: 0,
        duration_s: 150.,
        recording_start_utc: "2024-01-01 00:00:00".into(),
        sound_speed_mps: 343.,
        events: vec![event, thunder],
        atmosphere: None,
        regional_plan: None,
        regional_comparison: None,
        envelope_review: None,
        receiver_terrain: None,
        notes: "independent synthetic control".into(),
        mix: EvidenceMix {
            doppler_frequency: true,
            doppler_timing: true,
            thunder_timing: true,
            frequency_scale_hz: 2.0,
            thunder_scale_s: 1.5,
            ..Default::default()
        },
        map: MapSelection {
            bounds: [-0.1, -0.1, 0.1, 0.1],
            grid_spacing_m: 1000.,
            receiver_height_m: 0.,
            clock_adjustment_s: 0.,
        },
        datasets: vec![],
        dataset_options: vec![],
        strike_fallback_height_m: None,
    };
    (document, map, lightning, observer)
}

#[test]
fn sampled_curve_matches_independent_retarded_time_oracle_and_exact_fixed_residuals() {
    let (doc, map, lightning, receiver) = fixture();
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    let fixed = workspace_scoring::score_observers(
        &doc,
        &map,
        &lightning,
        None,
        std::slice::from_ref(&receiver),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&result.score_report).unwrap(),
        serde_json::to_value(&fixed).unwrap()
    );
    let frequency = &result.frequency[0];
    assert_eq!(frequency.segments.len(), 1);
    assert!(result.unavailable.is_empty());
    assert!(!result.location_verified);
    let samples = &frequency.segments[0].samples;
    assert!(samples.len() > 100);
    assert_eq!(samples.first().unwrap().time_s, 30.);
    assert_eq!(samples.last().unwrap().time_s, 100.);
    for sample in samples {
        assert!((sample.frequency_hz - oracle(sample.time_s, 343.)).abs() < 1e-10);
        assert_eq!(sample.arrival_utc_s, ORIGIN + sample.time_s);
    }
    let measured: Vec<_> = samples
        .iter()
        .filter(|s| s.measured_point_index.is_some())
        .collect();
    assert_eq!(measured.len(), doc.events[0].points.len());
    for sample in measured {
        let index = sample.measured_point_index.unwrap();
        assert_eq!(sample.time_s, doc.events[0].points[index].time_s);
        assert_eq!(sample.effective_residual_scale_hz, Some(2.5));
        assert!((sample.normalized_residual.unwrap() + 0.8).abs() < 1e-10);
    }
    assert_eq!(result.frequency_sample_count, samples.len());
}

#[test]
fn thunder_and_cpa_markers_use_exact_report_timing_and_observation_uncertainty() {
    let (doc, map, lightning, receiver) = fixture();
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    let thunder = result
        .timing
        .iter()
        .find(|m| m.kind == EventKind::Thunder)
        .unwrap();
    assert_eq!(thunder.predicted_arrival_s, 50.);
    assert_eq!(thunder.predicted_arrival_utc_s, ORIGIN + 50.);
    assert_eq!(thunder.observed_arrival_s, 51.);
    assert_eq!(thunder.observed_uncertainty_s, 2.);
    assert_eq!(thunder.effective_residual_scale_s, 2.5);
    assert_eq!(thunder.normalized_residual, -0.4);
    assert_eq!(thunder.strike_height_assumed, Some(false));
    for marker in &result.timing {
        let term = if marker.kind == EventKind::Thunder {
            EvidenceTerm::ThunderTiming
        } else {
            EvidenceTerm::DopplerTiming
        };
        let residual = &result.score_report.candidates[0]
            .terms
            .iter()
            .find(|t| t.term == term)
            .unwrap()
            .residuals[0];
        assert_eq!(
            marker.predicted_arrival_s.to_bits(),
            residual.predicted.to_bits()
        );
        assert_eq!(
            marker.normalized_residual.to_bits(),
            residual.normalized_residual.to_bits()
        );
    }
}

#[test]
fn one_shared_clock_shifts_frequency_and_both_arrival_markers_without_mutation() {
    let (mut doc, map, lightning, receiver) = fixture();
    doc.map.clock_adjustment_s = 3.25;
    let before = serde_json::to_vec(&(&doc, &map, &lightning)).unwrap();
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert_eq!(result.clock_adjustment_s, 3.25);
    assert_eq!(result.adjusted_recording_start_utc_s, ORIGIN + 3.25);
    for sample in result.frequency[0].segments.iter().flat_map(|s| &s.samples) {
        assert!((sample.frequency_hz - oracle(sample.time_s + 3.25, 343.)).abs() < 1e-10);
    }
    assert_eq!(
        result
            .timing
            .iter()
            .find(|m| m.kind == EventKind::Thunder)
            .unwrap()
            .predicted_arrival_s,
        46.75
    );
    assert_eq!(
        serde_json::to_vec(&(&doc, &map, &lightning)).unwrap(),
        before
    );
}

#[test]
fn original_gaps_remain_separate_and_unsupported_measurements_are_explicit() {
    let (mut doc, mut map, lightning, receiver) = fixture();
    doc.mix.doppler_timing = false;
    let x = physics::geodetic_to_ecef(0., 0., 0.).unwrap()[0];
    let times = [0., 35., 65., 120.];
    map.tracks[0].physics_track.points = times
        .into_iter()
        .map(|t| TrackPoint {
            t_s: t,
            xyz_m: [x + RANGE, SPEED * (t - 50.), 0.],
        })
        .collect();
    map.tracks[0].segments = vec![[0, 2], [2, 4]];
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert!(result.score_report.candidates[0].score.is_none());
    let pieces = &result.frequency[0].segments;
    assert_eq!(pieces.len(), 2);
    assert_eq!(pieces[0].support_segment_index, 0);
    assert_eq!(pieces[1].support_segment_index, 1);
    assert!(pieces[0].samples.last().unwrap().time_s < pieces[1].samples[0].time_s);
    let gaps: Vec<_> = result
        .unavailable
        .iter()
        .filter(|g| g.event_id == "tone" && g.measured_point_index.is_none())
        .collect();
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].start_s.unwrap() < 55. && gaps[0].end_s.unwrap() > 55.);
    assert!(
        result
            .unavailable
            .iter()
            .any(|g| g.measured_point_index == Some(2))
    );
    assert!(
        pieces
            .iter()
            .flat_map(|s| &s.samples)
            .any(|s| s.measured_point_index == Some(0))
    );
    assert!(
        pieces
            .iter()
            .flat_map(|s| &s.samples)
            .all(|s| s.normalized_residual.is_none())
    );
    let diagnostics = result.frequency[0].fit_diagnostics.as_ref().unwrap();
    assert_eq!(diagnostics.observed_points, doc.events[0].points.len());
    assert!(diagnostics.comparison.is_none());
    assert!(diagnostics.unavailable_reason.is_some());
}

#[test]
fn velocity_knots_are_not_joined_even_when_position_support_is_continuous() {
    let (mut doc, mut map, lightning, receiver) = fixture();
    doc.mix.doppler_timing = false;
    let x = physics::geodetic_to_ecef(0., 0., 0.).unwrap()[0];
    map.tracks[0].physics_track.points = vec![
        TrackPoint {
            t_s: 0.,
            xyz_m: [x + RANGE, -5000., 0.],
        },
        TrackPoint {
            t_s: 60.,
            xyz_m: [x + RANGE, 1000., 0.],
        },
        TrackPoint {
            t_s: 120.,
            xyz_m: [x + RANGE, 1000., 0.],
        },
    ];
    map.tracks[0].segments = vec![[0, 3]];
    let knot = 60. + 2.0_f64.sqrt() * 1000. / 343.;
    doc.events[0].points.push(geosonnar::workspace::TracePoint {
        time_s: knot,
        frequency_hz: F0,
    });
    doc.events[0]
        .points
        .sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    let pieces = &result.frequency[0].segments;
    assert_eq!(pieces.len(), 2);
    assert_eq!(
        pieces[0].support_segment_index,
        pieces[1].support_segment_index
    );
    assert_ne!(
        pieces[0].trajectory_edge_index,
        pieces[1].trajectory_edge_index
    );
    assert!(pieces[0].samples.last().unwrap().time_s < knot);
    assert_eq!(pieces[1].samples[0].time_s, knot);
    assert!(
        (pieces[0].samples.last().unwrap().frequency_hz - pieces[1].samples[0].frequency_hz).abs()
            > 10.
    );
    let measured: Vec<_> = pieces
        .iter()
        .flat_map(|s| &s.samples)
        .filter(|s| s.measured_point_index.is_some())
        .collect();
    assert_eq!(measured.len(), doc.events[0].points.len());
}

#[test]
fn overlapping_arrival_support_is_not_arbitrarily_resolved() {
    let (mut doc, mut map, lightning, receiver) = fixture();
    doc.mix.doppler_timing = false;
    let first = map.tracks[0].physics_track.points.clone();
    map.tracks[0].physics_track.points.extend(first);
    map.tracks[0].segments = vec![[0, 2], [2, 4]];
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert!(result.frequency[0].segments.is_empty());
    assert_eq!(result.frequency_sample_count, 0);
    assert!(
        result
            .unavailable
            .iter()
            .any(|u| u.reason.contains("unique"))
    );
    assert!(result.score_report.candidates[0].score.is_none());
}

fn dry_profile() -> AtmosphereProfile {
    AtmosphereProfile {
        schema_version: 1,
        name: "independent uniform dry air".into(),
        provenance: AtmosphereProvenance {
            provider: "synthetic".into(),
            source: "constant analytic atmosphere".into(),
            source_sha256: "b".repeat(64),
            notes: "not weather observations".into(),
        },
        valid_from_utc_s: ORIGIN - 10.,
        valid_to_utc_s: ORIGIN + 1000.,
        bounds: [-0.2, -0.2, 0.2, 0.2],
        height_datum: AtmosphereHeightDatum::Wgs84Ellipsoid,
        humidity_reference: HumidityReference::LiquidWater,
        levels: [-1000., 5000.]
            .into_iter()
            .map(|ellipsoid_height_m| AtmosphereLevel {
                ellipsoid_height_m,
                temperature_k: 293.15,
                pressure_pa: 101325.,
                relative_humidity_fraction: 0.,
                eastward_wind_mps: 0.,
                northward_wind_mps: 0.,
            })
            .collect(),
        integration: IntegrationSettings::default(),
    }
}

#[test]
fn atmosphere_uses_native_profile_predictions_and_uniform_analytic_limit() {
    let (mut doc, map, lightning, receiver) = fixture();
    doc.atmosphere = Some(dry_profile());
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings {
            target_step_s: 10.,
            max_frequency_samples: 200,
        },
    )
    .unwrap();
    let r = 8.31446261815324;
    let cp = 29.12;
    let c: f64 = ((cp / (cp - r)) * r / 0.02896546 * 293.15_f64).sqrt();
    assert!(result.model_label.contains("Stratified"));
    let worst = result.frequency[0]
        .segments
        .iter()
        .flat_map(|p| &p.samples)
        .max_by(|a, b| {
            (a.frequency_hz - oracle(a.time_s, c))
                .abs()
                .total_cmp(&(b.frequency_hz - oracle(b.time_s, c)).abs())
        })
        .unwrap();
    let error_hz = (worst.frequency_hz - oracle(worst.time_s, c)).abs();
    // Native profile Doppler uses numerical differentiation/inversion. Its
    // existing uniform-profile contract is 2e-6 in dimensionless frequency
    // factor, so the corresponding frequency tolerance is emitted tone ×2e-6.
    // The fixed-scorer agreement at measured points remains bit-for-bit exact.
    eprintln!(
        "Uniform profile overlay: max error={error_hz:.12e} Hz at t={:.9}; expected={:.12}; actual={:.12}; factor error={:.12e}",
        worst.time_s,
        oracle(worst.time_s, c),
        worst.frequency_hz,
        error_hz / F0
    );
    assert!(
        error_hz < F0 * 2e-6,
        "uniform-profile error={error_hz}Hz at t={} exceeds native factor tolerance after unit conversion",
        worst.time_s
    );
    let thunder = result
        .timing
        .iter()
        .find(|m| m.kind == EventKind::Thunder)
        .unwrap();
    assert!((thunder.predicted_arrival_s - (40. + 3430. / c)).abs() < 1e-8);
    assert!(
        result.frequency[0]
            .segments
            .iter()
            .flat_map(|p| &p.samples)
            .filter(|p| p.measured_point_index.is_some())
            .all(|p| p.normalized_residual.is_some())
    );
}

#[test]
fn missing_profile_or_strike_height_never_draws_a_fallback_prediction() {
    let (mut doc, map, mut lightning, receiver) = fixture();
    lightning.as_mut().unwrap().provenance.altitude_datum = LightningAltitudeDatum::Unknown;
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert!(result.timing.iter().all(|m| m.kind != EventKind::Thunder));
    assert!(
        result
            .unavailable
            .iter()
            .any(|m| m.event_id == "thunder" && m.reason.contains("height"))
    );
    let explicit = overlay::build(
        &doc,
        &map,
        &lightning,
        Some(3430.),
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert_eq!(
        explicit
            .timing
            .iter()
            .find(|m| m.kind == EventKind::Thunder)
            .unwrap()
            .strike_height_assumed,
        Some(true)
    );
    let mut profile = dry_profile();
    profile.valid_from_utc_s = ORIGIN + 200.;
    doc.atmosphere = Some(profile);
    let unsupported = overlay::build(
        &doc,
        &map,
        &lightning,
        Some(3430.),
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert!(unsupported.frequency[0].segments.is_empty());
    assert!(unsupported.timing.is_empty());
    assert!(unsupported.unavailable.iter().any(|m| m.event_id == "tone"));
}

#[test]
fn display_budget_coarsens_sampling_but_preserves_every_exact_measured_point() {
    let (doc, map, lightning, receiver) = fixture();
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings {
            target_step_s: 0.001,
            max_frequency_samples: 20,
        },
    )
    .unwrap();
    assert!(result.sampling_coarsened);
    assert!(result.frequency_sample_count <= 20);
    let indices: Vec<_> = result.frequency[0]
        .segments
        .iter()
        .flat_map(|s| &s.samples)
        .filter_map(|s| s.measured_point_index)
        .collect();
    assert_eq!(indices, vec![0, 1, 2, 3, 4]);
    assert!(
        overlay::build(
            &doc,
            &map,
            &lightning,
            None,
            &receiver,
            &OverlaySettings {
                target_step_s: 1.,
                max_frequency_samples: 4
            }
        )
        .is_err()
    );
    assert!(
        overlay::build(
            &doc,
            &map,
            &lightning,
            None,
            &receiver,
            &OverlaySettings {
                target_step_s: f64::NAN,
                max_frequency_samples: 20
            }
        )
        .is_err()
    );
}

#[test]
fn exact_supported_endpoint_is_retained_before_a_trailing_unavailable_interval() {
    let (mut doc, mut map, lightning, receiver) = fixture();
    doc.mix.doppler_timing = false;
    let x = physics::geodetic_to_ecef(0., 0., 0.).unwrap()[0];
    let endpoint = 65.;
    map.tracks[0].physics_track.points[1] = TrackPoint {
        t_s: endpoint,
        xyz_m: [x + RANGE, SPEED * (endpoint - 50.), 0.],
    };
    let arrival = endpoint
        + (RANGE * RANGE + SPEED * SPEED * (endpoint - 50.) * (endpoint - 50.)).sqrt() / 343.;
    doc.events[0].points = vec![geosonnar::workspace::TracePoint {
        time_s: arrival,
        frequency_hz: oracle(arrival, 343.),
    }];
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert_eq!(
        result.frequency[0]
            .segments
            .iter()
            .flat_map(|p| &p.samples)
            .filter(|s| s.measured_point_index.is_some())
            .count(),
        1
    );
    assert!(
        result
            .unavailable
            .iter()
            .any(|u| u.start_s.is_some_and(|t| t >= arrival - 1e-10))
    );
}

#[test]
fn inspection_identities_change_with_review_and_source_buffers_and_output_is_deterministic() {
    let (doc, map, lightning, receiver) = fixture();
    let one = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    let two = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&one).unwrap(),
        serde_json::to_vec(&two).unwrap()
    );
    let mut changed = doc.clone();
    changed.notes.push_str(" edited");
    let other = overlay::build(
        &changed,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert_ne!(one.document_sha256, other.document_sha256);
    let mut changed = map.clone();
    changed.tracks[0].physics_track.points[0].xyz_m[2] += 1.;
    let other = overlay::build(
        &doc,
        &changed,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert_ne!(one.inputs_sha256, other.inputs_sha256);
}

#[test]
fn unselected_or_pending_events_are_not_rendered_as_inspected_evidence() {
    let (mut doc, map, lightning, receiver) = fixture();
    doc.mix.thunder_timing = false;
    doc.events[1].selected_for_scoring = false;
    doc.events[1].review = ReviewState::Pending;
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert_eq!(result.frequency.len(), 1);
    assert_eq!(result.timing.len(), 1);
    assert_eq!(result.timing[0].kind, EventKind::Doppler);
}

#[test]
fn fragmented_support_inventory_is_bounded_before_physics_or_rendering() {
    let (doc, mut map, lightning, receiver) = fixture();
    let points = map.tracks[0].physics_track.points.clone();
    map.tracks[0].physics_track.points.clear();
    map.tracks[0].segments.clear();
    for i in 0..1000 {
        map.tracks[0].physics_track.points.extend(points.clone());
        map.tracks[0].segments.push([2 * i, 2 * i + 2]);
    }
    let error = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap_err();
    assert!(error.contains("support-boundary checks"));
}

#[test]
fn inspection_blocks_unassessed_active_sources_without_promoting_observations() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    let (mut doc, map, lightning, receiver) = fixture();
    doc.events[1].acoustic_source = Assessment::Unassessed;
    let before = serde_json::to_vec(&doc).unwrap();
    let error = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap_err();
    assert!(error.contains("thunder"));
    assert!(error.contains("unassessed"));
    assert_eq!(serde_json::to_vec(&doc).unwrap(), before);
    doc.mix.thunder_timing = false;
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert!(
        result
            .timing
            .iter()
            .all(|marker| marker.kind == EventKind::Doppler)
    );
    assert_eq!(doc.events[1].acoustic_source, Assessment::Unassessed);
}

#[test]
fn uncertain_thunder_and_doppler_keep_exact_predictions_with_explicit_source_warnings() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    let (mut doc, map, lightning, receiver) = fixture();
    let baseline = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    for event in &mut doc.events {
        event.acoustic_source = Assessment::UncertainHypothesis {
            rationale: format!(
                "Synthetic {} class-uncertainty control; source identity is not established",
                event.id
            ),
        };
    }
    let before = serde_json::to_vec(&doc).unwrap();
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&result.frequency).unwrap(),
        serde_json::to_value(&baseline.frequency).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&result.timing).unwrap(),
        serde_json::to_value(&baseline.timing).unwrap()
    );
    assert_ne!(result.document_sha256, baseline.document_sha256);
    for event in &doc.events {
        assert!(
            result
                .limitations
                .iter()
                .any(|warning| warning.contains("UNCERTAIN ACOUSTIC SOURCE")
                    && warning.contains(&event.id))
        );
        assert!(
            result
                .score_report
                .limitations
                .iter()
                .any(|warning| warning.contains("UNCERTAIN ACOUSTIC SOURCE")
                    && warning.contains(&event.id))
        );
    }
    assert!(!result.location_verified);
    assert!(!result.score_report.location_verified);
    assert_eq!(serde_json::to_vec(&doc).unwrap(), before);
}

#[test]
fn disabled_modality_rows_do_not_consume_active_overlay_capacity() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    let (mut doc, map, lightning, receiver) = fixture();
    doc.mix.thunder_timing = false;
    doc.events[1].acoustic_source = Assessment::Unassessed;
    for index in 1..32 {
        let mut extra = doc.events[1].clone();
        extra.id = format!("disabled-thunder-{index}");
        doc.events.push(extra);
    }
    assert_eq!(doc.events.len(), 33);
    let before = serde_json::to_vec(&doc).unwrap();
    let result = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    assert_eq!(result.frequency.len(), 1);
    assert_eq!(result.timing.len(), 1);
    assert_eq!(result.timing[0].kind, EventKind::Doppler);
    assert!(result.unavailable.is_empty());
    assert_eq!(serde_json::to_vec(&doc).unwrap(), before);
    doc.mix.thunder_timing = true;
    for event in &mut doc.events {
        event.acoustic_source = Assessment::Compatible;
    }
    let error = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap_err();
    assert!(error.contains("32"), "{error}");
}

#[test]
fn fit_comparison_uses_only_exact_measurements_and_never_changes_fixed_scoring() {
    let (doc, map, lightning, receiver) = fixture();
    let before = serde_json::to_vec(&(&doc, &map, &lightning)).unwrap();
    let dense = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings {
            target_step_s: 0.05,
            max_frequency_samples: 4000,
        },
    )
    .unwrap();
    let sparse = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings {
            target_step_s: 30.,
            max_frequency_samples: 20,
        },
    )
    .unwrap();
    assert!(dense.frequency_sample_count > sparse.frequency_sample_count * 10);
    let diagnostics = dense.frequency[0].fit_diagnostics.as_ref().unwrap();
    assert_eq!(diagnostics.observed_points, 5);
    assert_eq!(diagnostics.supported_points, 5);
    assert_eq!(diagnostics.span_s, Some(70.));
    assert_eq!(
        dense.frequency[0].fit_diagnostics,
        sparse.frequency[0].fit_diagnostics
    );
    // Independent fixture adds +2 Hz at every exact measurement; scoring scale
    // sqrt(1.5^2 + 2^2)=2.5 Hz. Intervening display values add no observations.
    let comparison = diagnostics.comparison.as_ref().unwrap();
    assert!((comparison.doppler.rmse_hz - 2.).abs() < 1e-10);
    assert!((comparison.doppler.normalized_rmse - 0.8).abs() < 1e-10);
    let fixed =
        workspace_scoring::score_observers(&doc, &map, &lightning, None, &[receiver]).unwrap();
    assert_eq!(
        serde_json::to_value(&dense.score_report).unwrap(),
        serde_json::to_value(&fixed).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&sparse.score_report).unwrap(),
        serde_json::to_value(&fixed).unwrap()
    );
    assert_eq!(
        before,
        serde_json::to_vec(&(&doc, &map, &lightning)).unwrap()
    );
}

#[test]
fn older_overlay_without_fit_diagnostics_still_deserializes_without_inventing_them() {
    let (doc, map, lightning, receiver) = fixture();
    let current = overlay::build(
        &doc,
        &map,
        &lightning,
        None,
        &receiver,
        &OverlaySettings::default(),
    )
    .unwrap();
    let mut older_json = serde_json::to_value(&current).unwrap();
    older_json["frequency"][0]
        .as_object_mut()
        .unwrap()
        .remove("fit_diagnostics");
    let older: overlay::PredictionOverlay = serde_json::from_value(older_json).unwrap();
    assert!(older.frequency[0].fit_diagnostics.is_none());
    assert_eq!(
        serde_json::to_value(&older.score_report).unwrap(),
        serde_json::to_value(&current.score_report).unwrap()
    );
}
