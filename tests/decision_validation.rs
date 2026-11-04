//! Ports of pinned DOPPLERGEIST engineering decision controls, with additional
//! strict-input controls. See THIRD_PARTY_NOTICES.md. No empirical accuracy claim.
use geosonnar::decision::{decide, distance_m, utc_seconds};
use serde_json::{Value, json};

const INPUT: &str = include_str!(
    "../benchmarks/upstream/70626ad44fb9c7f4e2f5b225956c1f9281f91c13/decision-inputs.json"
);
fn evidence() -> Value {
    serde_json::from_str::<Value>(INPUT).unwrap()["cases"][0].clone()
}
fn reason(input: &Value, expected: &str) {
    let r = decide(input);
    assert_eq!(r["reason_code"], expected, "{r}");
    assert_ne!(r["status"], "succeeded");
}
fn mutate(input: &Value, path: &str, value: Value) -> Value {
    let mut c = input.clone();
    *c.pointer_mut(path).unwrap() = value;
    c
}
fn alternative(input: &mut Value, distance_m: f64, score: f64) {
    // At 40 degrees latitude this east-west approximation is accurate enough
    // for margins of tens of metres; exact geodesic has separate golden tests.
    let mut other = input["candidates"][0].clone();
    other["lon"] = json!(-100. + distance_m / 85_393.856_958_618_82);
    other["score"] = json!(score);
    input["candidates"].as_array_mut().unwrap().push(other);
}
fn finite() -> Value {
    let mut e = evidence();
    alternative(&mut e, 500_000., -4.);
    let d = &mut e["matching_diagnostics"];
    d["alternatives_evaluated"] = json!(false);
    d["assessed_mode_ids"] = json!(["a", "b"]);
    d["candidate_mode_ids"] = json!(["a", "b"]);
    d["finite_search_evidence"] = json!({"schema_version":1,"scope":"evaluated_finite_grid","complete":true,"score_kind":"negative_robust_ridge_loss_hz","spacing_m":1000.,"refine_spacing_m":100.,"max_event_groups":2,"retained_modes":[{"mode_id":"a","score":-1.,"event_groups":2},{"mode_id":"b","score":-4.,"event_groups":2}],"discarded_count":7,"best_discarded_score":-5.,"pruning_accounted_for":true,"limitations":["Evaluated grids only; no continuous uniqueness."]});
    e
}
fn near_finite(distance: f64) -> Value {
    let mut e = finite();
    e["candidates"].as_array_mut().unwrap().truncate(1);
    alternative(&mut e, distance, -1.2);
    e["matching_diagnostics"]["finite_search_evidence"]["retained_modes"][1]["score"] = json!(-1.2);
    e
}

#[test]
fn all_sixteen_pinned_decision_controls_match_labels_loaded_after_predictions() {
    let cases = serde_json::from_str::<Value>(INPUT).unwrap();
    let predictions: Vec<_> = cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["case_id"].as_str().unwrap().to_owned(), decide(c)))
        .collect();
    // Truth is consumed only after native predictions have been produced.
    let labels: Value = serde_json::from_str(include_str!(
        "../benchmarks/upstream/70626ad44fb9c7f4e2f5b225956c1f9281f91c13/truth/decision-labels.json"
    ))
    .unwrap();
    assert_eq!(predictions.len(), 16);
    let mut successes = 0;
    for (id, p) in predictions {
        let expected = if labels[&id]["expect_inconclusive"] == true {
            "inconclusive"
        } else {
            "succeeded"
        };
        assert_eq!(p["status"], expected, "{id}: {p}");
        if p["status"] == "succeeded" {
            successes += 1;
        }
    }
    assert_eq!(successes, 2);
}

#[test]
fn complete_evidence_is_immutable_and_not_a_probability() {
    let e = evidence();
    let before = e.clone();
    let r = decide(&e);
    assert_eq!(r["status"], "succeeded");
    assert_eq!(r["diagnostics"]["calibration"], "uncalibrated");
    assert!(r["diagnostics"]["probability"].is_null());
    assert_eq!(e, before);
}

