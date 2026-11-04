"""Fetch public map replay slices and regional per-aircraft historical traces.

Independent implementation of the documented/public readsb wire layout. Replay
is a candidate index only: absence is not proof that an aircraft was absent.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import struct
import zlib

from field_validation_acquire import Budget, sha256, write_json

MAP_SCRIPT = "https://adsb.lol/script_22b772ac55265ba79eeeca99ab34165c.js"
MAGIC = 0x0E7F7C9D


def decoded(data, limit=128_000_000):
    if not data.startswith(b"\x1f\x8b"):
        if len(data) > limit: raise ValueError("Decoded size exceeded")
        return data
    inflater = zlib.decompressobj(31)
    result = inflater.decompress(data, limit + 1)
    if len(result) > limit or not inflater.eof or inflater.unused_data or inflater.unconsumed_tail:
        raise ValueError("Invalid or oversized gzip response")
    return result


def replay_records(data, start, end, bounds):
    if len(data) % 16:
        raise ValueError("Replay byte length is not a multiple of 16")
    timestamp, interval, slices, rows = None, None, [], []
    south, west, north, east = bounds
    first = next((offset for offset in range(0, len(data), 16) if struct.unpack_from("<I", data, offset)[0] == MAGIC), None)
    if first is None: raise ValueError("Replay timestamp markers missing")
    # Older files prepend an index of slice row offsets. The public browser
    # scans past it; validate its contents instead of treating it as positions.
    for offset, a, b, c in struct.iter_unpack("<IIII", data[:first]):
        if a or b or c or offset * 16 + 16 > len(data) or struct.unpack_from("<I", data, offset * 16)[0] != MAGIC:
            raise ValueError("Invalid legacy replay slice index")
    for word0, word1, word2, word3 in struct.iter_unpack("<IIII", data[first:]):
        if word0 == MAGIC:
            timestamp = ((word1 << 32) | word2) / 1000
            interval = (word3 & 65535) / 1000
            if not 0 < interval <= 60:
                raise ValueError("Invalid replay sample interval")
            slices.append(timestamp)
            continue
        if timestamp is None:
            raise ValueError("Replay row before timestamp marker")
        lat = word1 if word1 < 2**31 else word1 - 2**32
        lon = word2 if word2 < 2**31 else word2 - 2**32
        if lat >= 2**30:
            continue  # callsign/squawk metadata, not a position
        lat, lon = lat / 1e6, lon / 1e6
        if not -90 <= lat <= 90 or not -180 <= lon <= 180:
            raise ValueError("Replay coordinate outside geographic range")
        if not (start <= timestamp <= end and south <= lat <= north and west <= lon <= east):
            continue
        altitude = word3 & 65535
        if altitude >= 32768: altitude -= 65536
        speed = word3 >> 16
        if speed >= 32768: speed -= 65536
        rows.append({"icao24": ("~" if word0 & 0x1000000 else "") + f"{word0 & 0xffffff:06x}",
                     "timestamp": timestamp, "latitude": lat, "longitude": lon,
                     "barometric_altitude_ft": None if altitude in (-123, -124) else altitude * 25,
                     "on_ground": altitude == -123,
                     "groundspeed_kt": None if speed == -1 else speed / 10,
                     "position_source_code": (word0 >> 27) & 31, "replay_interval_s": interval})
    if not slices or any(b <= a for a, b in zip(slices, slices[1:])):
        raise ValueError("Missing or unordered replay slices")
    return rows, {"slices": len(slices), "start": slices[0], "end": slices[-1], "interval_s": interval}


def acquire(output, start_text, end_text, bounds, max_traces=100):
    start = datetime.fromisoformat(start_text.replace("Z", "+00:00"))
    end = datetime.fromisoformat(end_text.replace("Z", "+00:00"))
    if start.tzinfo is None or end.tzinfo is None or not 0 < (end - start).total_seconds() <= 3600:
        raise ValueError("Use an explicit UTC interval of at most one hour")
    start, end = start.astimezone(timezone.utc), end.astimezone(timezone.utc)
    if start.date() != end.date() or not 1 <= max_traces <= 200:
        raise ValueError("Use one UTC calendar day and at most 200 traces")
    south, west, north, east = bounds
    if not (-90 <= south < north <= 90 and -180 <= west < east <= 180):
        raise ValueError("Invalid geographic bounds")
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    source, traces = output / "source", output / "traces"
    source.mkdir(); traces.mkdir()
    budget = Budget(limit=95_000_000, journal=source / "http-journal.jsonl")
    script = budget.fetch(MAP_SCRIPT, 2_000_000)
    (source / "provider-map.js").write_bytes(script)
    all_rows, chunks = [], []
    first = start.hour * 2 + start.minute // 30
    last = end.hour * 2 + end.minute // 30
    day = start.strftime("%Y/%m/%d")
    for index in range(first, last + 1):
        url = f"https://adsb.lol/globe_history/{day}/heatmap/{index:02}.bin.ttf"
        data = budget.fetch(url, 20_000_000)
        name = f"replay-{index:02}.http.bin"
        (source / name).write_bytes(data)
        raw = decoded(data)
        rows, detail = replay_records(raw, start.timestamp(), end.timestamp(), bounds)
        expected = start.replace(hour=0, minute=0, second=0, microsecond=0).timestamp() + index * 1800
        if abs(detail["start"] - expected) > detail["interval_s"] or detail["end"] > expected + 1800:
            raise ValueError("Replay timestamps do not match requested day/half-hour")
        all_rows.extend(rows)
        chunks.append({"url": url, "path": "source/" + name, "sha256": sha256(data),
                       "transport_bytes": len(data), "decoded_bytes": len(raw), "timing": detail})
    write_json(output / "regional-replay-index.json", all_rows)
    identities = sorted({row["icao24"] for row in all_rows if not row["icao24"].startswith("~")})
    selected = identities[:max_traces]
    details = []
    for identity in selected:
        url = f"https://adsb.lol/globe_history/{day}/traces/{identity[-2:]}/trace_full_{identity}.json"
        data = budget.fetch(url, 2_000_000)
        payload = decoded(data, limit=8_000_000)
        trace = json.loads(payload)
        if trace.get("icao", trace.get("hex")) != identity or not isinstance(trace.get("trace"), list):
            raise ValueError("Trace identity/schema differs from regional index")
        name = f"trace_full_{identity}.json"
        (traces / name).write_bytes(payload)
        details.append({"url": url, "path": "traces/" + name, "icao24": identity,
                        "sha256": sha256(payload), "transport_sha256": sha256(data),
                        "transport_bytes": len(data), "trace_points": len(trace["trace"])})
    receipt = {"schema_version": 1, "provider": "ADSB.lol public map history", "license": "ODbL-1.0",
               "source_docs": "https://www.adsb.lol/docs/open-data/historical", "script_url": MAP_SCRIPT,
               "script_sha256": sha256(script), "utc_start": start.isoformat(), "utc_end": end.isoformat(),
               "aircraft_coverage_bounds": bounds, "receiver_truth_used_for_filtering": False,
               "selection_basis": "Predeclared broad geographic aircraft coverage, not distance from the publisher microphone coordinate",
               "replay_chunks": chunks, "regional_replay_points": len(all_rows),
               "icao_identities_in_replay": len(identities), "downloaded_traces": len(details),
               "trace_cap_reached": len(identities) > max_traces, "traces": details,
               "http_payload_bytes": budget.received, "coverage_complete": False,
               "limitations": ["Replay is a sampled candidate index, not an exhaustive sky or absence test",
                               "Untracked aircraft, missing positions, reception gaps, gaps between replay samples and non-ICAO targets may be omitted",
                               "Index altitude is barometric and quantized; full traces must retain altitude/source semantics",
                               "A matching trace in the region does not establish which acoustic proposal it produced"]}
    write_json(output / "acquisition.json", receipt)
    return {key: receipt[key] for key in ("regional_replay_points", "icao_identities_in_replay", "downloaded_traces", "trace_cap_reached", "http_payload_bytes", "coverage_complete")}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True)
    parser.add_argument("--start", default="2024-04-08T04:20:00Z")
    parser.add_argument("--end", default="2024-04-08T04:40:00Z")
    parser.add_argument("--bounds", type=float, nargs=4, default=[42.5, -73.5, 45.5, -70.0])
    args = parser.parse_args()
    print(json.dumps(acquire(args.output, args.start, args.end, args.bounds), indent=2))
