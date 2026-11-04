//! No windows, media, provider data or subprocesses are opened by these controls.
use super::*;
use geosonnar::casework::{CaseMedia, CaseRecording, SavedCase};

fn case() -> SavedCase {
    SavedCase {
        schema_version: 1,
        case_id: "history-message-control".into(),
        title: "Fabricated history lifecycle control".into(),
        created_unix_s: 1,
        local_only: true,
        recording: CaseRecording {
            path: "never-opened.raw".into(),
            sha256: "a".repeat(64),
            bytes: 1,
            media: CaseMedia {
                duration_s: 30.0,
                sample_rate_hz: 4000,
                channels: 1,
                audio_codec: "fabricated".into(),
            },
        },
    }
}
fn studio() -> Studio {
    let case = case();
    let document = ReviewWorkspace::new(&case, 0);
    Studio {
        document: Some(document.clone()),
        saved: Some(document),
        run_history: HistoryState {
            current_saved_id: Some("displayed-original".into()),
            listed: true,
            ..HistoryState::default()
        },
        ..Studio::default()
    }
}

fn failed(studio: &Studio, label: &str) -> RunCompletion {
    let mut document = studio.document.clone().unwrap();
    document.notes = label.into();
    RunCompletion {
        original_manifest: PathBuf::from("never-opened-original-case.json"),
        document,
        inputs_sha256: "b".repeat(64),
        outcome: Err(format!("Original diagnostic {label}")),
        cancelled: true,
        persistence: Err("Synthetic storage failure".into()),
        joint_request: None,
    }
}

fn saved(mut completion: RunCompletion, id: &str) -> RunCompletion {
    completion.persistence = Ok(RunSummary {
        id: id.into(),
        created_unix_ms: 1,
        status: RunStatus::Cancelled,
        inputs_sha256: completion.inputs_sha256.clone(),
        document_sha256: geosonnar::regional_search::document_sha256(&completion.document).unwrap(),
        has_result: false,
        error: Some("Original diagnostic retained".into()),
    });
    completion
}

#[test]
fn stale_failed_completion_preserves_current_display_and_running_flags() {
    let mut studio = studio();
    let previous = studio.document.clone();
    studio.joint.running = true;
    studio.regional.running = true;
    studio.joint.run_id = 17;
    let completion = failed(&studio, "old run");
    studio.accept_stale_run_completion(completion);
    assert_eq!(studio.document, previous);
    assert_eq!(
        studio.run_history.current_saved_id.as_deref(),
        Some("displayed-original")
    );
    assert!(studio.joint.running && studio.regional.running);
    assert_eq!(studio.joint.run_id, 17);
    assert!(studio.pending_action.is_none());
    assert!(!studio.run_history.listed && studio.has_unsaved_results());
    assert_eq!(
        studio
            .run_history
            .unsaved
            .as_ref()
            .unwrap()
            .outcome
            .as_ref()
            .unwrap_err(),
        "Original diagnostic old run"
    );
}

#[test]
fn two_late_storage_failures_retain_both_original_outcomes_in_order() {
    let mut studio = studio();
    let first = failed(&studio, "first");
    let second = failed(&studio, "second");
    studio.accept_stale_run_completion(first);
    studio.accept_stale_run_completion(second);
    assert_eq!(
        studio.run_history.unsaved.as_ref().unwrap().document.notes,
        "first"
    );
    assert_eq!(studio.run_history.unsaved_backlog.len(), 1);
    assert_eq!(
        studio
            .run_history
            .unsaved_backlog
            .front()
            .unwrap()
            .document
            .notes,
        "second"
    );
    assert!(studio.has_unsaved_results());
}

