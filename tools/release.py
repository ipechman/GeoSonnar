"""Public package verification and narrowly owned build cleanup."""
import json
import os
from pathlib import Path
import re
import shutil
import tomllib
import zipfile
from tools.evidence import ROOT, files, identity, member, ordinary, read, receipt, require, verify, write, rust_tests, python_tests

PROGRAMS = {"geosonnar.exe": "geosonnar.exe", "geosonnar-gui.exe": "geosonnar-gui.exe",
            "prediction_overlay.exe": "examples/prediction_overlay.exe",
            "regional_search.exe": "examples/regional_search.exe",
            "reopen_investigation.exe": "examples/reopen_investigation.exe",
            "wide_area_reference.exe": "examples/wide_area_reference.exe"}
REQUIRED_STAGES = ["format", "clippy", "native", "large_store", "media_gui_bridge_validation",
                   "media_review_media_validation", "media_envelope_workflow_validation",
                   "python_tools", "python_benchmarks", "build"]
BUILD_ARGUMENTS = ["cargo", "build", "--release", "--offline", "--locked", "--features", "gui", "--bins",
                   "--example", "prediction_overlay", "--example", "regional_search", "--example", "reopen_investigation",
                   "--example", "wide_area_reference"]


def build_environment(target):
    home = Path.home().absolute()
    cargo = Path(os.environ.get("CARGO_HOME", str(home / ".cargo"))).absolute()
    flags = ["-C", "target-feature=+crt-static"]
    for origin, replacement in ((home, "/user"), (cargo, "/cargo"), (ROOT, "/workspace")):
        for spelling in dict.fromkeys((str(origin), origin.as_posix())):
            flags.append("--remap-path-prefix=" + spelling + "=" + replacement)
    return {"CARGO_TARGET_DIR": str(target), "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)}


def path_screen(path):
    data = ordinary(path).read_bytes(); lower = data.lower()
    found = re.search(rb"(?i)[A-Z]:[\\/]+Users[\\/]+[^\\/\x00\r\n]+", data) is not None
    for origin in (Path.home(), ROOT):
        for spelling in (str(origin), origin.as_posix()):
            found |= spelling.encode().lower() in lower or spelling.encode("utf-16-le").lower() in lower
    require(not found, "Personal build-path bytes remain in " + Path(path).name)


def copy_exact(source, destination):
    before = identity(source)
    with Path(source).open("rb") as reader, Path(destination).open("xb") as writer:
        shutil.copyfileobj(reader, writer, 1024 * 1024)
    require(identity(source) == identity(destination) == before, "Binary changed during preservation")


def startup(session, binaries, version):
    session.command("cli-help", [binaries / "geosonnar.exe", "help"], timeout=30)
    require(session.text("cli-help", "stdout").startswith("GeoSonnar " + version + " "), "CLI version differs")
    outcomes = []
    for name, executable, flags, renderer in (
        ("default", "geosonnar-gui.exe", [], "Direct3D12"),
        ("safe", "geosonnar-safe.exe", [], "SoftwareDirect3D12"),
        ("opengl", "geosonnar-gui.exe", ["--opengl"], "OpenGl")):
        label = "startup-" + name
        session.command(label, [binaries / executable, *flags, "--headless-startup-check"], timeout=30)
        value = json.loads(session.text(label, "stdout"))
        require(value["version"] == version and value["renderer"] == renderer and
                not any(value[key] for key in ("window_opened", "graphics_initialized", "recording_opened")),
                "Headless startup configuration differs")
        outcomes.append(value)
    session.command("safe-rejects-opengl", [binaries / "geosonnar-safe.exe", "--opengl", "--headless-startup-check"],
                    timeout=30, expected=2)
    require("always uses software" in session.text("safe-rejects-opengl", "stderr"), "Safe launcher override accepted")
    return {"scope": "Headless configuration only; no driver or interactive GUI validation", "checks": outcomes}


def zip_tree(directory, destination, prefix):
    with zipfile.ZipFile(destination, "x", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in files(directory):
            archive.write(path, prefix + "/" + path.relative_to(directory).as_posix())


def verify_zip(archive_path, directory, prefix):
    expected = {prefix + "/" + path.relative_to(directory).as_posix(): path for path in files(directory)}
    with zipfile.ZipFile(ordinary(archive_path)) as archive:
        require(len(archive.namelist()) == len(expected) and set(archive.namelist()) == set(expected),
                "ZIP member inventory differs")
        for name, path in expected.items():
            info = archive.getinfo(name)
            require(info.file_size == path.stat().st_size, "ZIP member length differs")
            # Streaming comparison bounds working memory even for executables.
            with archive.open(name) as left, path.open("rb") as right:
                while chunk := right.read(1024 * 1024):
                    require(left.read(len(chunk)) == chunk, "ZIP member bytes differ")
                require(not left.read(1), "ZIP member grew")


def package(session):
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    output = session.output / "release"; output.mkdir()
    binaries = output / "binaries"; binaries.mkdir()
    for name, source in PROGRAMS.items():
        copy_exact(session.target / "release" / source, binaries / name)
    copy_exact(binaries / "geosonnar-gui.exe", binaries / "geosonnar-safe.exe")
    for path in files(binaries): path_screen(path)
    started = startup(session, binaries, version)
    # This immutable pre-package report is the packager's validation reference.
    write(session.output / "checks.json", session.report)
    portable = output / ("GeoSonnar-Windows-" + version)
    source = output / ("GeoSonnar-source-" + version)
    source_zip = output / (source.name + ".zip")
    portable_zip = output / (portable.name + ".zip")
    session.command("package-windows", [session.python, ROOT / "analysis/package_windows.py", "--binary-dir", binaries,
        "--output", portable, "--validation", session.output / "checks.json", "--readme", ROOT / "docs/WINDOWS_GUI.txt"])
    session.command("package-source", [session.python, ROOT / "analysis/package_source.py", "--output", source, "--zip", source_zip])
    zip_tree(portable, portable_zip, portable.name)
    proof = {"schema_version": 1, "version": version, "status": "assembled_not_yet_verified",
             "source_snapshot": session.report["source_snapshot"],
             "build_command": receipt(session.output / "logs/build.command.json", session.output),
             "build_environment": build_environment(session.target),
             "checks": receipt(session.output / "checks.json", session.output),
             "startup": started, "portable_directory": portable.relative_to(session.output).as_posix(),
             "source_directory": source.relative_to(session.output).as_posix(),
             "windows_zip": receipt(portable_zip, session.output), "source_zip": receipt(source_zip, session.output),
             "binaries": [receipt(path, session.output) for path in files(binaries) if path.suffix == ".exe"],
             "artifacts": [receipt(path, session.output) for path in files(output)],
             "field_accuracy_validated": False, "interactive_gui_tested": False}
    write(output / "manifest.json", proof)
    verify_release(session.output, terminal=False)
    return {"manifest": receipt(output / "manifest.json", session.output), "version": version,
            "status": "verified_software_release", "field_accuracy_validated": False}


def verify_checks(checks, directory):
    from tools.validate import MEDIA, LARGE, DISCOVERY, ignored_inventory
    from tools.process import verify_aggregate_proof, verify_command_proof
    verify_aggregate_proof(checks["job"])
    commands = {}
    for item in checks["commands"]:
        path = verify(item, directory)
        require(path.parent == directory / "logs" and path.name.endswith(".command.json"), "Command receipt path differs")
        name = path.name.removesuffix(".command.json")
        require(name not in commands, "Duplicate command receipt")
        command = read(path)
        require(command["status"] == "passed" and command["returncode"] == command["expected_returncode"] and
                command["cwd"] == checks["checkout_root"], "Command/resource proof differs")
        verify_command_proof(command["job"], checks["job"])
        require(len(command["logs"]) == 2, "Both command streams must be retained")
        streams = {}
        for stream, log in zip(("stdout", "stderr"), command["logs"]):
            expected = directory / "logs" / (name + "." + stream)
            require(Path(log["path"]).name == expected.name and identity(expected) ==
                    {key: log[key] for key in ("bytes", "sha256")}, "Command log changed or missing")
            require(log["bytes"] <= command["log_limit_bytes_per_stream"], "Command log limit differs")
            streams[stream] = expected.read_text(encoding="utf-8", errors="replace")
        commands[name] = (command, streams)
    for row in checks["stages"]:
        if row["status"] != "passed": continue
        name, details = row["id"], row["details"]
        require(name in commands, "Passing stage has no bound executed command")
        command, streams = commands[name]
        if name == "native":
            actual = rust_tests(streams["stdout"])
            require(set(details.get("media_helpers", {})) == {"ffmpeg", "ffprobe"} and
                    actual == {key: value for key, value in details.items() if key != "media_helpers"} and
                    ignored_inventory(actual["ignored_names"], actual["passed_names"]) ==
                    checks["ignored_test_inventory"], "Native result inventory differs")
        elif name == "large_store":
            require(rust_tests(streams["stdout"], [LARGE]) == details, "Large-store result inventory differs")
        elif name.startswith("media_"):
            require(rust_tests(streams["stdout"], MEDIA[name.removeprefix("media_")]) == details["tests"],
                    "Media result inventory differs")
        elif name == "existing_discovery":
            require(rust_tests(streams["stdout"], [DISCOVERY]) == details["tests"], "Optional integration inventory differs")
        elif name.startswith("python_"):
            actual = python_tests(streams["stderr"])
            require(all(details.get(key) == value for key, value in actual.items()), "Python result inventory differs")
        else:
            require(command == details, "Executed stage command differs from its details")
    return commands


def verify_release(directory, terminal=True):
    from tools.process import verify_command_proof
    directory = ordinary(directory, True)
    proof = read(directory / "release/manifest.json")
    inventory = proof["artifacts"]
    inventory_by_name = {item["path"]: item for item in inventory}
    require(len({item["path"] for item in inventory}) == len(inventory) and
            {member(directory, item["path"]) for item in inventory} ==
            set(files(directory / "release")) - {directory / "release/manifest.json"}, "Release inventory differs")
    for item in inventory: verify(item, directory)
    command = read(verify(proof["build_command"], directory))
    verify_command_proof(command["job"])
    require(command["status"] == "passed" and command["returncode"] == 0 and command["command"] == BUILD_ARGUMENTS and
            all(command["environment_overrides"].get(key) == value for key, value in proof["build_environment"].items()),
            "Static/remapped build receipt differs")
    require("target-feature=+crt-static" in proof["build_environment"]["CARGO_ENCODED_RUSTFLAGS"] and
            "--remap-path-prefix=" in proof["build_environment"]["CARGO_ENCODED_RUSTFLAGS"], "Build flags missing")
    checks = read(verify(proof["checks"], directory))
    required = [row["id"] for row in checks["stages"] if row["required"]]
    require(checks["source_snapshot"] == proof["source_snapshot"] and checks["tier"] == "release" and
            required in (REQUIRED_STAGES, REQUIRED_STAGES + ["existing_discovery"]) and
            all(row["status"] == "passed" for row in checks["stages"] if row["required"]), "Required checks did not pass")
    commands = verify_checks(checks, directory)
    require(proof["build_command"] in checks["commands"] and commands["build"][0] == command,
            "Build command is not in the check inventory")
    source, portable = member(directory, proof["source_directory"]), member(directory, proof["portable_directory"])
    source_manifest = read(source / "SOURCE_MANIFEST.json")
    require(source_manifest["files"] == proof["source_snapshot"], "Packaged source differs from compiled snapshot")
    require(tomllib.loads((source / "Cargo.toml").read_text())["package"]["version"] == proof["version"],
            "Packaged source version differs")
    from analysis import package_source
    package_source.verify_export(source, source_manifest)
    package_receipt = read(portable / "package-receipt.json")
    require(package_receipt["status"] == "completed" and package_receipt["validation_report"]["sha256"] == proof["checks"]["sha256"],
            "Portable package validation binding differs")
    indexed_source = {item["path"]: item for item in proof["source_snapshot"]}
    require(all(item == indexed_source.get(item["path"]) for item in package_receipt["source_files"]),
            "Portable package compiled source differs")
    expected = {item["path"] for item in package_receipt["artifacts"]} | {"package-receipt.json"}
    require({p.relative_to(portable).as_posix() for p in files(portable)} == expected, "Portable file inventory differs")
    for item in package_receipt["artifacts"]: verify(item, portable)
    require(len(proof["binaries"]) == 7 and {Path(i["path"]).name for i in proof["binaries"]} ==
            set(PROGRAMS) | {"geosonnar-safe.exe"}, "Seven retained executables required")
    require(all(inventory_by_name.get(item["path"]) == item for item in
                proof["binaries"] + [proof["windows_zip"], proof["source_zip"]]), "Output proof references unregistered artifacts")
    binaries = {Path(item["path"]).name: verify(item, directory) for item in proof["binaries"]}
    for name in ("geosonnar.exe", "geosonnar-gui.exe", "geosonnar-safe.exe"):
        require(identity(portable / name) == identity(binaries[name]), "Packaged executable differs")
    require(identity(binaries["geosonnar-safe.exe"]) == identity(binaries["geosonnar-gui.exe"]), "Safe alias differs")
    licenses = read(portable / "licenses/rust/manifest.json")
    require(licenses["package_count"] == len(licenses["packages"]) > 0, "Dependency license inventory missing")
    for dependency in licenses["packages"]:
        require(dependency["documents"], "Dependency license documents absent")
        for item in dependency["documents"]: verify(item, portable)
    for name in ("LICENSE", "THIRD_PARTY_NOTICES.md", "DOPPLERGEIST-MIT.txt", "NATURAL_EARTH.txt"):
        ordinary(portable / name)
    verify_zip(verify(proof["windows_zip"], directory), portable, portable.name)
    verify_zip(verify(proof["source_zip"], directory), source, "GeoSonnar")
    if terminal:
        report = read(directory / "report.json")
        require(report["status"] == "passed" and report["tier"] == "release" and
                report["source_unchanged"] is True and
                report["release"]["manifest"] == receipt(directory / "release/manifest.json", directory) and
                report["source_snapshot"] == proof["source_snapshot"], "No successful final release receipt")
    return {"status": "verified", "version": proof["version"], "manifest": receipt(directory / "release/manifest.json", directory),
            "field_accuracy_validated": False, "verification_uses_retained_files": True}


def cleanup(directory, execute=False):
    directory = ordinary(directory, True)
    marker_path = directory / "OWNED-RUN.json"; marker = read(marker_path)
    report = read(directory / "report.json")
    require(marker["kind"] == "geosonnar-public-validation" and marker["run_root"] == str(directory) and
            marker["scratch"] == "scratch" and report["owner"] == receipt(marker_path, directory) and
            report["status"] in {"passed", "failed", "unavailable"}, "Cleanup requires its own completed run")
    if report["tier"] == "release" and report["status"] == "passed": verify_release(directory)
    scratch = directory / "scratch"
    require(scratch.parent == directory and scratch.name == "scratch", "Cleanup target differs")
    paths = files(scratch)  # No links, junctions or special files anywhere in the owned tree.
    result = {"status": "preview", "target": str(scratch), "files": len(paths),
              "bytes": sum(path.stat().st_size for path in paths), "only_owned_scratch": True}
    if execute:
        require(not (directory / "cleanup.json").exists(), "Cleanup receipt already exists")
        ordinary(scratch, True)
        shutil.rmtree(scratch)
        require(not os.path.lexists(scratch), "Owned cleanup incomplete")
        result["status"] = "removed_owned_scratch"
        write(directory / "cleanup.json", result)
    return result