#[test]
fn missing_incomplete_or_unavailable_data_never_looks_like_a_location() {
    let e = evidence();
    for (p, v, r) in [
        ("/coverage", Value::Null, "coverage_unknown"),
        (
            "/coverage/data_status",
            json!("unavailable"),
            "missing_flight_data",
        ),
        (
            "/coverage/data_status",
            json!("partial"),
            "partial_flight_coverage",
        ),
        (
            "/coverage/search_complete",
            json!(false),
            "search_incomplete",
        ),
        ("/candidates", json!([]), "no_matching_flights"),
        (
            "/matching_diagnostics/clock_locked",
            json!(false),
            "timing_not_established",
        ),
        (
            "/matching_diagnostics/alternatives_evaluated",
            json!(false),
            "alternatives_not_assessed",
        ),
    ] {
        reason(&mutate(&e, p, v), r);
    }
    let mut noise = e.clone();
    noise["events"] = json!([]);
    noise["candidates"] = json!([]);
    reason(&noise, "no_doppler_events");
    let mut missing = e;
    missing["coverage"]["missing_data"] = json!(["hour unavailable"]);
    reason(&missing, "partial_flight_coverage");
}

#[test]
fn residual_support_drift_and_sensitivity_vetoes_are_binding() {
    let e = evidence();
    for (p, v, r) in [
        (
            "/matching_diagnostics/e0/rmse_hz",
            json!(8.),
            "inconsistent_event_match",
        ),
        (
            "/matching_diagnostics/e0/outlier_fraction",
            json!(0.26),
            "inconsistent_event_match",
        ),
        (
            "/matching_diagnostics/e1/support_fraction",
            json!(0.5),
            "insufficient_track_support",
        ),
        (
            "/matching_diagnostics/e0/drift_at_bound",
            json!(true),
            "source_drift_unresolved",
        ),
        (
            "/matching_diagnostics/sensitivity/sources",
            json!(["altitude"]),
            "uncertainty_not_established",
        ),
        (
            "/matching_diagnostics/sensitivity/complete",
            json!(false),
            "uncertainty_not_established",
        ),
        (
            "/matching_diagnostics/sensitivity/boundary_limited",
            json!(true),
            "refinement_boundary_limited",
        ),
        (
            "/matching_diagnostics/refinement_boundary_limited",
            json!(true),
            "refinement_boundary_limited",
        ),
        (
            "/candidates/0/uncertainty/radius_m",
            json!(1500.),
            "location_uncertainty_too_large",
        ),
        (
            "/candidates/0/uncertainty/radius_m",
            json!(60.),
            "uncertainty_not_established",
        ),
    ] {
        reason(&mutate(&e, p, v), r);
    }
    let mut missing = e;
    missing["matching_diagnostics"]
        .as_object_mut()
        .unwrap()
        .remove("e0");
    reason(&missing, "matching_diagnostics_missing");
}

#[test]
fn threshold_boundaries_are_inclusive_and_score_units_cannot_be_substituted() {
    let mut e = evidence();
    e["matching_diagnostics"]["e0"] = json!({"rmse_hz":3.,"outlier_fraction":0.25,"support_fraction":0.95,"drift_at_bound":false});
    assert_eq!(decide(&e)["status"], "succeeded");
    e["matching_diagnostics"]["e0"]["support_fraction"] = json!(0.95 - 1e-10);
    reason(&e, "insufficient_track_support");
    e = evidence();
    e["candidates"][0]["score_kind"] = json!("traced_ridge_rmse_hz");
    reason(&e, "unsupported_score_kind");
    e = evidence();
    alternative(&mut e, 90., -2.);
    reason(&e, "ambiguous_locations"); // exactly .5 per independent event
}

#[test]
fn nearby_ties_must_fit_the_actual_reported_envelope() {
    for (distance, score, status) in [
        (60., -1.2, "succeeded"),
        (90., -1.2, "inconclusive"),
        (150., -1.2, "inconclusive"),
        (90., -5., "succeeded"),
    ] {
        let mut e = evidence();
        alternative(&mut e, distance, score);
        assert_eq!(decide(&e)["status"], status);
    }
    let mut e = evidence();
    alternative(&mut e, 0., -1.2);
    e["candidates"][1]["flight_ids"] = json!(["x", "y"]);
    reason(&e, "ambiguous_flight_match");
    e["candidates"][1]["lat"] = json!(45.);
    reason(&e, "ambiguous_locations");
}

