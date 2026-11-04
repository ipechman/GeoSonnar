"""Headless controls for the ablation harness; never run the scored experiment.

All generated scenes use a separate, reduced test protocol and remain in memory.
Crafted scorer reports test bookkeeping, not the production scorer's accuracy.
"""
from copy import deepcopy
import importlib.util
import json
import math
from pathlib import Path
import random
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "hybrid_ablation", ROOT / "analysis" / "benchmark_hybrid_ablation.py"
)
BENCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BENCH)


def test_protocol():
    protocol = json.loads(BENCH.PROTOCOL.read_text(encoding="utf-8"))
    # Do not generate the registered experimental scenes in a harness test.
    protocol.update(generator_seed=19740517, development_per_family=1,
                    holdout_per_family=2, control_base_count=1,
                    emission_samples_per_aircraft=5, bootstrap_repetitions=100)
    protocol["grid"].update(side=5, half_width_m=500.0,
                            spacing_m=250.0, truth_half_width_m=200.0)
    return protocol


def evaluation_predictions(labels):
    """Invent outcomes with known displacement; no physics/scoring is invoked."""
    cases = []
    for label in labels:
        truth = label["observer"]
        modes = []
        for mode, east in (("doppler_only", 1200.0), ("thunder_only", 100.0),
                           ("combined", 0.0)):
            modes.append({"mode": mode, "status": "scored", "error": None,
                          "report": {"location_verified": False, "candidates": [{
                              "id": "receiver-00000", "rank": 1, "score": 0.5,
                              "observer": BENCH.location(truth["latitude_deg"],
                                  truth["longitude_deg"], east, 0.0,
                                  truth["ellipsoid_height_m"])}]}})
        cases.append({"id": label["id"], "modes": modes})
    return [{"cases": cases}]


def validator_fixture():
    """Small, internally consistent adapter output with all three modes."""
    document = {"sound_speed_mps": 343.0, "strike_fallback_height_m": None,
                "mix": {"frequency_weight": 1.0, "doppler_timing_weight": 1.0,
                        "thunder_weight": 1.0, "frequency_scale_hz": 1.0,
                        "doppler_timing_scale_s": 1.0, "thunder_scale_s": 1.0}}
    observers = [BENCH.location(0, 0, east, 0, 80) for east in (0, 1000)]
    batch = {"schema_version": 1, "cases": [{"id": "fixture",
             "document": document, "map": {}, "lightning": None,
             "observers": observers}]}
    input_identity = {"bytes": 123, "sha256": "1" * 64}
    scorer_identity = {"bytes": 456, "sha256": "2" * 64}
    sources = ("examples/hybrid_ablation.rs", "src/atmosphere.rs", "src/modality_diagnostics.rs", "src/workspace_scoring.rs",
               "src/evidence_score.rs", "src/physics.rs", "src/workspace.rs",
               "src/thunder.rs", "src/map_data.rs", "Cargo.toml", "Cargo.lock")
    snapshot = {name: {"sha256": "3" * 64} for name in sources}
    result = {"id": "fixture", "observer_count": 2,
              "document_invariant_sha256": "4" * 64, "modes": []}
    report = {"schema_version": 1,
              "scorer": "geosonnar::workspace_scoring::score_observers",
              "input": input_identity, "executable": scorer_identity,
              "evaluation_labels_read": False, "location_verified": False,
              "compiled_source_sha256": {name: "3" * 64 for name in sources},
              "cases": [result]}
    terms = (("frequency", "doppler_frequency"),
             ("doppler_timing", "doppler_timing"),
             ("thunder_timing", "thunder_timing"))
    selections = ((True, True, False), (False, False, True), (True, True, True))
    for mode, flags in zip(BENCH.MODES, selections):
        config = {"sound_speed_mps": 343.0,
                  "fallback_strike_ellipsoid_height_m": None,
                  "allow_exploratory_associations": False}
        selected = [term for (_, term), enabled in zip(terms, flags) if enabled]
        for (key, _), enabled in zip(terms, flags):
            config[key] = {"enabled": enabled, "weight": 1.0, "residual_scale": 1.0}
        candidates = []
        for index, observer in enumerate(observers):
            residuals = []
            for term_index, (_, term) in enumerate(terms):
                if term not in selected:
                    continue
                mean = float((index + 1) * (term_index + 1))
                residuals.append({"term": term, "weight": 1.0,
                    "residual_scale": 1.0, "available": True,
                    "unavailable_reason": None, "observation_count": 2,
                    "event_mean_squared_residuals": {"event-a": mean / 2,
                                                     "event-b": mean * 1.5},
                    "mean_normalized_squared_residual": mean,
                    "weighted_contribution": mean / len(selected)})
            candidates.append({"id": f"receiver-{index:05}", "observer": observer,
                "rank": index + 1, "score": sum(t["weighted_contribution"] for t in residuals),
                "terms": residuals, "unavailable_reasons": [], "location_verified": False})
        result["modes"].append({"mode": mode, "status": "scored", "error": None,
            "document_invariant_sha256": result["document_invariant_sha256"],
            "report": {"config": config, "selected_terms": selected,
                "candidates": candidates,
                "ranked_candidate_ids": [c["id"] for c in candidates],
                "calibration": "uncalibrated", "location_verified": False}})
    return batch, report, input_identity, scorer_identity, snapshot


