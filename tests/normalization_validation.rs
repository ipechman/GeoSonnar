//! Source-adapter contracts motivated by the pinned DOPPLERGEIST ingestion,
//! missing-velocity and motion-support tests (70626ad44fb9c7f4e2f5b225956c1f9281f91c13).
//! This adapter consumes simplified adsb.aero geometry, not upstream readsb
//! raw traces. Tests preserve that distinction and never claim absent measured
//! velocities or pressure-to-ellipsoid accuracy.

use geosonnar::normalization::*;
use geosonnar::physics::{ModelConfig, validate_track};
use serde_json::{Value, json};

const ORIGIN: f64 = 1_788_825_600.0;

fn config() -> NormalizationConfig {
    NormalizationConfig {
        source_label: "independent validation fixture".into(),
        time_origin_utc_s: ORIGIN,
        max_gap_s: 30.0,
        max_speed_mps: 330.0,
        altitude_treatment: AltitudeTreatment::PressureApproximation {
            ellipsoid_offset_m: -30.0,
            uncertainty_m: 150.0,
            rationale: "Declared approximation for unit and metadata tests only".into(),
        },
    }
}

fn flight(coordinates: Value, times: Value) -> Value {
    json!({"flight_id":"source-flight", "icao24":"ab1234", "path": {"type":"MultiLineString", "coordinates":coordinates}, "timestamps":times})
}

fn normalize(flights: Vec<Value>, config: &NormalizationConfig) -> NormalizationReport {
    normalize_adsb_aero(&json!({"flights":flights}).to_string(), config).unwrap()
}

fn equatorial_height(point: &geosonnar::physics::TrackPoint) -> f64 {
    point.xyz_m[0].hypot(point.xyz_m[1]) - 6_378_137.0
}

#[test]
fn exact_duplicates_are_removed_but_conflicting_or_reversed_rows_are_not_sorted() {
    let duplicate = flight(
        json!([[[0.0, 0.0, 1000.0], [0.0, 0.0, 1000.0], [0.001, 0.0, 1000.0]]]),
        json!([[ORIGIN, ORIGIN, ORIGIN + 10.0]]),
    );
    let report = normalize(vec![duplicate], &config());
    assert_eq!(report.stats.source_vertices, 3);
    assert_eq!(report.stats.duplicate_vertices_removed, 1);
    assert_eq!(report.stats.output_vertices, 2);
    assert_eq!(report.passes.len(), 1);
    for times in [
        json!([[ORIGIN, ORIGIN, ORIGIN + 10.0]]),
        json!([[ORIGIN + 10.0, ORIGIN, ORIGIN + 20.0]]),
    ] {
        let bad = flight(
            json!([[
                [0.0, 0.0, 1000.0],
                [0.001, 0.0, 1000.0],
                [0.002, 0.0, 1000.0]
            ]]),
            times,
        );
        let report = normalize(vec![bad], &config());
        assert_eq!(report.stats.rejected_subsequences, 1);
        assert!(report.passes.is_empty());
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].source_subsequence, Some(0));
    }
}

#[test]
fn provider_breaks_and_unphysical_jumps_preserve_clean_independent_chunks() {
    let raw = flight(
        json!([
            [
                [0.0, 0.0, 1000.0],
                [0.001, 0.0, 1000.0],
                [1.0, 0.0, 1000.0],
                [1.001, 0.0, 1000.0]
            ],
            [[1.002, 0.0, 1000.0], [1.003, 0.0, 1000.0]]
        ]),
        json!([
            [ORIGIN, ORIGIN + 10.0, ORIGIN + 20.0, ORIGIN + 30.0],
            [ORIGIN + 31.0, ORIGIN + 41.0]
        ]),
    );
    let report = normalize(vec![raw], &config());
    assert_eq!(report.stats.speed_jump_splits, 1);
    assert_eq!(report.stats.vertex_gap_splits, 0);
    assert_eq!(report.passes.len(), 3);
    assert_eq!(report.stats.output_vertices, 6);
    assert_eq!(report.passes[2].source_subsequence, 1);
    for pass in &report.passes {
        assert_eq!(pass.track.points.len(), 2);
        validate_track(&pass.track, ModelConfig::default()).unwrap();
    }
    assert_ne!(
        report.passes[0].track.flight_id,
        report.passes[1].track.flight_id
    );
}

#[test]
fn large_gap_is_split_without_fabricating_bridge_or_losing_the_next_clean_span() {
    let raw = flight(
        json!([[
            [0.0, 0.0, 1000.0],
            [0.001, 0.0, 1000.0],
            [0.002, 0.0, 1000.0],
            [0.003, 0.0, 1000.0],
            [0.004, 0.0, 1000.0]
        ]]),
        json!([[
            ORIGIN,
            ORIGIN + 10.0,
            ORIGIN + 100.0,
            ORIGIN + 110.0,
            ORIGIN + 200.0
        ]]),
    );
    let report = normalize(vec![raw], &config());
    assert_eq!(report.stats.vertex_gap_splits, 2);
    assert_eq!(report.stats.singleton_chunks_discarded, 1);
    assert_eq!(report.passes.len(), 2);
    assert_eq!(report.passes[0].track.points.last().unwrap().t_s, 10.0);
    assert_eq!(report.passes[1].track.points.first().unwrap().t_s, 100.0);
}

