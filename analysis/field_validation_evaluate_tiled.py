"""Verify every completed native tile before opening separate publisher truth."""
import argparse
import csv
from datetime import datetime, timezone
import json
from pathlib import Path

from field_validation_evaluate import distance
from field_validation_native import digest


def evaluate(prepared, searched, truth, output):
    prepared, searched, truth, output = map(Path, (prepared, searched, truth, output))
    if output.exists():
        raise ValueError("Use a new post-score receipt")
    tiling = json.loads((prepared / "tiling.json").read_text())
    merged = json.loads((searched / "merged.json").read_text())
    inputs = json.loads((prepared / "grouped-full-request.json").read_text())["cases"][0]
    if merged["status"] != "all_native_tiles_completed" or merged["tiling_sha256"] != digest(prepared / "tiling.json") or merged["grouped_input_sha256"] != digest(prepared / "grouped-full-request.json"):
        raise ValueError("Tiled prediction provenance changed")
    if len(merged["tiles"]) != len(tiling["tiles"]):
        raise ValueError("Missing tile results")
    native_rows = {}
    receiver_inventory = []
    for expected, actual in zip(tiling["tiles"], merged["tiles"]):
        source = prepared / expected["input"]
        if digest(source) != actual["input_sha256"] or actual["input_sha256"] != expected["sha256"] or digest(actual["report"]) != actual["report_sha256"]:
            raise ValueError("Native tile input/report integrity mismatch")
        native = json.loads(Path(actual["report"]).read_text())
        if native["status"] != "completed" or native["progress"]["completed_cells"] != native["progress"]["total_cells"]:
            raise ValueError("Partial native tile cannot enter evaluation")
        receiver_inventory.extend(json.loads(source.read_text())["cases"][0]["observers"])
        for row in native["hypotheses"]:
            native_rows[(actual["index"], row["rank"])] = row
    if receiver_inventory != inputs["observers"]:
        raise ValueError("Receiver inventory missing, duplicated, or reordered")
    for row in merged["hypotheses"]:
        native = native_rows[(row["tile"], row["native_rank"])]
        if any(row[key] != native[key] for key in ("score", "observer", "clock_offset_s", "assignments")):
            raise ValueError("Merged prediction differs from the original native result")
    if [row["score"] for row in merged["hypotheses"]] != sorted(row["score"] for row in native_rows.values())[:merged["retained_limit"]]:
        raise ValueError("Merge is not the lowest-score global retained set")
    # First access to publisher coordinates occurs after completed outputs verify.
    with truth.open(encoding="utf-8-sig", newline="") as stream:
        metadata = [row for row in csv.DictReader(stream) if row["ESID"] == "103"]
    if len(metadata) != 1:
        raise ValueError("Expected one ESID103 publisher record")
    coordinate = [float(metadata[0][key]) for key in ("Latitude", "Longitude")]
    candidates = []
    for row in merged["hypotheses"]:
        point = row["observer"]
        candidates.append({"rank": row["rank"], "score": row["score"], "coordinates": [point["latitude_deg"], point["longitude_deg"]],
                           "distance_to_publisher_m": distance(coordinate, [point["latitude_deg"], point["longitude_deg"]]),
                           "clock_offset_s": row["clock_offset_s"],
                           "source_ids": [assignment["source_id"] for assignment in row["assignments"]]})
    top = candidates[0] if candidates else None
    nearest = min(candidates, key=lambda row: row["distance_to_publisher_m"], default=None)
    result = {"schema_version": "geosonnar.field_tiled_evaluation.v1", "evaluated_utc": datetime.now(timezone.utc).isoformat(),
              "merged_predictions_sha256": digest(searched / "merged.json"), "tiling_sha256": digest(prepared / "tiling.json"),
              "truth_path": str(truth.resolve()), "truth_sha256": digest(truth), "primary_truth_coordinates": coordinate,
              "truth_used_only_after_scoring": True, "primary_truth_error_bound_m": None, "sample_zero_error_bound_s": None,
              "top": top, "top_ten": candidates[:10], "nearest_retained": nearest,
              "nearest_declared_grid_point_m": min(distance(coordinate, [p["latitude_deg"], p["longitude_deg"]]) for p in inputs["observers"]),
              "retained_count": len(candidates), "retained_unique_receivers": len({tuple(row["coordinates"]) for row in candidates}),
              "within_radius_m": {str(radius): {"retained_count": sum(row["distance_to_publisher_m"] <= radius for row in candidates), "best_retained_rank": min((row["rank"] for row in candidates if row["distance_to_publisher_m"] <= radius), default=None)} for radius in (100, 1000, 10000, 25000, 50000)},
              "tiles_completed": len(merged["tiles"]), "total_evaluations": sum(t["estimate"]["evaluations"] for t in merged["tiles"]),
              "total_residual_evaluations": sum(t["estimate"]["residual_evaluations"] for t in merged["tiles"]),
              "unavailable_alternative_evaluations": sum(t["unavailable_count"] for t in merged["tiles"]),
              "supported_hypothesis_count_overflowed": any(t["supported_hypotheses"] is None for t in merged["tiles"]),
              "location_recovered_within_25km": bool(top and top["distance_to_publisher_m"] <= 25000),
              "surveyed_100m_accuracy_validated": False,
              "interpretation": "Within-site time holdout, using unchanged automatic proposals as an explicitly unreviewed experimental baseline. Native segments and the frozen grid are preserved through research-only scheduling. Automatic proposals may be non-aircraft or correlated harmonics. Truncated assignments are not calibrated location probabilities; unquantified publisher marker and recorder-clock errors remain."}
    with output.open("x", encoding="utf-8") as stream:
        json.dump(result, stream, indent=2, allow_nan=False)
        stream.write("\n")
    print(json.dumps({key: result[key] for key in ("top", "nearest_retained", "retained_unique_receivers", "tiles_completed", "total_evaluations", "location_recovered_within_25km")}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared", required=True)
    parser.add_argument("--search", required=True)
    parser.add_argument("--truth", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    evaluate(args.prepared, args.search, args.truth, args.output)
