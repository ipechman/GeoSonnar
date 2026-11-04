//! File-bound, explicitly invoked thunder-envelope measurement and comparison.
//! Input receipts establish byte identity, not source authenticity or location.
use crate::{
    detailed_data::sha256,
    evidence_score::{self, LightningCatalog, LightningProvenance},
    media_tools, review_media,
    thunder::{EnvelopeSample, ThunderEnvelope},
    thunder_envelope::{self as core, EnvelopeSearchPlan, EnvelopeSearchRequest, ObservedEnvelope},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementConfig {
    pub schema_version: u32,
    pub recording: PathBuf,
    pub channel: usize,
    pub start_s: f64,
    pub end_s: f64,
    pub fft_size: usize,
    pub hop_s: f64,
    #[serde(default = "default_band")]
    pub band_hz: [f64; 2],
    pub reviewed_selection_note: String,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
}
fn default_band() -> [f64; 2] {
    [20.0, 250.0]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    pub schema_version: u32,
    pub recording: PathBuf,
    pub observation: PathBuf,
    pub lightning: PathBuf,
    /// Used only when the native interchange file has no embedded provenance.
    pub lightning_provenance: LightningProvenance,
    pub plan: EnvelopeSearchPlan,
    /// Explicit presentation work, included conservatively in the work budget.
    #[serde(default = "default_preview_curves")]
    pub preview_curves: usize,
}
fn default_preview_curves() -> usize {
    1
}

#[derive(Clone, Debug, Serialize)]
struct InputReceipt {
    path: PathBuf,
    bytes: u64,
    sha256: String,
}

fn read_input<T: DeserializeOwned>(path: &Path) -> Result<(T, InputReceipt), String> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("Read input: {e}"))?;
    let file = fs::File::open(&path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err("Envelope configuration/observation exceeds 8 MiB".into());
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "Input must be UTF-8 JSON")?
        .trim_start_matches('\u{feff}');
    let value = serde_json::from_str(text).map_err(|e| format!("Invalid envelope input: {e}"))?;
    Ok((
        value,
        InputReceipt {
            path,
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        },
    ))
}

fn file_receipt(path: &Path) -> Result<InputReceipt, String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err("Input must be a nonempty regular file".into());
    }
    Ok(InputReceipt {
        sha256: sha256(&path)?,
        bytes: metadata.len(),
        path,
    })
}

fn resolve(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.into()
    } else {
        base.join(path)
    }
}

// GUI, case intake and command-line measurement use the same resolver. Explicit
// selections keep config-relative semantics and never fall back to another tool.
fn measurement_tool(base: &Path, configured: Option<&Path>, name: &str) -> Result<PathBuf, String> {
    media_tools::resolve(media_tools::Tool::from_name(name)?, configured, base)
}

fn unchanged(inputs: &[InputReceipt]) -> Result<(), String> {
    for input in inputs {
        let now = file_receipt(&input.path)?;
        if now.bytes != input.bytes || now.sha256 != input.sha256 {
            return Err(format!(
                "Input changed during comparison: {}",
                input.path.display()
            ));
        }
    }
    Ok(())
}

fn fresh_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Use a new output file ({}): {e}", path.display()))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

fn fresh_directory(path: &Path) -> Result<PathBuf, String> {
    fs::create_dir(path)
        .map_err(|e| format!("Choose a new output directory with an existing parent: {e}"))?;
    path.canonicalize().map_err(|e| e.to_string())
}

/// Measures the same channel/time-preserving native PSD envelope used in review.
/// It never detects, confirms, selects or associates an event automatically.
pub fn measure(config_path: &Path, output: &Path) -> Result<Value, String> {
    let (config, config_receipt): (MeasurementConfig, _) = read_input(config_path)?;
    if output.exists() {
        return Err("Choose a new measurement output directory".into());
    }
    let base = config_receipt
        .path
        .parent()
        .ok_or("Configuration has no parent")?;
    let measured = measure_data(&config, base, &AtomicBool::new(false))?;
    let inputs = [config_receipt, measured.recording];
    unchanged(&inputs)?;
    let output = fresh_directory(output)?;
    fresh_json(&output.join("observation.json"), &measured.observation)?;
    let window = measured.window;
    let report = json!({"schema_version":1,"status":"measured_not_classified","config":config,
        "inputs":inputs,"observation":file_receipt(&output.join("observation.json"))?,
        "decoded_samples":window.decoded_samples,"sample_rate_hz":window.sample_rate_hz,"original_sample_rate_hz":measured.original_rate,
        "source_duration_s":window.source_duration_s,"decoded_interval_s":[window.start_s,window.end_s],
        "fft_size":window.fft_size,"hop_samples":window.hop_samples,"time_reference":window.time_reference,
        "sample_time_convention":"STFT window centers, seconds from the first decoded audio sample",
        "location_verified":false,"thunder_classification_verified":false,
        "limitation":"Resampling does not establish original instrument bandwidth, UTC, calibration, or source identity."});
    fresh_json(&output.join("measurement.json"), &report)?;
    Ok(report)
}

