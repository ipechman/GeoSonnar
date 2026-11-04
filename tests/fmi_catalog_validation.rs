//! Provider acquisition state machine with small frozen public replies and no network.
use geosonnar::{
    data_catalog::{self, fmi},
    detailed_data::sha256,
    evidence_score::{self, LightningAltitudeDatum, LightningProvenance},
    fmi_lightning::FmiRequest,
};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const XML: &str = include_str!("fixtures/fmi/nordlis-2025-07-19-minute.xml");
const EMPTY: &str = "<f:FeatureCollection xmlns:f=\"http://www.opengis.net/wfs/2.0\" numberMatched=\"0\" numberReturned=\"0\"/>";
struct Fixture {
    root: PathBuf,
    manifest: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("output/fmi-controls")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        let recording = root.join("fabricated.json");
        fs::write(&recording, b"{}\n").unwrap();
        let manifest = root.join("case.json");
        fs::write(&manifest,serde_json::to_vec(&json!({"schema_version":1,"case_id":"fmi-control","title":"FMI offline control","created_unix_s":1,"local_only":true,"recording":{"path":"fabricated.json","sha256":sha256(&recording).unwrap(),"bytes":3,"media":{"duration_s":1.,"sample_rate_hz":48000,"channels":1,"audio_codec":"fabricated"}}})).unwrap()).unwrap();
        Self { root, manifest }
    }
    fn path(&self, id: &str) -> PathBuf {
        self.root.join("managed-data").join(id)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn request() -> FmiRequest {
    FmiRequest {
        start_utc: "2025-07-19T16:00:00Z".into(),
        end_utc: "2025-07-19T16:01:00Z".into(),
        bounds: [65., 13., 66., 15.],
        max_total_bytes: 64 * 1024 * 1024,
    }
}
fn success() -> fmi::TransportResult {
    fmi::TransportResult {
        http_status: Some(200),
        error: None,
    }
}

fn assert_library_status(fixture: &Fixture, id: &str, expected: &str) {
    let item = data_catalog::list(&fixture.manifest)
        .unwrap()
        .into_iter()
        .find(|item| item.entry.id == id)
        .unwrap();
    assert_eq!(item.status, expected);
    assert!(item.problem.is_none(), "{:?}", item.problem);
    assert!(
        !item.integrity_verified,
        "Status does not verify raw responses"
    );
}

#[test]
fn explicit_plan_fetch_import_reload_filter_and_owned_delete_preserve_source_semantics() {
    let f = Fixture::new();
    let before = fs::read(&f.manifest).unwrap();
    let planned = fmi::plan(&f.manifest, &request()).unwrap();
    let id = &planned.entry.id;
    assert!(planned.report.is_none());
    assert_eq!(planned.entry.files.len(), 1);
    assert_library_status(&f, id, "planned");
    let done = fmi::fetch_with_transport(
        &f.manifest,
        id,
        &AtomicBool::new(false),
        |_| {},
        |_, limit, path, _| {
            // The live worker owns progress. Listing keeps the last published
            // state rather than interpreting its generic lock as completion.
            assert_library_status(&f, id, "planned");
            assert!(limit >= XML.len() as u64);
            fs::write(path, XML).unwrap();
            Ok(success())
        },
    )
    .unwrap();
    let report = done.report.as_ref().unwrap();
    assert_eq!(report.status, fmi::FmiStatus::Complete);
    assert_eq!(report.owned_rows, 36);
    assert_eq!(report.attempts, 1);
    assert!(!report.source_coverage_complete);
    assert_eq!(report.transferred_bytes, XML.len() as u64);
    assert_library_status(&f, id, "complete");
    let cached = fmi::fetch_with_transport(
        &f.manifest,
        id,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("cached request must not access network"),
    )
    .unwrap();
    assert_eq!(
        cached.report.unwrap().transferred_bytes,
        report.transferred_bytes
    );
    let reopened = fmi::load(&f.manifest, id).unwrap();
    assert_eq!(reopened.report.unwrap().owned_rows, 36);
    let imported = fmi::import(&f.manifest, id, &AtomicBool::new(false)).unwrap();
    assert_library_status(&f, &imported.id, "imported");
    assert_library_status(&f, id, "complete");
    let path = f.path(&imported.id).join(&imported.primary_file);
    let generic = LightningProvenance {
        provider: "generic caller".into(),
        source: "generic reload".into(),
        license: None,
        notes: None,
        altitude_datum: LightningAltitudeDatum::Wgs84Ellipsoid,
        source_metadata: None,
    };
    let catalog = evidence_score::import_lightning(&path, &generic).unwrap();
    assert_eq!(catalog.strikes.len(), 36);
    assert!(catalog.provenance.source_metadata.is_some());
    assert_eq!(
        catalog.provenance.altitude_datum,
        LightningAltitudeDatum::Unknown
    );
    assert!(
        catalog
            .provenance
            .license
            .as_ref()
            .unwrap()
            .contains("CC BY")
    );
    assert!(
        catalog
            .strikes
            .iter()
            .all(|s| s.source_record.is_some() && s.altitude_m.is_none())
    );
    let filtered = data_catalog::requests::filter_lightning_export(
        &f.manifest,
        &imported.id,
        &data_catalog::requests::LightningQuery {
            provider: "operator selection".into(),
            start_utc: request().start_utc,
            end_utc: request().end_utc,
            bounds: request().bounds,
        },
        "reviewed subset",
        &AtomicBool::new(false),
    )
    .unwrap();
    let filtered_catalog = evidence_score::import_lightning(
        &f.path(&filtered.id).join(&filtered.primary_file),
        &generic,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&filtered_catalog.strikes).unwrap(),
        serde_json::to_value(&catalog.strikes).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&filtered_catalog.provenance).unwrap(),
        serde_json::to_value(&catalog.provenance).unwrap()
    );
    let deleted = data_catalog::delete_owned(&f.manifest, id).unwrap();
    assert!(deleted.originals_preserved);
    assert!(fmi::load(&f.manifest, id).is_err());
    assert!(path.is_file());
    assert_eq!(fs::read(&f.manifest).unwrap(), before);
}

