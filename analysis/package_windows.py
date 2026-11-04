"""Assemble a new Windows GUI folder and its complete local license inventory.

This helper copies supplied executables; it does not build or run validation.
Cargo metadata is resolved offline for the locked Windows GUI dependency graph.
Published crates without license documents require checked upstream supplements
in analysis/licenses/rust-supplements.json. No recording or user state is copied.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
from urllib.parse import urlparse


ROOT = Path(__file__).resolve().parents[1]
TARGET = "x86_64-pc-windows-msvc"
METADATA_COMMAND = [
    "cargo", "metadata", "--format-version", "1", "--features", "gui",
    "--locked", "--offline", "--filter-platform", TARGET,
]
SUPPLEMENTS = ROOT / "analysis/licenses/rust-supplements.json"
DEFAULT_README = """GeoSonnar for Windows

Start geosonnar-gui.exe. If graphics cause problems, use geosonnar-safe.exe
for software rendering. Keep geosonnar.exe beside both launchers.

Choose the actual FFmpeg and FFprobe executable paths in Settings to work with
recordings. Those media tools and recordings are separate inputs, not bundled.
Use a writable folder for this portable copy and for your saved cases.

The interface supports saved cases, acoustic event review, and processing jobs.
Keeping an acoustic proposal or completing a job does not verify a location.
Review the evidence and declared search limits before interpreting a result.

