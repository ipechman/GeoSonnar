//! Synthetic exhaustive scalar-oracle controls; no location-accuracy claim.
use geosonnar::{
    adsblol::{TraceImportConfig, TraceImportStats},
    physics::{self, Track},
    receiver_grid::GridLayout,
    track_store::TrackStore,
    verification,
    wide_area_pair::{self as pruned, WideBudgets, WideCandidate, WidePairRequest},
    workspace::MapSelection,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

struct Fixture {
    root: PathBuf,
    request: WidePairRequest,
}

impl Fixture {
    fn new(tied: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "geosonnar-pruning-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).unwrap();
        let store = root.join("store");
        fs::create_dir(&store).unwrap();
        let mut reference = verification::demo_input().unwrap();
        reference.pair_config.clock_offset_bounds_s = Some([1000., 2000.]);
        let mut tracks = reference.tracks.clone();
        tracks[0].flight_id = "00-true-first".into();
        tracks[1].flight_id = "01-true-second".into();
        // Preserve all original times and samples. Only the explicitly synthetic
        // alternative geometry changes. Distinct pass IDs remain mandatory.
        for alternative in 1..=3 {
            let mut first = reference.tracks[0].clone();
            first.flight_id = format!("10-alternative-first-{alternative}");
            let mut second = reference.tracks[1].clone();
            second.flight_id = format!("20-alternative-second-{alternative}");
            if !tied {
                for point in &mut first.points {
                    point.xyz_m[0] += 1200. + 300. * alternative as f64;
                }
                for point in &mut second.points {
                    point.xyz_m[0] += alternative as f64;
                }
            }
            tracks.push(first);
            tracks.push(second);
        }
        tracks.sort_by(|a, b| a.flight_id.cmp(&b.flight_id));
        let source = write_store(&store, &tracks);
        let grid = GridLayout::new(&MapSelection {
            bounds: [0., 0., 0.002, 0.002],
            grid_spacing_m: 300.,
            receiver_height_m: 0.,
            clock_adjustment_s: 0.,
        })
        .unwrap();
        assert_eq!(grid.count, 4);
        let request = WidePairRequest {
            schema_version: 1,
            document_sha256: "a".repeat(64),
            recording_sha256: "b".repeat(64),
            channel: 0,
            grid,
            source,
            events: reference.events,
            pair_config: reference.pair_config,
            coarse_bounds: reference.coarse_bounds,
            measured_separation_s: reference.measured_separation_s,
            separation_tolerance_s: reference.separation_tolerance_s,
            budgets: WideBudgets {
                // Isolate numerical pruning from a work-budget stop. Runtime,
                // memory and disk limits remain enforced for these tiny runs.
                work_limit_enabled: false,
                max_duration_s: 60,
                max_output_bytes: 32 * 1024 * 1024,
                ..WideBudgets::default()
            },
            retain: 1,
        };
        Self { root, request }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for _ in 0..10 {
            match fs::remove_dir_all(&self.root) {
                Ok(()) => return,
                Err(error) if matches!(error.raw_os_error(), Some(32 | 33)) => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(error) => panic!("owned pruning fixture cleanup: {error}"),
            }
        }
        panic!("owned pruning fixture cleanup remained locked");
    }
}

fn write_store(directory: &Path, tracks: &[Track]) -> geosonnar::track_store::TrackStoreBinding {
    let mut passes = Vec::new();
    let mut stored_bytes = 0;
    let mut total_points = 0;
    for (pass_index, track) in tracks.iter().enumerate() {
        let mut blocks = Vec::new();
        let mut first_sample = 0;
        for (block_index, points) in track.points.chunks(9).enumerate() {
            let path = format!("pass-{pass_index}-{block_index}.bin");
            let mut bytes = Vec::new();
            for point in points {
                for value in [point.t_s, point.xyz_m[0], point.xyz_m[1], point.xyz_m[2]] {
                    bytes.extend(value.to_le_bytes());
                }
            }
            fs::write(directory.join(&path), &bytes).unwrap();
            let provenance_path = format!("pass-{pass_index}-{block_index}.json.gz");
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder
                .write_all(b"{\"synthetic_arithmetic_fixture\":true}")
                .unwrap();
            let provenance = encoder.finish().unwrap();
            fs::write(directory.join(&provenance_path), &provenance).unwrap();
            blocks.push(json!({
                "path":path,"bytes":bytes.len(),"sha256":sha(&bytes),
                "first_sample":first_sample,"point_count":points.len(),
                "provenance":{"path":provenance_path,"bytes":provenance.len(),
                    "sha256":sha(&provenance)}
            }));
            stored_bytes += bytes.len() + provenance.len();
            first_sample += points.len();
        }
        let minimum: Vec<_> = (0..3)
            .map(|axis| {
                track
                    .points
                    .iter()
                    .map(|p| p.xyz_m[axis])
                    .fold(f64::INFINITY, f64::min)
            })
            .collect();
        let maximum: Vec<_> = (0..3)
            .map(|axis| {
                track
                    .points
                    .iter()
                    .map(|p| p.xyz_m[axis])
                    .fold(f64::NEG_INFINITY, f64::max)
            })
            .collect();
        passes.push(json!({
            "id":track.flight_id,"point_count":track.points.len(),
            "start_s":track.points[0].t_s,"end_s":track.points.last().unwrap().t_s,
            "ecef_min":minimum,"ecef_max":maximum,"blocks":blocks
        }));
        total_points += track.points.len();
    }
    let config = TraceImportConfig {
        source_label: "fabricated exact-pruning regression fixture".into(),
        ..Default::default()
    };
    let stats = TraceImportStats {
        output_passes: tracks.len(),
        output_points: total_points,
        ..Default::default()
    };
    let path = directory.join("track-store.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema_version":1,"status":"complete","config":config,
            "source":{"synthetic":true},"stats":stats,"passes":passes,
            "stored_bytes":stored_bytes,"location_verified":false,
            "point_encoding":"le-f64-t_s-x_m-y_m-z_m;32bytes;no-resampling;gzip-json-source-provenance"
        })).unwrap(),
    )
    .unwrap();
    TrackStore::open(&path).unwrap().binding().clone()
}

