"""Audio ingestion, narrow-band spectrograms and reproducible Doppler measurements."""
from dataclasses import dataclass
import json
import subprocess

import numpy as np
from scipy import ndimage, optimize, signal

from .geometry import straight_doppler


def decode(path, start=0.0, duration=None, sample_rate=4000):
    command = ["ffmpeg", "-v", "error", "-ss", str(start), "-i", str(path)]
    if duration is not None:
        command += ["-t", str(duration)]
    command += ["-vn", "-ac", "1", "-ar", str(sample_rate), "-f", "f32le", "pipe:1"]
    result = subprocess.run(command, capture_output=True, check=True)
    data = np.frombuffer(result.stdout, dtype="<f4").astype(float)
    if not len(data) or not np.all(np.isfinite(data)):
        raise ValueError(f"Invalid/empty decoded audio: {path}")
    return data, sample_rate


@dataclass
class Spectrum:
    time: np.ndarray
    frequency: np.ndarray
    db: np.ndarray
    contrast: np.ndarray

    def sample(self, t, f):
        """Bilinear contrast along a template; unsupported samples stay NaN."""
        ti = (np.asarray(t) - self.time[0]) / (self.time[1] - self.time[0])
        fi = (np.asarray(f) - self.frequency[0]) / (self.frequency[1] - self.frequency[0])
        ti, fi = np.broadcast_arrays(ti, fi)
        values = ndimage.map_coordinates(self.contrast, [fi.ravel(), ti.ravel()],
                                         order=1, mode="constant", cval=np.nan)
        return values.reshape(ti.shape)


def spectrogram(data, sample_rate, band, start=0.0, window_s=2.0, hop_s=0.25):
    n = int(sample_rate * window_s)
    hop = int(sample_rate * hop_s)
    if len(data) < n:
        raise ValueError("Event is shorter than the spectral analysis window")
    f, t, power = signal.spectrogram(data, fs=sample_rate, window="hann", nperseg=n,
                                     noverlap=n - hop, nfft=4 * n, scaling="density")
    keep = (f >= band[0]) & (f <= band[1])
    f, power = f[keep], power[keep]
    db = 10 * np.log10(power + np.finfo(float).tiny)
    # Suppress stationary tones, then normalize each time column by its robust
    # noise floor. This is a numeric analogue of increasing spectrogram contrast.
    whitened = db - np.median(db, axis=1, keepdims=True)
    center = np.median(whitened, axis=0, keepdims=True)
    scale = 1.4826 * np.median(np.abs(whitened - center), axis=0, keepdims=True)
    contrast = np.clip((whitened - center) / np.maximum(scale, 1.0), -4, 12)
    return Spectrum(t + start, f, db, contrast)


def trace_ridge(spectrum, max_slope=12.0, smoothness=0.12, max_up_slope=1.0):
    """Viterbi ridge with a bounded frequency change and quadratic transition cost."""
    strength = np.clip(spectrum.contrast, -2, 7)
    df = spectrum.frequency[1] - spectrum.frequency[0]
    dt = spectrum.time[1] - spectrum.time[0]
    width = max(1, int(np.ceil(max_slope * dt / df)))
    nf, nt = strength.shape
    history = np.zeros((nt, nf), dtype=np.int16)
    state = strength[:, 0].copy()
    frequency_index = np.arange(nf)
    for j in range(1, nt):
        best = np.full(nf, -np.inf)
        parent = np.zeros(nf, dtype=np.int16)
        up_width = max(0, int(np.floor(max_up_slope * dt / df)))
        for shift in range(-up_width, width + 1):
            previous = frequency_index + shift
            valid = (previous >= 0) & (previous < nf)
            score = np.full(nf, -np.inf)
            score[valid] = state[previous[valid]] - smoothness * (shift * df / dt) ** 2 * dt
            better = score > best
            best[better] = score[better]
            parent[better] = previous[better]
        state = best + strength[:, j]
        history[j] = parent
    path = np.empty(nt, dtype=int)
    path[-1] = np.argmax(state)
    for j in range(nt - 1, 0, -1):
        path[j - 1] = history[j, path[j]]
    # Sub-bin parabolic interpolation improves ridge precision without claiming
    # that zero padding itself increased the physical spectral resolution.
    interior = np.clip(path, 1, nf - 2)
    cols = np.arange(nt)
    a, b, c = (spectrum.db[interior + offset, cols] for offset in (-1, 0, 1))
    shift = np.clip(0.5 * (a - c) / np.where(np.abs(a - 2*b + c) > 1e-9, a - 2*b + c, -1), -0.5, 0.5)
    frequency = spectrum.frequency[interior] + shift * df
    weight = np.clip(spectrum.contrast[path, cols], 0, 8)
    return frequency, weight


