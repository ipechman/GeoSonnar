"""Complete a capped cached replay index without re-downloading its payloads."""
import argparse
from datetime import datetime
import json
from pathlib import Path

from field_validation_acquire import Budget, sha256, write_json
from field_validation_tracks import decoded


def complete(source, output, limit):
    source, output = Path(source).resolve(), Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output / "traces").mkdir()
    receipt = json.loads((source / "acquisition.json").read_text(encoding="utf-8"))
    rows = json.loads((source / "regional-replay-index.json").read_text(encoding="utf-8"))
    identities = sorted({row["icao24"] for row in rows if not row["icao24"].startswith("~")})
    import os
    details = []
    for trace in receipt["traces"]:
        item = dict(trace)
        path = source / item["path"]
        if sha256(path.read_bytes()) != item["sha256"]:
            raise ValueError("Cached original trace checksum changed")
        item["path"] = os.path.relpath(path, output)
        details.append(item)
    budget = Budget(limit=limit, journal=output / "http-journal.jsonl")
    seen = {item["icao24"] for item in details}
    day = datetime.fromisoformat(receipt["utc_start"]).strftime("%Y/%m/%d")
    for identity in identities:
        if identity in seen:
            continue
        url = f"https://adsb.lol/globe_history/{day}/traces/{identity[-2:]}/trace_full_{identity}.json"
        data = budget.fetch(url, 2_000_000)
        payload = decoded(data, limit=8_000_000)
        trace = json.loads(payload)
        if trace.get("icao", trace.get("hex")) != identity or not isinstance(trace.get("trace"), list):
            raise ValueError("Trace identity/schema differs from cached index")
        name = f"trace_full_{identity}.json"
        (output / "traces" / name).write_bytes(payload)
        details.append({"url": url, "path": "traces/" + name, "icao24": identity,
                        "sha256": sha256(payload), "transport_sha256": sha256(data),
                        "transport_bytes": len(data), "trace_points": len(trace["trace"])})
    original_bytes = receipt["http_payload_bytes"]
    receipt.update(previous_acquisition_path=str(source / "acquisition.json"),
                   previous_acquisition_sha256=sha256((source / "acquisition.json").read_bytes()),
                   regional_index_path=str(source / "regional-replay-index.json"),
                   regional_index_sha256=sha256((source / "regional-replay-index.json").read_bytes()),
                   traces=sorted(details, key=lambda item: item["icao24"]), downloaded_traces=len(details),
                   trace_cap_reached=False, supplemental_http_payload_bytes=budget.received,
                   http_payload_bytes=original_bytes + budget.received)
    # All original raw replay/source-format artifacts remain in the prior receipt directory.
    for chunk in receipt["replay_chunks"]:
        chunk["path"] = os.path.relpath(source / chunk["path"], output)
    write_json(output / "acquisition.json", receipt)
    print(json.dumps({"downloaded_traces": len(details), "new_payload_bytes": budget.received,
                      "combined_payload_bytes": receipt["http_payload_bytes"], "coverage_complete": False}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--max-bytes", type=int, default=30_000_000)
    args = parser.parse_args()
    if not 1 <= args.max_bytes <= 95_000_000:
        raise ValueError("Supplemental transfer cap must be1..95MB")
    complete(args.source, args.output, args.max_bytes)
