//! Crash-boundary acquisition recovery, integrity and owned deletion controls.
//! Every provider byte is a frozen public fixture; no network or GUI is used.
use geosonnar::{
    data_catalog::{self, fmi},
    detailed_data::sha256,
    fmi_lightning::{self, FmiRequest},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const XML: &str = include_str!("fixtures/fmi/nordlis-2025-07-19-minute.xml");

struct Fixture {
    root: PathBuf,
    manifest: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("output/fmi-recovery-controls")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        let recording = root.join("fabricated.json");
        fs::write(&recording, b"{}\n").unwrap();
        let manifest = root.join("case.json");
        fs::write(&manifest,serde_json::to_vec(&json!({"schema_version":1,"case_id":"fmi-recovery","title":"FMI offline recovery control","created_unix_s":1,"local_only":true,"recording":{"path":"fabricated.json","sha256":sha256(&recording).unwrap(),"bytes":3,"media":{"duration_s":1.,"sample_rate_hz":48000,"channels":1,"audio_codec":"fabricated"}}})).unwrap()).unwrap();
        Self { root, manifest }
    }
    fn directory(&self, id: &str) -> PathBuf {
        self.root.join("managed-data").join(id)
    }
    fn plan(&self) -> fmi::FmiItem {
        fmi::plan(
            &self.manifest,
            &FmiRequest {
                start_utc: "2025-07-19T16:00:00Z".into(),
                end_utc: "2025-07-19T16:01:00Z".into(),
                bounds: [65., 13., 66., 15.],
                max_total_bytes: fmi_lightning::MAX_TOTAL_BYTES,
            },
        )
        .unwrap()
    }
    fn interrupted(&self, item: &fmi::FmiItem) -> PathBuf {
        let directory = self.directory(&item.entry.id);
        let request = json!({"sequence":1,"plan_sha256":fmi_lightning::plan_sha256(&item.plan).unwrap(),"chunk_index":0,"url":item.plan.chunks[0].url,"byte_limit":item.plan.max_response_bytes});
        fs::write(
            directory.join("request-000001.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        fs::write(directory.join("response-000001.xml"), b"interrupted").unwrap();
        fs::write(directory.join("response-000001.status"), b"200").unwrap();
        directory
    }
    fn complete_then_lose_registration(&self) -> (fmi::FmiItem, PathBuf) {
        let item = self.plan();
        let done = fmi::fetch_with_transport(
            &self.manifest,
            &item.entry.id,
            &AtomicBool::new(false),
            |_| {},
            |_, _, path, _| {
                fs::write(path, XML).unwrap();
                Ok(fmi::TransportResult {
                    http_status: Some(200),
                    error: None,
                })
            },
        )
        .unwrap();
        assert_eq!(done.report.unwrap().status, fmi::FmiStatus::Complete);
        let directory = self.directory(&item.entry.id);
        // Simulate a process exit after durable report write and before the
        // registration publication. Every earlier completed byte remains.
        fs::remove_file(directory.join("fmi-registration-00000001.json")).unwrap();
        (item, directory)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn an_orphan_report_does_not_wedge_resume_or_cause_a_second_download() {
    let f = Fixture::new();
    let (item, directory) = f.complete_then_lose_registration();
    let report = directory.join("report-000001.json");
    let original_report = fs::read(&report).unwrap();
    let raw_hash = sha256(&directory.join("response-000001.xml")).unwrap();
    let loaded = fmi::load(&f.manifest, &item.entry.id).unwrap();
    assert_eq!(loaded.report.unwrap().status, fmi::FmiStatus::Complete);
    let resumed = fmi::fetch_with_transport(
        &f.manifest,
        &item.entry.id,
        &AtomicBool::new(false),
        |_| {},
        |_, _, _, _| panic!("complete raw chunk must be reused"),
    )
    .unwrap();
    let summary = resumed.report.as_ref().unwrap();
    assert_eq!(summary.status, fmi::FmiStatus::Complete);
    assert_eq!(summary.attempts, 1);
    assert_eq!(summary.transferred_bytes, XML.len() as u64);
    assert_eq!(summary.owned_rows, 36);
    assert_eq!(fs::read(&report).unwrap(), original_report);
    assert_eq!(
        sha256(&directory.join("response-000001.xml")).unwrap(),
        raw_hash
    );
    assert_eq!(resumed.entry.format, "fmi_lightning_download");
    assert!(directory.join("fmi-registration-00000001.json").is_file());
    assert_eq!(
        fmi::load(&f.manifest, &item.entry.id)
            .unwrap()
            .report
            .unwrap()
            .owned_rows,
        36
    );
    let imported = fmi::import(&f.manifest, &item.entry.id, &AtomicBool::new(false)).unwrap();
    assert!(
        f.directory(&imported.id)
            .join(imported.primary_file)
            .is_file()
    );
}

#[test]
fn unfinished_unregistered_response_bytes_are_included_in_owned_deletion() {
    let f = Fixture::new();
    let item = f.plan();
    let directory = f.interrupted(&item);
    let before = fs::read(&f.manifest).unwrap();
    let loaded = fmi::load(&f.manifest, &item.entry.id).unwrap();
    assert_eq!(loaded.report.as_ref().unwrap().chunks_complete, 0);
    assert_eq!(loaded.report.unwrap().transferred_bytes, 11);
    let deleted = data_catalog::delete_owned(&f.manifest, &item.entry.id).unwrap();
    for name in [
        "fmi-plan.json",
        "request-000001.json",
        "response-000001.xml",
        "response-000001.status",
    ] {
        assert!(
            !directory.join(name).exists(),
            "Owned interrupted file survived deletion: {name}"
        );
    }
    assert!(deleted.deleted_bytes >= 14 && deleted.originals_preserved);
    assert_eq!(fs::read(&f.manifest).unwrap(), before);
    assert_eq!(fs::read(f.root.join("fabricated.json")).unwrap(), b"{}\n");
    assert!(fmi::load(&f.manifest, &item.entry.id).is_err());
}

#[test]
fn completed_unregistered_attempt_and_orphan_report_can_be_deleted_together() {
    let f = Fixture::new();
    let (item, directory) = f.complete_then_lose_registration();
    let original = fs::read(f.root.join("fabricated.json")).unwrap();
    let deleted = data_catalog::delete_owned(&f.manifest, &item.entry.id).unwrap();
    for name in [
        "fmi-plan.json",
        "request-000001.json",
        "response-000001.xml",
        "attempt-000001.json",
        "report-000001.json",
    ] {
        assert!(
            !directory.join(name).exists(),
            "Owned pre-publication file survived: {name}"
        );
    }
    assert!(deleted.deleted_bytes >= XML.len() as u64);
    assert_eq!(fs::read(f.root.join("fabricated.json")).unwrap(), original);
}

#[test]
fn changed_orphan_report_is_not_adopted_for_resume_or_deletion() {
    let f = Fixture::new();
    let (item, directory) = f.complete_then_lose_registration();
    let report = directory.join("report-000001.json");
    let mut changed: Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    changed["owned_rows"] = json!(999);
    fs::write(&report, serde_json::to_vec(&changed).unwrap()).unwrap();
    let raw_before = fs::read(directory.join("response-000001.xml")).unwrap();
    assert!(
        fmi::fetch_with_transport(
            &f.manifest,
            &item.entry.id,
            &AtomicBool::new(false),
            |_| {},
            |_, _, _, _| panic!("changed orphan must stop before network")
        )
        .is_err()
    );
    assert!(data_catalog::delete_owned(&f.manifest, &item.entry.id).is_err());
    assert!(directory.join("fmi-plan.json").is_file());
    assert_eq!(
        fs::read(directory.join("response-000001.xml")).unwrap(),
        raw_before
    );
}

#[test]
fn unknown_unregistered_files_prevent_adoption_and_remain_untouched() {
    let f = Fixture::new();
    let item = f.plan();
    let directory = f.interrupted(&item);
    let protected = directory.join("user-note.txt");
    fs::write(&protected, b"unregistered original").unwrap();
    assert!(data_catalog::delete_owned(&f.manifest, &item.entry.id).is_err());
    assert_eq!(fs::read(&protected).unwrap(), b"unregistered original");
    assert!(directory.join("fmi-plan.json").is_file());
    assert_eq!(
        fs::read(directory.join("response-000001.xml")).unwrap(),
        b"interrupted"
    );
}

#[cfg(windows)]
#[test]
fn reparse_directory_prevents_recovery_deletion_without_following_its_target() {
    use std::os::windows::process::CommandExt;
    let f = Fixture::new();
    let item = f.plan();
    let directory = f.interrupted(&item);
    let outside = f.root.join("separate-original");
    fs::create_dir(&outside).unwrap();
    let protected = outside.join("protected.txt");
    fs::write(&protected, b"original").unwrap();
    let link = directory.join("unexpected-link");
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
    let result = data_catalog::delete_owned(&f.manifest, &item.entry.id);
    // Remove only this fixture's junction, never recursively operate on it.
    fs::remove_dir(&link).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(&protected).unwrap(), b"original");
    assert!(directory.join("fmi-plan.json").is_file());
}

#[test]
fn changed_response_before_publication_never_returns_a_complete_result() {
    let f = Fixture::new();
    let item = f.plan();
    let directory = f.directory(&item.entry.id);
    let outcome = fmi::fetch_with_transport(
        &f.manifest,
        &item.entry.id,
        &AtomicBool::new(false),
        |progress| {
            if progress.chunks_complete == 1 {
                fs::write(
                    directory.join("response-000001.xml"),
                    b"changed after receipt",
                )
                .unwrap();
            }
        },
        |_, _, path, _| {
            fs::write(path, XML).unwrap();
            Ok(fmi::TransportResult {
                http_status: Some(200),
                error: None,
            })
        },
    );
    assert!(
        outcome.is_err(),
        "Changed raw bytes must invalidate completion before publication"
    );
    assert!(!directory.join("fmi-registration-00000001.json").exists());
    assert_eq!(fs::read(f.root.join("fabricated.json")).unwrap(), b"{}\n");
}
