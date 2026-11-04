//! Export checkable model overlays using the exact fitted shared clock.
//! The optional reference coordinate belongs in evaluation/plotting, never here.

use crate::physics::{self, PairScore};
use crate::verification::{Observer, SearchInput};
use serde_json::{Value, json};

const GRID_STEP_S: f64 = 0.25;
const MAX_EVENT_GRID_SAMPLES: usize = 100_000;

/// Export the best `top_n` distinct flight pairs (1..=10), including actual
/// observed trace points and a denser model-only grid. Interpolated values are
/// never presented as new measurements: model-only samples have a null observed
/// frequency. A mismatched search/result file is rejected by identity, position,
/// and a recomputed per-event RMSE check.
pub fn export(input: &SearchInput, report: &Value, top_n: usize) -> Result<Value, String> {
    if !(1..=10).contains(&top_n) {
        return Err("Overlay count must be between 1 and 10".into());
    }
    let ranked = report
        .get("best_per_flight_pair")
        .or_else(|| report.get("candidates"))
        .and_then(Value::as_array)
        .ok_or("Ranked report has no candidate array")?;
    let mut candidates = Vec::new();
    for (rank, candidate) in ranked.iter().take(top_n).enumerate() {
        let reported_observer: Observer = serde_json::from_value(candidate["observer"].clone())
            .map_err(|error| format!("Invalid candidate observer: {error}"))?;
        let mut matches = input
            .observers
            .iter()
            .filter(|o| o.id == reported_observer.id);
        let observer = matches.next().ok_or_else(|| {
            format!(
                "Observer {} is not in the search input",
                reported_observer.id
            )
        })?;
        if matches.next().is_some() {
            return Err(format!(
                "Observer ID {} is ambiguous in the search input",
                observer.id
            ));
        }
        if (observer.latitude_deg - reported_observer.latitude_deg).abs() > 1e-12
            || (observer.longitude_deg - reported_observer.longitude_deg).abs() > 1e-12
            || (observer.ellipsoid_height_m - reported_observer.ellipsoid_height_m).abs() > 1e-6
        {
            return Err(format!(
                "Observer {} coordinates differ between search and results",
                observer.id
            ));
        }
        let xyz = physics::geodetic_to_ecef(
            observer.latitude_deg,
            observer.longitude_deg,
            observer.ellipsoid_height_m,
        )?;
        let score: PairScore = serde_json::from_value(candidate["score"].clone())
            .map_err(|error| format!("Invalid candidate score: {error}"))?;
        if !score.clock_offset_s.is_finite()
            || score
                .observer_xyz_m
                .iter()
                .zip(xyz)
                .any(|(&a, b)| !a.is_finite() || (a - b).abs() > 1e-3)
        {
            return Err(
                "Candidate clock/Cartesian position is invalid or belongs to a different input"
                    .into(),
            );
        }
        let mut events = Vec::new();
        for (event_index, trace) in input.events.iter().enumerate() {
            let flight_id = &score.flight_ids[event_index];
            let mut tracks = input
                .tracks
                .iter()
                .filter(|track| &track.flight_id == flight_id);
            let track = tracks
                .next()
                .ok_or_else(|| format!("Flight {flight_id} is not in the search input"))?;
            if tracks.next().is_some() {
                return Err(format!(
                    "Flight ID {flight_id} is ambiguous in the search input"
                ));
            }
            if trace.points.is_empty()
                || trace
                    .points
                    .iter()
                    .any(|p| !p.t_rec_s.is_finite() || !p.frequency_hz.is_finite())
                || trace
                    .points
                    .windows(2)
                    .any(|pair| pair[0].t_rec_s >= pair[1].t_rec_s)
            {
                return Err(
                    "Overlay traces must contain finite, strictly time-ordered samples".into(),
                );
            }
            let start = trace.points.first().unwrap().t_rec_s;
            let end = trace.points.last().unwrap().t_rec_s;
            if !(end - start).is_finite()
                || end - start > GRID_STEP_S * (MAX_EVENT_GRID_SAMPLES - 1) as f64
            {
                return Err("Event window is too long for a detailed overlay; select a bounded event window".into());
            }
            let count = ((end - start) / GRID_STEP_S).floor() as usize + 1;
            if count > MAX_EVENT_GRID_SAMPLES {
                return Err("Event window is too long for a detailed overlay; select a bounded event window".into());
            }
            let mut times: Vec<f64> = (0..count).map(|i| start + i as f64 * GRID_STEP_S).collect();
            times.extend(trace.points.iter().map(|p| p.t_rec_s));
            times.sort_by(f64::total_cmp);
            times.dedup_by(|a, b| (*a - *b).abs() < 1e-10);
            let mut samples = Vec::with_capacity(times.len());
            let mut sum_squared_error = 0.0;
            let mut measured_count = 0;
            for time in times {
                let prediction = physics::frequency_at_arrival(
                    track,
                    xyz,
                    time + score.clock_offset_s,
                    score.fitted_f0_hz[event_index],
                    input.pair_config.model,
                )?;
                let next = trace.points.partition_point(|p| p.t_rec_s < time - 1e-10);
                let observed = trace
                    .points
                    .get(next)
                    .filter(|p| (p.t_rec_s - time).abs() < 1e-9)
                    .map(|p| p.frequency_hz);
                if let Some(measurement) = observed {
                    sum_squared_error += (prediction - measurement).powi(2);
                    measured_count += 1;
                }
                samples.push(json!({"t_rec_s":time,"observed_frequency_hz":observed,"predicted_frequency_hz":prediction}));
            }
            if measured_count != trace.points.len() {
                return Err(
                    "Overlay lost a measured trace sample while combining time grids".into(),
                );
            }
            let recomputed_rmse = (sum_squared_error / measured_count as f64).sqrt();
            if !score.rmse_hz[event_index].is_finite()
                || (recomputed_rmse - score.rmse_hz[event_index]).abs() > 1e-5
            {
                return Err(format!(
                    "Candidate event {} score does not match the supplied search input (saved {}, recalculated {})",
                    event_index + 1,
                    score.rmse_hz[event_index],
                    recomputed_rmse
                ));
            }
            events.push(json!({
                "event_index":event_index+1,"flight_id":flight_id,
                "fitted_f0_hz":score.fitted_f0_hz[event_index],
                "window_recording_s":[start,end],"observed_sample_count":measured_count,
                "recomputed_rmse_hz":recomputed_rmse,"samples":samples
            }));
        }
        candidates.push(json!({"distinct_pair_rank":rank+1,"observer":observer,"flight_ids":score.flight_ids,"scores":score,"events":events}));
    }
    Ok(json!({
        "schema_version":1,"status":if candidates.is_empty(){"no_candidates"}else{"overlays_exported"},
        "location_verified":false,"evidence_kind":input.evidence_kind,
        "time_reference":"input trace recording times; arrival time = recording time + fitted shared clock_offset_s",
        "model_grid_step_s":GRID_STEP_S,
        "observations":"Only input ridge samples have observed_frequency_hz; null marks a model-only grid sample. No independent event clock adjustments.",
        "candidate_selection":"best reported observer for each distinct ordered flight pair",
        "candidates":candidates
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verification;

    #[test]
    fn export_preserves_measurements_and_reproduces_scored_shared_clock() {
        let input = verification::demo_input().unwrap();
        let report = verification::rank(&input).unwrap();
        let overlay = export(&input, &report, 3).unwrap();
        assert_eq!(overlay["location_verified"], false);
        let event = &overlay["candidates"][0]["events"][0];
        assert_eq!(
            event["observed_sample_count"].as_u64().unwrap() as usize,
            input.events[0].points.len()
        );
        assert!(event["recomputed_rmse_hz"].as_f64().unwrap() < 1e-8);
        let samples = event["samples"].as_array().unwrap();
        assert!(samples.len() > input.events[0].points.len());
        assert!(
            samples
                .iter()
                .any(|point| point["observed_frequency_hz"].is_null())
        );
    }

    #[test]
    fn export_rejects_mismatched_identity_score_and_unbounded_count() {
        let input = verification::demo_input().unwrap();
        let report = verification::rank(&input).unwrap();
        assert!(export(&input, &report, 11).is_err());
        let mut changed = report.clone();
        changed["best_per_flight_pair"][0]["score"]["rmse_hz"][0] = json!(100.0);
        assert!(export(&input, &changed, 1).is_err());
        let mut changed = report;
        changed["best_per_flight_pair"][0]["observer"]["id"] = json!("missing-observer");
        assert!(export(&input, &changed, 1).is_err());
    }
}
