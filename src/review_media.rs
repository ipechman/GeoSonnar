//! Bounded media previews for manual review. These are measured audio/video
//! windows, not event classifications or location evidence.
use crate::{audio::SpectralEngine, detailed_data::sha256, media::quiet_command};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread::{self, JoinHandle},
    time::{Duration, Instant, UNIX_EPOCH},
};

pub const SAMPLE_RATE_HZ: usize = 4000;
pub const MAX_WINDOW_S: f64 = 300.0;
const MAX_CELLS: usize = 8_000_000;
const MAX_FRAME_PIXELS: usize = 1920 * 1080;
const STDERR_LIMIT: usize = 128 * 1024;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SourceStamp {
    pub path: PathBuf,
    pub bytes: u64,
    pub modified_unix_ns: u64,
}

pub fn source_stamp(input: &Path) -> Result<SourceStamp, String> {
    let path = input
        .canonicalize()
        .map_err(|e| format!("Recording unavailable: {e}"))?;
    let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err("Media preview needs a nonempty recording file".into());
    }
    let modified = metadata
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    Ok(SourceStamp {
        path,
        bytes: metadata.len(),
        modified_unix_ns: u64::try_from(modified)
            .map_err(|_| "Unsupported recording modification time")?,
    })
}

#[derive(Clone, Debug)]
pub struct WindowRequest {
    pub input: PathBuf,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub channel: usize,
    pub start_s: f64,
    pub end_s: f64,
    pub fft_size: usize,
    pub hop_s: f64,
    pub max_frequency_hz: f64,
}

impl WindowRequest {
    pub fn validate(&self) -> Result<(), String> {
        valid_interval(self.start_s, self.end_s)?;
        if !(256..=16384).contains(&self.fft_size)
            || !self.fft_size.is_power_of_two()
            || !self.hop_s.is_finite()
            || self.hop_s < 1.0 / SAMPLE_RATE_HZ as f64
            || self.hop_s > self.fft_size as f64 / SAMPLE_RATE_HZ as f64
            || !self.max_frequency_hz.is_finite()
            || self.max_frequency_hz <= 0.
            || self.max_frequency_hz > SAMPLE_RATE_HZ as f64 / 2.
            || self.channel >= 64
        {
            return Err(
                "Invalid review FFT, hop, frequency bounds or channel (FFT 256–16384, max 2000 Hz)"
                    .into(),
            );
        }
        let (first, last, hop) = self.sample_bounds();
        if last <= first || last - first < self.fft_size {
            return Err("Review interval must contain a complete FFT window".into());
        }
        let columns = 1 + (last - first - self.fft_size) / hop;
        let bins = (self.max_frequency_hz * self.fft_size as f64 / SAMPLE_RATE_HZ as f64).floor()
            as usize
            + 1;
        if columns.saturating_mul(bins) > MAX_CELLS {
            return Err(
                "Spectrogram exceeds 8 million cells; shorten the interval or increase the hop"
                    .into(),
            );
        }
        if columns.saturating_mul(self.fft_size) > 64_000_000 {
            return Err("Spectrogram exceeds the bounded FFT work budget; increase the hop or shorten the interval".into());
        }
        Ok(())
    }

    fn sample_bounds(&self) -> (usize, usize, usize) {
        (
            (self.start_s * SAMPLE_RATE_HZ as f64).ceil() as usize,
            (self.end_s * SAMPLE_RATE_HZ as f64).floor() as usize,
            (self.hop_s * SAMPLE_RATE_HZ as f64).round().max(1.) as usize,
        )
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct EnvelopeSample {
    pub time_s: f64,
    pub rms: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpectrogramWindow {
    pub input: PathBuf,
    pub source_sha256: String,
    pub source_stamp: SourceStamp,
    pub channel: usize,
    pub source_duration_s: f64,
    pub requested_start_s: f64,
    pub requested_end_s: f64,
    /// Actual complete decoded sample interval; the end is exclusive.
    pub start_s: f64,
    pub end_s: f64,
    pub decoded_samples: usize,
    pub sample_rate_hz: usize,
    pub fft_size: usize,
    pub hop_samples: usize,
    pub times_s: Vec<f64>,
    pub frequencies_hz: Vec<f64>,
    /// Time-major: power_db[time_index * frequencies_hz.len() + frequency_index].
    /// One-sided periodic-Hann power spectral density, dBFS/Hz. No row pooling.
    pub power_db: Vec<f32>,
    pub envelope: Vec<EnvelopeSample>,
    pub envelope_band_hz: [f64; 2],
    pub envelope_method: String,
    pub time_reference: String,
}

fn valid_interval(start: f64, end: f64) -> Result<(), String> {
    if !start.is_finite()
        || !end.is_finite()
        || start < 0.
        || start >= end
        || end > 86_401.
        || end - start > MAX_WINDOW_S
    {
        return Err(
            "Choose an ordered review interval of at most 300 seconds within a 24-hour recording"
                .into(),
        );
    }
    Ok(())
}

fn tool(path: &Path, name: &str) -> Result<PathBuf, String> {
    let resolved = path
        .canonicalize()
        .map_err(|e| format!("Select the actual {name} executable: {e}"))?;
    if !resolved.is_file() {
        return Err(format!("Selected {name} is not a file"));
    }
    Ok(resolved)
}

fn read_capped(mut reader: impl Read, limit: usize) -> Result<(Vec<u8>, bool), String> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut excess = false;
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        };
        let keep = count.min(limit.saturating_sub(data.len()));
        data.extend_from_slice(&buffer[..keep]);
        excess |= keep != count;
        // Drain remaining output so the process cannot block on a full pipe;
        // the parent enforces an independent wall-time bound.
    }
    Ok((data, excess))
}

