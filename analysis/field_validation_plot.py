"""Render real measured spectra and native proposals without opening windows."""
import argparse
import json
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np
from scipy import signal
from scipy.io import wavfile


def plot(waveform, discovery_path, output):
    out = Path(output)
    if out.exists():
        raise ValueError("Use a new plot path")
    rate, samples = wavfile.read(waveform)
    if samples.ndim != 1 or rate % 4000:
        raise ValueError("Expected mono source with integer 4000Hz downsample ratio")
    samples = signal.resample_poly(samples.astype(np.float32) / 32768, 1, rate // 4000)
    frequency, times, power = signal.spectrogram(samples, fs=4000, window="hann", nperseg=8192,
                                               noverlap=6192, detrend="constant", mode="psd")
    db = 10 * np.log10(np.maximum(power, 1e-16))
    discovery = json.loads(Path(discovery_path).read_text(encoding="utf-8"))
    mask = (frequency >= 40) & (frequency <= 1600)
    limit = np.percentile(db[mask], 99)
    figure, axes = plt.subplots(2, 1, figsize=(14, 8), constrained_layout=True)
    for ax in axes:
        ax.pcolormesh(times, frequency[mask], db[mask], vmin=limit-50, vmax=limit, cmap="magma", shading="auto", rasterized=True)
        for event in discovery["events"]:
            ax.plot([p["time_s"] for p in event["ridge"]], [p["frequency_hz"] for p in event["ridge"]],
                    color="#62e5f7", linewidth=1.3, label=event["event_id"])
        ax.set_xlabel("Seconds from original sample zero")
        ax.set_ylabel("Frequency (Hz)")
    axes[0].set_title("Actual field audio: full 600 seconds; cyan = unreviewed native proposals")
    axes[0].set_xlim(0, len(samples)/4000)
    if discovery["events"]:
        first = discovery["events"][0]
        band = [point["frequency_hz"] for point in first["ridge"]]
        axes[1].set_xlim(max(0, first["start_s"]-30), first["end_s"]+30)
        axes[1].set_ylim(max(40, min(band)-100), min(1600, max(band)+100))
        axes[1].set_title("First automatic proposal with surrounding sound; no source identity or location supplied")
    else:
        axes[1].set_ylim(40, 500)
        axes[1].set_title("No proposals: low-frequency context, same unchanged detector settings")
    figure.savefig(out, dpi=150)
    plt.close(figure)
    print(out.resolve())


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audio", required=True)
    parser.add_argument("--discovery", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    plot(args.audio, args.discovery, args.output)
