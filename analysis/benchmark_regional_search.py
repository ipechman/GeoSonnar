"""Fresh uncertain-clock/source benchmarks for the production guided-search engine.

Uses only the independent analytic geometry functions from the previous harness;
new seeds, inputs and protocol are separate from its frozen historical holdouts.
The native scorer receives no truth labels. Real-world validity is not asserted.
"""
from copy import deepcopy
import argparse
import hashlib
import json
import math
from pathlib import Path
import random
import statistics
import subprocess
import time

import benchmark_hybrid_ablation as oracle

ROOT = Path(__file__).resolve().parents[1]
PROTOCOL = ROOT / "benchmarks/regional_search_protocol.json"
MODES = ("doppler_only", "thunder_only", "combined")


def generated(protocol):
    base = json.loads((ROOT / "benchmarks/hybrid_ablation_protocol.json").read_text())
    base["generator_seed"] = protocol["generator_seed"]
    base["grid"] = protocol["grid"]
    cases, labels = [], []
    families = [(f, i, False) for f in protocol["families"] for i in range(protocol["holdouts_per_family"])]
    families += [("nominal", i, True) for i in range(protocol["analytic_cases"])]
    for family, index, analytic in families:
        settings = deepcopy(base)
        if analytic:
            settings["grid"]["truth_half_width_m"] = 0.0
            settings["noise"]["nominal"] = dict.fromkeys(settings["noise"]["nominal"], 0.0)
        case, truth = oracle.scene(settings, family, "regional-analytic" if analytic else "regional-holdout", index)
        rng = random.Random(int(case["id"], 16))
        offset = rng.choice(protocol["true_clock_offsets_s"])
        source_names = {}
        for source in [t["id"] for t in case["map"]["tracks"]] + [s["id"] for s in case["lightning"]["strikes"]]:
            source_names[source] = hashlib.sha256((case["id"]+":"+source).encode()).hexdigest()[:16]
        for track in case["map"]["tracks"]:
            track["id"] = source_names[track["id"]]
            track["physics_track"]["flight_id"] = track["id"]
        for strike in case["lightning"]["strikes"]:
            strike["id"] = source_names[strike["id"]]
        associations = {}
        options = []
        for event in case["document"]["events"]:
            is_doppler = event["kind"] == "doppler"
            source_field = "flight_id" if is_doppler else "strike_id"
            associations[event["id"]] = source_names[event[source_field]]
            event[source_field] = ""
            event["emitted_frequency_hz"] = 0.0
            event["arrival_s"] -= offset
            for point in event["points"]:
                point["time_s"] -= offset
            alternatives = [t["id"] for t in case["map"]["tracks"]] if is_doppler else [s["id"] for s in case["lightning"]["strikes"]]
            rng.shuffle(alternatives)
            options.append({"event_id": event["id"], "source_ids": alternatives,
                            "emitted_frequency_hz": protocol["emitted_frequency_bounds_hz"] if is_doppler else None})
        case["plan"] = {"schema_version": 1, "clock": deepcopy(protocol["clock"]), "events": options,
                        "max_evaluations": protocol["max_evaluations"], "retain_hypotheses": protocol["retained_hypotheses"]}
        case["map"]["source_sha256"] = oracle.sha(oracle.encoded(case["map"]["tracks"]))
        case["lightning"]["input_sha256"] = oracle.sha(oracle.encoded(case["lightning"]["strikes"]))
        case["document"]["regional_plan"] = deepcopy(case["plan"])
        truth.update(analytic=analytic, true_clock_offset_s=offset, associations=associations, control=None)
        cases.append(case)
        labels.append(truth)
    for control in protocol["controls"]:
        for case, truth in list(zip(cases, labels))[:protocol["control_bases"]]:
            copy, label = deepcopy(case), deepcopy(truth)
            copy["id"] = oracle.sha((case["id"]+":"+control).encode())[:20]
            copy["document"]["case_id"] = copy["id"]
            label.update(id=copy["id"], control=control, analytic=False)
            if control == "missing_track":
                missing = copy["map"]["tracks"][0]["id"]
                copy["map"]["tracks"] = [t for t in copy["map"]["tracks"] if t["id"] != missing]
                copy["map"]["source_sha256"] = oracle.sha(oracle.encoded(copy["map"]["tracks"]))
            elif control == "wrong_clock_range":
                copy["plan"]["clock"] = {"min_offset_s": 30.0, "max_offset_s": 40.0, "step_s": 5.0}
            elif control == "unreviewed":
                for event in copy["document"]["events"]:
                    event["review"] = "pending"
                    event["selected_for_scoring"] = False
            elif control == "budget_too_small":
                copy["plan"]["max_evaluations"] = 1
            copy["document"]["regional_plan"] = deepcopy(copy["plan"])
            cases.append(copy)
            labels.append(label)
    batches = {}
    for mode in MODES:
        rows = deepcopy(cases)
        for case in rows:
            case["document"]["mix"].update(doppler_frequency=mode != "thunder_only",
                                           doppler_timing=mode != "thunder_only", thunder_timing=mode != "doppler_only")
            enabled = {event["id"] for event in case["document"]["events"]
                       if (event["kind"] == "doppler") == (mode == "doppler_only") or mode == "combined"}
            case["plan"]["events"] = [e for e in case["plan"]["events"] if e["event_id"] in enabled]
            case["document"]["regional_plan"] = deepcopy(case["plan"])
        batches[mode] = {"schema_version": 1, "cases": rows}
    return batches, labels