#[test]
fn harmonics_are_not_independent_and_cannot_hide_a_bad_measurement() {
    let mut e = evidence();
    e["events"][1]["harmonic_group_id"] = json!("g0");
    reason(&e, "inconsistent_harmonic_association");
    e["candidates"][0]["flight_ids"] = json!(["a", "a"]);
    reason(&e, "insufficient_independent_events");
    e = evidence();
    let mut harmonic = e["events"][0].clone();
    harmonic["event_id"] = json!("e2");
    e["events"].as_array_mut().unwrap().push(harmonic);
    e["candidates"][0]["event_ids"] = json!(["e0", "e1", "e2"]);
    e["candidates"][0]["flight_ids"] = json!(["a", "b", "a"]);
    e["matching_diagnostics"]["e2"] = e["matching_diagnostics"]["e0"].clone();
    e["matching_diagnostics"]["e0"]["rmse_hz"] = json!(8.);
    reason(&e, "inconsistent_event_match");
}

#[test]
fn all_four_independent_events_are_checked() {
    let mut e = evidence();
    for i in 2..4 {
        let mut ev = e["events"][0].clone();
        ev["event_id"] = json!(format!("e{i}"));
        ev["harmonic_group_id"] = json!(format!("g{i}"));
        e["events"].as_array_mut().unwrap().push(ev);
        e["candidates"][0]["event_ids"]
            .as_array_mut()
            .unwrap()
            .push(json!(format!("e{i}")));
        e["candidates"][0]["flight_ids"]
            .as_array_mut()
            .unwrap()
            .push(json!(format!("flight{i}")));
        e["matching_diagnostics"][format!("e{i}")] = e["matching_diagnostics"]["e0"].clone();
    }
    assert_eq!(decide(&e)["status"], "succeeded");
    e["matching_diagnostics"]["e3"]["rmse_hz"] = json!(8.);
    reason(&e, "inconsistent_event_match");
}

#[test]
fn a_single_event_requires_full_independent_clock_window_sensitivity() {
    let mut e = evidence();
    e["events"].as_array_mut().unwrap().truncate(1);
    e["candidates"][0]["event_ids"] = json!(["e0"]);
    e["candidates"][0]["flight_ids"] = json!(["a"]);
    reason(&e, "insufficient_independent_events");
    let t = utc_seconds(e["candidates"][0]["recording_start_utc"].as_str().unwrap()).unwrap();
    e["matching_diagnostics"]["recording_start_window_s"] = json!([t - 1., t + 1.]);
    e["matching_diagnostics"]["sensitivity"]["settings"] = json!({"clock_bounds_s":[t,t]});
    reason(&e, "insufficient_independent_events");
    e["matching_diagnostics"]["sensitivity"]["settings"]["clock_bounds_s"] =
        json!([t - 1., t + 1.]);
    assert_eq!(decide(&e)["status"], "succeeded");
    e["matching_diagnostics"]["recording_start_window_s"] = json!([t + 1., t + 2.]);
    reason(&e, "recording_clock_mismatch");
}

#[test]
fn subset_scoring_must_account_for_unsupported_and_harmonic_proposals() {
    let mut e = evidence();
    let mut extra = e["events"][1].clone();
    extra["event_id"] = json!("e2");
    extra["harmonic_group_id"] = json!("g2");
    e["events"].as_array_mut().unwrap().push(extra);
    e["candidates"][0]["score_kind"] = json!("matched_group_support_minus_normalized_residual");
    e["candidates"][0]["score"] = json!(1.8);
    reason(&e, "unexplained_events");
    e["matching_diagnostics"]["unmatched_event_ids"] = json!(["e2"]);
    assert_eq!(decide(&e)["status"], "succeeded");
    e["matching_diagnostics"]["supported_event_ids"] = json!(["e0", "e1", "e2"]);
    reason(&e, "unmatched_supported_events");
    e["matching_diagnostics"]["supported_event_ids"] = json!(["e0", "e1"]);
    e["events"][2]["harmonic_group_id"] = json!("g0");
    reason(&e, "incomplete_harmonic_group");
}

