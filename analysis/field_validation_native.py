"""Run actual native discovery and geometric ADSB import without receiver truth."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def run(args):
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    audio_root, tracks_root = Path(args.audio).resolve(), Path(args.tracks).resolve()
    executable, tools = Path(args.executable).resolve(), Path(args.tools).resolve()
    policy_path = Path(args.policy).resolve()
    policy = json.loads(policy_path.read_text(encoding="utf-8"))
    audio_receipt = json.loads((audio_root / "acquisition.json").read_text(encoding="utf-8"))
    tracks_receipt = json.loads((tracks_root / "acquisition.json").read_text(encoding="utf-8"))
    waveform = audio_root / audio_receipt["audio"]["path"]
    if digest(waveform) != audio_receipt["audio"]["sha256"]:
        raise ValueError("Waveform checksum mismatch")
    settings = output / "discovery-settings.json"
    write(settings, policy["discovery_settings"])
    def stamp(value):
        return datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
    config = {"source_label": "Actual public ADSB.lol map history; independently acquired regional candidates; incomplete coverage",
              "time_origin_utc_s": stamp(audio_receipt["sample_zero_filename_utc_claim"]),
              "time_window_utc_s": [stamp(tracks_receipt[key]) for key in ("utc_start", "utc_end")],
              "bounds": policy["search_bounds_south_west_north_east"],
              "max_gap_s": 30.0, "max_speed_mps": 330.0,
              "accepted_sources": ["adsb_icao", "adsr_icao"], "altitude_policy": {"mode": "geometric_only"}}
    config_path = output / "track-import-config.json"
    write(config_path, config)
    tool_paths = [executable, tools / "ffmpeg.exe", tools / "ffprobe.exe"]
    receipt = {"schema_version": 1, "started_utc": datetime.now(timezone.utc).isoformat(),
               "policy_path": str(policy_path), "policy_sha256": digest(policy_path),
               "audio_acquisition_sha256": digest(audio_root / "acquisition.json"),
               "tracks_acquisition_sha256": digest(tracks_root / "acquisition.json"),
               "waveform_sha256": digest(waveform), "receiver_truth_read": False,
               "executables": [{"path": str(path), "sha256": digest(path)} for path in tool_paths],
               "commands": [], "location_accuracy_validated": False}
    env = dict(os.environ)
    env["PATH"] = str(tools) + os.pathsep + env.get("PATH", "")
    def invoke(label, arguments):
        started = time.monotonic()
        command = [str(executable), *map(str, arguments)]
        entry = {"label": label, "argv": command, "started_utc": datetime.now(timezone.utc).isoformat()}
        receipt["commands"].append(entry)
        write(output / "native-receipt.json", receipt)
        print("Starting " + label, flush=True)
        with (output / (label + ".stdout.txt")).open("w", encoding="utf-8") as stdout, (output / (label + ".stderr.txt")).open("w", encoding="utf-8") as stderr:
            result = subprocess.run(command, stdout=stdout, stderr=stderr, env=env,
                                    creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0), timeout=1200)
        entry.update(exit_code=result.returncode, elapsed_s=time.monotonic() - started)
        write(output / "native-receipt.json", receipt)
        if result.returncode:
            raise RuntimeError(f"Native {label} failed with {result.returncode}; see retained logs")
    invoke("discovery", ["discover", "--input", waveform, "--out", output / "discovery", "--channel", "0", "--settings", settings])
    (output / "normalized-traces").mkdir()
    for trace in tracks_receipt["traces"]:
        source = tracks_root / trace["path"]
        if digest(source) != trace["sha256"]:
            raise ValueError("Source aircraft trace checksum mismatch")
        invoke("import-" + trace["icao24"], ["import-adsblol-trace", "--input", source, "--config", config_path,
                                             "--out", output / "normalized-traces" / (trace["icao24"] + ".json")])
    discovery = json.loads((output / "discovery" / "discovery.json").read_text(encoding="utf-8"))
    normalized = [json.loads(path.read_text(encoding="utf-8")) for path in (output / "normalized-traces").glob("*.json")]
    summary = {"status": "native_audio_and_track_import_complete", "discovery_status": discovery["status"],
               "group_count": discovery["group_count"], "ridge_count": discovery["ridge_count"],
               "imported_trace_count": len(normalized),
               "output_passes": sum(item["stats"]["output_passes"] for item in normalized),
               "output_points": sum(item["stats"]["output_points"] for item in normalized),
               "receiver_truth_read": False, "geolocation_accuracy_validated": False,
               "coverage_complete": False, "policy_sha256": digest(policy_path)}
    write(output / "summary.json", summary)
    print(json.dumps(summary, indent=2), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audio", required=True)
    parser.add_argument("--tracks", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--policy", default="benchmarks/field_validation_policy.json")
    parser.add_argument("--executable", default="dist/GeoSonnar-Windows-0.3.0/geosonnar.exe")
    parser.add_argument("--tools", default="C:/ProgramData/chocolatey/lib/ffmpeg-full/tools/ffmpeg/bin")
    run(parser.parse_args())
