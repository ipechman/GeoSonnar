//! Independent wire-format, transfer-accounting and cache/normalization controls.
use flate2::{Compression, write::GzEncoder};
use geosonnar::{
    adsblol,
    adsblol_history::{self as history, *},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const DAY: f64 = 1_712_534_400.0; // April 8, 2024 00:00 UTC, independently fixed fixture epoch.
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "history-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .canonicalize()
            .unwrap();
        let path = self.0.canonicalize().unwrap();
        assert!(path.starts_with(&root) && path != root);
        fs::remove_dir_all(path).unwrap();
    }
}
fn request() -> HistoryRequest {
    HistoryRequest {
        utc_start: "2024-04-08T00:00:01Z".into(),
        utc_end: "2024-04-08T00:00:40Z".into(),
        bounds: [42.0, -73.0, 44.0, -71.0],
        max_traces: 10,
        max_download_bytes: 1_000_000,
    }
}
fn position(
    id: u32,
    source: u8,
    non_icao: bool,
    lat: f64,
    lon: f64,
    alt: i16,
    speed: i16,
) -> [u32; 4] {
    [
        id | (u32::from(source) << 27) | if non_icao { 1 << 24 } else { 0 },
        (lat * 1e6) as i32 as u32,
        (lon * 1e6) as i32 as u32,
        (alt as u16 as u32) | ((speed as u16 as u32) << 16),
    ]
}
fn marker(time: f64) -> [u32; 4] {
    let ms = (time * 1000.0) as u64;
    [0x0e7f7c9d, (ms >> 32) as u32, ms as u32, 10_000]
}
fn bytes(rows: &[[u32; 4]]) -> Vec<u8> {
    rows.iter()
        .flat_map(|r| r.iter().flat_map(|n| n.to_le_bytes()))
        .collect()
}
fn replay_with(positions: Vec<[u32; 4]>, legacy: bool) -> Vec<u8> {
    let mut rows = Vec::new();
    let mut indices = Vec::new();
    for slice in 0..180 {
        indices.push(rows.len());
        rows.push(marker(DAY + slice as f64 * 10.0));
        if slice == 1 {
            rows.extend(positions.clone());
        }
    }
    if legacy {
        let mut index: Vec<_> = indices
            .iter()
            .map(|&offset| [(offset + indices.len()) as u32, 0, 0, 0])
            .collect();
        index.extend(rows);
        bytes(&index)
    } else {
        bytes(&rows)
    }
}
fn replay() -> Vec<u8> {
    replay_with(
        vec![position(0xabc123, 0, false, 43.0, -72.0, 40, 1000)],
        false,
    )
}
fn trace(id: &str, source: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"icao":id,"timestamp":DAY,"trace":[
    [10.0,43.0,-72.0,1000.0,110.0,90.0,0,0.0,null,source,1100.0],
    [15.0,43.0,-71.999,1000.0,110.0,90.0,0,0.0,null,source,1100.0],
    [20.0,43.0,-71.998,1000.0,110.0,90.0,0,0.0,null,source,1100.0]]}))
    .unwrap()
}
fn gzip(data: &[u8]) -> Vec<u8> {
    let mut out = GzEncoder::new(Vec::new(), Compression::fast());
    out.write_all(data).unwrap();
    out.finish().unwrap()
}
fn reply(data: &[u8], limit: u64, path: &Path) -> Result<TransportResult, String> {
    let length = data.len().min(limit as usize);
    fs::write(path, &data[..length]).unwrap();
    Ok(TransportResult {
        http_status: Some(200),
        error: if length < data.len() {
            Some("Fixture payload exceeds remaining budget".into())
        } else {
            None
        },
    })
}
fn run(root: &Path, plan: &HistoryPlan) -> HistoryReport {
    history::fetch_with_transport(
        plan,
        root,
        &AtomicBool::new(false),
        |_| {},
        |url, limit, path, _| {
            reply(
                &if url.contains("heatmap") {
                    replay()
                } else {
                    trace("abc123", "adsb_icao")
                },
                limit,
                path,
            )
        },
    )
    .unwrap()
}
fn config() -> adsblol::TraceImportConfig {
    adsblol::TraceImportConfig {
        source_label: "independent history controls".into(),
        time_origin_utc_s: DAY,
        time_window_utc_s: [DAY + 1.0, DAY + 40.0],
        ..Default::default()
    }
}

