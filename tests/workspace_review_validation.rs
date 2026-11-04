//! Preservation checks for fabricated local recordings and manual annotations.
use geosonnar::{
    casework::{CaseMedia, CaseRecording, SavedCase},
    detailed_data::sha256,
    workspace::{
        self, EventKind, EvidenceEvent, ReviewState, ReviewWorkspace, SourceBinding, TracePoint,
    },
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn workspace_verification_cancellation_preserves_review_and_all_identity_checks() {
    let f = Fixture::new();
    let before = f.document.clone();
    let source_hash = sha256(&f.recording).unwrap();
    let mut count = 0;
    workspace::verify_with_checkpoint(&f.manifest, &f.document, || {
        count += 1;
        Ok(())
    })
    .unwrap();
    assert!(count > 10);
    for stop in [1, count / 2, count] {
        let mut seen = 0;
        let result = workspace::verify_with_checkpoint(&f.manifest, &f.document, || {
            seen += 1;
            if seen == stop {
                Err("requested stop".into())
            } else {
                Ok(())
            }
        });
        assert_eq!(result.unwrap_err(), "requested stop");
        assert_eq!(seen, stop);
    }
    assert!(
        workspace::verify_with_cancel(
            &f.manifest,
            &f.document,
            &std::sync::atomic::AtomicBool::new(true)
        )
        .unwrap_err()
        .contains("cancelled")
    );
    assert_eq!(before, f.document);
    assert_eq!(source_hash, sha256(&f.recording).unwrap());
    workspace::verify(&f.manifest, &f.document).unwrap();
    fs::write(&f.recording, b"changed").unwrap();
    assert!(
        workspace::verify_with_checkpoint(&f.manifest, &f.document, || Ok(()))
            .unwrap_err()
            .contains("changed")
    );
}
struct Fixture {
    root: PathBuf,
    manifest: PathBuf,
    recording: PathBuf,
    dataset: PathBuf,
    proposal: PathBuf,
    document: ReviewWorkspace,
}
impl Fixture {
    fn new() -> Self {
        let run = ROOT.get_or_init(|| {
            let p = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("output/benchmarks/workspace-review/integration")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            fs::create_dir_all(&p).unwrap();
            p
        });
        let root = run.join(format!("case-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(root.join("data")).unwrap();
        let recording = root.join("recording.fake");
        fs::write(
            &recording,
            b"Fabricated recording identity, no private audio",
        )
        .unwrap();
        let dataset = root.join("data/tracks.json");
        fs::write(&dataset, b"{\"fabricated\":\"track source\"}\n").unwrap();
        let proposal = root.join("data/proposal.json");
        fs::write(&proposal, b"{\"fabricated\":\"proposal source\"}\n").unwrap();
        let case = SavedCase {
            schema_version: 1,
            case_id: "synthetic-case".into(),
            title: "Fabricated manual review".into(),
            created_unix_s: 1_788_825_600,
            local_only: true,
            recording: CaseRecording {
                path: PathBuf::from("recording.fake"),
                sha256: sha256(&recording).unwrap(),
                bytes: fs::metadata(&recording).unwrap().len(),
                media: CaseMedia {
                    duration_s: 90.0,
                    sample_rate_hz: 4000,
                    channels: 2,
                    audio_codec: "fabricated".into(),
                },
            },
        };
        let manifest = root.join("case.json");
        fs::write(&manifest, serde_json::to_vec_pretty(&case).unwrap()).unwrap();
        let mut document = ReviewWorkspace::new(&case, 1);
        document.recording_start_utc = "2026-09-08 03:04:05".into();
        document.notes =
            "Keep my wording: thunder? needs a source association.\nUnicode café.".into();
        let mut event =
            EvidenceEvent::manual("manual-doppler-1".into(), EventKind::Doppler, 10.0, 30.0);
        event.label = "Edited aircraft ridge".into();
        event.arrival_s = 20.25;
        event.uncertainty_s = 0.4;
        event.frequency_sigma_hz = 0.7;
        event.flight_id = "fabricated-flight".into();
        event.emitted_frequency_hz = 103.125;
        event.points = vec![
            TracePoint {
                time_s: 10.25,
                frequency_hz: 110.25,
            },
            TracePoint {
                time_s: 19.5,
                frequency_hz: 103.125,
            },
            TracePoint {
                time_s: 29.75,
                frequency_hz: 98.375,
            },
        ];
        event.review = ReviewState::Confirmed;
        event.selected_for_scoring = true;
        event.source = Some(SourceBinding {
            path: proposal.canonicalize().unwrap(),
            sha256: sha256(&proposal).unwrap(),
        });
        let mut thunder =
            EvidenceEvent::manual("manual-thunder-1".into(), EventKind::Thunder, 40.0, 49.0);
        thunder.arrival_s = 40.7;
        thunder.review = ReviewState::Rejected;
        thunder.label = "Rejected handling noise".into();
        document.events = vec![event, thunder];
        document.mix.doppler_frequency = true;
        document.mix.frequency_weight = 2.5;
        document.mix.frequency_scale_hz = 0.25;
        document.map.clock_adjustment_s = -1.25;
        document.datasets.push(SourceBinding {
            path: dataset.canonicalize().unwrap(),
            sha256: sha256(&dataset).unwrap(),
        });
        Self {
            root,
            manifest,
            recording,
            dataset,
            proposal,
            document,
        }
    }
}

#[test]
fn old_review_roundtrips_with_unassessed_sources_and_unchanged_serialized_identity() {
    use geosonnar::workspace::AcousticSourceAssessment;
    let f = Fixture::new();
    let original_bytes = serde_json::to_vec(&f.document).unwrap();
    assert!(!String::from_utf8_lossy(&original_bytes).contains("acoustic_source"));
    let old: ReviewWorkspace = serde_json::from_slice(&original_bytes).unwrap();
    assert_eq!(serde_json::to_vec(&old).unwrap(), original_bytes);
    assert_eq!(
        geosonnar::regional_search::document_sha256(&old).unwrap(),
        geosonnar::regional_search::document_sha256(&f.document).unwrap()
    );
    assert!(
        old.events
            .iter()
            .all(|e| e.acoustic_source == AcousticSourceAssessment::Unassessed)
    );
    assert!(old.events[0].selected_for_scoring);
    old.validate().unwrap();
    assert!(
        old.validate_acoustic_sources_for_scoring()
            .unwrap_err()
            .contains("unassessed")
    );
    let saved = workspace::save(&f.manifest, &old).unwrap();
    let reopened = workspace::load(&f.manifest, &saved).unwrap();
    assert_eq!(reopened, old);
    assert!(
        !String::from_utf8(fs::read(&saved).unwrap())
            .unwrap()
            .contains("acoustic_source")
    );
}

#[test]
fn source_assessment_drafts_roundtrip_but_uncertain_reason_is_required_to_score() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    let mut f = Fixture::new();
    for assessment in [
        Assessment::Unassessed,
        Assessment::Compatible,
        Assessment::UncertainHypothesis {
            rationale: "".into(),
        },
        Assessment::UncertainHypothesis {
            rationale: "A reviewed synthetic uncertainty case".into(),
        },
        Assessment::Incompatible,
    ] {
        f.document.events[0].acoustic_source = assessment.clone();
        let saved = workspace::save(&f.manifest, &f.document).unwrap();
        let reopened = workspace::load(&f.manifest, &saved).unwrap();
        assert_eq!(reopened.events[0].acoustic_source, assessment);
        assert_eq!(reopened.events[0].review, ReviewState::Confirmed);
        assert!(reopened.events[0].selected_for_scoring);
        assert_eq!(
            reopened.validate_acoustic_sources_for_scoring().is_ok(),
            assessment.scoring_block_reason().is_none()
        );
    }
    f.document.events[0].acoustic_source = Assessment::UncertainHypothesis {
        rationale: "x".repeat(4001),
    };
    assert!(f.document.validate().unwrap_err().contains("4000 bytes"));
}

#[test]
fn acoustic_source_schema_rejects_unknown_and_incomplete_assessments() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    for kind in [EventKind::Doppler, EventKind::Thunder] {
        let event = EvidenceEvent::manual("new-event".into(), kind, 1.0, 3.0);
        assert_eq!(event.acoustic_source, Assessment::Unassessed);
        assert_eq!(event.review, ReviewState::Pending);
        assert!(!event.selected_for_scoring);
    }
    for value in [
        serde_json::json!("verified"),
        serde_json::json!({"uncertain_hypothesis":{}}),
        serde_json::json!({"uncertain_hypothesis":{"rationale":"x", "auto_accept":true}}),
    ] {
        assert!(serde_json::from_value::<Assessment>(value).is_err());
    }
}

#[test]
fn save_and_reopen_preserve_manual_points_decisions_and_user_settings_exactly() {
    let f = Fixture::new();
    let saved = workspace::save(&f.manifest, &f.document).unwrap();
    let loaded = workspace::load(&f.manifest, &saved).unwrap();
    assert_eq!(loaded, f.document);
    assert_eq!(loaded.events[0].review, ReviewState::Confirmed);
    assert!(loaded.events[0].selected_for_scoring);
    assert_eq!(loaded.events[1].review, ReviewState::Rejected);
    assert!(!loaded.events[1].selected_for_scoring);
    assert!(loaded.events[1].points.is_empty());
    let csv = fs::read_to_string(saved.parent().unwrap().join("manual-doppler-1.csv")).unwrap();
    assert_eq!(
        csv,
        "time_s,frequency_hz,weight\n10.25,110.25,1\n19.5,103.125,1\n29.75,98.375,1\n"
    );
    let trace: geosonnar::physics::EventTrace = serde_json::from_slice(
        &fs::read(saved.parent().unwrap().join("manual-doppler-1.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(trace.f0_hz, vec![103.125]);
    assert_eq!(trace.points.len(), 3);
    assert_eq!(trace.points[1].frequency_hz, 103.125);
}

#[test]
fn a_new_save_keeps_previous_review_and_exports_unchanged() {
    let mut f = Fixture::new();
    let first = workspace::save(&f.manifest, &f.document).unwrap();
    let original = fs::read(&first).unwrap();
    let old_csv = fs::read(first.parent().unwrap().join("manual-doppler-1.csv")).unwrap();
    f.document.events[0].points[1].frequency_hz = 102.5;
    f.document.events[0].review = ReviewState::Pending;
    f.document.events[0].selected_for_scoring = false;
    let second = workspace::save(&f.manifest, &f.document).unwrap();
    assert_ne!(first, second);
    assert_eq!(fs::read(&first).unwrap(), original);
    assert_eq!(
        fs::read(first.parent().unwrap().join("manual-doppler-1.csv")).unwrap(),
        old_csv
    );
    let reopened = workspace::load(&f.manifest, &second).unwrap();
    assert_eq!(reopened.events[0].points[1].frequency_hz, 102.5);
    assert!(!reopened.events[0].selected_for_scoring);
}

#[test]
fn pending_or_rejected_annotations_cannot_be_selected_even_when_loading_json() {
    for state in [ReviewState::Pending, ReviewState::Rejected] {
        let mut f = Fixture::new();
        f.document.events[0].review = state;
        assert!(f.document.validate().is_err());
        assert!(workspace::save(&f.manifest, &f.document).is_err());
        let path = f.root.join("invalid-selection.json");
        fs::write(&path, serde_json::to_vec(&f.document).unwrap()).unwrap();
        assert!(
            workspace::load(&f.manifest, &path)
                .unwrap_err()
                .contains("Confirm")
        );
        assert!(!f.root.join("workspace-reviews").exists());
    }
}

#[test]
fn changed_dataset_or_proposal_source_blocks_save_and_reopen_without_touching_saved_choices() {
    for source_kind in ["dataset", "proposal"] {
        let f = Fixture::new();
        let saved = workspace::save(&f.manifest, &f.document).unwrap();
        let original = fs::read(&saved).unwrap();
        let source = if source_kind == "dataset" {
            &f.dataset
        } else {
            &f.proposal
        };
        fs::write(source, b"Changed fabricated evidence bytes").unwrap();
        assert!(
            workspace::save(&f.manifest, &f.document)
                .unwrap_err()
                .contains("Evidence file changed")
        );
        assert!(
            workspace::load(&f.manifest, &saved)
                .unwrap_err()
                .contains("Evidence file changed")
        );
        assert_eq!(fs::read(&saved).unwrap(), original);
    }
}

#[test]
fn changed_recording_wrong_channel_and_foreign_case_are_rejected() {
    let mut f = Fixture::new();
    let saved = workspace::save(&f.manifest, &f.document).unwrap();
    f.document.channel = 2;
    assert!(workspace::save(&f.manifest, &f.document).is_err());
    f.document.channel = 1;
    f.document.case_id = "foreign-case".into();
    assert!(workspace::save(&f.manifest, &f.document).is_err());
    fs::write(&f.recording, b"Changed fabricated media").unwrap();
    assert!(workspace::load(&f.manifest, &saved).is_err());
}

#[test]
fn invalid_manual_trace_shape_and_unsafe_ids_fail_before_any_exports() {
    let f = Fixture::new();
    for variation in 0..6 {
        let mut doc = f.document.clone();
        match variation {
            0 => doc.events[0].points[1].time_s = doc.events[0].points[0].time_s,
            1 => doc.events[0].points[0].time_s = 9.9,
            2 => doc.events[0].points[1].frequency_hz = f64::NAN,
            3 => doc.events[0].arrival_s = 31.0,
            4 => doc.events[0].id = "../escape".into(),
            _ => doc.events[1].points.push(TracePoint {
                time_s: 42.0,
                frequency_hz: 80.0,
            }),
        }
        assert!(workspace::save(&f.manifest, &doc).is_err());
    }
    assert!(!f.root.join("workspace-reviews").exists());
}

#[test]
fn native_utc_parser_rejects_malformed_calendar_and_clock_syntax() {
    assert_eq!(
        workspace::parse_utc("2026-09-08T03:04:05Z").unwrap(),
        1_788_836_645.0
    );
    for utc in [
        "2026-09-08 03:04:05ZZ",
        "2026-09-08 +1:04:05",
        "2026-09-08 03:+1:05",
        "2026-09-08 03:04:+1",
        "2026-02-29 03:04:05",
        "2026-09-08 24:00:00",
        "2026-09-08 03:60:00",
        "2026-09-08 03:04:60",
        "2026-09-08 03:04:05+02:00",
        "２０２６-09-08 03:04:05",
    ] {
        assert!(
            workspace::parse_utc(utc).is_err(),
            "Accepted malformed UTC {utc:?}"
        );
    }
}

#[test]
fn serialized_review_retains_rejected_and_pending_rows_instead_of_pruning_them() {
    let mut f = Fixture::new();
    let pending = EvidenceEvent::manual("pending-manual".into(), EventKind::Doppler, 60.0, 70.0);
    f.document.events.push(pending);
    let saved = workspace::save(&f.manifest, &f.document).unwrap();
    let value: Value = serde_json::from_slice(&fs::read(saved).unwrap()).unwrap();
    assert_eq!(value["events"].as_array().unwrap().len(), 3);
    assert_eq!(value["events"][1]["review"], "rejected");
    assert_eq!(value["events"][2]["review"], "pending");
}

#[test]
fn saved_case_local_bindings_reopen_after_the_entire_case_is_relocated() {
    let mut f = Fixture::new();
    f.document
        .register_dataset(
            SourceBinding {
                path: f.dataset.canonicalize().unwrap(),
                sha256: sha256(&f.dataset).unwrap(),
            },
            Some(1_788_825_600.0),
            false,
        )
        .unwrap();
    f.document.strike_fallback_height_m = Some(125.0);
    let saved = workspace::save(&f.manifest, &f.document).unwrap();
    let value: Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    assert_eq!(
        Path::new(value["datasets"][0]["path"].as_str().unwrap()),
        Path::new("data/tracks.json")
    );
    assert_eq!(
        Path::new(value["events"][0]["source"]["path"].as_str().unwrap()),
        Path::new("data/proposal.json")
    );
    let original_root = f.root.canonicalize().unwrap();
    let allowed = original_root.parent().unwrap().canonicalize().unwrap();
    let destination = allowed.join(format!(
        "relocated-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    assert!(original_root.starts_with(&allowed));
    assert_eq!(destination.parent().unwrap(), allowed);
    assert!(!destination.exists());
    let relative_saved = saved.strip_prefix(&original_root).unwrap().to_path_buf();
    fs::rename(&original_root, &destination).unwrap();
    let moved = workspace::load(
        &destination.join("case.json"),
        &destination.join(relative_saved),
    )
    .unwrap();
    assert_eq!(moved.events[0].points, f.document.events[0].points);
    assert_eq!(moved.dataset_options, f.document.dataset_options);
    assert_eq!(moved.strike_fallback_height_m, Some(125.0));
    assert_eq!(
        moved.datasets[0].path,
        destination.join("data/tracks.json").canonicalize().unwrap()
    );
    assert_eq!(
        moved.events[0].source.as_ref().unwrap().path,
        destination
            .join("data/proposal.json")
            .canonicalize()
            .unwrap()
    );
    assert!(moved.events[0].selected_for_scoring);
}

#[test]
fn registered_assumptions_replace_duplicates_and_failed_changes_leave_choices_untouched() {
    let mut f = Fixture::new();
    let binding = f.document.datasets[0].clone();
    f.document
        .register_dataset(binding.clone(), Some(1_788_825_600.0), false)
        .unwrap();
    f.document
        .register_dataset(binding.clone(), Some(1_788_825_601.0), true)
        .unwrap();
    assert_eq!(f.document.datasets.len(), 1);
    assert_eq!(f.document.dataset_options.len(), 1);
    assert!(f.document.dataset_options[0].lightning_altitude_wgs84);
    assert_eq!(
        f.document.dataset_options[0].time_origin_utc_s,
        Some(1_788_825_601.0)
    );
    let before = f.document.clone();
    assert!(
        f.document
            .register_dataset(binding, Some(f64::NAN), false)
            .is_err()
    );
    assert_eq!(f.document, before);
    fs::write(&f.dataset, b"New fabricated source revision").unwrap();
    let new_hash = sha256(&f.dataset).unwrap();
    f.document
        .register_dataset(
            SourceBinding {
                path: f.dataset.clone(),
                sha256: new_hash.clone(),
            },
            None,
            false,
        )
        .unwrap();
    assert_eq!(f.document.datasets.len(), 1);
    assert_eq!(f.document.dataset_options.len(), 1);
    assert_eq!(f.document.dataset_options[0].source_sha256, new_hash);
    workspace::verify(&f.manifest, &f.document).unwrap();
}

#[test]
fn windows_reserved_ids_and_case_collisions_fail_before_creating_a_revision() {
    let f = Fixture::new();
    for reserved in ["workspace", "WORKSPACE", "CON", "nul", "COM1", "Lpt9"] {
        let mut doc = f.document.clone();
        doc.events[0].id = reserved.into();
        assert!(
            workspace::save(&f.manifest, &doc).is_err(),
            "Accepted reserved event ID {reserved}"
        );
    }
    let mut doc = f.document.clone();
    doc.events[1].id = doc.events[0].id.to_uppercase();
    assert!(workspace::save(&f.manifest, &doc).is_err());
    assert!(!f.root.join("workspace-reviews").exists());
}

#[test]
fn invalid_or_duplicate_dataset_options_cannot_change_the_saved_interpretation() {
    let mut f = Fixture::new();
    let binding = f.document.datasets[0].clone();
    f.document
        .register_dataset(binding, Some(1_788_825_600.0), false)
        .unwrap();
    let saved = workspace::save(&f.manifest, &f.document).unwrap();
    let saved_bytes = fs::read(&saved).unwrap();
    for variation in 0..4 {
        let mut doc = f.document.clone();
        match variation {
            0 => doc.dataset_options.push(doc.dataset_options[0].clone()),
            1 => doc.dataset_options[0].source_sha256 = "1".repeat(64),
            2 => doc.strike_fallback_height_m = Some(f64::NAN),
            _ => doc.datasets[0].path = PathBuf::from("../unrelated.json"),
        }
        assert!(workspace::save(&f.manifest, &doc).is_err());
    }
    assert_eq!(fs::read(saved).unwrap(), saved_bytes);
}

#[test]
fn older_review_fields_get_safe_defaults_but_unknown_fields_are_not_silently_lost() {
    let f = Fixture::new();
    let mut value = serde_json::to_value(&f.document).unwrap();
    value.as_object_mut().unwrap().remove("dataset_options");
    value
        .as_object_mut()
        .unwrap()
        .remove("strike_fallback_height_m");
    let path = f.root.join("old.json");
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let old = workspace::load(&f.manifest, &path).unwrap();
    assert!(old.dataset_options.is_empty());
    assert_eq!(old.strike_fallback_height_m, None);
    value["unrecognized_scoring_assumption"] = Value::Bool(true);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(workspace::load(&f.manifest, &path).is_err());
}

#[cfg(windows)]
#[test]
fn a_windows_junction_cannot_redirect_new_review_exports() {
    use std::os::windows::process::CommandExt;
    let f = Fixture::new();
    let outside = f.root.parent().unwrap().join(format!(
        "junction-target-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&outside).unwrap();
    let link = f.root.join("workspace-reviews");
    let status = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(link.to_string_lossy().replace('/', "\\"))
        .arg(outside.to_string_lossy().replace('/', "\\"))
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "Cannot create test junction: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let error = workspace::save(&f.manifest, &f.document).unwrap_err();
    assert!(error.contains("reparse") || error.contains("link"));
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
}
