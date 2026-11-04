//! Independent arithmetic and adversarial controls for descriptive tone fits.
use geosonnar::{
    fit_diagnostics::{self, MAX_MEASURED_POINTS, MeasuredPrediction},
    workspace::TracePoint,
};

fn predictions(points: &[TracePoint], frequency: impl Fn(usize) -> f64) -> Vec<MeasuredPrediction> {
    points
        .iter()
        .enumerate()
        .map(|(point_index, point)| MeasuredPrediction {
            point_index,
            time_s: point.time_s,
            observed_frequency_hz: point.frequency_hz,
            predicted_frequency_hz: frequency(point_index),
            effective_scale_hz: 2.0,
        })
        .collect()
}

fn trace(values: &[f64]) -> Vec<TracePoint> {
    values
        .iter()
        .enumerate()
        .map(|(i, frequency_hz)| TracePoint {
            time_s: i as f64,
            frequency_hz: *frequency_hz,
        })
        .collect()
}

fn near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-11 * expected.abs().max(1.0),
        "{actual} != {expected}"
    );
}

#[test]
fn unequal_scales_use_weighted_baselines_but_ordinary_hz_rmse() {
    let points = trace(&[10., 12., 11., 16.]);
    let mut values = predictions(&points, |_| 11.);
    for (value, scale) in values.iter_mut().zip([1., 2., 1., 2.]) {
        value.effective_scale_hz = scale;
    }
    let result = fit_diagnostics::compare(&points, &values);
    assert_eq!(result.observed_points, 4);
    assert_eq!(result.supported_points, 4);
    assert_eq!(result.span_s, Some(3.));
    assert!(result.unavailable_reason.is_none());
    let comparison = result.comparison.unwrap();
    // Hand calculation: sum(w)=2.5, weighted x=1.2, weighted y=11.2,
    // unnormalized centered covariance=3.4, variance=2.9.
    near(comparison.flat_frequency_hz, 11.2);
    near(comparison.linear_reference_time_s, 1.2);
    near(comparison.linear_frequency_at_reference_hz, 11.2);
    near(comparison.linear_slope_hz_per_s, 34. / 29.);
    near(comparison.doppler.rmse_hz, 6.75_f64.sqrt());
    near(comparison.doppler.normalized_rmse, 1.875_f64.sqrt());
    near(comparison.flat.rmse_hz, 6.29_f64.sqrt());
    near(comparison.flat.normalized_rmse, 1.85_f64.sqrt());
    // Line residuals are [-6, -30, 33, -78]/29.
    near(comparison.linear.rmse_hz, (8109.0_f64 / 3364.0).sqrt());
    near(
        comparison.linear.normalized_rmse,
        (2871.0_f64 / 3364.0).sqrt(),
    );
}

#[test]
fn flat_tone_and_linear_drift_can_outperform_a_misleading_curved_prediction() {
    for points in [trace(&[100.; 5]), trace(&[100., 102., 104., 106., 108.])] {
        let values = predictions(&points, |i| 101. + (i as f64 - 2.).powi(2));
        let result = fit_diagnostics::compare(&points, &values);
        let comparison = result.comparison.unwrap();
        assert!(comparison.doppler.rmse_hz > 1.);
        near(comparison.linear.rmse_hz, 0.);
        if points[0].frequency_hz == points[4].frequency_hz {
            near(comparison.flat.rmse_hz, 0.);
            near(comparison.linear_slope_hz_per_s, 0.);
        } else {
            near(comparison.flat.rmse_hz, 8.0_f64.sqrt());
            near(comparison.linear_slope_hz_per_s, 2.);
        }
    }
}

