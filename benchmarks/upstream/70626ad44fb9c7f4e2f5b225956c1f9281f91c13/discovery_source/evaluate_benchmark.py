"""Evaluate saved predictions after search has completed; never called by search."""
import argparse
import json
from pathlib import Path

from dopplergeist.evaluation import evaluate, sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("predictions", type=Path)
    parser.add_argument("truth", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--threshold-m", type=float, default=1000)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Output already exists; use a new experiment path")
    report = evaluate(*(json.loads(p.read_text()) for p in (args.manifest, args.predictions, args.truth)), threshold_m=args.threshold_m)
    report["input_sha256"] = {p.name: sha256(p) for p in (args.manifest, args.predictions, args.truth)}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(json.dumps(report["summary"], indent=2))


if __name__ == "__main__":
    main()
