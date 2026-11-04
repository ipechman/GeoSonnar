//! Immutable hypothesis predictions for review against the actual spectrogram.
//! Display sampling never adds scored observations or accepts an association.
use crate::{
    atmosphere::PreparedAtmosphere,
    evidence_score::{
        EvidenceTerm, LightningCatalog, ObserverLocation, ResidualDiagnostic, ScoreReport,
    },
    fit_diagnostics::{self, FitDiagnostics, MeasuredPrediction},
    map_data::MapData,
    physics::geodetic_to_ecef,
    regional_search,
    workspace::{EventKind, EvidenceEvent, ReviewWorkspace, parse_utc},
    workspace_scoring::{self, ReceiverModel},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_FREQUENCY_SAMPLES: usize = 20_000;
pub const MAX_ATMOSPHERE_SAMPLES: usize = 4_000;
pub const MAX_TRACK_POINTS: usize = 100_000;
pub const MAX_ATMOSPHERE_TRACK_POINTS: usize = 10_000;
pub const MAX_SUPPORT_CHECKS: usize = 5_000_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlaySettings {
    /// Desired display spacing; exact measured timestamps are always retained.
    pub target_step_s: f64,
    /// Includes display samples and exact measured-point markers, all events.
    pub max_frequency_samples: usize,
}
impl Default for OverlaySettings {
    fn default() -> Self {
        Self {
            target_step_s: 0.5,
            max_frequency_samples: 4_000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrequencySample {
    /// Original recording-relative reception time, including fractional seconds.
    pub time_s: f64,
    pub arrival_utc_s: f64,
    pub frequency_hz: f64,
    pub measured_point_index: Option<usize>,
    pub observed_frequency_hz: Option<f64>,
    pub normalized_residual: Option<f64>,
    pub effective_residual_scale_hz: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrequencySegment {
    /// Original GeoTrack.segments index, not a newly joined track.
    pub support_segment_index: usize,
    pub trajectory_edge_index: usize,
    pub samples: Vec<FrequencySample>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrequencyEventOverlay {
    pub event_id: String,
    pub source_id: String,
    pub event_label: String,
    pub emitted_frequency_hz: f64,
    pub start_s: f64,
    pub end_s: f64,
    /// Actual maximum requested display spacing for this event; not an error bound.
    pub sampling_step_s: f64,
    pub segments: Vec<FrequencySegment>,
    /// Optional for older saved overlays. Descriptive only; never part of score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit_diagnostics: Option<FitDiagnostics>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimingMarker {
    pub event_id: String,
    pub source_id: String,
    pub kind: EventKind,
    pub predicted_arrival_s: f64,
    pub predicted_arrival_utc_s: f64,
    pub observed_arrival_s: f64,
    pub observed_uncertainty_s: f64,
    pub effective_residual_scale_s: f64,
    pub normalized_residual: f64,
    pub strike_height_assumed: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OverlayUnavailable {
    pub event_id: String,
    pub source_id: String,
    pub start_s: Option<f64>,
    pub end_s: Option<f64>,
    pub measured_point_index: Option<usize>,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PredictionOverlay {
    pub schema_version: u32,
    pub document_sha256: String,
    pub inputs_sha256: String,
    pub observer: ObserverLocation,
    pub clock_adjustment_s: f64,
    pub adjusted_recording_start_utc_s: f64,
    pub model_label: String,
    pub settings: OverlaySettings,
    pub frequency: Vec<FrequencyEventOverlay>,
    pub timing: Vec<TimingMarker>,
    pub unavailable: Vec<OverlayUnavailable>,
    pub frequency_sample_count: usize,
    pub sampling_coarsened: bool,
    pub score_report: ScoreReport,
    pub location_verified: bool,
    pub limitations: Vec<String>,
}

fn issue(
    event: &EvidenceEvent,
    start: Option<f64>,
    end: Option<f64>,
    point: Option<usize>,
    reason: String,
) -> OverlayUnavailable {
    OverlayUnavailable {
        event_id: event.id.clone(),
        source_id: if event.kind == EventKind::Doppler {
            event.flight_id.clone()
        } else {
            event.strike_id.clone()
        },
        start_s: start,
        end_s: end,
        measured_point_index: point,
        reason,
    }
}

fn covering(models: &[ReceiverModel<'_, '_>], arrival: f64) -> Option<usize> {
    let mut found = None;
    for (index, model) in models.iter().enumerate() {
        let [first, last] = model.arrival_bounds_s();
        if arrival >= first && arrival <= last {
            if found.is_some() {
                return None;
            }
            found = Some(index);
        }
    }
    found
}

struct Piece {
    start: f64,
    end: f64,
    model: usize,
    edge: usize,
    include_end: bool,
}

/// Partition at every received track knot and support boundary. No rendered
/// segment spans a gap, ambiguous overlap, or velocity discontinuity.
fn pieces(
    event: &EvidenceEvent,
    start_from_origin: f64,
    models: &[ReceiverModel<'_, '_>],
    unavailable: &mut Vec<OverlayUnavailable>,
) -> Vec<Piece> {
    let first = start_from_origin + event.start_s;
    let last = start_from_origin + event.end_s;
    let mut cuts = vec![first, last];
    for model in models {
        cuts.extend(
            model
                .arrival_knots_s()
                .iter()
                .copied()
                .filter(|t| *t > first && *t < last),
        );
    }
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    let mut result = Vec::new();
    for pair in cuts.windows(2) {
        let middle = pair[0] + (pair[1] - pair[0]) * 0.5;
        let Some(index) = covering(models, middle) else {
            unavailable.push(issue(
                event,
                Some(pair[0] - start_from_origin),
                Some(pair[1] - start_from_origin),
                None,
                "No unique reception-time trajectory support across this interval".into(),
            ));
            continue;
        };
        let knots = models[index].arrival_knots_s();
        let edge = knots
            .partition_point(|&t| t <= middle)
            .saturating_sub(1)
            .min(knots.len() - 2);
        let include_end = covering(models, pair[1]) == Some(index)
            && ((pair[1] == last && (pair[1] < knots[edge + 1] || edge + 2 == knots.len()))
                || (edge + 2 == knots.len() && pair[1] == knots[edge + 1]));
        result.push(Piece {
            start: pair[0],
            end: pair[1],
            model: index,
            edge,
            include_end,
        });
    }
    result
}

fn find_residual<'a>(
    report: &'a ScoreReport,
    term: EvidenceTerm,
    id: &str,
) -> Option<&'a ResidualDiagnostic> {
    report
        .candidates
        .first()?
        .terms
        .iter()
        .find(|t| t.term == term)?
        .residuals
        .iter()
        .find(|r| r.observation_id == id)
}

pub fn build(
    document: &ReviewWorkspace,
    map: &MapData,
    lightning: &Option<LightningCatalog>,
    fallback_strike_height: Option<f64>,
    observer: &ObserverLocation,
    settings: &OverlaySettings,
) -> Result<PredictionOverlay, String> {
    let cap = if document.atmosphere.is_some() {
        MAX_ATMOSPHERE_SAMPLES
    } else {
        MAX_FREQUENCY_SAMPLES
    };
    if !settings.target_step_s.is_finite()
        || !(0.001..=3600.0).contains(&settings.target_step_s)
        || settings.max_frequency_samples == 0
        || settings.max_frequency_samples > cap
    {
        return Err(format!(
            "Prediction overlay needs a display step of 0.001..3600 s and 1..{cap} frequency samples"
        ));
    }
    document.validate()?;
    document.validate_acoustic_sources_for_scoring()?;
    let selected: Vec<_> = document.active_scoring_events().collect();
    if selected.len() > 32 {
        return Err("Prediction overlay supports at most 32 selected events".into());
    }
    let frequency_events: Vec<_> = selected
        .iter()
        .copied()
        .filter(|e| e.kind == EventKind::Doppler && document.mix.doppler_frequency)
        .collect();
    let measured: usize = frequency_events.iter().map(|e| e.points.len()).sum();
    if measured > settings.max_frequency_samples {
        return Err("Exact measured points exceed overlay budget; narrow the reviewed evidence or raise its display limit".into());
    }
    let needed: std::collections::BTreeSet<_> = selected
        .iter()
        .filter(|e| {
            e.kind == EventKind::Doppler
                && (document.mix.doppler_frequency || document.mix.doppler_timing)
        })
        .map(|e| e.flight_id.as_str())
        .collect();
    let track_points = map
        .tracks
        .iter()
        .filter(|t| needed.contains(t.id.as_str()))
        .try_fold(0usize, |total, t| {
            total
                .checked_add(t.physics_track.points.len())
                .ok_or("Overlay trajectory count overflow")
        })?;
    let point_limit = if document.atmosphere.is_some() {
        MAX_ATMOSPHERE_TRACK_POINTS
    } else {
        MAX_TRACK_POINTS
    };
    if track_points > point_limit {
        return Err(format!(
            "Prediction overlay exceeds {point_limit} trajectory points; import a narrower time window"
        ));
    }
    // Unique-support checks scan disjoint source segments, so a large inventory
    // of tiny segments can otherwise cost far more than the displayed samples.
    let mut support_checks = 0usize;
    for event in &frequency_events {
        if let Some(track) = map.tracks.iter().find(|t| t.id == event.flight_id) {
            let segments = track.segments.len();
            let checks = (track.physics_track.points.len() + 2)
                .checked_mul(3)
                .and_then(|n| n.checked_add(event.points.len().checked_mul(4)?))
                .and_then(|n| n.checked_mul(segments))
                .ok_or("Overlay support inventory overflow")?;
            support_checks = support_checks
                .checked_add(checks)
                .ok_or("Overlay support inventory overflow")?;
        }
    }
    if support_checks > MAX_SUPPORT_CHECKS {
        return Err(format!(
            "Prediction overlay exceeds {MAX_SUPPORT_CHECKS} support-boundary checks; import fewer disjoint source segments"
        ));
    }
    let score_report = workspace_scoring::score_observers(
        document,
        map,
        lightning,
        fallback_strike_height,
        std::slice::from_ref(observer),
    )?;
    let fixed_frequency: BTreeMap<_, _> = score_report.candidates[0]
        .candidate
        .frequency
        .iter()
        .map(|p| (p.observation_id.as_str(), p))
        .collect();
    let frequency_residuals: BTreeMap<_, _> = score_report.candidates[0]
        .terms
        .iter()
        .filter(|t| t.term == EvidenceTerm::DopplerFrequency)
        .flat_map(|t| &t.residuals)
        .map(|r| (r.observation_id.as_str(), r))
        .collect();
    let recording_start =
        parse_utc(&document.recording_start_utc)? + document.map.clock_adjustment_s;
    let profile = document
        .atmosphere
        .as_ref()
        .map(PreparedAtmosphere::new)
        .transpose()?;
    let xyz = geodetic_to_ecef(
        observer.latitude_deg,
        observer.longitude_deg,
        observer.ellipsoid_height_m,
    )?;
    let mut tracks = BTreeMap::new();
    let mut original_indices = BTreeMap::new();
    for event in &frequency_events {
        if tracks.contains_key(&event.flight_id) {
            continue;
        }
        let track = map
            .tracks
            .iter()
            .find(|t| t.id == event.flight_id)
            .ok_or("Overlay selected trajectory is missing")?;
        tracks.insert(
            event.flight_id.clone(),
            workspace_scoring::selected_track(track)?,
        );
        original_indices.insert(
            event.flight_id.clone(),
            track
                .segments
                .iter()
                .enumerate()
                .filter_map(|(i, [a, b])| (b - a >= 2).then_some(i))
                .collect::<Vec<_>>(),
        );
    }
    let prepared: BTreeMap<_, _> = tracks
        .iter()
        .map(|(id, track)| {
            (
                id.clone(),
                workspace_scoring::prepare_models(
                    track,
                    xyz,
                    document.sound_speed_mps,
                    profile.as_ref(),
                ),
            )
        })
        .collect();
    let mut unavailable = Vec::new();
    let mut event_pieces = BTreeMap::new();
    let mut mandatory = measured;
    let mut total_duration = 0.0;
    for event in &frequency_events {
        let models = match &prepared[&event.flight_id] {
            Ok(v) => v,
            Err(error) => {
                unavailable.push(issue(
                    event,
                    Some(event.start_s),
                    Some(event.end_s),
                    None,
                    error.clone(),
                ));
                continue;
            }
        };
        let local_start = recording_start - tracks[&event.flight_id].origin;
        let parts = pieces(event, local_start, models, &mut unavailable);
        mandatory = mandatory
            .checked_add(parts.len() * 2)
            .ok_or("Overlay sample inventory overflow")?;
        if mandatory > settings.max_frequency_samples {
            return Err("Track knots and exact measured points exceed overlay budget; narrow the reviewed interval or raise its display limit".into());
        }
        total_duration += parts.iter().map(|p| p.end - p.start).sum::<f64>();
        event_pieces.insert(event.id.clone(), parts);
    }
    if mandatory > settings.max_frequency_samples {
        return Err("Track knots and exact measured points exceed overlay budget; narrow the reviewed interval or raise its display limit".into());
    }
    let remaining = settings.max_frequency_samples - mandatory;
    let step = if remaining == 0 {
        f64::INFINITY
    } else {
        settings
            .target_step_s
            .max(total_duration / remaining as f64)
    };
    let sampling_coarsened = step > settings.target_step_s;
    let mut frequency = Vec::new();
    let mut count = 0;
    for event in frequency_events {
        let mut segments = Vec::new();
        let local_start = recording_start - tracks[&event.flight_id].origin;
        if let (Ok(models), Some(parts)) =
            (&prepared[&event.flight_id], event_pieces.get(&event.id))
        {
            let mut placed = vec![false; event.points.len()];
            for part in parts {
                let first = if covering(models, part.start) == Some(part.model) {
                    part.start
                } else {
                    part.start.next_up()
                };
                let end = if part.include_end {
                    part.end
                } else {
                    part.end.next_down()
                };
                if first > end {
                    continue;
                }
                let mut queries: Vec<(f64, Option<usize>)> = vec![(first, None)];
                let interior = if step.is_finite() {
                    ((end - first) / step).floor() as usize
                } else {
                    0
                };
                for i in 1..=interior {
                    let t = first + step * i as f64;
                    if t < end {
                        queries.push((t, None));
                    }
                }
                if end > first {
                    queries.push((end, None));
                }
                let first_measurement = event
                    .points
                    .partition_point(|p| local_start + p.time_s < part.start);
                for (i, point) in event.points.iter().enumerate().skip(first_measurement) {
                    let arrival = local_start + point.time_s;
                    if arrival > part.end || (arrival == part.end && !part.include_end) {
                        break;
                    }
                    let in_part = arrival >= part.start
                        && (arrival < part.end || (part.include_end && arrival == part.end));
                    if in_part && covering(models, arrival) == Some(part.model) {
                        queries.retain(|(t, index)| *t != arrival || index.is_some());
                        queries.push((arrival, Some(i)));
                        placed[i] = true;
                    }
                }
                queries.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                let mut run = Vec::new();
                for (arrival, point_index) in queries {
                    let factor = models[part.model].factor_at_arrival(arrival);
                    match factor {
                        Ok(factor) => {
                            let predicted = event.emitted_frequency_hz * factor;
                            let time_s = point_index
                                .map(|i| event.points[i].time_s)
                                .unwrap_or(arrival - local_start);
                            let residual = point_index.and_then(|i| {
                                frequency_residuals
                                    .get(format!("{}-sample-{i}", event.id).as_str())
                                    .copied()
                            });
                            if let Some(i) = point_index {
                                let fixed = fixed_frequency
                                    .get(format!("{}-sample-{i}", event.id).as_str())
                                    .ok_or(
                                        "Measured overlay prediction absent from the fixed scorer",
                                    )?;
                                if fixed.predicted_hz.to_bits() != predicted.to_bits() {
                                    return Err(
                                        "Overlay and fixed scorer predictions disagree".into()
                                    );
                                }
                            }
                            run.push(FrequencySample {
                                time_s,
                                arrival_utc_s: recording_start + time_s,
                                frequency_hz: predicted,
                                measured_point_index: point_index,
                                observed_frequency_hz: point_index
                                    .map(|i| event.points[i].frequency_hz),
                                normalized_residual: residual.map(|r| r.normalized_residual),
                                effective_residual_scale_hz: residual.map(|r| r.effective_scale),
                            });
                            count += 1;
                        }
                        Err(error) => {
                            unavailable.push(issue(
                                event,
                                Some(arrival - local_start),
                                Some(arrival - local_start),
                                point_index,
                                error,
                            ));
                            if !run.is_empty() {
                                segments.push(FrequencySegment {
                                    support_segment_index: original_indices[&event.flight_id]
                                        [part.model],
                                    trajectory_edge_index: part.edge,
                                    samples: std::mem::take(&mut run),
                                });
                            }
                        }
                    }
                }
                if !run.is_empty() {
                    segments.push(FrequencySegment {
                        support_segment_index: original_indices[&event.flight_id][part.model],
                        trajectory_edge_index: part.edge,
                        samples: run,
                    });
                }
            }
            for (i, point) in event.points.iter().enumerate().filter(|(i, _)| !placed[*i]) {
                let arrival = local_start + point.time_s;
                if let Some(index) = covering(models, arrival)
                    && let Ok(factor) = models[index].factor_at_arrival(arrival)
                {
                    let predicted = event.emitted_frequency_hz * factor;
                    let id = format!("{}-sample-{i}", event.id);
                    let fixed = fixed_frequency
                        .get(id.as_str())
                        .ok_or("Measured overlay prediction absent from fixed scorer")?;
                    if predicted.to_bits() != fixed.predicted_hz.to_bits() {
                        return Err("Overlay and fixed scorer predictions disagree".into());
                    }
                    let residual = frequency_residuals.get(id.as_str()).copied();
                    let knots = models[index].arrival_knots_s();
                    let edge = knots
                        .partition_point(|&t| t <= arrival)
                        .saturating_sub(1)
                        .min(knots.len() - 2);
                    segments.push(FrequencySegment {
                        support_segment_index: original_indices[&event.flight_id][index],
                        trajectory_edge_index: edge,
                        samples: vec![FrequencySample {
                            time_s: point.time_s,
                            arrival_utc_s: recording_start + point.time_s,
                            frequency_hz: predicted,
                            measured_point_index: Some(i),
                            observed_frequency_hz: Some(point.frequency_hz),
                            normalized_residual: residual.map(|r| r.normalized_residual),
                            effective_residual_scale_hz: residual.map(|r| r.effective_scale),
                        }],
                    });
                    count += 1;
                    continue;
                }
                unavailable.push(issue(
                    event,
                    Some(point.time_s),
                    Some(point.time_s),
                    Some(i),
                    "Measured point has no unique supported rendering segment".into(),
                ));
            }
        }
        let measured_predictions: Vec<_> = segments
            .iter()
            .flat_map(|segment| &segment.samples)
            .filter_map(|sample| {
                sample
                    .measured_point_index
                    .map(|point_index| MeasuredPrediction {
                        point_index,
                        time_s: sample.time_s,
                        observed_frequency_hz: sample.observed_frequency_hz.unwrap_or(f64::NAN),
                        predicted_frequency_hz: sample.frequency_hz,
                        effective_scale_hz: sample.effective_residual_scale_hz.unwrap_or(f64::NAN),
                    })
            })
            .collect();
        let fit_diagnostics = Some(fit_diagnostics::compare(
            &event.points,
            &measured_predictions,
        ));
        frequency.push(FrequencyEventOverlay {
            event_id: event.id.clone(),
            source_id: event.flight_id.clone(),
            event_label: event.label.clone(),
            emitted_frequency_hz: event.emitted_frequency_hz,
            start_s: event.start_s,
            end_s: event.end_s,
            sampling_step_s: if step.is_finite() {
                step
            } else {
                event.end_s - event.start_s
            },
            segments,
            fit_diagnostics,
        });
    }
    if count > settings.max_frequency_samples {
        return Err("Prediction overlay exceeded its declared sample budget".into());
    }
    let mut timing = Vec::new();
    for event in selected {
        let term = match event.kind {
            EventKind::Doppler if document.mix.doppler_timing => EvidenceTerm::DopplerTiming,
            EventKind::Thunder if document.mix.thunder_timing => EvidenceTerm::ThunderTiming,
            _ => continue,
        };
        if let Some(residual) = find_residual(&score_report, term, &event.id) {
            timing.push(TimingMarker {
                event_id: event.id.clone(),
                source_id: if event.kind == EventKind::Doppler {
                    event.flight_id.clone()
                } else {
                    event.strike_id.clone()
                },
                kind: event.kind,
                predicted_arrival_s: residual.predicted,
                predicted_arrival_utc_s: recording_start + residual.predicted,
                observed_arrival_s: residual.observed,
                observed_uncertainty_s: residual.observation_uncertainty,
                effective_residual_scale_s: residual.effective_scale,
                normalized_residual: residual.normalized_residual,
                strike_height_assumed: residual
                    .thunder_prediction
                    .as_ref()
                    .map(|p| p.strike_height_assumed),
            });
        } else {
            let reason = score_report.candidates[0]
                .terms
                .iter()
                .find(|t| t.term == term)
                .and_then(|t| t.unavailable_reason.clone())
                .unwrap_or_else(|| "Timing prediction unavailable".into());
            unavailable.push(issue(
                event,
                Some(event.start_s),
                Some(event.end_s),
                None,
                reason,
            ));
        }
    }
    let mut overlay = PredictionOverlay{schema_version:1,document_sha256:regional_search::document_sha256(document)?,inputs_sha256:regional_search::inputs_sha256(map,lightning)?,observer:observer.clone(),clock_adjustment_s:document.map.clock_adjustment_s,adjusted_recording_start_utc_s:recording_start,
        model_label:document.atmosphere.as_ref().map(|p|format!("Stratified straight-path atmosphere: {}",p.name)).unwrap_or_else(||format!("Constant sound speed: {} m/s",document.sound_speed_mps)),settings:settings.clone(),frequency,timing,unavailable,frequency_sample_count:count,sampling_coarsened,score_report,location_verified:false,
        limitations:vec!["Predicted overlays describe the inspected hypothesis only; displaying them neither applies an association nor verifies a location.".into(),
            "Display curves sample native physical predictions. Separate polylines preserve source support breaks, ambiguous overlaps, trajectory velocity knots and failed sample evaluations; rendering must never join those polylines.".into(),
            "Measured-time predictions use exactly the fixed scorer. Intervening display samples are not additional observations or additional evidence.".into(),
            "Arrival uncertainty and residual scales describe the reviewed observation and scoring assumptions; they are not calibrated prediction or location confidence intervals.".into(),
            "A successful set of displayed samples does not certify continuous physical-model support between them, particularly for a bounded atmospheric model.".into()]};
    overlay
        .limitations
        .splice(0..0, document.acoustic_source_warnings());
    Ok(overlay)
}