fn run_bounded(
    command: Command,
    stdout_limit: usize,
    timeout_s: u64,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    run_bounded_with_cancel(command, stdout_limit, timeout_s, None)
}

fn check_cancel(cancel: Option<&AtomicBool>) -> Result<(), String> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        Err("Media measurement cancelled".into())
    } else {
        Ok(())
    }
}

fn run_bounded_with_cancel(
    mut command: Command,
    stdout_limit: usize,
    timeout_s: u64,
    cancel: Option<&AtomicBool>,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    check_cancel(cancel)?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot launch media helper: {e}"))?;
    let stdout = child.stdout.take().ok_or("Missing helper output pipe")?;
    let stderr = child.stderr.take().ok_or("Missing helper error pipe")?;
    let output_reader = thread::spawn(move || read_capped(stdout, stdout_limit));
    let error_reader = thread::spawn(move || read_capped(stderr, STDERR_LIMIT));
    let start = Instant::now();
    let status = loop {
        if let Err(message) = check_cancel(cancel) {
            let _ = child.kill();
            let _ = child.wait();
            break Err(message);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() < Duration::from_secs(timeout_s) => {
                thread::sleep(Duration::from_millis(10))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("Media helper exceeded its preview time limit".to_owned());
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(e.to_string());
            }
        }
    };
    let (stdout, output_excess) = output_reader
        .join()
        .map_err(|_| "Media output reader failed")??;
    let (stderr, error_excess) = error_reader
        .join()
        .map_err(|_| "Media error reader failed")??;
    let status = status?;
    if output_excess || error_excess {
        return Err("Media helper output exceeded the bounded preview budget".into());
    }
    if !status.success() {
        return Err(format!(
            "Media helper failed: {}",
            String::from_utf8_lossy(&stderr)
                .chars()
                .take(1500)
                .collect::<String>()
        ));
    }
    Ok((stdout, stderr))
}

struct MediaDescription {
    duration_s: f64,
    format_start_s: f64,
    audio_start_s: f64,
    channels: Option<usize>,
    sample_rate_hz: Option<u32>,
    has_video: bool,
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
}

fn describe(input: &Path, ffprobe: &Path) -> Result<MediaDescription, String> {
    describe_with_cancel(input, ffprobe, None)
}
fn describe_with_cancel(
    input: &Path,
    ffprobe: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<MediaDescription, String> {
    check_cancel(cancel)?;
    let mut command = quiet_command(tool(ffprobe, "FFprobe")?);
    command.args(["-v", "error", "-show_entries",
        "format=duration,start_time:stream=codec_type,channels,sample_rate,width,height,start_time:stream_disposition=attached_pic",
        "-of", "json"]).arg(input);
    let (bytes, errors) = run_bounded_with_cancel(command, 1024 * 1024, 20, cancel)?;
    if !errors.is_empty() {
        return Err("FFprobe reported errors while reading the recording".into());
    }
    let report: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid FFprobe response: {e}"))?;
    let duration_s = number(&report["format"]["duration"])
        .filter(|d| *d > 0. && *d <= 86_401.)
        .ok_or("Recording needs a finite positive duration of at most 24 hours")?;
    let format_start_s = number(&report["format"]["start_time"]).unwrap_or(0.);
    let streams = report["streams"]
        .as_array()
        .ok_or("FFprobe omitted media streams")?;
    let audio = streams.iter().find(|s| s["codec_type"] == "audio");
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] != 1);
    if let Some(video) = video {
        let width = video["width"].as_u64().ok_or("Missing video width")?;
        let height = video["height"].as_u64().ok_or("Missing video height")?;
        if width == 0 || height == 0 || width.saturating_mul(height) > 8192 * 8192 {
            return Err("Source video dimensions exceed the review decoder limit".into());
        }
    }
    Ok(MediaDescription {
        duration_s,
        format_start_s,
        audio_start_s: audio
            .and_then(|s| number(&s["start_time"]))
            .unwrap_or(format_start_s),
        channels: audio
            .and_then(|s| s["channels"].as_u64())
            .and_then(|n| usize::try_from(n).ok()),
        sample_rate_hz: audio
            .and_then(|s| number(&s["sample_rate"]))
            .filter(|n| *n >= 1.0 && *n <= u32::MAX as f64 && n.fract() == 0.0)
            .map(|n| n as u32),
        has_video: video.is_some(),
    })
}

