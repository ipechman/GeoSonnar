"""Verify the pinned public reference asset and extract only regional inputs.

This is a one-time research import, not a dependency of the Rust application.
It deliberately never uses tar.extract/extractall or writes archive-provided paths.
"""
import hashlib
import json
import pathlib
import sys
import tarfile
from datetime import datetime, timezone

ARCHIVE_SHA256 = "1f8a03daf8cd851b33f19a47ae65eb4d20d59ce48956e99d51aac7517302d6a6"
ARCHIVE_BYTES = 1234649883
SOURCE_URL = "https://github.com/1337isnot1337/DOPPLERGEIST/releases/download/checkpoint-2026-11-02/data-adsb-derived.tar.gz"
MANIFEST_REVISION = "86c76ee77f31a7e2ca7c731f317518915eac6e49"
PREFIX = "data/adsb/2026-09-08/"
EXPECTED = {
    "reference-region.parquet": (32823780, "fa4e2b82849b3ea8c22ffbe50d4f8f00b7bb3727486307feeaab4821562718a0"),
    "reference-region-ingestion-report.json": (159, "e3dc533e91a690c81e0069dc23f51f24fbf0b3bac48b257533b8df710c687deb"),
    "release.json": (9962, "d48923d6d6a5cc2466305932a72b4daa67522e3222be3a939589d1ff488f5c5f"),
    "type-database.csv": (47916, "e862edbce353514bea97e2d819988f408009edd3cfcb47199ebc648d9a5b600f"),
}


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(8 * 1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def main():
    if len(sys.argv) != 3:
        raise SystemExit("Usage: extract_reference_archive.py <archive> <output-directory>")
    archive_path = pathlib.Path(sys.argv[1]).resolve()
    output_dir = pathlib.Path(sys.argv[2]).resolve()
    if archive_path.stat().st_size != ARCHIVE_BYTES:
        raise SystemExit("Archive size mismatch: download is incomplete or the release changed")
    print("Verifying archive SHA-256...", flush=True)
    archive_hash = digest(archive_path)
    if archive_hash != ARCHIVE_SHA256:
        raise SystemExit(f"Archive checksum mismatch: {archive_hash}")
    output_dir.mkdir(parents=True, exist_ok=True)
    remaining = set(EXPECTED)
    results = []
    print("Archive verified; streaming past national files without extracting them...", flush=True)
    with tarfile.open(archive_path, mode="r|gz") as archive:
        for member in archive:
            if not member.name.startswith(PREFIX):
                continue
            name = member.name[len(PREFIX):]
            if name not in remaining:
                continue
            size, expected_hash = EXPECTED[name]
            if not member.isfile() or member.size != size:
                raise SystemExit(f"Unexpected member type/size: {member.name}")
            destination = output_dir / name
            if destination.parent != output_dir:
                raise SystemExit("Unsafe output path")
            source = archive.extractfile(member)
            if source is None:
                raise SystemExit(f"Cannot read member: {member.name}")
            data = source.read(size + 1)
            actual_hash = hashlib.sha256(data).hexdigest()
            if len(data) != size or actual_hash != expected_hash:
                raise SystemExit(f"Member checksum mismatch: {member.name}")
            destination.write_bytes(data)
            results.append({"member": member.name, "local_path": str(destination), "bytes": size, "sha256": actual_hash})
            remaining.remove(name)
            print(f"Verified and extracted {name} ({size:,} bytes)", flush=True)
            if not remaining:
                break
    if remaining:
        raise SystemExit(f"Missing archive members: {sorted(remaining)}")
    report = {
        "source_url": SOURCE_URL,
        "manifest_revision": MANIFEST_REVISION,
        "manifest_url": f"https://raw.githubusercontent.com/1337isnot1337/DOPPLERGEIST/{MANIFEST_REVISION}/evidence/artifact-manifest.json",
        "archive_path": str(archive_path),
        "archive_bytes": ARCHIVE_BYTES,
        "archive_sha256": archive_hash,
        "verified_utc": datetime.now(timezone.utc).isoformat(),
        "files": results,
        "data_origin": "DOPPLERGEIST regional derivative of adsb.lol historical traces (ODbL). This is another implementation's prepared dataset, not the original creator's unpublished dataset.",
        "national_dataset_extracted": False,
    }
    (output_dir / "import-provenance.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
