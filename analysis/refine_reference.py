"""Conditional local refinement and sensitivity using the Rust ranking executable.

Take distinct flight pairs from an already completed coarse result. All locations
come from those results. An optional evaluation reference is opened only after
every search completes and is never used to construct candidates or fit models.
This is a local conditional test, not a blind nationwide validation.
"""
from __future__ import annotations

import argparse
import copy
import json
import math
from pathlib import Path
import subprocess
import time

import numpy as np

from import_reference import GeographicRaster, file_hash, write_json


def load_json(path: Path):
    with path.open(encoding="utf-8") as stream:
        return json.load(stream)


def grid_around(center, radius_m, spacing_m, dem, geoid, microphone_height_m):
    latitude = math.radians(center["latitude_deg"])
    e2 = 6.6943799901413165e-3
    denominator = 1.0 - e2 * math.sin(latitude) ** 2
    lat_m = 6378137.0 * (1.0 - e2) / denominator ** 1.5 * math.pi / 180
    lon_m = 6378137.0 / math.sqrt(denominator) * math.cos(latitude) * math.pi / 180
    count = math.ceil(radius_m / spacing_m)
    if (2 * count + 1) ** 2 > 100000:
        raise ValueError("Local refinement grid exceeds 100,000 points")
    observers = []
    for north in range(-count, count + 1):
        for east in range(-count, count + 1):
            lat = center["latitude_deg"] + north * spacing_m / lat_m
            lon = center["longitude_deg"] + east * spacing_m / lon_m
            height = dem.sample(lat, lon) + geoid.sample(lat, lon) + microphone_height_m
            observers.append({"id": f"n{north}-e{east}", "latitude_deg": lat,
                              "longitude_deg": lon, "ellipsoid_height_m": height})
    return observers, count


def on_grid_boundary(observer_id, count):
    north, east = observer_id[1:].split("-e")
    return abs(int(north)) == count or abs(int(east)) == count


def offset_aircraft_height(track, offset_m):
    if offset_m == 0:
        return copy.deepcopy(track)
    changed = copy.deepcopy(track)
    # Bowring latitude on WGS84 gives the ellipsoid normal. Adding the height
    # offset along that normal retains latitude/longitude, unlike a geocentric
    # radial shift. No source heading/velocity field is modified or consumed.
    a, f = 6378137.0, 1 / 298.257223563
    b = a * (1 - f)
    e2 = f * (2 - f)
    ep2 = (a*a - b*b) / (b*b)
    for point in changed["points"]:
        x, y, z = point["xyz_m"]
        horizontal = math.hypot(x, y)
        theta = math.atan2(z*a, horizontal*b)
        lat = math.atan2(z + ep2*b*math.sin(theta)**3,
                         horizontal - e2*a*math.cos(theta)**3)
        lon = math.atan2(y, x)
        normal = [math.cos(lat)*math.cos(lon), math.cos(lat)*math.sin(lon), math.sin(lat)]
        point["xyz_m"] = [v + offset_m*n for v, n in zip([x,y,z], normal)]
    return changed