fn exact_json(actual: &Value, expected: &Value, label: &str) {
    // Round-trip float parsing is configured by the harness. Serialized bytes
    // additionally preserve signed zero; approximate score equality is forbidden.
    assert_eq!(
        serde_json::to_vec(actual).unwrap(),
        serde_json::to_vec(expected).unwrap(),
        "{label}"
    );
}

fn counter(value: &Value, name: &str) -> u64 {
    value[name].as_u64().unwrap_or_else(|| {
        assert_eq!(
            name, "score_bound_pruned_candidates",
            "missing counter {name}"
        );
        assert!(value.get(name).is_none(), "invalid pruning counter");
        0
    })
}

fn rows(directory: &Path, report: &Value) -> Vec<Value> {
    let mut result = Vec::new();
    for page in report["pages"].as_array().unwrap() {
        let bytes = fs::read(directory.join(page["name"].as_str().unwrap())).unwrap();
        assert_eq!(bytes.len() as u64, page["bytes"].as_u64().unwrap());
        assert_eq!(sha(&bytes), page["sha256"].as_str().unwrap());
        let page_rows: Vec<Value> = bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(
            page_rows.len() as u64,
            page["receiver_count"].as_u64().unwrap()
        );
        assert_eq!(
            page["first_receiver"].as_u64().unwrap(),
            result.len() as u64
        );
        result.extend(page_rows);
    }
    result
}