struct Measured {
    observation: ObservedEnvelope,
    recording: InputReceipt,
    window: review_media::SpectrogramWindow,
    original_rate: u32,
}

/// Same channel/time-preserving measurement as the CLI, without temporary files.
/// Cancellation stops decoder helpers and is checked between spectral windows.
pub fn measure_observation(
    config: &MeasurementConfig,
    base: &Path,
    cancel: &AtomicBool,
) -> Result<ObservedEnvelope, String> {
    measure_data(config, base, cancel).map(|measured| measured.observation)
}

fn measure_data(
    config: &MeasurementConfig,
    base: &Path,
    cancel: &AtomicBool,
) -> Result<Measured, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Envelope measurement cancelled".into());
    }
    if config.schema_version != 1
        || config.reviewed_selection_note.trim().is_empty()
        || config.reviewed_selection_note.len() > 4000
        || !config.hop_s.is_finite()
        || !(0.05..=5.0).contains(&config.hop_s)
        || config.band_hz.iter().any(|n| !n.is_finite())
        || config.band_hz[0] < 20.0
        || config.band_hz[1] > 250.0
        || config.band_hz[1] <= config.band_hz[0]
    {
        return Err("Measurement requires schema1, a selection note, hop within 0.05..5 seconds and an ordered requested band within 20..250 Hz".into());
    }
    let recording_path = resolve(base, &config.recording);
    let recording_before = file_receipt(&recording_path)?;
    let ffprobe = measurement_tool(base, config.ffprobe.as_deref(), "ffprobe")?;
    let original_rate =
        review_media::original_audio_sample_rate_with_cancel(&recording_path, &ffprobe, cancel)?;
    if f64::from(original_rate) / 2.0 < config.band_hz[1] {
        return Err("Requested bandwidth exceeds the original sample rate's Nyquist limit. Resampling cannot supply missing bandwidth; explicitly choose a reduced band.".into());
    }
    let window = review_media::analyze_window_with_cancel(
        &review_media::WindowRequest {
            input: recording_path,
            ffmpeg: measurement_tool(base, config.ffmpeg.as_deref(), "ffmpeg")?,
            ffprobe,
            channel: config.channel,
            start_s: config.start_s,
            end_s: config.end_s,
            fft_size: config.fft_size,
            hop_s: config.hop_s,
            max_frequency_hz: 250.0,
        },
        cancel,
    )?;
    let recording = InputReceipt {
        path: window.input.clone(),
        bytes: window.source_stamp.bytes,
        sha256: window.source_sha256.clone(),
    };
    if recording.sha256 != recording_before.sha256 || recording.bytes != recording_before.bytes {
        return Err(
            "Recording changed between original-bandwidth inspection and measurement".into(),
        );
    }
    let included = window
        .frequencies_hz
        .iter()
        .enumerate()
        .filter(|(_, hz)| **hz >= config.band_hz[0] && **hz <= config.band_hz[1])
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    if included.len() < 2 {
        return Err("Requested envelope band needs at least two actual FFT bins; increase FFT size or widen the explicit band".into());
    }
    let spacing = window.sample_rate_hz as f64 / window.fft_size as f64;
    let samples = window
        .times_s
        .iter()
        .enumerate()
        .map(|(time_index, time)| {
            let energy: f64 = included
                .iter()
                .map(|bin| {
                    let db = window.power_db[time_index * window.frequencies_hz.len() + bin];
                    // The spectrogram's display floor must not turn decoded silence
                    // into a constant artificial RMS envelope that can match a storm.
                    if db <= -200.0 {
                        0.0
                    } else {
                        10.0_f64.powf(f64::from(db) / 10.0) * spacing
                    }
                })
                .sum();
            EnvelopeSample {
                time_s: *time,
                rms: energy.sqrt(),
            }
        })
        .collect::<Vec<_>>();
    if !samples.iter().any(|s| s.rms > 0.0) {
        return Err("Selected interval has no represented band energy above the numerical PSD floor; silence cannot be compared".into());
    }
    let observation = ObservedEnvelope {
        recording_sha256:window.source_sha256.clone(), channel:window.channel,
        envelope:ThunderEnvelope {
            samples,
            band_low_hz:window.frequencies_hz[included[0]], band_high_hz:window.frequencies_hz[*included.last().unwrap()],
            method:"Square root of integrated one-sided periodic-Hann PSD over explicitly selected FFT-bin centers; -200 dBFS/Hz display-floor bins treated as unrepresented energy, not an artificial signal. Not a brick-wall time-domain filter.".into(),
        },
        reviewed_selection_note:config.reviewed_selection_note.clone(),
    };
    unchanged(std::slice::from_ref(&recording))?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Envelope measurement cancelled".into());
    }
    Ok(Measured {
        observation,
        recording,
        window,
        original_rate,
    })
}

