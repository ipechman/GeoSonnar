"""Fabricated receipt/resource controls. No Rust builds, GUI or network."""
import copy
import io
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
import zipfile

from tools import evidence as e, process, release, validate


def aggregate_fixture():
    return {"schema_version": 2, "policy": "aggregate_20_percent",
            "job_name": "Local\\GeoSonnarValidation-fixture", "cpu_percent": 20,
            "job_flags": 5, "job_rate": 2000, "queried": True, "kill_on_close": True}


def command_job_fixture():
    return {"schema_version": 2, "policy": "inherited_aggregate_no_additional_cpu_cap",
            "aggregate": aggregate_fixture(),
            "owned_job": {"job_flags": 0, "job_rate": 0, "queried": True, "kill_on_close": True},
            "aggregate_membership_queried": True, "owned_membership_queried": True,
            "below_normal_priority_queried": True}


class PublicValidationControls(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()

    def file(self, name, data=b"fixture"):
        path = self.root / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(data)
        return path

    def test_rust_counts_require_names_and_reject_empty_or_failed_filtered_runs(self):
        text = "test sample::one ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n"
        self.assertEqual(e.rust_tests(text, ["sample::one"])["passed"], 1)
        self.assertEqual(e.rust_tests(text + text)["passed"], 2)  # Two independent test binaries.
        duplicate = "test sample::one ... ok\n" + text.replace("1 passed", "2 passed")
        with self.assertRaises(ValueError): e.rust_tests(duplicate)
        for bad in (text.replace("1 passed", "2 passed"), text.replace("ok\n", "FAILED\n"),
                    "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out;\n"):
            with self.assertRaises(ValueError): e.rust_tests(bad)
        with self.assertRaises(ValueError): e.rust_tests(text, ["renamed"])

    def test_python_skips_are_preserved_and_not_counted_as_passes(self):
        log = "test_a (suite.Control.test_a) ... ok\ntest_b (suite.Control.test_b) ... skipped 'unavailable'\n\nRan 2 tests in 0.01s\n\nOK (skipped=1)\n"
        result = e.python_tests(log)
        self.assertEqual((result["run"], result["passed"], result["skipped"]), (2, 1, 1))
        self.assertEqual(result["tests"][1]["reason"], "'unavailable'")
        with self.assertRaises(ValueError): e.python_tests(log.replace("Ran 2", "Ran 3"))
        with self.assertRaises(ValueError): e.python_tests(log.replace("OK (skipped=1)", "FAILED (failures=1)"))

    def test_ignored_shutdown_probe_is_not_an_optional_external_test(self):
        names = [validate.LARGE, validate.CHILD, validate.DISCOVERY, *validate.TERRAIN,
                 *(name for group in validate.MEDIA.values() for name in group)]
        rows = validate.ignored_inventory(names, [validate.PARENT])
        self.assertEqual(next(row for row in rows if row["name"] == validate.CHILD)["kind"], "parent_owned_process_helper")
        with self.assertRaises(ValueError): validate.ignored_inventory(names, [])
        with self.assertRaises(ValueError): validate.ignored_inventory(names + ["new_ignored_test"], [validate.PARENT])

    def test_retained_log_capture_stops_at_bound_and_signals_overflow(self):
        stop, errors = threading.Event(), []
        target = self.root / "log"
        process.capture(io.BytesIO(b"abcdefgh"), target, 5, stop, errors)
        self.assertEqual(target.read_bytes(), b"abcde")
        self.assertTrue(stop.is_set()); self.assertEqual(errors, [])
        stop.clear()
        process.capture(io.BytesIO(b"abcde"), self.root / "exact", 5, stop, errors)
        self.assertFalse(stop.is_set())

    def test_build_flags_are_per_child_static_and_remapped_without_environment_mutation(self):
        before = dict(os.environ)
        flags = release.build_environment(self.root / "target")
        self.assertIn("target-feature=+crt-static", flags["CARGO_ENCODED_RUSTFLAGS"])
        self.assertIn("--remap-path-prefix=", flags["CARGO_ENCODED_RUSTFLAGS"])
        self.assertEqual(flags["CARGO_TARGET_DIR"], str(self.root / "target"))
        self.assertEqual(dict(os.environ), before)

    def test_default_and_maximum_outer_deadlines_fit_command_runner(self):
        # Failure here must be the unsupported platform, not an inconsistent timeout bound.
        for timeout in (7200 + 30, 43200 + 30):
            with patch.object(process, "WindowsJob", side_effect=ValueError("fixture ownership refusal")):
                with self.assertRaisesRegex(ValueError, "fixture ownership refusal"):
                    process.run(["never-launched"], cwd=self.root, log_prefix=self.root / str(timeout), timeout_s=timeout)
            self.assertEqual(e.read(self.root / (str(timeout) + ".command.json"))["timeout_s"], timeout)

    def test_command_cpu_policy_inherits_one_cap_and_rejects_nested_or_unverified_rates(self):
        proof = command_job_fixture()
        process.verify_command_proof(proof, aggregate_fixture())
        for key, value in [("job_flags", 5), ("job_rate", 2000), ("job_rate", 10000),
                           ("queried", False), ("kill_on_close", False)]:
            changed = copy.deepcopy(proof); changed["owned_job"][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                process.verify_command_proof(changed)
        for key in ("aggregate_membership_queried", "owned_membership_queried", "below_normal_priority_queried"):
            changed = copy.deepcopy(proof); changed[key] = False
            with self.subTest(key=key), self.assertRaises(ValueError): process.verify_command_proof(changed)
        changed = copy.deepcopy(proof); changed["aggregate"]["job_name"] += "-other"
        with self.assertRaises(ValueError): process.verify_command_proof(changed, aggregate_fixture())
        with self.assertRaises(ValueError): process.verify_command_proof({"queried": True, "job_flags": 5, "job_rate": 2000})

    def test_host_ownership_job_allowed_but_known_cpu_control_refused(self):
        host = {"present": True, "queried": True, "immediate_job_flags": 0, "immediate_job_rate": 0,
                "ancestor_policies_unknown": True, "external_limits_may_reduce_availability": True}
        process.verify_launcher_host(host)
        process.verify_launcher_host({**host, "present": False})
        for flags in (1, 5, 9, 17):
            with self.subTest(flags=flags), self.assertRaises(ValueError):
                process.verify_launcher_host({**host, "immediate_job_flags": flags})

    def test_named_aggregate_requires_actual_membership_and_closes_transient_handle(self):
        for member in (False, True):
            calls = []
            class Kernel:
                def GetCurrentProcess(self): return 7
                def CloseHandle(self, handle): calls.append(("closed", handle))
            class Api:
                kernel = Kernel()
                def open_aggregate(self, name):
                    calls.append(("opened", name)); return 11
                def query(self, handle):
                    return {key: aggregate_fixture()[key] for key in ("job_flags", "job_rate", "queried", "kill_on_close")}
                def contains(self, handle, job):
                    calls.append(("membership", handle, job)); return member
            with patch.object(process, "_Windows", Api), patch.dict(os.environ, {process.AGGREGATE_JOB_ENV: aggregate_fixture()["job_name"]}):
                if member: self.assertEqual(process.current_job(), aggregate_fixture())
                else:
                    with self.assertRaisesRegex(ValueError, "not in the named"): process.current_job()
            self.assertEqual(calls[-1], ("closed", 11))
            self.assertIn(("membership", 7, 11), calls)

    def test_explicit_missing_helper_does_not_fall_back_but_bare_name_uses_path(self):
        session = object.__new__(validate.Session)
        class Args:
            ffmpeg = Path("nonexistent/custom-ffmpeg.exe")
            ffprobe = None
        session.args = Args()
        with patch.object(validate.shutil, "which", return_value=str(self.file("good.exe"))):
            with self.assertRaises(validate.Unavailable): session.helpers()
        session.args.ffmpeg = Path("ffmpeg.exe"); session.args.ffprobe = Path("ffprobe.exe")
        with patch.object(validate.shutil, "which", return_value=str(self.root / "good.exe")) as which:
            self.assertEqual(set(session.helpers()), {"ffmpeg", "ffprobe"})
            self.assertEqual([call.args[0] for call in which.call_args_list], ["ffmpeg.exe", "ffprobe.exe"])

    def test_manifest_members_reject_escape_and_absolute_names(self):
        for name in ("../outside", "/absolute", "C:/absolute", "safe\\..\\outside", ""):
            with self.assertRaises(ValueError): e.member(self.root, name)
        path = self.file("safe/file.txt")
        item = e.receipt(path, self.root)
        self.assertEqual(e.verify(item, self.root), path)
        path.write_bytes(b"changed")
        with self.assertRaises(ValueError): e.verify(item, self.root)

    def test_native_tier_passes_explicit_helpers_and_refuses_mutation_after_execution(self):
        session = object.__new__(validate.Session)
        first = self.file("ffmpeg-dir/ffmpeg.exe"); second = self.file("ffprobe-dir/ffprobe.exe")
        helpers = {"ffmpeg": e.receipt(first), "ffprobe": e.receipt(second)}
        with patch.object(session, "helpers", return_value=helpers), patch.object(session, "rust", return_value={"passed": 1}) as command:
            result = session.native()
            environment = command.call_args.kwargs["environment"]
            self.assertEqual(environment["GEOSONNAR_TEST_FFMPEG"], str(first))
            self.assertEqual(environment["PATH"].split(os.pathsep)[:2], [str(first.parent), str(second.parent)])
            self.assertEqual(result["media_helpers"], helpers)
        def changed(*args, **kwargs):
            first.write_bytes(b"mutation")
            return {"passed": 1}
        with patch.object(session, "helpers", return_value=helpers), patch.object(session, "rust", side_effect=changed):
            with self.assertRaises(ValueError): session.native()

    def test_zip_verification_rejects_changed_or_extra_members(self):
        folder = self.root / "folder"; folder.mkdir(); (folder / "a.txt").write_bytes(b"a")
        archive = self.root / "archive.zip"
        release.zip_tree(folder, archive, "Package"); release.verify_zip(archive, folder, "Package")
        with zipfile.ZipFile(archive, "a") as target: target.writestr("Package/extra.txt", b"x")
        with self.assertRaises(ValueError): release.verify_zip(archive, folder, "Package")

    def cleanup_fixture(self):
        run = self.root / "run"; run.mkdir(); (run / "scratch").mkdir()
        (run / "scratch/cache").write_bytes(b"owned")
        marker = {"kind": "geosonnar-public-validation", "run_root": str(run), "scratch": "scratch"}
        e.write(run / "OWNED-RUN.json", marker)
        report = {"owner": e.receipt(run / "OWNED-RUN.json", run), "status": "failed", "tier": "native"}
        e.write(run / "report.json", report)
        return run

    def test_cleanup_preview_preserves_and_execute_removes_only_owned_scratch(self):
        run = self.cleanup_fixture(); sibling = self.file("outside.txt", b"keep")
        self.assertEqual(release.cleanup(run)["status"], "preview")
        self.assertTrue((run / "scratch/cache").exists())
        self.assertEqual(release.cleanup(run, True)["status"], "removed_owned_scratch")
        self.assertFalse((run / "scratch").exists())
        self.assertTrue((run / "report.json").is_file()); self.assertEqual(sibling.read_bytes(), b"keep")

    def test_cleanup_refuses_rebound_marker_or_unfinished_run(self):
        run = self.cleanup_fixture()
        report = e.read(run / "report.json"); report["status"] = "running"
        e.write(run / "report.json", report, replace=True)
        with self.assertRaises(ValueError): release.cleanup(run, True)
        report["status"] = "failed"; e.write(run / "report.json", report, replace=True)
        marker = e.read(run / "OWNED-RUN.json"); marker["scratch"] = "../other"
        e.write(run / "OWNED-RUN.json", marker, replace=True)
        with self.assertRaises(ValueError): release.cleanup(run, True)
        self.assertTrue((run / "scratch/cache").is_file())

    def test_retained_check_verifier_detects_deleted_or_changed_test_logs(self):
        directory = self.root
        out = self.file("logs/python_tools.stdout", b"")
        err = self.file("logs/python_tools.stderr", b"test_a (suite.Control.test_a) ... ok\n\nRan 1 test in 0.01s\n\nOK\n")
        command = {"status": "passed", "returncode": 0, "expected_returncode": 0, "cwd": "checkout",
            "job": command_job_fixture(),
            "logs": [e.receipt(out), e.receipt(err)], "log_limit_bytes_per_stream": 1024}
        path = directory / "logs/python_tools.command.json"; e.write(path, command)
        checks = {"job": aggregate_fixture(), "commands": [e.receipt(path, directory)], "checkout_root": "checkout", "stages": [
            {"id": "python_tools", "status": "passed", "details": e.python_tests(err.read_text())}]}
        release.verify_checks(checks, directory)
        err.write_bytes(err.read_bytes() + b"changed")
        with self.assertRaises(ValueError): release.verify_checks(checks, directory)
        err.unlink()
        with self.assertRaises((ValueError, OSError)): release.verify_checks(checks, directory)


if __name__ == "__main__":
    unittest.main()