class IndependentOracleTests(unittest.TestCase):
    def test_wgs84_known_axes(self):
        self.assertEqual(BENCH.ecef(0, 0, 0), [6378137.0, 0.0, 0.0])
        east = BENCH.ecef(0, 90, 123)
        self.assertAlmostEqual(east[0], 0.0, places=8)
        self.assertAlmostEqual(east[1], 6378260.0, places=8)
        pole = BENCH.ecef(90, 0, 0)
        self.assertAlmostEqual(pole[0], 0.0, places=8)
        self.assertAlmostEqual(pole[2], 6356752.314245179, places=7)

    def test_wgs84_inverse_round_trip(self):
        for point in ((0, 0, 80), (46.8, -71.2, 1500), (-51.4, 170.2, -20),
                      (80, -179, 10000)):
            with self.subTest(point=point):
                actual = BENCH.geo(BENCH.ecef(*point))
                self.assertAlmostEqual(actual[0], point[0], places=9)
                self.assertAlmostEqual(actual[1], point[1], places=9)
                self.assertAlmostEqual(actual[2], point[2], places=6)

    def test_retarded_arrival_and_doppler_have_analytic_values(self):
        p0, velocity, receiver = [1000, 0, 0], [0, 100, 0], [0, 0, 0]
        at_cpa = BENCH.oracle_sample(p0, velocity, receiver, 0, 340, 100)
        self.assertAlmostEqual(at_cpa[0], 1000 / 340)
        self.assertEqual(at_cpa[1], 100)
        receding = BENCH.oracle_sample(p0, velocity, receiver, 10, 340, 100)
        self.assertAlmostEqual(receding[0], 10 + 1000 * math.sqrt(2) / 340)
        self.assertAlmostEqual(receding[1], 100 / (1 + 100 / (math.sqrt(2) * 340)))
        approaching = BENCH.oracle_sample(p0, velocity, receiver, -10, 340, 100)
        self.assertGreater(approaching[1], 100)
        self.assertLess(receding[1], 100)
        self.assertAlmostEqual(BENCH.oracle_cpa(p0, velocity, receiver, 340), 1000 / 340)

    def test_cpa_uses_emission_then_propagation(self):
        # The source reaches its closest point at t=20; the sound arrives later.
        arrival = BENCH.oracle_cpa([-2000, 600, 0], [100, 0, 0], [0, 0, 0], 300)
        self.assertEqual(arrival, 22.0)

    def test_horizontal_error_known_equatorial_arc(self):
        a = {"latitude_deg": 0, "longitude_deg": 0}
        b = {"latitude_deg": 0, "longitude_deg": 1}
        self.assertAlmostEqual(BENCH.horizontal_error(a, b), 111195.0802335329, places=6)
        self.assertEqual(BENCH.horizontal_error(a, b), BENCH.horizontal_error(b, a))
        self.assertEqual(BENCH.horizontal_error(a, a), 0)


class SceneIntegrityTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.protocol = test_protocol()
        cls.batches, cls.labels = BENCH.generate(cls.protocol)

    def test_seeded_generation_is_deterministic_and_local(self):
        state = random.getstate()
        self.assertEqual(BENCH.generate(self.protocol), (self.batches, self.labels))
        self.assertEqual(random.getstate(), state)

    def test_synthetic_source_class_is_explicit_even_with_misleading_associations(self):
        for cases in self.batches.values():
            for case in cases:
                for event in case["document"]["events"]:
                    self.assertEqual(event["acoustic_source"], "compatible")
                    self.assertIn("synthetic", event["origin"])
                    self.assertIn("no real audio", event["origin"])

    def test_disjoint_splits_counts_and_off_grid_truth(self):
        self.assertEqual({k: len(v) for k, v in self.batches.items()},
                         {"development": 4, "holdout": 8, "control": 8})
        self.assertEqual(len({x["id"] for x in self.labels}), 20)
        seeds = {split: {x["seed"] for x in self.labels if x["split"] == split}
                 for split in ("development", "holdout")}
        self.assertTrue(seeds["development"].isdisjoint(seeds["holdout"]))
        lookup = {x["id"]: x for x in self.labels}
        for case in self.batches["development"] + self.batches["holdout"]:
            self.assertEqual(len(case["observers"]), 25)
            truth = lookup[case["id"]]["observer"]
            self.assertGreater(min(BENCH.horizontal_error(truth, o) for o in case["observers"]), 0.01)
            center = case["observers"][12]
            self.assertGreater(BENCH.horizontal_error(truth, center), 0.01)
            self.assertLess(BENCH.horizontal_error(truth, center), 285)

    def test_scorer_inputs_exclude_evaluation_labels(self):
        forbidden = {"seed", "split", "family", "base_id", "true_sound_speed_mps",
                     "truth", "anchor", "control"}
        def inspect(value):
            if isinstance(value, dict):
                self.assertTrue(forbidden.isdisjoint(value))
                for item in value.values():
                    inspect(item)
            elif isinstance(value, list):
                for item in value:
                    inspect(item)
        for cases in self.batches.values():
            for case in cases:
                self.assertEqual(set(case), {"id", "document", "map", "lightning", "observers"})
                inspect(case)

    def test_controls_preserve_truth_and_original_observations(self):
        base = self.batches["holdout"][0]
        label = next(x for x in self.labels if x["id"] == base["id"])
        before = deepcopy(base)
        for name in self.protocol["controls"]:
            changed, truth = BENCH.control(self.protocol, base, label, name)
            self.assertEqual(truth["observer"], label["observer"])
            self.assertEqual(truth["base_id"], base["id"])
            self.assertNotEqual(changed["id"], base["id"])
            self.assertEqual(base, before)
            for original, event in zip(base["document"]["events"], changed["document"]["events"]):
                self.assertEqual(original["arrival_s"], event["arrival_s"])
                self.assertEqual(original["points"], event["points"])
            if name != "outside_region":
                self.assertEqual(changed["observers"], base["observers"])

    def test_label_contract_accepts_complete_test_splits(self):
        BENCH.validate_labels(self.protocol, self.labels)

    def test_label_contract_rejects_missing_duplicate_and_changed_truth_metadata(self):
        mutations = [lambda rows: rows.pop(),
                     lambda rows: rows.__setitem__(1, deepcopy(rows[0])),
                     lambda rows: rows[0].update(split="holdout"),
                     lambda rows: rows[0].update(seed=0),
                     lambda rows: rows[-1].update(base_id="unrelated")]
        for mutate in mutations:
            with self.subTest(mutation=mutations.index(mutate)):
                rows = deepcopy(self.labels)
                mutate(rows)
                with self.assertRaises(ValueError):
                    BENCH.validate_labels(self.protocol, rows)


