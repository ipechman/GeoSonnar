"""Inspect publisher clock evidence without inventing recording-relative labels."""
from pathlib import Path
import argparse
import datetime
import hashlib
import json
import struct


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def audit(corpus):
    corpus = Path(corpus).resolve()
    audio_manifest = json.loads((corpus / "recording-manifest.json").read_text())
    annotations = json.loads((corpus / "annotation-manifest.json").read_text())
    audio = {row["case_id"]: row for row in audio_manifest["recordings"]}
    epoch = lambda text: datetime.datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp()
    rows = []
    for item in annotations["recordings"]:
        case = audio[item["case_id"]]
        chunks = []
        with (corpus / case["audio_path"]).open("rb") as stream:
            if stream.read(4) != b"RIFF":
                raise ValueError("Expected publisher RIFF WAV")
            stream.read(4)
            if stream.read(4) != b"WAVE":
                raise ValueError("Invalid WAVE identifier")
            while header := stream.read(8):
                if len(header) != 8:
                    raise ValueError("Truncated RIFF header")
                name, count = struct.unpack("<4sI", header)
                chunks.append({"id": name.decode("ascii", errors="replace"), "bytes": count})
                stream.seek(count + count % 2, 1)
        first, last = map(epoch, item["annotation_utc_interval"])
        received = epoch(item["publisher_reference_row"]["received_start_utc"])
        rows.append({"case_id": item["case_id"], "source_case": item["source_case"],
                     "received_start_minus_first_annotation_s": received - first,
                     "annotation_span_s": last - first, "audio_duration_s": case["duration_s"],
                     "audio_duration_minus_annotation_span_s": case["duration_s"] - (last - first),
                     "riff_chunks": chunks,
                     "recording_relative_event_interval_s": None})
    extraction_tool = corpus / "source-metadata/frequency_extraction_tool.html"
    result = {"schema_version": 1, "recordings": len(rows),
              "received_start_equals_first_annotation_count": sum(abs(row["received_start_minus_first_annotation_s"]) < 1e-6 for row in rows),
              "wav_files_containing_only_fmt_and_data": sum({c["id"] for c in row["riff_chunks"]} == {"fmt ", "data"} for row in rows),
              "audio_sample_zero_utc_established": False, "event_time_truth_established": False,
              "conclusion": "Published UTC annotations do not establish each WAV sample-zero clock. Do not subtract received_start_utc and label the result recording-relative event time.",
              "source_tool_evidence": {"path": str(extraction_tool), "sha256": digest(extraction_tool),
                                       "member_sha256_verified_against_checkpoint": True,
                                       "functions": ["utcForAudioTime", "setMetaFromFilename", "applyManualTimeMeta"],
                                       "interpretation": "UTC export needs an original filename time/manual start plus loaded-segment offset. Those per-recording settings are not retained in the two-column frequency CSVs or generic audio.wav filenames."},
              "input_hashes": {name: digest(corpus / name) for name in ["recording-manifest.json", "annotation-manifest.json", "source-metadata/final_results_40_cases.csv"]},
              "script_sha256": digest(Path(__file__)), "rows": rows}
    with (corpus / "annotation-timing-audit.json").open("x", encoding="utf-8") as stream:
        json.dump(result, stream, indent=2, allow_nan=False)
        stream.write("\n")
    return {key: result[key] for key in ["recordings", "received_start_equals_first_annotation_count", "wav_files_containing_only_fmt_and_data", "event_time_truth_established"]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    args = parser.parse_args()
    print(json.dumps(audit(args.corpus), indent=2))
