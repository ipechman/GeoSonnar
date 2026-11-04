"""Source-gate compatibility for explicit unknown-tone field experiments."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "analysis"))
from field_validation_prepare import experiment_workspace


class ExperimentSourceReviewTests(unittest.TestCase):
    def setUp(self):
        self.pending = {"notes": "Unreviewed operator copy", "events": [{
            "id": "unidentified-tone", "kind": "doppler", "review": "pending",
            "selected_for_scoring": False,
            "points": [{"time_s": 12.0, "frequency_hz": 100.0}],
            "diagnostics": {"aircraft_identity_confirmed": False}}]}

    def test_experiment_preserves_original_and_exposes_source_uncertainty(self):
        original = deepcopy(self.pending)
        result = experiment_workspace(self.pending, "Frozen numerical experiment", "experiment_only_inclusion")
        self.assertEqual(self.pending, original)
        event = result["events"][0]
        self.assertEqual(event["points"], original["events"][0]["points"])
        self.assertEqual(event["review"], "confirmed")
        self.assertTrue(event["selected_for_scoring"])
        self.assertFalse(event["diagnostics"]["aircraft_identity_confirmed"])
        rationale = event["acoustic_source"]["uncertain_hypothesis"]["rationale"]
        self.assertIn("unidentified", rationale)
        self.assertIn("unverified", rationale)
        self.assertNotIn("acoustic_source", self.pending["events"][0])
        event["points"][0]["frequency_hz"] = 200.0
        self.assertEqual(self.pending, original)

    def test_existing_source_decision_is_never_replaced(self):
        for assessment in ("compatible", "incompatible", {"uncertain_hypothesis": {"rationale": "Existing interpretation"}}):
            with self.subTest(assessment=assessment):
                self.pending["events"][0]["acoustic_source"] = assessment
                original = deepcopy(self.pending)
                with self.assertRaisesRegex(ValueError, "cannot replace"):
                    experiment_workspace(self.pending, "Experiment", "experiment_only_inclusion")
                self.assertEqual(self.pending, original)

    def test_existing_measurement_decision_is_never_replaced(self):
        for field, value in (("review", "rejected"), ("review", "confirmed"), ("selected_for_scoring", True)):
            with self.subTest(field=field, value=value):
                document = deepcopy(self.pending)
                document["events"][0][field] = value
                with self.assertRaisesRegex(ValueError, "pending, unselected"):
                    experiment_workspace(document, "Experiment", "experiment_only_inclusion")


if __name__ == "__main__":
    unittest.main()