#[test]
fn partial_failure_stops_and_resumes_only_missing_chunks() {
    let f = Fixture::new();
    let mut req = request();
    req.end_utc = "2025-07-19T16:02:00Z".into();
    let item = fmi::plan(&f.manifest, &req).unwrap();
    let mut calls = 0;
    let partial = fmi::fetch_with_transport(
        &f.manifest,
        &item.entry.id,
        &AtomicBool::new(false),
        |_| {},
        |_, _, path, _| {
            calls += 1;
            fs::write(path, if calls == 1 { XML } else { "service error" }).unwrap();
            Ok(if calls == 1 {
                success()
            } else {
                fmi::TransportResult {
                    http_status: Some(500),
                    error: Some("provider unavailable".into()),
                }
            })
        },
    )
    .unwrap();
    assert_eq!(calls, 2);
    assert_eq!(partial.report.as_ref().unwrap().chunks_complete, 1);
    assert_eq!(
        partial.report.as_ref().unwrap().status,
        fmi::FmiStatus::Partial
    );
    assert_library_status(&f, &item.entry.id, "partial");
    assert!(fmi::import(&f.manifest, &item.entry.id, &AtomicBool::new(false)).is_err());
    let mut resumed_calls = 0;
    let complete = fmi::fetch_with_transport(
        &f.manifest,
        &item.entry.id,
        &AtomicBool::new(false),
        |_| {},
        |url, _, path, _| {
            resumed_calls += 1;
            assert!(url.contains("starttime=2025-07-19T16:01:00Z"));
            fs::write(path, EMPTY).unwrap();
            Ok(success())
        },
    )
    .unwrap();
    assert_eq!(resumed_calls, 1);
    let report = complete.report.unwrap();
    assert_eq!(report.status, fmi::FmiStatus::Complete);
    assert_eq!(report.attempts, 3);
    assert_eq!(report.owned_rows, 36);
    assert_eq!(report.errors.len(), 1);
    assert_library_status(&f, &item.entry.id, "complete");
    assert_eq!(
        report.transferred_bytes,
        (XML.len() + "service error".len() + EMPTY.len()) as u64
    );
}

