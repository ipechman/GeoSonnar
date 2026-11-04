"""One additional, explicit shared-clock-window sensitivity after baseline fitting."""
import argparse
import copy
import json
from pathlib import Path

import numpy as np

from import_reference import GeographicRaster, file_hash, write_json
from refine_reference import load_json, grid_around, on_grid_boundary, run_rank, best_row, distance_m


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--input", type=Path, required=True)
    p.add_argument("--coarse-results", type=Path, required=True)
    p.add_argument("--baseline-summary", type=Path, required=True)
    p.add_argument("--exe", type=Path, required=True)
    p.add_argument("--out-dir", type=Path, required=True)
    p.add_argument("--dem", type=Path, required=True)
    p.add_argument("--geoid", type=Path, required=True)
    p.add_argument("--evaluation-reference", type=Path)
    args = p.parse_args()
    if args.out_dir.exists():
        p.error("Use a fresh output directory")
    exe = args.exe.resolve(strict=True)
    source = load_json(args.input)
    coarse = load_json(args.coarse_results)
    # Read the baseline's search result only; do not consume its post-search
    # evaluation reference when constructing this new sensitivity search.
    baseline = load_json(args.baseline_summary)["scenarios"][0]["fine"]["best"]
    tracks = {t["flight_id"]: t for t in source["tracks"]}
    selected = []
    seen = set()
    for row in coarse["best_per_flight_pair"]:
        key = tuple(row["score"]["flight_ids"])
        if key not in seen:
            selected.append(row)
            seen.add(key)
        if len(selected) == 3:
            break
    dem, geoid = GeographicRaster(args.dem), GeographicRaster(args.geoid)
    args.out_dir.mkdir(parents=True)
    report = {"status": "complete", "location_verified": False,
              "reason": "Baseline shared timing shift reached the original +5s bound; this run explicitly tests a wider range without replacing that baseline.",
              "settings": {"shared_shift_range_s": [-10, 10], "shared_shift_step_s": .25,
                           "sound_speed_mps": 343, "aircraft_height_offset_m": 0,
                           "microphone_height_m": 3, "event1_cpa_recording_s": source["pair_config"]["event1_cpa_recording_s"],
                           "event2_timing": "locked to the same shared clock"},
              "source_input_sha256": file_hash(args.input), "coarse_result_sha256": file_hash(args.coarse_results),
              "executable_sha256": file_hash(exe), "script_sha256": file_hash(Path(__file__)),
              "baseline": baseline, "pair_runs": [],
              "limitations": ["Conditional on retained regional pairs and local footprints.",
                              "The original baseline and its five parameter scenarios remain unchanged.",
                              "This additional development sensitivity is not an independent validation."]}
    winners = []
    for index, seed in enumerate(selected):
        observers, boundary = grid_around(seed["observer"], 600, 100, dem, geoid, 3)
        search = {k: copy.deepcopy(v) for k, v in source.items() if k not in ("tracks", "observers")}
        search["tracks"] = [copy.deepcopy(tracks[fid]) for fid in seed["score"]["flight_ids"]]
        search["observers"] = observers
        search["pair_config"]["shared_shifts_s"] = np.arange(-10, 10.001, .25).tolist()
        search["pair_config"]["model"]["sound_speed_mps"] = 343
        search["provenance"].append("Explicit timing-window sensitivity: shared shift -10 to +10s at0.25s; event2 remains locked; original heights and343m/s retained; does not replace primary baseline")
        result, receipt = run_rank(exe, search, args.out_dir / f"pair-{index+1}", 300)
        winner = best_row(result)
        record = {"receipt": receipt, "best": winner}
        if winner:
            record["best_at_spatial_grid_boundary"] = on_grid_boundary(winner["observer"]["id"], boundary)
            winners.append((winner["score"]["combined_rmse_hz"], winner, search))
        report["pair_runs"].append(record)
    if not winners:
        report["status"] = "no_candidate"
    else:
        _, local, search = min(winners, key=lambda entry: entry[0])
        observers, boundary = grid_around(local["observer"], 150, 25, dem, geoid, 3)
        search["observers"] = observers
        result, receipt = run_rank(exe, search, args.out_dir / "fine", 300)
        winner = best_row(result)
        report["fine"] = {"receipt": receipt, "best": winner}
        if winner:
            report["fine"].update(best_at_spatial_grid_boundary=on_grid_boundary(winner["observer"]["id"], boundary),
                                   best_at_timing_boundary=abs(winner["score"]["shared_shift_s"]) == 10,
                                   distance_from_baseline_m=distance_m(baseline["observer"], winner["observer"]),
                                   same_pair_as_baseline=winner["score"]["flight_ids"] == baseline["score"]["flight_ids"])
    # Open evaluation-only reference after all rank subprocesses have finished.
    if args.evaluation_reference:
        reference = load_json(args.evaluation_reference)
        report["evaluation"] = {"reference": reference, "sha256": file_hash(args.evaluation_reference), "read_after_all_searches": True}
        if report.get("fine", {}).get("best"):
            report["fine"]["distance_from_video_coordinate_m"] = distance_m(report["fine"]["best"]["observer"], reference)
    write_json(args.out_dir / "summary.json", report)
    print(json.dumps({"fine": report.get("fine"), "output": str(args.out_dir / "summary.json")}, indent=2))


if __name__ == "__main__":
    main()