package-receipt.json identifies the supplied binaries and packaging inputs.
The packager does not build binaries or run validation. An optional referenced
validation report is identified by its hash, without copying its private inputs.
See LICENSE, THIRD_PARTY_NOTICES.md, NATURAL_EARTH.txt, and
licenses/rust/manifest.json for notices.
"""


def fingerprint(path):
    path = Path(path)
    if not path.is_file():
        raise ValueError(f"Required file is missing: {path}")
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            size += len(chunk)
            digest.update(chunk)
    return {"bytes": size, "sha256": digest.hexdigest()}


def confined(path, root):
    resolved = Path(path).resolve(strict=True)
    if not resolved.is_relative_to(root.resolve(strict=True)):
        raise ValueError(f"License file escapes its source directory: {path}")
    if not resolved.is_file():
        raise ValueError(f"License document must be a file: {path}")
    return resolved


def source_inventory():
    paths = {ROOT / "Cargo.toml", ROOT / "Cargo.lock", Path(__file__).resolve(),
             ROOT / "src/gui/world-map.json"}
    for folder in ("src", "tests", "examples"):
        paths.update((ROOT / folder).rglob("*.rs"))
    # XML/JSON fixtures used by include_str! are compiled inputs as well.
    paths.update(path for path in (ROOT / "tests/fixtures").rglob("*") if path.is_file())
    for optional in ("build.rs", ".cargo/config.toml"):
        if (ROOT / optional).is_file():
            paths.add(ROOT / optional)
    return [{"path": path.relative_to(ROOT).as_posix(), **fingerprint(path)}
            for path in sorted(paths, key=lambda p: p.relative_to(ROOT).as_posix())]


def windows_executable(path):
    with path.open("rb") as stream:
        header = stream.read(64)
        if len(header) != 64 or header[:2] != b"MZ":
            raise ValueError(f"Not a Windows executable: {path}")
        stream.seek(struct.unpack_from("<I", header, 60)[0])
        pe = stream.read(6)
    if len(pe) != 6 or pe[:4] != b"PE\0\0" or pe[4:] != b"\x64\x86":
        raise ValueError(f"Expected an x86-64 Windows executable: {path}")


def read_supplements():
    if not SUPPLEMENTS.exists():
        return {}, None
    before = fingerprint(SUPPLEMENTS)
    document = json.loads(SUPPLEMENTS.read_text(encoding="utf-8-sig"))
    if document.get("schema_version") != 1 or not isinstance(document.get("packages"), list):
        raise ValueError("Unsupported Rust license supplements manifest")
    entries = {}
    for entry in document["packages"]:
        key = (entry["name"], entry["version"])
        if key in entries or not isinstance(entry.get("reason"), str) or not entry["reason"].strip():
            raise ValueError(f"Duplicate or unexplained license supplement: {key}")
        if not isinstance(entry.get("files"), list) or not entry["files"]:
            raise ValueError(f"License supplement has no documents: {key}")
        entries[key] = entry
    if fingerprint(SUPPLEMENTS) != before:
        raise ValueError("License supplement manifest changed during reading")
    return entries, {"path": SUPPLEMENTS.relative_to(ROOT).as_posix(), **before}


def dependency_licenses(metadata, supplements):
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    root_id = metadata["resolve"]["root"]
    pending, reachable = [root_id], set()
    while pending:
        package_id = pending.pop()
        if package_id not in reachable:
            reachable.add(package_id)
            pending.extend(nodes[package_id]["dependencies"])
    copies, inventory, missing, destinations = [], [], [], set()
    for package_id in sorted(reachable - {root_id}, key=lambda k: (packages[k]["name"], packages[k]["version"], k)):
        package = packages[package_id]
        name, version = package["name"], package["version"]
        directory = f"{name}-{version}"
        if not re.fullmatch(r"[A-Za-z0-9_.+-]+", directory) or directory in destinations:
            raise ValueError(f"Unsafe or duplicate dependency directory: {directory}")
        destinations.add(directory)
        crate = Path(package["manifest_path"]).resolve(strict=True).parent
        files = set()
        for current, folders, filenames in os.walk(crate, followlinks=False):
            folders.sort()
            for filename in sorted(filenames):
                path = Path(current) / filename
                if filename.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT", "UNLICENSE", "OFL")):
                    files.add(path)
                elif name == "epaint_default_fonts" and "fonts" in path.relative_to(crate).parts and path.suffix.lower() in (".txt", ".md"):
                    files.add(path)
        if package.get("license_file"):
            files.add(crate / package["license_file"])
        documents = []
        for path in sorted(files):
            source = confined(path, crate)
            relative = path.relative_to(crate).as_posix()
            target = f"licenses/rust/{directory}/crate/{relative}"
            identity = fingerprint(source)
            if not identity["bytes"]:
                raise ValueError(f"License document is empty: {source}")
            copies.append((source, target, identity))
            documents.append({"path": target, "origin": "published_crate", "source_path": relative, **identity})
        supplement = supplements.get((name, version))
        if supplement:
            for index, item in enumerate(supplement["files"], 1):
                relative = Path(item["path"])
                if relative.is_absolute() or ".." in relative.parts:
                    raise ValueError(f"Supplement path must stay relative to its manifest: {relative}")
                source = confined(SUPPLEMENTS.parent / relative, SUPPLEMENTS.parent)
                url = urlparse(item["source_url"])
                if url.scheme != "https" or not url.netloc or url.username or url.password:
                    raise ValueError(f"Supplement needs an authoritative HTTPS source URL: {name}")
                identity = fingerprint(source)
                if not identity["bytes"] or identity["sha256"] != item["sha256"]:
                    raise ValueError(f"Supplement fingerprint mismatch: {source}")
                target = f"licenses/rust/{directory}/supplemental/{index:02d}-{source.name}"
                copies.append((source, target, identity))
                documents.append({"path": target, "origin": "upstream_supplement", "source_url": item["source_url"], "reason": supplement["reason"], **identity})
        if not documents:
            missing.append(f"{name} {version} ({package.get('license') or 'no SPDX expression'})")
        elif name == "epaint_default_fonts" and not supplement and all(
                item.get("source_path", "").startswith("fonts/") for item in documents):
            missing.append(f"{name} {version}: font notices do not replace the crate's own declared license")
        if not package.get("license") and not package.get("license_file"):
            missing.append(f"{name} {version}: neither SPDX license nor license_file declared")
        inventory.append({
            "name": name, "version": version, "package_id": package_id,
            "spdx": package.get("license"), "declared_license_file": package.get("license_file"),
            "repository": package.get("repository"), "source": package.get("source"),
            "resolved_features": sorted(nodes[package_id]["features"]),
            "documents": documents,
        })
    if missing:
        raise ValueError("Missing license documents/declarations; supply checked upstream supplements:\n  " + "\n  ".join(missing))
    return copies, inventory


def copy_checked(source, destination, expected):
    destination.parent.mkdir(parents=True, exist_ok=True)
    digest, size = hashlib.sha256(), 0
    with source.open("rb") as src, destination.open("xb") as dst:
        for chunk in iter(lambda: src.read(1024 * 1024), b""):
            dst.write(chunk)
            digest.update(chunk)
            size += len(chunk)
        dst.flush()
        os.fsync(dst.fileno())
    actual = {"bytes": size, "sha256": digest.hexdigest()}
    if actual != expected or fingerprint(source) != expected or fingerprint(destination) != expected:
        raise ValueError(f"Input changed or copy failed integrity check: {source}")


def write_json(path, document):
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(document, stream, indent=2, sort_keys=True, ensure_ascii=False, allow_nan=False)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def package(args):
    output = args.output.absolute()
    if os.path.lexists(output):
        raise ValueError(f"Output already exists; choose a new package directory: {output}")
    binary_dir = args.binary_dir.resolve(strict=True)
    sources = source_inventory()
    copies, binaries = [], []
    for name in ("geosonnar.exe", "geosonnar-gui.exe"):
        source = binary_dir / name
        windows_executable(source)
        identity = fingerprint(source)
        copies.append((source, name, identity))
        binaries.append({"path": name, **identity})
    # Same GUI executable; the exact safe filename selects the tested software
    # renderer before any window or graphics context is created.
    safe_source = binary_dir / "geosonnar-gui.exe"
    safe_identity = fingerprint(safe_source)
    copies.append((safe_source, "geosonnar-safe.exe", safe_identity))
    binaries.append({"path": "geosonnar-safe.exe", **safe_identity,
                     "launch_mode": "software_direct3d12_no_hardware_fallback"})
    for source, name in ((ROOT / "LICENSE", "LICENSE"),
                         (ROOT / "THIRD_PARTY_NOTICES.md", "THIRD_PARTY_NOTICES.md"),
                         (ROOT / "analysis/licenses/DOPPLERGEIST-MIT.txt", "DOPPLERGEIST-MIT.txt"),
                         (ROOT / "docs/NATURAL_EARTH.txt", "NATURAL_EARTH.txt"),
                         (ROOT / "Cargo.lock", "Cargo.lock")):
        copies.append((source, name, fingerprint(source)))
    if args.readme:
        copies.append((args.readme, "README.txt", fingerprint(args.readme)))
    validation = ({"file_name": args.validation.name, **fingerprint(args.validation),
                   "execution": "provided_report_only; packager_did_not_run_validation"}
                  if args.validation else None)
    supplements, supplement_receipt = read_supplements()
    completed = subprocess.run(METADATA_COMMAND, cwd=ROOT, capture_output=True, text=True, encoding="utf-8", check=False)
    if completed.returncode:
        raise ValueError(f"Offline locked Cargo metadata failed:\n{completed.stderr.strip()}")
    metadata = json.loads(completed.stdout)
    if Path(metadata["workspace_root"]).resolve() != ROOT:
        raise ValueError("Cargo metadata belongs to a different workspace")
    license_copies, packages = dependency_licenses(metadata, supplements)
    copies.extend(license_copies)
    paths = [target.casefold() for _, target, _ in copies]
    if len(paths) != len(set(paths)):
        raise ValueError("Package inputs would overwrite a destination")
    # Preflight above leaves no destination when licenses or inputs are missing.
    output.mkdir(parents=True, exist_ok=False)
    incomplete = output / "PACKAGE-INCOMPLETE.txt"
    incomplete.write_text("Packaging has not completed. Do not distribute this directory.\n", encoding="utf-8")
    for source, target, expected in copies:
        copy_checked(source, output / target, expected)
    if not args.readme:
        (output / "README.txt").write_text(DEFAULT_README, encoding="utf-8", newline="\n")
    license_manifest = {
        "schema_version": 1, "target": TARGET, "cargo_metadata_command": METADATA_COMMAND,
        "cargo_lock": fingerprint(ROOT / "Cargo.lock"), "supplements_manifest": supplement_receipt,
        "scope": "All resolved Windows GUI dependency nodes, including build dependencies; document inclusion does not assert every dependency is linked into both executables.",
        "package_count": len(packages), "packages": packages,
    }
    write_json(output / "licenses/rust/manifest.json", license_manifest)
    if source_inventory() != sources:
        raise ValueError("Source snapshot changed while packaging; use a fresh destination after edits finish")
    if supplement_receipt and fingerprint(SUPPLEMENTS) != {key: supplement_receipt[key] for key in ("bytes", "sha256")}:
        raise ValueError("License supplements manifest changed while packaging")
    if args.validation and fingerprint(args.validation) != {key: validation[key] for key in ("bytes", "sha256")}:
        raise ValueError("Referenced validation report changed while packaging")
    artifacts = [{"path": path.relative_to(output).as_posix(), **fingerprint(path)}
                 for path in sorted(output.rglob("*")) if path.is_file() and path != incomplete]
    receipt = {
        "schema_version": 1, "status": "completed", "target": TARGET,
        "build_profile": "provided", "binary_directory_name": binary_dir.name,
        "binaries": binaries, "artifacts": artifacts, "source_files": sources,
        "source_binding": "Snapshot at packaging time; the packager did not rebuild these binaries or verify they were built from this snapshot.",
        "validation_report": validation, "validation_run_by_packager": False,
        "location_verified": False,
    }
    write_json(output / "package-receipt.json", receipt)
    incomplete.unlink()
    return {"output": str(output), "package_count": len(packages), "artifact_count": len(artifacts), "binaries": binaries}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary-dir", type=Path, default=ROOT / "target/release")
    parser.add_argument("--output", type=Path, required=True, help="New directory; existing paths are refused")
    parser.add_argument("--validation", type=Path, help="Optional existing report; hash only, never executed or copied")
    parser.add_argument("--readme", type=Path, help="Optional current usage text to copy as README.txt")
    args = parser.parse_args()
    try:
        print(json.dumps(package(args), indent=2))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Packaging failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
