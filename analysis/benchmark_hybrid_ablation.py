"""Headless, frozen synthetic ablation of the production review scorer.

No recordings, GUI, network, atmospheric downloads or production model calls
are used to generate observations. Truth is read by evaluation only after all
predictions have been written. This is procedural separation, not a blind field
trial: the generator and evaluator share a local machine and an idealized model.
"""
from __future__ import annotations

import argparse
from copy import deepcopy
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import random
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
PROTOCOL = ROOT / "benchmarks/hybrid_ablation_protocol.json"
MODES = ("doppler_only", "thunder_only", "combined")


def encoded(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value):
    with Path(path).open("xb") as stream:
        stream.write(encoded(value))
        stream.flush()
        os.fsync(stream.fileno())


def identity(path):
    data = Path(path).read_bytes()
    return {"bytes": len(data), "sha256": sha(data)}


def ecef(lat, lon, height):
    """Independent WGS84 forward coordinates, in metres."""
    lat, lon = math.radians(lat), math.radians(lon)
    a, flattening = 6378137.0, 1 / 298.257223563
    eccentricity_sq = flattening * (2 - flattening)
    radius = a / math.sqrt(1 - eccentricity_sq * math.sin(lat) ** 2)
    return [(radius + height) * math.cos(lat) * math.cos(lon),
            (radius + height) * math.cos(lat) * math.sin(lon),
            (radius * (1 - eccentricity_sq) + height) * math.sin(lat)]


def geo(xyz):
    x, y, z = xyz
    a, f = 6378137.0, 1 / 298.257223563
    e2 = f * (2 - f)
    p = math.hypot(x, y)
    lat = math.atan2(z, p * (1 - e2))
    for _ in range(12):
        n = a / math.sqrt(1 - e2 * math.sin(lat) ** 2)
        lat = math.atan2(z + e2 * n * math.sin(lat), p)
    n = a / math.sqrt(1 - e2 * math.sin(lat) ** 2)
    return math.degrees(lat), math.degrees(math.atan2(y, x)), p / math.cos(lat) - n


def dot(a, b):
    return sum(x * y for x, y in zip(a, b))


def norm(a):
    return math.sqrt(dot(a, a))


def sub(a, b):
    return [x - y for x, y in zip(a, b)]


def add(a, b):
    return [x + y for x, y in zip(a, b)]


def scaled(a, scale):
    return [x * scale for x in a]


def basis(lat, lon):
    p, l = math.radians(lat), math.radians(lon)
    return ([-math.sin(l), math.cos(l), 0.0],
            [-math.sin(p) * math.cos(l), -math.sin(p) * math.sin(l), math.cos(p)],
            [math.cos(p) * math.cos(l), math.cos(p) * math.sin(l), math.sin(p)])


def offset_xyz(origin, axes, east, north, up):
    result = list(origin)
    for axis, value in zip(axes, (east, north, up)):
        result = add(result, scaled(axis, value))
    return result


def location(lat, lon, east, north, height):
    # Approximate local equal-angle coordinates; the actual forward distances
    # below use ellipsoid ECEF, and the same explicit vector goes to all modes.
    scale = 6371008.8 * math.pi / 180
    return {"latitude_deg": lat + north / scale,
            "longitude_deg": lon + east / (scale * math.cos(math.radians(lat))),
            "ellipsoid_height_m": height}


def xyz_of(observer):
    return ecef(observer["latitude_deg"], observer["longitude_deg"], observer["ellipsoid_height_m"])


def horizontal_error(a, b):
    p, q = math.radians(a["latitude_deg"]), math.radians(b["latitude_deg"])
    dl = math.radians(a["longitude_deg"] - b["longitude_deg"])
    hav = math.sin((p - q) / 2) ** 2 + math.cos(p) * math.cos(q) * math.sin(dl / 2) ** 2
    return 2 * 6371008.8 * math.asin(math.sqrt(min(1.0, max(0.0, hav))))


def oracle_sample(p0, velocity, receiver, emission, c, f0):
    displacement = sub(add(p0, scaled(velocity, emission)), receiver)
    distance = norm(displacement)
    radial_velocity = dot(displacement, velocity) / distance
    return emission + distance / c, f0 / (1 + radial_velocity / c)


