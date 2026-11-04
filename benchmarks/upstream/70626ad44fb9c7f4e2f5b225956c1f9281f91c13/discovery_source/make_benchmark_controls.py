"""Independent emission-domain discovery controls; labels never enter requests."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

import numpy as np
import soundfile as sf


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def make_controls(output):
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False)
    inputs = output / "inputs"
    hidden = output / "truth"
    inputs.mkdir()
    hidden.mkdir()
    cases, labels = [], {}
    sr = 8000
    for split, seed in (("development", 892741), ("held_out", 219305)):
        rng = np.random.default_rng(seed)
        for kind in ("noise", "stationary", "non_aircraft_sweep", "harmonic", "overlap", "short", "mp3"):
            # Opaque filenames prevent the algorithm from learning control labels.
            key = f"control-{len(cases) + 1:03d}"
            duration = 8 if kind == "short" else 100
            t = np.arange(duration * sr) / sr
            audio = rng.normal(0, 0.025, len(t))
            events = []
            if kind == "stationary":
                audio += 0.18 * np.sin(2 * np.pi * 173 * t)
            elif kind == "non_aircraft_sweep":
                # An ascending electronic chirp, without moving-source physics.
                audio += 0.2 * np.sin(2 * np.pi * (110 * t + 0.6 * t ** 2))
            elif kind not in {"noise"}:
                centers = [32, 59] if kind == "overlap" else [45]
                if kind == "short":
                    centers = [0]
                for index, center in enumerate(centers):
                    v = 73 + rng.uniform(-8, 8)
                    slant = 1400 + rng.uniform(-120, 120)
                    frequency = 126 + index * 59 + rng.uniform(-4, 4)
                    emission = np.arange(-32 * sr, 32 * sr) / sr
                    distance = np.sqrt(slant ** 2 + (v * emission) ** 2)
                    arrival = center + emission + distance / 343
                    envelope = np.hanning(len(emission)) * (slant / distance)
                    # Source phase is transported to receiver time, independent
                    # of dopplergeist.geometry and the inverse search model.
                    source = 0.22 * envelope * (np.sin(2 * np.pi * frequency * emission) +
                              0.6 * np.sin(2 * np.pi * 2 * frequency * emission + 0.3))
                    audio += np.interp(t, arrival, source, left=0, right=0)
                    # Nonzero finite support; short clips are deliberately too
                    # truncated for a full Doppler constraint and are negative.
                    if kind != "short":
                        events.append({"start_s": max(0, float(arrival[0])),
                                       "end_s": min(duration, float(arrival[-1]))})
            audio_path = inputs / f"{key}.wav"
            sf.write(audio_path, audio, sr, subtype="PCM_16")
            if kind == "mp3":
                mp3 = audio_path.with_suffix(".mp3")
                subprocess.run(["ffmpeg", "-v", "error", "-i", str(audio_path),
                                "-map_metadata", "-1", "-codec:a", "libmp3lame", "-b:a", "64k", str(mp3)], check=True)
                audio_path.unlink()
                audio_path = mp3
            request = {"schema_version": 1, "audio_path": audio_path.name,
                       "time_window": {"start_utc": "2026-09-21T14:12:20Z", "end_utc": "2026-09-21T14:14:20Z",
                                       "source": "user", "source_detail": "Synthetic fixture clock; not inferred from filesystem."},
                       "search": {"coverage": "conus"}}
            write_json(inputs / f"{key}.json", request)
            cases.append({"case_id": key, "split": split, "scope": "synthetic_discovery_control",
                          "request": f"{key}.json", "audio_sha256": hashlib.sha256(audio_path.read_bytes()).hexdigest(),
                          "duration_s": duration})
            labels[key] = {"events": events, "control_kind": kind, "location_accuracy_eligible": False,
                           "expect_inconclusive": True,
                           "eligibility_limit": "Discovery-only audio control, no location-labelled historical flight corpus."}
    write_json(inputs / "manifest.json", {"schema_version": 1, "cases": cases})
    write_json(hidden / "labels.json", labels)
    write_json(output / "provenance.json", {"generator": str(Path(__file__).name),
               "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
               "license": "Generated audio contains no third-party media; the repository has no declared license.",
               "seeds": {"development": 892741, "held_out": 219305},
               "limits": "Public deterministic synthetic splits, not independent real validation. Short clips are unsupported constraints."})
    return inputs / "manifest.json"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    print(make_controls(parser.parse_args().output))
