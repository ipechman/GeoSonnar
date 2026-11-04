"""Acquire a preregistered fresh deployment with separate audio-only review assets."""
import argparse
from datetime import datetime, timezone, timedelta
import hashlib
import json
from pathlib import Path
import re
import wave
import zipfile

from field_validation_acquire import Budget, RangeReader, riff_metadata, sha256, write_json


def clock(name):
    return datetime.strptime(Path(name).stem, "%Y%m%d_%H%M%S").replace(tzinfo=timezone.utc)


def collect(policy_path, output, range_cache=None):
    policy_path, output = Path(policy_path).resolve(), Path(output).resolve()
    policy_raw = policy_path.read_bytes()
    policy = json.loads(policy_raw)
    output.mkdir(parents=True, exist_ok=False)
    provenance, truth, review = [output / name for name in ("provenance", "truth", "review")]
    for folder in (provenance, truth, review): folder.mkdir()
    (provenance / "frozen-policy.json").write_bytes(policy_raw)
    budget = Budget(limit=95_000_000, journal=provenance / "http-journal.jsonl", cache=range_cache or provenance / "ranges")
    record_id = policy["source_record"].rsplit("/", 1)[-1]
    record_raw = budget.fetch("https://zenodo.org/api/records/" + record_id, 2_000_000)
    metadata = json.loads(record_raw)
    (provenance / "record.json").write_bytes(record_raw)
    if metadata["metadata"].get("license", {}).get("id") != "cc-by-4.0":
        raise ValueError("Require the checked CC-BY4 primary license")
    site = re.search(r"ESID#\s*(\d+)", metadata["metadata"]["title"]).group(1).zfill(3)
    files = {item["key"]: item for item in metadata["files"]}
    for name in ("total_eclipse_data.csv", "file_list.csv", "License.txt", "README.md"):
        item = files[name]
        if item["size"] > 2_000_000:
            raise ValueError("Unexpectedly large metadata")
        data = budget.fetch(item["links"]["self"], item["size"])
        if len(data) != item["size"] or "md5:" + hashlib.md5(data).hexdigest() != item["checksum"]:
            raise ValueError("Primary metadata checksum mismatch")
        (truth if name == "total_eclipse_data.csv" else provenance).joinpath(name).write_bytes(data)
    # These primary metadata bytes may contain coordinates. No review-stage file
    # includes them, and this process never parses or prints the truth CSV.
    archive_names = [f"ESID_{site}_2024_04_08.zip", f"ESID_{site}.zip"]
    choices = [files[name] for name in archive_names if name in files]
    if not choices:
        raise ValueError("No declared site/day archive; do not select another source")
    archive = choices[0]
    reader = RangeReader(archive["links"]["self"], archive["size"], budget)
    earliest = datetime.fromisoformat(policy["start_not_before_utc"].replace("Z", "+00:00"))
    selected = []
    with zipfile.ZipFile(reader) as bundle:
        members = sorted([item for item in bundle.infolist()
                          if re.fullmatch(r"20240408_\d{6}\.WAV", Path(item.filename).name, re.IGNORECASE)
                          and item.file_size > 4096 and clock(item.filename) >= earliest], key=lambda item: (clock(item.filename), item.filename))
        if not members:
            raise ValueError("No eligible April8 waveform at/after declared UTC; retain this failure")
        write_json(provenance / "eligible-inventory.json", [{"name": m.filename, "bytes": m.file_size, "compressed_bytes": m.compress_size, "crc32": m.CRC} for m in members])
        first_clock, expected_clock, format_key, remaining, total_frames = clock(members[0].filename), None, None, None, 0
        stopped_at_gap = False
        destination = None
        try:
            for index, member in enumerate(members):
                timestamp = clock(member.filename)
                if expected_clock is not None and abs((timestamp - expected_clock).total_seconds()) > 1e-6:
                    stopped_at_gap = True
                    break
                with bundle.open(member) as stream:
                    prefix = stream.read(4096)
                (provenance / f"header-{index:03}.bin").write_bytes(prefix)
                header = riff_metadata(prefix)
                comment = " ".join(item.get("text") or "" for item in header)
                expected_text = timestamp.strftime("Recorded at %H:%M:%S %d/%m/%Y (UTC)")
                if expected_text not in comment:
                    raise ValueError("Original header does not independently confirm filename UTC")
                with bundle.open(member) as stream, wave.open(stream, "rb") as source:
                    params = source.getparams()
                    current_key = params.nchannels, params.sampwidth, params.framerate, params.comptype
                    if params.nchannels != 1 or params.sampwidth != 2 or params.framerate not in (8000, 16000, 24000, 32000, 48000, 96000) or params.comptype != "NONE":
                        raise ValueError("Unsupported original audio format")
                    if destination is None:
                        format_key = current_key
                        remaining = int(policy["maximum_duration_s"] * params.framerate)
                        destination = wave.open(str(review / "recording.wav"), "wb")
                        destination.setparams(params._replace(nframes=remaining))
                    elif format_key != current_key:
                        raise ValueError("Consecutive originals have incompatible sample formats")
                    count = min(remaining, params.nframes)
                    consumed = 0
                    while consumed < count:
                        take = min(count-consumed, params.framerate*5)
                        data = source.readframes(take)
                        if len(data) != take*params.sampwidth*params.nchannels:
                            raise ValueError("Truncated original PCM")
                        destination.writeframesraw(data)
                        consumed += take
                    selected.append({"name": member.filename, "source_bytes": member.file_size,
                                     "compressed_bytes": member.compress_size, "source_crc32": member.CRC,
                                     "source_frames": params.nframes, "retained_frames": count,
                                     "sample_rate_hz": params.framerate, "header": header,
                                     "clock_utc": timestamp.isoformat(), "full_member_read": count == params.nframes,
                                     "whole_member_crc_verified": False})
                    total_frames += count
                    remaining -= count
                    expected_clock = timestamp + timedelta(seconds=params.nframes/params.framerate)
                    print(f"Copied original block{index+1}: {total_frames/params.framerate:.1f}s retained", flush=True)
                if remaining == 0: break
        finally:
            if destination is not None: destination.close()
    if not selected or not total_frames:
        raise ValueError("No complete original samples retained")
    waveform = review / "recording.wav"
    audio_hash = sha256(waveform.read_bytes())
    review_receipt = {"schema_version": 1, "case_id": policy["case_id"], "audio_path": "recording.wav",
                      "audio_sha256": audio_hash, "sample_zero_utc_claim": first_clock.isoformat(),
                      "sample_zero_error_bound_s": None, "sample_rate_hz": format_key[2],
                      "channels": format_key[0], "sample_width_bytes": format_key[1],
                      "frames": total_frames, "duration_s": total_frames/format_key[2],
                      "original_blocks": len(selected), "stopped_at_gap": stopped_at_gap,
                      "broad_region_south_west_north_east": policy["broad_aircraft_region_south_west_north_east"],
                      "coordinate_truth_included": False, "aircraft_ids_included": False,
                      "blindness": policy["blindness"], "exact_publisher_location_error_bound_m": None,
                      "transformation": "Original mono PCM samples in chronological order; only strictly contiguous blocks combined; no resampling, padding, normalization or time shift",
                      "policy_sha256": sha256(policy_raw), "event_selection": "No acoustic screening or detector-based interval selection"}
    write_json(review / "audio-only.json", review_receipt)
    provenance_receipt = {"schema_version": 1, "source_record": policy["source_record"], "site_id": site,
                          "license": "CC BY4.0", "creators": metadata["metadata"]["creators"],
                          "original_archive": {"name": archive["key"], "bytes": archive["size"], "checksum": archive["checksum"], "whole_archive_checksum_verified": False},
                          "selected_members": selected, "review_receipt_sha256": sha256((review / "audio-only.json").read_bytes()),
                          "http_payload_bytes": budget.received, "http_requests": budget.requests,
                          "truth_csv_sha256": sha256((truth / "total_eclipse_data.csv").read_bytes()),
                          "truth_csv_parsed": False, "acquisition_agent_saw_public_coordinates_in_search_result": policy.get("acquisition_agent_saw_exact_coordinates", True),
                          "no_aircraft_data_read": True, "geolocation_accuracy_validated": False}
    write_json(provenance / "acquisition.json", provenance_receipt)
    print(json.dumps({"review_folder": str(review), "sample_zero_utc_claim": first_clock.isoformat(),
                      "duration_s": total_frames/format_key[2], "original_blocks": len(selected),
                      "audio_sha256": audio_hash, "http_payload_bytes": budget.received}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", default="benchmarks/field_validation_fresh_policy.json")
    parser.add_argument("--output", required=True)
    parser.add_argument("--range-cache", type=Path)
    args = parser.parse_args()
    collect(args.policy, args.output, args.range_cache)