#[test]
fn finite_grid_completeness_and_retention_bounds_cannot_be_overridden_by_a_flag() {
    let e = finite();
    assert_eq!(decide(&e)["status"], "succeeded");
    for (p, v, r) in [
        (
            "/matching_diagnostics/finite_search_evidence/best_discarded_score",
            json!(-1.2),
            "retention_may_hide_alternatives",
        ),
        (
            "/matching_diagnostics/finite_search_evidence/complete",
            json!(false),
            "finite_search_incomplete",
        ),
        (
            "/matching_diagnostics/finite_search_evidence/pruning_accounted_for",
            json!(false),
            "finite_search_incomplete",
        ),
        (
            "/matching_diagnostics/assessed_mode_ids",
            json!(["a"]),
            "finite_modes_not_assessed",
        ),
        (
            "/matching_diagnostics/candidate_mode_ids",
            json!(["a", "a"]),
            "finite_candidate_mapping_incomplete",
        ),
        (
            "/matching_diagnostics/finite_search_evidence/retained_modes/1/score",
            json!(0.),
            "finite_candidate_scores_inconsistent",
        ),
    ] {
        let mut c = mutate(&e, p, v);
        c["matching_diagnostics"]["alternatives_evaluated"] = json!(true);
        reason(&c, r);
    }
}

#[test]
fn finite_subset_retention_uses_subset_score_units() {
    let mut e = finite();
    let kind = "matched_group_support_minus_normalized_residual";
    e["candidates"][0]["score_kind"] = json!(kind);
    e["candidates"][1]["score_kind"] = json!(kind);
    e["candidates"][0]["score"] = json!(1.8);
    e["candidates"][1]["score"] = json!(0.1);
    let f = &mut e["matching_diagnostics"]["finite_search_evidence"];
    f["score_kind"] = json!(kind);
    f["retained_modes"][0]["score"] = json!(1.8);
    f["retained_modes"][1]["score"] = json!(0.1);
    f["best_discarded_score"] = json!(1.5);
    reason(&e, "retention_may_hide_alternatives");
    e["matching_diagnostics"]["finite_search_evidence"]["best_discarded_score"] = json!(1.2);
    assert_eq!(decide(&e)["status"], "succeeded");
}

#[test]
fn finite_nearby_modes_expand_a_copied_envelope() {
    let e = near_finite(90.);
    let before = e.clone();
    let r = decide(&e);
    assert_eq!(r["status"], "succeeded");
    assert!(
        (r["candidates"][0]["uncertainty"]["radius_m"]
            .as_f64()
            .unwrap()
            - 160.)
            .abs()
            < 0.001
    );
    assert_eq!(
        r["candidates"][0]["uncertainty"]["method"],
        "finite_mode_and_tested_profile_union"
    );
    assert_eq!(e, before);
    reason(&near_finite(2000.), "ambiguous_locations");
    let mut different = near_finite(30.);
    different["candidates"][1]["flight_ids"] = json!(["x", "y"]);
    reason(&different, "ambiguous_flight_match");
}

#[test]
fn finite_union_cannot_fix_missing_or_incomplete_uncertainty() {
    let e = near_finite(90.);
    for (p, v, r) in [
        (
            "/matching_diagnostics/sensitivity/complete",
            json!(false),
            "uncertainty_not_established",
        ),
        (
            "/matching_diagnostics/sensitivity/boundary_limited",
            json!(true),
            "refinement_boundary_limited",
        ),
        (
            "/matching_diagnostics/sensitivity/sources",
            json!(["altitude"]),
            "uncertainty_not_established",
        ),
        (
            "/matching_diagnostics/finite_search_evidence/complete",
            json!(false),
            "finite_search_incomplete",
        ),
    ] {
        reason(&mutate(&e, p, v), r);
    }
    for i in 0..2 {
        let mut c = e.clone();
        c["candidates"][i]
            .as_object_mut()
            .unwrap()
            .remove("uncertainty");
        reason(&c, "uncertainty_not_established");
    }
}

