//! Streaming, channel-preserving audio inspection for the verification workflow.
//!
//! FFmpeg performs decoding and anti-aliased resampling; the STFT is implemented
//! here. GSPEC001 stores a fixed grid of power spectral density values in dB.
//! This analysis does not identify flights, infer UTC time, or infer location.

use serde_json::{Value, json};
use std::f64::consts::PI;
use std::fs::{self, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

const SAMPLE_RATE: usize = 4_000;
const FFT_SIZE: usize = 4_096;
const HOP_SIZE: usize = 1_000;
const MAX_HZ: f64 = 800.0;
const PREVIEW_COLUMNS: usize = 1_800;
const MAGIC: &[u8; 8] = b"GSPEC001";

/// Resolve a media helper through the shared, metadata-only discovery policy.
/// Returns an actual absolute file path, never an unchecked command name.
pub fn resolve_media_tool(name: &str) -> Result<PathBuf, String> {
    crate::media_tools::resolve(
        crate::media_tools::Tool::from_name(name)?,
        None,
        Path::new(""),
    )
}

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f64,
    im: f64,
}

/// In-place radix-2 Cooley-Tukey transform with an unnormalized forward sign.
fn fft(values: &mut [Complex]) {
    let n = values.len();
    debug_assert!(n.is_power_of_two());
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            values.swap(i, j);
        }
    }
    let mut length = 2;
    while length <= n {
        let angle = -2.0 * PI / length as f64;
        let (wi, wr) = angle.sin_cos();
        for start in (0..n).step_by(length) {
            let (mut current_re, mut current_im) = (1.0, 0.0);
            for k in 0..length / 2 {
                let a = values[start + k];
                let b = values[start + k + length / 2];
                let v = Complex {
                    re: b.re * current_re - b.im * current_im,
                    im: b.re * current_im + b.im * current_re,
                };
                values[start + k] = Complex {
                    re: a.re + v.re,
                    im: a.im + v.im,
                };
                values[start + k + length / 2] = Complex {
                    re: a.re - v.re,
                    im: a.im - v.im,
                };
                (current_re, current_im) = (
                    current_re * wr - current_im * wi,
                    current_re * wi + current_im * wr,
                );
            }
        }
        length *= 2;
    }
}

pub(crate) struct SpectralEngine {
    window: Vec<f64>,
    window_energy: f64,
    work: Vec<Complex>,
    bins: usize,
}

impl SpectralEngine {
    fn new() -> Self {
        Self::configured(FFT_SIZE, MAX_HZ)
    }

    pub(crate) fn configured(fft_size: usize, max_hz: f64) -> Self {
        assert!(fft_size.is_power_of_two());
        // Periodic Hann, matching SciPy's signal.spectrogram(window="hann").
        let window: Vec<_> = (0..fft_size)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / fft_size as f64).cos())
            .collect();
        let window_energy = window.iter().map(|x| x * x).sum();
        Self {
            window,
            window_energy,
            work: vec![Complex::default(); fft_size],
            bins: (max_hz * fft_size as f64 / SAMPLE_RATE as f64).floor() as usize + 1,
        }
    }

    pub(crate) fn power_db(&mut self, samples: &[f32]) -> Vec<f32> {
        debug_assert_eq!(samples.len(), self.window.len());
        for (i, &sample) in samples.iter().enumerate() {
            self.work[i] = Complex {
                re: sample as f64 * self.window[i],
                im: 0.0,
            };
        }
        fft(&mut self.work);
        self.work[..self.bins]
            .iter()
            .enumerate()
            .map(|(i, z)| {
                let multiplier = if i == 0 || i == self.window.len() / 2 {
                    1.0
                } else {
                    2.0
                };
                let density = multiplier * (z.re * z.re + z.im * z.im)
                    / (SAMPLE_RATE as f64 * self.window_energy);
                (10.0 * density.max(1e-20).log10()) as f32
            })
            .collect()
    }
}

fn frame_center_seconds(index: usize) -> f64 {
    (index * HOP_SIZE + FFT_SIZE / 2) as f64 / SAMPLE_RATE as f64
}

/// Reads complete little-endian f32 samples, permitting EOF only on a sample
/// boundary. A partial final STFT window is omitted rather than zero padded.
pub(crate) fn read_samples(
    reader: &mut impl Read,
    destination: &mut [f32],
) -> Result<usize, String> {
    let mut bytes = vec![0_u8; destination.len() * 4];
    let mut total = 0;
    while total < bytes.len() {
        match reader.read(&mut bytes[total..]) {
            Ok(0) => break,
            Ok(count) => total += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("Could not read decoded audio: {error}")),
        }
    }
    if total % 4 != 0 {
        return Err("Decoded audio ended inside a floating-point sample".into());
    }
    for (target, bytes) in destination.iter_mut().zip(bytes[..total].chunks_exact(4)) {
        *target = f32::from_le_bytes(bytes.try_into().unwrap());
        if !target.is_finite() {
            return Err("Decoded audio contains non-finite samples".into());
        }
    }
    Ok(total / 4)
}