#[test]
fn bounded_explicit_plan_has_exact_official_urls_and_no_network() {
    let p = history::plan(&request()).unwrap();
    assert_eq!(p.start_utc_s, DAY + 1.0);
    assert_eq!(p.replay_chunks.len(), 1);
    assert_eq!(
        p.replay_chunks[0].url,
        "https://adsb.lol/globe_history/2024/04/08/heatmap/00.bin.ttf"
    );
    assert!(!p.coverage_complete);
    let mut r = request();
    r.utc_start = "2024-04-08T00:29:59Z".into();
    r.utc_end = "2024-04-08T01:00:00Z".into();
    assert_eq!(
        history::plan(&r)
            .unwrap()
            .replay_chunks
            .iter()
            .map(|c| c.index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    let mut changed = p;
    changed.replay_chunks[0].url = "https://attacker.invalid/x".into();
    assert!(history::validate_plan(&changed).is_err());
}

#[test]
fn invalid_date_time_area_caps_and_cross_day_requests_reject() {
    for start in [
        "2024-02-30T00:00:01Z",
        "2024-04-08T24:00:01Z",
        "2024-04-08T00:00:01",
        "2024-04-08 00:00:01Z",
        "2022-04-08T00:00:01Z",
    ] {
        let mut r = request();
        r.utc_start = start.into();
        assert!(history::plan(&r).is_err(), "{start}");
    }
    for bounds in [
        [-91., -73., 44., -71.],
        [42., -73., 41., -71.],
        [0., -73., 44., -71.],
        [42., -180., 44., 180.],
        [42., f64::NAN, 44., -71.],
    ] {
        let mut r = request();
        r.bounds = bounds;
        assert!(history::plan(&r).is_err());
    }
    let mut r = request();
    r.utc_end = "2024-04-09T00:00:30Z".into();
    assert!(history::plan(&r).is_err());
    r = request();
    r.utc_end = "2024-04-08T01:00:02Z".into();
    assert!(history::plan(&r).is_err());
    r = request();
    r.max_traces = 257;
    assert!(history::plan(&r).is_err());
    r.max_traces = 0;
    assert!(history::plan(&r).is_err());
    r = request();
    r.max_download_bytes = MAX_DOWNLOAD_BYTES + 1;
    assert!(history::plan(&r).is_err());
}

#[test]
fn old_and_current_replay_layouts_preserve_signed_values_and_source_semantics() {
    let p = history::plan(&request()).unwrap();
    let positions = vec![
        position(0xabc123, 5, false, 43., -72., -4, 1234),
        position(0x123456, 31, true, 43., -72., -123, -1),
        position(0x123457, 12, false, 43., -72., -124, 0),
    ];
    let current = decode_replay(
        &replay_with(positions.clone(), false),
        &p,
        &p.replay_chunks[0],
    )
    .unwrap();
    let old = decode_replay(&replay_with(positions, true), &p, &p.replay_chunks[0]).unwrap();
    assert_eq!(old.records, current.records);
    assert_eq!(old.summary.legacy_index_rows, 180);
    assert_eq!(current.records[0].barometric_altitude_ft, Some(-100));
    assert_eq!(current.records[0].groundspeed_kt, Some(123.4));
    assert_eq!(current.records[0].position_source, "mlat");
    assert_eq!(current.records[1].icao24, "~123456");
    assert!(current.records[1].on_ground);
    assert_eq!(current.records[1].position_source_code, 31);
    assert_eq!(current.records[1].position_source, "unknown");
    assert_eq!(current.records[1].groundspeed_kt, None);
    assert!(!current.records[2].on_ground);
    assert_eq!(current.records[2].barometric_altitude_ft, None);
}

#[test]
fn replay_filters_time_and_date_line_without_losing_boundary_positions() {
    let mut r = request();
    r.bounds = [42., 175., 44., -175.];
    let p = history::plan(&r).unwrap();
    let positions = vec![
        position(1, 0, false, 42., 175., 1, 10),
        position(2, 0, false, 44., -175., 1, 10),
        position(3, 0, false, 43., 0., 1, 10),
    ];
    let out = decode_replay(&replay_with(positions, false), &p, &p.replay_chunks[0]).unwrap();
    assert_eq!(out.records.len(), 2);
    let mut r = request();
    r.utc_start = "2024-04-08T00:00:11Z".into();
    let p = history::plan(&r).unwrap();
    assert!(
        decode_replay(&replay(), &p, &p.replay_chunks[0])
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
fn malformed_markers_indices_coordinates_and_speed_are_rejected_before_filtering() {
    let p = history::plan(&request()).unwrap();
    for raw in [
        vec![],
        vec![0; 17],
        bytes(&[position(1, 0, false, 43., -72., 1, 1)]),
        bytes(&[marker(DAY + 10.), marker(DAY + 10.)]),
        bytes(&[marker(DAY - 1.)]),
        bytes(&[[0x0e7f7c9d, 0, 0, 0]]),
    ] {
        assert!(decode_replay(&raw, &p, &p.replay_chunks[0]).is_err());
    }
    let mut bad = replay_with(vec![], true);
    bad[4] = 1;
    assert!(decode_replay(&bad, &p, &p.replay_chunks[0]).is_err());
    let mut bad = replay_with(vec![], true);
    let first = bad[..4].to_vec();
    bad[16..20].copy_from_slice(&first);
    assert!(decode_replay(&bad, &p, &p.replay_chunks[0]).is_err());
    for pos in [
        position(1, 0, false, 91., 100., 1, 1),
        position(1, 0, false, 0., 0., 1, -2),
    ] {
        assert!(decode_replay(&replay_with(vec![pos], false), &p, &p.replay_chunks[0]).is_err());
    }
}

#[test]
fn metadata_rows_do_not_become_flights_and_replay_time_gaps_are_visible() {
    let p = history::plan(&request()).unwrap();
    let mut rows = vec![
        marker(DAY),
        [0xabc123, 1 << 30, 0x41424344, 0x45464748],
        marker(DAY + 20.),
    ];
    rows.push(position(0xabc123, 0, false, 43., -72., 40, 100));
    let index = decode_replay(&bytes(&rows), &p, &p.replay_chunks[0]).unwrap();
    assert_eq!(index.summary.metadata_rows, 1);
    assert_eq!(index.records.len(), 1);
    assert_eq!(index.summary.missing_slice_gaps, 1);
    assert!(index.summary.leading_or_trailing_gap);
}

#[test]
fn gzip_is_bounded_checks_crc_and_rejects_trailing_or_concatenated_members() {
    let valid = gzip(b"hello");
    assert_eq!(decode_payload(&valid, 5).unwrap(), b"hello");
    assert!(decode_payload(&valid, 4).is_err());
    assert!(decode_payload(&valid[..valid.len() - 1], 100).is_err());
    let mut bad = valid.clone();
    bad[12] ^= 127;
    assert!(decode_payload(&bad, 100).is_err());
    let mut trailing = valid.clone();
    trailing.push(0);
    assert!(decode_payload(&trailing, 100).is_err());
    let mut concat = valid.clone();
    concat.extend(valid);
    assert!(decode_payload(&concat, 100).is_err());
    assert!(decode_payload(b"12345", 4).is_err());
}

#[test]
fn complete_request_is_native_importable_and_resume_verifies_without_network() {
    let tmp = Scratch::new();
    let p = history::plan(&request()).unwrap();
    let one = run(&tmp.0, &p);
    assert_eq!(one.status, HistoryStatus::Complete);
    assert!(one.replay_files_complete && one.selected_traces_complete);
    assert!(!one.coverage_complete && !one.location_verified);
    assert_eq!(one.attempts.len(), 2);
    let imported = import_traces(&tmp.0, &one, &config(), false).unwrap();
    assert_eq!(imported.stats.output_passes, 1);
    assert_eq!(imported.stats.output_points, 3);
    assert_eq!(
        imported.passes[0].source_points[0].position_source,
        "adsb_icao"
    );
    let two = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("verified cache should not request network"),
    )
    .unwrap();
    assert_eq!(two.status, HistoryStatus::Complete);
    assert_eq!(two.new_http_payload_bytes, 0);
    assert_eq!(two.http_payload_bytes, one.http_payload_bytes);
    assert_eq!(two.reused_files, 2);
    assert_ne!(two.report_path, one.report_path);
    assert!(tmp.0.join(one.report_path).exists());
}

#[test]
fn non_icao_unknown_sources_and_explicit_trace_cap_remain_in_inventory() {
    let tmp = Scratch::new();
    let mut r = request();
    r.max_traces = 1;
    let p = history::plan(&r).unwrap();
    let body = replay_with(
        vec![
            position(3, 31, true, 43., -72., 40, 100),
            position(2, 5, false, 43., -72., 40, 100),
            position(1, 31, false, 43., -72., 40, 100),
        ],
        false,
    );
    let report = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |url, limit, path, _| {
            reply(
                &if url.contains("heatmap") {
                    body.clone()
                } else {
                    assert!(url.ends_with("000001.json"));
                    trace("000001", "mlat")
                },
                limit,
                path,
            )
        },
    )
    .unwrap();
    assert_eq!(report.status, HistoryStatus::Partial);
    assert_eq!(report.non_icao_omissions, 1);
    assert_eq!(report.trace_cap_omissions, 1);
    assert_eq!(report.candidates.len(), 3);
    assert_eq!(report.candidates[0].source_codes, vec![31]);
    assert_eq!(report.traces.len(), 1);
    assert!(import_traces(&tmp.0, &report, &config(), false).is_err());
    let imported = import_traces(&tmp.0, &report, &config(), true).unwrap();
    assert_eq!(imported.stats.rejected_position_source, 3);
    assert_eq!(imported.stats.output_passes, 0);
}

#[test]
fn missing_trace_is_partial_and_retry_fetches_only_failed_file() {
    let tmp = Scratch::new();
    let p = history::plan(&request()).unwrap();
    let one = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |url, limit, path, _| {
            if url.contains("heatmap") {
                reply(&replay(), limit, path)
            } else {
                Ok(TransportResult {
                    http_status: Some(404),
                    error: Some("Fixture missing trace".into()),
                })
            }
        },
    )
    .unwrap();
    assert_eq!(one.status, HistoryStatus::Partial);
    assert!(!one.selected_traces_complete);
    assert_eq!(one.candidates.len(), 1);
    assert!(one.traces[0].error.as_ref().unwrap().contains("missing"));
    let mut calls = 0;
    let two = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |url, limit, path, _| {
            assert!(url.contains("traces"));
            calls += 1;
            reply(&trace("abc123", "adsb_icao"), limit, path)
        },
    )
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(two.status, HistoryStatus::Complete);
    assert_eq!(two.attempts.len(), 3);
    assert!(two.attempts[1].error.is_some());
}

