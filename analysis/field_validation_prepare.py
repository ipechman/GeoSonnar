"""Prepare a frozen regional experiment using native measurements and tracks.

This does not read receiver truth or fit parameters. Pending operator annotations
remain separate from the explicit all-proposal numerical experiment.
"""
import argparse
import copy
import json
import math
from pathlib import Path

from field_validation_native import digest, write


def axis(low, high, step):
    return [min(high, low + i * step) for i in range(math.ceil((high - low) / step) + 1)]


def experiment_workspace(document, notes, diagnostic_flag):
    """Explicit unknown-source experiment copy; never confirm the operator review.

    These historical candidate-location protocols deliberately try unidentified
    tones against aircraft alternatives. Eligibility is an experiment assumption,
    not acoustic-source identification, and must survive into native reports.
    """
    experiment = copy.deepcopy(document)
    experiment["notes"] = notes
    for event in experiment["events"]:
        if event["review"] != "pending" or event["selected_for_scoring"]:
            raise ValueError("Experiment preparation requires pending, unselected observations")
        if event.get("acoustic_source", "unassessed") != "unassessed":
            raise ValueError("Experiment preparation cannot replace a source assessment")
        event["review"] = "confirmed"
        event["selected_for_scoring"] = True
        event["acoustic_source"] = {"uncertain_hypothesis": {"rationale":
            "Experiment-only inclusion of an unidentified tonal source. Aircraft origin is "
            "unverified; every frozen observation is retained to test candidate locations. "
            "This is not human confirmation or independent aircraft identification."}}
        event["diagnostics"][diagnostic_flag] = True
    return experiment


