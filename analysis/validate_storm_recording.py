"""Acquire a bounded public storm sample and audit real-recording readiness.

This is an integrity/audio integration lane, not a location-accuracy pass. Missing
receiver truth, strike positions/times or aircraft evidence stay explicit.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import math
from pathlib import Path, PurePosixPath
import struct
import subprocess
import urllib.request
import zlib

ROOT = Path(__file__).resolve().parents[1]
API = "https://api.figshare.com/v2/articles/34020543"
PAGE = "https://figshare.com/articles/dataset/Triggered-Lightning_DAS_Dataset/34020543"
ARCHIVE_URL = "https://ndownloader.figshare.com/files/69449499"
ARCHIVE_SIZE = 485024746
ARCHIVE_MD5 = "bf1aa2744778b7c79c8d105adac275ce"
MAX_RANGE = 1024 * 1024


def write_json(path, value):
    with Path(path).open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, allow_nan=False)
        stream.write("\n")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def new_directory(path):
    path = Path(path).resolve()
    path.mkdir(parents=True, exist_ok=False)
    return path


def fetch_range(start, length):
    if start < 0 or not 0 < length <= MAX_RANGE or start + length > ARCHIVE_SIZE:
        raise ValueError("Invalid or excessive byte range")
    end = start + length - 1
    request = urllib.request.Request(ARCHIVE_URL, headers={"Range": f"bytes={start}-{end}"})
    with urllib.request.urlopen(request, timeout=45) as response:
        if response.status != 206 or response.headers.get("Content-Range") != f"bytes {start}-{end}/{ARCHIVE_SIZE}":
            raise ValueError("Server did not honor the exact bounded range; no full archive download")
        data = response.read(length + 1)
    if len(data) != length:
        raise ValueError("Incomplete/excess range response")
    return data


def central_entries(data):
    entries, position = [], 0
    while position < len(data):
        if len(entries) >= 128 or len(data) - position < 46:
            raise ValueError("Invalid/oversized ZIP directory")
        row = struct.unpack_from("<4s6H3L5H2L", data, position)
        if row[0] != b"PK\x01\x02":
            raise ValueError("Invalid ZIP directory signature")
        name_size, extra_size, comment_size = row[10:13]
        end = position + 46 + name_size + extra_size + comment_size
        if end > len(data):
            raise ValueError("Truncated ZIP directory entry")
        name = data[position + 46:position + 46 + name_size].decode("utf-8")
        path = PurePosixPath(name)
        if path.is_absolute() or ".." in path.parts or "\\" in name or ":" in name:
            raise ValueError("Unsafe archive member name")
        entries.append({"name": name, "flags": row[3], "method": row[4], "crc32": row[7],
                        "compressed_bytes": row[8], "bytes": row[9], "offset": row[16]})
        position = end
    if len({e["name"] for e in entries}) != len(entries):
        raise ValueError("Duplicate archive member names")
    return entries


def inflate_member(entry, name_and_data, local_header):
    header = struct.unpack("<4s5H3L2H", local_header)
    if header[0] != b"PK\x03\x04" or header[2] != entry["flags"] or header[3] != entry["method"]:
        raise ValueError("Local archive header differs from directory")
    name_size, extra_size = header[-2:]
    if name_and_data[:name_size].decode("utf-8") != entry["name"]:
        raise ValueError("Archive member name differs from directory")
    if entry["flags"] & 1 or entry["method"] != 8 or not 632 < entry["bytes"] <= 128 * 1024:
        raise ValueError("Unsupported/encrypted/oversized selected member")
    compressed = name_and_data[name_size + extra_size:]
    if len(compressed) != entry["compressed_bytes"]:
        raise ValueError("Compressed member size mismatch")
    inflater = zlib.decompressobj(-15)
    raw = inflater.decompress(compressed, entry["bytes"] + 1)
    if len(raw) != entry["bytes"] or not inflater.eof or inflater.unused_data or inflater.unconsumed_tail:
        raise ValueError("Invalid or excessive inflated member")
    if zlib.crc32(raw) != entry["crc32"]:
        raise ValueError("Archive member CRC mismatch")
    return raw


def acquire(output):
    output = new_directory(output)
    with urllib.request.urlopen(API, timeout=45) as response:
        data = response.read(MAX_RANGE + 1)
    if len(data) > MAX_RANGE:
        raise ValueError("Dataset metadata exceeds size limit")
    metadata = json.loads(data)
    write_json(output / "provider-metadata.json", metadata)
    file = next(f for f in metadata["files"] if f["id"] == 69449499)
    if file["size"] != ARCHIVE_SIZE or file["computed_md5"] != ARCHIVE_MD5 or file["download_url"] != ARCHIVE_URL:
        raise ValueError("Provider archive identity changed; do not silently use another version")
    if metadata["license"]["name"] != "CC BY 4.0":
        raise ValueError("Provider license changed")
    tail = fetch_range(ARCHIVE_SIZE - 65557, 65557)
    position = tail.rfind(b"PK\x05\x06")
    if position < 0 or len(tail) - position < 22:
        raise ValueError("ZIP end record absent")
    end = struct.unpack_from("<4s4H2LH", tail, position)
    if any(end[i] != 0 for i in (1, 2)) or end[3] != end[4] or position + 22 + end[7] != len(tail):
        raise ValueError("Unsupported multipart or malformed archive")
    directory = fetch_range(end[6], end[5])
    entries = central_entries(directory)
    if len(entries) != end[4]:
        raise ValueError("ZIP entry inventory mismatch")
    write_json(output / "archive-directory.json", entries)
    selected = [e for e in entries if "/Infrasound." in e["name"] or
                ("/flash4/" in e["name"] and ".DPZ." in e["name"])]
    if len(selected) != 10:
        raise ValueError("Expected eight infrasound and two companion seismometer recordings")
    saved = []
    received = len(data) + len(tail) + len(directory)
    for entry in selected:
        header = fetch_range(entry["offset"], 30)
        fields = struct.unpack("<4s5H3L2H", header)
        size = fields[-2] + fields[-1] + entry["compressed_bytes"]
        payload = fetch_range(entry["offset"] + 30, size)
        received += 30 + size
        raw = inflate_member(entry, payload, header)
        name = entry["name"].replace("/", "__")
        with (output / name).open("xb") as stream:
            stream.write(raw)
        saved.append({**entry, "path": name, "sha256": digest(raw), "crc32_verified": True})
    manifest = {"schema_version": 1, "dataset_url": PAGE, "api_url": API,
                "attribution": "Heting Hong, Triggered-Lightning DAS Dataset, Figshare, 2026, DOI 10.6084/m9.figshare.34020543.v1",
                "license": "CC BY 4.0", "license_url": "https://creativecommons.org/licenses/by/4.0/",
                "archive_url": ARCHIVE_URL, "archive_bytes": ARCHIVE_SIZE, "archive_md5": ARCHIVE_MD5,
                "whole_archive_md5_verified": False, "directory_sha256": digest(directory),
                "http_payload_bytes": received, "files": saved,
                "scope": "Selected member CRC and SHA-256 verified; whole archive deliberately not downloaded"}
    write_json(output / "manifest.json", manifest)
    return manifest


def optional(value):
    return None if value == -12345 else value


def parse_sac(data):
    if len(data) < 636 or len(data) > 128 * 1024:
        raise ValueError("SAC file outside bounded size")
    endian = next((e for e in ("<", ">") if struct.unpack_from(e + "i", data, 304)[0] == 6), None)
    if endian is None:
        raise ValueError("Only SAC version 6 is supported")
    floats = struct.unpack(endian + "70f", data[:280])
    integers = struct.unpack(endian + "40i", data[280:440])
    count = integers[9]
    if not 1 <= count <= 30000 or len(data) != 632 + 4 * count:
        raise ValueError("SAC sample count disagrees with file length")
    if integers[15] != 1 or integers[35] != 1:
        raise ValueError("Only evenly sampled SAC time series are supported")
    delta, begin, end = floats[0], floats[5], floats[6]
    if not all(math.isfinite(v) for v in (delta, begin, end)) or delta <= 0 or delta > 10:
        raise ValueError("Invalid SAC sampling interval")
    if abs(end - (begin + (count - 1) * delta)) > max(delta / 4, 1e-4):
        raise ValueError("SAC end time disagrees with samples")
    y, day, hour, minute, second, ms = integers[:6]
    if not 1900 <= y <= 2100 or not 1 <= day <= 366 or not 0 <= ms <= 999:
        raise ValueError("Invalid SAC reference clock")
    reference = datetime(y, 1, 1, hour, minute, second, ms * 1000, tzinfo=timezone.utc) + timedelta(days=day - 1)
    if reference.year != y:
        raise ValueError("Invalid day of year")
    samples = struct.unpack(endian + f"{count}f", data[632:])
    if any(not math.isfinite(s) for s in samples):
        raise ValueError("SAC contains nonfinite samples")
    strings = [data[440 + i * 8:448 + i * 8].decode("ascii", "replace").strip() for i in range(24)]
    coordinate = {key: optional(floats[i]) for key, i in (("latitude_deg", 31), ("longitude_deg", 32), ("height_m", 33))}
    return {"sample_count": count, "sample_interval_s": delta, "sample_rate_hz": 1 / delta,
            "duration_s": count * delta, "begin_offset_s": begin, "end_offset_s": end,
            "reference_clock_as_utc": reference.isoformat(),
            "sample_zero_as_utc": (reference + timedelta(seconds=begin)).isoformat(),
            "clock_note": "SAC clock interpreted as UTC by format convention; independent synchronization/uncertainty not provided by this release",
            "station_id": strings[0], "channel": strings[20], "station_coordinate": coordinate,
            "source_coordinate": {key: optional(floats[i]) for key, i in (("latitude_deg",35),("longitude_deg",36),("height_m",37))},
            "origin_offset_s": optional(floats[7]), "picked_arrival_s": optional(floats[8])}, samples


def wave_bytes(samples, sample_rate):
    rate = round(sample_rate)
    if abs(rate - sample_rate) > 1e-4 or not 1 <= rate <= 192000:
        raise ValueError("SAC rate cannot be represented by this WAV adapter")
    peak = max(abs(value) for value in samples)
    if peak == 0:
        raise ValueError("Empty signal")
    gain = 0.9 / peak
    payload = struct.pack("<" + "f" * len(samples), *(s * gain for s in samples))
    fmt = struct.pack("<HHIIHH", 3, 1, rate, rate * 4, 4, 32)
    body = b"WAVEfmt " + struct.pack("<I",16) + fmt + b"fact" + struct.pack("<II",4,len(samples)) + b"data" + struct.pack("<I",len(payload)) + payload
    return b"RIFF" + struct.pack("<I",len(body)) + body, gain, rate


def inspect(input_directory, output, native=None):
    source = Path(input_directory).resolve()
    manifest = json.loads((source / "manifest.json").read_text(encoding="utf-8"))
    output = new_directory(output)
    records = []
    for file in manifest["files"]:
        name = file["path"]
        if Path(name).name != name or "\\" in name or "/" in name:
            raise ValueError("Unsafe manifest path")
        data = (source / name).read_bytes()
        if digest(data) != file["sha256"] or len(data) != file["bytes"] or zlib.crc32(data) != file["crc32"]:
            raise ValueError("Original recording differs from acquisition receipt")
        header, samples = parse_sac(data)
        row = {"file": name, "sha256": file["sha256"], "header": header,
               "is_infrasound": "/Infrasound." in file["name"], "finite_samples": True,
               "rms_uncalibrated": math.sqrt(sum(s*s for s in samples)/len(samples))}
        if row["is_infrasound"]:
            wav, gain, rate = wave_bytes(samples, header["sample_rate_hz"])
            destination = output / (name + ".wav")
            with destination.open("xb") as stream: stream.write(wav)
            row["wav"] = {"path":destination.name,"sha256":digest(wav),"gain":gain,"sample_rate_hz":rate,
                          "physical_band_ceiling_hz":rate/2,"note":"Amplitude normalized per recording; no new bandwidth, no pressure calibration, no temporal crop"}
            if native:
                result_file = output / (name + ".native.json")
                command = [str(Path(native).resolve()),"--input",str(destination),"--out",str(result_file),"--duration",str(len(samples)/rate)]
                result = subprocess.run(command,capture_output=True,text=True,timeout=120)
                if result.stdout: (output / (name + ".stdout")).write_text(result.stdout,encoding="utf-8")
                if result.stderr: (output / (name + ".stderr")).write_text(result.stderr,encoding="utf-8")
                row["native_exit_code"] = result.returncode
                if result.returncode: raise RuntimeError("Native audio integration failed; retained logs describe the failure")
                row["native_result"] = json.loads(result_file.read_text(encoding="utf-8"))
                if (row["native_result"].get("source_sha256") != row["wav"]["sha256"] or
                        row["native_result"].get("status") != "completed" or
                        row["native_result"].get("location_verified") is not False):
                    raise ValueError("Native receipt does not verify this waveform-only integration")
        records.append(row)
    infrasound = [r for r in records if r["is_infrasound"]]
    blockers = ["Infrasound receiver coordinates/elevation and their accuracy are absent; companion seismometer coordinates do not establish exact microphone co-location",
                "Independent lightning source positions, heights, emission UTC and uncertainties are absent; SAC origin=0 is not a verified discharge time",
                "Independent clock synchronization and sample-zero uncertainty are unspecified",
                "No reviewed aircraft tones, associations or matching flight tracks are supplied"]
    summary = {"schema_version":1,"status":"waveform_checks_passed_location_validation_blocked","dataset_url":PAGE,
               "real_recordings":len(infrasound),"companion_recordings":len(records)-len(infrasound),
               "native_audio_integration_ran":native is not None,"native_sha256":digest(Path(native).read_bytes()) if native else None,
               "known_location_accuracy_validated":False,"hybrid_accuracy_validated":False,"location_verified":False,
               "blockers":blockers,"records":records,"next_required":"Release-specific independently sourced microphone geometry/clock and lightning metadata, followed by reviewed arrivals; add aircraft evidence for a hybrid comparison"}
    write_json(output / "summary.json", summary)
    return summary


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    commands=parser.add_subparsers(dest="command",required=True)
    fetch=commands.add_parser("fetch");fetch.add_argument("--output",type=Path,required=True)
    check=commands.add_parser("inspect");check.add_argument("--input",type=Path,required=True);check.add_argument("--output",type=Path,required=True);check.add_argument("--native",type=Path)
    args=parser.parse_args()
    result=acquire(args.output) if args.command=="fetch" else inspect(args.input,args.output,args.native)
    print(json.dumps({k:v for k,v in result.items() if k not in ("files","records")},indent=2))
    # Missing scientific prerequisites are an explicit blocked result, not a pass.
    return 0 if args.command=="fetch" else 3


if __name__=="__main__":
    raise SystemExit(main())