#[test]
fn curved_moving_source_control_beats_both_simple_tones_without_refitting() {
    // Independent constant-speed source, evaluated at emission times. Reception
    // time includes range/c and frequency uses v dot range / range. No native
    // trajectory or scoring implementation generates these measurements.
    let points: Vec<_> = [-20., -10., 0., 10., 20.]
        .into_iter()
        .map(|emission: f64| {
            let speed: f64 = 100.;
            let range = ((speed * emission).powi(2) + 1000.0_f64.powi(2)).sqrt();
            TracePoint {
                time_s: 30. + emission + range / 343.,
                frequency_hz: 120. / (1. + speed * speed * emission / (343. * range)),
            }
        })
        .collect();
    let values = predictions(&points, |i| points[i].frequency_hz);
    let before_points = serde_json::to_value(&points).unwrap();
    let before_values: Vec<_> = values
        .iter()
        .map(|p| {
            (
                p.predicted_frequency_hz.to_bits(),
                p.effective_scale_hz.to_bits(),
            )
        })
        .collect();
    let result = fit_diagnostics::compare(&points, &values);
    let comparison = result.comparison.unwrap();
    assert_eq!(comparison.doppler.rmse_hz, 0.);
    assert_eq!(comparison.doppler.normalized_rmse, 0.);
    assert!(comparison.flat.rmse_hz > 10.);
    assert!(comparison.linear.rmse_hz > 1.);
    assert_eq!(before_points, serde_json::to_value(&points).unwrap());
    assert_eq!(
        before_values,
        values
            .iter()
            .map(|p| (
                p.predicted_frequency_hz.to_bits(),
                p.effective_scale_hz.to_bits()
            ))
            .collect::<Vec<_>>()
    );
}

#[test]
fn prediction_input_order_does_not_change_result() {
    let points = trace(&[100., 105., 101., 99.]);
    let mut values = predictions(&points, |i| 103. - i as f64);
    let original = fit_diagnostics::compare(&points, &values);
    values.reverse();
    assert_eq!(original, fit_diagnostics::compare(&points, &values));
}

#[test]
fn incomplete_or_duplicate_support_never_compares_a_favorable_subset() {
    let points = trace(&[100., 120., 80., 110.]);
    let mut values = predictions(&points, |i| points[i].frequency_hz);
    values.pop();
    let missing = fit_diagnostics::compare(&points, &values);
    assert_eq!(missing.supported_points, 3);
    assert!(missing.comparison.is_none());
    assert!(missing.unavailable_reason.is_some());
    values.push(values[0].clone());
    let duplicated = fit_diagnostics::compare(&points, &values);
    assert_eq!(duplicated.supported_points, 2);
    assert!(duplicated.comparison.is_none());
    values[3].point_index = usize::MAX;
    let out_of_bounds = fit_diagnostics::compare(&points, &values);
    assert_eq!(out_of_bounds.supported_points, 3);
    assert!(out_of_bounds.comparison.is_none());
}

#[test]
fn mismatched_measurements_invalid_predictions_and_scales_are_unavailable() {
    let points = trace(&[100., 105., 102.]);
    for bad_case in 0..9 {
        let mut values = predictions(&points, |i| points[i].frequency_hz);
        match bad_case {
            0 => values[1].time_s += 0.5,
            1 => values[1].observed_frequency_hz += 1.,
            2 => values[1].predicted_frequency_hz = f64::NAN,
            3 => values[1].predicted_frequency_hz = f64::INFINITY,
            4 => values[1].predicted_frequency_hz = 0.,
            5 => values[1].effective_scale_hz = 0.,
            6 => values[1].effective_scale_hz = -1.,
            7 => values[1].effective_scale_hz = f64::INFINITY,
            _ => values[1].effective_scale_hz = f64::NAN,
        }
        let result = fit_diagnostics::compare(&points, &values);
        assert_eq!(result.supported_points, 2);
        assert!(result.comparison.is_none(), "bad case {bad_case}");
        assert!(serde_json::to_string(&result).is_ok());
    }
}

#[test]
fn insufficient_or_repeated_times_and_invalid_observations_are_unavailable() {
    for count in 0..3 {
        let points = trace(&[100., 101.][..count]);
        let values = predictions(&points, |_| 100.);
        let result = fit_diagnostics::compare(&points, &values);
        assert_eq!(result.supported_points, count);
        assert!(result.comparison.is_none());
    }
    for bad_case in 0..5 {
        let mut points = trace(&[100., 101., 102.]);
        match bad_case {
            0 => points[1].time_s = points[0].time_s,
            1 => points[0].time_s = -1.,
            2 => points[1].time_s = f64::INFINITY,
            3 => points[1].frequency_hz = f64::NAN,
            _ => points[1].frequency_hz = 0.,
        }
        let values = predictions(&points, |_| 100.);
        assert!(
            fit_diagnostics::compare(&points, &values)
                .comparison
                .is_none()
        );
    }
}