/// Original audio rate before review resampling, using the bounded probe path.
pub fn original_audio_sample_rate(input: &Path, ffprobe: &Path) -> Result<u32, String> {
    describe(input, ffprobe)?
        .sample_rate_hz
        .ok_or_else(|| "Recording has no valid original audio sample rate".into())
}

pub fn original_audio_sample_rate_with_cancel(
    input: &Path,
    ffprobe: &Path,
    cancel: &AtomicBool,
) -> Result<u32, String> {
    describe_with_cancel(input, ffprobe, Some(cancel))?
        .sample_rate_hz
        .ok_or_else(|| "Recording has no valid original audio sample rate".into())
}

type SpectralData = (Vec<f64>, Vec<f64>, Vec<f32>, Vec<EnvelopeSample>, [f64; 2]);

#[cfg(test)]
fn spectral_data(request: &WindowRequest, samples: &[f32]) -> Result<SpectralData, String> {
    spectral_data_with_cancel(request, samples, None)
}

fn spectral_data_with_cancel(
    request: &WindowRequest,
    samples: &[f32],
    cancel: Option<&AtomicBool>,
) -> Result<SpectralData, String> {
    check_cancel(cancel)?;
    request.validate()?;
    let (first, last, hop) = request.sample_bounds();
    if samples.len() > last - first
        || samples.len() < request.fft_size
        || samples.iter().any(|n| !n.is_finite())
    {
        return Err("Decoded review audio has too few, excess or nonfinite samples".into());
    }
    let spacing = SAMPLE_RATE_HZ as f64 / request.fft_size as f64;
    let bins = (request.max_frequency_hz * request.fft_size as f64 / SAMPLE_RATE_HZ as f64).floor()
        as usize
        + 1;
    let frequencies = (0..bins).map(|i| i as f64 * spacing).collect::<Vec<_>>();
    let band_first = (20. / spacing).ceil() as usize;
    let band_last = (250. / spacing).floor() as usize;
    let mut engine =
        SpectralEngine::configured(request.fft_size, request.max_frequency_hz.max(250.));
    let mut times = Vec::new();
    let mut power = Vec::new();
    let mut envelope = Vec::new();
    for offset in (0..=samples.len() - request.fft_size).step_by(hop) {
        check_cancel(cancel)?;
        let time = (first + offset + request.fft_size / 2) as f64 / SAMPLE_RATE_HZ as f64;
        let spectrum = engine.power_db(&samples[offset..offset + request.fft_size]);
        let rms = (spectrum[band_first..=band_last]
            .iter()
            .map(|db| 10.0_f64.powf(f64::from(*db) / 10.) * spacing)
            .sum::<f64>())
        .sqrt();
        power.extend_from_slice(&spectrum[..bins]);
        times.push(time);
        envelope.push(EnvelopeSample { time_s: time, rms });
    }
    Ok((
        times,
        frequencies,
        power,
        envelope,
        [band_first as f64 * spacing, band_last as f64 * spacing],
    ))
}

pub fn analyze_window(request: &WindowRequest) -> Result<SpectrogramWindow, String> {
    analyze_window_impl(request, None)
}

pub fn analyze_window_with_cancel(
    request: &WindowRequest,
    cancel: &AtomicBool,
) -> Result<SpectrogramWindow, String> {
    analyze_window_impl(request, Some(cancel))
}

fn analyze_window_impl(
    request: &WindowRequest,
    cancel: Option<&AtomicBool>,
) -> Result<SpectrogramWindow, String> {
    check_cancel(cancel)?;
    request.validate()?;
    let stamp = source_stamp(&request.input)?;
    let initial_hash = sha256(&stamp.path)?;
    let info = describe_with_cancel(&stamp.path, &request.ffprobe, cancel)?;
    if info
        .channels
        .is_none_or(|channels| request.channel >= channels)
    {
        return Err("Selected audio channel does not exist in the first audio stream".into());
    }
    if request.end_s > info.duration_s + 1e-6 {
        return Err("Review interval extends beyond the recording duration".into());
    }
    let (first, last, hop) = request.sample_bounds();
    let mut command = quiet_command(tool(&request.ffmpeg, "FFmpeg")?);
    // Sample-based trim AFTER resampling matches discovery's decoded-input
    // timeline, including codec priming. Do not substitute an imprecise seek.
    let filter = format!(
        "pan=mono|c0=c{},aresample={SAMPLE_RATE_HZ},atrim=start_sample={first}:end_sample={last},asetpts=PTS-STARTPTS",
        request.channel
    );
    command
        .args(["-v", "error", "-xerror", "-nostdin", "-i"])
        .arg(&stamp.path)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-af", &filter, "-t"])
        .arg(format!(
            "{:.9}",
            (last - first) as f64 / SAMPLE_RATE_HZ as f64
        ))
        .args(["-f", "f32le", "-acodec", "pcm_f32le", "pipe:1"]);
    let (bytes, errors) = run_bounded_with_cancel(command, (last - first) * 4, 120, cancel)?;
    if !errors.is_empty() || bytes.len() % 4 != 0 {
        return Err("Audio decode reported errors or ended inside a sample".into());
    }
    let samples = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect::<Vec<_>>();
    let (times_s, frequencies_hz, power_db, envelope, envelope_band_hz) =
        spectral_data_with_cancel(request, &samples, cancel)?;
    if source_stamp(&request.input)? != stamp || sha256(&stamp.path)? != initial_hash {
        return Err("Recording changed while decoding its review window".into());
    }
    check_cancel(cancel)?;
    Ok(SpectrogramWindow {
        input:stamp.path.clone(), source_sha256:initial_hash, source_stamp:stamp,
        channel:request.channel, source_duration_s:info.duration_s,
        requested_start_s:request.start_s, requested_end_s:request.end_s,
        start_s:first as f64 / SAMPLE_RATE_HZ as f64,
        end_s:(first + samples.len()) as f64 / SAMPLE_RATE_HZ as f64,
        decoded_samples:samples.len(), sample_rate_hz:SAMPLE_RATE_HZ,
        fft_size:request.fft_size, hop_samples:hop, times_s, frequencies_hz, power_db,
        envelope, envelope_band_hz,
        envelope_method:"Square root of integrated one-sided periodic-Hann PSD in bins centered between 20 and 250 Hz; band edges quantized to FFT bins, not a brick-wall time-domain filter".into(),
        time_reference:"Seconds from the first decoded audio sample, matching acoustic discovery; complete STFT windows only, no padding or gap filling".into(),
    })
}