#[test]
fn one_byte_over_limit_is_charged_failed_resumable_and_owned_deletable() {
    use geosonnar::data_catalog as catalog;
    let tmp = Scratch::new();
    let manifest = catalog_case(&tmp.0);
    let original = fs::read(tmp.0.join("recording-control.bin")).unwrap();
    let mut request = request();
    request.max_download_bytes = 35;
    let planned = catalog::plan_history(&manifest, &request).unwrap();
    let acquired = catalog::fetch_history_with_transport(
        &manifest,
        &planned.entry.id,
        &AtomicBool::new(false),
        |_| {},
        |_, limit, path, _| {
            assert_eq!(limit, 35);
            fs::write(path, vec![b'x'; limit as usize + 1]).unwrap();
            // Even an erroneously successful transport cannot promote the
            // sentinel-bearing body to a usable response.
            Ok(TransportResult {
                http_status: Some(200),
                error: None,
            })
        },
    )
    .unwrap();
    let report = acquired.report.as_ref().unwrap();
    assert_eq!(report.status, HistoryStatus::BudgetExhausted);
    assert_eq!(report.http_payload_bytes, 36);
    assert_eq!(report.attempts.len(), 1);
    assert!(
        report.attempts[0]
            .error
            .as_ref()
            .unwrap()
            .contains("sentinel")
    );
    let cache = tmp
        .0
        .join("managed-data")
        .join(&planned.entry.id)
        .join("cache");
    let response = cache.join(&report.attempts[0].body.path);
    let before = fs::read(&response).unwrap();
    let resumed = catalog::fetch_history_with_transport(
        &manifest,
        &planned.entry.id,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("charged overflow must not reset the budget"),
    )
    .unwrap();
    let resumed_report = resumed.report.as_ref().unwrap();
    assert_eq!(resumed_report.http_payload_bytes, 36);
    assert_eq!(resumed_report.new_http_payload_bytes, 0);
    assert_eq!(resumed_report.attempts.len(), 1);
    assert_eq!(fs::read(&response).unwrap(), before);
    assert_eq!(
        catalog::read_history(&manifest, &planned.entry.id)
            .unwrap()
            .report
            .unwrap()
            .http_payload_bytes,
        36
    );
    let removed = catalog::delete_owned(&manifest, &planned.entry.id).unwrap();
    assert!(removed.originals_preserved);
    assert!(!response.exists());
    assert_eq!(
        fs::read(tmp.0.join("recording-control.bin")).unwrap(),
        original
    );
}

