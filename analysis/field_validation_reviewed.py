"""Bind a frozen audio-only review to every eligible native aircraft pass.

No receiver truth, aircraft-type selection, ICAO regrouping, or fitted support
filter is used. Temporal eligibility is exactly the declared look-back rule.
"""
import argparse
import copy
from datetime import datetime, timezone
import json
import math
from pathlib import Path
import subprocess
import time

from field_validation_native import digest, write
from field_validation_prepare import axis, experiment_workspace

FROZEN = {
    "protocol": "e06bfef70129f803488c12108d2551055b789c580a116b93576da0ce4250c3bc",
    "review": "aaf1fb919fff482a5e8c4956a754eb995033f948406ce38fdefcda91bde96e37",
    "config": "b5999ce013f1531b6cc1ff185646c5c1fe8856f48c7f5a5e08193d99e1367884",
    "import_executable": "cef773b17145e4547c5c9f74844f40f79fa81cd48d6994cfe2c232d129c5c010",
}


def prepare(args):
    paths = {key: Path(getattr(args, key)).resolve() for key in ("protocol", "review", "config", "import_executable")}
    for key, expected in FROZEN.items():
        if digest(paths[key]) != expected:
            raise ValueError("Frozen " + key + " binding mismatch")
    protocol = json.loads(paths["protocol"].read_text())
    review = json.loads(paths["review"].read_text())
    if protocol["review_sha256"] != digest(paths["review"]) or protocol["review_config_sha256"] != digest(paths["config"]) or review["configuration_sha256"] != digest(paths["config"]):
        raise ValueError("Protocol/review/configuration bindings differ")
    waveform = Path(review["source"]["path"])
    if digest(waveform) != protocol["waveform_sha256"] or review["source"]["sha256"] != protocol["waveform_sha256"]:
        raise ValueError("Original reviewed waveform changed")
    points = [{key: point[key] for key in ("time_s", "frequency_hz")} for point in review["measurement"]["points"]]
    if len(points) != protocol["event"]["measurement_count"] or len(points) < 2:
        raise ValueError("Frozen review measurement count changed")
    tracks_root = Path(args.tracks).resolve()
    tracks_receipt = json.loads((tracks_root / "acquisition.json").read_text())
    settings = protocol["search"]
    if tracks_receipt["aircraft_coverage_bounds"] != settings["bounds_south_west_north_east"] or tracks_receipt["trace_cap_reached"]:
        raise ValueError("Acquired source rectangle differs or sampled-index acquisition is still capped")
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output / "normalized-traces").mkdir()
    for key in ("protocol", "review", "config"):
        (output / ("frozen-"+key+".json")).write_bytes(paths[key].read_bytes())
    def stamp(text): return datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp()
    origin = stamp(protocol["sample_zero_utc_claim"])
    import_config = {"source_label": "Actual ADSB.lol sampled regional history; geometric-only native import; incomplete sky coverage",
                     "time_origin_utc_s": origin,
                     "time_window_utc_s": [stamp(tracks_receipt[key]) for key in ("utc_start", "utc_end")],
                     "bounds": settings["bounds_south_west_north_east"], "max_gap_s": 30.0, "max_speed_mps": 330.0,
                     "accepted_sources": ["adsb_icao", "adsr_icao"], "altitude_policy": {"mode": "geometric_only"}}
    config_path = output / "native-import-config.json"
    write(config_path, import_config)
    acquisition_sha = digest(tracks_root / "acquisition.json")
    commands = []
    for index, source in enumerate(tracks_receipt["traces"]):
        source_path = tracks_root / source["path"]
        if digest(source_path) != source["sha256"]:
            raise ValueError("Original acquired aircraft trace changed")
        destination = output / "normalized-traces" / (source["icao24"]+".json")
        command = [str(paths["import_executable"]), "import-adsblol-trace", "--input", str(source_path), "--config", str(config_path), "--out", str(destination)]
        started = time.monotonic()
        result = subprocess.run(command, capture_output=True, text=True, timeout=120,
                                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        commands.append({"argv": command, "exit_code": result.returncode, "elapsed_s": time.monotonic()-started,
                         "stdout": result.stdout, "stderr": result.stderr, "input_sha256": source["sha256"],
                         "output_sha256": digest(destination) if destination.exists() else None})
        write(output / "native-import-receipt.json", {"schema_version": 1, "import_executable_sha256": FROZEN["import_executable"],
              "track_acquisition_sha256": acquisition_sha, "commands": commands, "receiver_truth_read": False})
        if result.returncode:
            raise RuntimeError("Native trace import failed; full receipt retained")
        if index % 20 == 0: print(f"Imported{index+1}/{len(tracks_receipt['traces'])} original traces", flush=True)
    for key, expected in FROZEN.items():
        if digest(paths[key]) != expected:
            raise ValueError("Frozen " + key + " changed during import")
    first, last = min(point["time_s"] for point in points), max(point["time_s"] for point in points)
    support_window = [first+settings["clock_min_offset_s"]-600.0, last+settings["clock_max_offset_s"]]
    included, excluded, tracks, all_passes, datasets, options = [], [], [], [], [], []
    raw_stats = {}
    for path in sorted((output / "normalized-traces").glob("*.json")):
        normalized = json.loads(path.read_text())
        for key, count in normalized["stats"].items(): raw_stats[key] = raw_stats.get(key, 0)+count
        checksum = digest(path)
        datasets.append({"path": str(path), "sha256": checksum})
        options.append({"source_sha256": checksum, "time_origin_utc_s": origin, "lightning_altitude_wgs84": False})
        for item in normalized["passes"]:
            physics = copy.deepcopy(item["track"])
            all_passes.append(item)
            times = [point["t_s"] for point in physics["points"]]
            detail = {"id": physics["flight_id"], "timestamp_support_s": [min(times), max(times)], "point_count": len(times)}
            if max(times) < support_window[0] or min(times) > support_window[1]:
                excluded.append({**detail, "reason": "No timestamp-support overlap with frozen event/clock/600second look-back window"})
                continue
            included.append(detail)
            geo = [{"latitude_deg": source["latitude_deg"], "longitude_deg": source["longitude_deg"],
                    "height_m": source["ellipsoid_height_m"], "time_s": point["t_s"], "time_utc_s": source["timestamp_utc_s"]}
                   for source, point in zip(item["source_points"], physics["points"])]
            if len(geo) != len(physics["points"]): raise ValueError("Native source/physics inventory differs")
            tracks.append({"id": physics["flight_id"], "points": geo, "segments": [[0, len(geo)]],
                           "time_origin_utc_s": origin, "max_gap_s": 30.0, "physics_track": physics})
    source_ids = [track["id"] for track in tracks]
    if len(source_ids) != len(set(source_ids)): raise ValueError("Duplicate native pass identity")
    plan = {"schema_version": 1, "clock": {"min_offset_s": settings["clock_min_offset_s"], "max_offset_s": settings["clock_max_offset_s"], "step_s": settings["clock_step_s"]},
            "events": [{"event_id": protocol["event"]["id"], "source_ids": source_ids, "emitted_frequency_hz": settings["emitted_frequency_bounds_hz"]}],
            "max_evaluations": settings["max_evaluations"], "retain_hypotheses": settings["retain_hypotheses"]}
    event = {"id": protocol["event"]["id"], "label": "Audio-only reviewed unidentified tonal source", "kind": "doppler",
             "origin": "Assistant spectral review frozen before aircraft IDs or exact location", "review": "pending", "selected_for_scoring": False,
             "start_s": first-protocol["event"]["time_uncertainty_s"], "end_s": last+protocol["event"]["time_uncertainty_s"],
             "arrival_s": (first+last)/2, "points": points, "uncertainty_s": protocol["event"]["time_uncertainty_s"],
             "frequency_sigma_hz": protocol["event"]["frequency_sigma_hz"], "flight_id": "", "emitted_frequency_hz": 0.0,
             "strike_id": "", "source": {"path": str(paths["review"]), "sha256": digest(paths["review"])},
             "diagnostics": {"aircraft_identity_confirmed": False, "human_review_completed": False,
                             "review_protocol_sha256": digest(paths["protocol"]), "arrival_is_not_a_CPA_measurement": True}}
    document = {"schema_version": 1, "case_id": protocol["case_id"], "recording_sha256": protocol["waveform_sha256"],
                "channel": review["source"]["channel"], "duration_s": review["source"]["duration_s"],
                "recording_start_utc": protocol["sample_zero_utc_claim"], "sound_speed_mps": settings["sound_speed_mps"],
                "events": [event], "notes": "Pending operator annotation; assistant audio-only interpretation is not source identification or a human usability trial.",
                "mix": {"doppler_frequency": True, "doppler_timing": False, "thunder_timing": False,
                        "frequency_weight": 1.0, "doppler_timing_weight": 1.0, "thunder_weight": 1.0,
                        "frequency_scale_hz": settings["frequency_residual_scale_hz"], "doppler_timing_scale_s": 1.0, "thunder_scale_s": 1.0},
                "map": {"bounds": settings["bounds_south_west_north_east"], "grid_spacing_m": settings["grid_spacing_m"],
                        "receiver_height_m": settings["receiver_ellipsoid_height_m"], "clock_adjustment_s": 0.0},
                "datasets": datasets, "dataset_options": options, "regional_plan": plan}
    write(output / "pending-workspace.json", document)
    experiment = experiment_workspace(document,
        "Experiment-only event eligibility; all40 frozen measurements and all time-eligible native passes retained. No ICAO regrouping or aircraft identity confirmation. Unknown tone source; no location accepted.",
        "experiment_only_inclusion")
    south, west, north, east = settings["bounds_south_west_north_east"]
    lats = axis(south, north, settings["grid_spacing_m"]/111320)
    lons = axis(west, east, settings["grid_spacing_m"]/(111320*math.cos(math.radians((south+north)/2))))
    observers = [{"latitude_deg": lat, "longitude_deg": lon, "ellipsoid_height_m": settings["receiver_ellipsoid_height_m"]} for lat in lats for lon in lons]
    source_path = output / "all-native-passes.json"
    write(source_path, {"passes": all_passes})
    map_data = {"source_path": str(source_path), "source_sha256": digest(source_path), "time_origin_utc_s": origin,
                "tracks": tracks, "candidates": [], "observers": [], "grid": None, "terrain": None, "lightning": [],
                "notes": ["Every original native pass is independent; no ICAO regrouping, point editing or gap bridging", "Timestamp-only eligibility does not imply complete aircraft coverage"], "location_verified": False}
    batch = {"schema_version": 1, "cases": [{"id": "fresh-B-reviewed", "document": experiment, "map": map_data, "lightning": None, "observers": observers, "plan": plan}]}
    write(output / "batch.json", batch)
    clocks = len(axis(settings["clock_min_offset_s"], settings["clock_max_offset_s"], settings["clock_step_s"]))
    estimate = {"observers": len(observers), "clock_samples": clocks, "source_passes": len(tracks), "measured_points": len(points),
                "evaluations": len(observers)*clocks*len(tracks), "residual_evaluations": len(observers)*clocks*len(tracks)*len(points),
                "trajectory_point_visits": len(observers)*clocks*sum(len(track["physics_track"]["points"]) for track in tracks)}
    write(output / "preparation.json", {"schema_version": 1, "created_utc": datetime.now(timezone.utc).isoformat(),
          "frozen_bindings": FROZEN, "batch_sha256": digest(output / "batch.json"), "track_acquisition_sha256": acquisition_sha,
          "source_support_window_relative_s": support_window, "source_eligibility_basis": "Original timestamp extent only; inclusive overlap with actual frozen measurements plusclockrange and600secondlook-back",
          "included_passes": included, "excluded_passes": excluded, "native_import_stats": raw_stats, "estimated_work": estimate,
          "source_grouping": False, "receiver_truth_read": False, "unknown_tone_identity": True, "location_accepted": False})
    print(json.dumps({"batch": str(output / "batch.json"), "batch_sha256": digest(output / "batch.json"), "native_passes": len(all_passes),
                      "included_passes": len(included), "excluded_passes": len(excluded), "estimated_work": estimate, "receiver_truth_read": False}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--protocol", default="benchmarks/reviewed_field_protocol.json")
    parser.add_argument("--review", default="output/audio-review-fresh-B-003/review.json")
    parser.add_argument("--config", default="output/benchmarks/regional-workflow-001/fresh-B-trace-config.json")
    parser.add_argument("--tracks", default="data/validation/field-newsite-B-tracks-002")
    parser.add_argument("--import-executable", default="output/field-validation-reference-001/geosonnar-discovery-import.exe")
    parser.add_argument("--output", required=True)
    prepare(parser.parse_args())
