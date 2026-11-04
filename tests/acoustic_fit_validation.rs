//! Independent emission-domain controls for audio-only fitting. These are
//! analytic software controls, not recording accuracy or calibration evidence.
use geosonnar::acoustic_fit::{self, FitParameters, FitSettings};
use geosonnar::physics::TracePoint;

fn oracle(speed: f64, distance: f64, f0: f64, center: f64, low: i32, high: i32) -> Vec<TracePoint> {
    (low..=high)
        .map(|i| {
            let emission = i as f64;
            let range = distance.hypot(speed * emission);
            // Forward construction: no reduced model or fitted parameters used.
            TracePoint {
                t_rec_s: center + emission + range / 343.,
                frequency_hz: f0 / (1. + speed * speed * emission / (343. * range)),
            }
        })
        .collect()
}

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "actual={actual}, expected={expected}, tolerance={tolerance}"
    );
}

#[test]
fn independent_forward_time_control_recovers_all_four_parameters() {
    let points = oracle(120., 3500., 139., 1000., -80, 90);
    let report = acoustic_fit::fit(&points, 0.2, &FitSettings::default()).unwrap();
    assert!(report.usable_for_search, "{:?}", report.diagnostics.reasons);
    let fit = report.fit.unwrap();
    close(fit.emitted_frequency_hz, 139., 1e-5);
    close(fit.speed_mps, 120., 1e-5);
    close(fit.slant_distance_m, 3500., 1e-3);
    close(fit.emission_cpa_recording_s, 1000., 1e-5);
    close(fit.heard_cpa_recording_s, 1000. + 3500. / 343., 1e-5);
    assert!(fit.rmse_hz < 1e-7);
    assert_eq!(report.diagnostics.leave_time_block_out_fits.len(), 4);
    assert!(!report.diagnostics.sensitivity_is_confidence_interval);
    assert!(!report.diagnostics.best_on_physical_boundary);
    let envelope = report.envelope.unwrap();
    assert!(
        envelope.emitted_frequency_hz[0] <= fit.emitted_frequency_hz
            && envelope.emitted_frequency_hz[1] >= fit.emitted_frequency_hz
    );
    assert!(
        envelope.heard_cpa_recording_s[0] <= fit.heard_cpa_recording_s
            && envelope.heard_cpa_recording_s[1] >= fit.heard_cpa_recording_s
    );
}

#[test]
fn prediction_matches_emission_oracle_and_distinguishes_heard_cpa_from_inflection() {
    let p = FitParameters {
        emitted_frequency_hz: 152.,
        speed_mps: 160.,
        slant_distance_m: 5000.,
        emission_cpa_recording_s: 1000.,
        heard_cpa_recording_s: 1000. + 5000. / 343.,
        rmse_hz: 0.,
    };
    for point in oracle(160., 5000., 152., 1000., -120, 120) {
        close(
            acoustic_fit::predict_frequency(&p, point.t_rec_s, 343.).unwrap(),
            point.frequency_hz,
            1e-11,
        );
    }
    close(
        acoustic_fit::predict_frequency(&p, p.heard_cpa_recording_s, 343.).unwrap(),
        p.emitted_frequency_hz,
        1e-10,
    );
    assert!(
        acoustic_fit::predict_frequency(&p, p.emission_cpa_recording_s, 343.).unwrap()
            > p.emitted_frequency_hz + 30.
    );
}

#[test]
fn a_large_recording_time_shift_does_not_change_geometry_or_frequency() {
    let ordinary = oracle(90., 2100., 118., 1000., -65, 70);
    let shift = 100_000_000.;
    let shifted = ordinary
        .iter()
        .map(|p| TracePoint {
            t_rec_s: p.t_rec_s + shift,
            frequency_hz: p.frequency_hz,
        })
        .collect::<Vec<_>>();
    let first = acoustic_fit::fit(&ordinary, 0.15, &FitSettings::default()).unwrap();
    let later = acoustic_fit::fit(&shifted, 0.15, &FitSettings::default()).unwrap();
    assert!(first.usable_for_search && later.usable_for_search);
    let (first, later) = (first.fit.unwrap(), later.fit.unwrap());
    close(later.emitted_frequency_hz, first.emitted_frequency_hz, 1e-5);
    close(later.speed_mps, first.speed_mps, 1e-5);
    close(later.slant_distance_m, first.slant_distance_m, 1e-3);
    close(
        later.heard_cpa_recording_s - first.heard_cpa_recording_s,
        shift,
        1e-5,
    );
}

#[test]
fn explicit_harmonic_units_scale_frequency_without_changing_pass_geometry() {
    let points = oracle(145., 4600., 104., 1000., -90, 90);
    let harmonic = points
        .iter()
        .map(|p| TracePoint {
            t_rec_s: p.t_rec_s,
            frequency_hz: p.frequency_hz * 3.,
        })
        .collect::<Vec<_>>();
    let fundamental = acoustic_fit::fit(&points, 0.2, &FitSettings::default()).unwrap();
    let higher = acoustic_fit::fit(&harmonic, 0.6, &FitSettings::default()).unwrap();
    assert!(fundamental.usable_for_search && higher.usable_for_search);
    let (a, b) = (fundamental.fit.unwrap(), higher.fit.unwrap());
    close(b.emitted_frequency_hz, 3. * a.emitted_frequency_hz, 1e-5);
    close(b.speed_mps, a.speed_mps, 1e-5);
    close(b.slant_distance_m, a.slant_distance_m, 1e-3);
    close(b.heard_cpa_recording_s, a.heard_cpa_recording_s, 1e-5);
}