def oracle_cpa(p0, velocity, receiver, c):
    emission = -dot(sub(p0, receiver), velocity) / dot(velocity, velocity)
    return oracle_sample(p0, velocity, receiver, emission, c, 1.0)[0]


def event(identifier, kind, arrival, protocol):
    return {"id": identifier, "label": identifier, "kind": kind,
            "origin": "synthetic reviewed measurement; no real audio", "review": "confirmed",
            "selected_for_scoring": True, "acoustic_source": "compatible",
            "start_s": 0.0, "end_s": protocol["duration_s"],
            "arrival_s": arrival, "points": [], **protocol["observation_uncertainty"],
            "flight_id": "", "emitted_frequency_hz": 0.0, "strike_id": "", "source": None,
            "diagnostics": {}}


def track(identifier, p0, velocity, origin_utc):
    physical, geographic = [], []
    for t in range(0, 361, 5):
        xyz = add(p0, scaled(velocity, t))
        lat, lon, h = geo(xyz)
        physical.append({"t_s": float(t), "xyz_m": xyz})
        geographic.append({"latitude_deg": lat, "longitude_deg": lon, "height_m": h,
                           "time_s": float(t), "time_utc_s": origin_utc + t})
    return {"id": identifier, "points": geographic, "segments": [[0, len(physical)]],
            "time_origin_utc_s": origin_utc, "max_gap_s": 6.0,
            "physics_track": {"flight_id": identifier, "points": physical}}


