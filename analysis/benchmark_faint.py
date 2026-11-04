"""Native known-clock faint-event development regression against pinned real inputs.

The search sees only supplied source-informed bounds, ridge, terrain and tracks.
Evaluation references are opened after its subprocess exits. This is procedural
separation on one filesystem, not a sealed holdout or independent accuracy test.
"""
from __future__ import annotations
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path: Path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, allow_nan=False, separators=(",", ":"))
        stream.write("\n")


def wgs84_distance_m(first, second):
    """Vincenty inverse on WGS84; fail explicitly if it does not converge."""
    a, flattening = 6378137.0, 1 / 298.257223563
    b = a * (1 - flattening)
    lat1, lon1, lat2, lon2 = map(math.radians, (*first, *second))
    if first == second:
        return 0.0
    u1, u2 = math.atan((1 - flattening) * math.tan(lat1)), math.atan((1 - flattening) * math.tan(lat2))
    sin1, cos1, sin2, cos2 = math.sin(u1), math.cos(u1), math.sin(u2), math.cos(u2)
    longitude = lon2 - lon1
    current = longitude
    for _ in range(100):
        sin_l, cos_l = math.sin(current), math.cos(current)
        sin_sigma = math.hypot(cos2 * sin_l, cos1 * sin2 - sin1 * cos2 * cos_l)
        if sin_sigma == 0:
            return 0.0
        cos_sigma = sin1 * sin2 + cos1 * cos2 * cos_l
        sigma = math.atan2(sin_sigma, cos_sigma)
        sin_alpha = cos1 * cos2 * sin_l / sin_sigma
        cos2_alpha = 1 - sin_alpha**2
        cos2_middle = cos_sigma - 2 * sin1 * sin2 / cos2_alpha if cos2_alpha else 0.0
        coefficient = flattening / 16 * cos2_alpha * (4 + flattening * (4 - 3 * cos2_alpha))
        updated = longitude + (1 - coefficient) * flattening * sin_alpha * (
            sigma + coefficient * sin_sigma * (cos2_middle + coefficient * cos_sigma * (-1 + 2 * cos2_middle**2)))
        if abs(updated - current) < 1e-12:
            break
        current = updated
    else:
        raise ValueError("WGS84 inverse did not converge")
    u_squared = cos2_alpha * (a*a - b*b) / (b*b)
    aa = 1 + u_squared / 16384 * (4096 + u_squared * (-768 + u_squared * (320 - 175 * u_squared)))
    bb = u_squared / 1024 * (256 + u_squared * (-128 + u_squared * (74 - 47 * u_squared)))
    delta = bb * sin_sigma * (cos2_middle + bb / 4 * (
        cos_sigma * (-1 + 2 * cos2_middle**2) - bb / 6 * cos2_middle * (-3 + 4 * sin_sigma**2) * (-3 + 4 * cos2_middle**2)))
    return b * aa * (sigma - delta)