#[test]
fn interrupted_sentinel_is_charged_and_cannot_be_forged_into_a_success() {
    let tmp = Scratch::new();
    let mut request = request();
    request.max_download_bytes = 35;
    let p = history::plan(&request).unwrap();
    let first = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |_, limit, path, _| {
            fs::write(path, vec![b'x'; limit as usize + 1]).unwrap();
            Ok(TransportResult {
                http_status: Some(200),
                error: None,
            })
        },
    )
    .unwrap();
    let receipt_path = tmp.0.join("attempts/00000001/receipt.json");
    fs::remove_file(&receipt_path).unwrap();
    let resumed = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("unreceipted overflow still exhausts the budget"),
    )
    .unwrap();
    assert_eq!(resumed.http_payload_bytes, first.http_payload_bytes);
    assert_eq!(resumed.new_http_payload_bytes, 0);
    assert!(
        resumed.attempts[0]
            .error
            .as_ref()
            .unwrap()
            .contains("Previous process")
    );
    let mut forged: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    forged["error"] = Value::Null;
    forged["http_status"] = json!(200);
    fs::write(receipt_path, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert!(
        fetch_with_transport(
            &p,
            &tmp.0,
            &AtomicBool::new(false),
            |_| {},
            |_, _, _, _| panic!("forged success must fail before transfer")
        )
        .is_err()
    );
}

#[test]
fn failed_terminal_sentinel_keeps_earlier_valid_traces_available_for_explicit_partial_import() {
    let tmp = Scratch::new();
    let replay_body = replay_with(
        vec![
            position(1, 0, false, 43., -72., 40, 100),
            position(2, 0, false, 43., -72., 40, 100),
        ],
        false,
    );
    let first_trace = trace("000001", "adsb_icao");
    let mut request = request();
    request.max_download_bytes = (replay_body.len() + first_trace.len() + 8) as u64;
    let p = history::plan(&request).unwrap();
    let report = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |url, limit, path, _| {
            if url.contains("heatmap") {
                reply(&replay_body, limit, path)
            } else if url.ends_with("trace_full_000001.json") {
                reply(&first_trace, limit, path)
            } else {
                assert!(url.ends_with("trace_full_000002.json"));
                assert_eq!(limit, 8);
                fs::write(path, b"123456789").unwrap();
                Ok(TransportResult {
                    http_status: Some(200),
                    error: None,
                })
            }
        },
    )
    .unwrap();
    assert_eq!(report.status, HistoryStatus::BudgetExhausted);
    assert_eq!(report.http_payload_bytes, request.max_download_bytes + 1);
    assert_eq!(report.attempts.len(), 3);
    assert!(report.attempts[2].error.is_some());
    assert!(import_traces(&tmp.0, &report, &config(), false).is_err());
    let imported = import_traces(&tmp.0, &report, &config(), true).unwrap();
    assert_eq!(imported.stats.output_passes, 1);
    assert_eq!(imported.stats.output_points, 3);
    let resumed = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("only prior verified successes may be reused"),
    )
    .unwrap();
    assert_eq!(resumed.new_http_payload_bytes, 0);
    assert_eq!(resumed.http_payload_bytes, report.http_payload_bytes);
    assert_eq!(resumed.reused_files, 2);
    assert_eq!(resumed.attempts.len(), 3);
    // Import must independently reject a saved report edited to claim the
    // oversized response succeeded, even if its JSON snapshot matches memory.
    let mut forged = resumed;
    forged.attempts[2].error = None;
    fs::write(
        tmp.0.join(&forged.report_path),
        serde_json::to_vec_pretty(&forged).unwrap(),
    )
    .unwrap();
    assert!(import_traces(&tmp.0, &forged, &config(), true).is_err());
}

#[test]
fn byte_budget_counts_failed_payloads_and_prevents_more_requests_on_resume() {
    let tmp = Scratch::new();
    let mut r = request();
    r.max_download_bytes = 35;
    let p = history::plan(&r).unwrap();
    let mut calls = 0;
    let one = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |_, limit, path, _| {
            calls += 1;
            reply(&replay(), limit, path)
        },
    )
    .unwrap();
    assert_eq!(one.status, HistoryStatus::BudgetExhausted);
    assert_eq!(one.http_payload_bytes, 35);
    assert_eq!(calls, 1);
    assert!(!one.replay_files_complete);
    let two = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("budget exhausted"),
    )
    .unwrap();
    assert_eq!(two.status, HistoryStatus::BudgetExhausted);
    assert_eq!(two.new_http_payload_bytes, 0);
}