def scene(protocol, family, split, index):
    key = f'{protocol["generator_seed"]}:{family}:{split}:{index}'
    identifier = sha(key.encode())[:20]
    seed = int(sha(key.encode())[:16], 16)
    rng = random.Random(seed)
    lat, lon = rng.uniform(-55, 55), rng.uniform(-155, 155)
    grid = protocol["grid"]
    height = grid["receiver_height_m"]
    anchor, axes = ecef(lat, lon, height), basis(lat, lon)
    truth = location(lat, lon, rng.uniform(-grid["truth_half_width_m"], grid["truth_half_width_m"]),
                     rng.uniform(-grid["truth_half_width_m"], grid["truth_half_width_m"]), height)
    receiver = xyz_of(truth)
    stamp = datetime.strptime(protocol["recording_start_utc"], "%Y-%m-%d %H:%M:%S").replace(tzinfo=timezone.utc)
    origin = stamp.timestamp()
    c = rng.choice(protocol["mismatch"]["true_sound_speeds_mps"]) if family == "model_mismatch" else protocol["assumed_sound_speed_mps"]
    noise = protocol["noise"][family]
    observers = [location(lat, lon, -grid["half_width_m"] + col * grid["spacing_m"],
                          -grid["half_width_m"] + row * grid["spacing_m"], height)
                 for row in range(grid["side"]) for col in range(grid["side"])]
    heading = rng.uniform(0, 2 * math.pi)
    events, tracks, strikes = [], [], []
    decoy_shift = offset_xyz([0, 0, 0], axes, protocol["control_parameters"]["decoy_east_m"],
                             protocol["control_parameters"]["decoy_north_m"], 0)
    count = 1 if family == "ambiguous" else 2
    for j in range(count):
        theta = heading + j * rng.uniform(1.05, 2.1)
        speed, altitude = rng.uniform(75, 140), rng.uniform(1100, 2700)
        cross = rng.uniform(-1000, 1000)
        center_time = 115.0 + 40.0 * j
        velocity = offset_xyz([0, 0, 0], axes, speed * math.cos(theta), speed * math.sin(theta), 0)
        center = offset_xyz(anchor, axes, -cross * math.sin(theta), cross * math.cos(theta), altitude)
        p0 = sub(center, scaled(velocity, center_time))
        f0 = rng.uniform(85, 155)
        tracks.append(track(f"flight-{j}", p0, velocity, origin))
        tracks.append(track(f"alternative-flight-{j}", add(p0, decoy_shift), velocity, origin))
        arrival = oracle_cpa(p0, velocity, receiver, c) + rng.gauss(0, noise["doppler_time_sd_s"])
        observed = event(f"aircraft-{j}", "doppler", arrival, protocol)
        observed.update(flight_id=f"flight-{j}", emitted_frequency_hz=f0)
        previous_noise = rng.gauss(0, noise["frequency_sd_hz"])
        for k in range(protocol["emission_samples_per_aircraft"]):
            emission = center_time - 35 + 70 * k / (protocol["emission_samples_per_aircraft"] - 1)
            t, frequency = oracle_sample(p0, velocity, receiver, emission, c, f0)
            rho = noise["ridge_ar1"]
            previous_noise = rho * previous_noise + math.sqrt(1 - rho * rho) * rng.gauss(0, noise["frequency_sd_hz"])
            observed["points"].append({"time_s": t, "frequency_hz": frequency + previous_noise})
        events.append(observed)
    for j in range(protocol["strike_count"]):
        theta = heading + 0.4 + j * 2 * math.pi / protocol["strike_count"] + rng.uniform(-0.25, 0.25)
        radius = rng.uniform(3500, 8000)
        h = rng.uniform(600, 2000)
        coordinate = location(lat, lon, radius * math.cos(theta), radius * math.sin(theta), h)
        strike = {"id": f"strike-{j}", "time_utc_s": origin + 40 + j * 75,
                  "latitude_deg": coordinate["latitude_deg"], "longitude_deg": coordinate["longitude_deg"],
                  "altitude_m": h, "intensity": None}
        strikes.append(strike)
        alternate = location(coordinate["latitude_deg"], coordinate["longitude_deg"],
                             protocol["control_parameters"]["decoy_east_m"],
                             protocol["control_parameters"]["decoy_north_m"], h)
        strikes.append({**strike, "id": f"alternative-strike-{j}",
                        "latitude_deg": alternate["latitude_deg"], "longitude_deg": alternate["longitude_deg"]})
        source = xyz_of(coordinate)
        if family == "model_mismatch":
            angle = rng.uniform(0, 2 * math.pi)
            displacement = protocol["mismatch"]["thunder_source_displacement_m"]
            source = offset_xyz(source, axes, displacement * math.cos(angle), displacement * math.sin(angle), 0)
        arrival = strike["time_utc_s"] - origin + norm(sub(source, receiver)) / c + rng.gauss(0, noise["thunder_time_sd_s"])
        observed = event(f"thunder-{j}", "thunder", arrival, protocol)
        observed["strike_id"] = strike["id"]
        events.append(observed)
    lightning_bytes = encoded({"schema_version": 1, "strikes": strikes})
    case = {"id": identifier,
            "document": {"schema_version": 1, "case_id": identifier, "recording_sha256": sha(encoded(events)),
                         "channel": 0, "duration_s": protocol["duration_s"], "recording_start_utc": protocol["recording_start_utc"],
                         "sound_speed_mps": protocol["assumed_sound_speed_mps"], "events": events,
                         "notes": "Fabricated reviewed measurements; not a recording or a verified location",
                         "mix": {"doppler_frequency": True, "doppler_timing": True, "thunder_timing": True,
                                 **protocol["weights"], **protocol["scales"]},
                         "map": {"bounds": [observers[0]["latitude_deg"], observers[0]["longitude_deg"],
                                             observers[-1]["latitude_deg"], observers[-1]["longitude_deg"]],
                                 "grid_spacing_m": grid["spacing_m"], "receiver_height_m": height, "clock_adjustment_s": 0.0},
                         "datasets": [], "dataset_options": [], "strike_fallback_height_m": None},
            "map": {"source_path": "synthetic-tracks.json", "source_sha256": sha(encoded(tracks)),
                    "time_origin_utc_s": origin, "tracks": tracks, "candidates": [], "observers": [],
                    "grid": None, "terrain": None, "lightning": [], "notes": ["Synthetic trajectories"], "location_verified": False},
            "lightning": {"schema_version": 1, "input_path": "synthetic-lightning.json", "input_bytes": len(lightning_bytes),
                          "input_sha256": sha(lightning_bytes), "provenance": {"provider": "Independent synthetic oracle",
                          "source": "Fabricated point strikes", "license": "GPL-3.0-or-later", "notes": None,
                          "altitude_datum": "wgs84_ellipsoid"}, "strikes": strikes}, "observers": observers}
    label = {"id": identifier, "base_id": identifier, "split": split, "family": family, "seed": seed,
             "observer": truth, "true_sound_speed_mps": c, "anchor": [lat, lon], "control": None}
    return case, label


