"""Headless, audio-only review figures and measurements along supplied anchors.

This research helper never reads flight tracks or receiver coordinates. Anchors
are an explicit reviewer choice, not a detected or identified aircraft. The
result is a pending annotation; accepting it for an experiment is a separate act.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np
import scipy
from scipy import signal
from scipy.io import wavfile


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def spectrum(path):
    if Path(path).stat().st_size > 256 * 1024 * 1024:
        raise ValueError("Audio-only review is limited to 0.250 GiB of source PCM per run")
    before = digest(path)
    rate, samples = wavfile.read(path)
    if samples.ndim != 1 or samples.dtype not in (np.dtype("int16"), np.dtype("int32")) or rate < 4000 or rate % 4000:
        raise ValueError("This review helper requires mono 16-bit or 32-bit PCM at a multiple of 4000 Hz")
    if not 2.048 <= len(samples) / rate <= 3600:
        raise ValueError("Review 2.048 seconds to one hour at a time")
    # Preserve the existing PCM16 calculation exactly. PCM32 uses float64 so
    # quiet original samples are not quantized through a PCM16 intermediate.
    normalized = (samples.astype(np.float32) / 32768 if samples.dtype == np.int16
                  else samples.astype(np.float64) / 2147483648)
    audio = signal.resample_poly(normalized, 1, rate // 4000)
    frequency, times, power = signal.spectrogram(
        audio, fs=4000, window="hann", nperseg=8192, noverlap=6192,
        detrend="constant", scaling="density", mode="psd")
    if digest(path) != before:
        raise ValueError("Waveform changed during measurement")
    return frequency, times, 10 * np.log10(np.maximum(power, 1e-20)), {
        "path": str(Path(path).resolve()), "sha256": before,
        "sample_rate_hz": rate, "duration_s": len(samples) / rate,
        "pcm_container_bits": samples.dtype.itemsize * 8,
        "channel": 0, "window_s": 2.048, "hop_s": 0.5,
        "frequency_bin_hz": 4000 / 8192, "time_reference": "original PCM sample zero",
        "method": "polyphase resample to 4000 Hz; periodic Hann; per-frame mean removal; power spectral density",
    }


def measure(frequency, times, db, config):
    anchors = np.asarray(config["anchors"], dtype=float)
    width = float(config.get("corridor_half_width_hz", 5))
    step = float(config.get("measurement_step_s", 2))
    threshold = float(config.get("minimum_local_contrast_db", 3))
    if (anchors.ndim != 2 or anchors.shape[1] != 2 or not 2 <= len(anchors) <= 1000
            or not np.all(np.isfinite(anchors)) or np.any(np.diff(anchors[:, 0]) <= 0)
            or anchors[0, 0] < times[0] or anchors[-1, 0] > times[-1]
            or np.any(anchors[:, 1] < 40) or np.any(anchors[:, 1] > 1900)
            or not math.isfinite(width) or not 1 <= width <= 20
            or not math.isfinite(step) or not 0.5 <= step <= 30
            or not math.isfinite(threshold) or not 0 <= threshold <= 30):
        raise ValueError("Invalid ordered audio-only anchors or measurement controls")
    requested = np.arange(anchors[0, 0], anchors[-1, 0] + 1e-8, step)
    if len(requested) > 3000:
        raise ValueError("Too many trace measurements")
    frames = sorted(set(int(np.argmin(abs(times - value))) for value in requested))
    accepted, rejected = [], []
    for frame in frames:
        center = float(np.interp(times[frame], anchors[:, 0], anchors[:, 1]))
        bins = np.flatnonzero(abs(frequency - center) <= width)
        peak = int(bins[np.argmax(db[bins, frame])])
        # Only measured bins select the peak; the sub-bin correction uses its
        # local parabola and never follows a flight/model prediction.
        a, b, c = (float(db[peak + shift, frame]) for shift in (-1, 0, 1))
        denominator = a - 2 * b + c
        correction = 0.0 if denominator >= 0 else float(np.clip(0.5 * (a - c) / denominator, -0.5, 0.5))
        value = float(frequency[peak] + correction * (frequency[1] - frequency[0]))
        noise_bins = (abs(frequency - value) >= width + 2) & (abs(frequency - value) <= width + 15)
        contrast = float(b - np.median(db[noise_bins, frame]))
        edge = peak in (bins[0], bins[-1])
        row = {"time_s": float(times[frame]), "frequency_hz": value,
               "local_contrast_db": contrast, "corridor_edge": bool(edge)}
        if edge or contrast < threshold:
            row["reason"] = "corridor_edge" if edge else "weak_local_contrast"
            rejected.append(row)
        else:
            accepted.append(row)
    return {"points": accepted, "rejected": rejected, "settings": config,
            "aircraft_identity_confirmed": False, "human_review_completed": False,
            "review": "pending", "selected_for_scoring": False,
            "scope": "Measurements along explicit audio-only reviewer anchors; no source matching or location input",
            "frequency_sigma_hz": max(1.0, 2 * float(frequency[1] - frequency[0])),
            "uncertainty_s": 1.024,
            "uncertainty_note": "Numerical review scales, not calibrated measurement or physical-model confidence"}


def run(args):
    config_bytes = Path(args.config).read_bytes()
    config = json.loads(config_bytes)
    out = Path(args.output).resolve()
    out.mkdir(parents=True, exist_ok=False)
    (out / "review-config.json").write_bytes(config_bytes)
    frequency, times, db, source = spectrum(args.audio)
    interval = config.get("time_bounds_s", [float(times[0]), float(times[-1])])
    band = config.get("frequency_bounds_hz", [40, 1600])
    if not (len(interval) == len(band) == 2 and all(math.isfinite(x) for x in interval + band)
            and 0 <= interval[0] < interval[1] <= source["duration_s"]
            and 0 <= band[0] < band[1] <= 2000):
        raise ValueError("Invalid display bounds")
    selected_t = (times >= interval[0]) & (times <= interval[1])
    selected_f = (frequency >= band[0]) & (frequency <= band[1])
    shown = db[np.ix_(selected_f, selected_t)]
    if min(shown.shape) < 2:
        raise ValueError("Display contains too few measured bins/frames")
    measurement = measure(frequency, times, db, config) if "anchors" in config else None
    fig, ax = plt.subplots(figsize=(16, 6), constrained_layout=True)
    top = float(np.percentile(shown, 99.5))
    ax.pcolormesh(times[selected_t], frequency[selected_f], shown, shading="auto",
                  cmap="magma", vmin=top - 40, vmax=top, rasterized=True)
    if measurement:
        anchors = np.asarray(config["anchors"])
        ax.plot(anchors[:, 0], anchors[:, 1], color="#ffffff", linewidth=0.7, alpha=0.6, label="Reviewer anchors")
        points = measurement["points"]
        if points:
            ax.scatter([p["time_s"] for p in points], [p["frequency_hz"] for p in points],
                       s=8, color="#62e5f7", label="Measured spectral peaks")
        ax.legend()
    ax.set(xlabel="Seconds from original sample zero", ylabel="Frequency (Hz)",
           title="Audio-only spectral review — no aircraft identity or receiver coordinates supplied",
           xlim=interval, ylim=band)
    fig.savefig(out / "spectrogram.png", dpi=150)
    plt.close(fig)
    result = {"schema_version": 1, "source": source, "configuration": config,
              "configuration_sha256": hashlib.sha256(config_bytes).hexdigest(),
              "measurement": measurement, "location_verified": False,
              "generated_artifacts": {"spectrogram.png": digest(out / "spectrogram.png")}}
    script_bytes = Path(__file__).read_bytes()
    (out / "measurement-source.py").write_bytes(script_bytes)
    result["software"] = {
        "measurement_source_sha256": hashlib.sha256(script_bytes).hexdigest(),
        "numpy": np.__version__, "scipy": scipy.__version__, "matplotlib": matplotlib.__version__,
    }
    if digest(args.audio) != source["sha256"] or Path(args.config).read_bytes() != config_bytes:
        raise ValueError("Review inputs changed during processing")
    (out / "review.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"output": str(out), "points": len(measurement["points"]) if measurement else 0}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audio", required=True)
    parser.add_argument("--config", required=True)
    parser.add_argument("--output", required=True)
    run(parser.parse_args())