#[test]
fn cancellation_prevents_network_or_import_and_retains_received_attempts() {
    let f = Fixture::new();
    let item = fmi::plan(&f.manifest, &request()).unwrap();
    let cancelled = fmi::fetch_with_transport(
        &f.manifest,
        &item.entry.id,
        &AtomicBool::new(true),
        |_| {},
        |_, _, _, _| panic!("pre-cancelled"),
    )
    .unwrap();
    assert_eq!(cancelled.report.unwrap().status, fmi::FmiStatus::Cancelled);
    assert_library_status(&f, &item.entry.id, "cancelled");
    let flag = AtomicBool::new(false);
    let stopped = fmi::fetch_with_transport(
        &f.manifest,
        &item.entry.id,
        &flag,
        |_| {},
        |_, _, path, cancel| {
            fs::write(path, "partial XML").unwrap();
            cancel.store(true, Ordering::Relaxed);
            Ok(fmi::TransportResult {
                http_status: Some(200),
                error: Some("cancelled".into()),
            })
        },
    )
    .unwrap();
    assert_eq!(
        stopped.report.as_ref().unwrap().status,
        fmi::FmiStatus::Cancelled
    );
    assert_eq!(stopped.report.unwrap().transferred_bytes, 11);
    assert_library_status(&f, &item.entry.id, "cancelled");
    assert!(fmi::import(&f.manifest, &item.entry.id, &AtomicBool::new(true)).is_err());
    assert_eq!(
        fmi::load(&f.manifest, &item.entry.id)
            .unwrap()
            .report
            .unwrap()
            .status,
        fmi::FmiStatus::Cancelled
    );
}

#[test]
fn failures_and_byte_overshoot_are_charged_and_exhausted_cache_never_retries() {
    let f = Fixture::new();
    let mut req = request();
    req.max_total_bytes = 128;
    let item = fmi::plan(&f.manifest, &req).unwrap();
    let result = fmi::fetch_with_transport(
        &f.manifest,
        &item.entry.id,
        &AtomicBool::new(false),
        |_| {},
        |_, limit, path, _| {
            assert_eq!(limit, 128);
            fs::write(path, vec![b'x'; 129]).unwrap();
            Ok(success())
        },
    )
    .unwrap();
    assert_eq!(
        result.report.as_ref().unwrap().status,
        fmi::FmiStatus::Partial
    );
    assert_eq!(result.report.unwrap().transferred_bytes, 129);
    assert_library_status(&f, &item.entry.id, "failed");
    let resumed = fmi::fetch_with_transport(
        &f.manifest,
        &item.entry.id,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("budget exhausted"),
    )
    .unwrap();
    assert_eq!(resumed.report.unwrap().attempts, 1);
}

#[test]
fn empty_complete_and_http200_exception_are_not_loadable_lightning_layers() {
    for (body, complete) in [
        (EMPTY, true),
        (
            "<ows:ExceptionReport xmlns:ows=\"http://www.opengis.net/ows/1.1\"/>",
            false,
        ),
    ] {
        let f = Fixture::new();
        let item = fmi::plan(&f.manifest, &request()).unwrap();
        let result = fmi::fetch_with_transport(
            &f.manifest,
            &item.entry.id,
            &AtomicBool::new(false),
            |_| {},
            |_, _, path, _| {
                fs::write(path, body).unwrap();
                Ok(success())
            },
        )
        .unwrap();
        assert_eq!(
            result.report.as_ref().unwrap().status == fmi::FmiStatus::Complete,
            complete
        );
        assert_eq!(result.report.unwrap().owned_rows, 0);
        assert_library_status(
            &f,
            &item.entry.id,
            if complete { "complete_empty" } else { "failed" },
        );
        assert!(fmi::import(&f.manifest, &item.entry.id, &AtomicBool::new(false)).is_err());
        assert_eq!(data_catalog::list(&f.manifest).unwrap().len(), 1);
    }
}