def prepare(native_path, output, identifier, policy_path, search_policy_path):
    native, out = Path(native_path).resolve(), Path(output).resolve()
    out.mkdir(parents=True, exist_ok=False)
    policy = json.loads(Path(policy_path).read_text(encoding="utf-8"))
    search = json.loads(Path(search_policy_path).read_text(encoding="utf-8"))
    if digest(policy_path) != search["native_discovery_policy_sha256"]:
        raise ValueError("Frozen discovery policy changed")
    receipt = json.loads((native / "native-receipt.json").read_text(encoding="utf-8"))
    if receipt["policy_sha256"] != search["native_discovery_policy_sha256"]:
        raise ValueError("Native run used a different discovery policy")
    discovery_path = native / "discovery" / "discovery.json"
    discovery = json.loads(discovery_path.read_text(encoding="utf-8"))
    config = json.loads((native / "track-import-config.json").read_text(encoding="utf-8"))
    from datetime import datetime, timezone
    start_utc = datetime.fromtimestamp(config["time_origin_utc_s"], timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    datasets, options, tracks, native_passes = [], [], [], []
    for path in sorted((native / "normalized-traces").glob("*.json")):
        normalized = json.loads(path.read_text(encoding="utf-8"))
        checksum = digest(path)
        datasets.append({"path": str(path), "sha256": checksum})
        options.append({"source_sha256": checksum, "time_origin_utc_s": config["time_origin_utc_s"], "lightning_altitude_wgs84": False})
        for item in normalized["passes"]:
            physics = copy.deepcopy(item["track"])
            points = [{"latitude_deg": point["latitude_deg"], "longitude_deg": point["longitude_deg"],
                       "height_m": point["ellipsoid_height_m"], "time_s": physics["points"][index]["t_s"],
                       "time_utc_s": point["timestamp_utc_s"]} for index, point in enumerate(item["source_points"])]
            if len(points) != len(physics["points"]):
                raise ValueError("Native geographic/physics source counts differ")
            tracks.append({"id": physics["flight_id"], "points": points, "segments": [[0, len(points)]],
                           "time_origin_utc_s": config["time_origin_utc_s"], "max_gap_s": config["max_gap_s"],
                           "physics_track": physics})
            native_passes.append(item)
    source_ids = [track["id"] for track in tracks]
    if len(set(source_ids)) != len(source_ids):
        raise ValueError("Ambiguous native track identity")
    events = []
    for event in discovery["events"]:
        quality = event["quality"]
        events.append({"id": event["event_id"], "label": "Unreviewed native ridge " + event["event_id"],
                       "kind": "doppler", "origin": "automatic native discovery, unchanged measurements",
                       "review": "pending", "selected_for_scoring": False,
                       "start_s": event["start_s"], "end_s": event["end_s"],
                       "arrival_s": (event["start_s"] + event["end_s"]) / 2,
                       "points": [{key: point[key] for key in ("time_s", "frequency_hz")} for point in event["ridge"]],
                       "uncertainty_s": quality["time_uncertainty_s"], "frequency_sigma_hz": quality["frequency_sigma_hz"],
                       "flight_id": "", "emitted_frequency_hz": 0.0, "strike_id": "",
                       "source": {"path": str(discovery_path), "sha256": digest(discovery_path)},
                       "diagnostics": {"quality": quality, "human_review_completed": False,
                                       "aircraft_identity_confirmed": False, "arrival_is_not_a_CPA_measurement": True}})
    plan = {"schema_version": 1,
            "clock": {"min_offset_s": policy["clock_offset_bounds_s"][0], "max_offset_s": policy["clock_offset_bounds_s"][1], "step_s": search["clock_step_s"]},
            "events": [{"event_id": event["id"], "source_ids": source_ids, "emitted_frequency_hz": search["emitted_frequency_bounds_hz"]} for event in events],
            "max_evaluations": search["max_evaluations"], "retain_hypotheses": search["retained_hypotheses"]}
    document = {"schema_version": 1, "case_id": identifier, "recording_sha256": discovery["source_sha256"],
                "channel": 0, "duration_s": discovery["source_metadata"]["duration_s"], "recording_start_utc": start_utc,
                "sound_speed_mps": policy["sound_speed_mps"], "events": events,
                "notes": "Pending operator annotations; no coordinate truth read; no aircraft association supplied.",
                "mix": {"doppler_frequency": True, "doppler_timing": False, "thunder_timing": False,
                        "frequency_weight": 1.0, "doppler_timing_weight": 1.0, "thunder_weight": 1.0,
                        "frequency_scale_hz": 1.0, "doppler_timing_scale_s": 1.0, "thunder_scale_s": 1.0},
                "map": {"bounds": policy["search_bounds_south_west_north_east"], "grid_spacing_m": search["grid_spacing_m"],
                        "receiver_height_m": policy["receiver_height_ellipsoid_m"], "clock_adjustment_s": 0.0},
                "datasets": datasets, "dataset_options": options, "regional_plan": plan}
    write(out / "pending-workspace.json", document)
    experiment = experiment_workspace(document, search["review_semantics"], "benchmark_only_inclusion")
    south, west, north, east = policy["search_bounds_south_west_north_east"]
    lats = axis(south, north, search["grid_spacing_m"] / 111320.0)
    lons = axis(west, east, search["grid_spacing_m"] / (111320.0 * math.cos(math.radians((south + north) / 2))))
    observers = [{"latitude_deg": lat, "longitude_deg": lon, "ellipsoid_height_m": policy["receiver_height_ellipsoid_m"]} for lat in lats for lon in lons]
    merged = out / "native-passes.json"
    write(merged, {"passes": native_passes, "coordinates_transformed_after_native_import": False})
    map_data = {"source_path": str(merged), "source_sha256": digest(merged), "time_origin_utc_s": config["time_origin_utc_s"],
                "tracks": tracks, "candidates": [], "observers": [], "grid": None, "terrain": None,
                "lightning": [], "notes": ["Actual ADSB.lol observations; native geometric-only normalization; incomplete sky coverage; no absence penalty"], "location_verified": False}
    batch = {"schema_version": 1, "cases": [{"id": identifier, "document": experiment, "map": map_data, "lightning": None, "observers": observers, "plan": plan}]}
    result = {"schema_version": 1, "event_count": len(events), "point_count": sum(len(e["points"]) for e in events),
              "source_passes": len(tracks), "observers": len(observers), "clock_samples": len(axis(*policy["clock_offset_bounds_s"], search["clock_step_s"])),
              "receiver_truth_read": False, "geolocation_accuracy_validated": False,
              "native_receipt_sha256": digest(native / "native-receipt.json"), "policy_sha256": digest(policy_path),
              "search_policy_sha256": digest(search_policy_path), "no_event_abstention": not bool(events)}
    if events:
        write(out / "batch.json", batch)
        result["batch_sha256"] = digest(out / "batch.json")
    write(out / "preparation.json", result)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--id", required=True)
    parser.add_argument("--policy", default="benchmarks/field_validation_policy.json")
    parser.add_argument("--search-policy", default="benchmarks/field_validation_search_policy.json")
    args = parser.parse_args()
    prepare(args.native, args.output, args.id, args.policy, args.search_policy)