#[test]
fn numeric_extremes_do_not_emit_nonfinite_or_silently_drop_weights() {
    let points = trace(&[100., 101., 102.]);
    let mut values = predictions(&points, |_| 103.);
    values[0].effective_scale_hz = f64::MIN_POSITIVE;
    let result = fit_diagnostics::compare(&points, &values);
    assert!(result.comparison.is_none());
    assert_eq!(result.supported_points, 3);
    assert!(serde_json::to_string(&result).is_ok());
    for value in &mut values {
        value.effective_scale_hz = f64::MIN_POSITIVE;
        value.predicted_frequency_hz = f64::MAX;
    }
    assert!(
        fit_diagnostics::compare(&points, &values)
            .comparison
            .is_none()
    );
}

#[test]
fn exact_common_scale_change_preserves_fitted_hz_and_changes_only_normalized_error() {
    let points = trace(&[100., 106., 101., 103.]);
    let mut values = predictions(&points, |_| 105.);
    let before = fit_diagnostics::compare(&points, &values)
        .comparison
        .unwrap();
    for value in &mut values {
        value.effective_scale_hz *= 4.;
    }
    let after = fit_diagnostics::compare(&points, &values)
        .comparison
        .unwrap();
    assert_eq!(before.flat_frequency_hz, after.flat_frequency_hz);
    assert_eq!(before.linear_slope_hz_per_s, after.linear_slope_hz_per_s);
    for (before, after) in [
        (before.doppler, after.doppler),
        (before.flat, after.flat),
        (before.linear, after.linear),
    ] {
        assert_eq!(before.rmse_hz, after.rmse_hz);
        near(before.normalized_rmse / 4., after.normalized_rmse);
    }
}

#[test]
fn centered_fit_handles_large_recording_offsets_without_changing_slope() {
    let mut points = trace(&[100., 102., 104., 106.]);
    for point in &mut points {
        point.time_s += 1e12;
    }
    let values = predictions(&points, |_| 103.);
    let comparison = fit_diagnostics::compare(&points, &values)
        .comparison
        .unwrap();
    assert_eq!(comparison.linear_reference_time_s, 1e12 + 1.5);
    near(comparison.linear_slope_hz_per_s, 2.);
    near(comparison.linear.rmse_hz, 0.);
}

#[test]
fn point_count_bound_is_checked_before_index_allocation() {
    let points = vec![
        TracePoint {
            time_s: 0.,
            frequency_hz: 100.
        };
        MAX_MEASURED_POINTS + 1
    ];
    let result = fit_diagnostics::compare(&points, &[]);
    assert_eq!(result.observed_points, MAX_MEASURED_POINTS + 1);
    assert_eq!(result.supported_points, 0);
    assert!(result.comparison.is_none());
    assert!(result.unavailable_reason.unwrap().contains("budget"));
    let small = trace(&[100., 101., 102.]);
    let one = predictions(&small, |_| 100.);
    let too_many = vec![one[0].clone(); MAX_MEASURED_POINTS + 1];
    assert!(
        fit_diagnostics::compare(&small, &too_many)
            .comparison
            .is_none()
    );
}

#[test]
fn limits_state_parameter_count_search_history_and_source_ambiguity() {
    let points = trace(&[100., 101., 102.]);
    let values = predictions(&points, |_| 101.);
    let result = fit_diagnostics::compare(&points, &values);
    let text = result.limitations.join(" ");
    for required in [
        "one and two parameters",
        "unknown here",
        "correlated",
        "not a source identification",
        "unweighted",
    ] {
        assert!(text.contains(required), "missing {required}");
    }
}
