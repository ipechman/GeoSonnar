//! Headless replication stages. A score is a hypothesis, never a verification verdict.
use crate::{
    media,
    physics::{self, ClosestApproach, EventTrace, PairConfig, PairScore, Track},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventWindow {
    pub event_id: String,
    pub start_s: f64,
    pub end_s: f64,
    pub heard_cpa_s: f64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseManifest {
    pub schema_version: u32,
    pub case_id: String,
    pub input: PathBuf,
    pub recording_date: Option<String>,
    pub date_status: String,
    pub original_time_offset_s: f64,
    pub events: Vec<EventWindow>,
    pub search_input: Option<PathBuf>,
    pub notes: Vec<String>,
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(
        &fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?,
    )
    .map_err(|e| format!("Invalid JSON in {}: {e}", path.display()))
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    fs::write(path, bytes).map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

pub fn preflight(case_path: &Path) -> Result<Value, String> {
    let case: CaseManifest = read_json(case_path)?;
    if case.schema_version != 1 || case.case_id.trim().is_empty() {
        return Err("Case requires schema version 1 and a nonempty identity".into());
    }
    let base = case_path.parent().unwrap_or(Path::new("."));
    let info = media::inspect(&base.join(&case.input))?;
    let mut blockers = Vec::new();
    match &case.recording_date {
        None => blockers.push("Recording date/year has not been supplied".to_string()),
        Some(date) if !valid_recording_date(date) => blockers
            .push("Recording date must be a valid YYYY-MM-DD calendar date (1900-2200)".into()),
        _ => {}
    }
    if case.date_status.trim().is_empty() {
        blockers.push("Describe the source or uncertainty of the recording date".into());
    }
    if !case.original_time_offset_s.is_finite() || case.original_time_offset_s < 0.0 {
        return Err("Original recording offset must be finite and nonnegative".into());
    }
    if case.events.len() != 2 {
        blockers.push("Exactly two measured events are required for the pair experiment".into());
    }
    let mut event_ids = HashSet::new();
    for e in &case.events {
        if e.event_id.trim().is_empty() || !event_ids.insert(&e.event_id) {
            blockers.push("Event identities must be nonempty and unique".into());
        }
        if ![e.start_s, e.end_s, e.heard_cpa_s]
            .iter()
            .all(|v| v.is_finite())
            || e.start_s < 0.
            || e.end_s > info.duration_s
            || e.start_s >= e.end_s
            || e.heard_cpa_s < e.start_s
            || e.heard_cpa_s > e.end_s
        {
            blockers.push(format!(
                "{} has invalid recording-relative times",
                e.event_id
            ));
        }
    }
    if case.events.windows(2).any(|e| e[0].end_s >= e[1].start_s) {
        blockers.push("Case event windows must be ordered and nonoverlapping".into());
    }
    let mut search_contract = None;
    match &case.search_input {
        Some(p) if base.join(p).is_file() => {
            match read_json::<SearchInput>(&base.join(p)).and_then(|search| {
                validate_search_input(&search)?;
                if search.evidence_kind != "real_recording" {
                    return Err("A recording case requires real_recording search evidence, not a synthetic demo".into());
                }
                if case.events.len() == 2 {
                    for (index, (event, window)) in search.events.iter().zip(&case.events).enumerate() {
                        if event.points[0].t_rec_s < window.start_s || event.points.last().unwrap().t_rec_s > window.end_s {
                            return Err(format!("Search event {} ridge samples lie outside the case recording window", index + 1));
                        }
                    }
                    if (search.pair_config.event1_cpa_recording_s - case.events[0].heard_cpa_s).abs() > 1e-6 {
                        return Err("Search event-1 clock anchor differs from the case heard CPA".into());
                    }
                    let separation = case.events[1].heard_cpa_s - case.events[0].heard_cpa_s;
                    if (search.measured_separation_s - separation).abs() > 1e-6 {
                        return Err("Search measured separation differs from the case heard CPAs".into());
                    }
                }
                Ok(json!({"path":base.join(p),"schema_version":search.schema_version,"evidence_kind":search.evidence_kind,"track_count":search.tracks.len(),"observer_count":search.observers.len(),"status":"validated"}))
            }) {
                Ok(contract) => search_contract = Some(contract),
                Err(error) => blockers.push(format!("Search input is not ready: {error}")),
            }
        },
        _ => blockers.push("Historical tracks, observer heights and a candidate search configuration are not yet connected".into()),
    }
    Ok(
        json!({"case_id":case.case_id,"status":if blockers.is_empty(){"ready_for_search"}else{"blocked"},"location_verified":false,"media":info,"recording_date":case.recording_date,"date_status":case.date_status,"original_time_offset_s":case.original_time_offset_s,"events":case.events,"search_contract":search_contract,"blockers":blockers,"notes":case.notes}),
    )
}

fn valid_recording_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return false;
    }
    let year = value[..4].parse::<u32>().unwrap();
    let month = value[5..7].parse::<usize>().unwrap();
    let day = value[8..].parse::<u32>().unwrap();
    if !(1900..=2200).contains(&year) || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let lengths = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    day >= 1 && day <= lengths[month - 1]
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observer {
    pub id: String,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub ellipsoid_height_m: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoarseBounds {
    pub speed_mps: [f64; 2],
    pub slant_distance_m: [f64; 2],
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchInput {
    pub schema_version: u32,
    pub evidence_kind: String,
    pub provenance: Vec<String>,
    pub tracks: Vec<Track>,
    pub observers: Vec<Observer>,
    pub events: [EventTrace; 2],
    pub pair_config: PairConfig,
    pub coarse_bounds: [CoarseBounds; 2],
    pub measured_separation_s: f64,
    pub separation_tolerance_s: f64,
}

#[derive(Serialize)]
pub struct RankedCandidate {
    pub observer: Observer,
    pub score: PairScore,
}

fn in_bounds(cpa: &ClosestApproach, bounds: &CoarseBounds) -> bool {
    !cpa.at_track_boundary
        && cpa.speed_mps >= bounds.speed_mps[0]
        && cpa.speed_mps <= bounds.speed_mps[1]
        && cpa.slant_distance_m >= bounds.slant_distance_m[0]
        && cpa.slant_distance_m <= bounds.slant_distance_m[1]
}

/// Validate the declared experiment without evaluating or accepting candidates.
/// Preflight uses this same contract, so an existing but malformed search file
/// cannot make a media case ready. A single continuous track may hold two passes.
pub fn validate_search_input(input: &SearchInput) -> Result<(), String> {
    if input.schema_version != 1 {
        return Err("Unsupported search schema version".into());
    }
    if !["synthetic", "real_recording"].contains(&input.evidence_kind.as_str()) {
        return Err("evidence_kind must be synthetic or real_recording".into());
    }
    if input.provenance.is_empty() || input.provenance.iter().any(|p| p.trim().is_empty()) {
        return Err("Search provenance is required".into());
    }
    if !(1..=100_000).contains(&input.observers.len())
        || !(1..=100_000).contains(&input.tracks.len())
    {
        return Err("Supply 1..100000 observers and continuous tracks; two ordered passes must be available".into());
    }
    let settings = &input.pair_config;
    if !settings.event1_cpa_recording_s.is_finite()
        || !(1..=10_000).contains(&settings.shared_shifts_s.len())
        || settings.shared_shifts_s.iter().any(|v| !v.is_finite())
        || !settings.model.sound_speed_mps.is_finite()
        || settings.model.sound_speed_mps <= 0.
        || !settings.model.max_gap_s.is_finite()
        || settings.model.max_gap_s <= 0.
    {
        return Err("Invalid shared-clock sweep or propagation settings".into());
    }
    if settings
        .clock_offset_bounds_s
        .is_some_and(|[a, b]| !a.is_finite() || !b.is_finite() || a > b)
    {
        return Err("Recording-start bounds must be finite and ordered".into());
    }
    for event in &input.events {
        if !(3..=100_000).contains(&event.points.len())
            || !(1..=10_000).contains(&event.f0_hz.len())
            || event.f0_hz.iter().any(|f| !f.is_finite() || *f <= 0.)
            || event.points.iter().any(|p| {
                !p.t_rec_s.is_finite()
                    || p.t_rec_s < 0.0
                    || !p.frequency_hz.is_finite()
                    || p.frequency_hz <= 0.
            })
            || event
                .points
                .windows(2)
                .any(|p| p[0].t_rec_s >= p[1].t_rec_s)
        {
            return Err(
                "Each event requires ordered finite ridge samples and positive emitted frequencies"
                    .into(),
            );
        }
    }
    if input.events[0].points.last().unwrap().t_rec_s >= input.events[1].points[0].t_rec_s {
        return Err("Event windows must be ordered and nonoverlapping".into());
    }
    if !input.measured_separation_s.is_finite()
        || input.measured_separation_s <= 0.
        || !input.separation_tolerance_s.is_finite()
        || input.separation_tolerance_s < 0.
    {
        return Err("Separation must be finite and positive, with a nonnegative tolerance".into());
    }
    for b in &input.coarse_bounds {
        for range in [b.speed_mps, b.slant_distance_m] {
            if !range.iter().all(|v| v.is_finite() && *v >= 0.) || range[0] > range[1] {
                return Err("Invalid coarse speed/distance bounds".into());
            }
        }
    }
    // Reject a bad source track rather than making malformed input look like a
    // scientific filter rejection. Observer-specific failures remain counted.
    let mut identities = HashSet::new();
    for t in &input.tracks {
        physics::validate_track(t, input.pair_config.model)?;
        if !identities.insert(&t.flight_id) {
            return Err(format!(
                "Duplicate flight ID {}; every continuous input track needs a unique identity",
                t.flight_id
            ));
        }
    }
    identities.clear();
    for observer in &input.observers {
        physics::geodetic_to_ecef(
            observer.latitude_deg,
            observer.longitude_deg,
            observer.ellipsoid_height_m,
        )?;
        if observer.id.trim().is_empty() || !identities.insert(&observer.id) {
            return Err("Observer identities must be nonempty and unique".into());
        }
    }
    Ok(())
}

pub fn rank(input: &SearchInput) -> Result<Value, String> {
    validate_search_input(input)?;
    let mut ranked = Vec::new();
    let mut model_rejections = 0_u64;
    let mut timing_candidates = 0_u64;
    let mut scoring_rejections = 0_u64;
    let mut local_cpa_count = 0_u64;
    let mut boundary_cpa_rejections = 0_u64;
    let mut errors = Vec::new();
    for observer in &input.observers {
        let xyz = physics::geodetic_to_ecef(
            observer.latitude_deg,
            observer.longitude_deg,
            observer.ellipsoid_height_m,
        )?;
        let mut event1 = Vec::new();
        let mut event2 = Vec::new();
        for (i, track) in input.tracks.iter().enumerate() {
            match physics::local_closest_approaches(track, xyz, input.pair_config.model) {
                Ok(cpas) => {
                    for (cpa_index, cpa) in cpas.into_iter().enumerate() {
                        local_cpa_count += 1;
                        if cpa.at_track_boundary {
                            boundary_cpa_rejections += 1;
                        }
                        if in_bounds(&cpa, &input.coarse_bounds[0]) {
                            event1.push((i, cpa_index, cpa));
                        }
                        if in_bounds(&cpa, &input.coarse_bounds[1]) {
                            event2.push((i, cpa_index, cpa));
                        }
                    }
                }
                Err(e) => {
                    model_rejections += 1;
                    if errors.len() < 10 {
                        errors.push(format!("{} / {}: {e}", observer.id, track.flight_id));
                    }
                }
            }
        }
        event2.sort_by(|a, b| a.2.arrival_time_s.total_cmp(&b.2.arrival_time_s));
        for (i, cpa_index1, cpa1) in event1 {
            let target = cpa1.arrival_time_s + input.measured_separation_s;
            let first = event2.partition_point(|(_, _, c)| {
                c.arrival_time_s < target - input.separation_tolerance_s
            });
            for (j, cpa_index2, cpa2) in &event2[first..] {
                if cpa2.arrival_time_s > target + input.separation_tolerance_s {
                    break;
                }
                if cpa2.arrival_time_s <= cpa1.arrival_time_s
                    || (i == *j && cpa_index1 == *cpa_index2)
                {
                    continue;
                }
                timing_candidates += 1;
                match physics::score_pair_at_cpa(
                    [&input.tracks[i], &input.tracks[*j]],
                    xyz,
                    [&input.events[0], &input.events[1]],
                    &input.pair_config,
                    [cpa_index1, *cpa_index2],
                ) {
                    Ok(score) => ranked.push(RankedCandidate {
                        observer: observer.clone(),
                        score,
                    }),
                    Err(e) => {
                        scoring_rejections += 1;
                        if errors.len() < 10 {
                            errors.push(e);
                        }
                    }
                }
            }
        }
    }
    ranked.sort_by(|a, b| {
        a.score
            .combined_rmse_hz
            .total_cmp(&b.score.combined_rmse_hz)
    });
    let mut best_pairs = Vec::new();
    let mut pairs = std::collections::HashSet::new();
    for r in &ranked {
        if pairs.insert(r.score.flight_ids.clone()) {
            best_pairs.push(r);
        }
    }
    Ok(
        json!({"status":if ranked.is_empty(){"no_candidate"}else{"hypotheses_ranked"},"location_verified":false,"evidence_kind":input.evidence_kind,"method":"two-event traced-ridge RMSE, shared clock, all local CPA pairs, lower is better; not the creator's unpublished brightness normalization","provenance":input.provenance,"observer_count":input.observers.len(),"track_count":input.tracks.len(),"local_cpa_count":local_cpa_count,"boundary_cpa_rejections":boundary_cpa_rejections,"timing_candidates":timing_candidates,"model_rejections":model_rejections,"scoring_rejections":scoring_rejections,"diagnostic_examples":errors,"ranked_count":ranked.len(),"best_per_flight_pair":best_pairs,"candidates":ranked,"remaining_validation":["Inspect both overlays and competing flight pairs and passes","Repeat with plausible altitude, sound speed, time and grid changes","Compare with withheld ground truth or independent imagery","A constrained search around a known result is not a blind national replication"]}),
    )
}

pub fn demo_input() -> Result<SearchInput, String> {
    use physics::{ModelConfig, TracePoint, TrackPoint};
    let origin = physics::geodetic_to_ecef(0., 0., 0.)?;
    let config = ModelConfig::default();
    let make_track = |event: usize| Track {
        flight_id: format!("synthetic-flight-{event}"),
        points: (-24..=24)
            .map(|i| {
                let u = i as f64 * 5.;
                if event == 1 {
                    TrackPoint {
                        t_s: 2000. + u,
                        xyz_m: [origin[0] + 3000., 140. * u, 500.],
                    }
                } else {
                    TrackPoint {
                        t_s: 2920. + u,
                        xyz_m: [origin[0] + 2000., -400., 100. * u],
                    }
                }
            })
            .collect(),
    };
    let tracks = vec![make_track(1), make_track(2)];
    let cpas = [
        physics::closest_approach(&tracks[0], origin, config)?,
        physics::closest_approach(&tracks[1], origin, config)?,
    ];
    let clock = 1500.;
    let mut event_vec = Vec::new();
    for (k, f0) in [112., 128.].iter().enumerate() {
        let mut points = Vec::new();
        for i in -15..=15 {
            let arrival = cpas[k].arrival_time_s + i as f64 * 3.;
            points.push(TracePoint {
                t_rec_s: arrival - clock,
                frequency_hz: physics::frequency_at_arrival(
                    &tracks[k], origin, arrival, *f0, config,
                )?,
            });
        }
        event_vec.push(EventTrace {
            points,
            f0_hz: vec![f0 - 1., *f0, f0 + 1.],
        });
    }
    let mut observers = Vec::new();
    for i in -2..=2 {
        for j in -2..=2 {
            observers.push(Observer {
                id: format!("synthetic-{i}-{j}"),
                latitude_deg: i as f64 * 0.005,
                longitude_deg: j as f64 * 0.005,
                ellipsoid_height_m: 0.,
            });
        }
    }
    Ok(SearchInput {
        schema_version: 1,
        evidence_kind: "synthetic".into(),
        provenance: vec![
            "Synthetic regression fixture; no downloaded-video location evidence".into(),
        ],
        tracks,
        observers,
        events: event_vec.try_into().map_err(|_| "event count")?,
        pair_config: PairConfig {
            event1_cpa_recording_s: cpas[0].arrival_time_s - clock,
            shared_shifts_s: vec![-1., 0., 1.],
            clock_offset_bounds_s: None,
            model: config,
        },
        coarse_bounds: [
            CoarseBounds {
                speed_mps: [100., 160.],
                slant_distance_m: [1000., 8000.],
            },
            CoarseBounds {
                speed_mps: [80., 120.],
                slant_distance_m: [1000., 8000.],
            },
        ],
        measured_separation_s: cpas[1].arrival_time_s - cpas[0].arrival_time_s,
        separation_tolerance_s: 10.,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_tracks_are_input_errors_not_no_candidate_results() {
        let mut input = demo_input().unwrap();
        input.tracks[0].points[1].t_s = input.tracks[0].points[0].t_s;
        assert!(rank(&input).unwrap_err().contains("timestamps"));
    }
    #[test]
    fn synthetic_search_recovers_hidden_observer_but_is_not_real_verification() {
        let out = rank(&demo_input().unwrap()).unwrap();
        assert_eq!(out["status"], "hypotheses_ranked");
        assert_eq!(out["location_verified"], false);
        assert_eq!(out["evidence_kind"], "synthetic");
        assert_eq!(out["candidates"][0]["observer"]["latitude_deg"], 0.);
        assert_eq!(out["candidates"][0]["observer"]["longitude_deg"], 0.);
        assert!(
            out["candidates"][0]["score"]["combined_rmse_hz"]
                .as_f64()
                .unwrap()
                < 1e-8
        );
    }
}
