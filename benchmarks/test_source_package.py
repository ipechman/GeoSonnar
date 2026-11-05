"""Publication controls using fabricated source trees and no private recordings."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import tempfile
import unittest
from unittest.mock import patch
import zipfile


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("package_source", ROOT / "analysis/package_source.py")
packager = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(packager)


class SourcePackageControls(unittest.TestCase):
    def setUp(self):
        parent = ROOT / "output/benchmarks/source-package-controls"
        parent.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix="control-", dir=parent)
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name).resolve()
        self.assertTrue(self.base.is_relative_to(parent.resolve()))
        self.root = self.base / "source"
        self.root.mkdir()
        for name in packager.TOP_FILES:
            (self.root / name).write_text("Fabricated source fixture\n", encoding="utf-8")
        for name in packager.SOURCE_DIRS:
            (self.root / name).mkdir()
        (self.root / "src/lib.rs").write_bytes(b"// Fabricated source\r\n")
        self.output = self.base / "export"
        self.archive = self.base / "source.zip"

    def write(self, relative, data=b"Excluded fabricated input\n"):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return path

    def test_export_allowlist_excludes_private_files_without_exposing_their_names(self):
        excluded = [
            "data/ordinary.json", "output/result.json", "dist/old.rs", "target/build.rs",
            "Original Video/clip.mp4", "Known Videos with coordinates/clip.mp4",
            "Aircraft_Audio_Replication_Guide.pdf", "unlisted-root.txt",
            "cases/private-case.json", "cases/demo.private.json", "docs/.env.local",
            "analysis/credentials.json", "analysis/__pycache__/compiled.pyc",
            "benchmarks/private-validation/case.json", "tests/recordings/clip.txt",
            "src/embedded.mp4", "src/old.exe", "docs/archive.zip",
            packager.EXCLUDED_RECEIPT,
        ]
        for relative in excluded:
            self.write(relative)
        self.write("requirements-research.txt", b"numpy\n")
        self.write("benchmarks/test_private_audio_harness.py", b"# Fabricated tests are publishable\n")
        manifest = packager.package(self.output, self.archive, root=self.root)
        names = {entry["path"] for entry in manifest["files"]}
        self.assertIn("requirements-research.txt", names)
        self.assertIn("benchmarks/test_private_audio_harness.py", names)
        for relative in excluded:
            self.assertNotIn(relative, names)
            self.assertNotIn(relative, (self.output / packager.MANIFEST).read_text(encoding="utf-8"))
        with zipfile.ZipFile(self.archive) as archive:
            self.assertEqual(set(archive.namelist()), {"GeoSonnar/" + name for name in names | {packager.MANIFEST}})
            for name in names | {packager.MANIFEST}:
                self.assertEqual(archive.read("GeoSonnar/" + name), (self.output / name).read_bytes())

    def test_manifest_preserves_source_bytes_and_detects_tampering(self):
        manifest = packager.package(self.output, root=self.root)
        entry = next(item for item in manifest["files"] if item["path"] == "src/lib.rs")
        data = (self.root / "src/lib.rs").read_bytes()
        self.assertEqual(entry["bytes"], len(data))
        self.assertEqual(entry["sha256"], hashlib.sha256(data).hexdigest())
        self.assertEqual((self.output / "src/lib.rs").read_bytes(), data)
        self.assertEqual(json.loads((self.output / packager.MANIFEST).read_text(encoding="utf-8")), manifest)
        (self.output / "src/lib.rs").write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "src/lib.rs"):
            packager.verify_export(self.output, manifest)

    def test_public_tools_and_optional_contributor_guide_are_exported_without_private_tool_state(self):
        public = {
            "tools/validate.py": b"# Public validation entry point\r\n",
            "tools/README.md": b"Public validation tiers\n",
            "tools/nested/check.py": b"# Recursive public helper\n",
            "CONTRIBUTING.md": b"How to contribute\n",
        }
        private = ("tools/output/run.json", "tools/private/config.json", "tools/__pycache__/check.pyc")
        for name, data in public.items():
            self.write(name, data)
        for name in private:
            self.write(name)
        manifest = packager.package(self.output, self.archive, root=self.root)
        names = {entry["path"] for entry in manifest["files"]}
        self.assertTrue(set(public) <= names)
        self.assertTrue(names.isdisjoint(private))
        with zipfile.ZipFile(self.archive) as archive:
            for name, data in public.items():
                self.assertEqual((self.output / name).read_bytes(), data)
                self.assertEqual(archive.read("GeoSonnar/" + name), data)

    def test_existing_directory_and_archive_are_never_overwritten(self):
        self.output.mkdir()
        marker = self.output / "keep.txt"
        marker.write_bytes(b"existing evidence")
        with self.assertRaises(FileExistsError):
            packager.package(self.output, self.archive, root=self.root)
        self.assertEqual(marker.read_bytes(), b"existing evidence")
        self.assertFalse(self.archive.exists())
        self.archive.write_bytes(b"existing archive")
        other = self.base / "another-export"
        with self.assertRaises(FileExistsError):
            packager.package(other, self.archive, root=self.root)
        self.assertFalse(other.exists())
        self.assertEqual(self.archive.read_bytes(), b"existing archive")

    def test_archive_verification_detects_changed_bytes_and_extra_files(self):
        manifest = packager.package(self.output, self.archive, root=self.root)
        with zipfile.ZipFile(self.archive) as archive:
            original = {name: archive.read(name) for name in archive.namelist()}
        for extra in (False, True):
            contents = dict(original)
            contents["GeoSonnar/private.txt" if extra else "GeoSonnar/src/lib.rs"] = b"unexpected bytes"
            with zipfile.ZipFile(self.archive, "w") as archive:
                for name, data in contents.items():
                    archive.writestr(name, data)
            with self.assertRaises(ValueError):
                packager._verify_zip(self.archive, self.output, manifest)

    def test_destinations_cannot_recurse_into_source_or_into_each_other(self):
        for output, archive in [
            (self.root / "src/export", None),
            (self.output, self.root / "analysis/archive.zip"),
            (self.output, self.output / "archive.zip"),
            (self.root, None),
        ]:
            with self.subTest(output=output, archive=archive):
                with self.assertRaises((ValueError, FileExistsError)):
                    packager.package(output, archive, root=self.root)
        self.assertFalse(self.output.exists())

    def test_unexpected_binary_unknown_types_and_oversized_source_fail_before_export(self):
        for name, data, limit in [
            ("src/bad.rs", b"valid text\x00binary", packager.MAX_FILE_BYTES),
            ("docs/bad.txt", b"\xff\xfe", packager.MAX_FILE_BYTES),
            ("docs/unreviewed.xyz", b"plain text", packager.MAX_FILE_BYTES),
            ("src/large.rs", b"x" * 101, 100),
        ]:
            with self.subTest(name=name):
                path = self.write(name, data)
                with patch.object(packager, "MAX_FILE_BYTES", limit):
                    with self.assertRaisesRegex(ValueError, name):
                        packager.package(self.output, root=self.root)
                self.assertFalse(self.output.exists())
                path.unlink()

    def test_credentials_are_refused_without_printing_their_values(self):
        credentials = [
            "gh" + "p_" + "A" * 36,
            "github_" + "pat_" + "A" * 60,
            "-----BEGIN " + "RSA PRIVATE KEY-----",
            "AK" + "IA" + "A" * 16,
            'api_key="' + "A" * 32 + '"',
        ]
        for value in credentials:
            with self.subTest(kind=value[:3]):
                self.write("docs/accidental.txt", value.encode("utf-8"))
                with self.assertRaises(ValueError) as raised:
                    packager.package(self.output, root=self.root)
                self.assertIn("docs/accidental.txt", str(raised.exception))
                self.assertNotIn(value, str(raised.exception))
                self.assertFalse(self.output.exists())

    def test_reparse_metadata_is_refused_even_when_name_would_be_excluded(self):
        path = self.write("cases/private-case.json")
        original = Path.lstat

        def simulated_reparse(candidate, *args, **kwargs):
            actual = original(candidate, *args, **kwargs)
            if candidate == path:
                return type("ReparseStat", (), {
                    "st_mode": stat.S_IFREG | 0o644,
                    "st_file_attributes": 0x400,
                })()
            return actual

        with patch.object(Path, "lstat", simulated_reparse):
            with self.assertRaisesRegex(ValueError, "reparse"):
                packager.package(self.output, root=self.root)
        self.assertFalse(self.output.exists())

    def test_real_symlink_is_refused_when_platform_allows_creation(self):
        target = self.write("docs/ordinary.txt")
        link = self.root / "docs/link.txt"
        try:
            os.symlink(target, link)
        except (OSError, NotImplementedError):
            self.skipTest("Symlink creation is unavailable; simulated Windows reparse control still runs")
        with self.assertRaisesRegex(ValueError, "Links and reparse"):
            packager.package(self.output, root=self.root)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
