"""Lossless native-pass grouping and tiling of one frozen regional request.

Every receiver, clock, measured point and aircraft source segment is retained.
Grouping names disjoint passes by ICAO; original segment boundaries remain.
Grouping changes association scope: an event can use independently supported
samples from multiple passes of one aircraft. The old pass-level alternatives
required one pass to support the whole event. Only tiling is scheduling-only;
the grouped and pass-level hypothesis spaces are not claimed identical.
"""
import argparse
import copy
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import subprocess

from field_validation_native import digest, write


def group_tracks(tracks):
    groups, inventory = {}, []
    for track in tracks:
        identity = track["id"].split("#")[0]
        if identity not in groups:
            groups[identity] = {**copy.deepcopy(track), "id": identity, "points": [], "segments": [],
                                "physics_track": {"flight_id": identity, "points": []}}
        group = groups[identity]
        if (group["time_origin_utc_s"], group["max_gap_s"]) != (track["time_origin_utc_s"], track["max_gap_s"]):
            raise ValueError("Same ICAO has incompatible time/gap interpretation")
        offset = len(group["physics_track"]["points"])
        group["points"].extend(copy.deepcopy(track["points"]))
        group["physics_track"]["points"].extend(copy.deepcopy(track["physics_track"]["points"]))
        translated = [[a + offset, b + offset] for a, b in track["segments"]]
        group["segments"].extend(translated)
        if group["physics_track"]["points"][offset:] != track["physics_track"]["points"]:
            raise ValueError("Native physics vector changed during grouping")
        inventory.append({"original_pass_id": track["id"], "grouped_source_id": identity,
                          "grouped_point_range": [offset, len(group["physics_track"]["points"])],
                          "original_segments": track["segments"], "grouped_segments": translated})
    return [groups[key] for key in sorted(groups)], inventory


def prepare(input_path, output, tile_size):
    original = json.loads(Path(input_path).read_text(encoding="utf-8"))
    if len(original["cases"]) != 1 or not 1 <= tile_size <= 16:
        raise ValueError("Use one frozen case and1..16receivers per tile")
    out = Path(output).resolve()
    out.mkdir(parents=True, exist_ok=False)
    original_case = original["cases"][0]
    case = copy.deepcopy(original_case)
    tracks, inventory = group_tracks(case["map"]["tracks"])
    case["map"]["tracks"] = tracks
    identifiers = [track["id"] for track in tracks]
    for plan in (case["plan"], case["document"]["regional_plan"]):
        for event in plan["events"]:
            event["source_ids"] = identifiers
    case["map"]["notes"].append("All native passes grouped by ICAO with exact original segment boundaries; no measurements interpolated or dropped")
    write(out / "grouped-full-request.json", {"schema_version": 1, "cases": [case]})
    tiles = []
    for start in range(0, len(case["observers"]), tile_size):
        tile = copy.deepcopy(case)
        tile["id"] = f"{case['id']}-tile-{len(tiles):03}"
        tile["observers"] = case["observers"][start:start+tile_size]
        name = f"input-{len(tiles):03}.json"
        write(out / name, {"schema_version": 1, "cases": [tile]})
        tiles.append({"input": name, "sha256": digest(out / name), "case_id": tile["id"],
                      "observer_range": [start, start + len(tile["observers"])]})
    reconstructed = []
    for tile in tiles:
        reconstructed.extend(json.loads((out / tile["input"]).read_text())["cases"][0]["observers"])
    if reconstructed != original_case["observers"]:
        raise ValueError("Tiled receiver inventory changed")
    receipt = {"schema_version": "geosonnar.field_tiles.v1", "created_utc": datetime.now(timezone.utc).isoformat(),
               "original_input_path": str(Path(input_path).resolve()), "original_input_sha256": digest(input_path),
               "grouped_input_sha256": digest(out / "grouped-full-request.json"),
               "original_pass_count": len(original_case["map"]["tracks"]), "grouped_source_count": len(tracks),
               "native_point_count": sum(len(t["physics_track"]["points"]) for t in tracks),
               "segment_count": sum(len(t["segments"]) for t in tracks),
               "observer_count": len(case["observers"]), "receiver_truth_read": False,
               "observations_unchanged": case["document"]["events"] == original_case["document"]["events"],
               "all_native_points_and_segments_preserved": True, "source_grouping": inventory, "tiles": tiles,
               "interpretation": "Grouping preserves disjoint native source support but changes event-association scope: one aircraft can support samples in distinct observed passes, whereas an original pass-level alternative required one pass to support the complete event. The hypothesis spaces are not identical. Tiling of the resulting grouped request changes scheduling only. Neither operation uses a fit result or truth coordinate."}
    write(out / "tiling.json", receipt)
    print(json.dumps({key: receipt[key] for key in ("original_pass_count", "grouped_source_count", "native_point_count", "segment_count", "observer_count")}, indent=2))