#[test]
fn cancellation_preserves_complete_index_but_never_claims_completed_traces() {
    let tmp = Scratch::new();
    let p = history::plan(&request()).unwrap();
    let cancel = AtomicBool::new(false);
    let report = fetch_with_transport(
        &p,
        &tmp.0,
        &cancel,
        |progress| {
            if progress.stage == "replay_index" {
                cancel.store(true, Ordering::Relaxed)
            }
        },
        |url, limit, path, _| {
            assert!(url.contains("heatmap"));
            reply(&replay(), limit, path)
        },
    )
    .unwrap();
    assert_eq!(report.status, HistoryStatus::Cancelled);
    assert!(report.replay_files_complete);
    assert!(!report.selected_traces_complete);
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.attempts.len(), 1);
    let tmp2 = Scratch::new();
    let report = fetch_with_transport(
        &p,
        &tmp2.0,
        &AtomicBool::new(true),
        |_| {},
        |_, _, _, _| panic!("cancelled before request"),
    )
    .unwrap();
    assert_eq!(report.status, HistoryStatus::Cancelled);
    assert_eq!(report.http_payload_bytes, 0);
    assert!(!report.replay_files_complete);
}

#[test]
fn corrupt_cached_http_or_decoded_trace_is_rejected_without_redownload() {
    let tmp = Scratch::new();
    let p = history::plan(&request()).unwrap();
    let report = run(&tmp.0, &p);
    let path = tmp.0.join(&report.attempts[0].body.path);
    let mut data = fs::read(&path).unwrap();
    data[0] ^= 1;
    fs::write(&path, data).unwrap();
    assert!(
        fetch_with_transport(
            &p,
            &tmp.0,
            &AtomicBool::new(false),
            |_| {},
            |_, _, _, _| panic!("never refresh corrupt cache silently")
        )
        .unwrap_err()
        .contains("SHA-256")
    );
    let tmp2 = Scratch::new();
    let report = run(&tmp2.0, &p);
    let path = tmp2
        .0
        .join(&report.traces[0].decoded.as_ref().unwrap().path);
    let mut data = fs::read(&path).unwrap();
    data[1] = b' ';
    fs::write(path, data).unwrap();
    assert!(import_traces(&tmp2.0, &report, &config(), false).is_err());
}

#[test]
fn changed_request_lock_and_import_snapshot_do_not_overwrite_or_alias() {
    let tmp = Scratch::new();
    let p = history::plan(&request()).unwrap();
    let mut report = run(&tmp.0, &p);
    let original = fs::read(tmp.0.join(&report.report_path)).unwrap();
    let mut r = request();
    r.max_traces += 1;
    assert!(
        fetch_with_transport(
            &history::plan(&r).unwrap(),
            &tmp.0,
            &AtomicBool::new(false),
            |_| {},
            |_, _, _, _| panic!()
        )
        .is_err()
    );
    report.traces.clear();
    assert!(import_traces(&tmp.0, &report, &config(), false).is_err());
    assert_eq!(original, fs::read(tmp.0.join(&report.report_path)).unwrap());
    fs::write(tmp.0.join("download.lock"), b"fixture").unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(tmp.0.join("download.lock"))
        .unwrap();
    held.try_lock().unwrap();
    assert!(
        fetch_with_transport(
            &p,
            &tmp.0,
            &AtomicBool::new(false),
            |_| {},
            |_, _, _, _| panic!()
        )
        .unwrap_err()
        .contains("locked")
    );
    drop(held);
    let resumed = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("stale sentinel must not force redownload"),
    )
    .unwrap();
    assert_eq!(resumed.status, HistoryStatus::Complete);
    assert_eq!(fs::metadata(tmp.0.join("download.lock")).unwrap().len(), 0);
}

#[test]
fn malformed_or_wrong_day_trace_is_retained_as_failed_not_importable() {
    for data in [
        b"<html>error</html>".to_vec(),
        serde_json::to_vec(&json!({"hex":"abc123","timestamp":DAY,"trace":[]})).unwrap(),
        trace("ffffff", "adsb_icao"),
        serde_json::to_vec(&json!({"icao":"abc123","timestamp":DAY-86400.,"trace":[]})).unwrap(),
    ] {
        let tmp = Scratch::new();
        let p = history::plan(&request()).unwrap();
        let report = fetch_with_transport(
            &p,
            &tmp.0,
            &AtomicBool::new(false),
            |_| {},
            |url, limit, path, _| {
                reply(
                    &if url.contains("heatmap") {
                        replay()
                    } else {
                        data.clone()
                    },
                    limit,
                    path,
                )
            },
        )
        .unwrap();
        assert_eq!(report.status, HistoryStatus::Partial);
        assert!(report.traces[0].decoded.is_none());
        assert!(report.attempts[1].error.is_some());
        assert!(import_traces(&tmp.0, &report, &config(), true).is_err());
    }
}

