"""Verify and restore only the forty Livermore audio/annotation inputs.

The public archive must already be downloaded. Exact pinned manifest members are
streamed into fixed output paths; tar paths, links and executables are never used.
Discovery receives opaque audio identities. Publisher annotations stay separate.
"""
from __future__ import annotations
import argparse
import csv
import hashlib
import json
from pathlib import Path, PurePosixPath
import tarfile
import wave

PIN = "70626ad44fb9c7f4e2f5b225956c1f9281f91c13"
ARCHIVE_SHA256 = "1e8be2cf137052d82d3f9c2fe7ba66c6034e74da10253cf17182debae7ecdcd9"
ARCHIVE_BYTES = 283208311
PREFIX = "data/research/zenodo-23424278/"


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, value):
    with Path(path).open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, allow_nan=False)
        stream.write("\n")


def restore(project, archive, destination):
    project, archive, destination = Path(project).resolve(), Path(archive).resolve(), Path(destination).resolve()
    if destination.exists():
        raise FileExistsError("Use a fresh corpus directory; existing inputs are never overwritten")
    manifest_path = project / "data/cache/dopplergeist/artifact-manifest-pinned.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    asset = next(a for a in manifest["assets"] if a["name"] == "data-research.tar.gz")
    if asset["bytes"] != ARCHIVE_BYTES or asset["sha256"] != ARCHIVE_SHA256:
        raise ValueError("Unexpected archive identity in the pinned checkpoint manifest")
    if archive.stat().st_size != ARCHIVE_BYTES or sha256(archive) != ARCHIVE_SHA256:
        raise ValueError("Corpus archive does not match the pinned size and SHA256")
    all_files = {row["path"]: row for row in asset["files"]}
    audio = sorted(path for path in all_files if path.startswith(PREFIX + "extracted/")
                   and "/__MACOSX/" not in path and path.endswith("/audio.wav"))
    if len(audio) != 40:
        raise ValueError("Expected exactly forty original passage WAVs")
    selections, records = {}, []
    for index, path in enumerate(audio):
        parts = PurePosixPath(path).parts
        case_id, source_case, category = f"clip-{index:03}", parts[-2], parts[-3]
        annotation = str(PurePosixPath(path).with_name("frequency_vs_time_used.csv"))
        if annotation not in all_files:
            raise ValueError(f"Missing published frequency annotation for {case_id}")
        selections[path] = f"audio/{case_id}.wav"
        selections[annotation] = f"annotations/{case_id}.csv"
        records.append({"case_id": case_id, "source_case": source_case, "category": category,
                        "audio_member": path, "annotation_member": annotation})
    for name in ["record.json", "final_results_40_cases.csv"]:
        selections[PREFIX + name] = "source-metadata/" + name
    destination.mkdir(parents=True)
    restored = []
    with tarfile.open(archive, "r|gz") as bundle:
        for member in bundle:
            if member.name not in selections:
                continue
            expected = all_files[member.name]
            if not member.isfile() or member.size != expected["bytes"]:
                raise ValueError("Unexpected selected archive member type or size")
            relative = selections[member.name]
            target = destination / relative
            if not target.resolve().is_relative_to(destination):
                raise ValueError("Destination escaped the corpus root")
            target.parent.mkdir(parents=True, exist_ok=True)
            partial = target.with_suffix(target.suffix + ".part")
            hash_value = hashlib.sha256()
            with bundle.extractfile(member) as source, partial.open("xb") as output:
                while block := source.read(1024 * 1024):
                    hash_value.update(block)
                    output.write(block)
            if hash_value.hexdigest() != expected["sha256"] or partial.stat().st_size != expected["bytes"]:
                raise ValueError(f"Checksum failed for selected member {member.name}")
            partial.rename(target)
            restored.append({"archive_member": member.name, "local_path": relative,
                             "bytes": expected["bytes"], "sha256": expected["sha256"]})
    if len(restored) != len(selections):
        raise ValueError("Archive did not contain every selected corpus member")
    with (destination / "source-metadata/final_results_40_cases.csv").open(newline="", encoding="utf-8-sig") as stream:
        publisher_results = {row["case"]: row for row in csv.DictReader(stream)}
    if set(publisher_results) != {row["source_case"] for row in records}:
        raise ValueError("Published forty-case table differs from the restored WAV identities")
    source_record = json.loads((destination / "source-metadata/record.json").read_text(encoding="utf-8"))
    metadata = source_record.get("metadata", {})
    attribution = {"record_url": "https://zenodo.org/records/23424278", "creators": metadata.get("creators"),
                   "title": metadata.get("title"), "license": metadata.get("license"),
                   "scope": "Original publisher recordings and annotations retain their source license; upstream software license is separate."}
    recording_manifest = {"schema_version": 1, "corpus": "Livermore forty real aircraft passages",
                          "scope": "Audio-only discovery inputs; no aircraft identities, bands, event intervals, flight tracks or recorder coordinates supplied",
                          "source": attribution, "recording_count": 40, "recordings": []}
    annotation_manifest = {"schema_version": 1, "purpose": "Post-run publisher-annotation inspection only; never production discovery settings",
                           "source": attribution, "audio_sample_zero_utc_established": False,
                           "event_level_precision_recall_eligible": False,
                           "limitations": ["Publisher frequency annotations use UTC; no independently established audio sample-zero UTC mapping is supplied.",
                                           "Subtracting the first annotated UTC time defines annotation-relative time, not recording-relative time.",
                                           "A proposal in an aircraft-positive clip is not a confirmed target detection. Extra proposals have no complete negative/event labels.",
                                           "Source annotations may support post-run frequency-range diagnostics; do not treat upstream proposals as truth."],
                           "recordings": []}
    for row in records:
        case_id = row["case_id"]
        audio_file, annotation_file = destination / f"audio/{case_id}.wav", destination / f"annotations/{case_id}.csv"
        with wave.open(str(audio_file), "rb") as audio_info:
            info = {"sample_rate_hz": audio_info.getframerate(), "channels": audio_info.getnchannels(),
                    "sample_count": audio_info.getnframes(), "sample_width_bytes": audio_info.getsampwidth(),
                    "duration_s": audio_info.getnframes() / audio_info.getframerate()}
        recording_manifest["recordings"].append({"case_id": case_id, "audio_path": f"audio/{case_id}.wav",
                                                "bytes": audio_file.stat().st_size, "sha256": all_files[row["audio_member"]]["sha256"], **info})
        with annotation_file.open(newline="", encoding="utf-8-sig") as stream:
            reader = csv.DictReader(stream)
            columns, annotations = reader.fieldnames, list(reader)
        frequencies = [float(p["frequency_hz"]) for p in annotations]
        times = [p["time_utc"] for p in annotations]
        annotation_manifest["recordings"].append({"case_id": case_id, "source_case": row["source_case"], "category": row["category"],
            "annotation_path": f"annotations/{case_id}.csv", "annotation_sha256": all_files[row["annotation_member"]]["sha256"],
            "annotation_columns": columns, "annotation_count": len(annotations), "annotation_time_domain": "publisher UTC",
            "annotation_utc_interval": [min(times), max(times)] if times else None,
            "annotation_frequency_range_hz": [min(frequencies), max(frequencies)] if frequencies else None,
            "recording_relative_event_interval_s": None, "publisher_reference_row": publisher_results[row["source_case"]]})
    receipt = {"schema_version": 1, "upstream_commit": PIN, "checkpoint": manifest["checkpoint"],
               "archive_url": "https://github.com/1337isnot1337/DOPPLERGEIST/releases/download/checkpoint-2026-11-02/data-research.tar.gz",
               "archive_bytes": ARCHIVE_BYTES, "archive_sha256": ARCHIVE_SHA256,
               "checkpoint_manifest_sha256": sha256(manifest_path), "restore_script_sha256": sha256(Path(__file__)),
               "selected_member_count": len(restored), "restored_bytes": sum(row["bytes"] for row in restored),
               "all_selected_hashes_verified": True, "files": restored}
    write_json(destination / "recording-manifest.json", recording_manifest)
    write_json(destination / "annotation-manifest.json", annotation_manifest)
    write_json(destination / "restore-receipt.json", receipt)
    write_json(destination / "source-attribution.json", attribution)
    return {"recordings": 40, "total_duration_s": sum(row["duration_s"] for row in recording_manifest["recordings"]),
            "selected_members": len(restored), "restored_bytes": receipt["restored_bytes"], "destination": str(destination)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(restore(args.project, args.archive, args.destination), indent=2))


if __name__ == "__main__":
    main()