def control(protocol, base, truth, name):
    case, label = deepcopy(base), deepcopy(truth)
    case["id"] = sha(f'{base["id"]}:{name}'.encode())[:20]
    case["document"]["case_id"] = case["id"]
    label.update(id=case["id"], split="control", family=name, control=name)
    document = case["document"]
    if name == "wrong_aircraft":
        for e in document["events"]:
            if e["kind"] == "doppler": e["flight_id"] = "alternative-" + e["flight_id"]
    elif name == "wrong_strike":
        for e in document["events"]:
            if e["kind"] == "thunder": e["strike_id"] = "alternative-" + e["strike_id"]
    elif name in ("wrong_clock", "wrong_date"):
        key = "clock_shift_s" if name == "wrong_clock" else "wrong_date_shift_s"
        document["map"]["clock_adjustment_s"] = protocol["control_parameters"][key]
    elif name == "outside_region":
        shift = protocol["control_parameters"]["outside_grid_shift_east_m"]
        lat, lon = label["anchor"]
        delta = location(lat, lon, shift, 0, 80)["longitude_deg"] - lon
        for observer in case["observers"]: observer["longitude_deg"] += delta
        document["map"]["bounds"][1] += delta
        document["map"]["bounds"][3] += delta
    elif name == "missing_lightning":
        case["lightning"] = None
    elif name == "missing_track":
        case["map"]["tracks"] = [t for t in case["map"]["tracks"] if t["id"] != "flight-0"]
        case["map"]["source_sha256"] = sha(encoded(case["map"]["tracks"]))
    elif name == "unreviewed":
        for e in document["events"]:
            e.update(review="pending", selected_for_scoring=False)
    else:
        raise ValueError(name)
    return case, label


def generate(protocol):
    batches = {"development": [], "holdout": [], "control": []}
    labels, bases = [], []
    for split in ("development", "holdout"):
        for family in protocol["families"]:
            for index in range(protocol[split + "_per_family"]):
                case, label = scene(protocol, family, split, index)
                batches[split].append(case); labels.append(label)
                if split == "holdout" and family == protocol["control_base_family"] and index < protocol["control_base_count"]:
                    bases.append((case, label))
    for name in protocol["controls"]:
        for case, label in bases:
            changed, truth = control(protocol, case, label, name)
            batches["control"].append(changed); labels.append(truth)
    return batches, labels


def quantile(values, fraction):
    values = sorted(values)
    if not values: return None
    where = (len(values) - 1) * fraction
    lo = int(where); hi = min(lo + 1, len(values) - 1)
    return values[lo] + (values[hi] - values[lo]) * (where - lo)


def paired_statistics(rows, other, protocol):
    pairs = [(r["modes"]["combined"]["error_m"], r["modes"][other]["error_m"])
             for r in rows if r["modes"]["combined"]["error_m"] is not None and r["modes"][other]["error_m"] is not None]
    if not pairs: return {"paired_count": 0}
    differences = [a - b for a, b in pairs]
    tolerance = protocol["paired_tie_tolerance_m"]
    wins, losses = sum(d < -tolerance for d in differences), sum(d > tolerance for d in differences)
    rng = random.Random(protocol["bootstrap_seed"])
    means, medians = [], []
    for _ in range(protocol["bootstrap_repetitions"]):
        sample = [pairs[rng.randrange(len(pairs))] for _ in pairs]
        means.append(statistics.mean(a - b for a, b in sample))
        medians.append(statistics.median(a for a, _ in sample) - statistics.median(b for _, b in sample))
    n = wins + losses
    sign_p = min(1.0, 2 * sum(math.comb(n, k) for k in range(min(wins, losses) + 1)) / 2 ** n) if n else 1.0
    return {"paired_count": len(pairs), "combined_wins": wins, "ties_within_1m": len(pairs)-wins-losses,
            "combined_losses": losses, "mean_error_difference_m": statistics.mean(differences),
            "median_error_difference_m": statistics.median(a for a, _ in pairs) - statistics.median(b for _, b in pairs),
            "mean_difference_bootstrap_95_m": [quantile(means, .025), quantile(means, .975)],
            "median_difference_bootstrap_95_m": [quantile(medians, .025), quantile(medians, .975)],
            "sign_test_two_sided_p": sign_p,
            "interpretation": "Negative difference favors combined. Median difference is the difference of marginal medians, with paired scene resampling; descriptive synthetic inference, not field-population inference"}


