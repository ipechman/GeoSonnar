//! Small fabricated packed-storage controls; no providers or recordings.
use super::*;

const TEST_MEMORY: u64 = 64 * 1024 * 1024;

fn packed_fixture(count: usize, points_per_shard: u64) -> Fixture {
    let fixture = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut writer = TrackStoreWriter::new_with_memory_limit(
        &fixture.root,
        &config(),
        json!({"synthetic":true}),
        4 * 1024 * 1024,
        TEST_MEMORY,
    )
    .unwrap();
    writer.test_limits(points_per_shard, 4096, 4096, TEST_MEMORY);
    // Append order deliberately differs from the public lexical pass order.
    for aircraft in (0..count).rev() {
        writer
            .push(report(aircraft, 9), json!({"member":aircraft}), &cancel)
            .unwrap();
    }
    writer.finish(&cancel).unwrap();
    fixture
}

fn read_index(fixture: &Fixture) -> Value {
    serde_json::from_slice(&fs::read(fixture.root.join("index.json")).unwrap()).unwrap()
}

fn assert_exact_track(actual: &Track, expected: &Track) {
    assert_eq!(actual.flight_id, expected.flight_id);
    assert_eq!(actual.points.len(), expected.points.len());
    for (actual, expected) in actual.points.iter().zip(&expected.points) {
        assert_eq!(actual.t_s.to_bits(), expected.t_s.to_bits());
        for axis in 0..3 {
            assert_eq!(actual.xyz_m[axis].to_bits(), expected.xyz_m[axis].to_bits());
        }
    }
}

fn source_range(fixture: &Fixture, range: &StoreFile) -> Value {
    let whole = fs::read(fixture.root.join(&range.path)).unwrap();
    let start = range.offset as usize;
    let end = start + range.bytes as usize;
    let bytes = &whole[start..end];
    assert_eq!(digest(bytes), range.sha256);
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}

#[test]
fn packed_short_passes_cross_all_storage_boundaries_without_precision_or_provenance_loss() {
    const PASSES: usize = 64;
    let fixture = packed_fixture(PASSES, 4096);
    let index = read_index(&fixture);
    assert_eq!(index["schema_version"], 2);
    let mut registered_files = 1;
    for kind in ["metadata_pages", "point_shards", "provenance_shards"] {
        let rows = index[kind].as_array().unwrap();
        assert!(rows.len() > 1, "fixture did not cross {kind} boundaries");
        registered_files += rows.len();
    }
    assert!(
        registered_files < PASSES,
        "packing recreated a per-pass file inventory"
    );
    assert_eq!(
        fs::read_dir(&fixture.root).unwrap().count(),
        registered_files
    );
    let store = TrackStore::open(&fixture.root.join("index.json")).unwrap();
    assert_eq!(store.total_points(), (PASSES * 9) as u64);
    assert_eq!(store.passes().len(), PASSES);
    assert!(
        store
            .passes()
            .iter()
            .flat_map(|pass| &pass.blocks)
            .any(|block| block.offset > 0)
    );
    assert!(
        store
            .passes()
            .iter()
            .flat_map(|pass| &pass.blocks)
            .any(|block| block.provenance.offset > 0)
    );
    for (aircraft, metadata) in store.passes().iter().enumerate() {
        let expected = report(aircraft, 9);
        let expected = &expected.passes[0];
        assert_exact_track(&store.load_track(aircraft).unwrap(), &expected.track);
        let mut rows = 0;
        for block in &metadata.blocks {
            let raw = source_range(&fixture, &block.provenance);
            assert_eq!(raw["pass_id"], metadata.id);
            assert_eq!(raw["first_sample"], block.first_sample);
            assert_eq!(raw["source"]["archive_member"]["member"], aircraft);
            assert_eq!(
                raw["source_points"],
                serde_json::to_value(
                    &expected.source_points
                        [block.first_sample..block.first_sample + block.point_count]
                )
                .unwrap()
            );
            rows += block.point_count;
        }
        assert_eq!(rows, 9);
    }
    store.verify_unchanged().unwrap();
}

#[test]
fn packed_native_range_seam_does_not_split_one_continuous_pass() {
    let fixture = packed_fixture(2, 256);
    let store = TrackStore::open(&fixture.root.join("index.json")).unwrap();
    assert_eq!(store.passes().len(), 2);
    for (aircraft, metadata) in store.passes().iter().enumerate() {
        assert_eq!(metadata.blocks.len(), 2);
        assert_eq!(metadata.blocks[0].point_count, 8);
        assert_eq!(metadata.blocks[1].point_count, 1);
        assert_eq!(metadata.blocks[1].first_sample, 8);
        assert_ne!(metadata.blocks[0].path, metadata.blocks[1].path);
        assert_exact_track(
            &store.load_track(aircraft).unwrap(),
            &report(aircraft, 9).passes[0].track,
        );
        assert_eq!(
            source_range(&fixture, &metadata.blocks[1].provenance)["source_points"][0]["row_index"],
            8
        );
    }
}