#[derive(Clone, Debug)]
pub struct FrameRequest {
    pub input: PathBuf,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub time_s: f64,
    pub max_width: usize,
    pub max_height: usize,
    pub expected_source: Option<SourceStamp>,
}

impl FrameRequest {
    pub fn validate(&self) -> Result<(), String> {
        if !self.time_s.is_finite()
            || self.time_s < 0.
            || self.time_s > 86_400.
            || self.max_width == 0
            || self.max_height == 0
            || self.max_width > 1920
            || self.max_height > 1080
            || self.max_width.saturating_mul(self.max_height) > MAX_FRAME_PIXELS
        {
            return Err("Invalid video time or preview size (maximum 1920×1080)".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct VideoFrame {
    pub source_stamp: SourceStamp,
    pub source_duration_s: f64,
    pub requested_time_s: f64,
    pub frame_time_s: f64,
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
    pub time_reference: String,
}

fn ppm(
    bytes: &[u8],
    max_width: usize,
    max_height: usize,
) -> Result<(usize, usize, Vec<u8>), String> {
    let mut position = 0;
    let mut tokens = Vec::new();
    while tokens.len() < 4 {
        while bytes.get(position).is_some_and(u8::is_ascii_whitespace) {
            position += 1;
        }
        if bytes.get(position) == Some(&b'#') {
            while bytes.get(position).is_some_and(|b| *b != b'\n') {
                position += 1;
            }
            continue;
        }
        let start = position;
        while bytes
            .get(position)
            .is_some_and(|b| !b.is_ascii_whitespace())
        {
            position += 1;
        }
        if start == position || position > 1024 {
            return Err("Invalid or excessive video PPM header".into());
        }
        tokens.push(
            std::str::from_utf8(&bytes[start..position]).map_err(|_| "Invalid video PPM header")?,
        );
    }
    if tokens[0] != "P6" || tokens[3] != "255" {
        return Err("Video preview is not 8-bit RGB PPM".into());
    }
    let width = tokens[1]
        .parse::<usize>()
        .map_err(|_| "Invalid frame width")?;
    let height = tokens[2]
        .parse::<usize>()
        .map_err(|_| "Invalid frame height")?;
    if width == 0 || height == 0 || width > max_width || height > max_height {
        return Err("Decoded video frame exceeds the requested dimensions".into());
    }
    // The separator ends the header; whitespace-valued pixels are real bytes.
    if !bytes.get(position).is_some_and(u8::is_ascii_whitespace) {
        return Err("Missing PPM pixel separator".into());
    }
    if bytes.get(position..position + 2) == Some(b"\r\n") {
        position += 2;
    } else {
        position += 1;
    }
    let pixels = bytes.get(position..).ok_or("Missing video pixels")?;
    if pixels.len() != width * height * 3 {
        return Err("Video frame pixels are truncated or contain extra data".into());
    }
    Ok((width, height, pixels.to_vec()))
}

fn frame_timestamp(diagnostics: &[u8]) -> Result<f64, String> {
    let text = String::from_utf8_lossy(diagnostics);
    let lines = text
        .lines()
        .filter(|line| line.starts_with("[Parsed_showinfo_"));
    let mut time_base = None;
    for line in lines {
        if let Some(value) = line.split("config in time_base:").nth(1)
            && let Some(rational) = value.split_whitespace().next()
        {
            let parts = rational
                .trim_end_matches(',')
                .split('/')
                .collect::<Vec<_>>();
            if parts.len() == 2 {
                let numerator = parts[0].parse::<u64>().ok();
                let denominator = parts[1].parse::<u64>().ok();
                if let (Some(n), Some(d)) = (numerator, denominator)
                    && n > 0
                    && d > 0
                {
                    time_base = Some(n as f64 / d as f64);
                }
            }
        }
        if line.contains("] n:")
            && let Some(pts) = line
                .split(" pts:")
                .nth(1)
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse::<i64>().ok())
            && let Some(base) = time_base
        {
            let timestamp = pts as f64 * base;
            if timestamp.is_finite() {
                return Ok(timestamp);
            }
        }
    }
    Err("Decoder did not supply a video PTS and time base".into())
}

/// Extract the first video frame at/after the requested decoded-audio time.
/// Fast input seeking preserves source PTS; the returned timestamp is measured
/// from the decoded frame. A metadata stamp avoids rehashing an hour-long file
/// on each preview. Reverify its full recording hash before saving evidence.
pub fn extract_frame(request: &FrameRequest) -> Result<Option<VideoFrame>, String> {
    request.validate()?;
    let stamp = source_stamp(&request.input)?;
    if request
        .expected_source
        .as_ref()
        .is_some_and(|expected| *expected != stamp)
    {
        return Err("Recording changed since the media-review window was loaded".into());
    }
    let info = describe(&stamp.path, &request.ffprobe)?;
    if request.time_s >= info.duration_s {
        return Err("Video preview time is outside the recording".into());
    }
    if !info.has_video {
        if source_stamp(&request.input)? != stamp {
            return Err("Recording changed while checking its video stream".into());
        }
        return Ok(None);
    }
    let absolute_time = info.audio_start_s + request.time_s;
    let seek = (absolute_time - info.format_start_s).max(0.);
    let filter = format!(
        "select='gte(t,{absolute_time:.9})',scale=w='max(1,trunc(min(iw*sar,min({},{}*dar))))':h='max(1,trunc(min(ih,min({}, {}/dar))))',setsar=1,showinfo",
        request.max_width, request.max_height, request.max_height, request.max_width
    );
    let mut command = quiet_command(tool(&request.ffmpeg, "FFmpeg")?);
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "info",
            "-xerror",
            "-nostdin",
            "-ss",
        ])
        .arg(format!("{seek:.9}"))
        .args(["-copyts", "-i"])
        .arg(&stamp.path)
        .args([
            "-map",
            "0:V:0",
            "-an",
            "-sn",
            "-dn",
            "-vf",
            &filter,
            "-frames:v",
            "1",
            "-fps_mode",
            "passthrough",
            "-c:v",
            "ppm",
            "-pix_fmt",
            "rgb24",
            "-f",
            "image2pipe",
            "pipe:1",
        ]);
    let (bytes, errors) = run_bounded(
        command,
        request.max_width * request.max_height * 3 + 1024,
        30,
    )?;
    // pts_time is printed with limited decimal precision. Integer PTS and its
    // rational time base preserve frame timing even late in a long recording.
    let timestamp = frame_timestamp(&errors)?;
    let frame_time_s = timestamp - info.audio_start_s;
    if frame_time_s + 1e-5 < request.time_s {
        return Err("Decoder returned a frame before the requested time".into());
    }
    let (width, height, rgb) = ppm(&bytes, request.max_width, request.max_height)?;
    if source_stamp(&request.input)? != stamp {
        return Err("Recording changed while decoding a video frame".into());
    }
    Ok(Some(VideoFrame {
        source_stamp:stamp, source_duration_s:info.duration_s, requested_time_s:request.time_s,
        frame_time_s, width, height, rgb,
        time_reference:"Actual video PTS relative to the first audio-stream start timestamp; requested and decoded times may differ by a frame interval. Alignment assumes continuous audio timestamps; internal timestamp discontinuities are not mapped to decoded-sample time".into(),
    }))
}

#[derive(Clone, Debug)]
pub struct PlaybackRequest {
    pub input: PathBuf,
    pub ffplay: PathBuf,
    pub ffprobe: PathBuf,
    pub channel: usize,
    pub start_s: f64,
    pub end_s: f64,
    pub expected_source: Option<SourceStamp>,
}

/// Owns only the audio player this instance launched. Stop and Drop terminate
/// and reap that helper; unrelated players are never affected.
pub struct AudioPlayback {
    child: Option<Child>,
    errors: Option<JoinHandle<DiagnosticResult>>,
}

type DiagnosticResult = Result<(Vec<u8>, bool), String>;

fn playback_command(
    request: &PlaybackRequest,
    source: &SourceStamp,
    info: &MediaDescription,
) -> Result<Command, String> {
    if info
        .channels
        .is_none_or(|channels| request.channel >= channels)
    {
        return Err("Selected playback channel does not exist in the first audio stream".into());
    }
    if request.end_s > info.duration_s + 1e-6 {
        return Err("Playback interval extends beyond the recording".into());
    }
    let seek = (info.audio_start_s - info.format_start_s + request.start_s).max(0.);
    let mut command = quiet_command(tool(&request.ffplay, "FFplay")?);
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nodisp",
            "-autoexit",
            "-vn",
            "-ast",
            "a:0",
            "-ss",
        ])
        .arg(format!("{seek:.9}"))
        .arg("-t")
        .arg(format!("{:.9}", request.end_s - request.start_s))
        .arg("-af")
        .arg(format!("pan=mono|c0=c{}", request.channel))
        .arg(&source.path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    Ok(command)
}