def aggregate(rows, protocol, paired=True):
    result = {"cases": len(rows), "modes": {}}
    for mode in MODES:
        values = [r["modes"][mode] for r in rows]
        errors = [v["error_m"] for v in values if v["error_m"] is not None]
        result["modes"][mode] = {"ranked": len(errors), "no_rankable_or_error": len(values)-len(errors),
            "mean_error_m": statistics.mean(errors) if errors else None,
            "median_error_m": statistics.median(errors) if errors else None,
            "p90_error_m": quantile(errors,.9), "maximum_error_m": max(errors) if errors else None,
            "success_counts": {str(int(t)): sum(e <= t for e in errors) for t in protocol["success_thresholds_m"]},
            "success_denominator": len(rows), "misleading_low_score_wrong": sum(v["low_score_wrong"] for v in values),
            "verified_locations": sum(v["location_verified"] for v in values)}
    if paired:
        result["combined_vs_doppler"] = paired_statistics(rows, "doppler_only", protocol)
        result["combined_vs_thunder"] = paired_statistics(rows, "thunder_only", protocol)
    return result


def run_command(command, cwd, output, name):
    started = time.perf_counter()
    completed = subprocess.run(command, cwd=cwd, capture_output=True, text=True, encoding="utf-8", timeout=1800)
    (output / f"{name}.stdout").write_text(completed.stdout, encoding="utf-8")
    (output / f"{name}.stderr").write_text(completed.stderr, encoding="utf-8")
    row = {"argv": command, "returncode": completed.returncode, "elapsed_s": time.perf_counter()-started}
    write_json(output / f"{name}.command.json", row)
    if completed.returncode:
        raise RuntimeError(f"{name} failed; see its retained stderr log")
    return row


def require(condition, message):
    if not condition:
        raise ValueError(message)


def validate_labels(protocol, labels):
    """Reject denominator/split changes independently of scorer outcomes."""
    expected = {}
    bases = []
    for split in ("development", "holdout"):
        for family in protocol["families"]:
            for index in range(protocol[split + "_per_family"]):
                key = f'{protocol["generator_seed"]}:{family}:{split}:{index}'
                identifier = sha(key.encode())[:20]
                expected[identifier] = (split, family, identifier, int(sha(key.encode())[:16], 16), None)
                if split == "holdout" and family == protocol["control_base_family"] and index < protocol["control_base_count"]:
                    bases.append(identifier)
    for control_name in protocol["controls"]:
        for base in bases:
            identifier = sha(f'{base}:{control_name}'.encode())[:20]
            expected[identifier] = ("control", control_name, base, expected[base][3], control_name)
    require(len(labels) == len(expected), "Unexpected truth case count")
    require(len({row["id"] for row in labels}) == len(labels), "Duplicate truth IDs")
    require({row["id"] for row in labels} == set(expected), "Unexpected truth case IDs")
    for row in labels:
        require(tuple(row[k] for k in ("split", "family", "base_id", "seed", "control")) == expected[row["id"]],
                "Truth split/family/base/seed does not match protocol")