#[test]
fn common_utc_origin_preserves_midnight_crossing_and_interflight_separation() {
    let first = flight(
        json!([[[0.0, 0.0, 1000.0], [0.001, 0.0, 1000.0]]]),
        json!([[ORIGIN - 5.25, ORIGIN + 4.75]]),
    );
    let mut second = flight(
        json!([[[0.0, 0.0, 1000.0], [0.001, 0.0, 1000.0]]]),
        json!([[ORIGIN + 920.5, ORIGIN + 930.5]]),
    );
    second["flight_id"] = json!("second-source-flight");
    let report = normalize(vec![first, second], &config());
    assert_eq!(report.passes[0].track.points[0].t_s, -5.25);
    assert_eq!(report.passes[0].track.points[1].t_s, 4.75);
    assert_eq!(report.passes[1].track.points[0].t_s, 920.5);
    assert_eq!(
        report.passes[1].track.points[0].t_s - report.passes[0].track.points[0].t_s,
        925.75
    );
    assert_eq!(report.config.time_origin_utc_s, ORIGIN);
}

#[test]
fn calibration_before_first_selected_point_is_used_and_never_leaks_between_source_parts() {
    let mut cfg = config();
    cfg.altitude_treatment = AltitudeTreatment::QnhMslWithGeoidOffset {
        geoid_undulation_m: -31.0,
        uncertainty_m: 80.0,
        rationale: "Local declared geoid for fixture".into(),
    };
    let mut raw = flight(
        json!([
            [
                [0.0, 0.0, 1000.0],
                [0.001, 0.0, 1000.0],
                [0.002, 0.0, 1000.0]
            ],
            [[0.003, 0.0, 1000.0], [0.004, 0.0, 1000.0]]
        ]),
        json!([
            [ORIGIN + 10.0, ORIGIN + 20.0, ORIGIN + 30.0],
            [ORIGIN + 100.0, ORIGIN + 110.0]
        ]),
    );
    raw["alt_correction_ft"] = json!([[[ORIGIN - 100.0, 100.0], [ORIGIN + 25.0, 200.0]], []]);
    let report = normalize(vec![raw], &cfg);
    assert_eq!(report.passes.len(), 1);
    assert_eq!(report.stats.rejected_subsequences, 1);
    assert_eq!(report.issues[0].source_subsequence, Some(1));
    let heights: Vec<f64> = report.passes[0]
        .track
        .points
        .iter()
        .map(equatorial_height)
        .collect();
    assert!((heights[0] - (1100.0 * 0.3048 - 31.0)).abs() < 1e-8);
    assert!((heights[1] - heights[0]).abs() < 1e-8);
    assert!((heights[2] - (1200.0 * 0.3048 - 31.0)).abs() < 1e-8);
}

#[test]
fn absent_future_or_unordered_qnh_support_is_reported_not_silently_assumed() {
    let mut cfg = config();
    cfg.altitude_treatment = AltitudeTreatment::QnhMslWithGeoidOffset {
        geoid_undulation_m: 20.0,
        uncertainty_m: 80.0,
        rationale: "Explicit fixture geoid".into(),
    };
    for correction in [
        Value::Null,
        json!([[[ORIGIN + 5.0, 100.0]]]),
        json!([[[ORIGIN, 100.0], [ORIGIN - 1.0, 200.0]]]),
        json!([[[ORIGIN, 100.0], [ORIGIN, 200.0]]]),
    ] {
        let mut raw = flight(
            json!([[[0.0, 0.0, 1000.0], [0.001, 0.0, 1000.0]]]),
            json!([[ORIGIN, ORIGIN + 10.0]]),
        );
        raw["alt_correction_ft"] = correction;
        let report = normalize(vec![raw], &cfg);
        assert!(report.passes.is_empty());
        assert_eq!(report.stats.rejected_subsequences, 1);
        assert_eq!(report.issues.len(), 1);
    }
}