#[test]
fn provider_throttling_stops_and_enforces_a_cooldown_before_manual_resume() {
    let f = Fixture::new();
    let item = fmi::plan(&f.manifest, &request()).unwrap();
    let first = fmi::fetch_with_transport(
        &f.manifest,
        &item.entry.id,
        &AtomicBool::new(false),
        |_| {},
        |_, _, path, _| {
            fs::write(path, "retry later").unwrap();
            Ok(fmi::TransportResult {
                http_status: Some(429),
                error: None,
            })
        },
    )
    .unwrap();
    assert_eq!(first.report.unwrap().attempts, 1);
    let second = fmi::fetch_with_transport(
        &f.manifest,
        &item.entry.id,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("cooldown"),
    )
    .unwrap();
    let report = second.report.unwrap();
    assert_eq!(report.attempts, 1);
    assert!(report.errors.iter().any(|e| e.contains("one minute")));
}

#[test]
fn corrupted_or_unknown_cache_files_are_not_reused_or_adopted() {
    for altered in [true, false] {
        let f = Fixture::new();
        let item = fmi::plan(&f.manifest, &request()).unwrap();
        fmi::fetch_with_transport(
            &f.manifest,
            &item.entry.id,
            &AtomicBool::new(false),
            |_| {},
            |_, _, path, _| {
                fs::write(path, XML).unwrap();
                Ok(success())
            },
        )
        .unwrap();
        fs::write(
            f.path(&item.entry.id).join(if altered {
                "response-000001.xml"
            } else {
                "unrelated-user-file.txt"
            }),
            b"do not replace",
        )
        .unwrap();
        assert!(fmi::load(&f.manifest, &item.entry.id).is_err());
        assert!(
            fmi::fetch_with_transport(
                &f.manifest,
                &item.entry.id,
                &AtomicBool::new(false),
                |_| {},
                |_, _, _, _| panic!("corrupt cache")
            )
            .is_err()
        );
    }
}

#[test]
fn unfinished_attempt_bytes_survive_restart_and_resume_without_becoming_success() {
    let f = Fixture::new();
    let item = fmi::plan(&f.manifest, &request()).unwrap();
    let dir = f.path(&item.entry.id);
    fs::write(dir.join("request-000001.json"),serde_json::to_vec(&json!({"sequence":1,"plan_sha256":geosonnar::fmi_lightning::plan_sha256(&item.plan).unwrap(),"chunk_index":0,"url":item.plan.chunks[0].url,"byte_limit":item.plan.max_response_bytes})).unwrap()).unwrap();
    fs::write(dir.join("response-000001.xml"), b"interrupted").unwrap();
    let reopened = fmi::load(&f.manifest, &item.entry.id).unwrap();
    assert_eq!(reopened.report.as_ref().unwrap().chunks_complete, 0);
    assert_eq!(reopened.report.unwrap().transferred_bytes, 11);
    let resumed = fmi::fetch_with_transport(
        &f.manifest,
        &item.entry.id,
        &AtomicBool::new(false),
        |_| {},
        |_, _, path, _| {
            fs::write(path, XML).unwrap();
            Ok(success())
        },
    )
    .unwrap();
    assert_eq!(resumed.report.as_ref().unwrap().attempts, 2);
    assert_eq!(
        resumed.report.unwrap().transferred_bytes,
        11 + XML.len() as u64
    );
    assert_eq!(
        fs::read(dir.join("response-000001.xml")).unwrap(),
        b"interrupted"
    );
}