fn rewrite_first_page(fixture: &Fixture, change: impl FnOnce(&mut Value, &mut Value)) {
    let mut index = read_index(fixture);
    let relative = index["metadata_pages"][0]["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = fixture.root.join(relative);
    let original = fs::read(&path).unwrap();
    let mut page: Value = serde_json::from_slice(&original).unwrap();
    change(&mut page, &mut index);
    let changed = serde_json::to_vec(&page).unwrap();
    let stored = index["stored_bytes"].as_u64().unwrap();
    index["stored_bytes"] = json!(stored - original.len() as u64 + changed.len() as u64);
    index["metadata_pages"][0]["bytes"] = json!(changed.len());
    index["metadata_pages"][0]["sha256"] = json!(digest(&changed));
    fs::write(path, changed).unwrap();
    fs::write(
        fixture.root.join("index.json"),
        serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
}

#[test]
fn rehashed_packed_metadata_cannot_hide_overlaps_or_missing_ranges() {
    for provenance in [false, true] {
        let fixture = packed_fixture(3, 4096);
        TrackStore::open(&fixture.root.join("index.json")).unwrap();
        rewrite_first_page(&fixture, |page, _| {
            assert!(page.as_array().unwrap().len() >= 2);
            let second = &mut page[1]["blocks"][0];
            if provenance {
                assert!(second["provenance"]["offset"].as_u64().unwrap() > 0);
                second["provenance"]["offset"] = json!(0);
            } else {
                assert!(second["offset"].as_u64().unwrap() > 0);
                second["offset"] = json!(0);
            }
        });
        let error = TrackStore::open(&fixture.root.join("index.json"))
            .err()
            .unwrap();
        assert!(
            error.contains("overlap"),
            "expected structural range refusal: {error}"
        );
    }
    let fixture = packed_fixture(3, 4096);
    rewrite_first_page(&fixture, |page, index| {
        assert_eq!(page.as_array().unwrap().len(), 3);
        page.as_array_mut().unwrap().pop();
        index["pass_count"] = json!(2);
        index["block_count"] = json!(2);
        index["stats"]["output_passes"] = json!(2);
        index["stats"]["output_points"] = json!(18);
    });
    let error = TrackStore::open(&fixture.root.join("index.json"))
        .err()
        .unwrap();
    assert!(
        error.contains("missing or unreferenced ranges"),
        "expected complete-inventory refusal: {error}"
    );
}

#[test]
fn packed_writer_memory_refusal_and_cancel_never_publish_completed_index() {
    let fixture = Fixture::new();
    let mut writer = TrackStoreWriter::new_with_memory_limit(
        &fixture.root,
        &config(),
        json!({}),
        4 * 1024 * 1024,
        TEST_MEMORY,
    )
    .unwrap();
    // Test-only low ceiling exercises refusal without allocating large data.
    writer.test_limits(4096, 4096, 4096, 1);
    let error = writer
        .push(report(1, 9), json!({}), &AtomicBool::new(false))
        .unwrap_err();
    assert!(is_memory_limit_error(&error), "{error}");
    assert!(!fixture.root.join("index.json").exists());
    let cancelled = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut writer = TrackStoreWriter::new_with_memory_limit(
        &cancelled.root,
        &config(),
        json!({}),
        4 * 1024 * 1024,
        TEST_MEMORY,
    )
    .unwrap();
    writer.test_limits(256, 4096, 4096, TEST_MEMORY);
    writer.push(report(1, 9), json!({}), &cancel).unwrap();
    cancel.store(true, Ordering::Relaxed);
    assert!(writer.finish(&cancel).unwrap_err().contains("cancelled"));
    assert!(!cancelled.root.join("index.json").exists());
}

#[test]
fn buffered_provenance_preserves_exact_json_across_buffer_boundaries_and_tail() {
    let fixture = Fixture::new();
    let original = report(7, 513);
    let pass = &original.passes[0];
    let member = json!({"member":"fabricated-buffer-boundary"});
    let source = json!({"archive_member":member,"trace_source":original.sources[0],
        "provider":original.provider,"coordinate_frame":original.coordinate_frame,
        "timestamp_definition":original.timestamp_definition,"quality_notes":original.quality_notes,
        "icao24":pass.icao24,"source_label":pass.source_label,"source_timestamp_utc_s":pass.source_timestamp_utc_s});
    let expected = serde_json::to_vec(&StoredBlock {
        schema_version: 1,
        pass_id: &pass.track.flight_id,
        first_sample: 0,
        source_points: &pass.source_points,
        source: &source,
    })
    .unwrap();
    assert!(expected.len() > 2 * 64 * 1024);
    assert_ne!(expected.len() % (64 * 1024), 0);
    let expected_track = pass.track.clone();
    let cancel = AtomicBool::new(false);
    let mut writer = TrackStoreWriter::new_with_memory_limit(
        &fixture.root,
        &config(),
        json!({"synthetic":true}),
        4 * 1024 * 1024,
        TEST_MEMORY,
    )
    .unwrap();
    writer.push(original, member, &cancel).unwrap();
    writer.finish(&cancel).unwrap();
    let store = TrackStore::open(&fixture.root.join("index.json")).unwrap();
    assert_exact_track(&store.load_track(0).unwrap(), &expected_track);
    assert_eq!(store.passes()[0].blocks.len(), 1);
    let range = &store.passes()[0].blocks[0].provenance;
    let compressed =
        packed::range_bytes(&fixture.root.join(&range.path), range.offset, range.bytes).unwrap();
    assert_eq!(digest(&compressed), range.sha256);
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, expected);
    store.verify_unchanged().unwrap();
}

#[test]
fn buffered_provenance_final_drain_failure_never_publishes_completed_index() {
    let fixture = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut writer = TrackStoreWriter::new_with_memory_limit(
        &fixture.root,
        &config(),
        json!({}),
        4 * 1024 * 1024,
        TEST_MEMORY,
    )
    .unwrap();
    // This small JSON fits in the new buffer. Its final drain cannot even write
    // the gzip header into the one-byte target, and must propagate that error.
    writer.test_limits(4096, 1, 4096, TEST_MEMORY);
    let error = writer.push(report(1, 3), json!({}), &cancel).unwrap_err();
    assert!(
        error.contains("Packed provenance range exceeds shard limit"),
        "{error}"
    );
    assert!(!fixture.root.join("index.json").exists());
}
