//! Audio-only ridge quality, duplicate handling, fragments, and harmonics.
//!
//! Adapted from the MIT DOPPLERGEIST discovery contract at commit
//! 70626ad44fb9c7f4e2f5b225956c1f9281f91c13. These functions identify moving-tone
//! proposals, not aircraft. No position, flight, clock, or evaluation label is
//! used. Downward electronic sweeps can be acoustically indistinguishable.

use crate::discovery::{DiscoverySettings, Ridge, RidgePoint};
use serde::Serialize;

const COMPARISON_HOP_S: f64 = 0.5;
const COMPARISON_MAX_GAP_S: f64 = 2.0;
const TIME_ROUNDOFF_S: f64 = 1e-8;

#[derive(Clone, Debug, Serialize)]
pub struct RidgeQuality {
    pub median_contrast: f64,
    pub support_fraction: f64,
    pub frequency_resolution_hz: f64,
    pub time_uncertainty_s: f64,
    pub frequency_uncertainty_hz: f64,
    pub frequency_sigma_hz: f64,
    pub fragment_count: usize,
    pub max_observed_gap_s: f64,
    pub frequency_drop_hz: f64,
    pub duration_s: f64,
    pub monotone_fraction: f64,
    pub moving_fraction: f64,
    pub max_frequency_jump_hz: f64,
    pub median_jitter_hz: f64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct FragmentLink {
    pub from_start_s: f64,
    pub from_end_s: f64,
    pub to_start_s: f64,
    pub to_end_s: f64,
    pub gap_s: f64,
    pub accepted: bool,
    pub reason: String,
}

fn valid(path: &[RidgePoint]) -> bool {
    path.len() >= 2
        && path.iter().all(|p| {
            p.time_s.is_finite()
                && p.time_s >= 0.0
                && p.frequency_hz.is_finite()
                && p.frequency_hz > 0.0
                && p.weight.is_finite()
                && p.weight >= 0.0
        })
        && path.windows(2).all(|p| p[0].time_s < p[1].time_s)
}

fn quantile(values: &[f64], fraction: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = fraction * (sorted.len() - 1) as f64;
    let low = at.floor() as usize;
    let high = at.ceil() as usize;
    sorted[low] + (sorted[high] - sorted[low]) * (at - low as f64)
}

fn median(values: &[f64]) -> f64 {
    quantile(values, 0.5)
}

fn frequencies(path: &[RidgePoint]) -> Vec<f64> {
    path.iter().map(|p| p.frequency_hz).collect()
}

fn smooth_five(frequency: &[f64]) -> Vec<f64> {
    (0..frequency.len())
        .map(|i| {
            let neighbors: Vec<_> = (-2..=2)
                .map(|offset| {
                    frequency[(i as isize + offset).clamp(0, frequency.len() as isize - 1) as usize]
                })
                .collect();
            median(&neighbors)
        })
        .collect()
}

/// Describe the actual measurements, including absent support after stitching.
/// Resolution/MAD are diagnostics, not calibrated confidence intervals.
pub fn quality(path: &[RidgePoint], settings: &DiscoverySettings) -> Option<RidgeQuality> {
    if !valid(path)
        || path.len() < 3
        || ![settings.window_s, settings.hop_s, settings.max_gap_s]
            .iter()
            .all(|v| v.is_finite() && *v > 0.0)
    {
        return None;
    }
    let duration = path.last()?.time_s - path[0].time_s;
    let f = frequencies(path);
    let smooth = smooth_five(&f);
    let edge = 3.max(path.len() / 6).min(path.len());
    let drop = median(&f[..edge]) - median(&f[f.len() - edge..]);
    let gaps: Vec<_> = path.windows(2).map(|p| p[1].time_s - p[0].time_s).collect();
    let changes: Vec<_> = smooth.windows(2).map(|f| f[1] - f[0]).collect();
    let jitter: Vec<_> = f.iter().zip(&smooth).map(|(a, b)| (a - b).abs()).collect();
    let jitter = median(&jitter);
    let sigma = (1.0 / settings.window_s).max(1.4826 * jitter);
    let moving_threshold = 0.05_f64.max(drop / duration * 0.2);
    Some(RidgeQuality {
        median_contrast: median(&path.iter().map(|p| p.weight).collect::<Vec<_>>()),
        support_fraction: (path.len() as f64 / (duration / settings.hop_s + 1.0)).min(1.0),
        frequency_resolution_hz: 1.0 / settings.window_s,
        time_uncertainty_s: settings.window_s / 2.0,
        frequency_uncertainty_hz: sigma,
        frequency_sigma_hz: sigma,
        fragment_count: 1 + gaps.iter().filter(|&&gap| gap > settings.max_gap_s).count(),
        max_observed_gap_s: gaps.iter().copied().fold(0.0, f64::max),
        frequency_drop_hz: drop,
        duration_s: duration,
        monotone_fraction: changes.iter().filter(|&&change| change <= 0.5).count() as f64
            / changes.len() as f64,
        moving_fraction: changes
            .iter()
            .zip(&gaps)
            .filter(|(change, gap)| -**change / **gap > moving_threshold)
            .count() as f64
            / changes.len() as f64,
        max_frequency_jump_hz: changes
            .iter()
            .map(|change| change.abs())
            .fold(0.0, f64::max),
        median_jitter_hz: jitter,
    })
}

/// Apply to individual measured paths before fragment stitching. Missing spans
/// in stitched output remain explicit and must not become invented observations.
pub fn usable(path: &[RidgePoint], settings: &DiscoverySettings) -> bool {
    let Some(q) = quality(path, settings) else {
        return false;
    };
    if ![
        settings.min_duration_s,
        settings.min_support,
        settings.min_drop_hz,
        settings.min_fractional_drop,
    ]
    .iter()
    .all(|value| value.is_finite() && *value > 0.0)
        || settings.min_support > 1.0
    {
        return false;
    }
    let center_frequency = median(&frequencies(path));
    q.duration_s >= settings.min_duration_s
        && q.support_fraction >= settings.min_support
        && q.frequency_drop_hz
            >= settings
                .min_drop_hz
                .max(center_frequency * settings.min_fractional_drop)
        && q.monotone_fraction >= 0.75
        && q.moving_fraction >= 0.35
        && q.max_frequency_jump_hz <= (2.0 / settings.window_s).max(q.frequency_drop_hz * 0.25)
        && q.median_jitter_hz <= 1.0_f64.max(center_frequency * 0.01)
}

struct Overlap {
    first: Vec<f64>,
    second: Vec<f64>,
    supported_duration_s: f64,
}

// Comparison values are ephemeral, never emitted as measured ridge samples.
// Unlike interpolation over a whole stitched path, both bracketing measurements
// must be within one supported <=2s interval. No extrapolation or gap filling.
fn supported_frequency(path: &[RidgePoint], time: f64) -> Option<f64> {
    if time < path[0].time_s || time > path.last()?.time_s {
        return None;
    }
    let next = path.partition_point(|p| p.time_s < time);
    if next < path.len() && (path[next].time_s - time).abs() <= TIME_ROUNDOFF_S {
        return Some(path[next].frequency_hz);
    }
    if next == 0 || next == path.len() {
        return None;
    }
    let (a, b) = (&path[next - 1], &path[next]);
    let gap = b.time_s - a.time_s;
    if gap > COMPARISON_MAX_GAP_S || (time - a.time_s).min(b.time_s - time) > 1.0 {
        return None;
    }
    Some(a.frequency_hz + (b.frequency_hz - a.frequency_hz) * (time - a.time_s) / gap)
}

fn overlap(a: &[RidgePoint], b: &[RidgePoint]) -> Option<Overlap> {
    let start = a[0].time_s.max(b[0].time_s);
    let end = a.last()?.time_s.min(b.last()?.time_s);
    if end <= start {
        return None;
    }
    let count = ((end - start) / COMPARISON_HOP_S).floor() as usize + 1;
    let (mut first, mut second) = (Vec::new(), Vec::new());
    let mut supported_duration_s = 0.0;
    let mut previous_supported = false;
    for i in 0..count {
        let t = start + i as f64 * COMPARISON_HOP_S;
        if let (Some(f), Some(g)) = (supported_frequency(a, t), supported_frequency(b, t)) {
            first.push(f);
            second.push(g);
            if previous_supported {
                supported_duration_s += COMPARISON_HOP_S;
            }
            previous_supported = true;
        } else {
            previous_supported = false;
        }
    }
    (first.len() >= 3).then_some(Overlap {
        first,
        second,
        supported_duration_s,
    })
}

fn merge_measurements(mut a: Ridge, b: Ridge) -> Ridge {
    a.extend(b);
    a.sort_by(|x, y| {
        x.time_s
            .total_cmp(&y.time_s)
            .then_with(|| y.weight.total_cmp(&x.weight))
    });
    let mut result: Ridge = Vec::with_capacity(a.len());
    for point in a {
        if let Some(previous) = result.last_mut()
            && (point.time_s - previous.time_s).abs() <= TIME_ROUNDOFF_S
        {
            if point.weight > previous.weight {
                *previous = point;
            }
            continue;
        }
        result.push(point);
    }
    result
}

/// Merge repeated observations in overlapping audio chunks. Each duplicate time
/// retains one original strongest peak, including its original time/frequency.
pub fn merge_duplicates(mut paths: Vec<Ridge>) -> Vec<Ridge> {
    paths.retain(|path| valid(path));
    paths.sort_by(|a, b| a[0].time_s.total_cmp(&b[0].time_s));
    let mut merged: Vec<Ridge> = Vec::new();
    for path in paths {
        let duplicate = merged.iter().position(|previous| {
            let Some(shared) = overlap(&path, previous) else {
                return false;
            };
            if shared.first.len() < 8 || shared.supported_duration_s < 3.5 {
                return false;
            }
            let tolerance = 1.0_f64.max(median(&shared.first) * 0.006);
            let close = shared
                .first
                .iter()
                .zip(&shared.second)
                .filter(|(a, b)| (**a - **b).abs() <= tolerance)
                .count();
            close as f64 / shared.first.len() as f64 >= 0.85
        });
        if let Some(index) = duplicate {
            merged[index] = merge_measurements(std::mem::take(&mut merged[index]), path);
        } else {
            merged.push(path);
        }
    }
    merged
}

fn descending_rate(points: &[RidgePoint]) -> Option<f64> {
    if points.len() < 2 {
        return None;
    }
    let origin = points[0].time_s;
    let mean_t = points.iter().map(|p| p.time_s - origin).sum::<f64>() / points.len() as f64;
    let mean_f = points.iter().map(|p| p.frequency_hz).sum::<f64>() / points.len() as f64;
    let variance = points
        .iter()
        .map(|p| (p.time_s - origin - mean_t).powi(2))
        .sum::<f64>();
    if variance <= 0.0 {
        return None;
    }
    Some(
        -points
            .iter()
            .map(|p| (p.time_s - origin - mean_t) * (p.frequency_hz - mean_f))
            .sum::<f64>()
            / variance,
    )
}

/// Only reciprocal-unique tonal continuations join. No points are generated in
/// missing spans, and all plausible links (including ambiguous ones) are logged.
pub fn stitch_fragments(
    mut paths: Vec<Ridge>,
    settings: &DiscoverySettings,
) -> (Vec<Ridge>, Vec<FragmentLink>) {
    paths.retain(|path| valid(path));
    paths.sort_by(|a, b| a[0].time_s.total_cmp(&b[0].time_s));
    if !settings.max_fragment_gap_s.is_finite()
        || settings.max_fragment_gap_s <= 0.0
        || !settings.window_s.is_finite()
        || settings.window_s <= 0.0
    {
        return (paths, Vec::new());
    }
    let mut outgoing = vec![Vec::new(); paths.len()];
    let mut incoming = vec![Vec::new(); paths.len()];
    for (i, left) in paths.iter().enumerate() {
        let last = left.last().expect("validated path");
        let tail = left.partition_point(|p| p.time_s < last.time_s - 6.0);
        let Some(left_rate) = descending_rate(&left[tail..]) else {
            continue;
        };
        for (j, right) in paths.iter().enumerate().skip(i + 1) {
            let gap = right[0].time_s - last.time_s;
            if gap > settings.max_fragment_gap_s {
                break;
            }
            if gap <= 0.0 || gap / (right.last().unwrap().time_s - left[0].time_s) > 0.35 {
                continue;
            }
            let head = right.partition_point(|p| p.time_s <= right[0].time_s + 6.0);
            let Some(right_rate) = descending_rate(&right[..head]) else {
                continue;
            };
            let drop = last.frequency_hz - right[0].frequency_hz;
            let rate = drop / gap;
            let tolerance = (2.0 / settings.window_s).max(0.015 * last.frequency_hz);
            if left_rate.min(right_rate) <= 0.05
                || drop < -tolerance
                || drop > last.frequency_hz * 0.3
                || rate < 0.4 * left_rate.min(right_rate) - tolerance / gap
                || rate > 2.5 * left_rate.max(right_rate) + tolerance / gap
            {
                continue;
            }
            outgoing[i].push(j);
            incoming[j].push(i);
        }
    }
    let following: Vec<_> = outgoing
        .iter()
        .map(|targets| {
            if targets.len() == 1 && incoming[targets[0]].len() == 1 {
                Some(targets[0])
            } else {
                None
            }
        })
        .collect();
    let mut diagnostics = Vec::new();
    for (i, targets) in outgoing.iter().enumerate() {
        for &j in targets {
            let accepted = following[i] == Some(j);
            diagnostics.push(FragmentLink {
                from_start_s: paths[i][0].time_s,
                from_end_s: paths[i].last().unwrap().time_s,
                to_start_s: paths[j][0].time_s,
                to_end_s: paths[j].last().unwrap().time_s,
                gap_s: paths[j][0].time_s - paths[i].last().unwrap().time_s,
                accepted,
                reason: if accepted {
                    "reciprocal_unique_continuation"
                } else {
                    "ambiguous_continuation"
                }
                .into(),
            });
        }
    }
    let mut consumed = vec![false; paths.len()];
    let mut stitched = Vec::new();
    for first in 0..paths.len() {
        if consumed[first] {
            continue;
        }
        let mut current = first;
        let mut chain = Vec::new();
        loop {
            consumed[current] = true;
            chain.extend_from_slice(&paths[current]);
            match following[current] {
                Some(next) => current = next,
                None => break,
            }
        }
        stitched.push(chain);
    }
    (stitched, diagnostics)
}

fn harmonic_match(base: &[RidgePoint], path: &[RidgePoint]) -> bool {
    let Some(shared) = overlap(base, path) else {
        return false;
    };
    let minimum_span = (base.last().unwrap().time_s - base[0].time_s)
        .min(path.last().unwrap().time_s - path[0].time_s);
    if shared.supported_duration_s < 0.6 * minimum_span {
        return false;
    }
    let ratios: Vec<_> = shared
        .second
        .iter()
        .zip(&shared.first)
        .map(|(high, low)| high / low)
        .collect();
    let center = median(&ratios);
    let mut closest: Option<(f64, f64)> = None;
    for numerator in 2..=8 {
        for denominator in 1..numerator {
            let ratio = numerator as f64 / denominator as f64;
            if ratio >= 1.2 && closest.is_none_or(|(_, error)| (ratio - center).abs() < error) {
                closest = Some((ratio, (ratio - center).abs()));
            }
        }
    }
    let ratio = closest.expect("harmonic ratio list is nonempty").0;
    let deviations: Vec<_> = ratios
        .iter()
        .map(|observed| (observed - ratio).abs() / ratio)
        .collect();
    quantile(&deviations, 0.9) < 0.008
}

/// Rational harmonic ratios include absent fundamentals (e.g. measured 2f/3f).
/// No fundamental or extra observations are synthesized. A ridge compatible
/// with multiple existing groups stays separate rather than joining arbitrarily.
pub fn harmonic_groups(paths: Vec<Ridge>) -> Vec<Vec<Ridge>> {
    let mut ordered: Vec<_> = paths
        .into_iter()
        .filter(|p| valid(p))
        .map(|p| (median(&frequencies(&p)), p))
        .collect();
    ordered.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then_with(|| a.1[0].time_s.total_cmp(&b.1[0].time_s))
    });
    let mut groups: Vec<Vec<Ridge>> = Vec::new();
    for (_, path) in ordered {
        let matches: Vec<_> = groups
            .iter()
            .enumerate()
            .filter(|(_, group)| harmonic_match(&group[0], &path))
            .map(|(i, _)| i)
            .collect();
        if matches.len() == 1 {
            groups[matches[0]].push(path);
        } else {
            groups.push(vec![path]);
        }
    }
    groups.sort_by(|a, b| a[0][0].time_s.total_cmp(&b[0][0].time_s));
    groups
}
