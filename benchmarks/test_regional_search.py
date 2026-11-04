"""Control the fresh synthetic uncertain-clock/source benchmark protocol."""
import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "analysis"))
import benchmark_regional_search as benchmark


class RegionalHarnessTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.protocol = json.loads(benchmark.PROTOCOL.read_text())
        cls.batches, cls.labels = benchmark.generated(cls.protocol)

    def test_deterministic_fresh_inputs_and_separate_truth(self):
        self.assertEqual(benchmark.generated(self.protocol), (self.batches, self.labels))
        historic = json.loads((ROOT/"benchmarks/hybrid_ablation_protocol.json").read_text())
        self.assertNotEqual(historic["generator_seed"], self.protocol["generator_seed"])
        for mode in benchmark.MODES:
            for case in self.batches[mode]["cases"]:
                self.assertEqual(set(case), {"id", "document", "map", "lightning", "observers", "plan"})
                self.assertNotIn("true_clock_offset_s", case["document"])
                self.assertNotIn("observer", case)

    def test_measurements_and_sources_identical_between_modes(self):
        combined = {case["id"]: case for case in self.batches["combined"]["cases"]}
        for mode, batch in self.batches.items():
            for case in batch["cases"]:
                other = combined[case["id"]]
                for field in ("map", "lightning", "observers"):
                    self.assertEqual(case[field], other[field])
                self.assertEqual(case["document"]["events"], other["document"]["events"])
                self.assertEqual(case["plan"]["clock"], other["plan"]["clock"])
                for option in case["plan"]["events"]:
                    self.assertIn(option, other["plan"]["events"])

    def test_no_association_or_emitted_tone_is_preselected(self):
        for case in self.batches["combined"]["cases"]:
            for event in case["document"]["events"]:
                self.assertEqual(event["flight_id"], "")
                self.assertEqual(event["strike_id"], "")
                self.assertEqual(event["emitted_frequency_hz"], 0)
            for option in case["plan"]["events"]:
                self.assertGreater(len(option["source_ids"]), 1)
                self.assertTrue(all(len(source) == 16 for source in option["source_ids"]))

    def test_controls_are_declared_and_true_clock_can_be_excluded(self):
        labels = {label["id"]: label for label in self.labels}
        self.assertEqual({label["control"] for label in self.labels if label["control"]}, set(self.protocol["controls"]))
        for case in self.batches["combined"]["cases"]:
            if labels[case["id"]]["control"] == "wrong_clock_range":
                self.assertLess(labels[case["id"]]["true_clock_offset_s"], case["plan"]["clock"]["min_offset_s"])

    def test_analytic_truth_is_on_grid_and_observations_stay_in_recording(self):
        labels = {label["id"]: label for label in self.labels}
        for case in self.batches["combined"]["cases"]:
            label = labels[case["id"]]
            if label["analytic"]:
                self.assertIn(label["observer"], case["observers"])
            for event in case["document"]["events"]:
                self.assertTrue(0 <= event["arrival_s"] <= case["document"]["duration_s"])
                self.assertTrue(all(0 <= p["time_s"] <= case["document"]["duration_s"] for p in event["points"]))

    def test_missing_modes_and_outcomes_cannot_be_counted_as_passes(self):
        with self.assertRaises(ValueError):
            benchmark.evaluate(self.protocol, self.labels, {}, self.batches)
        with self.assertRaises(ValueError):
            benchmark.evaluate(self.protocol, self.labels, dict.fromkeys(benchmark.MODES, []), self.batches)


if __name__ == "__main__":
    unittest.main()