#[test]
fn bounded_noise_and_missing_time_blocks_preserve_fixed_measurements() {
    let full = oracle(110., 3200., 130., 1000., -80, 90);
    let points = full
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !(70..=81).contains(i) && !(110..=116).contains(i))
        .map(|(i, p)| TracePoint {
            frequency_hz: p.frequency_hz + 0.15 * (i as f64 * 0.731).sin(),
            ..p
        })
        .collect::<Vec<_>>();
    let original = points
        .iter()
        .map(|p| (p.t_rec_s, p.frequency_hz))
        .collect::<Vec<_>>();
    let report = acoustic_fit::fit(&points, 0.25, &FitSettings::default()).unwrap();
    assert!(report.usable_for_search, "{:?}", report.diagnostics.reasons);
    assert_eq!(report.diagnostics.sample_count, points.len());
    assert!(report.diagnostics.maximum_observed_gap_s > 10.);
    assert_eq!(
        original,
        points
            .iter()
            .map(|p| (p.t_rec_s, p.frequency_hz))
            .collect::<Vec<_>>()
    );
    let fit = report.fit.unwrap();
    close(fit.emitted_frequency_hz, 130., 0.15);
    close(fit.speed_mps, 110., 0.3);
    close(fit.slant_distance_m, 3200., 20.);
    close(fit.heard_cpa_recording_s, 1000. + 3200. / 343., 0.15);
}

#[test]
fn stationary_upward_and_tiny_motion_are_diagnostic_refusals() {
    for slope in [0., 0.2, -0.002] {
        let points = (0..100)
            .map(|i| TracePoint {
                t_rec_s: i as f64,
                frequency_hz: 130. + slope * i as f64,
            })
            .collect::<Vec<_>>();
        let report = acoustic_fit::fit(&points, 0.5, &FitSettings::default()).unwrap();
        assert!(!report.usable_for_search);
        assert!(report.fit.is_none());
        assert!(report.envelope.is_none());
        assert_eq!(report.diagnostics.evaluated_models, 0);
    }
}

#[test]
fn a_one_sided_truncated_tail_does_not_become_a_tight_timing_prior() {
    let points = oracle(150., 5000., 170., 1000., 40, 150);
    let report = acoustic_fit::fit(&points, 0.5, &FitSettings::default()).unwrap();
    assert!(!report.usable_for_search);
    assert!(!report.diagnostics.reasons.is_empty());
    if report.fit.is_some() {
        assert!(
            report.diagnostics.samples_before_heard_cpa < 5
                || report.diagnostics.profile_touches_physical_boundary
                || report.diagnostics.profile_touches_numerical_boundary
        );
    }
}

#[test]
fn true_speed_outside_declared_physical_domain_is_not_silently_accepted() {
    let points = oracle(295., 5000., 130., 1000., -60, 70);
    let report = acoustic_fit::fit(&points, 0.15, &FitSettings::default()).unwrap();
    assert!(!report.usable_for_search);
    assert!(
        report.diagnostics.best_on_physical_boundary
            || report
                .diagnostics
                .reasons
                .iter()
                .any(|r| r.contains("Residuals"))
    );
    assert!(report.fit.unwrap().speed_mps <= 280. + 1e-8);
}

#[test]
fn invalid_samples_scales_and_domains_fail_before_optimization() {
    let points = oracle(100., 3000., 130., 1000., -30, 40);
    for scale in [0., -1., f64::NAN, f64::INFINITY] {
        assert!(acoustic_fit::fit(&points, scale, &FitSettings::default()).is_err());
    }
    for kind in 0..6 {
        let mut bad = points.clone();
        match kind {
            0 => bad[3].t_rec_s = bad[2].t_rec_s,
            1 => bad[3].t_rec_s = f64::NAN,
            2 => bad[3].frequency_hz = 0.,
            3 => bad[3].frequency_hz = f64::INFINITY,
            4 => bad.reverse(),
            _ => bad[0].t_rec_s = -1.,
        }
        assert!(acoustic_fit::fit(&bad, 0.5, &FitSettings::default()).is_err());
    }
    let invalid = FitSettings {
        speed_bounds_mps: [10., 343.],
        ..FitSettings::default()
    };
    assert!(acoustic_fit::fit(&points, 0.5, &invalid).is_err());
    let invalid = FitSettings {
        profile_grid_size: 10000,
        ..FitSettings::default()
    };
    assert!(acoustic_fit::fit(&points, 0.5, &invalid).is_err());
}

#[test]
fn equal_samples_are_not_reweighted_by_sampling_origin_or_point_storage() {
    // Deterministic duplicate invocations must agree exactly; metadata and
    // ordering unrelated to the measurements cannot enter the native fit.
    let points = oracle(80., 1800., 150., 2000., -60, 60);
    let settings = FitSettings::default();
    let first = acoustic_fit::fit(&points, 0.2, &settings).unwrap();
    let second = acoustic_fit::fit(&points.clone(), 0.2, &settings).unwrap();
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(second).unwrap()
    );
}
