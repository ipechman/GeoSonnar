//! Synthetic controls for durable computation history; no real audio truth,
//! provider requests, GUI or external source files are needed.
use geosonnar::{
    casework::{CaseMedia, CaseRecording, SavedCase},
    evidence_score::{
        LightningAltitudeDatum, LightningCatalog, LightningProvenance, LightningStrike,
        ObserverLocation,
    },
    investigation_archive::{self, ReportKind},
    map_data::{self, MapData},
    regional_history::{self as history, RunStatus},
    regional_search::{self, ClockRange, EventAlternatives, RegionalPlan},
    workspace::{
        AcousticSourceAssessment, EventKind, EvidenceEvent, ReviewState, ReviewWorkspace,
        SourceBinding,
    },
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    manifest: PathBuf,
    document: ReviewWorkspace,
    map: MapData,
    lightning: Option<LightningCatalog>,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("output/benchmarks/regional-history-controls")
            .join(format!(
                "{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        let case = SavedCase {
            schema_version: 1,
            case_id: "synthetic-history".into(),
            title: "Synthetic journal control; not real storm evidence".into(),
            created_unix_s: 1,
            local_only: true,
            recording: CaseRecording {
                path: root.join("original-recording-not-present.wav"),
                sha256: "0".repeat(64),
                bytes: 100,
                media: CaseMedia {
                    duration_s: 30.0,
                    sample_rate_hz: 48000,
                    channels: 1,
                    audio_codec: "fabricated".into(),
                },
            },
        };
        let manifest = root.join("case.json");
        fs::write(&manifest, serde_json::to_vec(&case).unwrap()).unwrap();
        let mut document = ReviewWorkspace::new(&case, 0);
        document.recording_start_utc = "2026-09-08 00:00:00".into();
        document.datasets.push(SourceBinding {
            path: root.join("original-lightning-not-present.json"),
            sha256: "1".repeat(64),
        });
        let mut event = EvidenceEvent::manual("thunder".into(), EventKind::Thunder, 5.0, 15.0);
        event.arrival_s = 10.0;
        event.selected_for_scoring = true;
        event.review = ReviewState::Confirmed;
        event.acoustic_source = AcousticSourceAssessment::Compatible;
        document.events.push(event);
        document.mix.doppler_frequency = false;
        document.mix.doppler_timing = false;
        document.mix.thunder_timing = true;
        document.regional_plan = Some(RegionalPlan {
            schema_version: 1,
            clock: ClockRange {
                min_offset_s: -1.0,
                max_offset_s: 1.0,
                step_s: 1.0,
            },
            events: vec![EventAlternatives {
                event_id: "thunder".into(),
                source_ids: vec!["synthetic-strike".into()],
                emitted_frequency_hz: None,
            }],
            max_evaluations: 10,
            retain_hypotheses: 3,
        });
        let map = map_data::parse_value(&serde_json::json!({"schema_version":1,"strikes":[{"id":"unused","time_utc_s":1788825600.0,"latitude_deg":0.0,"longitude_deg":0.0}]})).unwrap();
        let lightning = Some(LightningCatalog {
            schema_version: 1,
            input_path: document.datasets[0].path.clone(),
            input_bytes: 1,
            input_sha256: document.datasets[0].sha256.clone(),
            provenance: LightningProvenance {
                provider: "Synthetic".into(),
                source: "Independent fabricated source".into(),
                license: None,
                notes: None,
                altitude_datum: LightningAltitudeDatum::Wgs84Ellipsoid,
                source_metadata: None,
            },
            strikes: vec![LightningStrike {
                id: "synthetic-strike".into(),
                time_utc_s: 1788825606.75,
                latitude_deg: 0.01,
                longitude_deg: 0.0,
                altitude_m: Some(0.0),
                intensity: None,
                source_record: None,
            }],
        });
        Self {
            root,
            manifest,
            document,
            map,
            lightning,
        }
    }
    fn inputs(&self) -> String {
        regional_search::inputs_sha256(&self.map, &self.lightning).unwrap()
    }
    fn report(&self, cancelled: bool) -> ReportKind {
        ReportKind::Regional(
            regional_search::run(
                &self.document,
                &self.map,
                &self.lightning,
                &[ObserverLocation {
                    latitude_deg: 0.0,
                    longitude_deg: 0.0,
                    ellipsoid_height_m: 0.0,
                }],
                self.document.regional_plan.as_ref().unwrap(),
                &AtomicBool::new(cancelled),
                |_| {},
            )
            .unwrap(),
        )
    }
    fn run_path(&self, id: &str) -> PathBuf {
        self.root.join("regional-runs").join(id)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("output/benchmarks/regional-history-controls")
            .canonicalize()
            .unwrap();
        let root = self.root.canonicalize().unwrap();
        assert!(root.starts_with(base));
        let mut queue = vec![root.clone()];
        while let Some(path) = queue.pop() {
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                assert_eq!(metadata.file_attributes() & 0x400, 0);
            }
            if metadata.is_dir() {
                for child in fs::read_dir(path).unwrap() {
                    queue.push(child.unwrap().path());
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn completed_result_survives_edit_restart_and_export_without_original_sources() {
    let mut f = Fixture::new();
    let original = f.document.clone();
    let handle = history::begin(&f.manifest, &original, &f.inputs()).unwrap();
    let id = handle.id().to_owned();
    assert_eq!(
        history::list(&f.manifest).unwrap()[0].status,
        RunStatus::Running
    );
    let report = f.report(false);
    history::finish(handle, &Ok(report), false).unwrap();
    let original_bytes = fs::read(f.run_path(&id).join("result.json")).unwrap();
    f.document.notes = "Later contradictory clues".into();
    f.document.events[0].arrival_s += 1.5;
    // A new load has no process state and never follows absent source paths.
    let loaded = history::load(&f.manifest, &id).unwrap();
    assert_eq!(loaded.document, original);
    assert_eq!(loaded.summary.status, RunStatus::Completed);
    assert_eq!(loaded.summary.inputs_sha256, f.inputs());
    assert_ne!(loaded.document, f.document);
    let export = f.root.join("explicit-result-copy");
    history::export_copy(&f.manifest, &id, &export).unwrap();
    let reopened = investigation_archive::load(&export).unwrap();
    assert_eq!(reopened.document, original);
    assert!(!reopened.has_replay_inputs);
    assert_eq!(reopened.inputs_sha256, f.inputs());
    assert_eq!(
        fs::read(f.run_path(&id).join("result.json")).unwrap(),
        original_bytes
    );
    assert!(history::export_copy(&f.manifest, &id, &export).is_err());
}

#[test]
fn cancellation_failure_and_interrupted_worker_are_distinct_durable_states() {
    let f = Fixture::new();
    let interrupted = history::begin(&f.manifest, &f.document, &f.inputs()).unwrap();
    let interrupted_id = interrupted.id().to_owned();
    assert!(history::delete(&f.manifest, &interrupted_id).is_err());
    drop(interrupted);
    assert_eq!(
        history::load(&f.manifest, &interrupted_id)
            .unwrap()
            .summary
            .status,
        RunStatus::Interrupted
    );
    let cancelled = history::begin(&f.manifest, &f.document, &f.inputs()).unwrap();
    let cancelled_id = cancelled.id().to_owned();
    history::finish(cancelled, &Ok(f.report(true)), true).unwrap();
    let loaded = history::load(&f.manifest, &cancelled_id).unwrap();
    assert_eq!(loaded.summary.status, RunStatus::Cancelled);
    assert!(loaded.report.is_some());
    let failed = history::begin(&f.manifest, &f.document, &f.inputs()).unwrap();
    let failed_id = failed.id().to_owned();
    history::finish(
        failed,
        &Err("Original source changed during computation".into()),
        false,
    )
    .unwrap();
    let loaded = history::load(&f.manifest, &failed_id).unwrap();
    assert_eq!(loaded.summary.status, RunStatus::Failed);
    assert!(loaded.report.is_none());
    assert!(loaded.summary.error.unwrap().contains("source changed"));
    assert_eq!(history::list(&f.manifest).unwrap().len(), 3);
    for id in [interrupted_id, cancelled_id, failed_id] {
        history::delete(&f.manifest, &id).unwrap();
    }
    assert!(history::list(&f.manifest).unwrap().is_empty());
    assert!(f.manifest.is_file());
}

#[test]
fn result_cannot_be_bound_to_newer_or_different_loaded_sources() {
    let f = Fixture::new();
    let handle = history::begin(&f.manifest, &f.document, &f.inputs()).unwrap();
    let id = handle.id().to_owned();
    let mut forged = f.report(false);
    let ReportKind::Regional(report) = &mut forged else {
        unreachable!()
    };
    report.inputs_sha256 = "f".repeat(64);
    assert!(
        history::finish(handle, &Ok(forged), false)
            .unwrap_err()
            .contains("original reviewed")
    );
    assert_eq!(
        history::load(&f.manifest, &id).unwrap().summary.status,
        RunStatus::Interrupted
    );
    let handle = history::begin(&f.manifest, &f.document, &f.inputs()).unwrap();
    let id = handle.id().to_owned();
    history::finish(handle, &Ok(f.report(false)), false).unwrap();
    let path = f.run_path(&id).join("result.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b' ');
    fs::write(&path, bytes).unwrap();
    assert!(history::load(&f.manifest, &id).is_err());
    assert!(history::export_copy(&f.manifest, &id, &f.root.join("tampered-copy")).is_err());
    assert!(history::delete(&f.manifest, &id).is_err());
}

#[test]
fn exact_case_and_workspace_receipts_prevent_cross_case_or_modified_snapshot_adoption() {
    let f = Fixture::new();
    let handle = history::begin(&f.manifest, &f.document, &f.inputs()).unwrap();
    let id = handle.id().to_owned();
    history::finish(handle, &Ok(f.report(false)), false).unwrap();
    let mut replacement = f.document.clone();
    replacement.events[0].arrival_s += 2.0;
    fs::write(
        f.run_path(&id).join("workspace.json"),
        serde_json::to_vec(&replacement).unwrap(),
    )
    .unwrap();
    assert!(history::load(&f.manifest, &id).is_err());
    assert!(history::load(&f.manifest, "../case.json").is_err());
    let mut case: serde_json::Value =
        serde_json::from_slice(&fs::read(&f.manifest).unwrap()).unwrap();
    case["case_id"] = "another-case".into();
    fs::write(&f.manifest, serde_json::to_vec(&case).unwrap()).unwrap();
    assert!(
        history::list(&f.manifest)
            .unwrap_err()
            .contains("another case")
    );
}

#[test]
fn unowned_storage_is_never_adopted_or_deleted() {
    let f = Fixture::new();
    let unrelated = f.root.join("regional-runs");
    fs::create_dir(&unrelated).unwrap();
    fs::write(unrelated.join("personal-note.txt"), b"keep me").unwrap();
    assert!(history::begin(&f.manifest, &f.document, &f.inputs()).is_err());
    assert_eq!(
        fs::read(unrelated.join("personal-note.txt")).unwrap(),
        b"keep me"
    );
    assert!(history::delete(&f.manifest, "run-1-2-3").is_err());
}