def evaluate(protocol, labels, predictions, batches):
    rows, checks = [], []
    truths = {label["id"]: label for label in labels}
    if set(predictions) != set(MODES):
        raise ValueError("Every scoring mode must have predictions")
    for mode, bundle in predictions.items():
        inputs = {case["id"]: case for case in batches[mode]["cases"]}
        outcome_ids = [outcome["id"] for outcome, _ in bundle]
        if len(outcome_ids) != len(set(outcome_ids)) or set(outcome_ids) != set(inputs):
            raise ValueError("Prediction cases are missing, repeated or unexpected")
        for outcome, report in bundle:
            label, case = truths[outcome["id"]], inputs[outcome["id"]]
            row = {"id": label["id"], "mode": mode, "family": label["family"], "control": label["control"],
                   "analytic": label["analytic"], "elapsed_s": outcome["elapsed_s"], "status": outcome["status"],
                   "error_m": None, "shortlist_nearest_error_m": None, "clock_error_s": None, "correct_associations": None}
            expected_error = label["control"] in ("unreviewed", "budget_too_small") or (label["control"] == "missing_track" and mode != "thunder_only")
            if expected_error:
                checks.append((f"{label['id']}:{mode}:explicit_error", outcome["status"] == "input_or_search_error"))
            else:
                checks.append((f"{label['id']}:{mode}:run_completed", report is not None and report["status"] == "completed"))
            if report is not None:
                clock = case["plan"]["clock"]
                count = math.ceil((clock["max_offset_s"]-clock["min_offset_s"])/clock["step_s"])
                clock_values = [min(clock["max_offset_s"], clock["min_offset_s"]+i*clock["step_s"]) for i in range(count+1)]
                checks.append((f"{label['id']}:{mode}:complete_coverage",
                    report["progress"]["completed_cells"] == report["progress"]["total_cells"]
                    == len(case["observers"])*len(clock_values)
                    and report["progress"]["completed_evaluations"] == report["estimate"]["evaluations"]
                    == len(case["observers"])*len(clock_values)*sum(len(e["source_ids"]) for e in case["plan"]["events"])
                    and report["rankable_cells"]+report["unavailable_cells"] == report["progress"]["total_cells"]))
                checks.append((f"{label['id']}:{mode}:identity_and_no_verdict", report["plan"] == case["plan"] and report["location_verified"] is False))
                hypotheses = report["hypotheses"]
                approved = {e["event_id"]: e for e in case["plan"]["events"]}
                faithful = True
                for hypothesis in hypotheses:
                    faithful &= hypothesis["observer"] in case["observers"] and hypothesis["clock_offset_s"] in clock_values
                    faithful &= len(hypothesis["assignments"]) == len(approved) and {a["event_id"] for a in hypothesis["assignments"]} == set(approved)
                    for assignment in hypothesis["assignments"]:
                        option = approved.get(assignment["event_id"])
                        faithful &= option is not None and assignment["source_id"] in option["source_ids"]
                        if option and option["emitted_frequency_hz"] is not None:
                            fitted = assignment["emitted_frequency_hz"]
                            faithful &= fitted is not None and option["emitted_frequency_hz"][0] <= fitted <= option["emitted_frequency_hz"][1]
                checks.append((f"{label['id']}:{mode}:approved_hypotheses_only", faithful))
                if hypotheses:
                    best = hypotheses[0]
                    row.update(error_m=oracle.horizontal_error(best["observer"], label["observer"]),
                               shortlist_nearest_error_m=min(oracle.horizontal_error(h["observer"], label["observer"]) for h in hypotheses),
                               clock_error_s=abs(best["clock_offset_s"]-label["true_clock_offset_s"]),
                               correct_associations=all(a["source_id"] == label["associations"][a["event_id"]] for a in best["assignments"]),
                               best_score=best["score"], retained=len(hypotheses), supported=report["supported_hypotheses"])
                if label["analytic"] and mode == "combined":
                    checks.append((f"{label['id']}:analytic_recovery", row["error_m"] is not None and row["error_m"] < 0.01 and row["clock_error_s"] < 1e-8 and row["correct_associations"]))
            rows.append(row)
    aggregates = []
    for mode in MODES:
        for family in protocol["families"]:
            selected = [r for r in rows if r["mode"] == mode and r["family"] == family and not r["analytic"] and r["control"] is None]
            errors = [r["error_m"] for r in selected if r["error_m"] is not None]
            aggregates.append({"mode": mode, "family": family, "cases": len(selected), "rankable": len(errors),
                               "median_error_m": statistics.median(errors) if errors else None,
                               "within_distance": {str(t): sum(e <= t for e in errors) for t in protocol["success_thresholds_m"]},
                               "total_elapsed_s": sum(r["elapsed_s"] for r in selected)})
    return {"schema_version": 1, "status": "passed" if all(ok for _, ok in checks) else "failed",
            "checks": [{"name": name, "passed": ok} for name, ok in checks], "rows": rows, "aggregates": aggregates,
            "field_accuracy_validated": False, "location_verified": False,
            "scope": "Reviewed synthetic observations; unknown shared clock, opaque approved sources and bounded tones. Does not test raw-audio detection, independent field accuracy or calibrated acceptance."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scorer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    protocol = json.loads(PROTOCOL.read_text())
    args.output.mkdir(parents=False, exist_ok=False)
    batches, labels = generated(protocol)
    oracle.write_json(args.output/"protocol.json", protocol)
    oracle.write_json(args.output/"truth.json", labels)
    identities = {}
    for mode, batch in batches.items():
        path = args.output/(mode+"-inputs.json")
        oracle.write_json(path, batch)
        identities[path.name] = oracle.identity(path)
    identities["truth.json"] = oracle.identity(args.output/"truth.json")
    identities["protocol.json"] = oracle.identity(args.output/"protocol.json")
    scorer_identity = oracle.identity(args.scorer)
    source_files = [PROTOCOL, Path(__file__), ROOT/"analysis/benchmark_hybrid_ablation.py"]
    oracle.write_json(args.output/"freeze.json", {"inputs": identities, "scorer": scorer_identity,
                      "sources": {str(p.relative_to(ROOT)): oracle.identity(p) for p in source_files}})
    for mode in MODES:
        start = time.monotonic()
        with (args.output/(mode+".log")).open("xb") as log:
            completed = subprocess.run([str(args.scorer.resolve()), "--input", str((args.output/(mode+"-inputs.json")).resolve()),
                                        "--out", str((args.output/mode).resolve())], stdout=log, stderr=subprocess.STDOUT, check=False)
        print(f"{mode}: exit={completed.returncode}, elapsed={time.monotonic()-start:.1f}s", flush=True)
        if completed.returncode:
            raise RuntimeError(f"Native adapter failed for {mode}; diagnostic preserved")
    # Predictions from all modes exist before the evaluator reads truth.
    for name, expected in identities.items():
        if oracle.identity(args.output/name) != expected:
            raise RuntimeError(f"Frozen input changed: {name}")
    if oracle.identity(args.scorer) != scorer_identity:
        raise RuntimeError("Scorer executable changed")
    predictions = {}
    for mode in MODES:
        directory = args.output/mode
        build = json.loads((directory/"build.json").read_text())
        for name, expected in build["compiled_source_sha256"].items():
            if oracle.identity(ROOT/name)["sha256"] != expected:
                raise RuntimeError(f"Compiled source no longer matches checkout: {name}")
        summary = json.loads((directory/"summary.json").read_text())
        if summary["input_sha256"] != identities[mode+"-inputs.json"]["sha256"] or summary["executable_sha256"] != scorer_identity["sha256"]:
            raise RuntimeError("Prediction input or executable binding mismatch")
        results = []
        for outcome in summary["cases"]:
            report = None
            if "report" in outcome:
                path = directory/outcome["report"]
                if oracle.identity(path)["sha256"] != outcome["report_sha256"]:
                    raise RuntimeError("Prediction report changed")
                report = json.loads(path.read_text())
            results.append((outcome, report))
        predictions[mode] = results
    result = evaluate(protocol, json.loads((args.output/"truth.json").read_text()), predictions, batches)
    oracle.write_json(args.output/"summary.json", result)
    print(json.dumps({"status": result["status"], "checks": len(result["checks"]), "failed": [c["name"] for c in result["checks"] if not c["passed"]]}))
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