impl AudioPlayback {
    pub fn start(request: &PlaybackRequest) -> Result<Self, String> {
        valid_interval(request.start_s, request.end_s)?;
        if request.channel >= 64 {
            return Err("Invalid playback channel".into());
        }
        let source = source_stamp(&request.input)?;
        if request
            .expected_source
            .as_ref()
            .is_some_and(|expected| *expected != source)
        {
            return Err("Recording changed since the displayed review window was loaded".into());
        }
        let info = describe(&source.path, &request.ffprobe)?;
        let mut command = playback_command(request, &source, &info)?;
        if source_stamp(&request.input)? != source {
            return Err("Recording changed while starting playback".into());
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("Cannot start audio playback: {e}"))?;
        let stderr = child.stderr.take().ok_or("Missing player error pipe")?;
        let errors = thread::spawn(move || read_capped(stderr, STDERR_LIMIT));
        let mut player = Self {
            child: Some(child),
            errors: Some(errors),
        };
        thread::sleep(Duration::from_millis(100));
        // A very short segment may have completed successfully already. Failed
        // launches still return their bounded diagnostics through this check.
        let _ = player.is_playing()?;
        Ok(player)
    }

    pub fn is_playing(&mut self) -> Result<bool, String> {
        let Some(child) = &mut self.child else {
            return Ok(false);
        };
        let Some(status) = child.try_wait().map_err(|e| e.to_string())? else {
            return Ok(true);
        };
        // Diagnostics can outlive the player (for example, an inherited pipe).
        // Keep this owner active until collection is ready; a UI status poll
        // must never wait for a reader before it can show the close controls.
        if self
            .errors
            .as_ref()
            .is_some_and(|reader| !reader.is_finished())
        {
            return Ok(true);
        }
        self.child = None;
        let (errors, excess) = self
            .errors
            .take()
            .ok_or("Missing playback diagnostics")?
            .join()
            .map_err(|_| "Playback diagnostic reader failed")??;
        if !status.success() || !errors.is_empty() || excess {
            return Err(format!(
                "Audio playback failed: {}",
                String::from_utf8_lossy(&errors)
                    .chars()
                    .take(1500)
                    .collect::<String>()
            ));
        }
        Ok(false)
    }

