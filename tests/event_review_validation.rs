//! Fabricated review contracts only. No private recording or evaluation truth.
use geosonnar::{
    event_review::{self, GroupChoice, ReviewActor, ReviewDecision, ReviewSession},
    reviewed_search,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);
static RUN_ROOT: OnceLock<PathBuf> = OnceLock::new();

fn digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn save_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn move_fixture_directory(source: &Path, destination: &Path) {
    let allowed = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("output/benchmarks/event-review/integration")
        .canonicalize()
        .unwrap();
    assert!(source.canonicalize().unwrap().starts_with(&allowed));
    assert!(
        destination
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap()
            .starts_with(&allowed)
    );
    assert!(!destination.exists());
    fs::rename(source, destination).unwrap();
}

struct Fixture {
    root: PathBuf,
    case: PathBuf,
    discovery: PathBuf,
    report: Value,
}

impl Fixture {
    fn new(name: &str) -> Self {
        // Create the shared test-run ancestry once, avoiding concurrent Windows
        // create_dir_all calls racing while that ancestry is first created.
        let run = RUN_ROOT.get_or_init(|| {
            let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("output/benchmarks/event-review/integration")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            fs::create_dir_all(&directory).unwrap();
            directory
        });
        let root = run.join(format!(
            "{}-{name}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("discovery données")).unwrap();
        let recording = root.join("recording é.raw");
        fs::write(
            &recording,
            b"Fabricated bytes: no real sound, aircraft or location evidence",
        )
        .unwrap();
        let case = root.join("case.json");
        save_json(
            &case,
            &json!({
                "schema_version":1,"case_id":"fabricated-case","title":"Fabricated review validation",
                "created_unix_s":1,"local_only":true,
                "recording":{"path":"recording é.raw","sha256":digest(&recording),
                    "bytes":fs::metadata(&recording).unwrap().len(),
                    "media":{"duration_s":100.,"sample_rate_hz":48000,"channels":2,"audio_codec":"fabricated"}}
            }),
        );
        let event = |id: &str, group: &str, start: f64, harmonic: f64| {
            json!({
                "event_id":id,"harmonic_group_id":group,"start_s":start-1.,"end_s":start+10.,
                "band_hz":[90.*harmonic,140.*harmonic],
                "ridge":[
                    {"time_s":start,"frequency_hz":121.12345678901235*harmonic,"weight":1.25},
                    {"time_s":start+0.5,"frequency_hz":119.5*harmonic,"weight":2.5},
                    {"time_s":start+8.,"frequency_hz":105.25*harmonic,"weight":3.75},
                    {"time_s":start+8.5,"frequency_hz":104.*harmonic,"weight":4.}
                ],
                "quality":{"support_fraction":0.2,"fragment_count":2,"frequency_sigma_hz":0.5}
            })
        };
        let report = json!({
            "schema_version":1,"status":"events_proposed","location_verified":false,
            "source":recording,"source_sha256":digest(&recording),
            "source_metadata":{"duration_s":100.,"selected_channel_index":1,"available_channels":2,"channels_averaged":false},
            "settings":{"min_frequency_hz":40.,"max_frequency_hz":1600.,"max_gap_s":2.},
            "group_count":2,"ridge_count":3,
            "groups":[
                {"harmonic_group_id":"group-a","start_s":9.,"end_s":20.,"ridge_count":2,"review_status":"unreviewed","aircraft_identity_confirmed":false},
                {"harmonic_group_id":"group-b","start_s":29.,"end_s":40.,"ridge_count":1,"review_status":"unreviewed","aircraft_identity_confirmed":false}
            ],
            "events":[event("ridge-a","group-a",10.,1.),event("ridge-a-h2","group-a",10.,2.),event("ridge-b","group-b",30.,1.)],
            "limitations":["Fabricated fixture; partial measured support is not interpolated"]
        });
        let fixture = Self {
            discovery: root.join("discovery données/discovery.json"),
            root,
            case,
            report,
        };
        fixture.write();
        fixture
    }

    fn write(&self) {
        save_json(&self.discovery, &self.report);
        for event in self.report["events"].as_array().unwrap() {
            let mut csv = String::from("time_s,frequency_hz,weight\n");
            for point in event["ridge"].as_array().unwrap() {
                csv.push_str(&format!(
                    "{},{},{}\n",
                    point["time_s"], point["frequency_hz"], point["weight"]
                ));
            }
            fs::write(self.csv(event["event_id"].as_str().unwrap()), csv).unwrap();
        }
    }

    fn csv(&self, id: &str) -> PathBuf {
        self.discovery.parent().unwrap().join(format!("{id}.csv"))
    }

    fn load(&self) -> Result<ReviewSession, String> {
        event_review::load(&self.case, &self.discovery, &digest(&self.discovery), 1)
    }

    fn choices(&self) -> Vec<GroupChoice> {
        vec![
            GroupChoice {
                harmonic_group_id: "group-a".into(),
                decision: ReviewDecision::KeepForMatching,
                representative_event_id: Some("ridge-a-h2".into()),
            },
            GroupChoice {
                harmonic_group_id: "group-b".into(),
                decision: ReviewDecision::KeepForMatching,
                representative_event_id: Some("ridge-b".into()),
            },
        ]
    }
}

#[test]
fn exact_measured_support_and_gap_slices_are_exposed_without_harmonic_blending() {
    let fixture = Fixture::new("gaps");
    let session = fixture.load().unwrap();
    fn send_and_clone<T: Send + Clone>(_: &T) {}
    send_and_clone(&session);
    assert_eq!(session.overview().selected_channel_index, 1);
    assert_eq!(session.overview().measured_point_count, 12);
    assert!(!session.overview().location_verified);
    assert_eq!(session.groups().len(), 2);
    for (ridge, expected) in session
        .groups()
        .iter()
        .flat_map(|g| &g.ridges)
        .zip(fixture.report["events"].as_array().unwrap())
    {
        assert_eq!(
            serde_json::to_value(&ridge.points).unwrap(),
            expected["ridge"]
        );
        assert_eq!(ridge.segments.len(), 2);
        assert_eq!(
            (
                ridge.segments[0].start_index,
                ridge.segments[0].end_index_exclusive
            ),
            (0, 2)
        );
        assert_eq!(
            (
                ridge.segments[1].start_index,
                ridge.segments[1].end_index_exclusive
            ),
            (2, 4)
        );
    }
    assert!(
        session.initial_choices().iter().all(
            |c| c.decision == ReviewDecision::Unreviewed && c.representative_event_id.is_none()
        )
    );
}

#[test]
fn discovery_import_does_not_assess_sources_or_overwrite_existing_analyst_assessment() {
    use geosonnar::workspace::{AcousticSourceAssessment, ReviewState, ReviewWorkspace};
    let fixture = Fixture::new("source-assessment");
    let session = fixture.load().unwrap();
    let case = geosonnar::casework::read_case(&fixture.case, true).unwrap();
    let mut document = ReviewWorkspace::new(&case, 1);
    assert_eq!(document.import_discovery(&session).unwrap(), 3);
    assert!(document.events.iter().all(|event| {
        event.acoustic_source == AcousticSourceAssessment::Unassessed
            && event.review == ReviewState::Pending
            && !event.selected_for_scoring
    }));
    document.events[0].acoustic_source = AcousticSourceAssessment::Incompatible;
    let before = document.clone();
    assert_eq!(document.import_discovery(&session).unwrap(), 0);
    assert_eq!(document, before);
}

#[test]
fn case_recording_channel_and_exact_discovery_fingerprint_must_match() {
    let fixture = Fixture::new("identity");
    assert!(
        event_review::load(&fixture.case, &fixture.discovery, &"0".repeat(64), 1)
            .unwrap_err()
            .contains("fingerprint")
    );
    assert!(
        event_review::load(
            &fixture.case,
            &fixture.discovery,
            &digest(&fixture.discovery),
            0
        )
        .is_err()
    );
    let mut different_case = read_json(&fixture.case);
    let other_recording = fixture.root.join("other.raw");
    fs::write(&other_recording, b"different fabricated recording").unwrap();
    different_case["recording"]["path"] = "other.raw".into();
    different_case["recording"]["sha256"] = digest(&other_recording).into();
    different_case["recording"]["bytes"] = fs::metadata(other_recording).unwrap().len().into();
    let other_case = fixture.root.join("other-case.json");
    save_json(&other_case, &different_case);
    assert!(
        event_review::load(
            &other_case,
            &fixture.discovery,
            &digest(&fixture.discovery),
            1
        )
        .is_err()
    );
    fs::write(
        fixture.root.join("recording é.raw"),
        b"changed original bytes",
    )
    .unwrap();
    assert!(fixture.load().is_err());
}

#[test]
fn exact_fingerprint_does_not_excuse_invalid_discovery_schema_or_measurements() {
    for variant in 0..10 {
        let mut fixture = Fixture::new(&format!("invalid-{variant}"));
        match variant {
            0 => fixture.report["source_metadata"]["channels_averaged"] = true.into(),
            1 => fixture.report["source_metadata"]["available_channels"] = 3.into(),
            2 => fixture.report["location_verified"] = true.into(),
            3 => fixture.report["group_count"] = 1.into(),
            4 => fixture.report["events"][0]["ridge"][1]["time_s"] = 10.0.into(),
            5 => fixture.report["events"][0]["ridge"][1]["frequency_hz"] = 1000.0.into(),
            6 => fixture.report["events"][0]["quality"]["support_fraction"] = 1.1.into(),
            7 => fixture.report["events"][0]["harmonic_group_id"] = "unknown".into(),
            8 => fixture.report["events"][0]["ridge"][1]["weight"] = 0.into(),
            _ => fixture.report["events"][0]["end_s"] = 21.0.into(),
        }
        fixture.write();
        assert!(fixture.load().is_err(), "invalid variant {variant} passed");
    }
}

#[test]
fn legacy_integral_float_fragment_counts_are_accepted_but_invalid_counts_are_rejected() {
    let mut fixture = Fixture::new("numeric-counts");
    fixture.report["events"][0]["quality"]["fragment_count"] = json!(2.0);
    fixture.write();
    let session = fixture.load().unwrap();
    assert_eq!(session.groups()[0].ridges[0].segments.len(), 2);
    assert_eq!(
        session.groups()[0].ridges[0].quality["fragment_count"],
        json!(2.0)
    );
    for invalid in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(5.0),
        json!(1e100),
        json!("2"),
        Value::Null,
    ] {
        fixture.report["events"][0]["quality"]["fragment_count"] = invalid.clone();
        fixture.write();
        assert!(
            fixture.load().unwrap_err().contains("fragment_count"),
            "Accepted invalid count {invalid}"
        );
    }
}

#[test]
fn missing_partial_extra_reordered_or_changed_csv_rows_are_rejected() {
    for variant in 0..5 {
        let fixture = Fixture::new(&format!("csv-{variant}"));
        let path = fixture.csv("ridge-a");
        let csv = fs::read_to_string(&path).unwrap();
        let mut lines = csv.lines().map(str::to_string).collect::<Vec<_>>();
        match variant {
            0 => {
                fs::remove_file(&path).unwrap();
            }
            1 => {
                lines.pop();
                fs::write(&path, lines.join("\n")).unwrap();
            }
            2 => {
                fs::write(&path, format!("{csv}19,103,4\n")).unwrap();
            }
            3 => {
                lines.swap(1, 2);
                fs::write(&path, lines.join("\n")).unwrap();
            }
            _ => {
                lines[1] = "10,121,1.25".into();
                fs::write(&path, lines.join("\n")).unwrap();
            }
        }
        assert!(fixture.load().is_err(), "CSV variant {variant} passed");
    }
}

#[test]
fn event_ids_cannot_request_parent_paths_or_windows_streams() {
    for id in [
        "../outside",
        "..\\outside",
        "C:\\outside",
        "ridge:secret",
        "/outside",
    ] {
        let mut fixture = Fixture::new("traversal");
        fixture.report["events"][0]["event_id"] = id.into();
        // Do not create the hostile path; the loader must reject the ID before
        // looking up any CSV. The report hash remains otherwise valid.
        save_json(&fixture.discovery, &fixture.report);
        assert!(fixture.load().unwrap_err().contains("ridge ID"));
    }
}

#[cfg(unix)]
#[test]
fn a_csv_symlink_cannot_escape_the_discovery_directory() {
    let fixture = Fixture::new("symlink");
    let csv = fixture.csv("ridge-a");
    let outside = fixture.root.join("outside.csv");
    fs::rename(&csv, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &csv).unwrap();
    assert!(fixture.load().unwrap_err().contains("outside"));
}

#[test]
fn saved_reviews_are_new_schema_two_files_and_reopen_without_changing_discovery() {
    let fixture = Fixture::new("roundtrip");
    let session = fixture.load().unwrap();
    let original = digest(&fixture.discovery);
    let choices = fixture.choices();
    let output = fixture.root.join("review 001");
    let saved = event_review::save(&session, &choices, ReviewActor::Human, &output).unwrap();
    assert_eq!(
        event_review::load_choices(&session, &output).unwrap(),
        choices
    );
    assert_eq!(digest(&fixture.discovery), original);
    let review = read_json(&saved.review_selection);
    assert_eq!(review["schema_version"], 2);
    assert_eq!(review["review_actor"], "human");
    assert_eq!(review["discovery_sha256"], original);
    assert_eq!(
        review["groups"][0]["event_ids"],
        json!(["ridge-a", "ridge-a-h2"])
    );
    assert!(review["groups"][0].get("representative_event_id").is_none());
    assert_eq!(
        read_json(&saved.receipt)["choices"][0]["representative_event_id"],
        "ridge-a-h2"
    );
    let before = fs::read(&saved.review_selection).unwrap();
    assert!(
        event_review::save(
            &session,
            &session.initial_choices(),
            ReviewActor::Human,
            &output
        )
        .is_err()
    );
    assert_eq!(fs::read(&saved.review_selection).unwrap(), before);
    // Keeping an acoustic proposal without a representative is a valid review,
    // but is deliberately not sufficient to start the two-event pipeline.
    let mut unselected = session.initial_choices();
    unselected[0].decision = ReviewDecision::KeepForMatching;
    event_review::save(
        &session,
        &unselected,
        ReviewActor::Human,
        &fixture.root.join("review 002"),
    )
    .unwrap();
    assert!(event_review::selected_pair(&session, &unselected).is_err());
}

#[test]
fn all_groups_and_explicit_representative_membership_are_required() {
    let fixture = Fixture::new("choices");
    let session = fixture.load().unwrap();
    for variant in 0..4 {
        let mut choices = fixture.choices();
        match variant {
            0 => {
                choices.pop();
            }
            1 => choices.push(choices[0].clone()),
            2 => choices[0].representative_event_id = Some("ridge-b".into()),
            _ => choices[0].decision = ReviewDecision::Reject,
        }
        let out = fixture.root.join(format!("invalid-{variant}"));
        assert!(event_review::save(&session, &choices, ReviewActor::Human, &out).is_err());
        assert!(!out.exists());
    }
    let mut reverse = fixture.choices();
    reverse.reverse();
    assert!(event_review::selected_pair(&session, &reverse).is_err());
    let pair = event_review::selected_pair(&session, &fixture.choices()).unwrap();
    assert_eq!(pair[0].event_id, "ridge-a-h2");
}

#[test]
fn changed_evidence_cannot_be_saved_from_a_stale_ui_snapshot() {
    for variant in 0..4 {
        let fixture = Fixture::new(&format!("stale-{variant}"));
        let session = fixture.load().unwrap();
        let path = match variant {
            0 => fixture.discovery.clone(),
            1 => fixture.case.clone(),
            2 => fixture.root.join("recording é.raw"),
            _ => fixture.csv("ridge-a"),
        };
        let mut bytes = fs::read(&path).unwrap();
        bytes.push(b' ');
        fs::write(path, bytes).unwrap();
        let output = fixture.root.join("stale-review");
        assert!(
            event_review::save(&session, &fixture.choices(), ReviewActor::Human, &output).is_err()
        );
        assert!(!output.exists());
    }
}

#[test]
fn reopening_rejects_changed_review_choices_receipts_and_foreign_case_binding() {
    for variant in 0..4 {
        let fixture = Fixture::new(&format!("reopen-{variant}"));
        let session = fixture.load().unwrap();
        let output = fixture.root.join("review");
        let saved =
            event_review::save(&session, &fixture.choices(), ReviewActor::Human, &output).unwrap();
        match variant {
            0 => {
                let mut review = read_json(&saved.review_selection);
                review["groups"][0]["review"] = "reject".into();
                save_json(&saved.review_selection, &review);
            }
            1 => {
                let mut receipt = read_json(&saved.receipt);
                receipt["choices"][0]["representative_event_id"] = "ridge-b".into();
                save_json(&saved.receipt, &receipt);
            }
            2 => {
                let mut receipt = read_json(&saved.receipt);
                receipt["discovery"]["sha256"] = "0".repeat(64).into();
                save_json(&saved.receipt, &receipt);
            }
            _ => {
                let other = Fixture::new("foreign");
                assert!(event_review::load_choices(&other.load().unwrap(), &output).is_err());
                continue;
            }
        }
        assert!(event_review::load_choices(&session, &output).is_err());
    }
}

#[test]
fn identical_review_and_relative_case_workspaces_can_be_relocated() {
    let fixture = Fixture::new("portable");
    let session = fixture.load().unwrap();
    let choices = fixture.choices();
    let original_review = fixture.root.join("review");
    event_review::save(&session, &choices, ReviewActor::Human, &original_review).unwrap();
    let moved_review = fixture.root.join("review moved");
    move_fixture_directory(&original_review, &moved_review);
    assert_eq!(
        event_review::load_choices(&session, &moved_review).unwrap(),
        choices
    );

    let new_root = fixture.root.with_file_name(format!(
        "{}-relocated",
        fixture.root.file_name().unwrap().to_str().unwrap()
    ));
    let relative_discovery = fixture.discovery.strip_prefix(&fixture.root).unwrap();
    let original_discovery_digest = digest(&fixture.discovery);
    move_fixture_directory(&fixture.root, &new_root);
    let moved_session = event_review::load(
        &new_root.join("case.json"),
        &new_root.join(relative_discovery),
        &original_discovery_digest,
        1,
    )
    .unwrap();
    assert_eq!(
        event_review::load_choices(&moved_session, &new_root.join("review moved")).unwrap(),
        choices
    );
    // Reopen is allowed through a newly verified session. An already-open
    // snapshot whose live source paths disappeared must still refuse saving.
    assert!(
        event_review::save(
            &session,
            &choices,
            ReviewActor::Human,
            &new_root.join("stale")
        )
        .is_err()
    );
}

#[test]
fn completed_empty_discovery_can_be_reviewed_without_claiming_no_aircraft() {
    let mut fixture = Fixture::new("empty");
    fixture.report["status"] = "no_events".into();
    fixture.report["group_count"] = 0.into();
    fixture.report["ridge_count"] = 0.into();
    fixture.report["groups"] = json!([]);
    fixture.report["events"] = json!([]);
    fixture.write();
    let session = fixture.load().unwrap();
    assert!(session.groups().is_empty());
    let output = fixture.root.join("empty-review");
    event_review::save(&session, &[], ReviewActor::Human, &output).unwrap();
    assert!(
        event_review::load_choices(&session, &output)
            .unwrap()
            .is_empty()
    );
    assert!(!session.overview().location_verified);
}

#[test]
fn native_review_is_consumed_by_existing_reviewed_search_with_exact_selected_samples() {
    let fixture = Fixture::new("downstream");
    let session = fixture.load().unwrap();
    let choices = fixture.choices();
    let saved = event_review::save(
        &session,
        &choices,
        ReviewActor::AutomatedTest,
        &fixture.root.join("review"),
    )
    .unwrap();
    let trace = |start: f64| {
        json!({"points":[
        {"t_rec_s":start,"frequency_hz":111.},{"t_rec_s":start+1.,"frequency_hz":110.},
        {"t_rec_s":start+2.,"frequency_hz":109.}],"f0_hz":[110.]})
    };
    let template = json!({
        "schema_version":1,"evidence_kind":"real_recording",
        "provenance":["Fabricated contract data, no actual recording or location evidence"],
        "tracks":[{"flight_id":"fabricated","points":[
            {"t_s":0.,"xyz_m":[6379637.,-1000.,0.]},{"t_s":20.,"xyz_m":[6379637.,0.,0.]},
            {"t_s":40.,"xyz_m":[6379637.,1000.,0.]}]}],
        "observers":[{"id":"fabricated","latitude_deg":0.,"longitude_deg":0.,"ellipsoid_height_m":0.}],
        "events":[trace(10.),trace(30.)],
        "pair_config":{"event1_cpa_recording_s":12.,"shared_shifts_s":[-1.,0.,1.],"clock_offset_bounds_s":[-100.,100.],
            "model":{"sound_speed_mps":343.,"max_gap_s":30.}},
        "coarse_bounds":[{"speed_mps":[30.,90.],"slant_distance_m":[1000.,2000.]},{"speed_mps":[30.,90.],"slant_distance_m":[1000.,2000.]}],
        "measured_separation_s":20.,"separation_tolerance_s":1.
    });
    save_json(&fixture.root.join("template.json"), &template);
    let config = fixture.root.join("bridge.json");
    save_json(
        &config,
        &json!({
            "schema_version":1,"discovery":fixture.discovery,"review_selection":saved.review_selection,
            "template_search_input":"template.json","selected_ridges":[
                {"harmonic_group_id":"group-a","event_id":"ridge-a-h2","f0_hz":[220.,221.]},
                {"harmonic_group_id":"group-b","event_id":"ridge-b","f0_hz":[110.,111.]}
            ],"selection_basis":"Explicit fabricated selection before flight scoring; not evidence"
        }),
    );
    let output = fixture.root.join("prepared");
    reviewed_search::prepare(&config, &output).unwrap();
    let input = read_json(&output.join("search_input.json"));
    for (index, original_index) in [1, 2].into_iter().enumerate() {
        let original = fixture.report["events"][original_index]["ridge"]
            .as_array()
            .unwrap();
        let copied = input["events"][index]["points"].as_array().unwrap();
        assert_eq!(copied.len(), original.len());
        for (actual, expected) in copied.iter().zip(original) {
            assert_eq!(actual["t_rec_s"], expected["time_s"]);
            assert_eq!(actual["frequency_hz"], expected["frequency_hz"]);
        }
    }
}

#[test]
#[ignore = "Explicit compatibility audit of existing workspace reports; set GEOSONNAR_TEST_DISCOVERY_REPORTS, requires FFprobe"]
fn existing_generated_discovery_reports_load_without_modifying_evidence() {
    let requested = std::env::var_os("GEOSONNAR_TEST_DISCOVERY_REPORTS")
        .expect("Supply existing discovery reports through GEOSONNAR_TEST_DISCOVERY_REPORTS");
    let paths = std::env::split_paths(&requested).collect::<Vec<_>>();
    assert!(!paths.is_empty() && paths.len() <= 10);
    for path in paths {
        let before = digest(&path);
        let report = read_json(&path);
        let source = PathBuf::from(report["source"].as_str().unwrap());
        let work = Fixture::new("existing-generated-report");
        let case_directory = work.root.join("actual source case");
        geosonnar::casework::create(
            &source,
            &case_directory,
            "Existing discovery compatibility audit",
        )
        .unwrap();
        let session = event_review::load(
            &case_directory.join("case.json"),
            &path,
            &before,
            report["source_metadata"]["selected_channel_index"]
                .as_u64()
                .unwrap() as usize,
        )
        .unwrap();
        assert_eq!(
            session.overview().ridge_count,
            report["events"].as_array().unwrap().len()
        );
        assert_eq!(digest(&path), before);
        let receipt = work.root.join("compatibility.json");
        save_json(
            &receipt,
            &json!({"status":"passed","location_verified":false,
            "discovery":path,"discovery_sha256":before,"overview":session.overview()}),
        );
        println!("Compatibility audit: {}", receipt.display());
    }
}
