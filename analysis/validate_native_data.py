"""Source-informed native ADSB.lol/terrain integration replay, without truth input.

Uses an already downloaded verified cache. No private clip or reference location
is read. Rust performs archive/terrain import, search preparation and ranking.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


def read(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))


def write(path, value):
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False) + "\n", encoding="utf-8")


def receipt(path):
    path = Path(path).resolve()
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    return {"path": str(path), "bytes": path.stat().st_size, "sha256": digest}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--exe", type=Path, default=ROOT / "target/release/geosonnar.exe")
    parser.add_argument("--manifest", type=Path, default=ROOT / "data/adsblol/2026-09-08/cache/archive-manifest.json")
    parser.add_argument("--import-config", type=Path, default=ROOT / "cases/adsblol_trace_import.json")
    parser.add_argument("--terrain-config", type=Path, default=ROOT / "cases/native_terrain.json")
    parser.add_argument("--priors", type=Path, default=ROOT / "output/benchmarks/audio-priors/pipeline-001/audio-priors/priors.json")
    parser.add_argument("--geometry", type=Path, default=ROOT / "output/benchmarks/audio-priors/pipeline-001/geometry.json")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    inputs = {name: receipt(getattr(args, name)) for name in
              ["exe", "manifest", "import_config", "terrain_config", "priors", "geometry"]}
    executable = out / "geosonnar.exe"
    shutil.copy2(args.exe, executable)
    report = {"schema_version": 1, "status": "running", "inputs": inputs,
              "executable": receipt(executable), "stages": [], "location_verified": False,
              "scope": "Native data adapter replay; source-informed date, two-hour UTC interval, region and reviewed acoustic priors. No evaluation coordinate input."}
    started = time.monotonic()

    def run(name, *arguments, allowed=(0,)):
        command = [str(executable), *map(str, arguments)]
        stage = {"name": name, "command": command}
        report["stages"].append(stage)
        write(out / "report.json", report)
        tick = time.monotonic()
        print(name, flush=True)
        with (out / (name + ".stdout.txt")).open("w", encoding="utf-8") as stdout, (out / (name + ".stderr.txt")).open("w", encoding="utf-8") as stderr:
            result = subprocess.run(command, cwd=ROOT, stdout=stdout, stderr=stderr, timeout=1800)
        stage.update(returncode=result.returncode, seconds=time.monotonic() - tick)
        if result.returncode not in allowed:
            raise RuntimeError(f"{name} exited {result.returncode}; inspect retained logs")

    try:
        run("prepare-import", "prepare-adsblol-import", "--manifest", args.manifest.resolve(),
            "--config", args.import_config.resolve(), "--out", out / "import-request.json")
        run("import", "import-adsblol", "--request", out / "import-request.json", "--out", out / "tracks")
        run("terrain", "terrain-grid", "--config", args.terrain_config.resolve(), "--out", out / "terrain")
        geometry = read(args.geometry)
        geometry.update(normalized_tracks=str(out / "tracks/normalized.json"), observers_file=str(out / "terrain/observers.json"))
        geometry["provenance"].append("Official raw ADSB.lol archive, geometric-only height policy, explicit source-informed regional/time filter, native terrain.")
        write(out / "geometry.json", geometry)
        run("prepare-search", "prepare-audio-search", "--priors", args.priors.resolve(),
            "--geometry", out / "geometry.json", "--out", out / "search")
        run("rank", "rank", "--input", out / "search/search_input.json", "--out", out / "results.json", allowed=(0, 3))
        for name, before in inputs.items():
            if receipt(before["path"]) != before:
                raise RuntimeError(f"Input changed during replay: {name}")
        imported, terrain, predictions = read(out / "tracks/receipt.json"), read(out / "terrain/receipt.json"), read(out / "results.json")
        report.update(status="completed", archive_import_complete=imported["archive_import_complete"],
                      real_world_coverage_complete=False, import_stats=imported["stats"],
                      observer_count=terrain["observer_count"], search_status=predictions["status"],
                      predictions=receipt(out / "results.json"), inputs_unchanged=True)
    except Exception as error:
        report.update(status="failed", error=str(error))
        raise
    finally:
        report["seconds"] = time.monotonic() - started
        write(out / "report.json", report)


if __name__ == "__main__":
    main()
