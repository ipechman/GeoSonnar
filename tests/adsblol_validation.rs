//! Independent fabricated controls for detailed readsb trace ingestion.
use geosonnar::adsblol::*;
use serde_json::{Value, json};

const ORIGIN: f64 = 1_788_825_600.0;

fn config() -> TraceImportConfig {
    TraceImportConfig {
        source_label: "fabricated readsb controls".into(),
        time_origin_utc_s: ORIGIN,
        time_window_utc_s: [ORIGIN, ORIGIN + 86_400.0],
        ..TraceImportConfig::default()
    }
}

fn point(time: f64, longitude: f64) -> Value {
    json!([
        time,
        0.0,
        longitude,
        1000.0,
        110.0,
        90.0,
        0,
        10.0,
        null,
        "adsb_icao",
        1100.0,
        20.0,
        100.0,
        -2.0
    ])
}

fn trace(rows: Vec<Value>) -> Value {
    json!({"icao":"abc123","timestamp":ORIGIN,"r":"TEST","t":"C172","version":"test-format","trace":rows})
}

fn import(rows: Vec<Value>, config: &TraceImportConfig) -> TraceImportReport {
    normalize_trace(&trace(rows).to_string(), config).unwrap()
}

fn equatorial_height(point: &geosonnar::physics::TrackPoint) -> f64 {
    point.xyz_m[0].hypot(point.xyz_m[1]) - 6_378_137.0
}

#[test]
fn geometric_slot_and_flag_are_distinct_from_pressure_altitude() {
    let first = point(10.125, 0.0);
    let mut second = point(15.375, 0.001);
    second[3] = json!(1110.0);
    second[6] = json!(8);
    second[10] = Value::Null;
    let result = import(vec![first, second], &config());
    assert_eq!(result.passes.len(), 1);
    let pass = &result.passes[0];
    assert_eq!(pass.track.points[0].t_s, 10.125);
    assert_eq!(pass.track.points[1].t_s, 15.375);
    assert!((equatorial_height(&pass.track.points[0]) - 1100.0 * 0.3048).abs() < 1e-8);
    assert!((equatorial_height(&pass.track.points[1]) - 1110.0 * 0.3048).abs() < 1e-8);
    assert_eq!(pass.source_points[0].altitude_basis, "geometric_slot");
    assert_eq!(pass.source_points[1].altitude_basis, "geometric_flag");
    assert_eq!(pass.source_points[0].altitude_main_ft, Some(1000.0));
    assert_eq!(pass.source_points[0].ground_speed_knots, Some(110.0));
    assert_eq!(
        pass.source_points[0].geometric_vertical_rate_fpm,
        Some(20.0)
    );
    assert_eq!(pass.source_points[0].indicated_airspeed_knots, Some(100.0));
    assert_eq!(pass.source_points[0].roll_deg, Some(-2.0));
    assert_eq!(result.stats.geometric_slot_points, 1);
    assert_eq!(result.stats.geometric_flag_points, 1);
    assert!(!result.location_verified);
}

#[test]
fn missing_altitude_never_uses_sparse_details_or_neighbor_interpolation() {
    let mut missing = point(20.0, 0.002);
    missing[10] = Value::Null;
    missing[8] = json!({"alt_geom":9900,"nav_qnh":1013.25});
    let result = import(
        vec![
            point(0.0, 0.0),
            point(10.0, 0.001),
            missing,
            point(30.0, 0.003),
            point(40.0, 0.004),
        ],
        &config(),
    );
    assert_eq!(result.stats.rejected_altitude, 1);
    assert_eq!(result.passes.len(), 2);
    assert_eq!(result.passes[0].track.points.len(), 2);
    assert_eq!(result.passes[1].track.points.len(), 2);
    assert_eq!(result.passes[1].source_points[0].row_index, 3);
}

