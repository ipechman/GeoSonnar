//! Synthetic arithmetic/storage controls, not independent location accuracy.
use geosonnar::{
    adsblol::{TraceImportConfig, TraceImportStats},
    physics::{Track, TrackPoint},
    receiver_grid::GridLayout,
    track_store::TrackStore,
    verification,
    wide_area_pair::{self, WideBudgets, WidePairRequest, WideStatus},
    workspace::MapSelection,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
struct Fixture {
    root: PathBuf,
    request: WidePairRequest,
    reference: verification::SearchInput,
}
impl Fixture {
    fn new(block_points: usize, same_aircraft: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "geosonnar-wide-pair-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let store_dir = root.join("store");
        fs::create_dir(&store_dir).unwrap();
        let mut reference = verification::demo_input().unwrap();
        reference.pair_config.clock_offset_bounds_s = Some([1000., 2000.]);
        if same_aircraft {
            let mut points = reference.tracks[0].points.clone();
            let a = points.last().unwrap().clone();
            let b = reference.tracks[1].points.first().unwrap().clone();
            let steps = ((b.t_s - a.t_s) / 20.).ceil() as usize;
            for i in 1..steps {
                let fraction = i as f64 / steps as f64;
                points.push(TrackPoint {
                    t_s: a.t_s + (b.t_s - a.t_s) * fraction,
                    xyz_m: std::array::from_fn(|axis| {
                        a.xyz_m[axis] + (b.xyz_m[axis] - a.xyz_m[axis]) * fraction
                    }),
                });
            }
            points.extend(reference.tracks[1].points.clone());
            reference.tracks = vec![Track {
                flight_id: "synthetic-loop".into(),
                points,
            }];
        }
        let grid = GridLayout::new(&MapSelection {
            bounds: [-0.005, -0.005, 0.005, 0.005],
            grid_spacing_m: 2000.,
            receiver_height_m: 0.,
            clock_adjustment_s: 0.,
        })
        .unwrap();
        reference.observers = (0..grid.count)
            .map(|i| {
                let o = grid.observer(i).unwrap();
                verification::Observer {
                    id: format!("receiver-{i}"),
                    latitude_deg: o.latitude_deg,
                    longitude_deg: o.longitude_deg,
                    ellipsoid_height_m: o.ellipsoid_height_m,
                }
            })
            .collect();
        let config = TraceImportConfig {
            source_label: "fabricated wide-area arithmetic fixture".into(),
            ..Default::default()
        };
        let mut passes = Vec::new();
        let mut stored_bytes = 0;
        let mut total_points = 0;
        for (pass_index, track) in reference.tracks.iter().enumerate() {
            let mut blocks = Vec::new();
            let mut first = 0;
            for (block_index, points) in track.points.chunks(block_points).enumerate() {
                let name = format!("pass-{pass_index}-{block_index}.bin");
                let mut bytes = Vec::new();
                for point in points {
                    for value in [point.t_s, point.xyz_m[0], point.xyz_m[1], point.xyz_m[2]] {
                        bytes.extend(value.to_le_bytes());
                    }
                }
                fs::write(store_dir.join(&name), &bytes).unwrap();
                let provenance_name = format!("pass-{pass_index}-{block_index}-source.json.gz");
                let mut encoded =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                std::io::Write::write_all(&mut encoded, b"{\"synthetic_arithmetic_fixture\":true}")
                    .unwrap();
                let provenance = encoded.finish().unwrap();
                fs::write(store_dir.join(&provenance_name), &provenance).unwrap();
                blocks.push(json!({"path":name,"bytes":bytes.len(),"sha256":sha(&bytes),"first_sample":first,"point_count":points.len(),
                    "provenance":{"path":provenance_name,"bytes":provenance.len(),"sha256":sha(&provenance)}}));
                first += points.len();
                stored_bytes += bytes.len() + provenance.len();
            }
            let min: Vec<_> = (0..3)
                .map(|a| {
                    track
                        .points
                        .iter()
                        .map(|p| p.xyz_m[a])
                        .fold(f64::INFINITY, f64::min)
                })
                .collect();
            let max: Vec<_> = (0..3)
                .map(|a| {
                    track
                        .points
                        .iter()
                        .map(|p| p.xyz_m[a])
                        .fold(f64::NEG_INFINITY, f64::max)
                })
                .collect();
            passes.push(json!({"id":track.flight_id,"point_count":track.points.len(),"start_s":track.points[0].t_s,"end_s":track.points.last().unwrap().t_s,"ecef_min":min,"ecef_max":max,"blocks":blocks}));
            total_points += track.points.len();
        }
        let stats = TraceImportStats {
            output_passes: reference.tracks.len(),
            output_points: total_points,
            ..Default::default()
        };
        let path = store_dir.join("track-store.json");
        fs::write(&path,serde_json::to_vec(&json!({"schema_version":1,"status":"complete","config":config,"source":{"synthetic":true},"stats":stats,"passes":passes,"stored_bytes":stored_bytes,"location_verified":false,
            "point_encoding":"le-f64-t_s-x_m-y_m-z_m;32bytes;no-resampling;gzip-json-source-provenance"})).unwrap()).unwrap();
        let source = TrackStore::open(&path).unwrap().binding().clone();
        let request = WidePairRequest {
            schema_version: 1,
            document_sha256: "a".repeat(64),
            recording_sha256: "b".repeat(64),
            channel: 0,
            grid,
            source,
            events: reference.events.clone(),
            pair_config: reference.pair_config.clone(),
            coarse_bounds: reference.coarse_bounds.clone(),
            measured_separation_s: reference.measured_separation_s,
            separation_tolerance_s: reference.separation_tolerance_s,
            budgets: WideBudgets::default(),
            retain: 1000,
        };
        Self {
            root,
            request,
            reference,
        }
    }
    fn output(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for _ in 0..10 {
            match fs::remove_dir_all(&self.root) {
                Ok(()) => return,
                Err(error) if matches!(error.raw_os_error(), Some(32 | 33)) => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                Err(error) => panic!("fixture cleanup: {error}"),
            }
        }
        panic!("fixture cleanup remained locked");
    }
}

#[test]
fn wide_grid_counts_national_domain_without_allocating_or_silently_coarsening() {
    let selection = MapSelection {
        bounds: [24., -125., 50., -66.],
        grid_spacing_m: 10_000.,
        ..Default::default()
    };
    let grid = GridLayout::new(&selection).unwrap();
    assert!(grid.count > 10_000);
    assert!(geosonnar::workspace_scoring::grid_observers(&selection).is_err());
    let first = grid.observer(0).unwrap();
    let last = grid.observer(grid.count - 1).unwrap();
    assert_eq!(
        [
            first.latitude_deg,
            first.longitude_deg,
            last.latitude_deg,
            last.longitude_deg
        ],
        selection.bounds
    );
    assert_eq!(grid.spacing_m, 10_000.);
    grid.validate().unwrap();
    let mut fixture = Fixture::new(9, false);
    fixture.request.grid = grid.clone();
    let estimate = wide_area_pair::estimate(&fixture.request).unwrap();
    assert_eq!(estimate.receiver_count, grid.count);
}
#[test]
fn indexed_grid_batches_preserve_exact_legacy_coordinates_and_boundaries() {
    let selection = MapSelection {
        bounds: [44., -74., 44.05, -73.95],
        grid_spacing_m: 2000.,
        ..Default::default()
    };
    let grid = GridLayout::new(&selection).unwrap();
    let expected = geosonnar::workspace_scoring::grid_observers(&selection).unwrap();
    let a = grid.batch(0, 3).unwrap();
    let b = grid.batch(3, expected.len() - 3).unwrap();
    assert_eq!(
        serde_json::to_value([a, b].concat()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert!(grid.observer(grid.count).is_err());
    assert!(grid.batch(grid.count - 1, 2).is_err());
    let mut tampered = grid;
    tampered.columns += 1;
    assert!(tampered.validate().is_err());
}

#[test]
fn index_memory_budget_preserves_old_request_hashes_and_reports_the_complete_source() {
    let mut fixture = Fixture::new(7, false);
    let old_bytes = serde_json::to_vec(&fixture.request).unwrap();
    assert!(
        !std::str::from_utf8(&old_bytes)
            .unwrap()
            .contains("max_index_memory_bytes")
    );
    let restored: WidePairRequest = serde_json::from_slice(&old_bytes).unwrap();
    assert_eq!(restored.budgets.max_index_memory_bytes, 512 * 1024 * 1024);
    assert_eq!(
        wide_area_pair::request_sha256(&restored).unwrap(),
        sha(&old_bytes)
    );
    let estimate = wide_area_pair::estimate(&restored).unwrap();
    let store = TrackStore::open(&restored.source.manifest_path).unwrap();
    assert_eq!(
        estimate.source_passes,
        fixture.reference.tracks.len() as u64
    );
    assert_eq!(
        estimate.source_metadata_bytes,
        store.resident_memory_bytes()
    );
    assert_eq!(
        estimate.required_index_memory_bytes,
        estimate.source_metadata_bytes
            + estimate.spatial_index_memory_bytes
            + estimate.local_cpa_memory_reserve_bytes
    );
    assert!(estimate.required_index_memory_bytes <= restored.budgets.max_index_memory_bytes);
    assert!(estimate.spatial_index_memory_bytes > 0 && estimate.local_cpa_memory_reserve_bytes > 0);
    fixture.request.budgets.max_index_memory_bytes = 64 * 1024 * 1024;
    let explicit = serde_json::to_value(&fixture.request).unwrap();
    assert_eq!(
        explicit["budgets"]["max_index_memory_bytes"],
        64 * 1024 * 1024u64
    );
    assert_ne!(
        wide_area_pair::request_sha256(&fixture.request).unwrap(),
        sha(&old_bytes)
    );
    assert_eq!(
        wide_area_pair::estimate(&fixture.request)
            .unwrap()
            .source_passes,
        estimate.source_passes
    );
    for invalid in [64 * 1024 * 1024 - 1, 4 * 1024 * 1024 * 1024 + 1] {
        fixture.request.budgets.max_index_memory_bytes = invalid;
        assert!(
            wide_area_pair::estimate(&fixture.request)
                .unwrap_err()
                .contains("index-memory limits")
        );
    }
}

#[test]
fn cancellation_inside_spatial_index_build_commits_no_receiver_or_final_source_verification() {
    let fixture = Fixture::new(7, false);
    let cancel = AtomicBool::new(false);
    let mut index_checkpoints = 0;
    let report = wide_area_pair::run(
        &fixture.request,
        &fixture.output("cancel-index"),
        &cancel,
        |progress| {
            if progress.stage == "preparing_index" {
                index_checkpoints += 1;
                if index_checkpoints == 5 {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        },
    )
    .unwrap();
    assert_eq!(index_checkpoints, 5);
    assert_eq!(report.status, WideStatus::Cancelled);
    assert_eq!(report.progress.completed_receivers, 0);
    assert_eq!(report.progress.timing_candidates, 0);
    assert!(report.source_time_origin_utc_s.is_some());
    assert!(!report.source_verified_at_end);
    assert!(report.best_hypotheses.is_empty() && report.best_flight_pairs.is_empty());
    let reopened = wide_area_pair::load_report(&fixture.output("cancel-index")).unwrap();
    assert_eq!(reopened.status, WideStatus::Cancelled);
    assert!(!reopened.source_verified_at_end);
}
fn compare_legacy(fixture: &Fixture) {
    let expected = verification::rank(&fixture.reference).unwrap();
    let report = wide_area_pair::run(
        &fixture.request,
        &fixture.output("run"),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.status, WideStatus::Completed);
    assert_eq!(
        report.progress.completed_receivers,
        fixture.request.grid.count
    );
    let rows = expected["candidates"].as_array().unwrap();
    assert!(!rows.is_empty());
    assert_eq!(report.best_hypotheses.len(), rows.len());
    for (actual, expected) in report.best_hypotheses.iter().zip(rows) {
        assert_eq!(
            serde_json::to_value(&actual.score).unwrap(),
            expected["score"]
        );
        assert_eq!(
            actual.observer.latitude_deg,
            expected["observer"]["latitude_deg"].as_f64().unwrap()
        );
        assert_eq!(
            actual.observer.longitude_deg,
            expected["observer"]["longitude_deg"].as_f64().unwrap()
        );
    }
    assert_eq!(
        report.progress.timing_candidates,
        expected["timing_candidates"].as_u64().unwrap()
    );
    let loaded = wide_area_pair::load_report(&fixture.output("run")).unwrap();
    assert_eq!(
        loaded.progress.completed_receivers,
        report.progress.completed_receivers
    );
}
#[test]
fn all_receiver_pair_scores_match_legacy_across_source_storage_blocks() {
    compare_legacy(&Fixture::new(7, false));
}
#[test]
fn same_aircraft_distinct_passes_and_block_seams_match_legacy() {
    compare_legacy(&Fixture::new(5, true));
}
#[test]
fn more_than_ten_thousand_receivers_finish_with_every_location_accounted_for() {
    let mut fixture = Fixture::new(9, false);
    // Deliberately distant: a cheap complete-domain/no-support control. It is
    // not a national aircraft workload benchmark or a field no-match result.
    fixture.request.grid = GridLayout::new(&MapSelection {
        bounds: [30., 30., 40., 40.],
        grid_spacing_m: 10_000.,
        ..Default::default()
    })
    .unwrap();
    assert!(fixture.request.grid.count > 10_000);
    let report = wide_area_pair::run(
        &fixture.request,
        &fixture.output("large-grid"),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.status, WideStatus::Completed);
    assert_eq!(
        report.progress.completed_receivers,
        fixture.request.grid.count
    );
    assert_eq!(
        report.pages.iter().map(|p| p.receiver_count).sum::<u64>(),
        fixture.request.grid.count
    );
    assert_eq!(
        report.progress.bbox_excluded_passes,
        fixture.request.grid.count * 2
    );
    assert!(report.best_hypotheses.is_empty());
    assert_eq!(
        wide_area_pair::load_report(&fixture.output("large-grid"))
            .unwrap()
            .progress
            .completed_receivers,
        fixture.request.grid.count
    );
}
#[test]
fn global_top_one_matches_full_legacy_ranking_without_restricting_pair_evaluation() {
    let mut fixture = Fixture::new(11, false);
    fixture.request.retain = 1;
    let expected = verification::rank(&fixture.reference).unwrap();
    let report = wide_area_pair::run(
        &fixture.request,
        &fixture.output("top-one"),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.status, WideStatus::Completed);
    assert_eq!(report.best_hypotheses.len(), 1);
    assert_eq!(
        serde_json::to_value(&report.best_hypotheses[0].score).unwrap(),
        expected["candidates"][0]["score"]
    );
    assert_eq!(
        report.progress.scored_candidates,
        expected["ranked_count"].as_u64().unwrap()
    );
    assert!(report.retained_truncated);
}
#[test]
fn cancellation_keeps_only_complete_receivers_and_historical_open_needs_no_originals() {
    let fixture = Fixture::new(7, false);
    let cancel = AtomicBool::new(false);
    let report = wide_area_pair::run(&fixture.request, &fixture.output("partial"), &cancel, |p| {
        if p.completed_receivers == 1 {
            cancel.store(true, Ordering::Relaxed)
        }
    })
    .unwrap();
    assert_eq!(report.status, WideStatus::Cancelled);
    assert_eq!(report.progress.completed_receivers, 1);
    assert!(report.best_hypotheses.iter().all(|r| r.receiver_index == 0));
    fs::rename(fixture.root.join("store"), fixture.root.join("store-moved")).unwrap();
    assert_eq!(
        wide_area_pair::load_report(&fixture.output("partial"))
            .unwrap()
            .status,
        WideStatus::Cancelled
    );
}
#[test]
fn pre_cancel_and_global_work_or_output_exhaustion_never_claim_complete() {
    let mut fixture = Fixture::new(9, false);
    let report = wide_area_pair::run(
        &fixture.request,
        &fixture.output("cancel"),
        &AtomicBool::new(true),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.status, WideStatus::Cancelled);
    assert_eq!(report.progress.completed_receivers, 0);
    fixture.request.budgets.max_work_units = fixture.request.grid.count;
    let report = wide_area_pair::run(
        &fixture.request,
        &fixture.output("work"),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.status, WideStatus::BudgetExceeded);
    assert_eq!(report.progress.completed_receivers, 0);
    fixture.request.budgets = WideBudgets {
        max_output_bytes: 16 * 1024 * 1024 + 400,
        ..Default::default()
    };
    let report = wide_area_pair::run(
        &fixture.request,
        &fixture.output("disk"),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.status, WideStatus::BudgetExceeded);
    assert_eq!(report.progress.completed_receivers, 0);
}
#[test]
fn existing_output_is_refused_and_saved_receiver_tampering_is_detected() {
    let fixture = Fixture::new(7, false);
    let out = fixture.output("run");
    let report =
        wide_area_pair::run(&fixture.request, &out, &AtomicBool::new(false), |_| {}).unwrap();
    let original = fs::read(out.join("report.json")).unwrap();
    assert!(wide_area_pair::run(&fixture.request, &out, &AtomicBool::new(false), |_| {}).is_err());
    assert_eq!(fs::read(out.join("report.json")).unwrap(), original);
    fs::write(out.join(&report.pages[0].name), b"altered").unwrap();
    assert!(wide_area_pair::load_report(&out).is_err());
}

#[test]
fn cancellation_during_initial_source_verification_saves_unknown_origin_without_scoring() {
    let fixture = Fixture::new(7, false);
    let cancel = AtomicBool::new(false);
    let mut source_checkpoints = 0;
    let report = wide_area_pair::run(
        &fixture.request,
        &fixture.output("cancel-in-open"),
        &cancel,
        |progress| {
            if progress.stage == "verifying_source" {
                source_checkpoints += 1;
                if source_checkpoints == 20 {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        },
    )
    .unwrap();
    assert_eq!(source_checkpoints, 20);
    assert_eq!(report.status, WideStatus::Cancelled);
    assert_eq!(report.progress.completed_receivers, 0);
    assert_eq!(report.progress.scored_candidates, 0);
    assert!(report.source_time_origin_utc_s.is_none());
    assert!(report.best_hypotheses.is_empty());
    assert!(report.best_flight_pairs.is_empty());
    let loaded = wide_area_pair::load_report(&fixture.output("cancel-in-open")).unwrap();
    assert_eq!(loaded.status, WideStatus::Cancelled);
    assert!(loaded.source_time_origin_utc_s.is_none());
}

#[test]
fn final_source_verification_cancellation_retains_rows_without_claiming_completion() {
    let fixture = Fixture::new(7, false);
    let cancel = AtomicBool::new(false);
    let report = wide_area_pair::run(
        &fixture.request,
        &fixture.output("cancel-final-check"),
        &cancel,
        |progress| {
            if progress.stage == "verifying_source_final" {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    )
    .unwrap();
    assert_eq!(report.status, WideStatus::Cancelled);
    assert_eq!(
        report.progress.completed_receivers,
        fixture.request.grid.count
    );
    assert!(!report.source_verified_at_end);
    assert!(report.source_time_origin_utc_s.is_some());
    assert!(!report.best_hypotheses.is_empty());
    assert_eq!(
        wide_area_pair::load_report(&fixture.output("cancel-final-check"))
            .unwrap()
            .status,
        WideStatus::Cancelled
    );
}

#[test]
fn source_verification_obeys_wall_deadline_before_any_receiver_is_committed() {
    let mut fixture = Fixture::new(7, false);
    fixture.request.budgets.max_duration_s = 1;
    let mut calls = 0;
    let report = wide_area_pair::run(
        &fixture.request,
        &fixture.output("deadline-in-open"),
        &AtomicBool::new(false),
        |progress| {
            if progress.stage == "verifying_source" {
                calls += 1;
                if calls == 2 {
                    std::thread::sleep(std::time::Duration::from_millis(1100));
                }
            }
        },
    )
    .unwrap();
    assert_eq!(report.status, WideStatus::BudgetExceeded);
    assert_eq!(report.progress.completed_receivers, 0);
    assert!(report.source_time_origin_utc_s.is_none());
    assert!(report.best_hypotheses.is_empty());
    assert!(report.error.as_deref().unwrap().contains("wall"));
}

#[test]
fn estimate_cancellation_and_affordable_pair_ceiling_preserve_declared_science() {
    let fixture = Fixture::new(7, false);
    let before = serde_json::to_value(&fixture.request).unwrap();
    let cancel = AtomicBool::new(false);
    let mut calls = 0;
    let error = wide_area_pair::estimate_with_cancel(&fixture.request, &cancel, |progress| {
        if progress.stage == "verifying_source" {
            calls += 1;
            if calls == 10 {
                cancel.store(true, Ordering::Relaxed);
            }
        }
    })
    .unwrap_err();
    assert_eq!(calls, 10);
    assert!(error.contains("Cancelled"));
    let estimate = wide_area_pair::estimate(&fixture.request).unwrap();
    // Demo has two49-point passes,62 fixed ridge samples,3tones,3sharedshifts.
    let minimum = 16 * 98 + 62 * (3 + 128) * 3 + 4 * fixture.request.retain as u64;
    assert_eq!(estimate.minimum_work_units_per_timing_pair, minimum);
    assert_eq!(
        estimate.maximum_timing_pairs_before_scan,
        fixture.request.budgets.max_work_units / minimum
    );
    assert_eq!(serde_json::to_value(&fixture.request).unwrap(), before);
}

#[test]
fn cancellation_inside_pair_scoring_is_not_recorded_as_a_rejected_match() {
    let fixture = Fixture::new(7, false);
    let cancel = AtomicBool::new(false);
    let mut pair_checkpoints = 0;
    let report = wide_area_pair::run(
        &fixture.request,
        &fixture.output("cancel-in-pair"),
        &cancel,
        |progress| {
            if progress.stage == "shared_clock_pair_scoring" {
                pair_checkpoints += 1;
                if pair_checkpoints == 5 {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        },
    )
    .unwrap();
    assert_eq!(pair_checkpoints, 5);
    assert_eq!(report.status, WideStatus::Cancelled);
    assert_eq!(report.progress.completed_receivers, 0);
    assert_eq!(report.scoring_rejections, 0);
    assert!(report.progress.timing_candidates > 0);
    assert!(report.best_hypotheses.is_empty());
    assert_eq!(
        wide_area_pair::load_report(&fixture.output("cancel-in-pair"))
            .unwrap()
            .status,
        WideStatus::Cancelled
    );
}

#[test]
fn cli_wide_search_accepts_relative_output_and_preserves_existing_run() {
    let fixture = Fixture::new(7, false);
    fs::write(
        fixture.root.join("request.json"),
        serde_json::to_vec(&fixture.request).unwrap(),
    )
    .unwrap();
    let execute = || {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_geosonnar"));
        command.current_dir(&fixture.root).args([
            "wide-pair-search",
            "--request",
            "request.json",
            "--out",
            "cli-result",
        ]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command.output().unwrap()
    };
    let result = execute();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let before = wide_area_pair::load_report(&fixture.output("cli-result")).unwrap();
    assert_eq!(before.status, WideStatus::Completed);
    assert!(!before.best_hypotheses.is_empty());
    assert_eq!(
        before.progress.completed_receivers,
        fixture.request.grid.count
    );
    let rejected = execute();
    assert!(!rejected.status.success());
    let after = wide_area_pair::load_report(&fixture.output("cli-result")).unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
}
