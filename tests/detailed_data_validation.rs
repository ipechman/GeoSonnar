//! Fabricated archive controls; no public/private recording or real flight label.
use flate2::{Compression, write::GzEncoder};
use geosonnar::{
    adsblol::TraceImportConfig,
    detailed_data::{
        ArchiveImportRequest, ArchivePart, PartReader, decode_member, import_archive,
        prepare_import, sha256, trace_member_name,
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

const ORIGIN: f64 = 1_788_825_600.0;
static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Workspace {
    path: PathBuf,
    permitted_root: PathBuf,
}
impl Workspace {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
        fs::create_dir_all(&root).unwrap();
        let permitted_root = root.canonicalize().unwrap();
        let path = permitted_root.join(format!(
            "archive-controls-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&path).unwrap();
        Self {
            path,
            permitted_root,
        }
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        if let Ok(actual) = self.path.canonicalize() {
            assert!(actual.starts_with(&self.permitted_root) && actual != self.permitted_root);
            let _ = fs::remove_dir_all(actual);
        }
    }
}

fn config() -> TraceImportConfig {
    TraceImportConfig {
        source_label: "fabricated verified daily archive".into(),
        time_origin_utc_s: ORIGIN,
        time_window_utc_s: [ORIGIN, ORIGIN + 86_400.0],
        bounds: Some([-1.0, -1.0, 1.0, 1.0]),
        ..TraceImportConfig::default()
    }
}

fn trace(hex: &str, latitude: f64) -> Vec<u8> {
    let rows: Vec<_> = (0..4)
        .map(|n| {
            json!([
                f64::from(n) * 10.0,
                latitude,
                f64::from(n) * 0.001,
                1000.0,
                100.0,
                90.0,
                0,
                0.0,
                null,
                "adsb_icao",
                1100.0,
                0.0,
                null,
                null
            ])
        })
        .collect();
    serde_json::to_vec(&json!({"icao":hex,"timestamp":ORIGIN,"trace":rows})).unwrap()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

struct Member {
    path: String,
    bytes: Vec<u8>,
    kind: tar::EntryType,
}
fn file(path: &str, bytes: Vec<u8>) -> Member {
    Member {
        path: path.into(),
        bytes,
        kind: tar::EntryType::Regular,
    }
}

fn archive(members: Vec<Member>) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for member in members {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_size(member.bytes.len() as u64);
        header.set_entry_type(member.kind);
        // Deliberately write raw paths, including hostile names that a safe
        // extraction builder normally rejects, to exercise the import boundary.
        assert!(member.path.len() < 100);
        header.as_mut_bytes()[..100].fill(0);
        header.as_mut_bytes()[..member.path.len()].copy_from_slice(member.path.as_bytes());
        header.set_cksum();
        builder.append(&header, member.bytes.as_slice()).unwrap();
    }
    builder.finish().unwrap();
    builder.into_inner().unwrap()
}

fn write_json(path: &Path, value: &impl serde::Serialize) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn setup(work: &Workspace, bytes: &[u8], split: usize) -> (PathBuf, PathBuf, ArchiveImportRequest) {
    assert!(0 < split && split < bytes.len());
    let mut parts = Vec::new();
    for (index, chunk) in [&bytes[..split], &bytes[split..]].into_iter().enumerate() {
        let path = PathBuf::from(format!("daily.tar.part-{index}"));
        let actual = work.path.join(&path);
        fs::write(&actual, chunk).unwrap();
        parts.push(ArchivePart {
            path,
            bytes: chunk.len() as u64,
            sha256: sha256(&actual).unwrap(),
        });
    }
    let manifest_path = work.path.join("acquisition.json");
    write_json(
        &manifest_path,
        &json!({"schema_version":1,"complete":true,"full_sha_verified":true,
        "date_utc":"2026-09-08","provider":"fabricated ADSB.lol archive", "parts":parts}),
    );
    let request = ArchiveImportRequest {
        parts,
        acquisition_manifest: "acquisition.json".into(),
        config: config(),
    };
    let request_path = work.path.join("import-request.json");
    write_json(&request_path, &request);
    (request_path, work.path.join("output"), request)
}

fn assert_failed(request: &Path, output: &Path) -> Value {
    assert!(import_archive(request, output).is_err());
    assert!(
        !output.join("receipt.json").exists(),
        "Failed imports cannot publish a success receipt"
    );
    let failure: Value =
        serde_json::from_slice(&fs::read(output.join("failure.json")).unwrap()).unwrap();
    assert_eq!(failure["status"], "failed");
    assert_eq!(failure["location_verified"], false);
    assert_eq!(failure["archive_import_complete"], false);
    failure
}

#[test]
fn indexed_archive_preserves_legacy_native_points_and_cancel_never_publishes() {
    use geosonnar::{
        detailed_data::{import_archive_indexed_with_cancel, import_archive_indexed_with_limits},
        track_store::TrackStore,
    };
    use std::sync::atomic::AtomicBool;
    let work = Workspace::new();
    let bytes = archive(vec![file(
        "traces/23/trace_full_abc123.json",
        gzip(&trace("abc123", 0.)),
    )]);
    let (request, legacy, _) = setup(&work, &bytes, 101);
    import_archive(&request, &legacy).unwrap();
    let indexed = work.path.join("indexed");
    let result = import_archive_indexed_with_limits(
        &request,
        &indexed,
        8 * 1024 * 1024,
        128 * 1024 * 1024,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result["archive_import_complete"], true);
    let plan: Value =
        serde_json::from_slice(&fs::read(indexed.join("plan.json")).unwrap()).unwrap();
    assert_eq!(plan["limits"]["index_memory_bytes"], 128 * 1024 * 1024);
    assert!(!indexed.join("normalized.json").exists());
    let old: Value =
        serde_json::from_slice(&fs::read(legacy.join("normalized.json")).unwrap()).unwrap();
    let store = TrackStore::open(&indexed.join("index.json")).unwrap();
    assert_eq!(store.passes().len(), 1);
    assert_eq!(
        serde_json::to_value(store.load_track(0).unwrap()).unwrap(),
        old["passes"][0]["track"]
    );
    let cancelled = work.path.join("cancelled");
    assert!(
        import_archive_indexed_with_cancel(
            &request,
            &cancelled,
            8 * 1024 * 1024,
            &AtomicBool::new(true)
        )
        .is_err()
    );
    assert!(!cancelled.join("index.json").exists());
    let invalid_memory = work.path.join("invalid-memory");
    assert!(
        import_archive_indexed_with_limits(
            &work.path.join("missing-request.json"),
            &invalid_memory,
            8 * 1024 * 1024,
            1,
            &AtomicBool::new(false),
        )
        .unwrap_err()
        .contains("memory")
    );
    assert!(
        !invalid_memory.exists(),
        "invalid budget must be rejected before creating a partial import"
    );
}

#[test]
fn two_verified_parts_cross_tar_header_and_compressed_body_without_losing_points() {
    let source = gzip(&trace("abc123", 0.0));
    let bytes = archive(vec![
        file("traces/23/trace_full_abc123.json", source.clone()),
        file(
            "traces/56/trace_full_abc456.json",
            gzip(&trace("abc456", 40.0)),
        ),
    ]);
    for split in [101, 512 + source.len() / 2] {
        let work = Workspace::new();
        let (request, output, settings) = setup(&work, &bytes, split);
        let mut joined = PartReader::new(
            settings
                .parts
                .iter()
                .map(|p| work.path.join(&p.path))
                .collect(),
        );
        assert_eq!(joined.read(&mut []).unwrap(), 0);
        let mut decoded = Vec::new();
        joined.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, bytes);
        let result = import_archive(&request, &output).unwrap();
        assert_eq!(result["archive_import_complete"], true);
        assert_eq!(result["real_world_coverage_complete"], false);
        assert_eq!(result["location_verified"], false);
        assert_eq!(result["traces_scanned"], 2);
        assert_eq!(result["stats"]["output_points"], 4);
        assert_eq!(result["selected_members"].as_array().unwrap().len(), 1);
        let normalized: Value =
            serde_json::from_slice(&fs::read(output.join("normalized.json")).unwrap()).unwrap();
        let points = normalized["passes"][0]["track"]["points"]
            .as_array()
            .unwrap();
        assert_eq!(
            points
                .iter()
                .map(|p| p["t_s"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            [0.0, 10.0, 20.0, 30.0]
        );
        for (index, point) in points.iter().enumerate() {
            let xyz = point["xyz_m"].as_array().unwrap();
            let radius = xyz[0].as_f64().unwrap().hypot(xyz[1].as_f64().unwrap());
            assert!((radius - 6_378_137.0 - 1100.0 * 0.3048).abs() < 1e-8);
            assert_eq!(
                normalized["passes"][0]["source_points"][index]["row_index"],
                index
            );
        }
        assert_eq!(
            fs::read(output.join("selected-traces/abc123.json.gz")).unwrap(),
            source
        );
        assert!(!output.join("selected-traces/abc456.json.gz").exists());
        assert!(
            !work.path.join("traces").exists(),
            "Global archive must be streamed, not unpacked"
        );
    }
}

#[test]
fn changed_missing_duplicate_and_reordered_parts_fail_with_receipts() {
    let bytes = archive(vec![file(
        "traces/23/trace_full_abc123.json",
        gzip(&trace("abc123", 0.0)),
    )]);
    for mode in ["changed", "missing", "duplicate", "reordered"] {
        let work = Workspace::new();
        let (request, output, mut settings) = setup(&work, &bytes, 100);
        match mode {
            "changed" => {
                let path = work.path.join(&settings.parts[0].path);
                let mut b = fs::read(&path).unwrap();
                b[0] ^= 1;
                fs::write(path, b).unwrap();
            }
            "missing" => {
                fs::remove_file(work.path.join(&settings.parts[1].path)).unwrap();
            }
            "duplicate" => {
                settings.parts[1] = settings.parts[0].clone();
                write_json(&request, &settings);
            }
            "reordered" => {
                settings.parts.reverse();
                write_json(&request, &settings);
            }
            _ => unreachable!(),
        }
        assert_failed(&request, &output);
    }
}

#[test]
fn manifest_completeness_and_date_are_actual_preconditions() {
    let bytes = archive(vec![file(
        "traces/23/trace_full_abc123.json",
        gzip(&trace("abc123", 0.0)),
    )]);
    for (key, value) in [
        ("schema_version", json!(2)),
        ("complete", json!(false)),
        ("full_sha_verified", json!(false)),
        ("date_utc", json!("2026-09-09")),
        ("date_utc", json!("2026-02-30")),
    ] {
        let work = Workspace::new();
        let (request, output, _) = setup(&work, &bytes, 100);
        let path = work.path.join("acquisition.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest[key] = value;
        write_json(&path, &manifest);
        assert_failed(&request, &output);
    }
}

#[test]
fn unsafe_or_nonregular_trace_members_fail_without_extraction() {
    for member in [
        file("../outside.json", b"{}".to_vec()),
        file("/absolute.json", b"{}".to_vec()),
        Member {
            path: "traces/23/trace_full_abc123.json".into(),
            bytes: vec![],
            kind: tar::EntryType::Symlink,
        },
        Member {
            path: "traces/23/trace_full_abc123.json".into(),
            bytes: vec![],
            kind: tar::EntryType::Directory,
        },
    ] {
        let bytes = archive(vec![member]);
        let work = Workspace::new();
        let (request, output, _) = setup(&work, &bytes, 100);
        assert_failed(&request, &output);
        assert!(!work.permitted_root.join("outside.json").exists());
    }
}

#[test]
fn malformed_traces_identity_mismatch_and_bad_gzip_fail_the_whole_import() {
    let mut broken = gzip(&trace("abc123", 0.0));
    let last = broken.len() - 5;
    broken[last] ^= 128;
    for contents in [
        b"{ malformed json".to_vec(),
        gzip(&trace("abc124", 0.0)),
        broken,
        vec![0x1f, 0x8b, 0x08, 0x00],
    ] {
        let bytes = archive(vec![file("traces/23/trace_full_abc123.json", contents)]);
        let work = Workspace::new();
        let (request, output, _) = setup(&work, &bytes, 100);
        assert_failed(&request, &output);
    }
}

#[test]
fn duplicate_aircraft_members_do_not_replace_first_retained_evidence() {
    let original = gzip(&trace("abc123", 0.0));
    let replacement = gzip(&trace("abc123", 0.1));
    let bytes = archive(vec![
        file("traces/23/trace_full_abc123.json", original.clone()),
        file("traces/23/trace_full_abc123.json", replacement),
    ]);
    let work = Workspace::new();
    let (request, output, _) = setup(&work, &bytes, 100);
    assert_failed(&request, &output);
    assert_eq!(
        fs::read(output.join("selected-traces/abc123.json.gz")).unwrap(),
        original
    );
}

#[test]
fn gzip_member_must_be_complete_and_exclusively_one_trace_stream() {
    let original = gzip(b"{\"trace\":[]}");
    assert_eq!(decode_member(&original).unwrap(), "{\"trace\":[]}");
    for suffix in [b"trailing garbage".to_vec(), gzip(b"{\"hidden\":true}")] {
        let mut bytes = original.clone();
        bytes.extend(suffix);
        assert!(decode_member(&bytes).is_err());
    }
    assert!(decode_member(&original[..original.len() - 1]).is_err());
}

fn terminated_trace_archive() -> (Vec<u8>, usize) {
    let payload = gzip(&trace("abc123", 0.0));
    let terminator_offset = 512 + payload.len().div_ceil(512) * 512;
    let bytes = archive(vec![file("traces/23/trace_full_abc123.json", payload)]);
    assert_eq!(bytes.len(), terminator_offset + 1024);
    assert!(bytes[terminator_offset..].iter().all(|byte| *byte == 0));
    (bytes, terminator_offset)
}

#[test]
fn tar_archive_without_a_terminator_fails_even_with_verified_complete_parts() {
    let (mut bytes, terminator_offset) = terminated_trace_archive();
    bytes.truncate(terminator_offset);
    let work = Workspace::new();
    let (request, output, _) = setup(&work, &bytes, 100);
    let failure = assert_failed(&request, &output);
    assert!(
        failure["error"]
            .as_str()
            .unwrap()
            .contains("two-block TAR terminator")
    );
    assert!(!output.join("normalized.json").exists());
}

#[test]
fn tar_archive_with_only_one_terminator_block_fails() {
    let (mut bytes, terminator_offset) = terminated_trace_archive();
    bytes.truncate(terminator_offset + 512);
    let work = Workspace::new();
    let (request, output, _) = setup(&work, &bytes, 100);
    let failure = assert_failed(&request, &output);
    assert!(
        failure["error"]
            .as_str()
            .unwrap()
            .contains("two-block TAR terminator")
    );
    assert!(!output.join("normalized.json").exists());
}

#[test]
fn tar_archive_rejects_nonzero_data_after_any_terminator_block() {
    let (original, terminator_offset) = terminated_trace_archive();
    for zero_blocks in [1, 2] {
        let mut bytes = original[..terminator_offset + zero_blocks * 512].to_vec();
        let mut trailing_block = [0; 512];
        trailing_block[511] = 42;
        bytes.extend(trailing_block);
        let work = Workspace::new();
        // The second archive part begins inside the terminator, so the padding
        // check must also consume data spanning an archive-part boundary.
        let (request, output, _) = setup(&work, &bytes, terminator_offset + 101);
        let failure = assert_failed(&request, &output);
        assert!(
            failure["error"]
                .as_str()
                .unwrap()
                .contains("nonzero data after its TAR terminator")
        );
        assert!(!output.join("normalized.json").exists());
    }
}

#[test]
fn tar_archive_rejects_partial_zero_padding_blocks() {
    let (original, _) = terminated_trace_archive();
    for size in [original.len() - 1, original.len() + 1] {
        let mut bytes = original.clone();
        bytes.resize(size, 0);
        let work = Workspace::new();
        let (request, output, _) = setup(&work, &bytes, 100);
        let failure = assert_failed(&request, &output);
        assert!(
            failure["error"]
                .as_str()
                .unwrap()
                .contains("complete sequence of TAR blocks")
        );
        assert!(!output.join("normalized.json").exists());
    }
}

#[test]
fn tar_archive_accepts_additional_complete_zero_padding_blocks() {
    let (mut bytes, terminator_offset) = terminated_trace_archive();
    bytes.resize(bytes.len() + 3 * 512, 0);
    let work = Workspace::new();
    let (request, output, _) = setup(&work, &bytes, terminator_offset + 101);
    let result = import_archive(&request, &output).unwrap();
    assert_eq!(result["archive_import_complete"], true);
    assert_eq!(result["stats"]["output_points"], 4);
    assert_eq!(result["traces_scanned"], 1);
    assert!(!output.join("failure.json").exists());
}

#[test]
fn plain_json_trace_keeps_its_format_and_original_bytes() {
    let source = trace("abc123", 0.0);
    let bytes = archive(vec![file(
        "traces/23/trace_full_abc123.json",
        source.clone(),
    )]);
    let work = Workspace::new();
    let (request, output, _) = setup(&work, &bytes, 100);
    let result = import_archive(&request, &output).unwrap();
    assert_eq!(result["archive_import_complete"], true);
    assert_eq!(
        result["selected_members"][0]["retained_file"],
        "selected-traces/abc123.json"
    );
    assert_eq!(
        fs::read(output.join("selected-traces/abc123.json")).unwrap(),
        source
    );
    assert!(!output.join("selected-traces/abc123.json.gz").exists());
}

#[test]
fn prepare_import_resolves_cache_paths_snapshots_settings_and_preserves_existing_requests() {
    let (bytes, _) = terminated_trace_archive();
    let work = Workspace::new();
    let (_, _, original_request) = setup(&work, &bytes, 100);
    let cache = work.path.join("download-cache");
    let case = work.path.join("separate-case");
    fs::create_dir_all(cache.join("parts")).unwrap();
    fs::create_dir(&case).unwrap();
    let acquired_parts: Vec<_> = original_request
        .parts
        .iter()
        .map(|part| {
            let relative = PathBuf::from("parts").join(&part.path);
            fs::copy(work.path.join(&part.path), cache.join(&relative)).unwrap();
            json!({"path":relative,"name":part.path.file_name().unwrap().to_str().unwrap(),
                "bytes":part.bytes,"sha256":part.sha256})
        })
        .collect();
    let manifest = json!({"schema_version":1,"provider":"fabricated ADSB.lol archive",
        "license":"fabricated test data", "date_utc":"2026-09-08",
        "release_tag":"synthetic-2026-09-08", "plan_sha256":"0".repeat(64),
        "parts":acquired_parts,"complete":true,"full_sha_verified":true,
        "coverage_complete":false});
    let manifest_path = cache.join("archive-manifest.json");
    write_json(&manifest_path, &manifest);
    let mut settings = config();
    settings.source_label = "distinct saved regional selection".into();
    settings.time_window_utc_s = [ORIGIN + 10.0, ORIGIN + 30.0];
    settings.bounds = Some([-0.5, -0.5, 0.5, 0.5]);
    settings.max_gap_s = 13.0;
    settings.max_speed_mps = 310.0;
    let config_path = work.path.join("settings.json");
    write_json(&config_path, &settings);
    let prepared_path = case.join("import-request.json");
    prepare_import(&manifest_path, &config_path, &prepared_path).unwrap();
    let prepared_bytes = fs::read(&prepared_path).unwrap();
    let prepared: ArchiveImportRequest = serde_json::from_slice(&prepared_bytes).unwrap();
    assert_eq!(
        prepared.acquisition_manifest,
        manifest_path.canonicalize().unwrap()
    );
    assert_eq!(
        serde_json::to_value(&prepared.config).unwrap(),
        serde_json::to_value(&settings).unwrap()
    );
    assert_eq!(prepared.parts.len(), original_request.parts.len());
    for (actual, original) in prepared.parts.iter().zip(&original_request.parts) {
        assert_eq!(
            actual.path,
            cache
                .join("parts")
                .join(&original.path)
                .canonicalize()
                .unwrap()
        );
        assert_eq!(actual.bytes, original.bytes);
        assert_eq!(actual.sha256, original.sha256);
    }

    // Later settings edits do not silently alter an already prepared case.
    settings.time_window_utc_s = [ORIGIN, ORIGIN + 5.0];
    write_json(&config_path, &settings);
    assert!(prepare_import(&manifest_path, &config_path, &prepared_path).is_err());
    assert_eq!(fs::read(&prepared_path).unwrap(), prepared_bytes);
    let imported = import_archive(&prepared_path, &case.join("imported")).unwrap();
    assert_eq!(imported["stats"]["output_points"], 3);

    for field in ["complete", "full_sha_verified"] {
        let mut incomplete = manifest.clone();
        incomplete[field] = json!(false);
        write_json(&manifest_path, &incomplete);
        let rejected = case.join(format!("rejected-{field}.json"));
        assert!(prepare_import(&manifest_path, &config_path, &rejected).is_err());
        assert!(!rejected.exists());
    }
}

#[test]
fn existing_output_is_preserved_without_even_writing_a_failure_receipt() {
    let bytes = archive(vec![file(
        "traces/23/trace_full_abc123.json",
        gzip(&trace("abc123", 0.0)),
    )]);
    let work = Workspace::new();
    let (request, output, _) = setup(&work, &bytes, 100);
    fs::create_dir(&output).unwrap();
    fs::write(output.join("keep.txt"), b"old case evidence").unwrap();
    assert!(import_archive(&request, &output).is_err());
    assert_eq!(
        fs::read(output.join("keep.txt")).unwrap(),
        b"old case evidence"
    );
    assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
}

#[test]
fn trace_names_require_safe_documented_shards_and_exact_aircraft_identifiers() {
    assert_eq!(
        trace_member_name(Path::new("traces/23/trace_full_abc123.json")).unwrap(),
        Some("abc123".into())
    );
    assert_eq!(
        trace_member_name(Path::new("./traces/23/trace_full_~abc123.json")).unwrap(),
        Some("~abc123".into())
    );
    assert_eq!(trace_member_name(Path::new("README.txt")).unwrap(), None);
    for name in [
        "../traces/23/trace_full_abc123.json",
        "traces/99/trace_full_abc123.json",
        "traces/23/trace_full_abc12z.json",
        "traces/23/trace_full_abc1234.json",
        "C:/traces/23/trace_full_abc123.json",
    ] {
        assert!(trace_member_name(Path::new(name)).is_err(), "{name}");
    }
}