#[test]
fn malformed_trace_rows_fail_acquisition_but_other_candidates_remain_importable() {
    let bad_rows = [
        json!(null),
        json!([10.0, 43.0, -72.0]),
        json!([null, 43.0, -72.0, 1000.0, 110.0, 90.0, 0, 0.0, null]),
        json!([-1.0, 43.0, -72.0, 1000.0, 110.0, 90.0, 0, 0.0, null]),
        json!([604801.0, 43.0, -72.0, 1000.0, 110.0, 90.0, 0, 0.0, null]),
        json!([10.0, 43.0, -72.0, 1000.0, 110.0, 90.0, -1, 0.0, null]),
        json!([
            10.0,
            43.0,
            -72.0,
            1000.0,
            110.0,
            90.0,
            4294967296u64,
            0.0,
            null
        ]),
    ];
    for row in bad_rows {
        let tmp = Scratch::new();
        let p = history::plan(&request()).unwrap();
        let index = replay_with(
            vec![
                position(0xabc123, 0, false, 43., -72., 40, 100),
                position(0xabc124, 0, false, 43., -72., 40, 100),
            ],
            false,
        );
        let bad =
            serde_json::to_vec(&json!({"icao":"abc124","timestamp":DAY,"trace":[row]})).unwrap();
        let report = fetch_with_transport(
            &p,
            &tmp.0,
            &AtomicBool::new(false),
            |_| {},
            |url, limit, path, _| {
                reply(
                    &if url.contains("heatmap") {
                        index.clone()
                    } else if url.ends_with("abc123.json") {
                        trace("abc123", "adsb_icao")
                    } else {
                        bad.clone()
                    },
                    limit,
                    path,
                )
            },
        )
        .unwrap();
        assert_eq!(report.status, HistoryStatus::Partial);
        assert_eq!(report.traces.len(), 2);
        assert!(report.traces[0].decoded.is_some());
        assert!(report.traces[1].decoded.is_none());
        assert!(
            report.traces[1]
                .error
                .as_ref()
                .unwrap()
                .contains("Trace row")
        );
        let imported = import_traces(&tmp.0, &report, &config(), true).unwrap();
        assert_eq!(imported.stats.output_points, 3);
        assert_eq!(imported.sources[0].icao24, "abc123");
        assert!(!imported.location_verified);
        assert!(adsblol::normalize_trace(std::str::from_utf8(&bad).unwrap(), &config()).is_err());
    }
}

/// A dedicated subprocess entry used by the hard-stop regression below. It does
/// nothing during ordinary test discovery; the parent owns and kills its child.
#[test]
fn history_cache_lock_child() {
    let Some(root) = std::env::var_os("GEOSONNAR_HISTORY_LOCK_CHILD") else {
        return;
    };
    let root = PathBuf::from(root);
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("download.lock"))
        .unwrap();
    file.try_lock().unwrap();
    fs::write(root.join("lock-child-ready"), b"OS lock held").unwrap();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

struct LockChild(std::process::Child);
impl Drop for LockChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn locked_child(root: &Path) -> LockChild {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "history_cache_lock_child", "--nocapture"])
        .env("GEOSONNAR_HISTORY_LOCK_CHILD", root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let child = LockChild(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !root.join("lock-child-ready").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "hidden lock child did not become ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    child
}

#[test]
fn actual_process_death_releases_cache_lock_without_manual_cleanup() {
    let tmp = Scratch::new();
    let p = history::plan(&request()).unwrap();
    let before = run(&tmp.0, &p);
    let child = locked_child(&tmp.0);
    assert!(
        fetch_with_transport(
            &p,
            &tmp.0,
            &AtomicBool::new(false),
            |_| {},
            |_, _, _, _| panic!()
        )
        .unwrap_err()
        .contains("locked")
    );
    drop(child); // kills only our hidden test subprocess; no graceful destructor.
    assert!(tmp.0.join("download.lock").exists());
    let after = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("verified cache reuse"),
    )
    .unwrap();
    assert_eq!(after.status, HistoryStatus::Complete);
    assert_eq!(after.http_payload_bytes, before.http_payload_bytes);
    assert_eq!(after.new_http_payload_bytes, 0);
}

#[test]
fn interrupted_attempt_is_accounted_and_retried_without_reusing_unreceipted_data() {
    let tmp = Scratch::new();
    let p = history::plan(&request()).unwrap();
    let report = run(&tmp.0, &p);
    let receipt = tmp.0.join("attempts/00000002/receipt.json");
    fs::remove_file(receipt).unwrap();
    let mut calls = 0;
    let resumed = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |url, limit, path, _| {
            assert!(url.contains("traces"));
            calls += 1;
            reply(&trace("abc123", "adsb_icao"), limit, path)
        },
    )
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(resumed.attempts.len(), 3);
    assert!(
        resumed.attempts[1]
            .error
            .as_ref()
            .unwrap()
            .contains("Previous process")
    );
    assert_eq!(
        resumed.http_payload_bytes,
        report.http_payload_bytes + trace("abc123", "adsb_icao").len() as u64
    );
}

