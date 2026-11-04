"""Compare a completed, hash-verified regional result to separate publisher truth."""
import argparse
import csv
from datetime import datetime, timezone
import json
import math
from pathlib import Path

from field_validation_native import digest


def distance(a, b):
    lat1, lon1, lat2, lon2 = map(math.radians, [*a, *b])
    chord = math.sin((lat2-lat1)/2)**2 + math.cos(lat1)*math.cos(lat2)*math.sin((lon2-lon1)/2)**2
    return 6371008.8 * 2 * math.asin(min(1.0, math.sqrt(chord)))


def evaluate(prepared, searched, truth, output):
    prepared, searched, truth, output = map(Path, (prepared, searched, truth, output))
    if output.exists():
        raise ValueError("Use a new evaluation receipt")
    summary = json.loads((searched / "summary.json").read_text(encoding="utf-8"))
    if not summary["input_and_binary_unchanged"] or digest(prepared) != summary["input_sha256"]:
        raise ValueError("Native input integrity receipt does not match prepared request")
    batch = json.loads(prepared.read_text(encoding="utf-8"))
    with truth.open(encoding="utf-8-sig", newline="") as stream:
        rows = [row for row in csv.DictReader(stream) if row["ESID"] == "103"]
    if len(rows) != 1:
        raise ValueError("Publisher metadata must identify exactly one ESID103 deployment")
    coordinate = [float(rows[0][key]) for key in ("Latitude", "Longitude")]
    outcomes = []
    for case in summary["cases"]:
        if case["status"] != "completed":
            outcomes.append({"id": case["id"], "status": case["status"], "error": case.get("error")})
            continue
        path = searched / case["report"]
        if digest(path) != case["report_sha256"]:
            raise ValueError("Native regional report changed")
        report = json.loads(path.read_text(encoding="utf-8"))
        inputs = next(item for item in batch["cases"] if item["id"] == case["id"])
        candidates = []
        for hypothesis in report["hypotheses"]:
            point = hypothesis["observer"]
            candidates.append({"rank": hypothesis["rank"], "score": hypothesis["score"],
                               "coordinates": [point["latitude_deg"], point["longitude_deg"]],
                               "distance_to_publisher_m": distance(coordinate, [point["latitude_deg"], point["longitude_deg"]]),
                               "clock_offset_s": hypothesis["clock_offset_s"],
                               "source_ids": [item["source_id"] for item in hypothesis["assignments"]],
                               "emitted_frequency_hz": [item["emitted_frequency_hz"] for item in hypothesis["assignments"]]})
        top = candidates[0] if candidates else None
        nearest = min(candidates, key=lambda item: item["distance_to_publisher_m"], default=None)
        nearest_grid = min(distance(coordinate, [p["latitude_deg"], p["longitude_deg"]]) for p in inputs["observers"])
        by_radius = {}
        for radius in (100, 1000, 10000, 25000, 50000):
            eligible = [item for item in candidates if item["distance_to_publisher_m"] <= radius]
            by_radius[str(radius)] = {"retained_count": len(eligible), "best_retained_rank": min((x["rank"] for x in eligible), default=None)}
        outcomes.append({"id": case["id"], "status": "publisher_location_comparison_complete",
                         "top": top, "top_ten": candidates[:10], "nearest_retained": nearest,
                         "nearest_declared_grid_point_m": nearest_grid, "within_radius_m": by_radius,
                         "retained_hypotheses": len(candidates), "supported_hypotheses": report["supported_hypotheses"],
                         "retained_truncated": report["retained_truncated"], "estimate": report["estimate"],
                         "distinct_retained_aircraft": sorted({source.split("#")[0] for item in candidates for source in item["source_ids"]}),
                         "unavailable_alternative_evaluations": report["unavailable_count"],
                         "location_recovered_within_25km": bool(top and top["distance_to_publisher_m"] <= 25000)})
    result = {"schema_version": 1, "evaluated_utc": datetime.now(timezone.utc).isoformat(),
              "primary_truth_path": str(truth.resolve()), "primary_truth_sha256": digest(truth),
              "primary_truth_coordinates": coordinate, "primary_truth_error_bound_m": None,
              "sample_zero_error_bound_s": None, "truth_used_only_after_scoring": True,
              "prepared_input_sha256": digest(prepared), "native_summary_sha256": digest(searched / "summary.json"),
              "cases": outcomes, "surveyed_100m_accuracy_validated": False,
              "interpretation": "Distances are to the independently published deployment marker, whose microphone registration error is unquantified. A coarse grid and truncated shortlist cannot establish precise accuracy or calibrated confidence. Source identities remain hypotheses. No numerical-fit failure is relabeled a successful location recovery."}
    with output.open("x", encoding="utf-8") as stream:
        json.dump(result, stream, indent=2, allow_nan=False)
        stream.write("\n")
    print(json.dumps({"cases": [{k: item[k] for k in ("id", "top", "nearest_retained", "nearest_declared_grid_point_m", "location_recovered_within_25km")} for item in outcomes if "top" in item], "surveyed_100m_accuracy_validated": False}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared", required=True)
    parser.add_argument("--search", required=True)
    parser.add_argument("--truth", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    evaluate(args.prepared, args.search, args.truth, args.output)