#[test]
fn a_held_item_lock_prevents_download_import_and_deletion() {
    let f = Fixture::new();
    let item = fmi::plan(&f.manifest, &request()).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.path(&item.entry.id).join(".busy"))
        .unwrap();
    lock.try_lock().unwrap();
    assert_library_status(&f, &item.entry.id, "planned");
    assert!(
        fmi::fetch_with_transport(
            &f.manifest,
            &item.entry.id,
            &AtomicBool::new(false),
            |_| {},
            |_, _, _, _| panic!("locked")
        )
        .is_err()
    );
    assert!(fmi::import(&f.manifest, &item.entry.id, &AtomicBool::new(false)).is_err());
    assert!(data_catalog::delete_owned(&f.manifest, &item.entry.id).is_err());
}

#[test]
fn a_same_length_changed_fmi_report_is_unavailable_in_the_library() {
    let f = Fixture::new();
    let planned = fmi::plan(&f.manifest, &request()).unwrap();
    let done = fmi::fetch_with_transport(
        &f.manifest,
        &planned.entry.id,
        &AtomicBool::new(false),
        |_| {},
        |_, _, path, _| {
            fs::write(path, XML).unwrap();
            Ok(success())
        },
    )
    .unwrap();
    let path = f.path(&done.entry.id).join(&done.entry.primary_file);
    let mut bytes = fs::read(&path).unwrap();
    let position = bytes
        .windows(8)
        .position(|value| value == b"complete")
        .unwrap();
    bytes[position] = b'X';
    fs::write(path, bytes).unwrap();
    let listed = data_catalog::list(&f.manifest).unwrap();
    assert_eq!(listed[0].status, "unavailable");
    assert!(
        listed[0]
            .problem
            .as_ref()
            .unwrap()
            .contains("report changed")
    );
    assert!(!listed[0].integrity_verified);
}

#[test]
fn unknown_or_inconsistent_registered_fmi_metadata_never_claims_available() {
    // Start from a real provider-fixture download, then retain a matching
    // metadata hash to isolate schema/counter checks from checksum rejection.
    for scenario in 0..6 {
        let f = Fixture::new();
        let planned = fmi::plan(&f.manifest, &request()).unwrap();
        let done = fmi::fetch_with_transport(
            &f.manifest,
            &planned.entry.id,
            &AtomicBool::new(false),
            |_| {},
            |_, _, path, _| {
                fs::write(path, XML).unwrap();
                Ok(success())
            },
        )
        .unwrap();
        let mut entry = done.entry;
        let path = f.path(&entry.id).join(&entry.primary_file);
        let mut report: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match scenario {
            0 => {
                report["status"] = "future_status".into();
                entry.source["status"] = "future_status".into();
            }
            1 => report["chunks_complete"] = 0.into(),
            2 => entry.source["status"] = "partial".into(),
            3 => entry.format = "fmi_lightning_future".into(),
            4 => {}
            5 => {
                report["plan_sha256"] = "a".repeat(64).into();
                entry.source["plan_sha256"] = "a".repeat(64).into();
            }
            _ => unreachable!(),
        }
        let bytes = if scenario == 4 {
            vec![b' '; 1024 * 1024 + 1]
        } else {
            serde_json::to_vec_pretty(&report).unwrap()
        };
        fs::write(&path, &bytes).unwrap();
        let file = entry
            .files
            .iter_mut()
            .find(|file| file.relative_path == entry.primary_file)
            .unwrap();
        file.bytes = bytes.len() as u64;
        file.sha256 = sha256(&path).unwrap();
        fs::write(
            f.path(&entry.id).join("fmi-registration-00000001.json"),
            serde_json::to_vec_pretty(&entry).unwrap(),
        )
        .unwrap();
        let listed = data_catalog::list(&f.manifest).unwrap();
        assert_eq!(listed[0].status, "unavailable", "scenario {scenario}");
        assert!(listed[0].problem.is_some(), "scenario {scenario}");
        assert!(!listed[0].integrity_verified);
    }
}