def run(prepared, executable, output):
    prepared, executable, output = map(lambda p: Path(p).resolve(), (prepared, executable, output))
    receipt = json.loads((prepared / "tiling.json").read_text(encoding="utf-8"))
    output.mkdir(parents=True, exist_ok=False)
    checksum = digest(executable)
    all_hypotheses, results = [], []
    original = json.loads((prepared / "grouped-full-request.json").read_text())["cases"][0]
    observer_indices = {tuple(point[key] for key in ("latitude_deg", "longitude_deg", "ellipsoid_height_m")): index for index, point in enumerate(original["observers"])}
    if len(observer_indices) != len(original["observers"]):
        raise ValueError("Global receiver inventory contains duplicate points")
    retain = original["plan"]["retain_hypotheses"]
    for index, tile in enumerate(receipt["tiles"]):
        request = prepared / tile["input"]
        if digest(request) != tile["sha256"] or digest(executable) != checksum:
            raise ValueError("Frozen tile or executable changed")
        folder = output / f"tile-{index:03}"
        command = [str(executable), "--input", str(request), "--out", str(folder)]
        with (output / f"tile-{index:03}.stdout.txt").open("w", encoding="utf-8") as stdout, (output / f"tile-{index:03}.stderr.txt").open("w", encoding="utf-8") as stderr:
            completed = subprocess.run(command, stdout=stdout, stderr=stderr, timeout=1200,
                                       creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        if completed.returncode:
            raise RuntimeError("Native tile failed; logs preserved")
        native = json.loads((folder / "summary.json").read_text())
        case = native["cases"][0]
        if not native["input_and_binary_unchanged"] or native["input_sha256"] != tile["sha256"] or native["executable_sha256"] != checksum or case["status"] != "completed":
            raise ValueError("Native tile did not complete the identical frozen request")
        report_path = folder / case["report"]
        if digest(report_path) != case["report_sha256"]:
            raise ValueError("Native tile report changed")
        report = json.loads(report_path.read_text())
        if report["status"] != "completed" or report["progress"]["completed_cells"] != report["progress"]["total_cells"]:
            raise ValueError("Native tile has partial work")
        results.append({"index": index, "input_sha256": tile["sha256"], "report": str(report_path),
                        "report_sha256": case["report_sha256"], "estimate": report["estimate"],
                        "progress": report["progress"], "supported_hypotheses": report["supported_hypotheses"],
                        "retained_truncated": report["retained_truncated"], "unavailable_count": report["unavailable_count"],
                        "rankable_cells": report["rankable_cells"], "elapsed_s": case["elapsed_s"]})
        for row in report["hypotheses"]:
            point = row["observer"]
            global_index = observer_indices[tuple(point[key] for key in ("latitude_deg", "longitude_deg", "ellipsoid_height_m"))]
            identity = hashlib.sha256(json.dumps({"observer_index": global_index, "clock": row["clock_offset_s"], "assignments": row["assignments"]}, sort_keys=True).encode()).hexdigest()
            all_hypotheses.append({**row, "id": f"field-{global_index:05}-{identity}", "native_id": row["id"],
                                   "global_observer_index": global_index, "native_rank": row["rank"], "tile": index})
        print(f"Completed tile{index+1}/{len(receipt['tiles'])}: {len(report['hypotheses'])} retained", flush=True)
    all_hypotheses.sort(key=lambda row: (row["score"], row["global_observer_index"], row["clock_offset_s"], row["id"]))
    selected = all_hypotheses[:retain]
    for rank, row in enumerate(selected, 1):
        row["rank"] = rank
    result = {"schema_version": "geosonnar.field_tile_merge.v1", "status": "all_native_tiles_completed",
              "tiling_sha256": digest(prepared / "tiling.json"), "original_input_sha256": receipt["original_input_sha256"],
              "grouped_input_sha256": receipt["grouped_input_sha256"], "native_executable_sha256": checksum,
              "tiles": results, "hypotheses": selected, "retained_total_before_merge": len(all_hypotheses),
              "retained_limit": retain, "receiver_truth_read": False, "location_verified": False,
              "merge_semantics": "Exact lowest-score global topK from each tile's topK; retained equal-score ties break by global receiver index, clock and canonical ID. Native IDs/ranks remain separate. Each declared receiver occurs once; all events, clocks and source alternatives remain in every tile. This aggregate is a research merge receipt, not a native RegionalReport or a claim that one GUI job accepts this total work."}
    write(output / "merged.json", result)
    print(json.dumps({"tiles": len(results), "retained": len(selected), "native_elapsed_s": sum(item["elapsed_s"] for item in results)}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    preparation = commands.add_parser("prepare")
    preparation.add_argument("--input", required=True)
    preparation.add_argument("--output", required=True)
    preparation.add_argument("--tile-size", type=int, default=16)
    execution = commands.add_parser("run")
    execution.add_argument("--prepared", required=True)
    execution.add_argument("--executable", required=True)
    execution.add_argument("--output", required=True)
    args = parser.parse_args()
    if args.command == "prepare": prepare(args.input, args.output, args.tile_size)
    else: run(args.prepared, args.executable, args.output)
