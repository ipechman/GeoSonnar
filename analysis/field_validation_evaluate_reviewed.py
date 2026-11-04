"""Commit verified predictions, then compare distinct locations to separate truth."""
import argparse
import csv
from datetime import datetime, timezone
import json
import math
from pathlib import Path

from field_validation_evaluate import distance
from field_validation_native import digest
from field_validation_reviewed import FROZEN


def write_new(path, value):
    with Path(path).open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, allow_nan=False)
        stream.write("\n")


def evaluate(prepared, searched, truth):
    prepared, searched, truth = map(Path, (prepared, searched, truth))
    preparation = json.loads((prepared / "preparation.json").read_text())
    protocol = json.loads((prepared / "frozen-protocol.json").read_text())
    summary = json.loads((searched / "summary.json").read_text())
    batch = json.loads((prepared / "batch.json").read_text())["cases"][0]
    for key in ("protocol", "review", "config"):
        if digest(prepared / ("frozen-"+key+".json")) != FROZEN[key]:
            raise ValueError("Frozen review binding changed before evaluation")
    if digest(prepared / "batch.json") != preparation["batch_sha256"] or summary["input_sha256"] != preparation["batch_sha256"] or not summary["input_and_binary_unchanged"]:
        raise ValueError("Native search input differs from frozen request")
    if summary["executable_sha256"] != "e4de3107ee3321fea28ff45f2e69d700db576c099033dcd033da597de964718d":
        raise ValueError("Native regional adapter differs")
    outcome = summary["cases"][0]
    if outcome["status"] != "completed":
        raise ValueError("Native request did not complete; preserve blocked outcome without truth-driven repair")
    report_path = searched / outcome["report"]
    if digest(report_path) != outcome["report_sha256"]:
        raise ValueError("Committed native report changed")
    report = json.loads(report_path.read_text())
    if report["status"] != "completed" or report["progress"]["completed_cells"] != report["progress"]["total_cells"]:
        raise ValueError("Regional search is incomplete")
    locations = report["location_summaries"]
    if len(locations) != len(batch["observers"]):
        raise ValueError("Incomplete distinct receiver summaries")
    for index, row in enumerate(locations):
        if row["observer_index"] != index or row["observer"] != batch["observers"][index] or row["completed_clocks"] != report["estimate"]["clock_count"]:
            raise ValueError("Receiver or clock coverage differs from original request")
    commit = {"schema_version": 1, "committed_before_truth_read_utc": datetime.now(timezone.utc).isoformat(),
              "protocol_sha256": FROZEN["protocol"], "review_sha256": FROZEN["review"],
              "batch_sha256": preparation["batch_sha256"], "report_sha256": digest(report_path),
              "native_summary_sha256": digest(searched / "summary.json"),
              "receiver_count": len(locations), "all_declared_clocks_completed": True,
              "receiver_truth_read_at_commit": False, "location_accepted": False}
    write_new(searched / "prediction-commit.json", commit)
    # Exact deployment metadata is first opened after the predictions commit.
    with truth.open(encoding="utf-8-sig", newline="") as stream:
        source_rows = [row for row in csv.DictReader(stream) if row["ESID"] == "108"]
    if len(source_rows) != 1:
        raise ValueError("Expected one independent publisher deployment row")
    coordinate = [float(source_rows[0][key]) for key in ("Latitude", "Longitude")]
    south, west, north, east = protocol["search"]["bounds_south_west_north_east"]
    latitudes = sorted({row["observer"]["latitude_deg"] for row in locations})
    longitudes = sorted({row["observer"]["longitude_deg"] for row in locations})
    def extents(value, ordered, lower, upper):
        i = ordered.index(value)
        return (lower if i == 0 else (ordered[i-1]+value)/2, upper if i+1 == len(ordered) else (value+ordered[i+1])/2)
    def area(row):
        p = row["observer"]
        lo_lat, hi_lat = extents(p["latitude_deg"], latitudes, south, north)
        lo_lon, hi_lon = extents(p["longitude_deg"], longitudes, west, east)
        return 6371.0088**2 * math.radians(hi_lon-lo_lon) * (math.sin(math.radians(hi_lat))-math.sin(math.radians(lo_lat)))
    ranked = sorted((row for row in locations if row["best_score"] is not None), key=lambda row: (row["best_score"], row["observer_index"]))
    entries = []
    for rank, row in enumerate(ranked, 1):
        p = row["observer"]
        entries.append({"rank": rank, "observer_index": row["observer_index"], "coordinates": [p["latitude_deg"], p["longitude_deg"]],
                        "distance_to_publisher_m": distance(coordinate, [p["latitude_deg"], p["longitude_deg"]]),
                        "score": row["best_score"], "clock_offset_s": row["best_clock_offset_s"],
                        "source_assignments": row["best_assignments"], "spatial_at_boundary": row["spatial_at_boundary"],
                        "clock_at_boundary": row["best_clock_offset_s"] in [protocol["search"]["clock_min_offset_s"], protocol["search"]["clock_max_offset_s"]],
                        "rankable_clocks": row["rankable_clocks"], "unavailable_clocks": row["unavailable_clocks"],
                        "clipped_grid_cell_area_km2": area(row)})
    nearest = min(locations, key=lambda row: distance(coordinate, [row["observer"]["latitude_deg"], row["observer"]["longitude_deg"]]))
    nearest_rank = next((row for row in entries if row["observer_index"] == nearest["observer_index"]), None)
    tolerance = protocol["evaluation"]["primary_top_location_tolerance_m"]
    top = entries[0] if entries else None
    recalls = {str(count): {"within_declared_tolerance": any(row["distance_to_publisher_m"] <= tolerance for row in entries[:count]),
                           "nearest_error_m": min((row["distance_to_publisher_m"] for row in entries[:count]), default=None),
                           "clipped_grid_cell_area_km2": sum(row["clipped_grid_cell_area_km2"] for row in entries[:count])} for count in (1, 10, 20)}
    result = {"schema_version": 1, "evaluated_utc": datetime.now(timezone.utc).isoformat(),
              "prediction_commit_sha256": digest(searched / "prediction-commit.json"),
              "truth_path": str(truth.resolve()), "truth_sha256": digest(truth), "publisher_coordinates": coordinate,
              "publisher_position_error_bound_m": None, "sample_zero_error_bound_s": None,
              "receiver_truth_used_only_after_committed_predictions": True, "source_identity_unknown": True,
              "protocol_sha256": FROZEN["protocol"], "review_sha256": FROZEN["review"],
              "top_distinct_location": top, "top_20_distinct_locations": entries[:20],
              "all_distinct_locations": entries, "ranked_receiver_count": len(entries), "declared_receiver_count": len(locations),
              "top_k_recall": recalls, "declared_location_tolerance_m": tolerance,
              "top_location_within_declared_tolerance": bool(top and top["distance_to_publisher_m"] <= tolerance),
              "nearest_declared_grid_point": {"observer_index": nearest["observer_index"], "observer": nearest["observer"],
                  "distance_to_publisher_m": distance(coordinate, [nearest["observer"]["latitude_deg"], nearest["observer"]["longitude_deg"]]),
                  "distinct_location_rank": nearest_rank["rank"] if nearest_rank else None,
                  "best_score": nearest["best_score"], "rankable_clocks": nearest["rankable_clocks"]},
              "area_summary": {"method": "Spherical area of midpoint grid cells clipped to the declared rectangle; descriptive support, not probability",
                  "declared_rectangle_km2": sum(area(row) for row in locations), "rankable_cell_area_km2": sum(row["clipped_grid_cell_area_km2"] for row in entries),
                  "within_best_plus_one_score_area_km2": sum(row["clipped_grid_cell_area_km2"] for row in entries if top and row["score"] <= top["score"]+1),
                  "score_plus_one_is_not_a_confidence_region": True},
              "native_runtime_s": outcome["elapsed_s"], "native_work": report["estimate"],
              "native_completed_work": report["progress"], "unavailable_source_evaluations": report["unavailable_count"],
              "rankable_receiver_clock_cells": report["rankable_cells"], "unavailable_receiver_clock_cells": report["unavailable_cells"],
              "supported_hypotheses": report["supported_hypotheses"], "retained_hypotheses": len(report["hypotheses"]),
              "retained_truncated": report["retained_truncated"], "ranked_spatial_boundary_receivers": sum(row["spatial_at_boundary"] for row in entries),
              "best_clock_boundary_receivers": sum(row["clock_at_boundary"] for row in entries),
              "location_accepted": False, "surveyed_100m_accuracy_validated": False,
              "interpretation": "Fresh deployment, audio-only assistant review frozen before aircraft identities and exact publisher coordinates. Descriptive regional trial of one unidentified tone; incomplete sky coverage, unknown recorder/position error and assumed receiver height/sound speed remain. No trace, source list, clock or region was tuned after seeing truth; no acoustic-source identification or calibrated confidence is established."}
    write_new(searched / "publisher-evaluation.json", result)
    print(json.dumps({key: result[key] for key in ("publisher_coordinates", "top_distinct_location", "top_k_recall", "nearest_declared_grid_point", "native_runtime_s", "top_location_within_declared_tolerance", "location_accepted")}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared", required=True)
    parser.add_argument("--search", required=True)
    parser.add_argument("--truth", required=True)
    args = parser.parse_args()
    evaluate(args.prepared, args.search, args.truth)