struct Prepared {
    request: EnvelopeSearchRequest,
    catalog: LightningCatalog,
    inputs: Vec<InputReceipt>,
    preview_curves: usize,
    total_work_upper_bound: u64,
}

fn prepare(config_path: &Path) -> Result<Prepared, String> {
    let (config, config_receipt): (RunConfig, _) = read_input(config_path)?;
    if config.schema_version != 1 || config.preview_curves > 5 {
        return Err("Envelope run requires schema_version1 and 0..5 preview curves".into());
    }
    let base = config_receipt
        .path
        .parent()
        .ok_or("Configuration has no parent")?;
    let (observation, observation_receipt): (ObservedEnvelope, _) =
        read_input(&resolve(base, &config.observation))?;
    let recording = file_receipt(&resolve(base, &config.recording))?;
    if recording.sha256 != observation.recording_sha256 {
        return Err("Observed envelope belongs to different recording bytes".into());
    }
    let catalog = evidence_score::import_lightning(
        &resolve(base, &config.lightning),
        &config.lightning_provenance,
    )?;
    let lightning = InputReceipt {
        path: catalog.input_path.clone(),
        bytes: catalog.input_bytes,
        sha256: catalog.input_sha256.clone(),
    };
    let request = config.plan.with_observation(observation);
    let estimate = core::preflight(&request, &catalog)?;
    let preview_curves = config.preview_curves.min(request.plan.keep_best);
    // Each one-receiver overlay is bounded above by an entire search. This is
    // intentionally conservative, and accounts for recomputation without
    // silently increasing the explicitly approved work budget.
    let total_work_upper_bound = estimate
        .estimated_work_units
        .checked_mul(1 + preview_curves as u64)
        .ok_or("Envelope preview work overflowed")?;
    if total_work_upper_bound > request.plan.limits.max_work_units {
        return Err(format!(
            "Search plus {preview_curves} requested previews needs at most {total_work_upper_bound} estimated work units; increase the declared limit within hard bounds, reduce previews, or narrow the search"
        ));
    }
    Ok(Prepared {
        request,
        catalog,
        inputs: vec![config_receipt, observation_receipt, recording, lightning],
        preview_curves,
        total_work_upper_bound,
    })
}

/// Validates actual source files and estimates work; does not score or download.
pub fn preview(config_path: &Path) -> Result<Value, String> {
    let prepared = prepare(config_path)?;
    let estimate = core::preflight(&prepared.request, &prepared.catalog)?;
    unchanged(&prepared.inputs)?;
    Ok(
        json!({"schema_version":1,"status":"ready_for_explicit_run","estimate":estimate,
        "inputs":prepared.inputs,"location_verified":false,"preview_curves":prepared.preview_curves,"total_work_upper_bound":prepared.total_work_upper_bound,
        "observation_semantics":"Recording bytes verified; supplied measurement values and analyst interpretation are not independently authenticated."}),
    )
}