#[test]
fn optional_barometric_approximation_is_explicit_and_splits_vertical_basis() {
    let mut settings = config();
    settings.altitude_policy = TraceAltitudePolicy::AllowBarometricApproximation {
        ellipsoid_offset_m: 42.0,
        uncertainty_m: 150.0,
        rationale: "Fabricated uncertainty treatment, not a pressure correction".into(),
    };
    let mut one = point(20.0, 0.002);
    one[10] = Value::Null;
    let mut two = point(30.0, 0.003);
    two[10] = Value::Null;
    let result = import(
        vec![point(0.0, 0.0), point(10.0, 0.001), one, two],
        &settings,
    );
    assert_eq!(result.stats.altitude_basis_splits, 1);
    assert_eq!(result.passes.len(), 2);
    assert_eq!(result.stats.barometric_approximation_points, 2);
    let pass = &result.passes[1];
    assert!((equatorial_height(&pass.track.points[0]) - (304.8 + 42.0)).abs() < 1e-8);
    assert_eq!(pass.source_points[0].altitude_uncertainty_m, Some(150.0));
    assert_eq!(
        pass.source_points[0].altitude_basis,
        "barometric_approximation"
    );
    settings.altitude_policy = TraceAltitudePolicy::AllowBarometricApproximation {
        ellipsoid_offset_m: 0.0,
        uncertainty_m: 0.0,
        rationale: String::new(),
    };
    assert!(validate_config(&settings).is_err());
}

#[test]
fn stale_ground_source_and_invalid_geometry_break_support() {
    for (slot, value, stat) in [
        (3, json!("ground"), "rejected_ground"),
        (6, json!(1), "rejected_stale_position"),
        (9, json!("mlat"), "rejected_position_source"),
        (1, json!(95.0), "rejected_geometry"),
    ] {
        let mut rejected = point(20.0, 0.002);
        rejected[slot] = value;
        let result = import(
            vec![
                point(0.0, 0.0),
                point(10.0, 0.001),
                rejected,
                point(30.0, 0.003),
                point(40.0, 0.004),
            ],
            &config(),
        );
        assert_eq!(result.passes.len(), 2, "{stat}");
        assert_eq!(serde_json::to_value(&result.stats).unwrap()[stat], 1);
        assert_eq!(result.stats.output_points, 4);
    }
}

#[test]
fn new_legs_long_gaps_and_motion_jumps_are_preserved_as_breaks() {
    for mode in 0..3 {
        let mut third = point(20.0, 0.002);
        let mut fourth = point(30.0, 0.003);
        if mode == 0 {
            third[6] = json!(2);
        }
        if mode == 1 {
            third[0] = json!(100.0);
            fourth[0] = json!(110.0);
        }
        if mode == 2 {
            third[2] = json!(2.0);
            fourth[2] = json!(2.001);
        }
        let report = import(
            vec![point(0.0, 0.0), point(10.0, 0.001), third, fourth],
            &config(),
        );
        assert_eq!(report.passes.len(), 2);
        assert_eq!(
            [
                report.stats.leg_splits,
                report.stats.gap_splits,
                report.stats.speed_jump_splits
            ][mode],
            1
        );
    }
}

#[test]
fn exact_duplicates_deduplicate_without_manufacturing_support() {
    let p = point(0.0, 0.0);
    let report = import(vec![p.clone(), p.clone(), point(10.0, 0.001)], &config());
    assert_eq!(report.stats.exact_duplicates_removed, 1);
    assert_eq!(report.stats.output_points, 2);
    assert_eq!(report.stats.conflicting_timestamp_groups, 0);
    assert_eq!(report.stats.rejected_time_reversal_rows, 0);
}

