"""Independent signal controls for the audio-only research review helper."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

import numpy as np
from scipy.io import wavfile

PATH = Path(__file__).resolve().parents[1] / "analysis/audio_review_measurements.py"
SPEC = importlib.util.spec_from_file_location("audio_review_measurements", PATH)
REVIEW = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(REVIEW)


class ReviewMeasurements(unittest.TestCase):
    def test_independent_chirp_preserves_sample_times_and_measures_frequency(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "chirp.wav"
            rate = 8000
            times = np.arange(30 * rate) / rate
            # Derivative of this independent analytic phase is 550 - 0.7*t Hz.
            phase = 2 * np.pi * (550 * times - 0.35 * times**2)
            samples = 0.4 * np.cos(phase) + np.random.default_rng(7241).normal(0, 0.01, len(times))
            wavfile.write(path, rate, (samples * 32767).astype(np.int16))
            frequency, frames, db, source = REVIEW.spectrum(path)
            result = REVIEW.measure(frequency, frames, db, {
                "anchors": [[2, 548.6], [26, 531.8]], "measurement_step_s": 2})
            self.assertEqual(source["sha256"], REVIEW.digest(path))
            self.assertEqual(len(result["points"]), 13)
            self.assertEqual(result["review"], "pending")
            self.assertFalse(result["selected_for_scoring"])
            self.assertFalse(result["aircraft_identity_confirmed"])
            for point in result["points"]:
                self.assertLess(abs(point["frequency_hz"] - (550 - 0.7 * point["time_s"])), 0.15)
                frame_index = (point["time_s"] - 1.024) / 0.5
                self.assertAlmostEqual(frame_index, round(frame_index), places=10)

    def test_silence_is_not_a_confirmed_or_measured_tone(self):
        frequency = np.arange(0, 2000, 0.5)
        times = np.arange(1.024, 10, 0.5)
        db = np.full((len(frequency), len(times)), -200.0)
        result = REVIEW.measure(frequency, times, db, {"anchors": [[2, 500], [8, 480]]})
        self.assertFalse(result["points"])
        self.assertTrue(result["rejected"])

    def test_ambiguous_channel_and_float_source_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            for samples in (np.zeros((24000, 2), dtype=np.int16), np.zeros(24000, dtype=np.float32)):
                path = Path(directory) / "unsupported.wav"
                wavfile.write(path, 8000, samples)
                with self.assertRaisesRegex(ValueError, "mono 16-bit or 32-bit PCM"):
                    REVIEW.spectrum(path)

    def test_equivalent_pcm16_and_pcm32_keep_frequency_scale_and_original_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            rate = 8000
            times = np.arange(16 * rate) / rate
            samples = (12000 * np.cos(2 * np.pi * (530 * times - 0.1 * times**2))).astype(np.int16)
            paths = [Path(directory) / "pcm16.wav", Path(directory) / "pcm32.wav"]
            wavfile.write(paths[0], rate, samples)
            wavfile.write(paths[1], rate, samples.astype(np.int32) * 65536)
            before = [path.read_bytes() for path in paths]
            results = [REVIEW.spectrum(path) for path in paths]
            np.testing.assert_array_equal(results[0][0], results[1][0])
            np.testing.assert_array_equal(results[0][1], results[1][1])
            prominent = results[0][2] > -50
            np.testing.assert_allclose(results[0][2][prominent], results[1][2][prominent], atol=0.001, rtol=0)
            config = {"anchors": [[2, 529.6], [12, 527.6]], "measurement_step_s": 1}
            measurements = [REVIEW.measure(*result[:3], config) for result in results]
            self.assertEqual(len(measurements[0]["points"]), 11)
            self.assertEqual(len(measurements[1]["points"]), 11)
            for a, b in zip(measurements[0]["points"], measurements[1]["points"]):
                self.assertEqual(a["time_s"], b["time_s"])
                self.assertLess(abs(a["frequency_hz"] - b["frequency_hz"]), 0.0001)
            self.assertEqual([r[3]["pcm_container_bits"] for r in results], [16, 32])
            self.assertEqual(before, [path.read_bytes() for path in paths])

    def test_quiet_pcm32_tone_below_pcm16_resolution_is_measured(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "quiet.wav"
            rate = 8000
            times = np.arange(12 * rate) / rate
            samples = (2e-6 * 2147483648 * np.cos(2 * np.pi * 537.3 * times)).astype(np.int32)
            self.assertTrue(np.all(np.abs(samples) < 65536))
            wavfile.write(path, rate, samples)
            frequency, frames, db, _ = REVIEW.spectrum(path)
            result = REVIEW.measure(frequency, frames, db, {"anchors": [[2, 537.3], [8, 537.3]]})
            self.assertEqual(len(result["points"]), 4)
            for point in result["points"]:
                self.assertLess(abs(point["frequency_hz"] - 537.3), 0.03)
            self.assertFalse(result["selected_for_scoring"])

    def test_reversed_or_nonfinite_anchors_cannot_generate_measurements(self):
        frequency = np.arange(0, 2000, 0.5)
        times = np.arange(1.024, 10, 0.5)
        db = np.full((len(frequency), len(times)), -100.0)
        for anchors in ([[3, 500], [2, 480]], [[2, 500], [8, float("nan")]], [[0, 500], [8, 480]]):
            with self.assertRaises(ValueError):
                REVIEW.measure(frequency, times, db, {"anchors": anchors})


if __name__ == "__main__":
    unittest.main()