class OutcomeAccountingTests(unittest.TestCase):
    def setUp(self):
        self.protocol = test_protocol()

    def test_missing_results_stay_in_success_denominator(self):
        def row(errors):
            return {"modes": {mode: {"error_m": error, "low_score_wrong": False,
                                    "location_verified": False}
                              for mode, error in zip(BENCH.MODES, errors)}}
        rows = [row((100, 900, 50)), row((None, 100, None)), row((600, None, 800))]
        report = BENCH.aggregate(rows, self.protocol)
        for mode in BENCH.MODES:
            result = report["modes"][mode]
            self.assertEqual(result["success_denominator"], 3)
            self.assertEqual(result["success_counts"]["500"], 1)
            self.assertEqual(result["ranked"], 2)
            self.assertEqual(result["no_rankable_or_error"], 1)
        self.assertEqual(report["combined_vs_doppler"]["paired_count"], 2)
        empty = BENCH.aggregate([row((None, None, None))], self.protocol)
        self.assertEqual(empty["combined_vs_doppler"], {"paired_count": 0})
        self.assertIsNone(empty["modes"]["combined"]["mean_error_m"])
        self.assertEqual(empty["modes"]["combined"]["success_denominator"], 1)
        self.assertEqual(empty["modes"]["combined"]["success_counts"]["500"], 0)

    def test_paired_bootstrap_is_repeatable_and_uses_difference_of_medians(self):
        rows = [{"modes": {"combined": {"error_m": a}, "doppler_only": {"error_m": b}}}
                for a, b in ((1, 2), (2, 100), (100, 101))]
        state = random.getstate()
        report = BENCH.paired_statistics(rows, "doppler_only", self.protocol)
        self.assertEqual(report, BENCH.paired_statistics(rows, "doppler_only", self.protocol))
        self.assertEqual(random.getstate(), state)
        self.assertEqual(report["median_error_difference_m"], -98)
        self.assertEqual(report["combined_wins"], 1)
        self.assertEqual(report["ties_within_1m"], 2)
        self.assertEqual(report["sign_test_two_sided_p"], 1)

    def test_primary_gate_requires_full_paired_coverage(self):
        _, labels = BENCH.generate(self.protocol)
        predictions = evaluation_predictions(labels)
        complete = BENCH.evaluate(self.protocol, labels, predictions)
        self.assertTrue(complete["predeclared_primary_synthetic_improvement"])
        primary = next(label for label in labels if label["split"] == "holdout"
                       and label["family"] in self.protocol["primary_families"])
        case = next(case for case in predictions[0]["cases"] if case["id"] == primary["id"])
        mode = next(mode for mode in case["modes"] if mode["mode"] == "combined")
        mode.update(status="unavailable", error="Fixture missing evidence", report=None)
        incomplete = BENCH.evaluate(self.protocol, labels, predictions)
        self.assertFalse(incomplete["predeclared_primary_synthetic_improvement"])
        self.assertEqual(incomplete["primary"]["modes"]["combined"]["success_denominator"], 4)
        self.assertEqual(incomplete["primary"]["combined_vs_doppler"]["paired_count"], 3)
        self.assertFalse(incomplete["real_world_accuracy_improvement_established"])
        self.assertFalse(incomplete["production_confidence_calibrated"])

    def test_evaluation_rejects_missing_duplicate_and_unknown_prediction_ids(self):
        _, labels = BENCH.generate(self.protocol)
        for change in ("missing", "duplicate", "unknown"):
            predictions = evaluation_predictions(labels)
            cases = predictions[0]["cases"]
            if change == "missing":
                cases.pop()
            elif change == "duplicate":
                cases.append(deepcopy(cases[0]))
            else:
                cases[0]["id"] = "unknown"
            with self.subTest(change=change), self.assertRaises(ValueError):
                BENCH.evaluate(self.protocol, labels, predictions)

    def test_low_score_wrong_is_only_a_diagnostic(self):
        _, labels = BENCH.generate(self.protocol)
        report = BENCH.evaluate(self.protocol, labels, evaluation_predictions(labels))
        doppler = report["primary"]["modes"]["doppler_only"]
        self.assertEqual(doppler["misleading_low_score_wrong"], 4)
        self.assertEqual(doppler["verified_locations"], 0)
        self.assertFalse(report["production_confidence_calibrated"])


