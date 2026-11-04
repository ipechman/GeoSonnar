"""Checks the acquired local real-reference artifacts; skips when absent."""
import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "analysis"))
from field_validation_native import digest
from field_validation_evaluate import distance


class LocalFieldArtifacts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.native = ROOT / "output/field-validation-reference-001"
        cls.prepared = ROOT / "output/field-validation-reference-prepared-001"
        if not (cls.prepared / "batch.json").exists():
            raise unittest.SkipTest("Acquire and prepare the optional real reference first")
        cls.case = json.loads((cls.prepared / "batch.json").read_text())["cases"][0]
        cls.discovery = json.loads((cls.native / "discovery/discovery.json").read_text())

    def test_complete_native_frequency_observations_are_unchanged(self):
        measured = {event["event_id"]: event for event in self.discovery["events"]}
        selected = {event["id"]: event for event in self.case["document"]["events"]}
        self.assertEqual(set(measured), set(selected))
        for identifier, event in selected.items():
            expected = [{key: point[key] for key in ("time_s", "frequency_hz")} for point in measured[identifier]["ridge"]]
            self.assertEqual(event["points"], expected)
            self.assertEqual(event["frequency_sigma_hz"], measured[identifier]["quality"]["frequency_sigma_hz"])
            self.assertFalse(event["diagnostics"]["human_review_completed"])
        self.assertFalse(self.case["document"]["mix"]["doppler_timing"])
        self.assertFalse(self.case["document"]["mix"]["thunder_timing"])

    def test_every_native_aircraft_pass_and_ecef_point_is_preserved(self):
        expected = {}
        for path in (self.native / "normalized-traces").glob("*.json"):
            report = json.loads(path.read_text())
            for item in report["passes"]:
                expected[item["track"]["flight_id"]] = item["track"]
        actual = {track["id"]: track for track in self.case["map"]["tracks"]}
        self.assertEqual(set(actual), set(expected))
        for identifier, track in actual.items():
            self.assertEqual(track["physics_track"], expected[identifier])
            self.assertEqual(track["segments"], [[0, len(expected[identifier]["points"])]])
            self.assertEqual(track["max_gap_s"], 30)
        for choice in self.case["plan"]["events"]:
            self.assertEqual(set(choice["source_ids"]), set(expected))

    def test_operator_review_is_pending_and_frozen_policy_matches(self):
        pending = json.loads((self.prepared / "pending-workspace.json").read_text())
        self.assertTrue(all(event["review"] == "pending" and not event["selected_for_scoring"] for event in pending["events"]))
        receipt = json.loads((self.prepared / "preparation.json").read_text())
        self.assertEqual(digest(self.prepared / "batch.json"), receipt["batch_sha256"])
        self.assertFalse(receipt["receiver_truth_read"])
        self.assertEqual(len(self.case["observers"]), 288)
        self.assertTrue(all(item["ellipsoid_height_m"] == 200 for item in self.case["observers"]))


class PostScoreDistanceChecks(unittest.TestCase):
    def test_known_geodesic_symmetry_identity_and_quarter_equator(self):
        self.assertEqual(distance((43, -72), (43, -72)), 0)
        self.assertAlmostEqual(distance((0, 0), (0, 90)), 10007557.221017962)
        self.assertAlmostEqual(distance((43, -72), (44, -71)), distance((44, -71), (43, -72)))


class LocalHoldoutTiling(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.folder = ROOT / "output/field-validation-holdout-tiles-001"
        if not (cls.folder / "tiling.json").exists():
            raise unittest.SkipTest("Acquire and prepare the optional time holdout first")
        cls.receipt = json.loads((cls.folder / "tiling.json").read_text())
        cls.original = json.loads(Path(cls.receipt["original_input_path"]).read_text())["cases"][0]
        cls.grouped = json.loads((cls.folder / "grouped-full-request.json").read_text())["cases"][0]

    def test_all_original_segments_keep_exact_points_and_gaps(self):
        original = {track["id"]: track for track in self.original["map"]["tracks"]}
        grouped = {track["id"]: track for track in self.grouped["map"]["tracks"]}
        self.assertEqual(len(self.receipt["source_grouping"]), len(original))
        for entry in self.receipt["source_grouping"]:
            before = original[entry["original_pass_id"]]
            after = grouped[entry["grouped_source_id"]]
            start, end = entry["grouped_point_range"]
            self.assertEqual(after["physics_track"]["points"][start:end], before["physics_track"]["points"])
            self.assertEqual(after["points"][start:end], before["points"])
            self.assertEqual(after["max_gap_s"], before["max_gap_s"])
            for segment in entry["grouped_segments"]:
                self.assertIn(segment, after["segments"])
        self.assertEqual(sum(len(t["segments"]) for t in grouped.values()), 546)
        self.assertEqual(sum(len(t["physics_track"]["points"]) for t in grouped.values()), 11956)

    def test_disjoint_tiles_cover_same_receivers_and_complete_evidence(self):
        observers = []
        for tile in self.receipt["tiles"]:
            path = self.folder / tile["input"]
            self.assertEqual(digest(path), tile["sha256"])
            case = json.loads(path.read_text())["cases"][0]
            self.assertEqual(case["document"]["events"], self.original["document"]["events"])
            self.assertEqual(case["plan"], self.grouped["plan"])
            self.assertEqual(case["map"], self.grouped["map"])
            observers.extend(case["observers"])
        self.assertEqual(observers, self.original["observers"])
        self.assertEqual(len({tuple(p.values()) for p in observers}), len(observers))


if __name__ == "__main__":
    unittest.main()
