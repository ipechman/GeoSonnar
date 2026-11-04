"""Offline checks of field acquisition boundaries and replay wire decoding."""
import gzip
import hashlib
import io
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "analysis"))
import field_validation_acquire as audio
import field_validation_tracks as tracks


class MemoryBudget:
    def __init__(self, data): self.data, self.ranges = data, []
    def fetch(self, url, size, start, total):
        assert total == len(self.data)
        assert 0 <= start < total and start + size <= total
        self.ranges.append((start, size))
        return self.data[start:start + size]


class AcquisitionTests(unittest.TestCase):
    def test_seekable_remote_zip_preserves_actual_member_bytes(self):
        content = bytes(range(256)) * 1300
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, "w", zipfile.ZIP_DEFLATED) as bundle:
            bundle.writestr("field/source.wav", content)
        encoded = stream.getvalue()
        budget = MemoryBudget(encoded)
        with zipfile.ZipFile(audio.RangeReader("memory", len(encoded), budget)) as bundle:
            self.assertEqual(bundle.read("field/source.wav"), content)
        self.assertTrue(budget.ranges)

    def test_remote_reader_refuses_unbounded_reads_and_invalid_seeks(self):
        remote = audio.RangeReader("memory", 2 * audio.MAX_TRANSFER, MemoryBudget(b""))
        with self.assertRaises(ValueError): remote.read()
        with self.assertRaises(ValueError): remote.seek(-1)
        with self.assertRaises(ValueError): remote.seek(remote.size + 1)

    def test_cached_ranges_must_match_original_hash_receipt(self):
        temporary_root = ROOT / "tmp"
        temporary_root.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="field-validation-test-", dir=temporary_root) as folder:
            self.assertTrue(Path(folder).resolve().is_relative_to(temporary_root.resolve()))
            cache = Path(folder) / "ranges"
            cache.mkdir()
            key = audio.sha256(b"https://example.invalid/file\n0\n4")
            (cache / (key + ".bin")).write_bytes(b"evil")
            (cache / (key + ".sha256")).write_text(audio.sha256(b"good"))
            budget = audio.Budget(cache=cache)
            with self.assertRaisesRegex(ValueError, "receipt"):
                budget.fetch("https://example.invalid/file", 4, 0, 4)

    def test_audio_header_metadata_is_preserved_and_bounded(self):
        text = b"Recorded at 04:24:53 08/04/2024 (UTC)"
        prefix = b"RIFF" + struct.pack("<I", 1000) + b"WAVE" + b"LIST" + struct.pack("<I", len(text)) + text + b"\0" * (len(text) % 2) + b"data" + struct.pack("<I", 100)
        self.assertIn("UTC", audio.riff_metadata(prefix)[0]["text"])
        with self.assertRaises(ValueError): audio.riff_metadata(prefix[:25])


class ReplayTests(unittest.TestCase):
    @staticmethod
    def fixture(legacy=True):
        timestamp = 1712548800000
        marker = struct.pack("<4I", tracks.MAGIC, timestamp >> 32, timestamp & 0xffffffff, 10000)
        position = struct.pack("<IiiI", 0xabc123, 44000000, -72000000, (1234 << 16) | 120)
        header = struct.pack("<4I", 1, 0, 0, 0) if legacy else b""
        return header + marker + position

    def test_legacy_index_and_current_layout_match(self):
        results = [tracks.replay_records(self.fixture(legacy), 1712548800, 1712548810,
                                        [42, -74, 46, -70]) for legacy in (True, False)]
        self.assertEqual(results[0], results[1])
        row = results[0][0][0]
        self.assertEqual(row["icao24"], "abc123")
        self.assertEqual(row["barometric_altitude_ft"], 3000)
        self.assertEqual(row["groundspeed_kt"], 123.4)
        self.assertEqual(row["timestamp"], 1712548800)

    def test_missing_corrupt_or_truncated_markers_are_rejected(self):
        with self.assertRaises(ValueError): tracks.replay_records(b"\0" * 32, 0, 1, [-90,-180,90,180])
        data = bytearray(self.fixture()); struct.pack_into("<I", data, 0, 999)
        with self.assertRaises(ValueError): tracks.replay_records(data, 0, 1, [-90,-180,90,180])
        with self.assertRaises(ValueError): tracks.replay_records(self.fixture()[:-1], 0, 1, [-90,-180,90,180])

    def test_time_and_region_filters_do_not_invent_absence(self):
        rows, info = tracks.replay_records(self.fixture(), 0, 1, [42, -74, 46, -70])
        self.assertEqual(rows, [])
        self.assertEqual(info["slices"], 1)

    def test_gzip_output_is_bounded_and_complete(self):
        data = gzip.compress(b"x" * 100000)
        with self.assertRaises(ValueError): tracks.decoded(data, 100)
        with self.assertRaises(ValueError): tracks.decoded(data[:-3])
        self.assertEqual(tracks.decoded(data), b"x" * 100000)


if __name__ == "__main__": unittest.main()