def run_rank(exe, search, directory, timeout_s):
    directory.mkdir(parents=True, exist_ok=False)
    input_path, result_path = directory / "input.json", directory / "results.json"
    write_json(input_path, search)
    command = [str(exe), "rank", "--input", str(input_path.resolve()), "--out", str(result_path.resolve())]
    start = time.monotonic()
    completed = subprocess.run(command, capture_output=True, text=True, timeout=timeout_s,
                               creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    elapsed = time.monotonic() - start
    (directory / "stdout.txt").write_text(completed.stdout, encoding="utf-8")
    (directory / "stderr.txt").write_text(completed.stderr, encoding="utf-8")
    if completed.returncode not in (0, 3) or not result_path.is_file():
        raise RuntimeError(f"Rust rank failed in {directory}: exit {completed.returncode}; inspect saved logs")
    result = load_json(result_path)
    return result, {"input": str(input_path.resolve()), "results": str(result_path.resolve()),
                    "return_code": completed.returncode, "elapsed_s": elapsed,
                    "input_sha256": file_hash(input_path), "results_sha256": file_hash(result_path)}


def best_row(result):
    candidates = result.get("candidates", [])
    return candidates[0] if candidates else None


def distance_m(first, second):
    phi1, phi2 = map(math.radians, [first["latitude_deg"], second["latitude_deg"]])
    dphi = phi2 - phi1
    dlambda = math.radians(second["longitude_deg"] - first["longitude_deg"])
    h = math.sin(dphi/2)**2 + math.cos(phi1)*math.cos(phi2)*math.sin(dlambda/2)**2
    return 6371008.8 * 2 * math.asin(min(1.0, math.sqrt(max(0.0, h))))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--coarse-results", type=Path, required=True)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--dem", type=Path, required=True)
    parser.add_argument("--geoid", type=Path, required=True)
    parser.add_argument("--microphone-height-m", type=float, default=3)
    parser.add_argument("--top-pairs", type=int, default=3)
    parser.add_argument("--radius-m", type=float, default=600)
    parser.add_argument("--spacing-m", type=float, default=100)
    parser.add_argument("--fine-radius-m", type=float, default=150)
    parser.add_argument("--fine-spacing-m", type=float, default=25)
    parser.add_argument("--timeout-s", type=float, default=300)
    parser.add_argument("--evaluation-reference", type=Path,
                        help="Optional JSON with latitude_deg, longitude_deg and source; read only after every search")
    args = parser.parse_args()
    if not 1 <= args.top_pairs <= 10:
        parser.error("Use 1 to 10 distinct coarse flight pairs")
    for name in ["radius_m", "spacing_m", "fine_radius_m", "fine_spacing_m", "timeout_s"]:
        if not math.isfinite(getattr(args, name)) or getattr(args, name) <= 0:
            parser.error(f"{name} must be finite and positive")
    if not math.isfinite(args.microphone_height_m) or args.microphone_height_m < 0:
        parser.error("Microphone height must be finite and nonnegative")
    if args.out_dir.exists():
        parser.error("Use a fresh output directory to preserve prior evidence")
    if not args.coarse_results.is_file():
        parser.error("The coarse result has not completed; no search was started")
    exe = args.exe.resolve(strict=True)
    source = load_json(args.input)
    coarse = load_json(args.coarse_results)
    tracks = {t["flight_id"]: t for t in source["tracks"]}
    selected, seen = [], set()
    for row in coarse.get("best_per_flight_pair", []):
        pair = tuple(row["score"]["flight_ids"])
        if pair not in seen:
            if any(identifier not in tracks for identifier in pair):
                raise ValueError("Coarse result references tracks missing from the supplied search input")
            selected.append(row)
            seen.add(pair)
        if len(selected) == args.top_pairs:
            break
    if not selected:
        parser.error("Coarse search has no ranked pair to refine")
    dem, geoid = GeographicRaster(args.dem), GeographicRaster(args.geoid)
    args.out_dir.mkdir(parents=True)
    plan = {"scope": "conditional refinement of top distinct flight pairs from an existing coarse search",
            "source_input": str(args.input.resolve()), "source_input_sha256": file_hash(args.input),
            "coarse_results": str(args.coarse_results.resolve()), "coarse_results_sha256": file_hash(args.coarse_results),
            "executable": str(exe), "executable_sha256": file_hash(exe),
            "script_sha256": file_hash(Path(__file__)),
            "selected_coarse_pairs": selected, "radius_m": args.radius_m, "spacing_m": args.spacing_m,
            "fine_radius_m": args.fine_radius_m, "fine_spacing_m": args.fine_spacing_m,
            "microphone_height_m": args.microphone_height_m,
            "dem": dem.provenance(), "geoid": geoid.provenance(),
            "evaluation_reference_not_read_before_search": True}
    write_json(args.out_dir / "plan.json", plan)
    scenarios = [("baseline", 343.0, 0.0), ("sound-338", 338.0, 0.0),
                 ("sound-348", 348.0, 0.0), ("aircraft-height-minus50", 343.0, -50.0),
                 ("aircraft-height-plus50", 343.0, 50.0)]
    summary = {"status": "completed", "location_verified": False, "scenarios": [],
               "scope": plan["scope"],
               "limitations": [
                   "Only retained coarse flight pairs and local footprints are evaluated; a different omitted pair or distant location can still win.",
                   "The 25 m stage refines only the best 100 m result under each scenario; it is conditional on that pair and local region.",
                   "Sensitivity tests change one assumption at a time; these are not exhaustive uncertainty intervals or calibrated probabilities.",
                   "All scoring uses the Rust executable with the original trace samples, frequency sweeps, date clock bounds, and coarse filters.",
                   "A grid-edge minimum signals incomplete local coverage, not an established optimum."]}
    for scenario_name, c, height_offset in scenarios:
        scenario_dir = args.out_dir / scenario_name
        scenario_dir.mkdir()
        scenario = {"name": scenario_name, "sound_speed_mps": c,
                    "aircraft_height_offset_m": height_offset, "pair_runs": []}
        winners = []
        for pair_index, seed in enumerate(selected):
            identifiers = seed["score"]["flight_ids"]
            observers, boundary = grid_around(seed["observer"], args.radius_m, args.spacing_m,
                                               dem, geoid, args.microphone_height_m)
            search = {k: copy.deepcopy(v) for k,v in source.items() if k not in ("tracks", "observers")}
            search["tracks"] = [offset_aircraft_height(tracks[identifier], height_offset) for identifier in identifiers]
            search["observers"] = observers
            search["pair_config"]["model"]["sound_speed_mps"] = c
            search["provenance"].append(f"Conditional local refinement of coarse pair {pair_index}; sound={c}m/s, aircraft height shift={height_offset}m, microphone height={args.microphone_height_m}m")
            result, receipt = run_rank(exe, search, scenario_dir / f"pair-{pair_index+1}", args.timeout_s)
            winner = best_row(result)
            record = {"flight_ids": identifiers, "observer_count": len(observers), "status": result["status"],
                      "ranked_count": result.get("ranked_count", 0), "receipt": receipt, "best": winner}
            if winner:
                record["best_at_grid_boundary"] = on_grid_boundary(winner["observer"]["id"], boundary)
                winners.append((winner["score"]["combined_rmse_hz"], pair_index, winner, search))
            scenario["pair_runs"].append(record)
        if winners:
            _, pair_index, local_best, search = min(winners, key=lambda item: item[0])
            fine_observers, boundary = grid_around(local_best["observer"], args.fine_radius_m,
                                                   args.fine_spacing_m, dem, geoid, args.microphone_height_m)
            search["observers"] = fine_observers
            search["provenance"].append("Conditional fine grid around the winning local refinement point; no evaluation coordinate used")
            result, receipt = run_rank(exe, search, scenario_dir / "fine", args.timeout_s)
            winner = best_row(result)
            scenario["fine"] = {"parent_pair_index": pair_index, "observer_count": len(fine_observers),
                                "status": result["status"], "receipt": receipt, "best": winner}
            if winner:
                scenario["fine"]["best_at_grid_boundary"] = on_grid_boundary(winner["observer"]["id"], boundary)
        else:
            scenario["fine"] = {"status": "not_run_no_local_candidate", "best": None}
        summary["scenarios"].append(scenario)
        write_json(scenario_dir / "summary.json", scenario)
        winner = scenario["fine"].get("best")
        print(json.dumps({"scenario": scenario_name, "best": winner}, separators=(",", ":")), flush=True)
    baseline = summary["scenarios"][0]["fine"].get("best")
    for scenario in summary["scenarios"]:
        winner = scenario["fine"].get("best")
        if baseline and winner:
            scenario["distance_from_baseline_m"] = distance_m(baseline["observer"], winner["observer"])
            scenario["same_flight_pair_as_baseline"] = winner["score"]["flight_ids"] == baseline["score"]["flight_ids"]
    # Evaluation reference is intentionally opened only here, after every
    # candidate generation, model run, scenario choice and refinement is done.
    if args.evaluation_reference:
        reference = load_json(args.evaluation_reference)
        if not (math.isfinite(reference["latitude_deg"]) and -90 <= reference["latitude_deg"] <= 90
                and math.isfinite(reference["longitude_deg"]) and -180 <= reference["longitude_deg"] <= 180
                and reference.get("source")):
            raise ValueError("Evaluation reference needs valid latitude, longitude, and source")
        summary["evaluation"] = {"reference": reference, "reference_sha256": file_hash(args.evaluation_reference),
                                  "read_after_all_searches": True, "distance_method": "mean-Earth-radius great-circle distance"}
        for scenario in summary["scenarios"]:
            winner = scenario["fine"].get("best")
            if winner:
                scenario["distance_from_evaluation_reference_m"] = distance_m(winner["observer"], reference)
    write_json(args.out_dir / "summary.json", summary)
    print(f"Completed conditional refinement: {args.out_dir / 'summary.json'}", flush=True)


if __name__ == "__main__":
    main()