#[test]
fn every_conflicting_timestamp_row_is_excluded_and_support_is_split() {
    let one = point(20.0, 0.002);
    // A later conflict invalidates both earlier copies; it must not select the
    // first/last position, average positions, or manufacture timestamp jitter.
    let mut different = one.clone();
    different[2] = json!(0.00201);
    let report = import(
        vec![
            point(0.0, 0.0),
            point(10.0, 0.001),
            one.clone(),
            one,
            different,
            point(30.0, 0.003),
            point(40.0, 0.004),
        ],
        &config(),
    );
    assert_eq!(report.stats.conflicting_timestamp_groups, 1);
    assert_eq!(report.stats.rejected_conflicting_timestamp_rows, 3);
    assert_eq!(report.stats.exact_duplicates_removed, 0);
    assert_eq!(report.stats.output_points, 4);
    assert_eq!(report.passes.len(), 2);
    assert_eq!(
        report.passes[0]
            .source_points
            .iter()
            .map(|p| p.row_index)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(
        report.passes[1]
            .source_points
            .iter()
            .map(|p| p.row_index)
            .collect::<Vec<_>>(),
        [5, 6]
    );
    assert_eq!(report.sources[0].position_source_counts["adsb_icao"], 7);
    let mut second = report.clone();
    second.sources[0].icao24 = "def456".into();
    for pass in &mut second.passes {
        pass.icao24 = "def456".into();
        pass.track.flight_id = pass.track.flight_id.replace("abc123", "def456");
    }
    let merged = merge_reports(vec![report, second], &config()).unwrap();
    assert_eq!(merged.stats.conflicting_timestamp_groups, 2);
    assert_eq!(merged.stats.rejected_conflicting_timestamp_rows, 6);
}

#[test]
fn metadata_conflict_is_not_silently_merged_into_one_measurement() {
    let one = point(20.0, 0.002);
    let mut different = one.clone();
    different[8] = json!({"type":"adsb_icao","alt_geom":9999});
    let result = import(
        vec![
            point(0.0, 0.0),
            point(10.0, 0.001),
            one,
            different,
            point(30.0, 0.003),
            point(40.0, 0.004),
        ],
        &config(),
    );
    assert_eq!(result.stats.rejected_conflicting_timestamp_rows, 2);
    assert_eq!(result.passes.len(), 2);
    assert_eq!(result.stats.output_points, 4);
}

#[test]
fn reversal_excludes_both_overlapping_branches_without_sorting() {
    for (times, expected_rows, boundary_count, rejected) in [
        (
            vec![0., 10., 20., 15., 16., 25., 30.],
            vec![vec![0, 1], vec![5, 6]],
            1,
            3,
        ),
        (
            vec![0., 5., 10., 20., 15., 12., 25., 30.],
            vec![vec![0, 1, 2], vec![6, 7]],
            2,
            3,
        ),
        // Repeated identical rows in a reversed branch do not make duplicate
        // independent passes or restore the ambiguous earlier branch.
        (
            vec![0., 10., 20., 10., 20., 30., 40.],
            vec![vec![5, 6]],
            1,
            4,
        ),
    ] {
        let rows = times.iter().map(|&t| point(t, t * 0.0001)).collect();
        let report = import(rows, &config());
        assert_eq!(report.stats.time_reversal_boundaries, boundary_count);
        assert_eq!(report.stats.rejected_time_reversal_rows, rejected);
        assert_eq!(report.stats.conflicting_timestamp_groups, 0);
        assert_eq!(report.stats.exact_duplicates_removed, 0);
        assert_eq!(
            report
                .passes
                .iter()
                .map(|pass| pass
                    .source_points
                    .iter()
                    .map(|p| p.row_index)
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            expected_rows
        );
        assert!(
            report
                .passes
                .windows(2)
                .all(|pair| pair[0].track.points.last().unwrap().t_s < pair[1].track.points[0].t_s)
        );
    }
    let all_reversed = import(vec![point(20., 0.002), point(10., 0.001)], &config());
    assert!(all_reversed.passes.is_empty());
    assert_eq!(all_reversed.stats.rejected_time_reversal_rows, 2);
}

#[test]
fn nonadjacent_conflicts_remove_earlier_positions_and_count_once() {
    let report = import(
        vec![
            point(0., 0.),
            point(10., 0.001),
            point(20., 0.002),
            point(10., 0.00101),
            point(30., 0.003),
            point(40., 0.004),
        ],
        &config(),
    );
    assert_eq!(report.stats.conflicting_timestamp_groups, 1);
    assert_eq!(report.stats.rejected_conflicting_timestamp_rows, 2);
    assert_eq!(report.stats.time_reversal_boundaries, 1);
    assert_eq!(report.stats.rejected_time_reversal_rows, 1);
    assert_eq!(report.stats.singleton_chunks_discarded, 1);
    assert_eq!(report.passes.len(), 1);
    assert_eq!(
        report.passes[0]
            .source_points
            .iter()
            .map(|p| p.row_index)
            .collect::<Vec<_>>(),
        [4, 5]
    );
}

#[test]
fn time_window_keeps_exact_rows_and_region_selection_does_not_clip_paths() {
    let mut settings = config();
    settings.time_window_utc_s = [ORIGIN + 10.0, ORIGIN + 40.0];
    settings.bounds = Some([-0.001, -0.001, 0.001, 0.001]);
    let report = import(
        vec![
            point(0.0, -0.004),
            point(10.0, -0.003),
            point(20.0, -0.002),
            point(30.0, 0.002),
            point(40.0, 0.003),
            point(50.0, 0.004),
        ],
        &settings,
    );
    assert_eq!(report.stats.outside_time_window, 2);
    assert_eq!(report.stats.selected_traces, 1);
    assert_eq!(report.passes[0].track.points.len(), 4);
    assert_eq!(report.passes[0].source_points[0].longitude_deg, -0.003);
    assert_eq!(report.passes[0].source_points[3].longitude_deg, 0.003);
    assert_eq!(report.passes[0].track.points[0].t_s, 10.0);
    settings.bounds = Some([10.0, 10.0, 11.0, 11.0]);
    let outside = import(vec![point(10.0, 0.0), point(20.0, 0.001)], &settings);
    assert_eq!(outside.stats.source_rows, 2);
    assert_eq!(outside.stats.selected_traces, 0);
    assert!(outside.passes.is_empty());
}

#[test]
fn date_line_segment_selection_uses_the_short_crossing() {
    let bounds = [-1.0, 179.9, 1.0, -179.9];
    assert!(point_in_bounds(0.0, 179.95, bounds));
    assert!(point_in_bounds(0.0, -179.95, bounds));
    assert!(!point_in_bounds(0.0, 0.0, bounds));
    assert!(segment_intersects_bounds(
        [0.0, 179.8],
        [0.0, -179.8],
        bounds
    ));
    assert!(!segment_intersects_bounds(
        [0.0, 179.8],
        [0.0, -179.8],
        [-1.0, -1.0, 1.0, 1.0]
    ));
    assert!(!segment_intersects_bounds(
        [2.0, 179.8],
        [2.0, -179.8],
        bounds
    ));
    assert!(!segment_intersects_bounds(
        [f64::NAN, 179.8],
        [0.0, -179.8],
        bounds
    ));
    let mut settings = config();
    settings.bounds = Some([-1.0, 179.9995, 1.0, -179.9995]);
    let result = import(vec![point(0.0, 179.999), point(10.0, -179.999)], &settings);
    assert_eq!(result.stats.selected_traces, 1);
    assert_eq!(result.stats.speed_jump_splits, 0);
    assert_eq!(result.passes[0].track.points.len(), 2);
}

#[test]
fn source_allowlist_is_explicit_and_never_carried_between_rows() {
    let mut one = point(0.0, 0.0);
    let mut two = point(10.0, 0.001);
    one[9] = json!("mlat");
    two[9] = json!("mlat");
    assert!(
        import(vec![one.clone(), two.clone()], &config())
            .passes
            .is_empty()
    );
    let mut settings = config();
    settings.accepted_sources = vec!["mlat".into()];
    assert_eq!(import(vec![one, two], &settings).passes.len(), 1);
    settings.accepted_sources = vec!["invented_source".into()];
    assert!(validate_config(&settings).is_err());
    let mut missing = point(20.0, 0.002);
    missing[9] = Value::Null;
    let mut first = point(0.0, 0.0);
    first[8] = json!({"type":"adsb_icao","alt_geom":9999});
    let result = import(
        vec![
            first,
            point(10.0, 0.001),
            missing,
            point(30.0, 0.003),
            point(40.0, 0.004),
        ],
        &config(),
    );
    assert_eq!(result.stats.rejected_position_source, 1);
    assert_eq!(result.passes.len(), 2);
}

#[test]
fn source_counts_include_rejected_rows_and_preserve_explicit_fallback_precedence() {
    let mut rows: Vec<_> = (0..12)
        .map(|index| point(index as f64 * 10.0, index as f64 * 0.001))
        .collect();
    // A missing explicit source may use the sparse object's type, but an
    // explicit per-row source wins even when that object names another source.
    for index in [2, 10] {
        rows[index][9] = Value::Null;
        rows[index][8] = json!({"type":"adsr_icao"});
    }
    rows[3][8] = json!({"type":"mlat"});
    rows[4][9] = json!("mlat");
    rows[5][9] = Value::Null;
    rows[6][9] = json!("unrecognized_provider_source");
    rows[7][6] = json!(1); // stale position
    rows[8][3] = json!("ground");
    let mut settings = config();
    settings.time_window_utc_s = [ORIGIN + 10.0, ORIGIN + 100.0];
    let report = import(rows, &settings);

    assert_eq!(
        serde_json::to_value(&report.sources).unwrap(),
        json!([{
            "icao24":"abc123", "source_timestamp_utc_s":ORIGIN,
            "registration":"TEST", "aircraft_type":"C172",
            "readsb_version":"test-format", "selected_by_region":true,
            "source_rows":12,
            "position_source_counts":{
                "adsb_icao":7, "adsr_icao":2, "mlat":1, "unknown":1,
                "unrecognized_provider_source":1
            }
        }])
    );
    let expected_stats = TraceImportStats {
        source_traces: 1,
        source_rows: 12,
        selected_traces: 1,
        outside_time_window: 2,
        rejected_ground: 1,
        rejected_stale_position: 1,
        rejected_position_source: 3,
        geometric_slot_points: 5,
        output_passes: 2,
        output_points: 5,
        ..TraceImportStats::default()
    };
    assert_eq!(
        serde_json::to_value(&report.stats).unwrap(),
        serde_json::to_value(expected_stats).unwrap()
    );
    assert_eq!(
        report
            .passes
            .iter()
            .map(|pass| (
                pass.track.flight_id.as_str(),
                pass.source_points
                    .iter()
                    .map(|point| (point.row_index, point.position_source.as_str()))
                    .collect::<Vec<_>>()
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                "abc123#part0",
                vec![(1, "adsb_icao"), (2, "adsr_icao"), (3, "adsb_icao")]
            ),
            ("abc123#part1", vec![(9, "adsb_icao"), (10, "adsr_icao")])
        ]
    );
}

#[test]
fn malformed_flags_time_identity_and_nonfinite_json_fail() {
    for slot in [0, 6] {
        let mut bad = point(0.0, 0.0);
        bad[slot] = json!(-1.5);
        assert!(normalize_trace(&trace(vec![bad]).to_string(), &config()).is_err());
    }
    let mut bad = trace(vec![point(0.0, 0.0)]);
    bad["timestamp"] = json!(ORIGIN * 1000.0);
    assert!(normalize_trace(&bad.to_string(), &config()).is_err());
    bad["timestamp"] = json!(ORIGIN);
    bad["icao"] = json!("../abc123");
    assert!(normalize_trace(&bad.to_string(), &config()).is_err());
    assert!(normalize_trace("{\"timestamp\":NaN}", &config()).is_err());
    let mut settings = config();
    settings.bounds = Some([-1.0, 0.0, 1.0, 0.0]);
    assert!(validate_config(&settings).is_err());
    settings = config();
    settings.time_window_utc_s = [ORIGIN, ORIGIN];
    assert!(validate_config(&settings).is_err());
}

#[test]
fn merge_keeps_all_distinct_aircraft_and_rejects_implicit_instance_deduplication() {
    let one = import(vec![point(0.0, 0.0), point(10.0, 0.001)], &config());
    let mut raw = trace(vec![point(0.0, 0.0), point(10.0, 0.001)]);
    raw["icao"] = json!("def456");
    let two = normalize_trace(&raw.to_string(), &config()).unwrap();
    let all = merge_reports(vec![one.clone(), two], &config()).unwrap();
    assert_eq!(all.stats.source_traces, 2);
    assert_eq!(all.stats.source_rows, 4);
    assert_eq!(all.stats.output_points, 4);
    assert_eq!(all.passes.len(), 2);
    assert_ne!(all.passes[0].track.flight_id, all.passes[1].track.flight_id);
    assert!(merge_reports(vec![one.clone(), one.clone()], &config()).is_err());
    let mut changed = one;
    changed.config.max_gap_s = 25.0;
    assert!(merge_reports(vec![changed], &config()).is_err());
}

#[test]
fn sparse_singletons_never_become_a_synthetic_track() {
    let result = import(vec![point(0.0, 0.0), point(100.0, 0.001)], &config());
    assert!(result.passes.is_empty());
    assert_eq!(result.stats.singleton_chunks_discarded, 2);
    assert_eq!(result.stats.output_points, 0);
}