/// The output directory starts with bound request/input receipts. A terminal
/// receipt is written last; its absence means an interrupted or failed write.
pub fn run(
    config_path: &Path,
    output: &Path,
    cancel: &AtomicBool,
    progress: impl FnMut(&core::EnvelopeProgress),
) -> Result<Value, String> {
    let prepared = prepare(config_path)?;
    let output = fresh_directory(output)?;
    fresh_json(&output.join("request.json"), &prepared.request)?;
    fresh_json(
        &output.join("started.json"),
        &json!({"schema_version":1,"inputs":prepared.inputs,
        "location_verified":false,"status":"started","completion_file":"completed.json",
        "preview_curves":prepared.preview_curves,"total_work_upper_bound":prepared.total_work_upper_bound,
        "replay":"Requires the original recording and lightning inputs; normalized inputs are not copied."}),
    )?;
    let result = (|| {
        let report = core::search(&prepared.request, &prepared.catalog, cancel, progress)?;
        unchanged(&prepared.inputs)?;
        fresh_json(&output.join("results.json"), &report)?;
        let mut overlays = Vec::new();
        if !report.cancelled && !cancel.load(std::sync::atomic::Ordering::Relaxed) {
            for hypothesis in report.hypotheses.iter().take(prepared.preview_curves) {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let Some(view) = core::overlay_with_cancel(
                    &prepared.request,
                    &prepared.catalog,
                    hypothesis,
                    cancel,
                )?
                else {
                    break;
                };
                overlays.push(view);
            }
        }
        fresh_json(&output.join("overlays.json"), &overlays)?;
        let html = overlay_html(&report, &overlays);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output.join("review.html"))
            .map_err(|e| e.to_string())?;
        file.write_all(html.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        let artifacts = [
            "request.json",
            "started.json",
            "results.json",
            "overlays.json",
            "review.html",
        ]
        .iter()
        .map(|name| file_receipt(&output.join(name)))
        .collect::<Result<Vec<_>, _>>()?;
        unchanged(&prepared.inputs)?;
        let expected_previews = prepared.preview_curves.min(report.hypotheses.len());
        let presentation_complete = overlays.len() == expected_previews;
        let cancelled = report.cancelled || cancel.load(std::sync::atomic::Ordering::Relaxed);
        let complete = report.complete && presentation_complete && !cancelled;
        let completed = json!({"schema_version":1,"status":if cancelled {"cancelled"} else if complete {"completed"} else {"incomplete"},
            "complete":complete,"search_complete":report.complete,"presentation_complete":presentation_complete,
            "cancelled":cancelled,"preview_curves_written":overlays.len(),"preview_curves_requested":prepared.preview_curves,
            "retained_hypotheses":report.hypotheses.len(),
            "artifacts":artifacts,"location_verified":false,"independent_field_validation":false});
        fresh_json(&output.join("completed.json"), &completed)?;
        Ok(completed)
    })();
    if let Err(error) = &result {
        let _ = fresh_json(
            &output.join("failure.json"),
            &json!({"schema_version":1,"status":"failed","error":error,"location_verified":false}),
        );
    }
    result
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn overlay_html(report: &core::EnvelopeReport, overlays: &[core::EnvelopeOverlay]) -> String {
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Thunder envelope review</title><style>body{font:16px system-ui;max-width:1100px;margin:2rem auto;padding:1rem;color:#222;background:#f1f2f3}table{border-collapse:collapse;width:100%}th,td{padding:.5rem;border:1px solid #aaa;text-align:left}svg{background:white;width:100%;height:auto}section{margin:2rem 0}p{line-height:1.5}</style><h1>Thunder envelope review</h1><p>Unverified hypotheses. Cosine similarity measures shape agreement; it is not location confidence or a thunder classification. The observed and assumed curves each use their own peak normalization for display only.</p>",
    );
    html.push_str(&format!("<p>Completed receivers: {} / {}. Evaluated offsets: {} / {}. Cancelled: {}.</p><table><thead><tr><th>Receiver</th><th>Latitude</th><th>Longitude</th><th>Clock offset (s)</th><th>Cosine</th><th>Clock boundary</th></tr></thead><tbody>",report.progress.completed_receivers,report.progress.total_receivers,report.progress.evaluated_offsets,report.progress.total_offsets,report.cancelled));
    for h in &report.hypotheses {
        html.push_str(&format!("<tr><td>{}</td><td>{:.6}</td><td>{:.6}</td><td>{:.3}</td><td>{:.8}</td><td>{}</td></tr>",escape(&h.receiver_id),h.observer.latitude_deg,h.observer.longitude_deg,h.clock_offset_s,h.cosine_similarity,h.clock_at_boundary));
    }
    html.push_str("</tbody></table><h2>Limitations</h2><ul>");
    for item in &report.limitations {
        html.push_str(&format!("<li>{}</li>", escape(item)));
    }
    html.push_str("</ul>");
    for overlay in overlays {
        let first = overlay.times_s[0];
        let last = overlay.times_s[overlay.times_s.len() - 1];
        html.push_str(&format!("<section><h2>{}: offset {:.3} s</h2><p>Blue: observed RMS. Orange: assumed relative RMS. Audio interval {:.3}–{:.3} s.</p><svg viewBox=\"0 0 1000 250\" role=\"img\" aria-label=\"Observed and predicted thunder envelope\"><path d=\"M40 10V220H980\" fill=\"none\" stroke=\"#555\"/>",escape(&overlay.hypothesis.receiver_id),overlay.hypothesis.clock_offset_s,first,last));
        for (values, color) in [
            (&overlay.observed_rms, "#2266b0"),
            (&overlay.predicted_relative_rms, "#b75410"),
        ] {
            let peak = values
                .iter()
                .copied()
                .fold(0.0_f64, f64::max)
                .max(f64::MIN_POSITIVE);
            let points = overlay
                .times_s
                .iter()
                .zip(values)
                .map(|(t, v)| {
                    format!(
                        "{:.3},{:.3}",
                        40.0 + 940.0 * (t - first) / (last - first),
                        220.0 - 200.0 * (v / peak)
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            html.push_str(&format!("<polyline points=\"{points}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"1.8\"/>"));
        }
        html.push_str("</svg></section>");
    }
    html.push_str("</html>");
    html
}
