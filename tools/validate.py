"""Clean-checkout validation tiers. See tools/README.md; no historical output imports."""
import argparse
from datetime import datetime, timezone
import importlib.util
import importlib.metadata
import json
import os
from pathlib import Path
import shutil
import sys
import time
import uuid

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools import evidence as e, process, release

MEDIA = {
    "gui_bridge_validation": ["actual_companion_creates_case_and_discovery_job_then_verifies_review_artifact"],
    "review_media_validation": ["actual_stereo_chirp_windows_keep_channel_time_frequency_and_rms",
        "actual_video_frame_reports_decoded_timestamp_aspect_and_source_stamp",
        "shifted_first_audio_stream_stays_aligned_with_frame_time_and_spectrum"],
    "envelope_workflow_validation": ["actual_measurement_preserves_selected_channel_center_timestamps_and_original_bandwidth",
        "cli_measurement_resolves_null_helpers_from_child_path"],
}
LARGE = "track_store::tests::indexed_store_accepts_more_than_two_million_points_without_truncation"
CHILD = "gui::shutdown::tests::process_ownership::isolated_process_probe"
PARENT = "gui::shutdown::tests::process_ownership::gui_exit_kills_owned_descendant_but_leaves_sibling_and_test_parent_alive"
DISCOVERY = "existing_generated_discovery_reports_load_without_modifying_evidence"
TERRAIN = ["cached_copernicus_and_egm96_match_independent_python_sampler",
           "cached_native_observer_grid_matches_all_independent_reference_heights"]
CARGO_TEST = ["cargo", "test", "--offline", "--locked", "--features", "gui"]


class Unavailable(ValueError):
    """A declared prerequisite is missing; this is not a passing check."""


def planned(tier):
    options = {"quick": ["format", "check"], "native": ["format", "clippy", "native"],
               "integration": ["large_store", *("media_" + name for name in MEDIA)],
               "python": ["python_tools", "python_benchmarks"], "release": release.REQUIRED_STAGES}
    return list(options[tier])


def ignored_inventory(names, passed):
    kinds = {LARGE: "explicit_synthetic_large_store", CHILD: "parent_owned_process_helper",
             DISCOVERY: "optional_existing_discovery"}
    kinds.update({name: "explicit_synthetic_media" for tests in MEDIA.values() for name in tests})
    kinds.update({name: "historical_external_terrain" for name in TERRAIN})
    e.require(len(set(names)) == len(names) and set(names) == set(kinds),
              "Ignored-test inventory changed; classify the new prerequisite explicitly")
    e.require(PARENT in passed, "Parent-owned process control did not execute")
    return [{"name": name, "kind": kinds[name], "status": "ignored_in_default_run"} for name in names]


def fresh_directory(path):
    path = Path(os.path.abspath(path))
    from analysis import package_source
    package_source._destination(path, e.ROOT)
    e.require(path != e.ROOT and not e.ROOT.is_relative_to(path), "Run destination overlaps checkout")
    path.parent.mkdir(parents=True, exist_ok=True)
    e.ordinary(path.parent, True)
    path.mkdir()
    return path