#[test]
fn malformed_contracts_fail_explicitly() {
    let e = evidence();
    for (p, v) in [
        ("/events/0/ridge/1/time_s", json!(0.)),
        ("/events/0/ridge/1/frequency_hz", json!(900.)),
        ("/events/0/band_hz", json!([200, 90])),
        ("/events/1/event_id", json!("e0")),
        ("/candidates/0/score", Value::Null),
        ("/candidates/0/lat", json!(100)),
        ("/candidates/0/event_ids", json!(["unknown", "e1"])),
        ("/candidates/0/flight_ids", json!(["a"])),
        ("/coverage/search_complete", json!(1)),
        ("/coverage/data_status", json!("possibly")),
        (
            "/candidates/0/recording_start_utc",
            json!("2026-09-21T14:13:20"),
        ),
        ("/matching_diagnostics/e0/support_fraction", json!(1.1)),
    ] {
        let r = decide(&mutate(&e, p, v));
        assert_eq!(r["status"], "failed", "{p}: {r}");
        assert_eq!(r["reason_code"], "invalid_evidence");
    }
    let mut unsorted = e.clone();
    alternative(&mut unsorted, 0., 0.);
    reason(&unsorted, "invalid_evidence");
    let mut mixed = e;
    alternative(&mut mixed, 0., -4.);
    mixed["candidates"][1]["score_kind"] = json!("other");
    reason(&mixed, "invalid_evidence");
}

#[test]
fn sensitivity_cannot_be_relabelled_as_calibrated_probability() {
    let mut e = evidence();
    e["candidates"][0]["uncertainty"]["coverage_probability"] = json!(0.95);
    reason(&e, "invalid_evidence");
    e["candidates"][0]["uncertainty"]["kind"] = json!("calibrated");
    e["candidates"][0]["uncertainty"]["calibration_reference"] = json!("unverified external claim");
    reason(&e, "calibration_not_verified");
}

#[test]
fn malformed_finite_ledgers_fail_explicitly() {
    let e = finite();
    for (p, v) in [
        (
            "/matching_diagnostics/finite_search_evidence/complete",
            json!(1),
        ),
        (
            "/matching_diagnostics/finite_search_evidence/spacing_m",
            json!(0.),
        ),
        (
            "/matching_diagnostics/finite_search_evidence/max_event_groups",
            json!(1),
        ),
        (
            "/matching_diagnostics/finite_search_evidence/retained_modes/1/mode_id",
            json!("a"),
        ),
        (
            "/matching_diagnostics/finite_search_evidence/discarded_count",
            json!(-1),
        ),
        (
            "/matching_diagnostics/finite_search_evidence/best_discarded_score",
            Value::Null,
        ),
    ] {
        assert_eq!(decide(&mutate(&e, p, v))["status"], "failed", "{p}");
    }
}

#[test]
fn explicit_zone_calendar_and_subsecond_timestamps_are_validated() {
    assert_eq!(utc_seconds("1970-01-01T00:00:00Z").unwrap(), 0.);
    assert_eq!(utc_seconds("1970-01-01T01:30:00+01:30").unwrap(), 0.);
    assert_eq!(utc_seconds("1969-12-31T23:59:59.25Z").unwrap(), -0.75);
    assert_eq!(utc_seconds("2000-02-29T00:00:00Z").unwrap(), 951_782_400.);
    for s in [
        "1900-02-29T00:00:00Z",
        "2026-09-31T00:00:00Z",
        "2026-09-08T24:00:00Z",
        "2026-09-08T-1:00:00Z",
        "2026-09-08T00:00:00+24:00",
        "2026-09-08T00:00:1e1Z",
        "2026-09-08T00:00:00.Z",
        "2026-09-08T00:00:00",
        "2026-09-08T00:00:60Z",
    ] {
        assert!(utc_seconds(s).is_err(), "{s}");
    }
}

#[test]
fn geodesic_does_not_shrink_nearby_or_antipodal_ambiguities() {
    assert!((distance_m(0., 0., 0., 1.).unwrap() - 111_319.490_793).abs() < 0.001);
    assert!((distance_m(0., 0., 1., 0.).unwrap() - 110_574.388_558).abs() < 0.001);
    assert!((distance_m(0., 179.9, 0., -179.9).unwrap() - 22_263.898_159).abs() < 0.001);
    assert!(distance_m(0., 0., 0., 180.).is_err());
    assert_eq!(distance_m(40., -100., 40., -100.).unwrap(), 0.);
}
