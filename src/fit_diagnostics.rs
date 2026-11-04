//! Descriptive, in-sample checks of an inspected Doppler curve against simple
//! observed-tone baselines. These diagnostics never contribute to scoring.
use crate::workspace::TracePoint;
use serde::{Deserialize, Serialize};

/// Bound work and allocation even when this public helper is called outside the
/// overlay builder. The overlay has the same upper bound for all its samples.
pub const MAX_MEASURED_POINTS: usize = 20_000;

#[derive(Clone, Debug)]
pub struct MeasuredPrediction {
    pub point_index: usize,
    pub time_s: f64,
    pub observed_frequency_hz: f64,
    pub predicted_frequency_hz: f64,
    /// Exactly the effective frequency scale used by the inspected scorer.
    pub effective_scale_hz: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorSummary {
    /// Ordinary, unweighted root mean squared error in Hz. Baselines minimize
    /// normalized_rmse, so this need not be the least possible unweighted RMSE.
    pub rmse_hz: f64,
    /// sqrt(mean((prediction - observation)^2 / effective_scale_hz^2)).
    /// Descriptive only: scales and correlated observations are not calibrated.
    pub normalized_rmse: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToneComparison {
    /// The existing hypothesis, including its supplied emitted frequency. No
    /// Doppler parameter is fitted or adjusted by this diagnostic.
    pub doppler: ErrorSummary,
    pub flat: ErrorSummary,
    pub linear: ErrorSummary,
    pub flat_frequency_hz: f64,
    pub linear_reference_time_s: f64,
    pub linear_frequency_at_reference_hz: f64,
    pub linear_slope_hz_per_s: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FitDiagnostics {
    pub observed_points: usize,
    /// Expected measurements with exactly one valid, matching prediction.
    /// Duplicate, missing or invalid records never count as supported.
    pub supported_points: usize,
    pub span_s: Option<f64>,
    pub comparison: Option<ToneComparison>,
    pub unavailable_reason: Option<String>,
    pub limitations: Vec<String>,
}

fn limitations() -> Vec<String> {
    vec![
        "This is an in-sample description of the inspected hypothesis, not a source identification, location verification, probability or acceptance test.".into(),
        "Flat and linear observed-tone baselines fit one and two parameters respectively, using the same measured points and inverse-squared residual-scale weights. Their errors are not corrected for parameter count.".into(),
        "The Doppler curve and emitted frequency are unchanged. The number of receiver, source, clock, frequency or other alternatives searched before inspection is unknown here; this comparison does not account for that search.".into(),
        "RMSE in Hz is unweighted. Normalized RMSE uses each point's original scoring scale; the baselines minimize normalized squared error. Display samples are excluded.".into(),
        "Closely spaced trace measurements can be correlated. Smaller errors, including an advantage over both baselines, do not establish statistical significance, constant source pitch or the correct aircraft.".into(),
    ]
}

fn unavailable(mut result: FitDiagnostics, reason: &str) -> FitDiagnostics {
    result.unavailable_reason = Some(reason.into());
    result
}

/// Compare a complete measured trace with its already computed Doppler values.
/// Every observation must have exactly one matching prediction; comparisons on
/// a convenient supported subset would conceal missing physical support.
pub fn compare(points: &[TracePoint], predictions: &[MeasuredPrediction]) -> FitDiagnostics {
    let mut result = FitDiagnostics {
        observed_points: points.len(),
        supported_points: 0,
        span_s: None,
        comparison: None,
        unavailable_reason: None,
        limitations: limitations(),
    };
    if points.len() > MAX_MEASURED_POINTS || predictions.len() > MAX_MEASURED_POINTS {
        return unavailable(result, "Measured-point diagnostic budget exceeded.");
    }
    if points.iter().any(|p| {
        !p.time_s.is_finite()
            || p.time_s < 0.0
            || !p.frequency_hz.is_finite()
            || p.frequency_hz <= 0.0
    }) || points.windows(2).any(|p| p[0].time_s >= p[1].time_s)
    {
        return unavailable(
            result,
            "Observations need finite positive frequencies and distinct increasing recording-relative times.",
        );
    }
    if let (Some(first), Some(last)) = (points.first(), points.last()) {
        result.span_s = Some(last.time_s - first.time_s);
    }
    // At most 20,000 slots; invalid external input cannot request an unbounded
    // allocation through its point indices or observation count.
    let mut slots = vec![None; points.len()];
    let mut occurrences = vec![0usize; points.len()];
    let mut invalid = false;
    for prediction in predictions {
        let Some(point) = points.get(prediction.point_index) else {
            invalid = true;
            continue;
        };
        occurrences[prediction.point_index] += 1;
        let matches = prediction.time_s.to_bits() == point.time_s.to_bits()
            && prediction.observed_frequency_hz.to_bits() == point.frequency_hz.to_bits()
            && prediction.predicted_frequency_hz.is_finite()
            && prediction.predicted_frequency_hz > 0.0
            && prediction.effective_scale_hz.is_finite()
            && prediction.effective_scale_hz > 0.0;
        if matches {
            slots[prediction.point_index] = Some(prediction);
        } else {
            invalid = true;
        }
    }
    result.supported_points = slots
        .iter()
        .zip(&occurrences)
        .filter(|(slot, count)| slot.is_some() && **count == 1)
        .count();
    if invalid || result.supported_points != points.len() {
        return unavailable(
            result,
            "Every observed point needs exactly one valid Doppler prediction with its unchanged time, frequency and scoring scale; missing, duplicate or invalid predictions prevent comparison.",
        );
    }
    if points.len() < 3 {
        return unavailable(
            result,
            "At least three distinct measured times are required for the flat and two-parameter linear comparison.",
        );
    }
    let predictions: Vec<_> = slots.into_iter().flatten().collect();
    match fit(points, &predictions) {
        Some(comparison) => {
            result.comparison = Some(comparison);
            result
        }
        None => unavailable(
            result,
            "The time span or residual scales do not allow a finite, numerically supported baseline comparison.",
        ),
    }
}

fn errors(
    points: &[TracePoint],
    predictions: &[&MeasuredPrediction],
    fitted: impl Fn(usize) -> f64,
) -> Option<ErrorSummary> {
    let root_count = (points.len() as f64).sqrt();
    let mut rmse_hz: f64 = 0.0;
    let mut normalized_rmse: f64 = 0.0;
    for (i, point) in points.iter().enumerate() {
        let residual = fitted(i) - point.frequency_hz;
        let normalized = residual / predictions[i].effective_scale_hz;
        if !residual.is_finite() || !normalized.is_finite() {
            return None;
        }
        // Divide before accumulating so a finite RMS is not lost to an
        // overflowing sum of squares (or norm prior to division).
        rmse_hz = rmse_hz.hypot(residual / root_count);
        normalized_rmse = normalized_rmse.hypot(normalized / root_count);
    }
    (rmse_hz.is_finite() && normalized_rmse.is_finite()).then_some(ErrorSummary {
        rmse_hz,
        normalized_rmse,
    })
}

fn fit(points: &[TracePoint], predictions: &[&MeasuredPrediction]) -> Option<ToneComparison> {
    let first_time = points.first()?.time_s;
    let span = points.last()?.time_s - first_time;
    if !span.is_finite() || span <= 0.0 {
        return None;
    }
    let min_scale = predictions
        .iter()
        .map(|p| p.effective_scale_hz)
        .fold(f64::INFINITY, f64::min);
    let frequency_scale = points.iter().map(|p| p.frequency_hz).fold(0.0, f64::max);
    let weights: Vec<_> = predictions
        .iter()
        .map(|p| (min_scale / p.effective_scale_hz).powi(2))
        .collect();
    if weights.iter().any(|w| !w.is_finite() || *w <= 0.0) {
        // Never silently drop observations when relative weights underflow.
        return None;
    }
    let weight_sum: f64 = weights.iter().sum();
    let normalized_time: Vec<_> = points
        .iter()
        .map(|p| (p.time_s - first_time) / span)
        .collect();
    let mean_time = normalized_time
        .iter()
        .zip(&weights)
        .map(|(x, w)| x * w)
        .sum::<f64>()
        / weight_sum;
    let mean_frequency = points
        .iter()
        .zip(&weights)
        .map(|(p, w)| (p.frequency_hz / frequency_scale) * w)
        .sum::<f64>()
        / weight_sum;
    let mut variance = 0.0;
    let mut covariance = 0.0;
    for ((point, time), weight) in points.iter().zip(&normalized_time).zip(&weights) {
        let dx = time - mean_time;
        variance += weight * dx * dx;
        covariance += weight * dx * (point.frequency_hz / frequency_scale - mean_frequency);
    }
    if !variance.is_finite() || variance <= 0.0 || !covariance.is_finite() {
        return None;
    }
    let slope_scaled = covariance / variance;
    let flat_frequency_hz = mean_frequency * frequency_scale;
    let linear_reference_time_s = first_time + mean_time * span;
    let linear_slope_hz_per_s = (slope_scaled * frequency_scale) / span;
    if !flat_frequency_hz.is_finite()
        || !linear_reference_time_s.is_finite()
        || !linear_slope_hz_per_s.is_finite()
    {
        return None;
    }
    Some(ToneComparison {
        doppler: errors(points, predictions, |i| {
            predictions[i].predicted_frequency_hz
        })?,
        flat: errors(points, predictions, |_| flat_frequency_hz)?,
        linear: errors(points, predictions, |i| {
            (mean_frequency + slope_scaled * (normalized_time[i] - mean_time)) * frequency_scale
        })?,
        flat_frequency_hz,
        linear_reference_time_s,
        linear_frequency_at_reference_hz: flat_frequency_hz,
        linear_slope_hz_per_s,
    })
}