class Session:
    def __init__(self, args):
        self.args, self.python = args, sys.executable
        self.started = time.monotonic(); self.deadline = self.started + args.wall_seconds
        self.output = fresh_directory(args.output)
        scratch = self.output / "scratch"; scratch.mkdir()
        self.target = scratch / "target"; self.target.mkdir()
        (scratch / "temp").mkdir(); (self.output / "logs").mkdir()
        marker = {"schema_version": 1, "kind": "geosonnar-public-validation", "run_id": uuid.uuid4().hex,
                  "run_root": str(self.output), "scratch": "scratch"}
        e.write(self.output / "OWNED-RUN.json", marker)
        self.report = {"schema_version": 1, "tier": args.tier, "status": "running",
            "checkout_root": str(e.ROOT),
            "started_utc": datetime.now(timezone.utc).isoformat(), "owner": e.receipt(self.output / "OWNED-RUN.json", self.output),
            "job": process.current_job(), "limits": {"per_command_seconds": args.timeout,
            "whole_run_seconds": args.wall_seconds, "log_bytes_per_stream": 16 * 1024 * 1024},
            "stages": [{"id": name, "required": True, "status": "not_started"} for name in planned(args.tier)],
            "commands": [], "source_snapshot": [], "ignored_test_inventory": [],
            "optional_integrations": [{"name": name, "status": "not_requested",
                "reason": "Historical external terrain/reference inputs are not part of clean-checkout validation"} for name in TERRAIN],
            "field_accuracy_validated": False, "interactive_gui_tested": False, "network_requested": False}
        if args.tier in ("integration", "release"):
            self.report["stages"].append({"id": "existing_discovery", "required": bool(args.discovery_report),
                "status": "not_started" if args.discovery_report else "not_requested",
                "reason": "Only runs when --discovery-report explicitly supplies existing data"})
        self.save()

    def save(self):
        e.write(self.output / "report.json", self.report, replace=(self.output / "report.json").exists())

    def text(self, name, stream):
        path = self.output / "logs" / (name + "." + stream)
        with path.open("rb") as source: raw = source.read(16 * 1024 * 1024 + 1)
        e.require(len(raw) <= 16 * 1024 * 1024, "Log exceeded its bound")
        return raw.decode("utf-8", errors="replace")

    def command(self, name, arguments, environment=None, timeout=None, expected=0):
        remaining = self.deadline - time.monotonic()
        e.require(remaining > 1, "Whole-run wall budget exhausted before command")
        if not shutil.which(str(arguments[0])) and not Path(arguments[0]).is_file():
            raise Unavailable("Required executable is missing: " + str(arguments[0]))
        env = {"CARGO_TARGET_DIR": str(self.target), "CARGO_BUILD_JOBS": "2", "RUST_TEST_THREADS": "2",
               "CARGO_INCREMENTAL": "0", "PYTHONDONTWRITEBYTECODE": "1",
               "TEMP": str(self.output / "scratch/temp"), "TMP": str(self.output / "scratch/temp")}
        env.update(environment or {})
        try:
            return process.run(arguments, cwd=e.ROOT, log_prefix=self.output / "logs" / name,
                environment=env, timeout_s=min(timeout or self.args.timeout, remaining), expected=expected)
        finally:
            path = self.output / "logs" / (name + ".command.json")
            if path.exists(): self.report["commands"].append(e.receipt(path, self.output))
            self.save()

    def stage(self, name, task):
        row = next(row for row in self.report["stages"] if row["id"] == name)
        row["status"] = "running"; began = time.monotonic(); self.save()
        try:
            row["details"] = task(); row["status"] = "passed"
        except Unavailable as error:
            row.update(status="unavailable", reason=str(error))
        except Exception as error:
            row.update(status="failed", reason=type(error).__name__ + ": " + str(error))
        row["elapsed_s"] = time.monotonic() - began; self.save()
        print(json.dumps({"stage": name, "status": row["status"]}), flush=True)
        return row["status"] == "passed"

    def rust(self, label, arguments, expected=None, environment=None):
        self.command(label, arguments, environment)
        result = e.rust_tests(self.text(label, "stdout"), expected)
        if label == "native":
            self.report["ignored_test_inventory"] = ignored_inventory(result["ignored_names"], result["passed_names"])
        return result

    def helpers(self):
        bound = {}
        for name in ("ffmpeg", "ffprobe"):
            selected = getattr(self.args, name)
            if selected is None:
                selected = shutil.which(name)
            elif len(selected.parts) == 1 and not selected.is_absolute():
                selected = shutil.which(str(selected))
            if not selected or not Path(selected).is_file():
                raise Unavailable("Actual " + name + " is required; supply --" + name + " EXE or put it on PATH")
            bound[name] = e.receipt(selected)
        return bound

    def media(self, target):
        helpers = self.helpers()
        env = self.helper_environment(helpers)
        result = self.rust("media_" + target, [*CARGO_TEST, "--test", target, "--", "--ignored"], MEDIA[target], env)
        for item in helpers.values(): e.verify(item)
        return {"tests": result, "helpers": helpers, "synthetic_media_only": True}

    @staticmethod
    def helper_environment(helpers):
        env = {"GEOSONNAR_TEST_" + name.upper(): item["path"] for name, item in helpers.items()}
        env["PATH"] = os.pathsep.join(dict.fromkeys(str(Path(item["path"]).parent) for item in helpers.values())) + os.pathsep + os.environ.get("PATH", "")
        return env

    def native(self):
        # Several default audio tests also decode generated audio, so bind tools
        # here rather than treating only the explicitly ignored tests as media.
        helpers = self.helpers()
        result = self.rust("native", [*CARGO_TEST, "--all-targets", "--no-fail-fast"], environment=self.helper_environment(helpers))
        for item in helpers.values(): e.verify(item)
        result["media_helpers"] = helpers
        return result

    def python_tests(self, folder):
        if folder == "benchmarks":
            missing = [name for name in ("numpy", "scipy", "soundfile", "matplotlib") if importlib.util.find_spec(name) is None]
            if missing: raise Unavailable("Python benchmark prerequisites missing: " + ", ".join(missing) + "; see requirements-research.txt")
        name = "python_" + folder
        self.command(name, [self.python, "-B", "-m", "unittest", "discover", "-s", folder, "-p", "test_*.py", "-v"])
        result = e.python_tests(self.text(name, "stderr"))
        if folder == "benchmarks":
            result["prerequisite_versions"] = {name: importlib.metadata.version(name)
                for name in ("numpy", "scipy", "soundfile", "matplotlib")}
        return result

    def discovery(self):
        helpers = self.helpers(); report = e.receipt(self.args.discovery_report)
        env = {"GEOSONNAR_TEST_DISCOVERY_REPORTS": report["path"],
               "PATH": str(Path(helpers["ffprobe"]["path"]).parent) + os.pathsep + os.environ.get("PATH", "")}
        result = self.rust("existing_discovery", [*CARGO_TEST, "--test", "event_review_validation", DISCOVERY,
            "--", "--ignored", "--exact"], [DISCOVERY], env)
        e.verify(report)
        for item in helpers.values(): e.verify(item)
        return {"tests": result, "supplied_report": report, "helpers": helpers,
                "scope": "Compatibility only; this is not a new field-accuracy trial"}

    def execute(self):
        try:
            if os.name != "nt": raise Unavailable("These tiers require Windows for the enforced CPU/ownership policy")
            self.report["source_snapshot"] = e.snapshot(); self.save()
            tasks = {
                "format": lambda: self.command("format", ["cargo", "fmt", "--all", "--", "--check"]),
                "check": lambda: self.command("check", ["cargo", "check", "--offline", "--locked", "--features", "gui", "--all-targets"]),
                "clippy": lambda: self.command("clippy", ["cargo", "clippy", "--offline", "--locked", "--features", "gui", "--all-targets", "--", "-D", "warnings"]),
                "native": self.native,
                "large_store": lambda: self.rust("large_store", [*CARGO_TEST, "--lib", LARGE, "--", "--ignored", "--exact"], [LARGE]),
                "python_tools": lambda: self.python_tests("tools"),
                "python_benchmarks": lambda: self.python_tests("benchmarks"),
                "build": lambda: self.command("build", release.BUILD_ARGUMENTS, release.build_environment(self.target)),
                "existing_discovery": self.discovery}
            tasks.update({"media_" + target: (lambda target=target: self.media(target)) for target in MEDIA})
            for row in self.report["stages"]:
                if row["status"] != "not_requested" and not self.stage(row["id"], tasks[row["id"]]): break
            if all(row["status"] == "passed" for row in self.report["stages"] if row["required"]):
                if self.args.tier == "release": self.report["release"] = release.package(self)
                self.report["status"] = "passed"
            else:
                self.report["status"] = "unavailable" if any(row["status"] == "unavailable" for row in self.report["stages"]) else "failed"
            e.require(e.snapshot() == self.report["source_snapshot"], "Source changed during validation/build")
            self.report["source_unchanged"] = True
        except Unavailable as error:
            self.report.update(status="unavailable", error=str(error))
        except BaseException as error:
            self.report.update(status="failed", error=type(error).__name__ + ": " + str(error))
        self.report["elapsed_s"] = time.monotonic() - self.started; self.save()
        print(json.dumps({"status": self.report["status"], "report": str(self.output / "report.json")}), flush=True)
        return {"passed": 0, "unavailable": 2}.get(self.report["status"], 1)


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest="tier", required=True)
    for tier in ("quick", "native", "integration", "python", "release"):
        item = subs.add_parser(tier)
        item.add_argument("--output", type=Path, required=True, help="Fresh run directory outside source folders")
        item.add_argument("--timeout", type=int, default=1800, help="Maximum seconds per child command")
        item.add_argument("--wall-seconds", type=int, default=7200, help="Maximum seconds for the whole tier")
        item.add_argument("--ffmpeg", type=Path); item.add_argument("--ffprobe", type=Path)
        item.add_argument("--discovery-report", type=Path, help="Explicit optional existing-report integration")
    for tier in ("verify", "cleanup"):
        item = subs.add_parser(tier); item.add_argument("--run", type=Path, required=True)
        if tier == "cleanup": item.add_argument("--execute", action="store_true", help="Remove this run's marked scratch only")
    args = parser.parse_args()
    if hasattr(args, "output"):
        e.require(1 <= args.timeout <= 7200 and args.timeout <= args.wall_seconds <= 43200, "Invalid command/whole-run deadlines")
        e.require(not args.discovery_report or args.tier in ("integration", "release"), "Discovery input only applies to integration/release")
    return args


