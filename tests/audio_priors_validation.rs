//! Pipeline contracts use fabricated recordings and independent emission-domain
//! measurements; these tests are not empirical evidence of location accuracy.
use geosonnar::{audio_priors, verification};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};
static SEQ: AtomicU64 = AtomicU64::new(0);
fn save(p: &Path, v: &Value) {
    verification::write_json(p, v).unwrap();
}
fn load(p: &Path) -> Value {
    verification::read_json(p).unwrap()
}
fn hash(p: &Path) -> String {
    #[cfg(windows)]
    let mut c = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("certutil.exe");
        c.arg("-hashfile")
            .arg(p)
            .arg("SHA256")
            .creation_flags(0x08000000);
        c
    };
    #[cfg(not(windows))]
    let mut c = {
        let mut c = Command::new("sha256sum");
        c.arg("--").arg(p);
        c
    };
    let out = c.stdin(Stdio::null()).output().unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .find(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap()
        .to_ascii_lowercase()
}
struct Fixture {
    root: PathBuf,
    request: Value,
    geometry: Value,
    discovery: Value,
    review: Value,
}
impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("output/benchmarks/audio-priors/integration")
            .join(format!(
                "{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("fabricated source.raw");
        fs::write(&source, b"contract fixture, not recorded audio").unwrap();
        let digest = hash(&source);
        let mut events = Vec::new();
        let mut groups = Vec::new();
        let mut passes = Vec::new();
        for (i, center, v, d, f0) in [(0, 200., 100., 2000., 110.), (1, 600., 80., 1500., 140.)] {
            let ridge:Vec<Value>=(-120..=120).step_by(4).filter(|u|!(-20..=-8).contains(u)).map(|u|{let u=u as f64;let range=(v*u).hypot(d);json!({"time_s":center+u+range/343.,"frequency_hz":f0*343./(343.+v*v*u/range),"weight":3.})}).collect();
            let start = ridge[0]["time_s"].as_f64().unwrap() - 1.;
            let end = ridge.last().unwrap()["time_s"].as_f64().unwrap() + 1.;
            events.push(json!({"event_id":format!("ridge-{i}"),"harmonic_group_id":format!("group-{i}"),"start_s":start,"end_s":end,"band_hz":[50.,300.],"ridge":ridge,"quality":{"frequency_sigma_hz":0.1,"support_fraction":0.9,"fragment_count":2}}));
            groups.push(json!({"harmonic_group_id":format!("group-{i}"),"start_s":start,"end_s":end,"ridge_count":1,"aircraft_identity_confirmed":false,"review_status":"unreviewed"}));
            let track: Vec<Value> = (-200..=200)
                .step_by(10)
                .map(|u| json!({"t_s":center+u as f64+1000.,"xyz_m":[6378137.+d,v*u as f64,0.]}))
                .collect();
            passes.push(json!({"track":{"flight_id":format!("flight-{i}"),"points":track}}));
        }
        save(
            &root.join("tracks.json"),
            &json!({"passes":passes,"config":{"evidence":"synthetic oracle"},"stats":{},"quality_notes":[],"issues":[]}),
        );
        save(
            &root.join("observers.json"),
            &json!([{"id":"oracle","latitude_deg":0.,"longitude_deg":0.,"ellipsoid_height_m":0.}]),
        );
        let discovery = json!({"schema_version":1,"status":"events_proposed","location_verified":false,"source":source,"source_sha256":digest,"source_metadata":{"duration_s":1000.,"selected_channel_index":0,"available_channels":1,"channels_averaged":false},"settings":{"max_gap_s":5.},"group_count":2,"ridge_count":2,"groups":groups,"events":events});
        let review = json!({"schema_version":2,"discovery_sha256":"pending","source_sha256":digest,"selected_channel_index":0,"location_verified":false,"review_actor":"automated_test","scope":"Independent analytic fixture; not human review","groups":[{"harmonic_group_id":"group-0","review":"keep_for_matching","event_ids":["ridge-0"]},{"harmonic_group_id":"group-1","review":"keep_for_matching","event_ids":["ridge-1"]}]});
        let request = json!({"schema_version":1,"discovery":"discovery.json","review_selection":"review.json","selected_ridges":[{"harmonic_group_id":"group-0","event_id":"ridge-0"},{"harmonic_group_id":"group-1","event_id":"ridge-1"}],"selection_basis":"Fabricated oracle, not a real recording"});
        let geometry = json!({"schema_version":1,"normalized_tracks":"tracks.json","observer_bounds":[-0.001,-0.001,0.001,0.001],"grid_spacing_m":1000.,"observer_ellipsoid_height_m":0.,"observers_file":"observers.json","clock_offset_bounds_s":[900.,1100.],"model":{"sound_speed_mps":343.,"max_gap_s":30.},"provenance":["Independent forward emission-time fixture"]});
        let mut f = Self {
            root,
            request,
            geometry,
            discovery,
            review,
        };
        f.save();
        f
    }
    fn save(&mut self) {
        save(&self.root.join("discovery.json"), &self.discovery);
        self.review["discovery_sha256"] = hash(&self.root.join("discovery.json")).into();
        save(&self.root.join("review.json"), &self.review);
        save(&self.root.join("request.json"), &self.request);
        save(&self.root.join("geometry.json"), &self.geometry);
    }
    fn derive(&self) -> Value {
        audio_priors::derive(&self.root.join("request.json"), &self.root.join("derived")).unwrap()
    }
    fn prepare(&self, name: &str) -> Result<Value, String> {
        audio_priors::prepare(
            &self.root.join("derived/priors.json"),
            &self.root.join("geometry.json"),
            &self.root.join(name),
        )
    }
}

