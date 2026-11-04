"""Render audio-only review pages without detections, tracks or location labels."""
import argparse
import json
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np
from scipy import signal
from scipy.io import wavfile

from field_validation_native import digest, write


def build(folder):
    folder = Path(folder).resolve()
    receipt = json.loads((folder / "audio-only.json").read_text())
    waveform = folder / receipt["audio_path"]
    if digest(waveform) != receipt["audio_sha256"]:
        raise ValueError("Original review waveform changed")
    out = folder / "spectrograms"
    out.mkdir(exist_ok=False)
    rate, pcm = wavfile.read(waveform)
    if pcm.ndim != 1 or rate % 4000:
        raise ValueError("Expected original mono waveform with integer4000Hz downsampling")
    samples = signal.resample_poly(pcm.astype(np.float32) / 32768.0, 1, rate // 4000)
    frequencies, times, power = signal.spectrogram(samples, fs=4000, window="hann", nperseg=8192,
                                                 noverlap=7192, detrend="constant", mode="psd")
    db = 10 * np.log10(np.maximum(power, 1e-16))
    contrast = db - np.median(db, axis=1, keepdims=True)
    bands = [(40, 1600), (40, 300), (300, 800), (800, 1600)]
    assets = []
    def render(name, start, end, full=False):
        figure, axes = plt.subplots(4, 1, figsize=(16, 12), constrained_layout=True)
        keep_time = (times >= start) & (times <= end)
        for index, (axis, (low, high)) in enumerate(zip(axes, bands)):
            keep_frequency = (frequencies >= low) & (frequencies <= high)
            data = db if index == 0 else contrast
            view = data[keep_frequency][:, keep_time]
            if index == 0:
                vmax = np.percentile(db[keep_frequency], 99)
                vmin, label = vmax-50, "raw power spectral density (dB)"
            else:
                vmin, vmax, label = -6, 18, "contrast to each frequency's whole-clip median (dB)"
            mesh = axis.pcolormesh(times[keep_time], frequencies[keep_frequency], view,
                                   shading="auto", cmap="magma", vmin=vmin, vmax=vmax, rasterized=True)
            axis.set_xlim(start, end)
            axis.set_ylim(low, high)
            axis.set_ylabel("Frequency (Hz)")
            axis.set_xlabel("Seconds from original sample zero")
            axis.set_title(label, fontsize=10)
            figure.colorbar(mesh, ax=axis, label="dB", pad=0.01)
        figure.suptitle(f"{receipt['case_id']} — audio only — {start:g}–{end:g}s\nNo detections, aircraft identities, fitted curves or exact location supplied", fontsize=14)
        path = out / name
        figure.savefig(path, dpi=140)
        plt.close(figure)
        assets.append({"path": "spectrograms/"+name, "sha256": digest(path), "start_s": start, "end_s": end})
    duration = receipt["duration_s"]
    render("overview.png", 0, duration, True)
    for start in range(0, int(np.ceil(duration)), 120):
        render(f"page-{start//120:03}.png", start, min(duration, start+120))
    manifest = {"schema_version": 1, "audio_sha256": receipt["audio_sha256"],
                "sample_zero_utc_claim": receipt["sample_zero_utc_claim"], "duration_s": duration,
                "spectral_display": {"analysis_rate_hz": 4000, "resampling": "scipy.signal.resample_poly, display only",
                                     "window": "Hann8192samples", "window_s": 2.048, "hop_s": 0.25,
                                     "detrend": "per-frame constant", "power_floor": 1e-16,
                                     "contrast": "Subtract frequency-wise whole-clip median; raw panel retained above"},
                "assets": assets, "aircraft_or_location_fit_used": False,
                "human_review_required": True,
                "review_guidance": "Freeze accepted/rejected intervals and approximate ridge anchors from these pages/audio before inspecting candidate tracks or truth. Do not assume every descending band is an aircraft. Correlated harmonics and clipped events need explicit notes; do not invent CPA timing from the loudest moment. No usable event is a valid outcome."}
    write(folder / "review-assets.json", manifest)
    print(json.dumps({"review_folder": str(folder), "duration_s": duration, "pages": len(assets), "aircraft_or_location_fit_used": False}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--review", required=True)
    args = parser.parse_args()
    build(args.review)