def main():
    args = arguments()
    if os.environ.get("GEOSONNAR_VALIDATION_WORKER") != "1":
        if os.name != "nt":
            print("Capped validation requires Windows; no uncapped fallback. Pure controls can run with unittest.", file=sys.stderr)
            return 2
        if hasattr(args, "output"):
            logdir = fresh_directory(Path(str(args.output) + ".launcher")); timeout = args.wall_seconds + 30
        else:
            run = e.ordinary(args.run, True)
            logdir = run / (args.tier + "-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S") + "-" + uuid.uuid4().hex[:6])
            logdir.mkdir(); timeout = 1800
        print("Running serial checks in a 20% CPU job. Logs: " + str(logdir), flush=True)
        result = process.run([sys.executable, "-B", Path(__file__).resolve(), *sys.argv[1:]], cwd=e.ROOT,
            log_prefix=logdir / "worker", timeout_s=timeout, expected=(0, 1, 2), aggregate=True,
            environment={"GEOSONNAR_VALIDATION_WORKER": "1", "PYTHONDONTWRITEBYTECODE": "1"})
        print((logdir / "worker.stdout").read_text(encoding="utf-8", errors="replace"), end="")
        return result["returncode"]
    process.current_job()  # Environment marker alone is never accepted as resource proof.
    if args.tier == "verify":
        print(json.dumps(release.verify_release(args.run), indent=2)); return 0
    if args.tier == "cleanup":
        print(json.dumps(release.cleanup(args.run, args.execute), indent=2)); return 0
    return Session(args).execute()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as error:
        print("Validation refused: " + str(error), file=sys.stderr)
        raise SystemExit(1)