def fit_flyover(t, frequency, weight=None, c=343.0):
    t, frequency = np.asarray(t), np.asarray(frequency)
    weight = np.ones(len(t)) if weight is None else np.maximum(np.asarray(weight), 0.05)
    weight /= np.mean(weight)
    middle = (t[0] + t[-1]) / 2
    x = t - middle
    initial = [np.median(frequency), 80.0, 1500.0, 0.0]
    bounds = ([np.min(frequency)*0.5, 10, 50, x[0]],
              [np.max(frequency)*1.5, min(250, c*0.85), 20000, x[-1]])
    result = optimize.least_squares(
        lambda p: (straight_doppler(x, *p, c=c) - frequency) * np.sqrt(weight),
        initial, bounds=bounds, loss="soft_l1", f_scale=1.0, x_scale="jac", max_nfev=1000)
    residual = straight_doppler(x, *result.x, c=c) - frequency
    f0, speed, slant, tc = result.x
    return {"f0_hz": float(f0), "speed_mps": float(speed), "slant_m": float(slant),
            "cpa_receive_s": float(tc + middle),
            "rmse_hz": float(np.sqrt(np.average(residual**2, weights=weight))),
            "frequency_drop_hz": float(np.percentile(frequency[:max(3,len(t)//8)],50) -
                                       np.percentile(frequency[-max(3,len(t)//8):],50)),
            "optimizer_success": bool(result.success)}


def measure(path, settings, output=None):
    data, sr = decode(path, settings["start_s"], settings["end_s"] - settings["start_s"])
    spec = spectrogram(data, sr, settings["band_hz"], settings["start_s"],
                       settings.get("window_s", 2.0), settings.get("hop_s", 0.25))
    if settings.get("ridge_csv"):
        ridge = np.genfromtxt(settings["ridge_csv"], names=True, delimiter=",")
        frequency = np.interp(spec.time, ridge["time_s"], ridge["frequency_hz"], left=np.nan, right=np.nan)
        weight = np.ones(len(frequency))
    else:
        frequency, weight = trace_ridge(spec, settings.get("max_slope_hz_s", 12),
                                        max_up_slope=settings.get("max_up_slope_hz_s",1.0))
    valid = np.isfinite(frequency)
    fit = fit_flyover(spec.time[valid], frequency[valid], weight[valid])
    fit["ridge_contrast"] = float(np.median(spec.sample(spec.time[valid], frequency[valid])))
    fit["detected"] = bool(fit["ridge_contrast"] >= settings.get("min_contrast", 1.5)
                           and fit["frequency_drop_hz"] >= settings.get("min_drop_hz", 4)
                           and fit["rmse_hz"] <= settings.get("max_fit_rmse_hz", 8))
    if output:
        from pathlib import Path
        prefix = Path(output)
        prefix.parent.mkdir(parents=True, exist_ok=True)
        prefix.with_suffix(".json").write_text(json.dumps(fit, indent=2) + "\n")
        np.savetxt(prefix.with_suffix(".csv"), np.column_stack((spec.time, frequency, weight)),
                   delimiter=",", header="time_s,frequency_hz,weight", comments="")
        np.savez_compressed(prefix.with_suffix(".npz"), time=spec.time, frequency=spec.frequency,
                            db=spec.db, contrast=spec.contrast, ridge=frequency, weight=weight)
        plot_measurement(spec, frequency, fit, prefix.with_suffix(".png"))
    return spec, frequency, weight, fit


def plot_measurement(spec, ridge, fit, path):
    import matplotlib
    matplotlib.use("Agg")
    from matplotlib import pyplot as plt
    fig, ax = plt.subplots(figsize=(10, 4), layout="constrained")
    ax.pcolormesh(spec.time, spec.frequency, spec.contrast, shading="auto", cmap="magma", vmin=0, vmax=6)
    ax.plot(spec.time, ridge, color="cyan", lw=0.8, label="Measured ridge")
    ax.plot(spec.time, straight_doppler(spec.time, fit["f0_hz"], fit["speed_mps"],
            fit["slant_m"], fit["cpa_receive_s"]), color="lime", lw=1, label="Straight-pass fit")
    ax.set(xlabel="Recording time (s)", ylabel="Frequency (Hz)",
           title=f"v={fit['speed_mps']:.1f} m/s; slant={fit['slant_m']:.0f} m; residual={fit['rmse_hz']:.2f} Hz")
    ax.legend(loc="upper right")
    fig.savefig(path, dpi=150)
    plt.close(fig)