// This deliberately uses the public scalar physics scorer and the original
// exhaustive ordered join, never the cache, range helper or pruning certificate.
// The tiny fixture permits retaining every score before independently reducing.
fn exhaustive_scalar(request: &WidePairRequest) -> (Value, Vec<Value>) {
    let store = TrackStore::open(&request.source.manifest_path).unwrap();
    let tracks: Vec<_> = (0..store.passes().len())
        .map(|index| store.load_track(index).unwrap())
        .collect();
    let mut ranked = Vec::new();
    let mut rows = Vec::new();
    let mut timing = 0_u64;
    let mut scored = 0_u64;
    let mut rejected = 0_u64;
    for receiver_index in 0..request.grid.count {
        let observer = request.grid.observer(receiver_index).unwrap();
        let xyz = physics::geodetic_to_ecef(
            observer.latitude_deg,
            observer.longitude_deg,
            observer.ellipsoid_height_m,
        )
        .unwrap();
        let mut events = [Vec::new(), Vec::new()];
        for (pass_index, track) in tracks.iter().enumerate() {
            for (cpa_index, cpa) in
                physics::local_closest_approaches(track, xyz, request.pair_config.model)
                    .unwrap()
                    .into_iter()
                    .enumerate()
            {
                for (event, bounds) in events.iter_mut().zip(&request.coarse_bounds) {
                    if !cpa.at_track_boundary
                        && cpa.speed_mps >= bounds.speed_mps[0]
                        && cpa.speed_mps <= bounds.speed_mps[1]
                        && cpa.slant_distance_m >= bounds.slant_distance_m[0]
                        && cpa.slant_distance_m <= bounds.slant_distance_m[1]
                    {
                        event.push((pass_index, cpa_index, cpa));
                    }
                }
            }
        }
        events[1].sort_by(|a, b| a.2.arrival_time_s.total_cmp(&b.2.arrival_time_s));
        let mut local = Vec::new();
        let mut attempts = 0_u64;
        let mut errors = 0_u64;
        for &(first, first_cpa, cpa1) in &events[0] {
            let target = cpa1.arrival_time_s + request.measured_separation_s;
            let begin = events[1].partition_point(|entry| {
                entry.2.arrival_time_s < target - request.separation_tolerance_s
            });
            for &(second, second_cpa, cpa2) in &events[1][begin..] {
                if cpa2.arrival_time_s > target + request.separation_tolerance_s {
                    break;
                }
                if cpa2.arrival_time_s <= cpa1.arrival_time_s
                    || (first == second && first_cpa == second_cpa)
                {
                    continue;
                }
                let encounter_index = timing;
                timing += 1;
                attempts += 1;
                match physics::score_pair_at_cpa(
                    [&tracks[first], &tracks[second]],
                    xyz,
                    [&request.events[0], &request.events[1]],
                    &request.pair_config,
                    [first_cpa, second_cpa],
                ) {
                    Ok(score) => {
                        scored += 1;
                        local.push(WideCandidate {
                            receiver_index,
                            observer: observer.clone(),
                            score,
                            encounter_index,
                        });
                    }
                    Err(_) => {
                        rejected += 1;
                        errors += 1;
                    }
                }
            }
        }
        local.sort_by(candidate_order);
        rows.push(json!({"receiver_index":receiver_index,"observer":observer,
            "status":if !local.is_empty() {"scored"} else if errors>0 {
                "unavailable_pair_support"} else {"no_supported_pair"},
            "eligible_event_cpas":[events[0].len(),events[1].len()],
            "timing_candidates":attempts,"scored_candidates":local.len(),
            "model_rejections":0,"scoring_rejections":errors,"best":local.first()}));
        ranked.extend(local);
    }
    ranked.sort_by(candidate_order);
    let mut seen = std::collections::HashSet::new();
    let mut pairs: Vec<_> = ranked
        .iter()
        .filter(|row| seen.insert(row.score.flight_ids.clone()))
        .cloned()
        .collect();
    pairs.truncate(request.retain);
    ranked.truncate(request.retain);
    (
        json!({"status":"completed","error":null,"source_verified_at_end":true,
        "location_verified":false,"source":request.source,
        "source_time_origin_utc_s":store.time_origin_utc_s(),"retain":request.retain,
        "request_sha256":pruned::request_sha256(request).unwrap(),
        "scoring_rejections":rejected,"progress":{"completed_receivers":request.grid.count,
            "timing_candidates":timing,"scored_candidates":scored},
        "best_hypotheses":ranked,"best_flight_pairs":pairs}),
        rows,
    )
}

fn candidate_order(a: &WideCandidate, b: &WideCandidate) -> std::cmp::Ordering {
    a.score
        .combined_rmse_hz
        .total_cmp(&b.score.combined_rmse_hz)
        .then(a.encounter_index.cmp(&b.encounter_index))
}

