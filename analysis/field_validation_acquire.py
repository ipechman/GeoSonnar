"""Acquire one bounded, timestamp-preserving Eclipse Soundscapes excerpt.

Independent publisher location metadata is stored separately from the audio.
This does not confirm aircraft identity, microphone survey accuracy or clock error.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import io
import json
from pathlib import Path
import re
import struct
import subprocess
import tempfile
import time
import urllib.request
import wave
import zipfile

MAX_TRANSFER = 95_000_000
BLOCK = 512 * 1024
USER_AGENT = "GeoSonnar-field-validation/1.0 (bounded scientific acquisition)"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value):
    with Path(path).open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, allow_nan=False)
        stream.write("\n")


class Budget:
    def __init__(self, limit=MAX_TRANSFER, journal=None, cache=None):
        self.limit, self.received, self.requests = limit, 0, []
        self.journal = journal
        self.cache = Path(cache) if cache else None
        if self.cache: self.cache.mkdir(exist_ok=True)
        self.old_hashes = {}
        old_journal = self.cache.parent / "http-journal.jsonl" if self.cache else None
        if old_journal and old_journal.is_file():
            for line in old_journal.read_text(encoding="utf-8").splitlines():
                row = json.loads(line)
                if row.get("start") is not None and row.get("bytes") == row.get("requested_bytes") and not row.get("error"):
                    key = sha256(f"{row['url']}\n{row['start']}\n{row['requested_bytes']}".encode())
                    self.old_hashes[key] = row["sha256"]

    def fetch(self, url, size, start=None, archive_size=None):
        cache_path = self.cache / (sha256(f"{url}\n{start}\n{size}".encode()) + ".bin") if self.cache and start is not None else None
        if cache_path and cache_path.is_file():
            data = cache_path.read_bytes()
            if len(data) != size: raise ValueError("Invalid cached range length")
            hash_path = cache_path.with_suffix(".sha256")
            expected = hash_path.read_text().strip() if hash_path.exists() else self.old_hashes.get(cache_path.stem)
            if expected is None or sha256(data) != expected:
                raise ValueError("Cached range differs from its original acquisition receipt")
            self.requests.append({"url": url, "start": start, "bytes": len(data), "sha256": sha256(data), "cache_path": str(cache_path)})
            return data
        if size <= 0 or self.received + size > self.limit:
            raise ValueError("Transfer budget exhausted before request")
        headers = {"User-Agent": USER_AGENT}
        if start is not None:
            if start < 0 or start + size > archive_size:
                raise ValueError("Invalid archive byte range")
            headers["Range"] = f"bytes={start}-{start + size - 1}"
        for attempt in range(2):
            if self.received + size > self.limit:
                raise ValueError("Transfer budget exhausted before retry")
            request = urllib.request.Request(url, headers=headers)
            data, failure = bytearray(), None
            try:
                if start is not None:
                    # Windows' native HTTP stack avoids short SSL reads seen in
                    # this machine's Python environment. No shell interpolation.
                    with tempfile.TemporaryDirectory(prefix="field-range-", dir=self.cache) as temporary:
                        body = Path(temporary) / "body.bin"
                        response_headers = Path(temporary) / "headers.txt"
                        result = subprocess.run(["curl.exe", "--silent", "--show-error", "--fail", "--max-time", "45",
                            "--range", f"{start}-{start + size - 1}", "--max-filesize", str(size),
                            "--output", str(body), "--dump-header", str(response_headers), "--write-out", "%{http_code}", url],
                            capture_output=True, text=True, timeout=50,
                            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
                        if body.exists():
                            if body.stat().st_size > size: raise ValueError("Excess native downloader output")
                            data = bytearray(body.read_bytes())
                        header_text = response_headers.read_text() if response_headers.exists() else ""
                        expected = f"bytes {start}-{start + size - 1}/{archive_size}"
                        ranges = re.findall(r"(?im)^content-range:\s*(.+?)\s*$", header_text)
                        if result.returncode or result.stdout.strip() != "206":
                            failure = f"Native downloader failed: {result.returncode}: {result.stderr.strip()}"
                        elif not ranges or ranges[-1].strip() != expected:
                            raise ValueError("Native downloader range receipt differs from request")
                else:
                    with urllib.request.urlopen(request, timeout=45) as response:
                        while len(data) <= size:
                            block = response.read(min(65536, size + 1 - len(data)))
                            if not block: break
                            data.extend(block)
            except OSError as error:
                failure = f"{type(error).__name__}: {error}"
            data = bytes(data)
            self.received += len(data)
            row = {"url": url, "start": start, "requested_bytes": size, "bytes": len(data),
                   "attempt": attempt, "sha256": sha256(data), "error": failure}
            self.requests.append(row)
            if self.journal:
                with Path(self.journal).open("a", encoding="utf-8") as stream:
                    stream.write(json.dumps(row) + "\n")
            if len(data) > size:
                raise ValueError(f"Excess response length: {len(data)} > {size}")
            if failure is None and (start is None or len(data) == size):
                if cache_path:
                    with cache_path.open("xb") as stream: stream.write(data)
                    cache_path.with_suffix(".sha256").write_text(sha256(data), encoding="ascii")
                    self.old_hashes[cache_path.stem] = sha256(data)
                return data
            if attempt == 0: time.sleep(1)
        raise ValueError(f"Truncated range after bounded retry: {len(data)} != {size}")


class RangeReader(io.RawIOBase):
    """Seekable bounded reader; zipfile supplies the ZIP64 parser."""
    def __init__(self, url, size, budget):
        self.url, self.size, self.budget = url, size, budget
        self.position, self.cache_start, self.cache = 0, -1, b""

    def seekable(self):
        return True

    def readable(self):
        return True

    def tell(self):
        return self.position

    def seek(self, offset, whence=0):
        position = offset if whence == 0 else self.position + offset if whence == 1 else self.size + offset if whence == 2 else -1
        if position < 0 or position > self.size:
            raise ValueError("Seek outside archive")
        self.position = position
        return position

    def read(self, size=-1):
        if size < 0:
            size = self.size - self.position
        if size > MAX_TRANSFER:
            raise ValueError("Unbounded archive read")
        remaining = min(size, self.size - self.position)
        result = bytearray()
        while remaining:
            if not self.cache_start <= self.position < self.cache_start + len(self.cache):
                self.cache_start = self.position
                length = min(max(BLOCK, remaining), self.size - self.position)
                self.cache = self.budget.fetch(self.url, length, self.position, self.size)
            offset = self.position - self.cache_start
            length = min(remaining, len(self.cache) - offset)
            result.extend(self.cache[offset:offset + length])
            remaining -= length
            self.position += length
        return bytes(result)


def riff_metadata(prefix):
    if prefix[:4] != b"RIFF" or prefix[8:12] != b"WAVE":
        raise ValueError("Expected publisher RIFF WAV")
    position, result = 12, []
    while position + 8 <= len(prefix):
        name, size = struct.unpack_from("<4sI", prefix, position)
        if name == b"data":
            return result
        if position + 8 + size > len(prefix):
            raise ValueError("Metadata exceeds preserved header prefix")
        payload = prefix[position + 8:position + 8 + size]
        result.append({"chunk": name.decode("ascii", "replace"), "bytes": size,
                       "text": payload.decode("ascii", "replace") if name in (b"LIST", b"ICMT", b"bext") else None})
        position += 8 + size + size % 2
    raise ValueError("Audio data header not found within bounded prefix")


def acquire(record_id, output, duration=600, range_cache=None, member_name=None):
    if not 60 <= duration <= 600 or not str(record_id).isdigit():
        raise ValueError("Use a numeric record and 60–600 seconds")
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    source_dir, audio_dir, truth_dir = (output / name for name in ("source", "audio", "truth"))
    for directory in (source_dir, audio_dir, truth_dir): directory.mkdir()
    budget = Budget(journal=source_dir / "http-journal.jsonl", cache=range_cache or source_dir / "ranges")
    api = f"https://zenodo.org/api/records/{record_id}"
    metadata_raw = budget.fetch(api, 1_000_000)
    metadata = json.loads(metadata_raw)
    (source_dir / "record.json").write_bytes(metadata_raw)
    if metadata["metadata"].get("license", {}).get("id") != "cc-by-4.0":
        raise ValueError("This lane requires the checked CC BY 4.0 source license")
    match = re.search(r"ESID#\s*(\d+)", metadata["metadata"]["title"])
    if not match:
        raise ValueError("Not the expected Eclipse Soundscapes record")
    site = match.group(1).zfill(3)
    files = {file["key"]: file for file in metadata["files"]}
    for name in ("total_eclipse_data.csv", "file_list.csv", "License.txt", "README.md"):
        file = files[name]
        if file["size"] > 100_000:
            raise ValueError("Unexpected metadata size")
        data = budget.fetch(file["links"]["self"], file["size"])
        if len(data) != file["size"] or "md5:" + hashlib.md5(data).hexdigest() != file["checksum"]:
            raise ValueError("Provider metadata checksum mismatch")
        (truth_dir if name == "total_eclipse_data.csv" else source_dir).joinpath(name).write_bytes(data)
    archive_name = f"ESID_{site}_2024_04_08.zip"
    archive = files[archive_name]
    reader = RangeReader(archive["links"]["self"], archive["size"], budget)
    with zipfile.ZipFile(reader) as zipped:
        candidates = sorted((info for info in zipped.infolist()
                             if re.fullmatch(r"20240408_\d{6}\.WAV", Path(info.filename).name, re.IGNORECASE)
                             and info.file_size > 4096), key=lambda info: Path(info.filename).name)
        if not candidates:
            raise ValueError("No April 8 WAV members")
        selected = [item for item in candidates if Path(item.filename).name == member_name] if member_name else candidates[:1]
        if len(selected) != 1:
            raise ValueError("Requested source member must identify exactly one April 8 waveform")
        member = selected[0]
        write_json(source_dir / "archive-inventory.json", [{"name": i.filename, "bytes": i.file_size,
                   "compressed_bytes": i.compress_size, "crc32": i.CRC} for i in candidates])
        with zipped.open(member) as stream:
            prefix = stream.read(4096)
        (source_dir / "wav-header-prefix.bin").write_bytes(prefix)
        header_metadata = riff_metadata(prefix)
        with zipped.open(member) as stream, wave.open(stream, "rb") as audio:
            params = audio.getparams()
            if params.comptype != "NONE" or params.nchannels != 1 or params.sampwidth != 2 or params.framerate not in (16000, 24000, 32000, 48000, 96000):
                raise ValueError("Unsupported source format; no silent conversion")
            frames = duration * params.framerate
            if audio.getnframes() < frames:
                raise ValueError("Source member shorter than predeclared excerpt")
            with wave.open(str(audio_dir / "field.wav"), "wb") as destination:
                destination.setparams(params._replace(nframes=frames))
                remaining = frames
                while remaining:
                    count = min(remaining, params.framerate * 5)
                    data = audio.readframes(count)
                    if len(data) != count * params.sampwidth * params.nchannels:
                        raise ValueError("Truncated audio member")
                    destination.writeframesraw(data)
                    remaining -= count
    clock = datetime.strptime(Path(member.filename).stem, "%Y%m%d_%H%M%S").replace(tzinfo=timezone.utc).isoformat()
    waveform = audio_dir / "field.wav"
    receipt = {"schema_version": 1, "kind": "source_informed_field_reference",
               "record_url": f"https://zenodo.org/records/{record_id}", "site_id": site,
               "attribution": metadata["metadata"]["creators"], "license": "CC BY 4.0",
               "source_archive": {"name": archive_name, "bytes": archive["size"], "checksum": archive["checksum"],
                                  "whole_archive_checksum_verified": False},
               "source_member": {"name": member.filename, "bytes": member.file_size, "crc32": member.CRC,
                                 "whole_member_crc_verified": False, "header_metadata": header_metadata},
               "selection": ("Explicit predeclared member; first declared seconds" if member_name else "Earliest nonempty April 8 WAV; first declared seconds, selected before local analysis"),
               "sample_zero_filename_utc_claim": clock, "excerpt_start_frame": 0,
               "audio": {"path": "audio/field.wav", "sha256": sha256(waveform.read_bytes()),
                         "bytes": waveform.stat().st_size, "sample_rate_hz": params.framerate,
                         "frames": frames, "duration_s": duration, "channels": params.nchannels,
                         "sample_width_bytes": params.sampwidth, "transformation": "Original leading PCM samples with a new WAV container; no resampling, normalization, or temporal edits"},
               "http_payload_bytes": budget.received, "http_requests": budget.requests,
               "receiver_location_independent_of_acoustic_fit": True,
               "receiver_position_error_bound_m": None, "sample_zero_error_bound_s": None,
               "aircraft_identity_confirmed": False, "matched_tracks_acquired": False,
               "geolocation_accuracy_validated": False,
               "limitations": ["Publisher deployment coordinate has unknown microphone registration error",
                               "Filename/header clock is publisher evidence, not a measured UTC error bound",
                               "A partial ZIP member cannot verify its full-file CRC or publisher SHA-512",
                               "Upstream moving-tone screening informed source selection; this is not a blind random corpus"]}
    write_json(output / "acquisition.json", receipt)
    return {k: receipt[k] for k in ("record_url", "site_id", "sample_zero_filename_utc_claim", "audio", "http_payload_bytes", "geolocation_accuracy_validated")}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", default="22049202")
    parser.add_argument("--output", required=True)
    parser.add_argument("--duration", type=int, default=600)
    parser.add_argument("--range-cache", type=Path, help="Reuse previously verified public archive ranges")
    parser.add_argument("--member", help="Exact predeclared waveform basename for a separate time holdout")
    args = parser.parse_args()
    print(json.dumps(acquire(args.record, args.output, args.duration, args.range_cache, args.member), indent=2))
