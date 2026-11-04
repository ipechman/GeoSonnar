//! Tiny fabricated persistence controls; no audio decoder, provider, GUI or field claim.
use geosonnar::{
    casework::{CaseMedia, CaseRecording, SavedCase},
    detailed_data::sha256,
    evidence_score::{
        LightningAltitudeDatum, LightningCatalog, LightningProvenance, LightningStrike,
        ObserverLocation,
    },
    investigation_archive::{self as archive, JointSnapshot, ReportKind},
    investigation_report,
    joint_evidence::{self as joint, JointLimits, JointRequest, JointStatus},
    map_data,
    regional_history::{self as history, RunStatus},
    regional_search::{self, ClockRange, EventAlternatives, RegionalPlan},
    thunder::{EnvelopeSample, ThunderEnvelope},
    thunder_envelope::{
        EnvelopeLimits, EnvelopeModel, EnvelopeReceiver, EnvelopeSearchPlan, ObservedEnvelope,
    },
    workspace::{
        AcousticSourceAssessment, EnvelopeReview, EnvelopeSelection, EventKind, EvidenceEvent,
        ReviewState, ReviewWorkspace, SourceBinding, TracePoint,
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

const ORIGIN: f64 = 1_788_825_600.0;
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    manifest: PathBuf,
    request: JointRequest,
}
impl Fixture {
    fn new() -> Self {
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
        fs::create_dir_all(&parent).unwrap();
        let root = parent.join(format!(
            "joint-history-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let recording = root.join("recording.bin");
        fs::write(&recording, b"Fabricated identity fixture; not audio").unwrap();
        let case = SavedCase {
            schema_version: 1,
            case_id: "joint-history-control".into(),
            title: "Fabricated persistence fixture".into(),
            created_unix_s: 1,
            local_only: true,
            recording: CaseRecording {
                path: recording.clone(),
                sha256: sha256(&recording).unwrap(),
                bytes: fs::metadata(&recording).unwrap().len(),
                media: CaseMedia {
                    duration_s: 30.0,
                    sample_rate_hz: 4000,
                    channels: 1,
                    audio_codec: "fabricated".into(),
                },
            },
        };
        let manifest = root.join("case.json");
        fs::write(&manifest, serde_json::to_vec(&case).unwrap()).unwrap();
        let mut document = ReviewWorkspace::new(&case, 0);
        document.recording_start_utc = "2026-09-08 00:00:00".into();
        document.map.bounds = [-0.1, -0.1, 0.1, 0.1];
        document.map.receiver_height_m = 0.0;
        document.mix.doppler_frequency = true;
        document.mix.doppler_timing = false;
        document.mix.thunder_timing = false;
        let mut event = EvidenceEvent::manual("tone".into(), EventKind::Doppler, 19.0, 25.0);
        event.review = ReviewState::Confirmed;
        event.selected_for_scoring = true;
        event.acoustic_source = AcousticSourceAssessment::Compatible;
        event.points = (0..4)
            .map(|i| TracePoint {
                time_s: 20.2 + i as f64,
                frequency_hz: 100.0 + i as f64,
            })
            .collect();
        document.events.push(event);
        let regional_plan = RegionalPlan {
            schema_version: 1,
            clock: ClockRange {
                min_offset_s: -1.0,
                max_offset_s: 1.0,
                step_s: 1.0,
            },
            events: vec![EventAlternatives {
                event_id: "tone".into(),
                source_ids: vec!["aircraft".into()],
                emitted_frequency_hz: Some([80.0, 160.0]),
            }],
            max_evaluations: 10,
            retain_hypotheses: 3,
        };
        document.regional_plan = Some(regional_plan.clone());
        let map_value = json!({"config":{"time_origin_utc_s":ORIGIN,"max_gap_s":11.0},"tracks":[{
            "flight_id":"aircraft","points":(-6..=12).map(|i|json!({"t_s":i as f64*10.0,"xyz_m":[6379137.0,500.0*i as f64,0.0]})).collect::<Vec<_>>()
        }]});
        let map_path = root.join("tracks.json");
        fs::write(&map_path, serde_json::to_vec(&map_value).unwrap()).unwrap();
        let mut map = map_data::parse_value(&map_value).unwrap();
        map.source_path = map_path.clone();
        map.source_sha256 = sha256(&map_path).unwrap();
        let lightning_path = root.join("lightning.json");
        fs::write(
            &lightning_path,
            b"Synthetic independently bound lightning source",
        )
        .unwrap();
        let lightning = LightningCatalog {
            schema_version: 1,
            input_path: lightning_path.clone(),
            input_bytes: fs::metadata(&lightning_path).unwrap().len(),
            input_sha256: sha256(&lightning_path).unwrap(),
            provenance: LightningProvenance {
                provider: "Synthetic".into(),
                source: "Persistence control".into(),
                license: None,
                notes: None,
                altitude_datum: LightningAltitudeDatum::Wgs84Ellipsoid,
                source_metadata: None,
            },
            strikes: vec![LightningStrike {
                id: "strike".into(),
                time_utc_s: ORIGIN + 17.0,
                latitude_deg: 0.01,
                longitude_deg: 0.0,
                altitude_m: Some(0.0),
                intensity: None,
                source_record: None,
            }],
        };
        document.datasets = vec![
            SourceBinding {
                path: map_path,
                sha256: map.source_sha256.clone(),
            },
            SourceBinding {
                path: lightning_path,
                sha256: lightning.input_sha256.clone(),
            },
        ];
        let note = "Fabricated envelope and source-class control <script>not executable</script>"
            .to_string();
        let observation = ObservedEnvelope {
            recording_sha256: document.recording_sha256.clone(),
            channel: 0,
            envelope: ThunderEnvelope {
                samples: [1.0, 2.0, 0.3, 1.0]
                    .iter()
                    .enumerate()
                    .map(|(i, rms)| EnvelopeSample {
                        time_s: 20.512 + i as f64,
                        rms: *rms,
                    })
                    .collect(),
                band_low_hz: 20.5078125,
                band_high_hz: 250.0,
                method: "Fabricated; no measurement claim".into(),
            },
            reviewed_selection_note: note.clone(),
        };
        document.envelope_review = Some(EnvelopeReview {
            selection: EnvelopeSelection {
                start_s: 20.0,
                end_s: 24.1,
                fft_size: 4096,
                hop_s: 1.0,
                band_hz: [20.0, 250.0],
                reviewed_selection_note: note,
            },
            observation: Some(observation.clone()),
        });
        let envelope_plan = EnvelopeSearchPlan {
            schema_version: 1,
            catalog_sha256: lightning.input_sha256.clone(),
            catalog_utc_interval: [ORIGIN - 120.0, ORIGIN + 120.0],
            receivers: vec![EnvelopeReceiver {
                id: "receiver".into(),
                location: ObserverLocation {
                    latitude_deg: 0.0,
                    longitude_deg: 0.0,
                    ellipsoid_height_m: 0.0,
                },
            }],
            nominal_recording_start_utc_s: ORIGIN,
            min_clock_offset_s: -1.0,
            max_clock_offset_s: 1.0,
            model: EnvelopeModel {
                sound_speed_mps: 343.0,
                fallback_strike_ellipsoid_height_m: None,
                minimum_distance_m: 1.0,
                maximum_distance_m: 5000.0,
                kernel_decay_s: 2.0,
                kernel_duration_s: 10.0,
            },
            source_policy: None,
            keep_best: 3,
            limits: EnvelopeLimits::default(),
        };
        let request = JointRequest {
            schema_version: 1,
            document,
            map,
            lightning,
            observation,
            regional_plan,
            envelope_plan,
            limits: JointLimits::default(),
        };
        joint::estimate(&request).unwrap();
        Self {
            root,
            manifest,
            request,
        }
    }
    fn input_hash(&self) -> String {
        regional_search::inputs_sha256(&self.request.map, &Some(self.request.lightning.clone()))
            .unwrap()
    }
    fn snapshot(&self, cancelled: bool) -> JointSnapshot {
        JointSnapshot {
            request: self.request.clone(),
            report: joint::run(&self.request, &AtomicBool::new(cancelled), |_| {}).unwrap(),
        }
    }
    fn begin(&self) -> history::RunHandle {
        history::begin_joint(
            &self.manifest,
            &self.request.document,
            &self.request,
            &self.input_hash(),
        )
        .unwrap()
    }
    fn run_path(&self, id: &str) -> PathBuf {
        self.root.join("regional-runs").join(id)
    }
    fn save(&self, cancelled: bool) -> history::RunSummary {
        let handle = self.begin();
        history::finish(
            handle,
            &Ok(ReportKind::JointEnvelope(Box::new(
                self.snapshot(cancelled),
            ))),
            cancelled,
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .canonicalize()
            .unwrap();
        let root = self.root.canonicalize().unwrap();
        assert_eq!(root.parent(), Some(parent.as_path()));
        let mut pending = vec![root.clone()];
        while let Some(path) = pending.pop() {
            let m = fs::symlink_metadata(&path).unwrap();
            assert!(!m.file_type().is_symlink());
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                assert_eq!(m.file_attributes() & 0x400, 0);
            }
            if m.is_dir() {
                for child in fs::read_dir(&path).unwrap() {
                    pending.push(child.unwrap().path());
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn joint_history_and_full_archive_reopen_without_original_sources() {
    let f = Fixture::new();
    let summary = f.save(false);
    assert_eq!(summary.status, RunStatus::Completed);
    let current = history::load(&f.manifest, &summary.id).unwrap();
    let ReportKind::JointEnvelope(current_snapshot) = current.report.as_ref().unwrap() else {
        panic!("wrong report kind")
    };
    archive::verify_joint_sources(
        &f.manifest,
        current_snapshot,
        &current.document,
        &f.request.map,
        &Some(f.request.lightning.clone()),
    )
    .unwrap();
    for name in ["recording.bin", "tracks.json", "lightning.json"] {
        fs::remove_file(f.root.join(name)).unwrap();
    }
    let loaded = history::load(&f.manifest, &summary.id).unwrap();
    assert!(loaded.joint_request.is_some() && loaded.result_sha256.is_some());
    let ReportKind::JointEnvelope(snapshot) = loaded.report.as_ref().unwrap() else {
        panic!("wrong report kind")
    };
    assert_eq!(snapshot.report.cells.len(), 3);
    assert!(
        archive::verify_joint_sources(
            &f.manifest,
            snapshot,
            &loaded.document,
            &f.request.map,
            &Some(f.request.lightning.clone())
        )
        .is_err()
    );
    let out = f.root.join("export");
    history::export_copy(&f.manifest, &summary.id, &out).unwrap();
    let reopened = archive::load(&out).unwrap();
    let ReportKind::JointEnvelope(reopened_snapshot) = reopened.report else {
        panic!("wrong archive kind")
    };
    assert_eq!(
        joint::request_sha256(&reopened_snapshot.request).unwrap(),
        snapshot.report.request_sha256
    );
    assert_eq!(
        serde_json::to_value(&reopened_snapshot.report).unwrap(),
        serde_json::to_value(&snapshot.report).unwrap()
    );
    let html = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(investigation_report::export_joint(&out, &loaded.document, snapshot).is_err());
    history::delete(&f.manifest, &summary.id).unwrap();
    assert!(history::list(&f.manifest).unwrap().is_empty());
}

#[test]
fn interrupted_and_failed_runs_retain_complete_joint_request() {
    let f = Fixture::new();
    let handle = f.begin();
    let id = handle.id().to_string();
    drop(handle);
    let loaded = history::load(&f.manifest, &id).unwrap();
    assert_eq!(loaded.summary.status, RunStatus::Interrupted);
    assert!(loaded.report.is_none());
    assert_eq!(
        joint::request_sha256(loaded.joint_request.as_ref().unwrap()).unwrap(),
        joint::request_sha256(&f.request).unwrap()
    );
    let failed = history::finish(
        f.begin(),
        &Err("Synthetic computation failure".into()),
        false,
    )
    .unwrap();
    let loaded = history::load(&f.manifest, &failed.id).unwrap();
    assert_eq!(loaded.summary.status, RunStatus::Failed);
    assert!(loaded.joint_request.is_some());
    assert!(history::export_copy(&f.manifest, &failed.id, &f.root.join("missing-result")).is_err());
    assert!(!f.root.join("missing-result").exists());
}

#[test]
fn cancelled_joint_run_preserves_unstarted_cells_and_execution_status() {
    let f = Fixture::new();
    let saved = f.save(true);
    let loaded = history::load(&f.manifest, &saved.id).unwrap();
    assert_eq!(saved.status, RunStatus::Cancelled);
    let ReportKind::JointEnvelope(snapshot) = loaded.report.unwrap() else {
        panic!("wrong kind")
    };
    assert_eq!(snapshot.report.status, JointStatus::Cancelled);
    assert!(!snapshot.report.ranking_available);
    assert_eq!(snapshot.report.cells.len(), 3);
    let out = f.root.join("cancelled-export");
    investigation_report::export_joint(&out, &loaded.document, &snapshot).unwrap();
    assert_eq!(
        archive::load(&out).unwrap().inputs_sha256,
        saved.inputs_sha256
    );
}

#[test]
fn request_substitution_is_refused_without_destroying_journal() {
    let f = Fixture::new();
    let handle = f.begin();
    let id = handle.id().to_string();
    let mut changed = f.request.clone();
    changed.envelope_plan.model.minimum_distance_m = 2.0;
    let report = joint::run(&changed, &AtomicBool::new(false), |_| {}).unwrap();
    assert!(
        history::finish(
            handle,
            &Ok(ReportKind::JointEnvelope(Box::new(JointSnapshot {
                request: changed,
                report
            }))),
            false
        )
        .is_err()
    );
    let loaded = history::load(&f.manifest, &id).unwrap();
    assert_eq!(loaded.summary.status, RunStatus::Interrupted);
    assert!(!f.run_path(&id).join("result.json").exists());
}

#[test]
fn changed_start_request_and_rehashed_forged_archive_are_refused() {
    let f = Fixture::new();
    let saved = f.save(false);
    let out = f.root.join("forgery");
    history::export_copy(&f.manifest, &saved.id, &out).unwrap();
    let mut snapshot: Value =
        serde_json::from_slice(&fs::read(out.join("joint-envelope.json")).unwrap()).unwrap();
    snapshot["report"]["inputs_sha256"] = json!("f".repeat(64));
    fs::write(
        out.join("joint-envelope.json"),
        serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
    let mut receipt: Value =
        serde_json::from_slice(&fs::read(out.join("receipt.json")).unwrap()).unwrap();
    for a in receipt["artifacts"].as_array_mut().unwrap() {
        if a["path"] == "joint-envelope.json" {
            a["sha256"] = json!(sha256(&out.join("joint-envelope.json")).unwrap());
            a["bytes"] = json!(fs::metadata(out.join("joint-envelope.json")).unwrap().len());
        }
    }
    fs::write(
        out.join("receipt.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    assert!(archive::load(&out).is_err());
    fs::write(f.run_path(&saved.id).join("joint-request.json"), b"{}").unwrap();
    assert!(history::load(&f.manifest, &saved.id).is_err());
    assert!(history::delete(&f.manifest, &saved.id).is_err());
    assert!(f.run_path(&saved.id).join("result.json").exists());
}

#[test]
fn completed_execution_with_unavailable_envelope_is_not_relabelled_cancelled() {
    let mut f = Fixture::new();
    f.request.lightning.strikes[0].time_utc_s = ORIGIN - 100.0;
    let saved = f.save(false);
    assert_eq!(saved.status, RunStatus::Completed);
    let loaded = history::load(&f.manifest, &saved.id).unwrap();
    let ReportKind::JointEnvelope(snapshot) = loaded.report.unwrap() else {
        panic!("wrong kind")
    };
    assert_eq!(snapshot.report.status, JointStatus::Completed);
    assert!(!snapshot.report.all_cells_available && !snapshot.report.ranking_available);
    assert_eq!(snapshot.report.cells.len(), 3);
}

#[test]
fn legacy_start_bytes_stay_unchanged_and_cannot_accept_an_unjournaled_joint_result() {
    let f = Fixture::new();
    let handle = history::begin(&f.manifest, &f.request.document, &f.input_hash()).unwrap();
    let id = handle.id().to_string();
    let start_path = f.run_path(&id).join("start.json");
    let original = fs::read(&start_path).unwrap();
    let start: Value = serde_json::from_slice(&original).unwrap();
    assert!(start.get("joint_request").is_none());
    assert!(
        history::finish(
            handle,
            &Ok(ReportKind::JointEnvelope(Box::new(f.snapshot(false)))),
            false
        )
        .is_err()
    );
    let loaded = history::load(&f.manifest, &id).unwrap();
    assert!(loaded.joint_request.is_none() && loaded.report.is_none());
    assert_eq!(fs::read(start_path).unwrap(), original);
    assert_eq!(loaded.summary.status, RunStatus::Interrupted);
}

#[test]
fn an_edited_review_cannot_be_exported_as_the_original_joint_result() {
    let f = Fixture::new();
    let snapshot = f.snapshot(false);
    let mut edited = f.request.document.clone();
    edited.notes.push_str("changed after run");
    assert!(
        investigation_report::export_joint(&f.root.join("edited"), &edited, &snapshot).is_err()
    );
    assert!(!f.root.join("edited").exists());
    investigation_report::export_joint(&f.root.join("original"), &f.request.document, &snapshot)
        .unwrap();
}
