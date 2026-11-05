//! Actual FFmpeg/FFprobe discovery I/O checks. Missing media tools fail these
//! tests; no test is skipped. The generated controls contain no flight labels.
mod support;

use geosonnar::{
    audio,
    discovery::{self, DiscoverySettings},
    media,
};
use serde_json::Value;
use std::{
    f64::consts::PI,
    fs,
    path::{Path, PathBuf},
    process::Command,
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
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("output/benchmarks/discovery/io integration")
            .join(format!(
                "run-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&base).unwrap();
        base
    });
    let root = run.join(format!(
        "{name}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    root
}

fn write_wav(path: &Path, channels: u16, samples: &[i16]) {
    support::write_wav(path, 8000, channels, samples);
}

fn assert_no_success(output: &Path) {
    assert!(
        !output.join("discovery.json").exists(),
        "An error must not publish a completed scan"
    );
    assert!(
        !output.join("review.html").exists(),
        "An error must not publish a completed review"
    );
    if output.join("failure.json").is_file() {
        let failure: Value =
            serde_json::from_slice(&fs::read(output.join("failure.json")).unwrap()).unwrap();
        assert_eq!(failure["status"], "failed");
        assert_eq!(failure["location_verified"], false);
        assert!(failure["error"].as_str().unwrap().len() > 3);
    }
}

fn assert_complete(output: &Path, report: &Value) {
    assert!(!output.join("failure.json").exists());
    assert!(output.join("review.html").is_file());
    let saved = fs::read(output.join("discovery.json")).unwrap();
    serde_json::from_slice::<Value>(&saved).unwrap();
    // Compare the writer's actual representation: serde_json's default parser
    // may differ by one ULP from the original in-memory floating-point value.
    assert_eq!(saved, serde_json::to_vec_pretty(report).unwrap());
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["location_verified"], false);
    assert_eq!(report["source_metadata"]["channels_averaged"], false);
    assert!(report.get("latitude_deg").is_none());
    assert!(report.get("longitude_deg").is_none());
    assert!(report.get("utc").is_none());
}

#[test]
fn a_one_second_recording_with_spaces_is_complete_without_events_and_preserved() {
    let root = directory("short input");
    let input = root.join("très short input with spaces.wav");
    write_wav(&input, 1, &vec![0; 8000]);
    let original = fs::read(&input).unwrap();
    let output = root.join("review with spaces");
    let report = discovery::discover(&input, &output, 0, &DiscoverySettings::default()).unwrap();
    assert_complete(&output, &report);
    assert_eq!(report["status"], "no_events");
    assert_eq!(report["events"], serde_json::json!([]));
    assert_eq!(report["groups"], serde_json::json!([]));
    assert_eq!(report["source_metadata"]["decoded_samples"], 4000);
    assert_eq!(report["source_metadata"]["duration_s"], 1.0);
    assert_eq!(
        report["source"].as_str().unwrap(),
        input.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(report["source_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(report["chunks"][0]["duration_s"], 1.0);
    let saved_report = fs::read(output.join("discovery.json")).unwrap();
    let saved_review = fs::read(output.join("review.html")).unwrap();
    assert!(
        discovery::discover(&input, &output, 0, &DiscoverySettings::default())
            .unwrap_err()
            .contains("existing")
    );
    assert_eq!(fs::read(&input).unwrap(), original);
    assert_eq!(
        fs::read(output.join("discovery.json")).unwrap(),
        saved_report
    );
    assert_eq!(fs::read(output.join("review.html")).unwrap(), saved_review);
}

#[test]
fn missing_corrupt_and_missing_channel_inputs_never_report_a_completed_scan() {
    let root = directory("preflight errors");
    let settings = DiscoverySettings::default();
    let missing_output = root.join("missing");
    assert!(discovery::discover(&root.join("missing.wav"), &missing_output, 0, &settings).is_err());
    assert_no_success(&missing_output);
    let corrupt = root.join("corrupt.mp3");
    fs::write(&corrupt, b"not a valid audio recording").unwrap();
    let corrupt_output = root.join("corrupt");
    assert!(discovery::discover(&corrupt, &corrupt_output, 0, &settings).is_err());
    assert_no_success(&corrupt_output);
    let mono = root.join("mono.wav");
    write_wav(&mono, 1, &vec![0; 8000]);
    let channel_output = root.join("missing channel");
    assert!(
        discovery::discover(&mono, &channel_output, 1, &settings)
            .unwrap_err()
            .contains("channel")
    );
    assert_no_success(&channel_output);
    assert!(!channel_output.exists());
}

#[test]
fn a_real_video_container_without_audio_is_rejected() {
    let root = directory("no audio stream");
    let input = root.join("video without audio.mkv");
    let encoded = Command::new(audio::resolve_media_tool("ffmpeg").unwrap())
        .args([
            "-v",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=16x16:d=0.2",
            "-an",
            "-c:v",
            "ffv1",
        ])
        .arg(&input)
        .output()
        .expect("FFmpeg is required for discovery integration validation");
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    let output = root.join("discovery");
    assert!(
        discovery::discover(&input, &output, 0, &DiscoverySettings::default())
            .unwrap_err()
            .contains("audio")
    );
    assert_no_success(&output);
}

#[test]
fn empty_audio_cannot_be_reported_as_a_successful_empty_scan() {
    let root = directory("empty audio");
    let input = root.join("empty.wav");
    write_wav(&input, 1, &[]);
    let output = root.join("discovery");
    assert!(discovery::discover(&input, &output, 0, &DiscoverySettings::default()).is_err());
    assert_no_success(&output);
}

#[test]
fn a_partial_pcm_decode_with_recoverable_ffmpeg_errors_is_not_complete() {
    let root = directory("partial decode");
    let input = root.join("truncated.wav");
    write_wav(&input, 1, &vec![0; 8000]);
    // Keep the complete header, half the declared audio, and half one sample.
    // FFmpeg without -xerror emits 0.5s of audio, logs a PCM decoding error,
    // and returns exit 0. A strict whole-recording scan must refuse that data.
    fs::OpenOptions::new()
        .write(true)
        .open(&input)
        .unwrap()
        .set_len(44 + 8001)
        .unwrap();
    assert_eq!(
        media::inspect(&input).unwrap().channels,
        1,
        "Fixture must reach the decoder after preflight"
    );
    let original = fs::read(&input).unwrap();
    let output = root.join("discovery");
    let error = discovery::discover(&input, &output, 0, &DiscoverySettings::default()).unwrap_err();
    assert!(error.contains("FFmpeg"), "{error}");
    assert_no_success(&output);
    assert!(output.join("failure.json").is_file());
    assert_eq!(fs::read(&input).unwrap(), original);
}

#[test]
fn stereo_selection_preserves_antiphase_tones_and_keeps_other_channels_separate() {
    let root = directory("channel selection");
    let mut opposite = Vec::new();
    let mut different = Vec::new();
    for i in 0..24 * 8000 {
        let t = i as f64 / 8000.0;
        // Known synthetic acoustic sweep, never a claim of an aircraft event.
        let moving = (8000.0 * (2.0 * PI * (280.0 * t - 2.5 * t * t)).sin()).round() as i16;
        let stationary = (8000.0 * (2.0 * PI * 400.0 * t).sin()).round() as i16;
        opposite.extend([moving, -moving]);
        different.extend([moving, stationary]);
    }
    let antiphase = root.join("opposite phase.wav");
    let separate = root.join("different channels.wav");
    write_wav(&antiphase, 2, &opposite);
    write_wav(&separate, 2, &different);
    let mut previous_events = None;
    for (fixture, name, channel, expect_event) in [
        (&antiphase, "opposite left", 0, true),
        (&antiphase, "opposite right", 1, true),
        (&separate, "moving channel", 0, true),
        (&separate, "stationary channel", 1, false),
    ] {
        let output = root.join(name);
        let report =
            discovery::discover(fixture, &output, channel, &DiscoverySettings::default()).unwrap();
        assert_complete(&output, &report);
        assert_eq!(report["source_metadata"]["selected_channel_index"], channel);
        assert_eq!(report["source_metadata"]["available_channels"], 2);
        assert_eq!(report["source_metadata"]["decoded_samples"], 96_000);
        assert_eq!(
            report["group_count"].as_u64().unwrap() > 0,
            expect_event,
            "{name}"
        );
        if expect_event {
            assert_eq!(report["status"], "events_proposed");
            let points = report["events"][0]["ridge"].as_array().unwrap();
            let start = points.first().unwrap()["frequency_hz"].as_f64().unwrap();
            let end = points.last().unwrap()["frequency_hz"].as_f64().unwrap();
            assert!(
                start > end + 40.0,
                "The selected moving tone must retain its downward motion"
            );
            assert!(points.last().unwrap()["time_s"].as_f64().unwrap() < 24.0);
            if let Some(prior) = &previous_events {
                assert_eq!(
                    &report["events"], prior,
                    "Changing another channel or reversing polarity must not alter measured power ridges"
                );
            }
            previous_events = Some(report["events"].clone());
        } else {
            assert_eq!(report["status"], "no_events");
        }
    }
}

#[test]
fn invalid_settings_are_rejected_before_output_is_created() {
    let root = directory("invalid settings");
    let input = root.join("input.wav");
    write_wav(&input, 1, &vec![0; 8000]);
    let baseline = serde_json::to_value(DiscoverySettings::default()).unwrap();
    for (index, (field, value)) in [
        ("hop_s", serde_json::json!(0.0)),
        ("min_support", serde_json::json!(1.1)),
        ("overlap_s", serde_json::json!(120.0)),
        ("window_s", serde_json::json!(2.0)),
        ("max_peaks_per_frame", serde_json::json!(257)),
        ("max_frequency_hz", serde_json::json!(2000.0)),
    ]
    .into_iter()
    .enumerate()
    {
        let mut invalid = baseline.clone();
        invalid[field] = value;
        let settings: DiscoverySettings = serde_json::from_value(invalid).unwrap();
        let output = root.join(format!("bad-{index}"));
        assert!(
            discovery::discover(&input, &output, 0, &settings).is_err(),
            "{field}"
        );
        assert!(
            !output.exists(),
            "Invalid settings must fail before allocating output"
        );
    }
    let mut unknown = baseline;
    unknown["unrecognized_setting"] = serde_json::json!(true);
    assert!(serde_json::from_value::<DiscoverySettings>(unknown).is_err());
    let nonfinite = DiscoverySettings {
        min_contrast: f64::NAN,
        ..DiscoverySettings::default()
    };
    assert!(nonfinite.validate().is_err());
}

#[test]
fn cli_rejects_unknown_flags_unknown_settings_and_invalid_channel_values() {
    let root = directory("CLI controls");
    let input = root.join("input with spaces.wav");
    write_wav(&input, 1, &vec![0; 8000]);
    let mut unknown = serde_json::to_value(DiscoverySettings::default()).unwrap();
    unknown["unknown_parameter"] = serde_json::json!(5);
    let settings_file = root.join("unknown settings.json");
    fs::write(&settings_file, serde_json::to_vec(&unknown).unwrap()).unwrap();
    for (index, extra) in [
        vec!["--unknown-flag".to_owned(), "yes".to_owned()],
        vec!["--channel".to_owned(), "-1".to_owned()],
        vec![
            "--settings".to_owned(),
            settings_file.to_string_lossy().into_owned(),
        ],
    ]
    .into_iter()
    .enumerate()
    {
        let output = root.join(format!("CLI output {index}"));
        let result = Command::new(env!("CARGO_BIN_EXE_geosonnar"))
            .arg("discover")
            .arg("--input")
            .arg(&input)
            .arg("--out")
            .arg(&output)
            .args(extra)
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "Invalid CLI configuration succeeded"
        );
        assert!(!result.stderr.is_empty());
        assert_no_success(&output);
        assert!(!output.exists());
    }
}