#[test]
fn optional_preserved_provider_replays_match_independent_python_index_exactly() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("data/validation/field-esid103-tracks-002");
    if !root.join("acquisition.json").is_file() {
        return;
    }
    let request = HistoryRequest {
        utc_start: "2024-04-08T04:20:00Z".into(),
        utc_end: "2024-04-08T04:40:00Z".into(),
        bounds: [42.5, -73.5, 45.5, -70.0],
        max_traces: 100,
        max_download_bytes: 95_000_000,
    };
    let p = history::plan(&request).unwrap();
    let mut native = Vec::new();
    for chunk in &p.replay_chunks {
        native.extend(
            decode_replay(
                &fs::read(root.join(format!("source/replay-{:02}.http.bin", chunk.index))).unwrap(),
                &p,
                chunk,
            )
            .unwrap()
            .records,
        );
    }
    let expected: Vec<Value> =
        serde_json::from_slice(&fs::read(root.join("regional-replay-index.json")).unwrap())
            .unwrap();
    assert_eq!(native.len(), 615);
    assert_eq!(native.len(), expected.len());
    for (a, b) in native.iter().zip(&expected) {
        let a = serde_json::to_value(a).unwrap();
        for (key, value) in b.as_object().unwrap() {
            assert_eq!(&a[key], value, "{key}");
        }
    }
    let ids: BTreeMap<_, _> = native
        .iter()
        .filter(|r| !r.icao24.starts_with('~'))
        .map(|r| (r.icao24.clone(), ()))
        .collect();
    assert_eq!(ids.len(), 19);
    // A complete injected local replay exercises real gzip, identities, raw JSON
    // and native import without network or changes to frozen source artifacts.
    let tmp = Scratch::new();
    let report = fetch_with_transport(
        &p,
        &tmp.0,
        &AtomicBool::new(false),
        |_| {},
        |url, limit, path, _| {
            let source = if url.contains("heatmap") {
                let index = &url[url.len() - 10..url.len() - 8];
                root.join(format!("source/replay-{index}.http.bin"))
            } else {
                root.join("traces").join(url.rsplit('/').next().unwrap())
            };
            reply(&fs::read(source).unwrap(), limit, path)
        },
    )
    .unwrap();
    assert_eq!(report.status, HistoryStatus::Complete);
    assert_eq!(report.traces.len(), 19);
    let cfg = adsblol::TraceImportConfig {
        source_label: "frozen provider input native acquisition control".into(),
        time_origin_utc_s: DAY + 4. * 3600. + 24. * 60. + 53.,
        time_window_utc_s: [p.start_utc_s, p.end_utc_s],
        ..Default::default()
    };
    let actual = import_traces(&tmp.0, &report, &cfg, false).unwrap();
    let expected = adsblol::merge_reports(
        ids.keys()
            .map(|id| {
                adsblol::normalize_trace(
                    &fs::read_to_string(root.join("traces").join(format!("trace_full_{id}.json")))
                        .unwrap(),
                    &cfg,
                )
                .unwrap()
            })
            .collect(),
        &cfg,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&actual.passes).unwrap(),
        serde_json::to_value(&expected.passes).unwrap()
    );
    assert_eq!(actual.stats.source_rows, 42_604);
}

