use super::*;
use crate::adsblol::{TracePass, TraceSource};
use std::{collections::BTreeMap, sync::atomic::AtomicUsize};

#[path = "track_store_packed_tests.rs"]
mod packed_tests;

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
    parent: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
        fs::create_dir_all(&parent).unwrap();
        let parent = parent.canonicalize().unwrap();
        let root = parent.join(format!(
            "track-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self { root, parent }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(path) = self.root.canonicalize() {
            assert!(path.starts_with(&self.parent) && path != self.parent);
            let _ = fs::remove_dir_all(path);
        }
    }
}
fn config() -> TraceImportConfig {
    TraceImportConfig {
        source_label: "fabricated precision control".into(),
        time_origin_utc_s: 1788825600.,
        time_window_utc_s: [1788825600., 1788912000.],
        bounds: Some([-1., -1., 1., 1.]),
        ..Default::default()
    }
}
fn report(aircraft: usize, count: usize) -> TraceImportReport {
    let cfg = config();
    let id = format!("{aircraft:06x}");
    let source_points: Vec<_> = (0..count)
        .map(|n| TracePointSource {
            row_index: n,
            timestamp_utc_s: cfg.time_origin_utc_s + n as f64 * 0.1,
            latitude_deg: 0.,
            longitude_deg: 0.,
            ellipsoid_height_m: 1000.,
            altitude_basis: "geometric_slot".into(),
            altitude_uncertainty_m: None,
            altitude_main_ft: Some(3000.),
            altitude_geom_ft: Some(3280.839895013123),
            flags: 0,
            position_source: "adsb_icao".into(),
            ground_speed_knots: None,
            track_deg: None,
            vertical_rate_fpm: None,
            geometric_vertical_rate_fpm: None,
            indicated_airspeed_knots: None,
            roll_deg: None,
        })
        .collect();
    let points = (0..count)
        .map(|n| TrackPoint {
            t_s: n as f64 * 0.1,
            xyz_m: [
                6379137.0 + n as f64 * 0.000123456789,
                0.001 * n as f64,
                -0.25,
            ],
        })
        .collect();
    TraceImportReport {
        schema_version: 1,
        provider: "fabricated".into(),
        config: cfg.clone(),
        coordinate_frame: "ECEF".into(),
        timestamp_definition: "UTC origin+seconds".into(),
        location_verified: false,
        quality_notes: vec![],
        sources: vec![TraceSource {
            icao24: id.clone(),
            source_timestamp_utc_s: cfg.time_origin_utc_s,
            registration: None,
            aircraft_type: None,
            readsb_version: None,
            selected_by_region: true,
            source_rows: count,
            position_source_counts: BTreeMap::new(),
        }],
        stats: TraceImportStats {
            source_traces: 1,
            source_rows: count,
            selected_traces: 1,
            output_passes: 1,
            output_points: count,
            ..Default::default()
        },
        passes: vec![TracePass {
            track: Track {
                flight_id: format!("{id}#part0"),
                points,
            },
            icao24: id,
            source_label: cfg.source_label,
            source_timestamp_utc_s: cfg.time_origin_utc_s,
            source_points,
        }],
    }
}