class PredictionContractTests(unittest.TestCase):
    def test_accepts_consistent_crafted_report(self):
        BENCH.validate_prediction(*validator_fixture())

    def test_rejects_changed_identity_sources_modes_and_assumptions(self):
        for change in ("input", "binary", "source", "mode", "duplicate_mode", "invariant", "weight", "labels"):
            args = validator_fixture()
            batch, report, _, _, _ = args
            mode = report["cases"][0]["modes"][0]
            if change == "input":
                report["input"] = {"bytes": 124, "sha256": "1" * 64}
            elif change == "binary":
                report["executable"] = {"bytes": 457, "sha256": "2" * 64}
            elif change == "source":
                report["compiled_source_sha256"]["src/physics.rs"] = "0" * 64
            elif change == "mode":
                report["cases"][0]["modes"].pop()
            elif change == "duplicate_mode":
                report["cases"][0]["modes"][1] = deepcopy(mode)
            elif change == "invariant":
                mode["document_invariant_sha256"] = "5" * 64
            elif change == "weight":
                mode["report"]["config"]["frequency"]["weight"] = 2.0
            else:
                batch["cases"][0]["truth"] = {"latitude_deg": 0}
            with self.subTest(change=change), self.assertRaises(ValueError):
                BENCH.validate_prediction(*args)

    def test_rejects_grid_rank_score_and_verification_changes(self):
        for change in ("grid", "missing_candidate", "rank", "rank_ids", "nan", "sum", "verified", "term"):
            args = validator_fixture()
            scored = args[1]["cases"][0]["modes"][0]["report"]
            candidate = scored["candidates"][0]
            if change == "grid":
                candidate["observer"] = {**candidate["observer"], "latitude_deg": 1.0}
            elif change == "missing_candidate":
                scored["candidates"].pop()
            elif change == "rank":
                candidate["rank"] = 2
            elif change == "rank_ids":
                scored["ranked_candidate_ids"].reverse()
            elif change == "nan":
                candidate["score"] = float("nan")
            elif change == "sum":
                candidate["score"] += 1.0
            elif change == "verified":
                candidate["location_verified"] = True
            else:
                candidate["terms"][0]["event_mean_squared_residuals"]["event-a"] += 1
            with self.subTest(change=change), self.assertRaises(ValueError):
                BENCH.validate_prediction(*args)

    def test_unavailable_mode_requires_reason_and_no_report(self):
        args = validator_fixture()
        mode = args[1]["cases"][0]["modes"][2]
        mode.update(status="unavailable", error="Missing evidence", report=None)
        BENCH.validate_prediction(*args)
        mode["error"] = None
        with self.assertRaises(ValueError):
            BENCH.validate_prediction(*args)

    def test_shared_residuals_cannot_change_with_evidence_mode(self):
        args = validator_fixture()
        combined = args[1]["cases"][0]["modes"][2]["report"]
        candidate = combined["candidates"][0]
        term = candidate["terms"][0]
        term["event_mean_squared_residuals"] = {"event-a": 2.0, "event-b": 2.0}
        term["mean_normalized_squared_residual"] = 2.0
        term["weighted_contribution"] = 2.0 / 3
        candidate["score"] = sum(t["weighted_contribution"] for t in candidate["terms"])
        with self.assertRaisesRegex(ValueError, "Common term changed"):
            BENCH.validate_prediction(*args)

    def test_exclusive_artifact_writes_preserve_existing_bytes(self):
        with tempfile.TemporaryDirectory(prefix=".hybrid-test-", dir=ROOT) as directory:
            self.assertTrue(Path(directory).resolve().is_relative_to(ROOT.resolve()))
            path = Path(directory) / "frozen.json"
            BENCH.write_json(path, {"value": 1})
            original = path.read_bytes()
            with self.assertRaises(FileExistsError):
                BENCH.write_json(path, {"value": 2})
            self.assertEqual(path.read_bytes(), original)
            self.assertEqual(BENCH.identity(path)["bytes"], len(original))
            with self.assertRaises(ValueError):
                BENCH.encoded({"invalid": float("nan")})


if __name__ == "__main__":
    unittest.main()