def run_faint_benchmark(project_root, output_dir, executable=None):
    root, output = Path(project_root).resolve(), Path(output_dir).resolve()
    output.mkdir(parents=True, exist_ok=False)
    source_binary = Path(executable).resolve() if executable else root / "target/release/geosonnar.exe"
    binary = output / "geosonnar-snapshot.exe"
    shutil.copy2(source_binary, binary)
    shutil.copy2(Path(__file__), output / "runner-snapshot.py")
    (output / "source-snapshot").mkdir()
    for source_path in sorted((root / "src").glob("*.rs")):
        shutil.copy2(source_path, output / "source-snapshot" / source_path.name)
    config_path = root / "benchmarks/cases/reference-faint.json"
    config = json.loads(config_path.read_text(encoding="utf-8"))
    base = config_path.parent
    paths = {name: (base / config[name]).resolve() for name in ("normalized_tracks", "observers_file", "ridge_csv")}
    normalized = json.loads(paths["normalized_tracks"].read_text(encoding="utf-8"))
    observers = json.loads(paths["observers_file"].read_text(encoding="utf-8"))
    with paths["ridge_csv"].open(newline="", encoding="utf-8") as stream:
        ridge = [{"t_rec_s": float(row["time_s"]), "frequency_hz": float(row["frequency_hz"])} for row in csv.DictReader(stream)]
    start, end = config["event_window_s"]
    if not ridge or any(not start <= p["t_rec_s"] <= end for p in ridge):
        raise ValueError("Pinned ridge lies outside declared event window")
    provenance = list(config["provenance"])
    provenance.append("Normalization statistics: " + json.dumps(normalized.get("stats", {})))
    provenance.extend(normalized.get("quality_notes", []))
    source = normalized["config"]["source_label"]
    expected_source_hash = "b43c7cd878d8378bede0705046c9034bed4741930a1669a544688859675c6154"
    if digest(Path(source)) != expected_source_hash or normalized["config"]["source_sha256"] != expected_source_hash:
        raise ValueError("Faint-window source identity mismatch")
    native_input = {
        "schema_version": 1, "evidence_kind": "real_recording", "provenance": provenance,
        "tracks": [row["track"] for row in normalized["passes"]], "observers": observers,
        "event": {"points": ridge, "f0_hz": config["f0_hz"]}, "model": config["model"],
        "clock_offsets_s": config["clock_offsets_s"], "coarse_bounds": config["coarse_bounds"],
    }
    input_path = output / "search-input.json"
    write_json(input_path, native_input)
    receipt = {
        "schema_version": 1, "case_id": config["case_id"], "upstream_commit": config["upstream_commit"],
        "config_sha256": digest(config_path), "input_sha256": digest(input_path),
        "binary_sha256": digest(binary), "binary_snapshot": str(binary), "python": sys.version, "platform": platform.platform(),
        "source_files": {str(p.relative_to(root)): digest(p) for p in sorted((root / "src").glob("*.rs"))},
        "runner_sha256": digest(Path(__file__)), "input_files": {str(path): digest(path) for path in paths.values()},
        "source_parquet_sha256": expected_source_hash, "normalization_stats": normalized.get("stats"),
        "frozen_parameters": {key: config[key] for key in ("observer_bounds", "grid_spacing_m", "event_window_s", "f0_hz", "clock_offsets_s", "model", "coarse_bounds")},
        "observed_ridge_weight_policy": "All pinned CSV rows used; equal weights. Upstream weight column is not used by this RMSE adapter.",
        "source_aircraft_count": normalized.get("stats", {}).get("source_aircraft"),
        "track_count": len(native_input["tracks"]), "observer_count": len(observers), "ridge_samples": len(ridge),
        "acceptance": {"expected_aircraft_matches_frozen_upstream_result": True, "maximum_platform_error_m": 1000,
                       "historical_100m_gate_reported_separately": True, "independent_location_accuracy_eligible": False},
        "separation": "Evaluation files are opened after the native process exits; same filesystem, not an isolation claim.",
    }
    write_json(output / "input-receipt.json", receipt)
    command = [str(binary), "rank-single", "--input", str(input_path), "--out", str(output / "results.json")]
    # Avoid retaining the Python object graph alongside the native search.
    del native_input, normalized
    began = time.perf_counter()
    try:
        with (output / "stdout.log").open("wb") as stdout, (output / "stderr.log").open("wb") as stderr:
            completed = subprocess.run(command, cwd=root, stdout=stdout, stderr=stderr, timeout=900, check=False)
    except subprocess.TimeoutExpired:
        execution = {"command": command, "returncode": None, "elapsed_s": time.perf_counter() - began,
                     "timed_out": True, "timeout_s": 900}
        write_json(output / "execution.json", execution)
        failure = {"schema_version": 1, "case_id": config["case_id"], "passed": False,
                   "status": "execution_timeout", "execution": execution,
                   "truth_read_after_search": False,
                   "limitation": "Native search exceeded the frozen runtime budget; no scientific outcome inferred."}
        write_json(output / "report.json", failure)
        return failure
    execution = {"command": command, "returncode": completed.returncode, "elapsed_s": time.perf_counter() - began}
    write_json(output / "execution.json", execution)
    if completed.returncode != 0:
        result = {"passed": False, "status": "execution_failed" if completed.returncode != 3 else "no_candidate", "execution": execution}
        write_json(output / "report.json", result)
        return result
    result_path = output / "results.json"
    result = json.loads(result_path.read_text(encoding="utf-8"))
    # Evaluation boundary: all search output already exists and is immutable.
    baseline_path = (base / config["evaluation_only"]["baseline_file"]).resolve()
    baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
    association_path = baseline_path.with_name("report.json")
    baseline_search = json.loads(association_path.read_text(encoding="utf-8"))
    references_path = root / "benchmarks/truth/reference-locations.json"
    references = json.loads(references_path.read_text(encoding="utf-8"))
    best = result["candidates"][0]
    location = [best["observer"]["latitude_deg"], best["observer"]["longitude_deg"]]
    errors = {name: wgs84_distance_m(location, info["coordinate"]) for name, info in references.items()}
    actual_id = best["score"]["flight_id"].split("#", 1)[0]
    expected_id = baseline_search["best"]["flight_ids"][0]
    checks = {
        "native_search_completed": result["status"] == "hypotheses_ranked",
        "matching_source_aircraft": actual_id == expected_id,
        "full_fixed_ridge_support": best["score"]["support_fraction"] == 1.0,
        "location_not_automatically_verified": result["location_verified"] is False,
        "finite_residual": math.isfinite(best["score"]["rmse_hz"]),
        "within_current_1000m_development_target": errors["upstream_platform"] <= 1000,
    }
    report = {
        "schema_version": 1, "case_id": config["case_id"], "passed": all(checks.values()), "checks": checks,
        "execution": execution, "search_result_sha256": digest(result_path), "input_receipt_sha256": digest(output / "input-receipt.json"),
        "best": best, "expected_source_aircraft": expected_id, "source_aircraft_matches": actual_id == expected_id,
        "scoring_counts": {key: result.get(key) for key in ("ranked_count", "supported_clock_trials", "unsupported_clock_trials", "coarse_rejections", "model_rejections", "factor_evaluations", "factor_cache_hits")},
        "horizontal_error_m": errors, "tiers": {name: {str(tier): error <= tier for tier in (100, 500, 1000)} for name, error in errors.items()},
        "historical_upstream_error_m": baseline["error_m"], "historical_upstream_100m_passed": baseline["meets_threshold"],
        "evaluation_files": {str(p): digest(p) for p in (baseline_path, association_path, references_path)},
        "truth_read_after_search": True, "independent_location_accuracy_eligible": False,
        "limitations": ["Source-informed observer region, event ridge and inherited fitted clock.",
                        "Coarse 200m grid only; this run performs no post-evaluation refinement or threshold tuning.",
                        "Linear tracks and equal-weight fixed-ridge RMSE differ from upstream interpolation and weighted scoring.",
                        "Published platform and video coordinates are not surveyed microphone truth; this is not a worldwide accuracy result."],
    }
    write_json(output / "report.json", report)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--exe", type=Path)
    args = parser.parse_args()
    report = run_faint_benchmark(args.project, args.out, args.exe)
    print(json.dumps({key: report.get(key) for key in ("passed", "checks", "horizontal_error_m", "best", "execution")}, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