def validate_prediction(batch, report, input_identity, scorer_identity, source_snapshot):
    """Audit label-free input, frozen binary/source, all modes and ranking contract."""
    require(report["schema_version"] == batch["schema_version"] == 1, "Wrong schema")
    require(report["scorer"] == "geosonnar::workspace_scoring::score_observers", "Wrong scorer")
    require(report["input"] == input_identity and report["executable"] == scorer_identity, "Input or executable identity changed")
    require(report["evaluation_labels_read"] is False and report["location_verified"] is False, "Unexpected truth or verification claim")
    compiled = report["compiled_source_sha256"]
    required_sources = {"examples/hybrid_ablation.rs", "src/atmosphere.rs", "src/modality_diagnostics.rs", "src/workspace_scoring.rs", "src/evidence_score.rs", "src/physics.rs",
                        "src/workspace.rs", "src/thunder.rs", "src/map_data.rs", "Cargo.toml", "Cargo.lock"}
    require(set(compiled) == required_sources, "Unexpected compiled source inventory")
    require(all(digest == source_snapshot[name]["sha256"] for name, digest in compiled.items()), "Stale compiled source")
    require([c["id"] for c in report["cases"]] == [c["id"] for c in batch["cases"]], "Prediction case order/IDs changed")
    flags = {"doppler_only": (True, True, False), "thunder_only": (False, False, True), "combined": (True, True, True)}
    definitions = (("frequency", "doppler_frequency", "frequency_weight", "frequency_scale_hz"),
                   ("doppler_timing", "doppler_timing", "doppler_timing_weight", "doppler_timing_scale_s"),
                   ("thunder_timing", "thunder_timing", "thunder_weight", "thunder_scale_s"))
    for supplied, result in zip(batch["cases"], report["cases"]):
        require(set(supplied) == {"id", "document", "map", "lightning", "observers"}, "Unexpected scorer input fields")
        require(result["observer_count"] == len(supplied["observers"]), "Observer count changed")
        require(sorted(m["mode"] for m in result["modes"]) == sorted(MODES), "Missing or duplicate modes")
        shared_terms = {}
        for mode in result["modes"]:
            require(mode["document_invariant_sha256"] == result["document_invariant_sha256"], "Mode changed shared assumptions")
            scored = mode["report"]
            if mode["status"] == "unavailable":
                require(scored is None and bool(mode["error"]), "Unavailable mode has scores or no reason")
                continue
            require(mode["status"] == "scored" and mode["error"] is None and scored is not None, "Invalid score status")
            require(scored["location_verified"] is False, "Unexpected verified report")
            require("uncalibrated" in scored["calibration"], "Unexpected calibration claim")
            document, mix = supplied["document"], supplied["document"]["mix"]
            config = {"sound_speed_mps": document["sound_speed_mps"],
                      "fallback_strike_ellipsoid_height_m": document["strike_fallback_height_m"],
                      "allow_exploratory_associations": False}
            selected = []
            settings = {}
            for enabled, (key, term, weight, scale) in zip(flags[mode["mode"]], definitions):
                config[key] = {"enabled": enabled, "weight": mix[weight], "residual_scale": mix[scale]}
                if enabled:
                    selected.append(term)
                    settings[term] = config[key]
            require(scored["config"] == config and scored["selected_terms"] == selected, "Changed scoring settings")
            candidates = scored["candidates"]
            require(len(candidates) == len(supplied["observers"]), "Missing candidates")
            total_weight = sum(s["weight"] for s in settings.values())
            for i, (candidate, observer) in enumerate(zip(candidates, supplied["observers"])):
                require(candidate["id"] == f"receiver-{i:05}" and candidate["observer"] == observer, "Candidate grid/order changed")
                require(candidate["location_verified"] is False, "Unexpected verified candidate")
                require([t["term"] for t in candidate["terms"]] == selected, "Missing or duplicate terms")
                for term in candidate["terms"]:
                    setting = settings[term["term"]]
                    require(term["weight"] == setting["weight"] and term["residual_scale"] == setting["residual_scale"], "Changed term weights/floors")
                    key = (i, term["term"])
                    invariant = {k:v for k,v in term.items() if k != "weighted_contribution"}
                    require(key not in shared_terms or shared_terms[key] == invariant, "Common term changed between modes")
                    shared_terms[key] = invariant
                    if term["available"]:
                        means = list(term["event_mean_squared_residuals"].values())
                        require(bool(means) and term["observation_count"] > 0 and all(math.isfinite(v) and v >= 0 for v in means), "Invalid term residuals")
                        mean = statistics.mean(means)
                        require(math.isclose(mean, term["mean_normalized_squared_residual"], rel_tol=1e-10, abs_tol=1e-10), "Invalid event aggregation")
                        require(math.isclose(mean * term["weight"] / total_weight, term["weighted_contribution"], rel_tol=1e-10, abs_tol=1e-10), "Invalid term normalization")
                    else:
                        require(term["weighted_contribution"] is None and bool(term["unavailable_reason"]), "Missing term silently used")
                if candidate["score"] is None:
                    require(candidate["rank"] is None and bool(candidate["unavailable_reasons"]), "Unavailable candidate ranked")
                else:
                    require(math.isfinite(candidate["score"]) and candidate["score"] >= 0, "Invalid score")
                    require(not candidate["unavailable_reasons"] and all(t["available"] for t in candidate["terms"]), "Missing evidence silently dropped")
                    require(math.isclose(candidate["score"], sum(t["weighted_contribution"] for t in candidate["terms"]), rel_tol=1e-10, abs_tol=1e-10), "Invalid combined score")
            ranked = sorted((c for c in candidates if c["score"] is not None), key=lambda c:(c["score"],c["id"]))
            require([c["rank"] for c in ranked] == list(range(1, len(ranked)+1)), "Invalid candidate ranks")
            require(scored["ranked_candidate_ids"] == [c["id"] for c in ranked], "Ranking IDs disagree")