fn compare(fixture: &Fixture, retain: usize) -> (Value, Value, Vec<Value>) {
    let mut request = fixture.request.clone();
    request.retain = retain;
    let request_bytes = serde_json::to_vec(&request).unwrap();
    let candidate_request: pruned::WidePairRequest =
        serde_json::from_slice(&request_bytes).unwrap();
    assert_eq!(
        serde_json::to_vec(&candidate_request).unwrap(),
        request_bytes
    );
    let candidate_path = fixture.root.join(format!("pruned-{retain}"));
    let cancel = AtomicBool::new(false);
    let (baseline, baseline_rows) = exhaustive_scalar(&request);
    let candidate = serde_json::to_value(
        pruned::run(&candidate_request, &candidate_path, &cancel, |_| {}).unwrap(),
    )
    .unwrap();
    for report in [&baseline, &candidate] {
        assert_eq!(report["status"], "completed", "{report}");
        assert_eq!(
            report["progress"]["completed_receivers"].as_u64(),
            Some(request.grid.count)
        );
        assert_eq!(report["source_verified_at_end"], true);
        assert_eq!(report["location_verified"], false);
        assert!(report["error"].is_null());
    }
    for key in [
        "request_sha256",
        "source",
        "source_time_origin_utc_s",
        "retain",
        "best_hypotheses",
        "best_flight_pairs",
    ] {
        exact_json(&candidate[key], &baseline[key], key);
    }
    assert!(
        !candidate["scoring_implementation"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    assert!(baseline.get("scoring_implementation").is_none());
    assert_eq!(
        candidate["progress"]["timing_candidates"],
        baseline["progress"]["timing_candidates"]
    );
    let pruned_count = counter(&candidate["progress"], "score_bound_pruned_candidates");
    assert_eq!(
        counter(&candidate["progress"], "scored_candidates")
            + counter(&candidate, "scoring_rejections")
            + pruned_count,
        counter(&candidate["progress"], "timing_candidates"),
    );
    let candidate_rows = rows(&candidate_path, &candidate);
    assert_eq!(candidate_rows.len() as u64, request.grid.count);
    assert_eq!(baseline_rows.len(), candidate_rows.len());
    let mut row_pruned = 0;
    for (index, (actual, expected)) in candidate_rows.iter().zip(&baseline_rows).enumerate() {
        assert_eq!(actual["receiver_index"].as_u64(), Some(index as u64));
        for key in [
            "receiver_index",
            "observer",
            "status",
            "eligible_event_cpas",
            "timing_candidates",
            "best",
        ] {
            exact_json(&actual[key], &expected[key], key);
        }
        let skipped = counter(actual, "score_bound_pruned_candidates");
        assert_eq!(
            counter(actual, "scored_candidates") + counter(actual, "scoring_rejections") + skipped,
            counter(actual, "timing_candidates")
        );
        row_pruned += skipped;
    }
    assert_eq!(row_pruned, pruned_count);
    exact_json(
        &serde_json::to_value(pruned::load_report(&candidate_path).unwrap()).unwrap(),
        &candidate,
        "reopened candidate report",
    );
    assert_eq!(
        fs::read(candidate_path.join("request.json")).unwrap(),
        request_bytes
    );
    (baseline, candidate, candidate_rows)
}

#[test]
fn strict_score_bound_prunes_real_alternatives_without_changing_top_one_or_two_or_receiver_best() {
    let fixture = Fixture::new(false);
    for retain in [1, 2] {
        let (baseline, candidate, _) = compare(&fixture, retain);
        assert!(
            counter(&candidate["progress"], "score_bound_pruned_candidates") > 0,
            "fixture must exercise pruning at retain={retain}"
        );
        assert!(
            counter(&candidate["progress"], "scored_candidates")
                < counter(&baseline["progress"], "scored_candidates")
        );
        assert_eq!(
            candidate["best_hypotheses"].as_array().unwrap().len(),
            retain
        );
        assert_eq!(
            candidate["best_flight_pairs"].as_array().unwrap().len(),
            retain
        );
    }
}

#[test]
fn underfilled_retention_keeps_every_scored_hypothesis_without_pruning() {
    let fixture = Fixture::new(false);
    let (baseline, candidate, _) = compare(&fixture, 512);
    assert_eq!(
        counter(&candidate["progress"], "score_bound_pruned_candidates"),
        0
    );
    let count = counter(&baseline["progress"], "scored_candidates");
    assert!(count > 2 && count < 512);
    assert_eq!(
        candidate["best_hypotheses"].as_array().unwrap().len() as u64,
        count
    );
    assert_eq!(
        candidate["progress"]["scored_candidates"],
        baseline["progress"]["scored_candidates"]
    );
    assert_eq!(candidate["retained_truncated"], false);
    assert_eq!(candidate["pair_retention_truncated"], false);
}

#[test]
fn equal_score_alternatives_preserve_first_encounter_ties_without_strict_pruning() {
    let fixture = Fixture::new(true);
    let (all, _, _) = compare(&fixture, 512);
    let first_receiver: Vec<_> = all["best_hypotheses"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["receiver_index"] == 0)
        .collect();
    assert_eq!(
        first_receiver.len(),
        16,
        "four first passes by four second passes"
    );
    let tied_score = &first_receiver[0]["score"]["combined_rmse_hz"];
    for row in &first_receiver {
        exact_json(
            &row["score"]["combined_rmse_hz"],
            tied_score,
            "exact tied score",
        );
    }
    assert!(
        first_receiver
            .windows(2)
            .all(|pair| pair[0]["encounter_index"].as_u64() < pair[1]["encounter_index"].as_u64())
    );
    for retain in [1, 2] {
        let (_, candidate, _) = compare(&fixture, retain);
        assert_eq!(
            counter(&candidate["progress"], "score_bound_pruned_candidates"),
            0
        );
        for key in ["best_hypotheses", "best_flight_pairs"] {
            let expected = Value::Array(all[key].as_array().unwrap()[..retain].to_vec());
            exact_json(&candidate[key], &expected, "stable exhaustive tie prefix");
        }
    }
}

#[test]
fn reopened_pruning_journal_rejects_rehashed_row_and_global_counter_changes() {
    let fixture = Fixture::new(false);
    let output = fixture.root.join("counter-integrity");
    let report = pruned::run(&fixture.request, &output, &AtomicBool::new(false), |_| {}).unwrap();
    assert!(report.progress.score_bound_pruned_candidates > 0);
    let original = serde_json::to_value(report).unwrap();
    let page_name = original["pages"][0]["name"].as_str().unwrap();
    let original_page = fs::read(output.join(page_name)).unwrap();
    let mut altered_rows: Vec<Value> = original_page
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    altered_rows[0]["score_bound_pruned_candidates"] =
        json!(counter(&altered_rows[0], "score_bound_pruned_candidates") + 1);
    let mut altered_page = Vec::new();
    for row in altered_rows {
        altered_page.extend(serde_json::to_vec(&row).unwrap());
        altered_page.push(b'\n');
    }
    fs::write(output.join(page_name), &altered_page).unwrap();
    let mut altered = original.clone();
    altered["pages"][0]["bytes"] = json!(altered_page.len());
    altered["pages"][0]["sha256"] = json!(sha(&altered_page));
    rewrite_terminal_report(&output, &altered);
    assert!(
        pruned::load_report(&output)
            .unwrap_err()
            .contains("receiver scoring/pruning")
    );

    fs::write(output.join(page_name), &original_page).unwrap();
    for name in ["scored_candidates", "score_bound_pruned_candidates"] {
        let mut altered = original.clone();
        altered["progress"][name] = json!(counter(&original["progress"], name) + 1);
        rewrite_terminal_report(&output, &altered);
        assert!(
            pruned::load_report(&output)
                .unwrap_err()
                .contains("global scoring/pruning")
        );
    }
    rewrite_terminal_report(&output, &original);
    exact_json(
        &serde_json::to_value(pruned::load_report(&output).unwrap()).unwrap(),
        &original,
        "restored untouched result",
    );
}

fn rewrite_terminal_report(output: &Path, report: &Value) {
    let bytes = serde_json::to_vec(report).unwrap();
    fs::write(output.join("report.json"), &bytes).unwrap();
    let mut terminal: Value =
        serde_json::from_slice(&fs::read(output.join("terminal.json")).unwrap()).unwrap();
    terminal["report_bytes"] = json!(bytes.len());
    terminal["report_sha256"] = json!(sha(&bytes));
    fs::write(
        output.join("terminal.json"),
        serde_json::to_vec(&terminal).unwrap(),
    )
    .unwrap();
}
