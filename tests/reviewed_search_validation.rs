//! Review-to-search boundary checks. Tiny fabricated reports exercise contracts;
//! only the independent forward-time fixture below exercises matching physics.
//! None of these fixtures is evidence of a real aircraft or location.
use geosonnar::verification;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);
static RUN_ROOT: OnceLock<PathBuf> = OnceLock::new();

fn directory(name: &str) -> PathBuf {
    let run = RUN_ROOT.get_or_init(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("output/benchmarks/reviewed-search/integration")
            .join(format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&path).unwrap();
        path
    });
    let path = run.join(format!(
        "{name}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn save(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn load(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

// Independently invoke the platform utility rather than trusting the production
// helper to supply its own expected digest.
fn sha256(path: &Path) -> String {
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("certutil.exe");
        cmd.args(["-hashfile"]).arg(path).arg("SHA256");
        cmd.creation_flags(0x08000000);
        cmd
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut cmd = Command::new("sha256sum");
        cmd.arg("--").arg(path);
        cmd
    };
    let output = command.stdin(Stdio::null()).output().unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .find(|word| word.len() == 64 && word.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap()
        .to_ascii_lowercase()
}

struct Fixture {
    root: PathBuf,
    discovery: Value,
    review: Value,
    template: Value,
    config: Value,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = directory(name);
        let source = root.join("original source é.raw");
        fs::write(
            &source,
            b"synthetic contract bytes, not an acoustic recording",
        )
        .unwrap();
        let source_hash = sha256(&source);
        let event = |id: &str, group: &str, start: f64, scale: f64| {
            json!({
                "event_id":id,"harmonic_group_id":group,"start_s":start-1.,"end_s":start+9.,
                "band_hz":[90.*scale,130.*scale],
                "ridge":[
                    {"time_s":start,"frequency_hz":120.*scale,"weight":2.5},
                    {"time_s":start+0.5,"frequency_hz":118.5*scale,"weight":3.5},
                    {"time_s":start+8.,"frequency_hz":105.25*scale,"weight":5.5}
                ],
                "quality":{"support_fraction":0.3,"fragment_count":2,"harmonic_count":2,
                    "frequency_sigma_hz":0.5,"frequency_uncertainty_hz":0.5,"time_uncertainty_s":1.024}
            })
        };
        let discovery = json!({
            "schema_version":1,"status":"events_proposed","location_verified":false,
            "source":source,"source_sha256":source_hash,"group_count":2,"ridge_count":3,
            "source_metadata":{"duration_s":100.,"selected_channel_index":1,"available_channels":2,
                "channels_averaged":false,"decoded_samples":400000,"sample_rate_hz":4000},
            "settings":{"min_frequency_hz":40.,"max_frequency_hz":1600.,"max_gap_s":2.},
            "events":[event("ridge-a","group-a",10.,1.),event("ridge-a-harmonic","group-a",10.,2.),event("ridge-b","group-b",30.,1.)],
            "groups":[
                {"harmonic_group_id":"group-a","start_s":9.,"end_s":19.,"ridge_count":2,"review_status":"unreviewed","aircraft_identity_confirmed":false},
                {"harmonic_group_id":"group-b","start_s":29.,"end_s":39.,"ridge_count":1,"review_status":"unreviewed","aircraft_identity_confirmed":false}
            ],"limitations":["Fabricated test data, not aircraft evidence"]
        });
        let review = json!({
            "schema_version":2,"discovery_sha256":"will be set from bytes","review_actor":"automated_test",
            "source_sha256":source_hash,"selected_channel_index":1,"location_verified":false,
            "scope":"Fabricated contract fixture, not human review or aircraft evidence",
            "groups":[
                {"harmonic_group_id":"group-a","review":"keep_for_matching","event_ids":["ridge-a","ridge-a-harmonic"]},
                {"harmonic_group_id":"group-b","review":"keep_for_matching","event_ids":["ridge-b"]}
            ]
        });
        let trace = |start: f64| {
            json!({"points":[
            {"t_rec_s":start,"frequency_hz":111.},
            {"t_rec_s":start+1.,"frequency_hz":110.},
            {"t_rec_s":start+2.,"frequency_hz":109.}],"f0_hz":[110.]})
        };
        let template = json!({
            "schema_version":1,"evidence_kind":"real_recording",
            "provenance":["Fabricated test template: no actual recording or location evidence"],
            "tracks":[{"flight_id":"candidate-track","points":[
                {"t_s":0.,"xyz_m":[6379637.,-1000.,0.]},
                {"t_s":20.,"xyz_m":[6379637.,0.,0.]},
                {"t_s":40.,"xyz_m":[6379637.,1000.,0.]}]}],
            "observers":[{"id":"test-observer","latitude_deg":0.,"longitude_deg":0.,"ellipsoid_height_m":0.}],
            "events":[trace(10.),trace(30.)],
            "pair_config":{"event1_cpa_recording_s":12.,"shared_shifts_s":[-1.,0.,1.],"clock_offset_bounds_s":[-100.,100.],
                "model":{"sound_speed_mps":343.,"max_gap_s":30.}},
            "coarse_bounds":[{"speed_mps":[30.,90.],"slant_distance_m":[1000.,2000.]},{"speed_mps":[30.,90.],"slant_distance_m":[1000.,2000.]}],
            "measured_separation_s":20.,"separation_tolerance_s":1.
        });
        let config = json!({
            "schema_version":1,"discovery":"discovery report.json","review_selection":"review selection.json",
            "template_search_input":"template input.json",
            "selected_ridges":[
                {"harmonic_group_id":"group-a","event_id":"ridge-a","f0_hz":[108.,109.,110.]},
                {"harmonic_group_id":"group-b","event_id":"ridge-b","f0_hz":[107.,108.,109.]}
            ],"selection_basis":"Automated contract test, selected before any flight scoring"
        });
        Self {
            root,
            discovery,
            review,
            template,
            config,
        }
    }

    fn save(&mut self) {
        save(&self.root.join("discovery report.json"), &self.discovery);
        self.review["discovery_sha256"] = sha256(&self.root.join("discovery report.json")).into();
        save(&self.root.join("review selection.json"), &self.review);
        save(&self.root.join("template input.json"), &self.template);
        save(&self.root.join("bridge config.json"), &self.config);
    }

    fn run(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_geosonnar"))
            .arg("prepare-reviewed-search")
            .arg("--config")
            .arg(self.root.join("bridge config.json"))
            .arg("--out")
            .arg(self.root.join("prepared output"))
            // Deliberately run outside config directory, proving relative inputs
            // are resolved against the config and not the process directory.
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap()
    }

    fn success(&self) -> Value {
        let output = self.run();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(self.root.join("prepared output/receipt.json").is_file());
        assert!(!self.root.join("prepared output/failure.json").exists());
        load(&self.root.join("prepared output/search_input.json"))
    }

    fn failure(&self) -> String {
        let output = self.run();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let out = self.root.join("prepared output");
        for name in [
            "search_input.json",
            "event-1.json",
            "event-2.json",
            "receipt.json",
        ] {
            assert!(!out.join(name).exists(), "Invalid input published {name}");
        }
        if out.join("failure.json").exists() {
            let receipt = load(&out.join("failure.json"));
            assert_eq!(receipt["location_verified"], false);
            assert_eq!(receipt["status"], "failed");
        }
        String::from_utf8_lossy(&output.stderr).into_owned()
    }
}

#[test]
fn reviewed_points_are_copied_exactly_without_gap_fill_or_harmonic_blending() {
    let mut fixture = Fixture::new("exact measured points");
    // This decimal exposed a one-ULP parse drift with serde_json's default
    // fast parser. Exact copying must preserve the correctly rounded value.
    fixture.discovery["events"][0]["ridge"][0]["frequency_hz"] = 1243.0421079096013_f64.into();
    fixture.discovery["events"][0]["band_hz"] = json!([100., 1244.]);
    fixture.save();
    let discovery_before = fs::read(fixture.root.join("discovery report.json")).unwrap();
    let review_before = fs::read(fixture.root.join("review selection.json")).unwrap();
    let input = fixture.success();
    assert_eq!(
        input["events"][0]["points"][0]["frequency_hz"]
            .as_f64()
            .unwrap(),
        1243.0421079096013_f64
    );
    for (i, event_index) in [0, 2].into_iter().enumerate() {
        let expected: Vec<Value> = fixture.discovery["events"][event_index]["ridge"]
            .as_array()
            .unwrap()
            .iter()
            .map(|point| json!({"t_rec_s":point["time_s"],"frequency_hz":point["frequency_hz"]}))
            .collect();
        assert_eq!(input["events"][i]["points"], json!(expected));
        assert_eq!(
            input["events"][i]["f0_hz"],
            fixture.config["selected_ridges"][i]["f0_hz"]
        );
        assert_eq!(
            load(
                &fixture
                    .root
                    .join(format!("prepared output/event-{}.json", i + 1))
            ),
            input["events"][i]
        );
    }
    for key in [
        "tracks",
        "observers",
        "pair_config",
        "coarse_bounds",
        "measured_separation_s",
        "separation_tolerance_s",
    ] {
        assert_eq!(input[key], fixture.template[key], "Bridge changed {key}");
    }
    assert_eq!(
        fs::read(fixture.root.join("discovery report.json")).unwrap(),
        discovery_before
    );
    assert_eq!(
        fs::read(fixture.root.join("review selection.json")).unwrap(),
        review_before
    );
    let receipt = load(&fixture.root.join("prepared output/receipt.json"));
    assert_eq!(receipt["location_verified"], false);
    assert_eq!(
        receipt["selected_ridges"][0]["contrast_weights"],
        json!([2.5, 3.5, 5.5])
    );
    assert_eq!(
        receipt["selected_ridges"][0]["quality"],
        fixture.discovery["events"][0]["quality"]
    );
    assert_eq!(
        receipt["selected_ridges"][0]["preserved_gaps"],
        json!([{
            "after_time_s":10.5,"before_time_s":18.,"duration_s":7.5
        }])
    );
    let text = serde_json::to_string(&receipt).unwrap();
    assert!(text.contains("automated_test"));
    assert!(
        text.contains("ridge-a-harmonic"),
        "Unused harmonic membership must remain auditable"
    );
}

#[test]
fn an_explicit_higher_harmonic_keeps_its_native_frequency_units() {
    let mut fixture = Fixture::new("higher harmonic");
    fixture.config["selected_ridges"][0]["event_id"] = "ridge-a-harmonic".into();
    fixture.config["selected_ridges"][0]["f0_hz"] = json!([216., 218., 220.]);
    fixture.save();
    let input = fixture.success();
    assert_eq!(input["events"][0]["points"][0]["frequency_hz"], 240.);
    assert_eq!(input["events"][0]["f0_hz"], json!([216., 218., 220.]));
    assert_eq!(input["events"][0]["points"].as_array().unwrap().len(), 3);
    let receipt = load(&fixture.root.join("prepared output/receipt.json"));
    assert_eq!(
        receipt["selected_ridges"][0]["other_group_ridges_not_scored"],
        json!(["ridge-a"])
    );
}

#[test]
fn geometry_configuration_needs_no_manual_ridge_files_and_keeps_all_candidates() {
    let mut fixture = Fixture::new("fresh geometry route");
    fixture
        .config
        .as_object_mut()
        .unwrap()
        .remove("template_search_input");
    fixture.config["search_config"] = "geometry/search config.json".into();
    let geometry_dir = fixture.root.join("geometry");
    fs::create_dir(&geometry_dir).unwrap();
    let mut tracks = fixture.template["tracks"].as_array().unwrap().clone();
    let mut alternative = tracks[0].clone();
    alternative["flight_id"] = "unselected-alternative".into();
    tracks.push(alternative);
    save(
        &geometry_dir.join("tracks.json"),
        &json!({"passes":tracks.iter().map(|track| json!({"track":track})).collect::<Vec<_>>(),"stats":{"excluded":0},"quality_notes":["Fabricated contract data"]}),
    );
    save(
        &geometry_dir.join("observers.json"),
        &fixture.template["observers"],
    );
    let config = json!({
        "normalized_tracks":"tracks.json","observers_file":"observers.json",
        "observer_bounds":[-0.01,-0.01,0.01,0.01],"grid_spacing_m":1000.,"observer_ellipsoid_height_m":0.,
        "pair_config":fixture.template["pair_config"],"coarse_bounds":fixture.template["coarse_bounds"],
        "measured_separation_s":fixture.template["measured_separation_s"],
        "separation_tolerance_s":fixture.template["separation_tolerance_s"],
        "provenance":["Geometry route contract fixture; no manual event files exist"]
    });
    assert!(config.get("event_files").is_none());
    save(&geometry_dir.join("search config.json"), &config);
    fixture.save();
    fs::remove_file(fixture.root.join("template input.json")).unwrap();
    let input = fixture.success();
    assert_eq!(input["tracks"], json!(tracks));
    for key in [
        "observers",
        "pair_config",
        "coarse_bounds",
        "measured_separation_s",
        "separation_tolerance_s",
    ] {
        assert_eq!(input[key], fixture.template[key]);
    }
    let receipt = load(&fixture.root.join("prepared output/receipt.json"));
    assert_eq!(receipt["provenance"]["search_basis_kind"], "search_config");
    let dependencies = receipt["provenance"]["dependencies"].as_array().unwrap();
    assert!(
        dependencies
            .iter()
            .any(|d| d["path"].as_str().unwrap().ends_with("tracks.json"))
    );
    assert!(
        dependencies
            .iter()
            .any(|d| d["path"].as_str().unwrap().ends_with("observers.json"))
    );
}

#[test]
fn only_explicit_keep_reviews_can_enter_the_matcher() {
    for review in ["unreviewed", "reject", "uncertain", "invented-status"] {
        let mut fixture = Fixture::new(review);
        fixture.review["groups"][0]["review"] = review.into();
        fixture.save();
        fixture.failure();
    }
}

#[test]
fn review_binding_rejects_changed_report_source_channel_and_false_acceptance() {
    for kind in [
        "discovery",
        "source",
        "channel",
        "accepted",
        "actor",
        "legacy",
    ] {
        let mut fixture = Fixture::new(kind);
        fixture.save();
        match kind {
            "discovery" => fixture.review["discovery_sha256"] = "f".repeat(64).into(),
            "source" => fixture.review["source_sha256"] = "f".repeat(64).into(),
            "channel" => fixture.review["selected_channel_index"] = 0.into(),
            "accepted" => fixture.review["location_verified"] = true.into(),
            "actor" => fixture.review["review_actor"] = "unknown".into(),
            "legacy" => {
                fixture.review["schema_version"] = 1.into();
                fixture
                    .review
                    .as_object_mut()
                    .unwrap()
                    .remove("discovery_sha256");
            }
            _ => unreachable!(),
        }
        save(&fixture.root.join("review selection.json"), &fixture.review);
        fixture.failure();
    }
}

#[test]
fn all_review_memberships_must_match_even_for_unused_harmonics() {
    for kind in [
        "missing-harmonic",
        "unknown-event",
        "duplicate-event",
        "missing-group",
        "duplicate-group",
        "unknown-group",
    ] {
        let mut fixture = Fixture::new(kind);
        match kind {
            "missing-harmonic" => fixture.review["groups"][0]["event_ids"] = json!(["ridge-a"]),
            "unknown-event" => {
                fixture.review["groups"][0]["event_ids"] = json!(["ridge-a", "invented"])
            }
            "duplicate-event" => {
                fixture.review["groups"][0]["event_ids"] = json!(["ridge-a", "ridge-a"])
            }
            "missing-group" => {
                fixture.review["groups"].as_array_mut().unwrap().pop();
            }
            "duplicate-group" => fixture.review["groups"][1] = fixture.review["groups"][0].clone(),
            "unknown-group" => fixture.review["groups"][1]["harmonic_group_id"] = "invented".into(),
            _ => unreachable!(),
        }
        fixture.save();
        fixture.failure();
    }
}

#[test]
fn selection_requires_two_distinct_matching_groups_and_explicit_tone_grids() {
    for kind in [
        "same-group",
        "foreign-event",
        "unknown-event",
        "empty-grid",
        "zero-grid",
        "negative-grid",
        "reversed",
        "empty-basis",
    ] {
        let mut fixture = Fixture::new(kind);
        match kind {
            "same-group" => {
                fixture.config["selected_ridges"][1] = fixture.config["selected_ridges"][0].clone()
            }
            "foreign-event" => fixture.config["selected_ridges"][0]["event_id"] = "ridge-b".into(),
            "unknown-event" => fixture.config["selected_ridges"][0]["event_id"] = "invented".into(),
            "empty-grid" => fixture.config["selected_ridges"][0]["f0_hz"] = json!([]),
            "zero-grid" => fixture.config["selected_ridges"][0]["f0_hz"] = json!([0.]),
            "negative-grid" => fixture.config["selected_ridges"][0]["f0_hz"] = json!([-10.]),
            "reversed" => fixture.config["selected_ridges"]
                .as_array_mut()
                .unwrap()
                .reverse(),
            "empty-basis" => fixture.config["selection_basis"] = " ".into(),
            _ => unreachable!(),
        }
        fixture.save();
        fixture.failure();
    }
}

#[test]
fn invalid_ridge_measurements_are_errors_not_silently_repaired() {
    for kind in [
        "duplicate-time",
        "descending-time",
        "negative-time",
        "outside-source",
        "zero-frequency",
        "null-frequency",
        "too-short",
        "duplicate-id",
        "wrong-count",
    ] {
        let mut fixture = Fixture::new(kind);
        match kind {
            "duplicate-time" => fixture.discovery["events"][0]["ridge"][1]["time_s"] = 10.0.into(),
            "descending-time" => fixture.discovery["events"][0]["ridge"][1]["time_s"] = 9.5.into(),
            "negative-time" => fixture.discovery["events"][0]["ridge"][0]["time_s"] = (-1.0).into(),
            "outside-source" => fixture.discovery["events"][0]["ridge"][2]["time_s"] = 101.0.into(),
            "zero-frequency" => {
                fixture.discovery["events"][0]["ridge"][0]["frequency_hz"] = 0.0.into()
            }
            "null-frequency" => {
                fixture.discovery["events"][0]["ridge"][0]["frequency_hz"] = Value::Null
            }
            "too-short" => {
                fixture.discovery["events"][0]["ridge"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            "duplicate-id" => fixture.discovery["events"][1]["event_id"] = "ridge-a".into(),
            "wrong-count" => fixture.discovery["ridge_count"] = 4.into(),
            _ => unreachable!(),
        }
        fixture.save();
        fixture.failure();
    }
}

#[test]
fn raw_source_changes_fail_but_explicit_relocation_preserves_identity() {
    let mut changed = Fixture::new("source changed");
    changed.save();
    fs::write(changed.root.join("original source é.raw"), b"changed bytes").unwrap();
    changed.failure();
    let mut relocated = Fixture::new("source relocated");
    let old_path = relocated.root.join("original source é.raw");
    let new_path = relocated.root.join("relocated é source.raw");
    fs::rename(&old_path, &new_path).unwrap();
    relocated.config["source_override"] = "relocated é source.raw".into();
    relocated.save();
    relocated.success();
    assert!(!old_path.exists());
}

#[test]
fn existing_output_and_unknown_configuration_are_never_overwritten_or_ignored() {
    let mut fixture = Fixture::new("existing output");
    fixture.save();
    let out = fixture.root.join("prepared output");
    fs::create_dir(&out).unwrap();
    fs::write(out.join("keep.txt"), b"preserve me").unwrap();
    assert_eq!(fixture.run().status.code(), Some(2));
    assert_eq!(fs::read(out.join("keep.txt")).unwrap(), b"preserve me");
    assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
    for kind in ["unknown-field", "both-templates", "no-template"] {
        let mut fixture = Fixture::new(kind);
        match kind {
            "unknown-field" => fixture.config["allow_unreviewed"] = true.into(),
            "both-templates" => fixture.config["search_config"] = "other.json".into(),
            "no-template" => {
                fixture
                    .config
                    .as_object_mut()
                    .unwrap()
                    .remove("template_search_input");
            }
            _ => unreachable!(),
        }
        fixture.save();
        fixture.failure();
    }
}

#[test]
fn prepared_ridges_reach_the_real_pair_matcher_with_independent_forward_time_truth() {
    let mut fixture = Fixture::new("physical pair search");
    let track: Vec<Value> = (-6..=26)
        .map(|i| {
            let t = i as f64 * 10.;
            let along = if t <= 100. {
                100. * t
            } else {
                100. * (200. - t)
            };
            json!({"t_s":t,"xyz_m":[6379637.,along,0.]})
        })
        .collect();
    fixture.template["tracks"] = json!([{"flight_id":"return-flight","points":track}]);
    fixture.template["pair_config"] = json!({"event1_cpa_recording_s":100.+1500./343.,"shared_shifts_s":[0.],"clock_offset_bounds_s":null,"model":{"sound_speed_mps":343.,"max_gap_s":30.}});
    fixture.template["coarse_bounds"] = json!([{"speed_mps":[90.,110.],"slant_distance_m":[1000.,2000.]},{"speed_mps":[90.,110.],"slant_distance_m":[1000.,2000.]}]);
    fixture.template["measured_separation_s"] = 200.0.into();
    fixture.template["separation_tolerance_s"] = 0.01.into();
    fixture.discovery["source_metadata"]["duration_s"] = 400.0.into();
    for (slot, event_index, center, f0) in [(0, 0, 0., 110.), (1, 2, 200., 140.)] {
        let ridge: Vec<Value> = (-30..=30).step_by(3).map(|t| {
            let t = t as f64;
            let range = (100.*t).hypot(1500.);
            json!({"time_s":center+t+range/343.+100.,"frequency_hz":f0*343./(343.+10000.*t/range),"weight":3.})
        }).collect();
        let start = ridge[0]["time_s"].as_f64().unwrap() - 1.;
        let end = ridge.last().unwrap()["time_s"].as_f64().unwrap() + 1.;
        fixture.discovery["events"][event_index]["ridge"] = json!(ridge);
        fixture.discovery["events"][event_index]["start_s"] = start.into();
        fixture.discovery["events"][event_index]["end_s"] = end.into();
        fixture.discovery["events"][event_index]["band_hz"] = json!([50., 300.]);
        fixture.discovery["groups"][slot]["start_s"] = start.into();
        fixture.discovery["groups"][slot]["end_s"] = end.into();
        fixture.config["selected_ridges"][slot]["f0_hz"] = json!([f0]);
    }
    // Keep the unselected harmonic consistent with its group's envelope.
    fixture.discovery["events"][1] = fixture.discovery["events"][0].clone();
    fixture.discovery["events"][1]["event_id"] = "ridge-a-harmonic".into();
    for point in fixture.discovery["events"][1]["ridge"]
        .as_array_mut()
        .unwrap()
    {
        point["frequency_hz"] = (point["frequency_hz"].as_f64().unwrap() * 2.).into();
    }
    fixture.discovery["events"][1]["band_hz"] = json!([100., 600.]);
    fixture.save();
    let input = fixture.success();
    let report = verification::rank(&serde_json::from_value(input).unwrap()).unwrap();
    assert_eq!(report["status"], "hypotheses_ranked");
    assert_eq!(report["location_verified"], false);
    assert_eq!(
        report["candidates"][0]["score"]["flight_ids"],
        json!(["return-flight", "return-flight"])
    );
    assert!(
        report["candidates"][0]["score"]["combined_rmse_hz"]
            .as_f64()
            .unwrap()
            < 1e-10
    );
    assert!(
        (report["candidates"][0]["score"]["clock_offset_s"]
            .as_f64()
            .unwrap()
            + 100.)
            .abs()
            < 1e-10
    );
}

#[test]
fn regenerated_review_binds_exact_report_bytes_and_refuses_overwrite() {
    let mut fixture = Fixture::new("review regeneration");
    fixture.discovery["events"][0]["ridge"][0]["frequency_hz"] = 1243.0421079096013_f64.into();
    fixture.discovery["events"][0]["band_hz"] = json!([100., 1244.]);
    fixture.save();
    let source = fixture.root.join("discovery report.json");
    let output = fixture.root.join("fresh review.html");
    geosonnar::discovery::write_review(&source, &output).unwrap();
    let before = fs::read(&output).unwrap();
    let html = String::from_utf8(before.clone()).unwrap();
    assert!(html.contains(&format!("const discoverySha256='{}';", sha256(&source))));
    assert!(html.contains("schema_version:2"));
    assert!(html.contains("1243.0421079096013"));
    assert!(geosonnar::discovery::write_review(&source, &output).is_err());
    assert_eq!(fs::read(&output).unwrap(), before);
}