#[test]
fn audio_only_priors_reach_rank_with_known_clock_and_unfilled_gaps() {
    let fixture = Fixture::new();
    let p = fixture.derive();
    assert_eq!(p["status"], "audio_priors_ready", "{p}");
    for i in 0..2 {
        let actual = p["events"][i]["points"].as_array().unwrap();
        let source = fixture.discovery["events"][i]["ridge"].as_array().unwrap();
        assert_eq!(actual.len(), source.len());
        for (a, b) in actual.iter().zip(source) {
            assert_eq!(a["t_rec_s"], b["time_s"]);
            assert_eq!(a["frequency_hz"], b["frequency_hz"]);
        }
        assert!(
            actual.windows(2).any(|p| p[1]["t_rec_s"].as_f64().unwrap()
                - p[0]["t_rec_s"].as_f64().unwrap()
                > 12.)
        );
        assert_eq!(
            p["fits"][i]["diagnostics"]["sensitivity_is_confidence_interval"],
            false
        );
    }
    fixture.prepare("search").unwrap();
    let input = verification::read_json(&fixture.root.join("search/search_input.json")).unwrap();
    let result = verification::rank(&input).unwrap();
    assert_eq!(result["location_verified"], false);
    assert_eq!(
        result["candidates"][0]["score"]["flight_ids"],
        json!(["flight-0", "flight-1"])
    );
    assert!(
        (result["candidates"][0]["score"]["clock_offset_s"]
            .as_f64()
            .unwrap()
            - 1000.)
            .abs()
            < 0.001
    );
    assert!(
        result["candidates"][0]["score"]["combined_rmse_hz"]
            .as_f64()
            .unwrap()
            < 0.001
    );
}

#[test]
fn manual_priors_unknown_geometry_fields_and_wrong_sound_speed_are_rejected() {
    let mut f = Fixture::new();
    f.request["selected_ridges"][0]["f0_hz"] = json!([110.]);
    f.save();
    assert!(
        audio_priors::derive(&f.root.join("request.json"), &f.root.join("bad-request")).is_err()
    );
    f.request["selected_ridges"][0]
        .as_object_mut()
        .unwrap()
        .remove("f0_hz");
    f.save();
    f.derive();
    for (i, key) in [
        "pair_config",
        "coarse_bounds",
        "event_files",
        "measured_separation_s",
    ]
    .iter()
    .enumerate()
    {
        let mut g = f.geometry.clone();
        g[*key] = json!("manual assumption forbidden");
        save(&f.root.join("geometry.json"), &g);
        assert!(f.prepare(&format!("bad-geometry-{i}")).is_err());
        assert!(
            !f.root
                .join(format!("bad-geometry-{i}/search_input.json"))
                .exists()
        );
    }
    f.geometry["model"]["sound_speed_mps"] = 338.0.into();
    save(&f.root.join("geometry.json"), &f.geometry);
    assert!(f.prepare("wrong-sound").is_err());
}

#[test]
fn altered_priors_or_source_cannot_be_used_and_existing_output_is_preserved() {
    let f = Fixture::new();
    f.derive();
    let original = fs::read(f.root.join("derived/priors.json")).unwrap();
    let mut p = load(&f.root.join("derived/priors.json"));
    p["derived"]["event1_cpa_recording_s"] = 123.0.into();
    save(&f.root.join("derived/priors.json"), &p);
    assert!(f.prepare("altered-priors").is_err());
    assert!(!f.root.join("altered-priors/search_input.json").exists());
    fs::write(f.root.join("derived/priors.json"), &original).unwrap();
    fs::write(f.root.join("fabricated source.raw"), b"different bytes").unwrap();
    assert!(f.prepare("altered-source").is_err());
    assert!(audio_priors::derive(&f.root.join("request.json"), &f.root.join("derived")).is_err());
    assert_eq!(
        fs::read(f.root.join("derived/priors.json")).unwrap(),
        original
    );
}

#[test]
fn motionless_ridge_preserves_inconclusive_diagnostics_and_never_prepares_search() {
    let mut f = Fixture::new();
    for p in f.discovery["events"][0]["ridge"].as_array_mut().unwrap() {
        p["frequency_hz"] = 110.0.into();
    }
    f.save();
    let p = f.derive();
    assert_eq!(p["status"], "inconclusive_audio_priors");
    assert_eq!(p["usable_for_search"], false);
    assert_eq!(p["utc_clock_inferred"], false);
    assert!(f.prepare("blocked").is_err());
    assert!(f.root.join("derived/priors.json").is_file());
    assert!(f.root.join("blocked/failure.json").is_file());
}

#[test]
fn missing_measurement_scale_and_unreviewed_selection_fail_without_completed_priors() {
    for mode in 0..2 {
        let mut f = Fixture::new();
        if mode == 0 {
            f.discovery["events"][0]["quality"]
                .as_object_mut()
                .unwrap()
                .remove("frequency_sigma_hz");
        } else {
            f.review["groups"][0]["review"] = "unreviewed".into();
        }
        f.save();
        assert!(audio_priors::derive(&f.root.join("request.json"), &f.root.join("bad")).is_err());
        assert!(!f.root.join("bad/priors.json").exists());
        assert!(f.root.join("bad/failure.json").is_file());
    }
}