def evaluate(protocol, labels, predictions):
    # Adapter schema validation and matched-input auditing live here, not in the
    # physics generator. No score-derived tuning is performed by this function.
    validate_labels(protocol, labels)
    expected = {label["id"]: label for label in labels}
    observed = {case["id"]: case for report in predictions for case in report["cases"]}
    if len(observed) != sum(len(r["cases"]) for r in predictions) or set(observed) != set(expected):
        raise ValueError("Missing, duplicated or extra prediction cases")
    rows = []
    for identifier, label in expected.items():
        case = observed[identifier]
        modes = {}
        for mode in MODES:
            prediction = next(item for item in case["modes"] if item["mode"] == mode)
            scored = prediction.get("report")
            ranked = [r for r in scored["candidates"] if r["rank"] is not None] if scored else []
            winner = min(ranked, key=lambda r:r["rank"]) if ranked else None
            error = horizontal_error(winner["observer"], label["observer"]) if winner else None
            score = winner["score"] if winner else None
            modes[mode] = {"error_m": error, "score": score, "winner": winner["id"] if winner else None,
                           "observer": winner["observer"] if winner else None,
                           "status": prediction["status"], "error": prediction.get("error"),
                           "rankable_count": len(ranked), "location_verified": scored["location_verified"] if scored else False,
                           "low_score_wrong": error is not None and error > protocol["diagnostic_wrong_error_m"] and score <= protocol["diagnostic_low_score_max"]}
        rows.append({"id": identifier, "base_id": label["base_id"], "split": label["split"], "family": label["family"], "modes": modes})
    primary = [r for r in rows if r["split"] == "holdout" and r["family"] in protocol["primary_families"]]
    report = {"schema_version": 1, "evidence_class": protocol["evidence_class"], "primary": aggregate(primary,protocol),
              "holdout_by_family": {f: aggregate([r for r in rows if r["split"] == "holdout" and r["family"] == f],protocol) for f in protocol["families"]},
              "controls": {f: aggregate([r for r in rows if r["split"] == "control" and r["family"] == f],protocol,paired=False) for f in protocol["controls"]},
              "development": aggregate([r for r in rows if r["split"] == "development"],protocol,paired=False),
              "cases": rows, "real_world_accuracy_improvement_established": False,
              "production_confidence_calibrated": False}
    paired = report["primary"]["combined_vs_doppler"]
    report["predeclared_primary_synthetic_improvement"] = (paired["paired_count"] == len(primary) > 0
        and paired.get("mean_difference_bootstrap_95_m", [0,0])[1] < 0
        and paired.get("median_difference_bootstrap_95_m",[0,0])[1] < 0
        and report["primary"]["modes"]["combined"]["success_counts"]["500"] >= report["primary"]["modes"]["doppler_only"]["success_counts"]["500"])
    return report


