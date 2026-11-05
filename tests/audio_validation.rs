//! Actual FFmpeg/FFprobe integration checks. These are never silently skipped.
//! Fixtures are generated locally; no network, prior output, or source video is
//! required. Run with `cargo test --test audio_validation`.

mod support;
use support::write_wav;

use geosonnar::{audio, media};
use serde_json::Value;
use std::f64::consts::PI;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn directory(name: &str) -> PathBuf {
    let unique = format!(
        "{}-{}-{}-{}",
        name,
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("output/benchmarks/audio/integration")
        .join(unique);
    fs::create_dir_all(&root).unwrap();
    root
}

fn tones(path: &Path, seconds: f64, frequencies: &[f64]) {
    let rate = 48_000;
    let mut samples = Vec::new();
    for i in 0..(seconds * f64::from(rate)).round() as usize {
        for &frequency in frequencies {
            let value = 0.25
                * (2.0 * PI * frequency.abs() * i as f64 / f64::from(rate)).sin()
                * frequency.signum();
            samples.push((value * 32767.0).round() as i16);
        }
    }
    write_wav(path, rate, frequencies.len() as u16, &samples);
}

fn record(directory: &Path, header: &Value, index: usize) -> (f64, Vec<f32>) {
    let bytes = fs::read(directory.join("spectrogram.gspec")).unwrap();
    assert_eq!(&bytes[..8], b"GSPEC001");
    assert_eq!(
        bytes.len() as u64,
        header["binary"]["expected_file_bytes"].as_u64().unwrap()
    );
    let width = header["binary"]["record_bytes"].as_u64().unwrap() as usize;
    let offset = 8 + index * width;
    let time = f64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    let bins = bytes[offset + 8..offset + width]
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    (time, bins)
}

#[test]
fn selected_stereo_channels_keep_their_own_tones_and_dimensions() {
    let root = directory("stereo");
    let input = root.join("stéréo input with spaces.wav");
    tones(&input, 3.25, &[125.0, 250.0]);
    let info = media::inspect(&input).unwrap();
    assert_eq!((info.channels, info.sample_rate_hz), (2, 48_000));
    assert!((info.duration_s - 3.25).abs() < 1e-6);
    for (channel, bin) in [(0, 128), (1, 256)] {
        let out = root.join(format!("channel-{channel}"));
        let header = audio::analyze(&input, &out, channel).unwrap();
        assert_eq!(header["selected_channel_index"], channel);
        assert_eq!(header["channels_averaged"], false);
        assert_eq!(header["decoded_samples"], 13_000);
        assert_eq!(header["frame_count"], 9);
        assert_eq!(header["frequency_bins"], 820);
        let (time, spectrum) = record(&out, &header, 3);
        assert!((time - 1.262).abs() < 1e-12);
        let peak = spectrum
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(peak, bin);
        assert!(
            spectrum[bin] > -20.0,
            "Valid tone was unexpectedly attenuated"
        );
        let other = if bin == 128 { 256 } else { 128 };
        assert!(
            spectrum[bin] - spectrum[other] > 55.0,
            "Channels leaked or were averaged"
        );
        assert!((record(&out, &header, 8).0 - 2.512).abs() < 1e-12);
        assert!(header["last_frame_center_s"].as_f64().unwrap() + 0.512 <= 3.25);
    }
}

#[test]
fn opposite_phase_stereo_does_not_cancel_the_selected_channel() {
    let root = directory("opposite-phase");
    let input = root.join("opposite.wav");
    tones(&input, 2.0, &[125.0, -125.0]);
    let out = root.join("analysis");
    let header = audio::analyze(&input, &out, 0).unwrap();
    let (_, spectrum) = record(&out, &header, 1);
    assert!(
        spectrum[128] > -20.0,
        "Averaging opposite-phase channels would erase this signal"
    );
}

#[test]
fn downsampling_rejects_a_tone_that_would_alias_into_the_aircraft_band() {
    let root = directory("anti-alias");
    let input = root.join("alias.wav");
    let samples: Vec<i16> = (0..48_000 * 3)
        .map(|i| {
            let t = f64::from(i) / 48_000.0;
            let signal = 0.2 * ((2.0 * PI * 125.0 * t).sin() + (2.0 * PI * 3500.0 * t).sin());
            (signal * 32767.0).round() as i16
        })
        .collect();
    write_wav(&input, 48_000, 1, &samples);
    let out = root.join("analysis");
    let header = audio::analyze(&input, &out, 0).unwrap();
    let (_, spectrum) = record(&out, &header, 3);
    assert!(
        spectrum[128] - spectrum[512] > 60.0,
        "3500 Hz must not alias to 500 Hz when resampling to 4000 Hz"
    );
}

#[test]
fn malformed_missing_short_and_missing_channel_inputs_never_report_completion() {
    let root = directory("rejections");
    assert!(audio::analyze(&root.join("missing.wav"), &root.join("missing"), 0).is_err());
    assert!(media::inspect(&root.join("missing.wav")).is_err());
    let invalid = root.join("invalid.mp4");
    fs::write(&invalid, b"this is not an encoded recording").unwrap();
    assert!(audio::analyze(&invalid, &root.join("invalid"), 0).is_err());
    assert!(media::inspect(&invalid).is_err());
    let short = root.join("short.wav");
    tones(&short, 0.25, &[125.0]);
    let short_out = root.join("short");
    assert!(audio::analyze(&short, &short_out, 0).is_err());
    assert!(!short_out.join("spectrogram.json").exists());
    assert!(!short_out.join("spectrogram.pgm").exists());
    let normal = root.join("normal.wav");
    tones(&normal, 2.0, &[125.0]);
    let missing_channel = root.join("missing-channel");
    assert!(
        audio::analyze(&normal, &missing_channel, 1)
            .unwrap_err()
            .contains("channel")
    );
    assert!(!missing_channel.exists());
}

#[test]
fn a_real_video_container_without_audio_is_rejected() {
    let root = directory("no-audio");
    let input = root.join("silent-video.mkv");
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
        .expect("FFmpeg is required for actual media integration validation");
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    assert!(media::inspect(&input).unwrap_err().contains("audio"));
    assert!(
        audio::analyze(&input, &root.join("analysis"), 0)
            .unwrap_err()
            .contains("audio")
    );
    assert!(!root.join("analysis/spectrogram.json").exists());
}

#[test]
fn input_data_and_existing_results_are_preserved() {
    let root = directory("preservation");
    let input = root.join("input.wav");
    tones(&input, 2.0, &[125.0]);
    let before = fs::read(&input).unwrap();
    let out = root.join("analysis");
    let header = audio::analyze(&input, &out, 0).unwrap();
    let binary_before = fs::read(out.join("spectrogram.gspec")).unwrap();
    assert!(
        audio::analyze(&input, &out, 0)
            .unwrap_err()
            .contains("Refusing to overwrite")
    );
    assert_eq!(fs::read(&input).unwrap(), before);
    assert_eq!(
        fs::read(out.join("spectrogram.gspec")).unwrap(),
        binary_before
    );
    assert_eq!(header["status"], "audio_analyzed_location_not_inferred");
    assert!(header.get("detected").is_none());
    assert!(header.get("latitude_deg").is_none());
}
