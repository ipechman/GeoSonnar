//! File-bound workflow controls; synthetic data are not location validation.
use geosonnar::{detailed_data::sha256, envelope_workflow as workflow};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "envelope-workflow-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn write(&self, name: &str, value: &Value) {
        fs::write(self.file(name), serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }
    fn prepare(&self) -> Value {
        fs::write(
            self.file("recording.bin"),
            b"Synthetic identity fixture; not measured audio",
        )
        .unwrap();
        self.write("lightning.json",&json!({"schema_version":1,"provenance":{
            "provider":"Synthetic","source":"workflow control","license":null,"notes":null,"altitude_datum":"wgs84_ellipsoid"},
            "strikes":[{"id":"a","time_utc_s":1700000015.0,"latitude_deg":40.0,"longitude_deg":-75.0,"altitude_m":1000.0,"intensity":null}]}));
        self.write("observation.json",&json!({"recording_sha256":sha256(&self.file("recording.bin")).unwrap(),"channel":0,
            "envelope":{"samples":[{"time_s":20.0,"rms":1.0},{"time_s":21.0,"rms":2.0},{"time_s":22.0,"rms":0.0},{"time_s":23.0,"rms":1.0}],
                "band_low_hz":20.0,"band_high_hz":250.0,"method":"Synthetic fixture; no measurement claim"},
            "reviewed_selection_note":"Synthetic test interval"}));
        let config = json!({"schema_version":1,"recording":"recording.bin","observation":"observation.json","lightning":"lightning.json",
            "lightning_provenance":{"provider":"Fallback","source":"fallback description","license":null,"notes":null},
            "plan":{"schema_version":1,"catalog_sha256":sha256(&self.file("lightning.json")).unwrap(),"catalog_utc_interval":[1699999900.0,1700000100.0],
                "receivers":[{"id":"receiver<script>alert(1)</script>","location":{"latitude_deg":40.0,"longitude_deg":-75.0,"ellipsoid_height_m":0.0}}],
                "nominal_recording_start_utc_s":1700000000.0,"min_clock_offset_s":-1.0,"max_clock_offset_s":1.0,
                "model":{"sound_speed_mps":343.0,"fallback_strike_ellipsoid_height_m":null,"minimum_distance_m":1.0,"maximum_distance_m":5000.0,"kernel_decay_s":2.0,"kernel_duration_s":10.0},
                "source_policy":null,"keep_best":3,"limits":{"max_work_units":200000000,"max_memory_bytes":268435456}}});
        self.write("run.json", &config);
        config
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .canonicalize()
            .unwrap();
        let path = self.0.canonicalize().unwrap();
        assert_eq!(path.parent(), Some(root.as_path()));
        assert!(
            !fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(path).unwrap();
    }
}
fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn preview_and_run_bind_exact_files_preserve_inputs_and_escape_review_labels() {
    let f = Fixture::new();
    f.prepare();
    let names = [
        "recording.bin",
        "lightning.json",
        "observation.json",
        "run.json",
    ];
    let before: Vec<_> = names.iter().map(|n| sha256(&f.file(n)).unwrap()).collect();
    let preview = workflow::preview(&f.file("run.json")).unwrap();
    assert_eq!(preview["estimate"]["total_offsets"], 3);
    assert_eq!(preview["location_verified"], false);
    let report = workflow::run(
        &f.file("run.json"),
        &f.file("result"),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(report["status"], "completed");
    assert_eq!(report["retained_hypotheses"], 3);
    assert_eq!(report["independent_field_validation"], false);
    for artifact in report["artifacts"].as_array().unwrap() {
        assert_eq!(
            sha256(Path::new(artifact["path"].as_str().unwrap())).unwrap(),
            artifact["sha256"].as_str().unwrap()
        );
    }
    let html = fs::read_to_string(f.file("result/review.html")).unwrap();
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    let curves = read(&f.file("result/overlays.json"));
    assert_eq!(curves[0]["times_s"], json!([20.0, 21.0, 22.0, 23.0]));
    assert_eq!(curves[0]["location_verified"], false);
    assert_eq!(
        before,
        names
            .iter()
            .map(|n| sha256(&f.file(n)).unwrap())
            .collect::<Vec<_>>()
    );
}

#[test]
fn changed_recording_catalog_and_unknown_observation_fields_fail_before_output() {
    for variant in 0..3 {
        let f = Fixture::new();
        f.prepare();
        match variant {
            0 => fs::write(f.file("recording.bin"), b"Changed recording").unwrap(),
            1 => {
                let mut value = read(&f.file("lightning.json"));
                value["strikes"][0]["time_utc_s"] = json!(1700000016.0);
                f.write("lightning.json", &value);
            }
            _ => {
                let mut value = read(&f.file("observation.json"));
                value["mask"] = json!([true, false, true, true]);
                f.write("observation.json", &value);
            }
        }
        assert!(
            workflow::run(
                &f.file("run.json"),
                &f.file("result"),
                &AtomicBool::new(false),
                |_| {}
            )
            .is_err()
        );
        assert!(!f.file("result").exists());
    }
}

#[test]
fn repeated_output_is_refused_without_replacing_results() {
    let f = Fixture::new();
    f.prepare();
    workflow::run(
        &f.file("run.json"),
        &f.file("result"),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let original = sha256(&f.file("result/completed.json")).unwrap();
    assert!(
        workflow::run(
            &f.file("run.json"),
            &f.file("result"),
            &AtomicBool::new(false),
            |_| {}
        )
        .is_err()
    );
    assert_eq!(sha256(&f.file("result/completed.json")).unwrap(), original);
}

#[test]
fn cancellation_and_midrun_input_change_leave_honest_terminal_receipts() {
    let f = Fixture::new();
    f.prepare();
    let report = workflow::run(
        &f.file("run.json"),
        &f.file("cancelled"),
        &AtomicBool::new(true),
        |_| {},
    )
    .unwrap();
    assert_eq!(report["status"], "cancelled");
    assert_eq!(report["complete"], false);
    assert_eq!(report["retained_hypotheses"], 0);
    assert!(
        workflow::run(
            &f.file("run.json"),
            &f.file("changed"),
            &AtomicBool::new(false),
            |_| {
                fs::write(f.file("recording.bin"), b"Changed during run").unwrap();
            }
        )
        .is_err()
    );
    assert!(f.file("changed/failure.json").is_file());
    assert!(!f.file("changed/completed.json").exists());
    assert!(f.file("changed/started.json").is_file());
}

#[test]
fn cancellation_after_search_reports_missing_presentation_without_losing_results() {
    let f = Fixture::new();
    f.prepare();
    let cancel = AtomicBool::new(false);
    let report = workflow::run(&f.file("run.json"), &f.file("result"), &cancel, |p| {
        if p.completed_receivers == p.total_receivers {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert_eq!(report["status"], "cancelled");
    assert_eq!(report["search_complete"], true);
    assert_eq!(report["presentation_complete"], false);
    assert_eq!(report["complete"], false);
    assert_eq!(report["retained_hypotheses"], 3);
    assert_eq!(report["preview_curves_written"], 0);
    assert!(f.file("result/results.json").is_file());
}

#[test]
fn requested_curve_work_is_budgeted_and_large_finite_rms_renders_finite_points() {
    let f = Fixture::new();
    let mut config = f.prepare();
    let preview = workflow::preview(&f.file("run.json")).unwrap();
    config["plan"]["limits"]["max_work_units"] =
        preview["estimate"]["estimated_work_units"].clone();
    f.write("run.json", &config);
    assert!(
        workflow::preview(&f.file("run.json"))
            .unwrap_err()
            .contains("requested previews")
    );
    config["preview_curves"] = json!(0);
    f.write("run.json", &config);
    workflow::preview(&f.file("run.json")).unwrap();
    config["preview_curves"] = json!(1);
    config["plan"]["limits"]["max_work_units"] = json!(200000000);
    f.write("run.json", &config);
    let mut observation = read(&f.file("observation.json"));
    for sample in observation["envelope"]["samples"].as_array_mut().unwrap() {
        sample["rms"] = json!(sample["rms"].as_f64().unwrap() * 1e307);
    }
    f.write("observation.json", &observation);
    workflow::run(
        &f.file("run.json"),
        &f.file("result"),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let html = fs::read_to_string(f.file("result/review.html")).unwrap();
    assert!(html.contains("<polyline"));
    assert!(!html.contains(",inf") && !html.contains(",-inf") && !html.contains(",NaN"));
}

fn wave(path: &Path, rate: u32, tones: [f64; 2]) {
    let frames = rate * 3;
    let size = frames * 4;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + size).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(2_u16.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * 4).to_le_bytes());
    bytes.extend(4_u16.to_le_bytes());
    bytes.extend(16_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(size.to_le_bytes());
    for i in 0..frames {
        let t = f64::from(i) / f64::from(rate);
        for hz in tones {
            bytes.extend(
                ((16000.0 * (std::f64::consts::TAU * hz * t).sin()).round() as i16).to_le_bytes(),
            );
        }
    }
    fs::write(path, bytes).unwrap();
}

#[test]
#[ignore = "Requires actual FFmpeg and FFprobe; run explicitly in the release profile"]
fn actual_measurement_preserves_selected_channel_center_timestamps_and_original_bandwidth() {
    let f = Fixture::new();
    wave(&f.file("stereo.wav"), 8000, [500.0, 100.0]);
    let tools = PathBuf::from("C:/ProgramData/chocolatey/lib/ffmpeg-full/tools/ffmpeg/bin");
    let config = json!({"schema_version":1,"recording":"stereo.wav","channel":1,"start_s":0.5,"end_s":2.5,"fft_size":1024,"hop_s":0.1,
        "reviewed_selection_note":"Frozen synthetic channel/time control","ffmpeg":tools.join("ffmpeg.exe"),"ffprobe":tools.join("ffprobe.exe")});
    f.write("measure.json", &config);
    let report = workflow::measure(&f.file("measure.json"), &f.file("measured")).unwrap();
    assert_eq!(report["original_sample_rate_hz"], 8000);
    let observed = read(&f.file("measured/observation.json"));
    let parsed: workflow::MeasurementConfig = serde_json::from_value(config.clone()).unwrap();
    let in_memory = workflow::measure_observation(&parsed, &f.0, &AtomicBool::new(false))
        .expect("The GUI measurement uses the same native envelope as the file workflow");
    assert_eq!(serde_json::to_value(&in_memory).unwrap(), observed);
    let cancelled =
        workflow::measure_observation(&parsed, &f.0, &AtomicBool::new(true)).unwrap_err();
    assert!(cancelled.to_ascii_lowercase().contains("cancelled"));
    assert_eq!(observed["channel"], 1);
    assert_eq!(
        observed["recording_sha256"],
        sha256(&f.file("stereo.wav")).unwrap()
    );
    let samples = observed["envelope"]["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 18);
    for (index, sample) in samples.iter().enumerate() {
        assert!((sample["time_s"].as_f64().unwrap() - (0.628 + index as f64 * 0.1)).abs() < 1e-10);
        assert!(
            (sample["rms"].as_f64().unwrap() - 16000.0 / 32768.0 / 2.0_f64.sqrt()).abs() < 0.002
        );
    }
    wave(&f.file("lowrate.wav"), 100, [30.0, 25.0]);
    let mut low = config;
    low["recording"] = json!("lowrate.wav");
    f.write("lowrate.json", &low);
    assert!(
        workflow::measure(&f.file("lowrate.json"), &f.file("lowrate-output"))
            .unwrap_err()
            .contains("original sample rate")
    );
    assert!(!f.file("lowrate-output").exists());
    low["band_hz"] = json!([20.0, 40.0]);
    f.write("lowrate-reduced.json", &low);
    workflow::measure(&f.file("lowrate-reduced.json"), &f.file("lowrate-reduced")).unwrap();
    let reduced = read(&f.file("lowrate-reduced/observation.json"));
    assert!(reduced["envelope"]["band_low_hz"].as_f64().unwrap() >= 20.0);
    assert!(reduced["envelope"]["band_high_hz"].as_f64().unwrap() <= 40.0);
    wave(&f.file("silence.wav"), 8000, [0.0, 0.0]);
    low["recording"] = json!("silence.wav");
    f.write("silence.json", &low);
    assert!(
        workflow::measure(&f.file("silence.json"), &f.file("silence-output"))
            .unwrap_err()
            .contains("no represented band energy")
    );
    assert!(!f.file("silence-output").exists());
}

#[test]
#[ignore = "Requires actual FFmpeg and FFprobe; run explicitly in the release profile"]
fn cli_measurement_resolves_null_helpers_from_child_path() {
    let f = Fixture::new();
    wave(&f.file("stereo.wav"), 8000, [500.0, 100.0]);
    let helpers = ["FFMPEG", "FFPROBE"].map(|name| {
        std::env::var_os(format!("GEOSONNAR_TEST_{name}"))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from("C:/ProgramData/chocolatey/lib/ffmpeg-full/tools/ffmpeg/bin")
                    .join(format!("{}.exe", name.to_ascii_lowercase()))
            })
            .canonicalize()
            .expect("Provide actual media helpers using GEOSONNAR_TEST_FFMPEG/FFPROBE")
    });
    for (helper, name) in helpers.iter().zip(["ffmpeg", "ffprobe"]) {
        let filename = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.into()
        };
        assert_eq!(helper.file_name().unwrap(), std::ffi::OsStr::new(&filename));
    }
    let child_path =
        std::env::join_paths(helpers.iter().map(|tool| tool.parent().unwrap())).unwrap();
    // Isolate the CLI from sidecars and the known ProgramData installation so
    // this regression must exercise PATH discovery. No helper is copied merely
    // to resolve its path. Both overrides apply only to the child process.
    let cli_directory = f.file("isolated-cli");
    fs::create_dir(&cli_directory).unwrap();
    let cli = cli_directory.join(if cfg!(windows) {
        "geosonnar.exe"
    } else {
        "geosonnar"
    });
    fs::copy(env!("CARGO_BIN_EXE_geosonnar"), &cli).unwrap();
    let invoke = |config: &Path, output: &Path| {
        let mut command = std::process::Command::new(&cli);
        command
            .current_dir(&f.0)
            .env("PATH", &child_path)
            .env_remove("ProgramData")
            .args(["measure-thunder-envelope", "--config"])
            .arg(config)
            .arg("--out")
            .arg(output)
            .stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command.output().unwrap()
    };
    let mut config = json!({"schema_version":1,"recording":"stereo.wav","channel":1,
        "start_s":0.5,"end_s":2.5,"fft_size":1024,"hop_s":0.1,
        "reviewed_selection_note":"Synthetic PATH-only CLI regression; not storm evidence",
        "ffmpeg":null,"ffprobe":null});
    f.write("measure-defaults.json", &config);
    let result = invoke(
        &f.file("measure-defaults.json"),
        &f.file("default-measured"),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let observation = read(&f.file("default-measured/observation.json"));
    assert_eq!(
        observation["recording_sha256"],
        sha256(&f.file("stereo.wav")).unwrap()
    );
    assert_eq!(observation["channel"], 1);
    let samples = observation["envelope"]["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 18);
    for (index, sample) in samples.iter().enumerate() {
        assert!((sample["time_s"].as_f64().unwrap() - (0.628 + index as f64 * 0.1)).abs() < 1e-10);
        assert!(
            (sample["rms"].as_f64().unwrap() - 16000.0 / 32768.0 / 2.0_f64.sqrt()).abs() < 0.002
        );
    }
    let measurement = read(&f.file("default-measured/measurement.json"));
    assert!(
        measurement["config"]["ffmpeg"].is_null() && measurement["config"]["ffprobe"].is_null()
    );
    // An explicit bad selection must stay an error even with working PATH tools.
    config["ffprobe"] = json!("missing-selected-ffprobe");
    f.write("measure-bad-selection.json", &config);
    let failed_output = f.file("bad-selection-output");
    let failed = invoke(&f.file("measure-bad-selection.json"), &failed_output);
    assert_eq!(failed.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("Select the actual FFprobe executable")
    );
    assert!(!failed_output.exists());
}