#[test]
fn disk_blocks_preserve_exact_points_and_continuity_across_partition_edges() {
    let f = Fixture::new();
    let original = report(1, POINTS_PER_BLOCK + 3);
    let expected = original.passes[0].track.clone();
    let cancel = AtomicBool::new(false);
    let mut writer = TrackStoreWriter::new(
        &f.root,
        &config(),
        json!({"fixture":true}),
        64 * 1024 * 1024,
    )
    .unwrap();
    writer
        .push(original, json!({"source_sha256":"a".repeat(64)}), &cancel)
        .unwrap();
    writer.finish(&cancel).unwrap();
    let store = TrackStore::open(&f.root.join("index.json")).unwrap();
    assert_eq!(store.passes()[0].blocks.len(), 2);
    let actual = store.load_track(0).unwrap();
    assert_eq!(actual.flight_id, expected.flight_id);
    assert_eq!(actual.points.len(), expected.points.len());
    for (a, b) in actual.points.iter().zip(&expected.points) {
        assert_eq!(a.t_s.to_bits(), b.t_s.to_bits());
        for axis in 0..3 {
            assert_eq!(a.xyz_m[axis].to_bits(), b.xyz_m[axis].to_bits());
        }
    }
    let provenance = &store.passes()[0].blocks[1].provenance;
    let data = packed::range_bytes(
        &f.root.join(&provenance.path),
        provenance.offset,
        provenance.bytes,
    )
    .unwrap();
    let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(data));
    let raw: Value = serde_json::from_reader(decoder).unwrap();
    assert_eq!(raw["source_points"][0]["row_index"], POINTS_PER_BLOCK);
    assert_eq!(raw["first_sample"], POINTS_PER_BLOCK);
    store.verify_unchanged().unwrap();
}

#[test]
fn changed_points_provenance_and_shrunken_bounds_are_rejected() {
    let f = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut writer = TrackStoreWriter::new(&f.root, &config(), json!({}), 4 * 1024 * 1024).unwrap();
    writer.push(report(1, 3), json!({}), &cancel).unwrap();
    writer.finish(&cancel).unwrap();
    let path = f.root.join("index.json");
    let original = fs::read(&path).unwrap();
    let store = TrackStore::open(&path).unwrap();
    let block = f.root.join(&store.passes()[0].blocks[0].path);
    let native = fs::read(&block).unwrap();
    let mut changed = native.clone();
    changed[8] ^= 1;
    fs::write(&block, changed).unwrap();
    assert!(store.load_track(0).is_err());
    assert!(TrackStore::open(&path).is_err());
    fs::write(&block, native).unwrap();
    let mut manifest: Value = serde_json::from_slice(&original).unwrap();
    let page_path = f
        .root
        .join(manifest["metadata_pages"][0]["path"].as_str().unwrap());
    let page_original = fs::read(&page_path).unwrap();
    let mut page: Value = serde_json::from_slice(&page_original).unwrap();
    page[0]["ecef_max"][0] = page[0]["ecef_min"][0].clone();
    let changed_page = serde_json::to_vec(&page).unwrap();
    manifest["stored_bytes"] = json!(
        manifest["stored_bytes"].as_u64().unwrap() - page_original.len() as u64
            + changed_page.len() as u64
    );
    manifest["metadata_pages"][0]["bytes"] = json!(changed_page.len());
    manifest["metadata_pages"][0]["sha256"] = json!(digest(&changed_page));
    fs::write(&page_path, changed_page).unwrap();
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(TrackStore::open(&path).err().unwrap().contains("bounds"));
    fs::write(&page_path, page_original).unwrap();
    fs::write(&path, &original).unwrap();
    let provenance = f.root.join(&store.passes()[0].blocks[0].provenance.path);
    let mut data = fs::read(&provenance).unwrap();
    data[0] ^= 1;
    fs::write(&provenance, data).unwrap();
    assert!(store.verify_unchanged().is_err());
}