#[test]
fn explicit_pressure_assumption_and_uncertainty_survive_serialization() {
    let raw = flight(
        json!([[[0.0, 0.0, 10000.0], [0.001, 0.0, 10000.0]]]),
        json!([[ORIGIN, ORIGIN + 10.0]]),
    );
    let report = normalize(vec![raw], &config());
    assert!((equatorial_height(&report.passes[0].track.points[0]) - 3018.0).abs() < 1e-8);
    let saved = serde_json::to_value(&report).unwrap();
    assert_eq!(
        saved["config"]["altitude_treatment"]["mode"],
        "pressure_approximation"
    );
    assert_eq!(
        saved["config"]["altitude_treatment"]["uncertainty_m"],
        150.0
    );
    assert!(
        report
            .quality_notes
            .iter()
            .any(|note| note.contains("pressure, MSL and ellipsoid"))
    );
    let mut invalid = serde_json::to_value(config()).unwrap();
    invalid
        .as_object_mut()
        .unwrap()
        .remove("altitude_treatment");
    assert!(serde_json::from_value::<NormalizationConfig>(invalid).is_err());
    for (uncertainty, rationale) in [(0.0, "declared"), (-1.0, "declared"), (100.0, "")] {
        let mut cfg = config();
        cfg.altitude_treatment = AltitudeTreatment::PressureApproximation {
            ellipsoid_offset_m: 0.0,
            uncertainty_m: uncertainty,
            rationale: rationale.into(),
        };
        assert!(normalize_adsb_aero("{\"flights\":[]}", &cfg).is_err());
    }
}

#[test]
fn longitude_wrap_uses_ecef_and_does_not_create_a_world_spanning_speed_jump() {
    let raw = flight(
        json!([[[179.999, 0.0, 1000.0], [-179.999, 0.0, 1000.0]]]),
        json!([[ORIGIN, ORIGIN + 10.0]]),
    );
    let report = normalize(vec![raw], &config());
    assert_eq!(report.passes.len(), 1);
    assert_eq!(report.stats.speed_jump_splits, 0);
    let points = &report.passes[0].track.points;
    let distance =
        (points[1].xyz_m[0] - points[0].xyz_m[0]).hypot(points[1].xyz_m[1] - points[0].xyz_m[1]);
    assert!((220.0..225.0).contains(&distance));
}

#[test]
fn missing_type_or_velocity_fields_do_not_exclude_aircraft_or_fabricate_measured_velocity() {
    let first = flight(
        json!([[[0.0, 0.0, 1000.0], [0.001, 0.0, 1000.0]]]),
        json!([[ORIGIN, ORIGIN + 10.0]]),
    );
    let mut second = first.clone();
    second["flight_id"] = json!("propeller");
    second["icao_type"] = json!("C172");
    second["path_gs"] = Value::Null;
    second["path_tracks"] = Value::Null;
    let report = normalize(vec![first, second], &config());
    assert_eq!(report.passes.len(), 2);
    assert_eq!(report.passes[0].aircraft_type, None);
    assert_eq!(report.passes[0].callsign, None);
    assert_eq!(report.passes[1].aircraft_type.as_deref(), Some("C172"));
    assert_eq!(
        report.passes[0].track.points[0].xyz_m,
        report.passes[1].track.points[0].xyz_m
    );
    let serialized = serde_json::to_value(&report.passes[0].track.points[0]).unwrap();
    assert!(serialized.get("velocity").is_none());
    assert!(
        report
            .quality_notes
            .iter()
            .any(|note| note.contains("Derived segment velocities"))
    );
}

#[test]
fn malformed_subsequence_is_auditable_and_does_not_erase_clean_other_flights() {
    let invalid = flight(
        json!([[[0.0, 91.0, 1000.0], [0.001, 91.0, 1000.0]]]),
        json!([[ORIGIN, ORIGIN + 10.0]]),
    );
    let mut valid = flight(
        json!([[[0.0, 0.0, 1000.0], [0.001, 0.0, 1000.0]]]),
        json!([[ORIGIN, ORIGIN + 10.0]]),
    );
    valid["flight_id"] = json!("valid");
    let report = normalize(vec![invalid, valid], &config());
    assert_eq!(report.stats.source_flights, 2);
    assert_eq!(report.stats.source_subsequences, 2);
    assert_eq!(report.stats.source_vertices, 4);
    assert_eq!(report.stats.rejected_subsequences, 1);
    assert_eq!(report.stats.output_vertices, 2);
    assert_eq!(report.passes[0].source_flight_id, "valid");
    assert_eq!(report.issues[0].source_flight_id, "source-flight");
    assert!(normalize_adsb_aero("{\"flights\":42}", &config()).is_err());
}

#[test]
fn seconds_and_milliseconds_cannot_be_mixed_or_auto_guessed() {
    let raw = flight(
        json!([[[0.0, 0.0, 1000.0], [0.001, 0.0, 1000.0]]]),
        json!([[ORIGIN * 1000.0, (ORIGIN + 10.0) * 1000.0]]),
    );
    let report = normalize(vec![raw], &config());
    assert!(report.passes.is_empty());
    assert_eq!(report.stats.rejected_subsequences, 1);
    let mut cfg = config();
    cfg.time_origin_utc_s *= 1000.0;
    assert!(normalize_adsb_aero("{\"flights\":[]}", &cfg).is_err());
}
