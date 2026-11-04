use geosonnar::{
    adsblol_acquisition,
    data_catalog::{self, DataKind, ImportRequest},
    detailed_data::sha256,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn catalog_verification_cancels_between_hash_chunks_without_changing_owned_files() {
    let f = Fixture::new();
    let payload = f.root.join("large-source.bin");
    fs::write(&payload, vec![0x5a; 3 * 1024 * 1024]).unwrap();
    let mut request = f.request();
    request.max_bytes = 4 * 1024 * 1024;
    let entry = data_catalog::import_file(&f.case, &payload, &request).unwrap();
    let mut count = 0;
    let expected = data_catalog::verified_files_with_checkpoint(&f.case, &entry.id, || {
        count += 1;
        Ok(())
    })
    .unwrap();
    assert!(count >= 6);
    for stop in [1, count / 2, count] {
        let mut seen = 0;
        let error = data_catalog::verified_files_with_checkpoint(&f.case, &entry.id, || {
            seen += 1;
            if seen == stop {
                Err("requested stop".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error, "requested stop");
        assert_eq!(seen, stop);
    }
    assert!(
        data_catalog::verified_files_with_cancel(&f.case, &entry.id, &AtomicBool::new(true))
            .unwrap_err()
            .contains("cancelled")
    );
    assert_eq!(
        data_catalog::verified_files(&f.case, &entry.id).unwrap(),
        expected
    );
    assert_eq!(sha256(&expected[0]).unwrap(), sha256(&payload).unwrap());
}

struct Fixture {
    root: PathBuf,
    case: PathBuf,
    source: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("output/catalog-controls")
            .join(format!(
                "{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source é.json");
        fs::write(&source, b"{\"fabricated\":true}\n").unwrap();
        let case = root.join("case.json");
        fs::write(&case,serde_json::to_vec(&json!({"schema_version":1,"case_id":"fabricated","title":"Catalog control","created_unix_s":1,"local_only":true,"recording":{"path":"source é.json","sha256":sha256(&source).unwrap(),"bytes":fs::metadata(&source).unwrap().len(),"media":{"duration_s":1.,"sample_rate_hz":48000,"channels":1,"audio_codec":"fabricated"}}})).unwrap()).unwrap();
        Self { root, case, source }
    }
    fn request(&self) -> ImportRequest {
        ImportRequest {
            label: "Fabricated export".into(),
            kind: DataKind::Other,
            date_utc: Some("2026-05-12".into()),
            bounds: Some([40., -75., 50., -65.]),
            notes: "Unit control; no recording or location evidence".into(),
            max_bytes: 1024,
        }
    }
}

#[test]
fn copied_data_verifies_deletes_only_owned_bytes_and_retains_provenance() {
    let f = Fixture::new();
    let before = sha256(&f.source).unwrap();
    let e = data_catalog::import_file(&f.case, &f.source, &f.request()).unwrap();
    let paths = data_catalog::verified_files(&f.case, &e.id).unwrap();
    assert_eq!(paths.len(), 1);
    assert_ne!(paths[0], f.source);
    assert_eq!(fs::read(&paths[0]).unwrap(), fs::read(&f.source).unwrap());
    let listed = data_catalog::list(&f.case).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].integrity_verified);
    assert_eq!(listed[0].status, "available");
    let deleted = data_catalog::delete_owned(&f.case, &e.id).unwrap();
    assert!(deleted.originals_preserved);
    assert_eq!(deleted.deleted_bytes, e.files[0].bytes);
    assert!(!paths[0].exists());
    assert_eq!(sha256(&f.source).unwrap(), before);
    assert_eq!(data_catalog::list(&f.case).unwrap()[0].status, "deleted");
    assert!(data_catalog::verified_files(&f.case, &e.id).is_err());
    assert_eq!(
        data_catalog::delete_owned(&f.case, &e.id)
            .unwrap()
            .deleted_files,
        1
    );
    assert!(
        f.root
            .join("managed-data")
            .join(&e.id)
            .join("entry.json")
            .exists()
    );
}

fn fabricated_archive(
    f: &Fixture,
    corrupt: bool,
) -> (
    data_catalog::CatalogEntry,
    geosonnar::adsblol::TraceImportConfig,
    PathBuf,
) {
    use std::io::Write;
    const ORIGIN: f64 = 1_788_825_600.;
    let rows: Vec<_> = (0..4)
        .map(|n| {
            json!([
                f64::from(n) * 10.,
                0.,
                f64::from(n) * 0.001,
                1000.,
                100.,
                90.,
                0,
                0.,
                null,
                "adsb_icao",
                1100.,
                0.,
                null,
                null
            ])
        })
        .collect();
    let raw =
        serde_json::to_vec(&json!({"icao":"abc123","timestamp":ORIGIN,"trace":rows})).unwrap();
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&raw).unwrap();
    let bytes = gzip.finish().unwrap();
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(bytes.len() as u64);
    header.set_cksum();
    tar.append_data(
        &mut header,
        "traces/23/trace_full_abc123.json",
        bytes.as_slice(),
    )
    .unwrap();
    tar.finish().unwrap();
    let original = f.root.join("original.tar");
    fs::write(
        &original,
        if corrupt {
            b"corrupt".to_vec()
        } else {
            tar.into_inner().unwrap()
        },
    )
    .unwrap();
    let mut request = f.request();
    request.kind = DataKind::Adsb;
    request.max_bytes = 1_000_000;
    let mut item = data_catalog::import_file(&f.case, &original, &request).unwrap();
    let directory = f.root.join("managed-data").join(&item.id);
    let acquired = adsblol_acquisition::ArchiveManifest {
        schema_version: 1,
        provider: "Fabricated ADSB.lol archive control".into(),
        license: "ODbL-1.0".into(),
        date_utc: "2026-09-08".into(),
        release_tag: "fixture".into(),
        plan_sha256: "0".repeat(64),
        parts: vec![adsblol_acquisition::ArchivePart {
            path: item.primary_file.clone(),
            name: "fabricated.tar".into(),
            bytes: item.files[0].bytes,
            sha256: item.files[0].sha256.clone(),
        }],
        complete: true,
        full_sha_verified: true,
        coverage_complete: false,
    };
    let manifest = directory.join("acquisition.json");
    fs::write(&manifest, serde_json::to_vec(&acquired).unwrap()).unwrap();
    item.files.push(data_catalog::CatalogFile {
        relative_path: "acquisition.json".into(),
        bytes: fs::metadata(&manifest).unwrap().len(),
        sha256: sha256(&manifest).unwrap(),
    });
    item.primary_file = "acquisition.json".into();
    item.format = "adsblol_archive".into();
    fs::write(
        directory.join("entry.json"),
        serde_json::to_vec(&item).unwrap(),
    )
    .unwrap();
    let config = geosonnar::adsblol::TraceImportConfig {
        source_label: "Fabricated bounded archive control".into(),
        time_origin_utc_s: ORIGIN,
        time_window_utc_s: [ORIGIN, ORIGIN + 40.],
        bounds: Some([-1., -1., 1., 1.]),
        ..Default::default()
    };
    (item, config, original)
}

#[test]
fn owned_archive_import_produces_real_bounded_tracks_and_preserves_source() {
    let f = Fixture::new();
    let (archive, config, original) = fabricated_archive(&f, false);
    let before = sha256(&original).unwrap();
    let item = data_catalog::import_adsblol_archive(&f.case, &archive.id, &config).unwrap();
    assert_eq!(item.format, "normalized_tracks");
    assert_eq!(item.source["source_catalog_id"], archive.id);
    assert!(!item.source["location_verified"].as_bool().unwrap());
    let paths = data_catalog::verified_files(&f.case, &item.id).unwrap();
    let map = geosonnar::map_data::load(
        paths
            .iter()
            .find(|p| p.ends_with("native/normalized.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(map.time_origin_utc_s, Some(config.time_origin_utc_s));
    assert_eq!(map.tracks.len(), 1);
    assert_eq!(map.tracks[0].points.len(), 4);
    assert_eq!(
        map.tracks[0].points[0].time_utc_s,
        Some(config.time_origin_utc_s)
    );
    assert!((map.tracks[0].points[0].height_m.unwrap() - 1100. * 0.3048).abs() < 1e-6);
    data_catalog::delete_owned(&f.case, &item.id).unwrap();
    data_catalog::verified_files(&f.case, &archive.id).unwrap();
    assert_eq!(sha256(&original).unwrap(), before);
}

#[test]
fn managed_indexed_archive_registers_all_lossless_blocks_and_deletes_only_its_copy() {
    use std::sync::atomic::AtomicBool;
    let f = Fixture::new();
    let (archive, config, original) = fabricated_archive(&f, false);
    let before = sha256(&original).unwrap();
    let item = data_catalog::import_adsblol_indexed_with_limits(
        &f.case,
        &archive.id,
        &config,
        64 * 1024 * 1024,
        128 * 1024 * 1024,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(item.format, "indexed_tracks");
    assert_eq!(item.source["index_memory_budget_bytes"], 128 * 1024 * 1024);
    let files = data_catalog::verified_files(&f.case, &item.id).unwrap();
    let index = files
        .iter()
        .find(|p| p.ends_with("native/index.json"))
        .unwrap();
    let store = geosonnar::track_store::TrackStore::open(index).unwrap();
    assert_eq!(store.total_points(), 4);
    assert_eq!(store.load_track(0).unwrap().points.len(), 4);
    for pass in store.passes() {
        for block in &pass.blocks {
            assert!(files.iter().any(|p| p.ends_with(&block.path)));
            assert!(files.iter().any(|p| p.ends_with(&block.provenance.path)));
        }
    }
    drop(store);
    data_catalog::delete_owned(&f.case, &item.id).unwrap();
    data_catalog::verified_files(&f.case, &archive.id).unwrap();
    assert_eq!(sha256(&original).unwrap(), before);
}

#[test]
fn archive_import_requires_day_region_and_registered_source_parts() {
    let f = Fixture::new();
    let (archive, mut config, _) = fabricated_archive(&f, false);
    let count = data_catalog::list(&f.case).unwrap().len();
    assert!(
        data_catalog::import_adsblol_archive_with_cancel(
            &f.case,
            &archive.id,
            &config,
            &AtomicBool::new(true)
        )
        .is_err()
    );
    config.bounds = None;
    assert!(data_catalog::import_adsblol_archive(&f.case, &archive.id, &config).is_err());
    config.bounds = Some([-1., -1., 1., 1.]);
    config.time_origin_utc_s += 1.;
    assert!(data_catalog::import_adsblol_archive(&f.case, &archive.id, &config).is_err());
    config.time_origin_utc_s -= 1.;
    config.time_window_utc_s[1] += 86_400.;
    assert!(data_catalog::import_adsblol_archive(&f.case, &archive.id, &config).is_err());
    assert_eq!(data_catalog::list(&f.case).unwrap().len(), count);
    config.time_window_utc_s[1] -= 86_400.;
    let directory = f.root.join("managed-data").join(&archive.id);
    let path = directory.join("acquisition.json");
    let mut acquired: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    acquired["parts"][0]["path"] = json!(f.source.canonicalize().unwrap());
    fs::write(&path, serde_json::to_vec(&acquired).unwrap()).unwrap();
    let mut malicious = archive;
    let registered = malicious
        .files
        .iter_mut()
        .find(|p| p.relative_path == std::path::Path::new("acquisition.json"))
        .unwrap();
    registered.bytes = fs::metadata(&path).unwrap().len();
    registered.sha256 = sha256(&path).unwrap();
    fs::write(
        directory.join("entry.json"),
        serde_json::to_vec(&malicious).unwrap(),
    )
    .unwrap();
    assert!(
        data_catalog::import_adsblol_archive(&f.case, &malicious.id, &config)
            .unwrap_err()
            .contains("outside the managed item")
    );
}

#[test]
fn corrupt_archive_is_registered_as_unusable_and_deletable() {
    let f = Fixture::new();
    let (archive, config, original) = fabricated_archive(&f, true);
    let error = data_catalog::import_adsblol_archive(&f.case, &archive.id, &config).unwrap_err();
    assert!(error.contains("incomplete managed import"));
    let items = data_catalog::list(&f.case).unwrap();
    let failed = items
        .iter()
        .find(|i| i.entry.format == "incomplete_import")
        .unwrap();
    assert!(data_catalog::verified_files(&f.case, &failed.entry.id).is_err());
    data_catalog::delete_owned(&f.case, &failed.entry.id).unwrap();
    assert!(original.exists());
}
#[test]
fn changed_owned_data_fails_verification_and_deletion() {
    let f = Fixture::new();
    let e = data_catalog::import_file(&f.case, &f.source, &f.request()).unwrap();
    let p = data_catalog::verified_files(&f.case, &e.id)
        .unwrap()
        .remove(0);
    let mut bytes = fs::read(&p).unwrap();
    bytes[0] = b'X';
    fs::write(&p, bytes).unwrap();
    assert!(data_catalog::verified_files(&f.case, &e.id).is_err());
    assert!(data_catalog::delete_owned(&f.case, &e.id).is_err());
    assert!(p.exists());
    assert!(f.source.exists());
}
#[test]
fn metadata_cannot_redirect_deletion_to_parent_paths_or_windows_streams() {
    for relative in [
        "../source é.json",
        "payload.json:stream",
        "C:\\outside.json",
    ] {
        let f = Fixture::new();
        let e = data_catalog::import_file(&f.case, &f.source, &f.request()).unwrap();
        let entry = f.root.join("managed-data").join(&e.id).join("entry.json");
        let mut v: Value = serde_json::from_slice(&fs::read(&entry).unwrap()).unwrap();
        v["files"][0]["relative_path"] = relative.into();
        v["primary_file"] = relative.into();
        fs::write(entry, serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(data_catalog::delete_owned(&f.case, &e.id).is_err());
        assert!(f.source.exists());
    }
}
#[test]
fn wrong_case_registration_and_invalid_ids_fail_closed() {
    let f = Fixture::new();
    let e = data_catalog::import_file(&f.case, &f.source, &f.request()).unwrap();
    assert!(data_catalog::verified_files(&f.case, "../outside").is_err());
    let mut case: Value = serde_json::from_slice(&fs::read(&f.case).unwrap()).unwrap();
    case["case_id"] = "different".into();
    fs::write(&f.case, serde_json::to_vec(&case).unwrap()).unwrap();
    assert!(data_catalog::list(&f.case).is_err());
    assert!(data_catalog::delete_owned(&f.case, &e.id).is_err());
    assert!(f.source.exists());
}
#[test]
fn import_limit_and_pre_cancellation_create_no_managed_copy() {
    let f = Fixture::new();
    let mut request = f.request();
    request.max_bytes = 1;
    assert!(data_catalog::import_file(&f.case, &f.source, &request).is_err());
    assert!(!f.root.join("managed-data").exists());
    assert!(
        data_catalog::import_file_with_cancel(
            &f.case,
            &f.source,
            &f.request(),
            &AtomicBool::new(true)
        )
        .is_err()
    );
    assert!(!f.root.join("managed-data").exists());
    assert!(
        adsblol_acquisition::fetch_with_cancel(
            &f.root.join("absent-plan"),
            &f.root.join("absent-cache"),
            &AtomicBool::new(true)
        )
        .unwrap_err()
        .contains("canceled")
    );
    assert!(!f.root.join("absent-cache").exists());
}
#[test]
fn unauthorized_protocol_credentials_and_pre_canceled_https_do_not_start_network() {
    let f = Fixture::new();
    for url in [
        "http://example.invalid/file.json",
        "file:///C:/private",
        "https://user:password@example.invalid/export",
        "https://example.invalid/\nfile",
    ] {
        assert!(
            data_catalog::download_url_with_cancel(
                &f.case,
                url,
                &f.request(),
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
    assert!(
        data_catalog::download_url_with_cancel(
            &f.case,
            "https://example.invalid/export?secret=not-sent",
            &f.request(),
            &AtomicBool::new(true)
        )
        .is_err()
    );
    assert!(!f.root.join("managed-data").exists());
}
#[test]
fn an_active_item_lock_prevents_deletion() {
    let f = Fixture::new();
    let e = data_catalog::import_file(&f.case, &f.source, &f.request()).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.root.join("managed-data").join(&e.id).join(".busy"))
        .unwrap();
    lock.try_lock().unwrap();
    assert!(
        data_catalog::delete_owned(&f.case, &e.id)
            .unwrap_err()
            .contains("busy")
    );
    drop(lock);
    data_catalog::delete_owned(&f.case, &e.id).unwrap();
    assert!(f.source.exists());
}
#[cfg(windows)]
#[test]
fn windows_junction_cannot_redirect_catalog_deletion() {
    use std::os::windows::process::CommandExt;
    let f = Fixture::new();
    let e = data_catalog::import_file(&f.case, &f.source, &f.request()).unwrap();
    let dir = f.root.join("managed-data").join(&e.id);
    let outside = f.root.join("outside");
    fs::create_dir(&outside).unwrap();
    let protected = outside.join("protected.json");
    fs::write(&protected, b"protected original").unwrap();
    let link = dir.join("linked");
    let result = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(link.to_string_lossy().replace('/', "\\"))
        .arg(outside.to_string_lossy().replace('/', "\\"))
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let record = dir.join("entry.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    value["files"][0]["relative_path"] = "linked/protected.json".into();
    value["primary_file"] = "linked/protected.json".into();
    value["files"][0]["bytes"] = 18.into();
    value["files"][0]["sha256"] = sha256(&protected).unwrap().into();
    fs::write(record, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(
        data_catalog::delete_owned(&f.case, &e.id)
            .unwrap_err()
            .contains("reparse")
    );
    assert_eq!(fs::read(&protected).unwrap(), b"protected original");
    // Remove only the junction itself, never recurse into its target.
    assert!(link.starts_with(&f.root));
    fs::remove_dir(link).unwrap();
}
