//! Headless real-recording integration of the GUI's measured spectrum and
//! experimental thunder proposals. This adapter never locates a receiver.
use geosonnar::{audio::resolve_media_tool, review_media, thunder};
use std::{fs::OpenOptions, io::Write, path::PathBuf};

fn media_tool(name: &str) -> Result<PathBuf, String> {
    resolve_media_tool(name)
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 6 || args[0] != "--input" || args[2] != "--out" || args[4] != "--duration" {
        return Err("Usage: storm_audio --input FILE.wav --out NEW.json --duration SECONDS".into());
    }
    let output = PathBuf::from(&args[3]);
    if output.exists() {
        return Err("Output already exists".into());
    }
    let duration = args[5]
        .to_str()
        .ok_or("Invalid duration")?
        .parse::<f64>()
        .map_err(|e| e.to_string())?;
    let window = review_media::analyze_window(&review_media::WindowRequest {
        input: PathBuf::from(&args[1]),
        ffmpeg: media_tool("ffmpeg")?,
        ffprobe: media_tool("ffprobe")?,
        channel: 0,
        start_s: 0.0,
        end_s: duration,
        fft_size: 4096,
        hop_s: 0.1,
        max_frequency_hz: 250.0,
    })?;
    let envelope = thunder::ThunderEnvelope {
        samples: window
            .envelope
            .iter()
            .map(|s| thunder::EnvelopeSample {
                time_s: s.time_s,
                rms: s.rms,
            })
            .collect(),
        band_low_hz: window.envelope_band_hz[0],
        band_high_hz: window.envelope_band_hz[1],
        method: window.envelope_method.clone(),
    };
    let proposed = thunder::propose(&envelope, &thunder::ThunderConfig::default())?;
    let report = serde_json::json!({
        "schema_version":1,"status":"completed","source_sha256":window.source_sha256,
        "source_duration_s":window.source_duration_s,"decoded_samples":window.decoded_samples,
        "analysis_sample_rate_hz":window.sample_rate_hz,"fft_size":window.fft_size,
        "spectrogram_columns":window.times_s.len(),"spectrogram_bins":window.frequencies_hz.len(),
        "thunder":proposed,"location_verified":false,
        "limitations":["Real waveform integration only; no independently labelled onset or geolocation accuracy is claimed",
          "Input infrasound is sampled at 100 Hz; the 4000 Hz analysis resampling creates no information above its original 50 Hz Nyquist limit",
          "Proposals remain unreviewed energetic intervals; no source association or receiver coordinate is inferred"]
    });
    let bytes = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| e.to_string())?
        .write_all(&bytes)
        .map_err(|e| e.to_string())?;
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("Storm integration failed: {error}");
        std::process::exit(2);
    }
}