/// Max pooling is for the preview only. GSPEC retains every STFT frame.
struct Preview {
    columns: Vec<Vec<f32>>,
    pending: Vec<f32>,
    pending_frames: usize,
    frames_per_column: usize,
    limit: usize,
}

impl Preview {
    fn new(bins: usize, limit: usize) -> Self {
        Self {
            columns: Vec::new(),
            pending: vec![f32::NEG_INFINITY; bins],
            pending_frames: 0,
            frames_per_column: 1,
            limit,
        }
    }

    fn add(&mut self, values: &[f32]) {
        for (target, &source) in self.pending.iter_mut().zip(values) {
            *target = target.max(source);
        }
        self.pending_frames += 1;
        if self.pending_frames == self.frames_per_column {
            self.columns.push(self.pending.clone());
            self.pending.fill(f32::NEG_INFINITY);
            self.pending_frames = 0;
            if self.columns.len() == 2 * self.limit {
                let reduced = self
                    .columns
                    .chunks_exact(2)
                    .map(|pair| {
                        pair[0]
                            .iter()
                            .zip(&pair[1])
                            .map(|(&a, &b)| a.max(b))
                            .collect()
                    })
                    .collect();
                self.columns = reduced;
                self.frames_per_column *= 2;
            }
        }
    }

    fn finish(&mut self) {
        if self.pending_frames > 0 {
            self.columns.push(self.pending.clone());
            self.pending_frames = 0;
        }
        // The streaming reduction permits up to twice the desired width; do a
        // final reduction so exported images stay bounded by the requested cap.
        if self.columns.len() > self.limit {
            self.columns = self
                .columns
                .chunks(2)
                .map(|pair| {
                    if pair.len() == 1 {
                        pair[0].clone()
                    } else {
                        pair[0]
                            .iter()
                            .zip(&pair[1])
                            .map(|(&a, &b)| a.max(b))
                            .collect()
                    }
                })
                .collect();
            self.frames_per_column *= 2;
        }
    }
}

