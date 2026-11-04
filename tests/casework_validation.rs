use geosonnar::{
    casework::{self, CaseMedia, CaseRecording, SavedCase},
    detailed_data::sha256,
    verification::write_json,
};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Workspace {
    root: PathBuf,
    path: PathBuf,
}
impl Workspace {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let path = root.join(format!(
            "saved-case-control-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&path).unwrap();
        Self { root, path }
    }
    fn fixture(&self) -> PathBuf {
        fs::write(
            self.path.join("recording.wav"),
            b"fabricated source bytes for receipt tests",
        )
        .unwrap();
        let directory = self.path.join("case");
        fs::create_dir(&directory).unwrap();
        let source = self.path.join("recording.wav");
        let record = SavedCase {
            schema_version: 1,
            case_id: "control-001".into(),
            title: "Fabricated case".into(),
            created_unix_s: 1,
            local_only: true,
            recording: CaseRecording {
                path: PathBuf::from("../recording.wav"),
                sha256: sha256(&source).unwrap(),
                bytes: fs::metadata(&source).unwrap().len(),
                media: CaseMedia {
                    duration_s: 10.0,
                    sample_rate_hz: 8000,
                    channels: 1,
                    audio_codec: "pcm_s16le".into(),
                },
            },
        };
        let manifest = directory.join("case.json");
        write_json(&manifest, &record).unwrap();
        manifest
    }
    fn clue(&self) -> PathBuf {
        let path = self.path.join("clue.json");
        write_json(&path,&json!({"kind":"region","text":"Possibly a broad northern region or nearby; unverified", "source":"user","certainty":"uncertain"})).unwrap();
        path
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        if let Ok(p) = self.path.canonicalize() {
            assert!(p.starts_with(&self.root) && p != self.root);
            let _ = fs::remove_dir_all(p);
        }
    }
}

#[test]
fn relative_recording_identity_survives_moving_the_whole_layout() {
    let w = Workspace::new();
    let case = w.fixture();
    assert!(casework::read_case(&case, true).is_ok());
    let moved = w.path.join("relocated");
    fs::create_dir(&moved).unwrap();
    fs::rename(w.path.join("recording.wav"), moved.join("recording.wav")).unwrap();
    fs::rename(w.path.join("case"), moved.join("case")).unwrap();
    assert!(casework::read_case(&moved.join("case/case.json"), true).is_ok());
}

#[test]
fn same_length_recording_mutation_prevents_reuse_but_status_stays_readable() {
    let w = Workspace::new();
    let case = w.fixture();
    let source = w.path.join("recording.wav");
    let mut bytes = fs::read(&source).unwrap();
    bytes[0] ^= 1;
    fs::write(&source, bytes).unwrap();
    assert!(
        casework::read_case(&case, true)
            .unwrap_err()
            .contains("changed")
    );
    let status = casework::status(&case).unwrap();
    assert_eq!(status["recording_matches"], false);
    assert_eq!(status["location_verified"], false);
    fs::remove_file(source).unwrap();
    assert_eq!(casework::status(&case).unwrap()["recording_matches"], false);
}

#[test]
fn clue_is_append_only_and_never_becomes_a_search_bound() {
    let w = Workspace::new();
    let case = w.fixture();
    let before = sha256(&case).unwrap();
    let request = w.clue();
    let first = casework::add_hint(&case, &request).unwrap();
    let original = fs::read(&first).unwrap();
    let second = casework::add_hint(&case, &request).unwrap();
    assert_ne!(first, second);
    assert_eq!(fs::read(&first).unwrap(), original);
    assert_eq!(sha256(&case).unwrap(), before);
    let clues = casework::hints(&case).unwrap();
    assert_eq!(clues.len(), 2);
    let status = casework::status(&case).unwrap();
    assert_eq!(status["recording_matches"], true);
    assert!(status.get("bounds").is_none());
    assert_eq!(status["location_verified"], false);
}

#[test]
fn changed_case_identity_cannot_inherit_previous_hints() {
    let w = Workspace::new();
    let case = w.fixture();
    casework::add_hint(&case, &w.clue()).unwrap();
    let mut value: SavedCase = casework::read_case(&case, false).unwrap();
    value.title = "A different revision".into();
    write_json(&case, &value).unwrap();
    assert!(casework::hints(&case).unwrap_err().contains("immutable"));
}

#[test]
fn clues_can_be_recorded_offline_without_claiming_source_verification() {
    let w = Workspace::new();
    let case = w.fixture();
    fs::remove_file(w.path.join("recording.wav")).unwrap();
    casework::add_hint(&case, &w.clue()).unwrap();
    let state = casework::status(&case).unwrap();
    assert_eq!(state["recording_matches"], false);
    assert_eq!(state["hints"].as_array().unwrap().len(), 1);
}

#[test]
fn unsupported_hint_fields_cannot_smuggle_truth_or_hard_bounds() {
    let w = Workspace::new();
    let case = w.fixture();
    let request = w.clue();
    write_json(&request,&json!({"kind":"region","text":"A clue","source":"user","certainty":"uncertain","bounds":[1,2,3,4]})).unwrap();
    assert!(casework::add_hint(&case, &request).is_err());
    write_json(
        &request,
        &json!({"kind":"region","text":"A clue","source":"user","certainty":"verified"}),
    )
    .unwrap();
    assert!(casework::add_hint(&case, &request).is_err());
}

#[test]
fn corrupt_hint_directory_does_not_silently_lose_evidence() {
    let w = Workspace::new();
    let case = w.fixture();
    let path = casework::add_hint(&case, &w.clue()).unwrap();
    fs::write(path, b"incomplete").unwrap();
    assert!(casework::hints(&case).is_err());
}

#[test]
fn interrupted_staging_never_publishes_partial_hints_or_blocks_later_writes() {
    let w = Workspace::new();
    let case = w.fixture();
    let staging = case.parent().unwrap().join(".case-pending");
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("interrupted.json"), b"{\"partial\":").unwrap();
    assert!(casework::hints(&case).unwrap().is_empty());
    let hint = casework::add_hint(&case, &w.clue()).unwrap();
    assert!(serde_json::from_slice::<serde_json::Value>(&fs::read(hint).unwrap()).is_ok());
    assert_eq!(casework::hints(&case).unwrap().len(), 1);
    assert_eq!(
        fs::read(staging.join("interrupted.json")).unwrap(),
        b"{\"partial\":"
    );
    assert_eq!(casework::status(&case).unwrap()["recording_matches"], true);
}

#[test]
fn bad_recording_receipts_and_case_replacement_are_rejected() {
    let w = Workspace::new();
    let case = w.fixture();
    assert!(
        casework::create(&w.path.join("not-present"), case.parent().unwrap(), "x")
            .unwrap_err()
            .contains("new")
    );
    let mut record = casework::read_case(&case, false).unwrap();
    record.recording.sha256 = "a".repeat(63);
    write_json(&case, &record).unwrap();
    assert!(casework::read_case(&case, false).is_err());
    record.recording.sha256 = "a".repeat(64);
    record.local_only = false;
    write_json(&case, &record).unwrap();
    assert!(casework::read_case(&case, false).is_err());
}