    /// GUI retirement: request termination now, detach bounded diagnostics,
    /// and reap the owned process on a worker. No process/thread wait runs on
    /// the caller; a guarded GUI process remains the fallback on any failure.
    pub fn stop_in_background(mut self) -> Result<(), String> {
        // Extract before spawning: a failed spawn must never drop AudioPlayback
        // on the UI thread and enter its normal blocking Drop implementation.
        drop(self.errors.take());
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        if matches!(child.try_wait(), Ok(Some(_))) {
            return Ok(());
        }
        child
            .kill()
            .map_err(|error| format!("Cannot request audio helper stop: {error}"))?;
        // Only a process wait is moved here. Stalled diagnostic readers do not
        // accumulate an additional blocked cleanup worker for each retirement.
        // Its existing capped reader normally ends when the killed pipe closes.
        thread::Builder::new()
            .name("reap-audio-player".into())
            .spawn(move || {
                let _ = child.wait();
            })
            .map(|_| ())
            .map_err(|error| format!("Cannot start audio helper cleanup: {error}"))
    }

    /// Exit-only cleanup for a caller that is about to terminate its owned
    /// process group. Request this player's termination without waiting for a
    /// process or diagnostic thread. Normal stop/Drop still reap and collect.
    pub fn stop_for_exit(mut self) -> Result<(), String> {
        // Taking both resources first prevents our normal Drop from joining.
        // Dropping a JoinHandle detaches it; process exit ends any stuck reader.
        drop(self.errors.take());
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        if matches!(child.try_wait(), Ok(Some(_))) {
            return Ok(());
        }
        // This exact owned handle is used, never a name/PID-based broad kill.
        // On Windows the GUI's job remains the fallback if this request fails.
        child
            .kill()
            .map_err(|error| format!("Cannot request audio helper stop: {error}"))
    }

    pub fn stop(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.child.take()
            && !matches!(child.try_wait(), Ok(Some(_)))
        {
            if let Err(error) = child.kill() {
                // Keep ownership if termination failed; Drop may retry.
                self.child = Some(child);
                return Err(format!("Cannot stop audio helper: {error}"));
            }
            child.wait().map_err(|e| e.to_string())?;
        }
        if let Some(reader) = self.errors.take() {
            let _ = reader.join();
        }
        Ok(())
    }
}

impl Drop for AudioPlayback {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_measurement_never_launches_a_helper_or_reads_a_source() {
        let cancel = AtomicBool::new(true);
        assert!(
            analyze_window_with_cancel(&request(), &cancel)
                .unwrap_err()
                .contains("cancelled")
        );
        let command = quiet_command("this-helper-does-not-exist");
        assert!(
            run_bounded_with_cancel(command, 1024, 30, Some(&cancel))
                .unwrap_err()
                .contains("cancelled")
        );
    }