pub(crate) fn channel_count(input: &Path) -> Result<usize, String> {
    let result = crate::media::quiet_command(resolve_media_tool("ffprobe")?)
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=channels", "-of", "json"])
        .arg(input)
        .output()
        .map_err(|error| format!("Could not run FFprobe: {error}. Place ffprobe beside the executable or in tools, or install it on PATH."))?;
    if !result.status.success() {
        return Err(format!(
            "FFprobe could not inspect audio: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    let report: Value = serde_json::from_slice(&result.stdout)
        .map_err(|error| format!("Invalid FFprobe response: {error}"))?;
    report["streams"][0]["channels"]
        .as_u64()
        .filter(|&count| count > 0)
        .map(|count| count as usize)
        .ok_or_else(|| "The input has no usable first audio stream".into())
}

/// Analyze the complete first audio stream without averaging its channels.
/// Channel indexes are zero based. Existing generated output files are refused.
/// On failure no header is written, so an incomplete binary cannot be mistaken
/// for a successfully completed analysis.
pub fn analyze(input: &Path, output_dir: &Path, channel: usize) -> Result<Value, String> {
    if !input.is_file() {
        return Err(format!(
            "Audio/video input does not exist: {}",
            input.display()
        ));
    }
    let available_channels = channel_count(input)?;
    if channel >= available_channels {
        return Err(format!(
            "Requested channel {channel}, but this audio has {available_channels} channel(s); indexes start at zero"
        ));
    }
    let ffmpeg = resolve_media_tool("ffmpeg")?;
    fs::create_dir_all(output_dir)
        .map_err(|error| format!("Cannot create analysis folder: {error}"))?;
    let spec_path = output_dir.join("spectrogram.gspec");
    let header_path = output_dir.join("spectrogram.json");
    let preview_path = output_dir.join("spectrogram.pgm");
    for path in [&spec_path, &header_path, &preview_path] {
        if path.exists() {
            return Err(format!(
                "Refusing to overwrite {}; choose a new output folder",
                path.display()
            ));
        }
    }
    let spec_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&spec_path)
        .map_err(|error| format!("Cannot create spectrogram: {error}"))?;
    let mut writer = BufWriter::new(spec_file);
    writer.write_all(MAGIC).map_err(|error| error.to_string())?;

    let mut child = crate::media::quiet_command(&ffmpeg)
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(input)
        .args(["-map", "0:a:0", "-vn", "-af"])
        .arg(format!("pan=mono|c0=c{channel}"))
        .args(["-ar", "4000", "-c:a", "pcm_f32le", "-f", "f32le", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not run FFmpeg: {error}. Place ffmpeg beside the executable or in tools, or install it on PATH."))?;
    let stderr = child.stderr.take().unwrap();
    let error_reader = std::thread::spawn(move || {
        let mut source = BufReader::new(stderr);
        let mut kept = Vec::new();
        let mut buffer = [0_u8; 4096];
        // Drain the entire pipe to prevent a blocked child; retain only 64 KiB.
        while let Ok(count) = source.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let keep = count.min(65_536_usize.saturating_sub(kept.len()));
            kept.extend_from_slice(&buffer[..keep]);
        }
        String::from_utf8_lossy(&kept).into_owned()
    });
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut engine = SpectralEngine::new();
    let mut preview = Preview::new(engine.bins, PREVIEW_COLUMNS);
    let mut frames = 0_usize;
    let mut sample_count = 0_usize;
    let calculation = (|| -> Result<(), String> {
        let mut window = vec![0_f32; FFT_SIZE];
        let read = read_samples(&mut reader, &mut window)?;
        sample_count += read;
        if read != FFT_SIZE {
            return Err("Audio is shorter than one complete 1.024-second analysis window".into());
        }
        loop {
            let values = engine.power_db(&window);
            writer
                .write_all(&frame_center_seconds(frames).to_le_bytes())
                .map_err(|e| e.to_string())?;
            for &value in &values {
                writer
                    .write_all(&value.to_le_bytes())
                    .map_err(|e| e.to_string())?;
            }
            preview.add(&values);
            frames += 1;
            window.copy_within(HOP_SIZE.., 0);
            let read = read_samples(&mut reader, &mut window[FFT_SIZE - HOP_SIZE..])?;
            sample_count += read;
            if read != HOP_SIZE {
                break;
            }
        }
        writer.flush().map_err(|error| error.to_string())?;
        Ok(())
    })();
    drop(reader);
    if calculation.is_err() {
        let _ = child.kill();
    }
    let status = child
        .wait()
        .map_err(|error| format!("Could not wait for FFmpeg: {error}"))?;
    let error_text = error_reader
        .join()
        .unwrap_or_else(|_| "Could not collect FFmpeg error output".into());
    if !status.success() {
        return Err(format!(
            "FFmpeg did not complete successfully: {error_text}"
        ));
    }
    calculation?;
    preview.finish();

    // A fixed dB display range makes previews comparable. Frequency increases
    // upward; numeric data is neither pooled nor clipped in the GSPEC file.
    let (display_min, display_max) = (-110.0_f32, -35.0_f32);
    let mut pgm = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&preview_path)
            .map_err(|error| format!("Cannot create preview: {error}"))?,
    );
    writeln!(
        pgm,
        "P5\n# time increases right; frequency increases up\n{} {}\n255",
        preview.columns.len(),
        engine.bins
    )
    .map_err(|error| error.to_string())?;
    for bin in (0..engine.bins).rev() {
        for column in &preview.columns {
            let intensity = ((column[bin] - display_min) / (display_max - display_min) * 255.0)
                .clamp(0.0, 255.0)
                .round() as u8;
            pgm.write_all(&[intensity])
                .map_err(|error| error.to_string())?;
        }
    }
    pgm.flush().map_err(|error| error.to_string())?;
    let version = crate::media::quiet_command(&ffmpeg)
        .arg("-version")
        .output()
        .ok()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .to_owned()
        });
    let header = json!({
        "status": "audio_analyzed_location_not_inferred",
        "source": input.canonicalize().unwrap_or_else(|_| input.to_owned()).display().to_string(),
        "selected_audio_stream": "0:a:0 (first audio stream)",
        "selected_channel_index": channel,
        "available_channels": available_channels,
        "channels_averaged": false,
        "ffmpeg_version": version,
        "sample_rate_hz": SAMPLE_RATE,
        "decoded_samples": sample_count,
        "decoded_duration_s": sample_count as f64 / SAMPLE_RATE as f64,
        "time_reference": "seconds from beginning of downloaded input; no crop or UTC offset applied",
        "fft_size": FFT_SIZE,
        "hop_samples": HOP_SIZE,
        "window": "periodic Hann",
        "detrending": false,
        "padding": false,
        "frequency_bins": engine.bins,
        "frequency_first_hz": 0.0,
        "frequency_step_hz": SAMPLE_RATE as f64 / FFT_SIZE as f64,
        "frequency_last_hz": (engine.bins - 1) as f64 * SAMPLE_RATE as f64 / FFT_SIZE as f64,
        "frame_count": frames,
        "first_frame_center_s": frame_center_seconds(0),
        "last_frame_center_s": frame_center_seconds(frames - 1),
        "frame_step_s": HOP_SIZE as f64 / SAMPLE_RATE as f64,
        "value_units": "10*log10(one-sided power spectral density in full-scale-amplitude squared per Hz), floor -200 dB",
        "binary": {
            "filename": "spectrogram.gspec", "magic_ascii": "GSPEC001", "header_bytes": 8,
            "byte_order": "little-endian", "layout": "time-major records; each record is f64 recording center time, then frequency_bins f32 PSD dB values in ascending frequency",
            "record_bytes": 8 + engine.bins * 4,
            "expected_file_bytes": 8 + frames * (8 + engine.bins * 4)
        },
        "preview": {
            "filename": "spectrogram.pgm", "format": "binary PGM P5 grayscale",
            "width": preview.columns.len(), "height": engine.bins,
            "frames_per_column": preview.frames_per_column,
            "seconds_per_column": preview.frames_per_column as f64 * HOP_SIZE as f64 / SAMPLE_RATE as f64,
            "pooling": "maximum power over time bucket, final bucket may be partial; highest frequency is top row",
            "display_min_db": display_min, "display_max_db": display_max
        }
    });
    let header_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&header_path)
        .map_err(|error| format!("Cannot create spectrogram header: {error}"))?;
    let mut header_writer = BufWriter::new(header_file);
    serde_json::to_writer_pretty(&mut header_writer, &header).map_err(|error| error.to_string())?;
    header_writer.flush().map_err(|error| error.to_string())?;
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn hann_tone_has_expected_peak_energy_and_frequency() {
        let mut engine = SpectralEngine::new();
        let samples: Vec<f32> = (0..FFT_SIZE)
            .map(|i| (2.0 * PI * 128.0 * i as f64 / FFT_SIZE as f64).cos() as f32)
            .collect();
        let db = engine.power_db(&samples);
        let peak = db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(peak, 128);
        assert_eq!(db.len(), 820);
        assert!((peak as f64 * SAMPLE_RATE as f64 / FFT_SIZE as f64 - 125.0).abs() < 1e-10);
        let expected_peak_db = 10.0 * (FFT_SIZE as f64 / (3.0 * SAMPLE_RATE as f64)).log10();
        assert!((db[peak] as f64 - expected_peak_db).abs() < 1e-5);
        let integrated_power: f64 = db
            .iter()
            .map(|&d| 10_f64.powf(d as f64 / 10.0))
            .sum::<f64>()
            * SAMPLE_RATE as f64
            / FFT_SIZE as f64;
        assert!((integrated_power - 0.5).abs() < 1e-5);
    }

    #[test]
    fn frames_use_window_centers_without_a_crop_offset() {
        assert_eq!(frame_center_seconds(0), 0.512);
        assert_eq!(frame_center_seconds(1), 0.762);
        assert_eq!(frame_center_seconds(4), 1.512);
    }

    #[test]
    fn sample_decoder_handles_short_final_blocks_and_rejects_corruption() {
        let bytes: Vec<u8> = [1.0_f32, -2.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let mut destination = [0.0_f32; 4];
        assert_eq!(
            read_samples(&mut Cursor::new(bytes), &mut destination).unwrap(),
            2
        );
        assert_eq!(destination[..2], [1.0, -2.0]);
        assert!(read_samples(&mut Cursor::new(vec![0, 1, 2]), &mut destination).is_err());
        assert!(read_samples(&mut Cursor::new(f32::NAN.to_le_bytes()), &mut destination).is_err());
    }

    #[test]
    fn preview_reduction_preserves_peak_and_respects_size_cap() {
        let mut preview = Preview::new(2, 3);
        for i in 0..37 {
            preview.add(&[i as f32, -(i as f32)]);
        }
        preview.finish();
        assert!(preview.columns.len() <= 3);
        assert_eq!(
            preview
                .columns
                .iter()
                .map(|c| c[0])
                .fold(f32::NEG_INFINITY, f32::max),
            36.0
        );
        assert_eq!(
            preview
                .columns
                .iter()
                .map(|c| c[1])
                .fold(f32::NEG_INFINITY, f32::max),
            0.0
        );
        assert!(
            preview
                .columns
                .iter()
                .all(|c| c.iter().all(|value| value.is_finite()))
        );
    }
}
