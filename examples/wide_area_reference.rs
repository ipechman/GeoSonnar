//! Explicit local real-input software parity check. No location-accuracy claim.
use geosonnar::{
    adsblol::TraceImportReport,
    detailed_data::sha256,
    receiver_grid::GridLayout,
    track_store,
    verification::{self, Observer, SearchInput},
    wide_area_pair::{self, WideBudgets, WidePairRequest, WideStatus},
    workspace::MapSelection,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::atomic::AtomicBool, time::Instant};

fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            (x - y).abs() <= 1e-9_f64.max(1e-12 * x.abs().max(y.abs()))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| equal(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| equal(v, w)))
        }
        _ => a == b,
    }
}
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        return Err(
            "Use wide_area_reference SEARCH.json NORMALIZED.json OLD_RESULTS.json NEW_OUTPUT_DIR"
                .into(),
        );
    }
    let paths: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    if paths[3].exists() {
        return Err("Use a fresh output directory".into());
    }
    let identities: Vec<_> = paths[..3]
        .iter()
        .map(|p| sha256(p))
        .collect::<Result<_, _>>()?;
    let mut search: SearchInput = verification::read_json(&paths[0])?;
    let normalized: TraceImportReport = verification::read_json(&paths[1])?;
    let previous: Value = verification::read_json(&paths[2])?;
    let center: &Value = &previous["candidates"][0]["observer"];
    let lat = center["latitude_deg"]
        .as_f64()
        .ok_or("No historical coarse leader")?;
    let lon = center["longitude_deg"]
        .as_f64()
        .ok_or("No historical coarse longitude")?;
    let height = center["ellipsoid_height_m"]
        .as_f64()
        .ok_or("No historical coarse height")?;
    let expected: BTreeMap<_, _> = search
        .tracks
        .iter()
        .map(|t| (&t.flight_id, &t.points))
        .collect();
    if normalized.passes.len() != expected.len()
        || normalized.passes.iter().any(|p| {
            expected.get(&p.track.flight_id).is_none_or(|points| {
                serde_json::to_value(points).unwrap()
                    != serde_json::to_value(&p.track.points).unwrap()
            })
        })
    {
        return Err("Normalized source and native search tracks differ".into());
    }
    drop(expected);
    fs::create_dir(&paths[3]).map_err(|e| e.to_string())?;
    let output = paths[3].canonicalize().map_err(|e| e.to_string())?;
    let indexed_started = Instant::now();
    let binding = track_store::write_normalized(
        normalized,
        &paths[3].join("indexed"),
        2 * 1024_u64.pow(3),
        json!({"type":"explicit_real_input_software_regression","normalized_sha256":identities[1]}),
    )?;
    let indexed_elapsed_s = indexed_started.elapsed().as_secs_f64();
    let grid = GridLayout::new(&MapSelection {
        bounds: [lat, lon, lat + 0.00001, lon + 0.00001],
        grid_spacing_m: 1000.,
        receiver_height_m: height,
        ..Default::default()
    })?;
    let observations = grid.batch(0, grid.count as usize)?;
    search.observers = observations
        .iter()
        .enumerate()
        .map(|(i, o)| Observer {
            id: format!("reference-{i}"),
            latitude_deg: o.latitude_deg,
            longitude_deg: o.longitude_deg,
            ellipsoid_height_m: o.ellipsoid_height_m,
        })
        .collect();
    let legacy_started = Instant::now();
    let legacy = verification::rank(&search)?;
    let legacy_elapsed_s = legacy_started.elapsed().as_secs_f64();
    let rows = legacy["candidates"]
        .as_array()
        .ok_or("Missing legacy rows")?;
    if rows.is_empty() || rows.len() > 1000 {
        return Err("Parity fixture needs positive untruncated legacy rows".into());
    }
    let request = WidePairRequest {
        schema_version: 1,
        document_sha256: "d".repeat(64),
        recording_sha256: "e".repeat(64),
        channel: 0,
        grid,
        source: binding,
        events: search.events.clone(),
        pair_config: search.pair_config.clone(),
        coarse_bounds: search.coarse_bounds.clone(),
        measured_separation_s: search.measured_separation_s,
        separation_tolerance_s: search.separation_tolerance_s,
        budgets: WideBudgets {
            max_work_units: 2_000_000_000,
            max_output_bytes: 128 * 1024 * 1024,
            max_duration_s: 300,
            ..Default::default()
        },
        retain: 1000,
    };
    let estimate = wide_area_pair::estimate(&request)?;
    let wide_started = Instant::now();
    let result = wide_area_pair::run(
        &request,
        &output.join("run"),
        &AtomicBool::new(false),
        |_| {},
    )?;
    let wide_elapsed_s = wide_started.elapsed().as_secs_f64();
    if result.status != WideStatus::Completed
        || result.retained_truncated
        || result.best_hypotheses.len() != rows.len()
    {
        return Err("Wide-area run is incomplete or differs in retained row count".into());
    }
    let mut remaining: Vec<_> = result.best_hypotheses.iter().collect();
    for row in rows {
        let index = remaining
            .iter()
            .position(|candidate| {
                equal(
                    &row["score"],
                    &serde_json::to_value(&candidate.score).unwrap(),
                ) && equal(
                    &row["observer"]["latitude_deg"],
                    &json!(candidate.observer.latitude_deg),
                ) && equal(
                    &row["observer"]["longitude_deg"],
                    &json!(candidate.observer.longitude_deg),
                ) && equal(
                    &row["observer"]["ellipsoid_height_m"],
                    &json!(candidate.observer.ellipsoid_height_m),
                )
            })
            .ok_or("Wide-area candidate/clock/CPA differs from legacy")?;
        remaining.swap_remove(index);
    }
    if legacy["timing_candidates"].as_u64() != Some(result.progress.timing_candidates)
        || legacy["scoring_rejections"].as_u64() != Some(result.scoring_rejections)
    {
        return Err("Timing/scoring accounting differs".into());
    }
    for (p, identity) in paths[..3].iter().zip(&identities) {
        if sha256(p)? != *identity {
            return Err("Reference input changed".into());
        }
    }
    let report = json!({"status":"software_parity_passed","source_tracks":search.tracks.len(),"receiver_count":4,"matched_rows":rows.len(),
        "indexed_elapsed_s":indexed_elapsed_s,"legacy_elapsed_s":legacy_elapsed_s,"wide_elapsed_s":wide_elapsed_s,
        "wide_progress":result.progress,"estimate":estimate,
        "event_point_counts":[search.events[0].points.len(),search.events[1].points.len()],
        "tone_grid_counts":[search.events[0].f0_hz.len(),search.events[1].f0_hz.len()],"shared_shift_count":search.pair_config.shared_shifts_s.len(),
        "all_scores_clocks_tones_cpas_matched":true,"timing_candidates_matched":true,"source_sha256":identities,
        "location_accuracy_validated":false,"selection":"Four fixed-height points adjacent to the previously exposed coarse leader; source-informed positive software regression only",
        "independence_limit":"Same native physics implementation; no independent geolocation or full-national performance claim","indexed_manifest":request.source});
    verification::write_json(&paths[3].join("parity.json"), &report)?;
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|e| e.to_string())?
    );
    Ok(())
}