    #[test]
    fn measurement_cancellation_stops_a_running_helper_and_reaps_its_pipes() {
        let cancel = AtomicBool::new(false);
        let mut command = quiet_command(std::env::current_exe().unwrap());
        command.args(["--exact", "review_media::tests::owned_audio_process_is_stopped_and_reaped_without_waiting_for_its_duration"])
            .env("GEOSONNAR_REVIEW_PLAYER_TEST_CHILD", "1");
        let started = Instant::now();
        let result = thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(100));
                cancel.store(true, Ordering::Relaxed);
            });
            run_bounded_with_cancel(command, 4096, 30, Some(&cancel))
        });
        assert!(result.unwrap_err().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    fn request() -> WindowRequest {
        WindowRequest {
            input: PathBuf::new(),
            ffmpeg: PathBuf::new(),
            ffprobe: PathBuf::new(),
            channel: 0,
            start_s: 10.,
            end_s: 14.25,
            fft_size: 1024,
            hop_s: 0.128,
            max_frequency_hz: 1000.,
        }
    }

    #[test]
    fn spectrum_orientation_centers_partial_tail_and_band_rms_are_measured() {
        let request = request();
        let samples = (0..17000)
            .map(|i| (0.5 * (std::f64::consts::TAU * 125. * i as f64 / 4000.).sin()) as f32)
            .collect::<Vec<_>>();
        let (times, frequencies, power, envelope, band) =
            spectral_data(&request, &samples).unwrap();
        assert_eq!(times[0], 10.128);
        assert_eq!(times.len(), 1 + (samples.len() - 1024) / 512);
        assert!(times.last().unwrap() + 0.128 <= 14.25);
        assert_eq!(frequencies[32], 125.);
        assert_eq!(power.len(), times.len() * frequencies.len());
        for row in power.chunks(frequencies.len()) {
            assert_eq!(
                row.iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .unwrap()
                    .0,
                32
            );
        }
        assert!((envelope[3].rms - 0.5 / 2_f64.sqrt()).abs() < 1e-5);
        assert_eq!(band, [23.4375, 250.]);
    }

    #[test]
    fn spectral_band_rejects_out_of_band_tone_and_nonfinite_audio() {
        let request = request();
        let mut samples = (0..17000)
            .map(|i| (0.5 * (std::f64::consts::TAU * 500. * i as f64 / 4000.).sin()) as f32)
            .collect::<Vec<_>>();
        let (_, _, _, envelope, _) = spectral_data(&request, &samples).unwrap();
        assert!(envelope.iter().all(|sample| sample.rms < 1e-6));
        samples[100] = f32::NAN;
        assert!(spectral_data(&request, &samples).is_err());
    }

    #[test]
    fn ppm_retains_whitespace_pixels_and_rejects_excess_or_truncated_data() {
        let mut bytes = b"P6\n2 1\n255\n".to_vec();
        bytes.extend([32, 10, 13, 0, 1, 255]);
        assert_eq!(
            ppm(&bytes, 2, 1).unwrap(),
            (2, 1, vec![32, 10, 13, 0, 1, 255])
        );
        assert!(ppm(&bytes, 1, 1).is_err());
        bytes.pop();
        assert!(ppm(&bytes, 2, 1).is_err());
    }

    #[test]
    fn video_timestamp_uses_integer_pts_not_rounded_printed_seconds() {
        let lines=b"metadata showinfo pts_time:99999\n[Parsed_showinfo_3 @ 0x123] config in time_base: 1/1000, frame_rate: 60/1\n[Parsed_showinfo_3 @ 0x123] n:   0 pts:36000123 pts_time:36000.1 fmt:rgb24\n";
        assert!((frame_timestamp(lines).unwrap() - 36000.123).abs() < 1e-9);
        assert!(frame_timestamp(b"metadata showinfo pts_time:1").is_err());
    }

    #[test]
    fn owned_audio_process_is_stopped_and_reaped_without_waiting_for_its_duration() {
        const CHILD_FLAG: &str = "GEOSONNAR_REVIEW_PLAYER_TEST_CHILD";
        if std::env::var_os(CHILD_FLAG).is_some() {
            thread::sleep(Duration::from_secs(30));
            return;
        }
        let mut child = quiet_command(std::env::current_exe().unwrap())
            .args(["--exact", "review_media::tests::owned_audio_process_is_stopped_and_reaped_without_waiting_for_its_duration"])
            .env(CHILD_FLAG, "1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
        let stderr = child.stderr.take().unwrap();
        let mut player = AudioPlayback {
            child: Some(child),
            errors: Some(thread::spawn(move || read_capped(stderr, STDERR_LIMIT))),
        };
        assert!(player.is_playing().unwrap());
        let started = Instant::now();
        player.stop().unwrap();
        assert!(!player.is_playing().unwrap());
        assert!(player.child.is_none() && player.errors.is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    fn stalled_diagnostics() -> (
        JoinHandle<DiagnosticResult>,
        std::sync::mpsc::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    ) {
        let (release, blocked) = std::sync::mpsc::channel();
        let (finished, completion) = std::sync::mpsc::channel();
        let reader = thread::spawn(move || {
            let _ = blocked.recv();
            let _ = finished.send(());
            Ok((Vec::new(), false))
        });
        (reader, release, completion)
    }

    fn sleeping_playback_child() -> Child {
        quiet_command(std::env::current_exe().unwrap())
            .args(["--exact", "review_media::tests::owned_audio_process_is_stopped_and_reaped_without_waiting_for_its_duration"])
            .env("GEOSONNAR_REVIEW_PLAYER_TEST_CHILD", "1")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().unwrap()
    }

    #[test]
    fn gui_playback_retirement_never_waits_for_stalled_diagnostics_or_drop() {
        for exiting in [false, true] {
            let (reader, release, completed) = stalled_diagnostics();
            let player = AudioPlayback {
                child: None,
                errors: Some(reader),
            };
            let (returned, result) = std::sync::mpsc::channel();
            let stopping = thread::spawn(move || {
                let outcome = if exiting {
                    player.stop_for_exit()
                } else {
                    player.stop_in_background()
                };
                let _ = returned.send(outcome);
            });
            let immediate = result.recv_timeout(Duration::from_secs(1));
            // Always release the fixture before asserting, including on regression.
            let _ = release.send(());
            completed.recv_timeout(Duration::from_secs(2)).unwrap();
            stopping.join().unwrap();
            immediate
                .expect("GUI cleanup waited for the diagnostic reader")
                .unwrap();
        }
    }

    #[test]
    fn exited_playback_poll_does_not_join_stalled_diagnostics_and_retains_owner() {
        let mut child = sleeping_playback_child();
        // Fixture cleanup is allowed to wait; the operation under test is not.
        child.kill().unwrap();
        child.wait().unwrap();
        let (reader, release, completed) = stalled_diagnostics();
        let mut player = AudioPlayback {
            child: Some(child),
            errors: Some(reader),
        };
        let (returned, result) = std::sync::mpsc::channel();
        let (cleanup, cleanup_requested) = std::sync::mpsc::channel();
        let polling = thread::spawn(move || {
            let outcome = player.is_playing();
            let _ = returned.send((outcome, player.child.is_some(), player.errors.is_some()));
            let _ = cleanup_requested.recv();
            player.stop_for_exit()
        });
        let immediate = result.recv_timeout(Duration::from_secs(1));
        let _ = release.send(());
        let _ = cleanup.send(());
        completed.recv_timeout(Duration::from_secs(2)).unwrap();
        polling.join().unwrap().unwrap();
        let (active, owns_child, owns_reader) =
            immediate.expect("Playback status poll waited for the diagnostic reader");
        assert!(active.unwrap() && owns_child && owns_reader);
    }

    #[cfg(windows)]
    #[test]
    fn gui_playback_retirement_requests_its_owned_child_termination() {
        use std::{
            ffi::c_void,
            os::windows::io::{AsHandle, AsRawHandle, OwnedHandle},
        };
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn TerminateProcess(process: *mut c_void, exit_code: u32) -> i32;
            fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        }
        struct FixtureCleanup(OwnedHandle);
        impl Drop for FixtureCleanup {
            fn drop(&mut self) {
                // This duplicate is the exact fixture child, never a PID lookup.
                // Reap outside the nonblocking application method even on panic.
                unsafe {
                    TerminateProcess(self.0.as_raw_handle().cast(), 1);
                    WaitForSingleObject(self.0.as_raw_handle().cast(), 5000);
                }
            }
        }
        for exiting in [false, true] {
            let child = sleeping_playback_child();
            let cleanup = FixtureCleanup(child.as_handle().try_clone_to_owned().unwrap());
            let player = AudioPlayback {
                child: Some(child),
                errors: None,
            };
            if exiting {
                player.stop_for_exit().unwrap();
            } else {
                player.stop_in_background().unwrap();
            }
            assert_eq!(
                unsafe { WaitForSingleObject(cleanup.0.as_raw_handle().cast(), 5000) },
                0
            );
        }
    }

    #[test]
    fn playback_selects_first_audio_stream_and_maps_its_time_origin() {
        let executable = std::env::current_exe().unwrap();
        let source = source_stamp(&executable).unwrap();
        let request = PlaybackRequest {
            input: executable.clone(),
            ffplay: executable.clone(),
            ffprobe: executable,
            channel: 1,
            start_s: 2.,
            end_s: 4.,
            expected_source: None,
        };
        let info = MediaDescription {
            duration_s: 10.,
            format_start_s: 5.,
            audio_start_s: 6.5,
            channels: Some(2),
            sample_rate_hz: Some(48000),
            has_video: true,
        };
        let command = playback_command(&request, &source, &info).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.windows(2).any(|pair| pair == ["-ast", "a:0"]));
        assert!(args.windows(2).any(|pair| pair == ["-ss", "3.500000000"]));
        assert!(args.windows(2).any(|pair| pair == ["-t", "2.000000000"]));
        assert!(args.iter().any(|arg| arg == "pan=mono|c0=c1"));
    }
}