def snapshot():
    paths = [PROTOCOL, Path(__file__), ROOT / "examples/hybrid_ablation.rs", ROOT / "Cargo.toml", ROOT / "Cargo.lock"]
    paths += [ROOT / "benchmarks/test_hybrid_ablation.py"]
    paths += sorted((ROOT / "src").rglob("*.rs"))
    return {p.relative_to(ROOT).as_posix(): identity(p) for p in paths}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="New experiment directory; existing paths are refused")
    parser.add_argument("--scorer", type=Path, help="Existing compiled hybrid_ablation example; otherwise build it offline")
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists():
        parser.error("Choose a new output directory; previous evidence is preserved")
    output.mkdir(parents=True)
    protocol = json.loads(PROTOCOL.read_text(encoding="utf-8"))
    before = snapshot()
    write_json(output / "protocol.json", protocol)
    write_json(output / "source-snapshot.json", before)
    batches, labels = generate(protocol)
    for split, cases in batches.items(): write_json(output / f"{split}-input.json", {"schema_version":1,"cases":cases})
    write_json(output / "evaluation-truth.json", {"schema_version":1,"cases":labels})
    del labels, batches
    frozen = {p.name: identity(p) for p in sorted(output.glob("*.json"))}
    write_json(output / "frozen-before-predictions.json", {"created_utc":datetime.now(timezone.utc).isoformat(),"files":frozen,"rules":"No weights, thresholds or inputs retuned after this receipt"})
    scorer = args.scorer.resolve() if args.scorer else ROOT / "target/release/examples" / ("hybrid_ablation.exe" if os.name=="nt" else "hybrid_ablation")
    if not args.scorer:
        run_command(["cargo","build","--release","--offline","--locked","--example","hybrid_ablation"],ROOT,output,"build")
    scorer_before = identity(scorer)
    for split in ("development","holdout","control"):
        print(json.dumps({"stage":split,"status":"running"}),flush=True)
        run_command([str(scorer),"--input",str(output/f"{split}-input.json"),"--out",str(output/f"{split}-predictions.json")],ROOT,output,split)
    prediction_receipt = {f"{s}-predictions.json":identity(output/f"{s}-predictions.json") for s in ("development","holdout","control")}
    write_json(output / "predictions-committed-before-evaluation.json", {"created_utc":datetime.now(timezone.utc).isoformat(),"files":prediction_receipt,"scorer":scorer_before})
    predictions = [json.loads((output/f"{s}-predictions.json").read_text(encoding="utf-8")) for s in ("development","holdout","control")]
    for split, prediction in zip(("development", "holdout", "control"), predictions):
        supplied = json.loads((output/f"{split}-input.json").read_text(encoding="utf-8"))
        validate_prediction(supplied, prediction, frozen[f"{split}-input.json"], scorer_before, before)
    write_json(output / "prediction-contract-checked.json", {"passed":True,"truth_accessed":False})
    # First evaluation-time access to truth, after every prediction is committed and audited.
    labels = json.loads((output/"evaluation-truth.json").read_text(encoding="utf-8"))["cases"]
    summary = evaluate(protocol,labels,predictions)
    if before != snapshot() or scorer_before != identity(scorer):
        raise RuntimeError("Code or scorer changed during the experiment")
    for name, expected in {**frozen, **prediction_receipt}.items():
        if identity(output/name) != expected: raise RuntimeError(f"Frozen experiment artifact changed: {name}")
    summary.update(status="completed",frozen_source_unchanged=True,scorer=scorer_before,protocol_sha256=identity(output/"protocol.json")["sha256"])
    write_json(output/"summary.json",summary)
    print(json.dumps({"status":"completed","summary":str(output/"summary.json"),"primary":summary["primary"],"predeclared_primary_synthetic_improvement":summary["predeclared_primary_synthetic_improvement"]},indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