#[test]
fn explicit_retry_releases_only_its_original_queue_head() {
    let mut studio = studio();
    let previous = studio.document.clone();
    let first = failed(&studio, "first");
    let second = failed(&studio, "second");
    studio.accept_stale_run_completion(first.clone());
    studio.accept_stale_run_completion(second);
    studio.joint.running = true;
    studio.regional.running = true;
    studio.accept_run_retry(first.clone());
    assert_eq!(
        studio.run_history.unsaved_backlog.len(),
        1,
        "Failed retries must not duplicate an outcome"
    );
    studio.accept_run_retry(saved(first, "saved-first"));
    assert!(studio.has_unsaved_results());
    assert_eq!(
        studio.run_history.unsaved.as_ref().unwrap().document.notes,
        "second"
    );
    assert!(studio.run_history.unsaved_backlog.is_empty());
    assert_eq!(studio.document, previous);
    assert_eq!(
        studio.run_history.current_saved_id.as_deref(),
        Some("displayed-original")
    );
    assert!(studio.joint.running && studio.regional.running);
    assert!(studio.pending_action.is_none());
}

#[test]
fn unrelated_saved_completion_does_not_erase_an_unsaved_original() {
    let mut studio = studio();
    let old = failed(&studio, "must retain");
    studio.accept_stale_run_completion(old);
    let unrelated = saved(failed(&studio, "unrelated"), "saved-other");
    studio.accept_run_completion(unrelated);
    assert!(studio.has_unsaved_results());
    assert_eq!(
        studio.run_history.unsaved.as_ref().unwrap().document.notes,
        "must retain"
    );
    assert_eq!(
        studio.run_history.current_saved_id.as_deref(),
        Some("displayed-original")
    );
}

#[test]
fn crossed_case_unsaved_retry_writes_only_to_its_original_case() {
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
    std::fs::create_dir_all(&parent).unwrap();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = parent.join(format!(
        "history-original-case-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir(&root).unwrap();
    let original_dir = root.join("original");
    let current_dir = root.join("current");
    std::fs::create_dir(&original_dir).unwrap();
    std::fs::create_dir(&current_dir).unwrap();
    let original_manifest = original_dir.join("case.json");
    let current_manifest = current_dir.join("case.json");
    let original_case = case();
    let mut current_case = original_case.clone();
    current_case.case_id = "different-current-case".into();
    std::fs::write(
        &original_manifest,
        serde_json::to_vec(&original_case).unwrap(),
    )
    .unwrap();
    std::fs::write(
        &current_manifest,
        serde_json::to_vec(&current_case).unwrap(),
    )
    .unwrap();
    let mut studio = studio();
    let mut old = failed(&studio, "original case's late outcome");
    old.original_manifest = original_manifest.clone();
    let current_document = ReviewWorkspace::new(&current_case, 0);
    studio.document = Some(current_document.clone());
    studio.saved = Some(current_document.clone());
    studio.case = Some(CaseOverview {
        manifest: current_manifest,
        recording_path: current_dir.join("never-opened.raw"),
        recording_verified: Some(false),
        recording_problem: None,
        hints: vec![],
        case: current_case,
    });
    studio.accept_stale_run_completion(old);
    let completion = studio.run_history.unsaved.clone().unwrap().retry_history();
    let id = completion.persistence.as_ref().unwrap().id.clone();
    let loaded = regional_history::load(&original_manifest, &id).unwrap();
    assert_eq!(loaded.document.notes, "original case's late outcome");
    assert!(!current_dir.join("regional-runs").exists());
    studio.accept_run_retry(completion);
    assert!(!studio.has_unsaved_results());
    assert_eq!(studio.document.as_ref(), Some(&current_document));
    assert_eq!(
        studio.run_history.current_saved_id.as_deref(),
        Some("displayed-original")
    );
    drop(studio);
    let checked_root = root.canonicalize().unwrap();
    assert_eq!(
        checked_root.parent(),
        Some(parent.canonicalize().unwrap().as_path())
    );
    let mut pending = vec![checked_root.clone()];
    while let Some(path) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert!(!metadata.file_type().is_symlink());
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            assert_eq!(metadata.file_attributes() & 0x400, 0);
        }
        if metadata.is_dir() {
            pending.extend(
                std::fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
        }
    }
    std::fs::remove_dir_all(checked_root).unwrap();
}