fn catalog_case(root: &Path) -> PathBuf {
    let source = root.join("recording-control.bin");
    fs::write(
        &source,
        b"fabricated source: never changed by data management",
    )
    .unwrap();
    let manifest = root.join("case.json");
    let doc = json!({"schema_version":1,"case_id":"history-control","title":"Regional history control","created_unix_s":1,"local_only":true,
        "recording":{"path":"recording-control.bin","sha256":geosonnar::detailed_data::sha256(&source).unwrap(),"bytes":fs::metadata(&source).unwrap().len(),
        "media":{"duration_s":40.0,"sample_rate_hz":48000,"channels":1,"audio_codec":"fabricated"}}});
    fs::write(&manifest, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    manifest
}

#[test]
fn managed_history_reopens_imports_and_deletes_only_registered_cache_files() {
    use geosonnar::data_catalog as catalog;
    let tmp = Scratch::new();
    let manifest = catalog_case(&tmp.0);
    let source_hash =
        geosonnar::detailed_data::sha256(&tmp.0.join("recording-control.bin")).unwrap();
    let planned = catalog::plan_history(&manifest, &request()).unwrap();
    let id = &planned.entry.id;
    assert_eq!(planned.entry.format, "adsblol_history_plan");
    assert!(planned.report.is_none());
    assert_eq!(catalog::list(&manifest).unwrap()[0].status, "planned");
    assert!(
        catalog::read_history(&manifest, id)
            .unwrap()
            .report
            .is_none()
    );
    let acquired = catalog::fetch_history_with_transport(
        &manifest,
        id,
        &AtomicBool::new(false),
        |_| {},
        |url, limit, path, _| {
            reply(
                &if url.contains("heatmap") {
                    replay()
                } else {
                    trace("abc123", "adsb_icao")
                },
                limit,
                path,
            )
        },
    )
    .unwrap();
    assert_eq!(acquired.entry.format, "adsblol_history");
    assert_eq!(
        acquired.report.as_ref().unwrap().status,
        HistoryStatus::Complete
    );
    let reopened = catalog::read_history(&manifest, id).unwrap();
    assert_eq!(reopened.entry.primary_file, acquired.entry.primary_file);
    let raw_paths = catalog::verified_files(&manifest, id).unwrap();
    assert!(raw_paths.iter().any(|p| p.ends_with("response.bin")));
    assert!(
        raw_paths
            .iter()
            .any(|p| p.ends_with("trace_full_abc123.json"))
    );
    let imported =
        catalog::import_history(&manifest, id, &config(), false, &AtomicBool::new(false)).unwrap();
    assert_eq!(imported.format, "normalized_tracks");
    let normalized_paths = catalog::verified_files(&manifest, &imported.id).unwrap();
    let path = normalized_paths
        .iter()
        .find(|p| p.ends_with("normalized.json"))
        .unwrap();
    let normalized: adsblol::TraceImportReport =
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(normalized.stats.output_points, 3);
    let resumed = catalog::fetch_history_with_transport(
        &manifest,
        id,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("complete cache should be reused"),
    )
    .unwrap();
    assert_eq!(resumed.report.as_ref().unwrap().new_http_payload_bytes, 0);
    assert_ne!(resumed.entry.primary_file, acquired.entry.primary_file);
    let deleted = catalog::delete_owned(&manifest, id).unwrap();
    assert!(deleted.originals_preserved);
    assert!(raw_paths.iter().all(|p| !p.exists()));
    assert!(path.exists());
    assert_eq!(
        geosonnar::detailed_data::sha256(&tmp.0.join("recording-control.bin")).unwrap(),
        source_hash
    );
    assert!(catalog::read_history(&manifest, id).is_err());
}

#[test]
fn managed_partial_coverage_requires_explicit_import_and_survives_retry() {
    use geosonnar::data_catalog as catalog;
    let tmp = Scratch::new();
    let manifest = catalog_case(&tmp.0);
    let planned = catalog::plan_history(&manifest, &request()).unwrap();
    let id = &planned.entry.id;
    let body = replay_with(
        vec![
            position(0xabc123, 0, false, 43., -72., 40, 100),
            position(0x123456, 8, true, 43., -72., 40, 100),
        ],
        false,
    );
    let acquired = catalog::fetch_history_with_transport(
        &manifest,
        id,
        &AtomicBool::new(false),
        |_| {},
        |url, limit, path, _| {
            reply(
                &if url.contains("heatmap") {
                    body.clone()
                } else {
                    trace("abc123", "adsb_icao")
                },
                limit,
                path,
            )
        },
    )
    .unwrap();
    assert_eq!(
        acquired.report.as_ref().unwrap().status,
        HistoryStatus::Partial
    );
    assert_eq!(catalog::list(&manifest).unwrap()[0].status, "partial");
    assert!(
        catalog::import_history(&manifest, id, &config(), false, &AtomicBool::new(false)).is_err()
    );
    let imported =
        catalog::import_history(&manifest, id, &config(), true, &AtomicBool::new(false)).unwrap();
    assert_eq!(imported.source["allow_partial"], true);
    assert_eq!(imported.source["history_status"], "partial");
    let reopen = catalog::read_history(&manifest, id).unwrap();
    assert_eq!(reopen.report.as_ref().unwrap().non_icao_omissions, 1);
}

#[test]
fn managed_history_refuses_unexpected_or_changed_files_before_use_or_deletion() {
    use geosonnar::data_catalog as catalog;
    let tmp = Scratch::new();
    let manifest = catalog_case(&tmp.0);
    let planned = catalog::plan_history(&manifest, &request()).unwrap();
    let id = &planned.entry.id;
    let acquired = catalog::fetch_history_with_transport(
        &manifest,
        id,
        &AtomicBool::new(false),
        |_| {},
        |url, limit, path, _| {
            reply(
                &if url.contains("heatmap") {
                    replay()
                } else {
                    trace("abc123", "adsb_icao")
                },
                limit,
                path,
            )
        },
    )
    .unwrap();
    let root = tmp.0.join("managed-data").join(id);
    let unexpected = root.join("cache/my-notes.txt");
    fs::write(&unexpected, b"must not adopt or delete this").unwrap();
    assert!(
        catalog::fetch_history_with_transport(
            &manifest,
            id,
            &AtomicBool::new(false),
            |_| {},
            |_, _, _, _| panic!()
        )
        .is_err()
    );
    assert!(catalog::delete_owned(&manifest, id).is_err());
    assert!(unexpected.exists());
    fs::remove_file(unexpected).unwrap();
    let trace = root.join("cache").join(
        &acquired.report.as_ref().unwrap().traces[0]
            .decoded
            .as_ref()
            .unwrap()
            .path,
    );
    let mut bytes = fs::read(&trace).unwrap();
    bytes[1] = b' ';
    fs::write(&trace, bytes).unwrap();
    assert!(catalog::read_history(&manifest, id).is_err());
    assert!(catalog::delete_owned(&manifest, id).is_err());
    assert!(trace.exists());
}

#[test]
fn managed_delete_and_resume_share_the_core_os_lock_after_a_hard_stop() {
    use geosonnar::data_catalog as catalog;
    let tmp = Scratch::new();
    let manifest = catalog_case(&tmp.0);
    let planned = catalog::plan_history(&manifest, &request()).unwrap();
    let id = &planned.entry.id;
    let acquired = catalog::fetch_history_with_transport(
        &manifest,
        id,
        &AtomicBool::new(false),
        |_| {},
        |url, limit, path, _| {
            reply(
                &if url.contains("heatmap") {
                    replay()
                } else {
                    trace("abc123", "adsb_icao")
                },
                limit,
                path,
            )
        },
    )
    .unwrap();
    let cache = tmp.0.join("managed-data").join(id).join("cache");
    let child = locked_child(&cache);
    assert!(
        catalog::delete_owned(&manifest, id)
            .unwrap_err()
            .contains("locked")
    );
    assert!(
        catalog::fetch_history_with_transport(
            &manifest,
            id,
            &AtomicBool::new(false),
            |_| {},
            |_, _, _, _| panic!()
        )
        .unwrap_err()
        .contains("locked")
    );
    assert!(
        cache
            .join(
                &acquired.report.as_ref().unwrap().traces[0]
                    .decoded
                    .as_ref()
                    .unwrap()
                    .path
            )
            .exists()
    );
    drop(child);
    fs::remove_file(cache.join("lock-child-ready")).unwrap();
    let resumed = catalog::fetch_history_with_transport(
        &manifest,
        id,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("resume should reuse all files"),
    )
    .unwrap();
    assert_eq!(resumed.report.as_ref().unwrap().new_http_payload_bytes, 0);
    assert_eq!(
        resumed.report.as_ref().unwrap().status,
        HistoryStatus::Complete
    );
    catalog::delete_owned(&manifest, id).unwrap();
    assert!(cache.join("download.lock").exists());
    assert_eq!(fs::metadata(cache.join("download.lock")).unwrap().len(), 0);
}