#[test]
fn legacy_v1_store_remains_readable_and_exact_under_memory_budget() {
    let f = Fixture::new();
    let cancel = AtomicBool::new(false);
    let expected = report(1, 17).passes.remove(0).track;
    let mut writer =
        LegacyTrackStoreWriter::new(&f.root, &config(), json!({"legacy":true}), 4 * 1024 * 1024)
            .unwrap();
    writer.push(report(1, 17), json!({}), &cancel).unwrap();
    writer.finish(&cancel).unwrap();
    let path = f.root.join("index.json");
    let original = fs::read(&path).unwrap();
    let store = TrackStore::open_with_memory_limit(&path, 64 * 1024 * 1024, || Ok(())).unwrap();
    assert_eq!(
        serde_json::to_vec(&store.load_track(0).unwrap()).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    assert!(store.resident_memory_bytes() < 64 * 1024 * 1024);
    store.verify_unchanged().unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(is_memory_limit_error(
        &TrackStore::open_with_memory_limit(&path, 1, || Ok(()))
            .err()
            .unwrap()
    ));
}

#[test]
fn completed_index_growth_is_rejected_before_final_verification_reads_it() {
    let f = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut writer = TrackStoreWriter::new(&f.root, &config(), json!({}), 4 * 1024 * 1024).unwrap();
    writer.push(report(1, 3), json!({}), &cancel).unwrap();
    writer.finish(&cancel).unwrap();
    let path = f.root.join("index.json");
    let store = TrackStore::open(&path).unwrap();
    let mut changed = fs::read(&path).unwrap();
    changed.push(b' ');
    fs::write(&path, changed).unwrap();
    assert_eq!(
        store.verify_unchanged().unwrap_err(),
        "Track store file exceeds byte limit"
    );
}

#[test]
fn growing_index_cannot_bypass_open_memory_preflight_or_exact_read_limit() {
    let f = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut writer = TrackStoreWriter::new(&f.root, &config(), json!({}), 4 * 1024 * 1024).unwrap();
    writer.push(report(1, 3), json!({}), &cancel).unwrap();
    writer.finish(&cancel).unwrap();
    let path = f.root.join("index.json");
    let original = fs::read(&path).unwrap();
    let mut visits = 0;
    let error = TrackStore::open_with_checkpoint(&path, || {
        visits += 1;
        if visits == 2 {
            let mut changed = original.clone();
            changed.push(b' ');
            fs::write(&path, changed).unwrap();
        }
        Ok(())
    })
    .err()
    .unwrap();
    assert_eq!(error, "Track store file exceeds byte limit");
    fs::write(&path, &original).unwrap();
    let mut visits = 0;
    let error = bounded_bytes_with_checkpoint(&path, original.len() as u64, &mut || {
        visits += 1;
        if visits == 2 {
            let mut changed = original.clone();
            changed.push(b' ');
            fs::write(&path, changed).unwrap();
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error, "Track store file grew beyond its byte limit");
}

#[test]
fn disk_limit_and_cancel_never_publish_completed_index() {
    let f = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut writer = TrackStoreWriter::new(&f.root, &config(), json!({}), 1024 * 1024).unwrap();
    assert!(
        writer
            .push(report(1, POINTS_PER_BLOCK), json!({}), &cancel)
            .is_err()
    );
    assert!(!f.root.join("index.json").exists());
    let g = Fixture::new();
    let mut writer = TrackStoreWriter::new(&g.root, &config(), json!({}), 1024 * 1024).unwrap();
    writer.push(report(1, 3), json!({}), &cancel).unwrap();
    cancel.store(true, Ordering::Relaxed);
    assert!(writer.finish(&cancel).is_err());
    assert!(!g.root.join("index.json").exists());
}

#[test]
#[ignore = "Explicit bounded large-store control:2.1million points streamed in60000-point reports; no provider or original recording"]
fn indexed_store_accepts_more_than_two_million_points_without_truncation() {
    let f = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut writer = TrackStoreWriter::new(
        &f.root,
        &config(),
        json!({"synthetic":true}),
        512 * 1024 * 1024,
    )
    .unwrap();
    for aircraft in 0..35 {
        writer
            .push(report(aircraft, 60000), json!({"member":aircraft}), &cancel)
            .unwrap();
    }
    writer.finish(&cancel).unwrap();
    let store = TrackStore::open(&f.root.join("index.json")).unwrap();
    assert_eq!(store.total_points(), 2_100_000);
    assert_eq!(store.passes().len(), 35);
    for i in 0..35 {
        let track = store.load_track(i).unwrap();
        assert_eq!(track.points.len(), 60000);
        assert_eq!(track.points[59999].t_s, 59_999.0 * 0.1);
    }
    store.verify_unchanged().unwrap();
}

#[test]
fn checkpointed_open_stops_before_access_during_validation_and_before_success() {
    let f = Fixture::new();
    let stopped = "fixture requested stop";
    assert_eq!(
        TrackStore::open_with_checkpoint(&f.root.join("absent.json"), || Err(stopped.into()))
            .err()
            .unwrap(),
        stopped
    );
    let mut writer = TrackStoreWriter::new(&f.root, &config(), json!({}), 4 * 1024 * 1024).unwrap();
    let cancel = AtomicBool::new(false);
    writer.push(report(1, 4097), json!({}), &cancel).unwrap();
    writer.finish(&cancel).unwrap();
    let path = f.root.join("index.json");
    let before = fs::read(&path).unwrap();
    let mut checkpoints = 0;
    let store = TrackStore::open_with_checkpoint(&path, || {
        checkpoints += 1;
        Ok(())
    })
    .unwrap();
    assert!(
        checkpoints > 20,
        "Verification must be interruptible within its reads and decoding"
    );
    for stop_at in [1, checkpoints / 2, checkpoints] {
        let mut visited = 0;
        let error = TrackStore::open_with_checkpoint(&path, || {
            visited += 1;
            if visited == stop_at {
                Err(stopped.into())
            } else {
                Ok(())
            }
        })
        .err()
        .unwrap();
        assert_eq!(error, stopped);
        assert_eq!(visited, stop_at);
    }
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(store.total_points(), 4097);
    store.verify_unchanged().unwrap();
}

#[test]
fn checkpointed_final_verification_preserves_stop_and_tamper_errors() {
    let f = Fixture::new();
    let mut writer = TrackStoreWriter::new(&f.root, &config(), json!({}), 4 * 1024 * 1024).unwrap();
    let cancel = AtomicBool::new(false);
    writer.push(report(1, 3), json!({}), &cancel).unwrap();
    writer.push(report(2, 3), json!({}), &cancel).unwrap();
    writer.finish(&cancel).unwrap();
    let store = TrackStore::open(&f.root.join("index.json")).unwrap();
    let mut checkpoints = 0;
    store
        .verify_unchanged_with_checkpoint(|| {
            checkpoints += 1;
            Ok(())
        })
        .unwrap();
    assert!(checkpoints > 10);
    for stop_at in [1, checkpoints / 2, checkpoints] {
        let mut visited = 0;
        let error = store
            .verify_unchanged_with_checkpoint(|| {
                visited += 1;
                if visited == stop_at {
                    Err("fixture final scan stopped".into())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert_eq!(error, "fixture final scan stopped");
        assert_eq!(visited, stop_at);
    }
    let provenance = f.root.join(&store.passes()[1].blocks[0].provenance.path);
    let mut data = fs::read(&provenance).unwrap();
    data[0] ^= 1;
    fs::write(&provenance, data).unwrap();
    assert_eq!(
        store
            .verify_unchanged_with_checkpoint(|| Ok(()))
            .unwrap_err(),
        "Track store content changed"
    );
}

#[test]
fn bounded_read_and_hash_honor_mid_file_checkpoints() {
    let f = Fixture::new();
    let data = vec![7u8; 2 * 1024 * 1024];
    let path = f.root.join("bounded.bin");
    fs::write(&path, &data).unwrap();
    let mut reads = 0;
    let error = bounded_bytes_with_checkpoint(&path, data.len() as u64, &mut || {
        reads += 1;
        if reads == 4 {
            Err("fixture read stopped".into())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert_eq!(error, "fixture read stopped");
    assert_eq!(reads, 4);
    let mut chunks = 0;
    let error = digest_with_checkpoint(&data, &mut || {
        chunks += 1;
        if chunks == 2 {
            Err("fixture hash stopped".into())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert_eq!(error, "fixture hash stopped");
    assert_eq!(chunks, 2);
    assert_eq!(bounded_bytes(&path, data.len() as u64).unwrap(), data);
    assert_eq!(
        digest_with_checkpoint(&data, &mut || Ok(())).unwrap(),
        digest(&data)
    );
}
