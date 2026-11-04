//! Known-clock single-event diagnostic search. A complete fixed ridge is required.
use crate::{
    physics::{self, EventTrace, ModelConfig, PreparedTrack, Track},
    verification::{CoarseBounds, Observer},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SingleInput {
    pub schema_version: u32,
    pub evidence_kind: String,
    pub provenance: Vec<String>,
    pub tracks: Vec<Track>,
    pub observers: Vec<Observer>,
    pub event: EventTrace,
    pub model: ModelConfig,
    /// Recording sample zero in the same common time origin as track timestamps.
    pub clock_offsets_s: Vec<f64>,
    /// Optional source-informed CPA filter; omit for multi-pass trajectories.
    #[serde(default)]
    pub coarse_bounds: Option<CoarseBounds>,
}

#[derive(Clone, Serialize)]
struct Score {
    flight_id: String,
    rmse_hz: f64,
    fitted_f0_hz: f64,
    clock_offset_s: f64,
    support_fraction: f64,
}

#[derive(Clone, Serialize)]
struct Candidate {
    observer: Observer,
    score: Score,
}

fn validate(input: &SingleInput) -> Result<(), String> {
    if input.schema_version != 1
        || !["synthetic", "real_recording"].contains(&input.evidence_kind.as_str())
        || input.provenance.is_empty()
    {
        return Err("Single-event search requires schema 1, evidence_kind and provenance".into());
    }
    if input.tracks.is_empty()
        || input.tracks.len() > 100_000
        || input.observers.is_empty()
        || input.observers.len() > 100_000
    {
        return Err("Single-event search requires 1..100000 tracks and observers".into());
    }
    if input.clock_offsets_s.is_empty()
        || input.clock_offsets_s.len() > 10_000
        || input.clock_offsets_s.iter().any(|v| !v.is_finite())
    {
        return Err("Supply 1..10000 finite explicit recording-start trials".into());
    }
    let event = &input.event;
    if !(3..=100_000).contains(&event.points.len())
        || !(1..=10_000).contains(&event.f0_hz.len())
        || event.f0_hz.iter().any(|v| !v.is_finite() || *v <= 0.)
        || event.points.iter().any(|p| {
            !p.t_rec_s.is_finite()
                || p.t_rec_s < 0.
                || !p.frequency_hz.is_finite()
                || p.frequency_hz <= 0.
        })
        || event
            .points
            .windows(2)
            .any(|p| p[0].t_rec_s >= p[1].t_rec_s)
    {
        return Err("Use ordered nonnegative fixed ridge times, positive frequencies and a bounded emitted-tone grid".into());
    }
    let mut ids = HashSet::new();
    for track in &input.tracks {
        physics::validate_track(track, input.model)?;
        if !ids.insert(&track.flight_id) {
            return Err("Single-event track identities must be unique".into());
        }
    }
    ids.clear();
    for observer in &input.observers {
        physics::geodetic_to_ecef(
            observer.latitude_deg,
            observer.longitude_deg,
            observer.ellipsoid_height_m,
        )?;
        if observer.id.is_empty() || !ids.insert(&observer.id) {
            return Err("Observer identities must be nonempty and unique".into());
        }
    }
    if let Some(b) = &input.coarse_bounds {
        for range in [b.speed_mps, b.slant_distance_m] {
            if !range.iter().all(|v| v.is_finite() && *v >= 0.) || range[0] > range[1] {
                return Err("Invalid single-event coarse bounds".into());
            }
        }
    }
    Ok(())
}

pub fn rank(input: &SingleInput) -> Result<Value, String> {
    validate(input)?;
    let event = &input.event;
    let first_time = event.points[0].t_rec_s;
    let last_time = event.points.last().unwrap().t_rec_s;
    let mut ranked: Vec<Candidate> = Vec::new();
    let mut unsupported_trials = 0_u64;
    let mut supported_trials = 0_u64;
    let mut model_rejections = 0_u64;
    let mut coarse_rejections = 0_u64;
    let mut factor_evaluations = 0_u64;
    let mut factor_cache_hits = 0_u64;
    let mut diagnostics = Vec::new();
    for observer in &input.observers {
        let xyz = physics::geodetic_to_ecef(
            observer.latitude_deg,
            observer.longitude_deg,
            observer.ellipsoid_height_m,
        )?;
        for track in &input.tracks {
            // Endpoint arrival bounds are exact for the validated subsonic linear
            // model. Avoid preparing an entire flight that covers no clock trial.
            let arrival = |p: &physics::TrackPoint| {
                p.t_s
                    + (p.xyz_m[0] - xyz[0])
                        .hypot(p.xyz_m[1] - xyz[1])
                        .hypot(p.xyz_m[2] - xyz[2])
                        / input.model.sound_speed_mps
            };
            let first_arrival = arrival(&track.points[0]);
            let last_arrival = arrival(track.points.last().unwrap());
            if !input.clock_offsets_s.iter().any(|clock| {
                first_time + clock >= first_arrival && last_time + clock <= last_arrival
            }) {
                unsupported_trials += input.clock_offsets_s.len() as u64;
                continue;
            }
            let model = match PreparedTrack::new(track, xyz, input.model) {
                Ok(model) => model,
                Err(error) => {
                    model_rejections += 1;
                    if diagnostics.len() < 10 {
                        diagnostics.push(error);
                    }
                    continue;
                }
            };
            if let Some(bounds) = &input.coarse_bounds {
                let cpa = model.closest_approach();
                if cpa.at_track_boundary
                    || cpa.speed_mps < bounds.speed_mps[0]
                    || cpa.speed_mps > bounds.speed_mps[1]
                    || cpa.slant_distance_m < bounds.slant_distance_m[0]
                    || cpa.slant_distance_m > bounds.slant_distance_m[1]
                {
                    coarse_rejections += 1;
                    continue;
                }
            }
            let mut best: Option<Score> = None;
            // Nearby clock trials often request the exact same reception times.
            // Cache only identical f64 inputs within this observer/track/model;
            // there is no rounding, interpolation or change to the solver.
            let mut factor_cache: HashMap<u64, f64> = HashMap::new();
            for &clock in &input.clock_offsets_s {
                if first_time + clock < first_arrival || last_time + clock > last_arrival {
                    unsupported_trials += 1;
                    continue;
                }
                let factors = event
                    .points
                    .iter()
                    .map(|p| -> Result<f64, String> {
                        let arrival = p.t_rec_s + clock;
                        let key = arrival.to_bits();
                        if let Some(&factor) = factor_cache.get(&key) {
                            factor_cache_hits += 1;
                            return Ok(factor);
                        }
                        factor_evaluations += 1;
                        let factor = model.factor_at_arrival(arrival)?;
                        // Bound memory for pathological nonoverlapping trial
                        // grids. Beyond the cap, exact uncached scoring resumes.
                        if factor_cache.len() < 1_000_000 {
                            factor_cache.insert(key, factor);
                        }
                        Ok(factor)
                    })
                    .collect::<Result<Vec<_>, _>>();
                let factors = match factors {
                    Ok(factors) => factors,
                    Err(error) => {
                        unsupported_trials += 1;
                        if diagnostics.len() < 10 {
                            diagnostics.push(error);
                        }
                        continue;
                    }
                };
                // Least-squares loss is convex in f0. The closest declared grid
                // frequency to its analytic minimum is exactly the grid winner.
                let denominator: f64 = factors.iter().map(|x| x * x).sum();
                let numerator: f64 = factors
                    .iter()
                    .zip(&event.points)
                    .map(|(x, p)| x * p.frequency_hz)
                    .sum();
                let optimum = numerator / denominator;
                if !optimum.is_finite() || denominator <= 0. {
                    return Err("Single-event frequency fit overflowed".into());
                }
                let f0 = *event
                    .f0_hz
                    .iter()
                    .min_by(|a, b| (**a - optimum).abs().total_cmp(&(**b - optimum).abs()))
                    .unwrap();
                let rmse = (factors
                    .iter()
                    .zip(&event.points)
                    .map(|(x, p)| (f0 * x - p.frequency_hz).powi(2))
                    .sum::<f64>()
                    / factors.len() as f64)
                    .sqrt();
                if !rmse.is_finite() {
                    return Err("Single-event residual overflowed".into());
                }
                supported_trials += 1;
                if best.as_ref().is_none_or(|previous| rmse < previous.rmse_hz) {
                    best = Some(Score {
                        flight_id: track.flight_id.clone(),
                        rmse_hz: rmse,
                        fitted_f0_hz: f0,
                        clock_offset_s: clock,
                        support_fraction: 1.,
                    });
                }
            }
            if let Some(score) = best {
                if ranked.len() >= 1_000_000 {
                    return Err("Single-event search exceeded one million retained hypotheses; split the declared job".into());
                }
                ranked.push(Candidate {
                    observer: observer.clone(),
                    score,
                });
            }
        }
    }
    ranked.sort_by(|a, b| {
        a.score
            .rmse_hz
            .total_cmp(&b.score.rmse_hz)
            .then_with(|| a.score.flight_id.cmp(&b.score.flight_id))
            .then_with(|| a.observer.id.cmp(&b.observer.id))
    });
    let mut ids = HashSet::new();
    let best_per_flight: Vec<_> = ranked
        .iter()
        .filter(|r| ids.insert(&r.score.flight_id))
        .collect();
    Ok(json!({
        "schema_version":1,"status":if ranked.is_empty(){"no_candidate"}else{"hypotheses_ranked"},"location_verified":false,
        "evidence_kind":input.evidence_kind,"provenance":input.provenance,
        "method":"known-clock single-event fixed-ridge RMSE; lower is better; every observed sample required",
        "observer_count":input.observers.len(),"track_count":input.tracks.len(),"ranked_count":ranked.len(),
        "supported_clock_trials":supported_trials,"unsupported_clock_trials":unsupported_trials,
        "model_rejections":model_rejections,"coarse_rejections":coarse_rejections,"diagnostic_examples":diagnostics,
        "factor_evaluations":factor_evaluations,"factor_cache_hits":factor_cache_hits,
        "clock_offsets_s":input.clock_offsets_s,"best_per_flight":best_per_flight,"candidates":ranked,
        "limitations":["A supplied or previously fitted recording clock is not an independently measured clock.","One flyover can retain spatial or flight ambiguity; rankings are not accepted locations.","Source-informed CPA bounds, when supplied, select a global closest approach and require one physical pass per input track.","Frequency and clock nuisance trials are profiled per location/track; near-optimal clock uncertainty requires separate sensitivity evaluation."]
    }))
}
