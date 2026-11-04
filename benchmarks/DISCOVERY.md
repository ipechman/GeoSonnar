# Automatic acoustic discovery

GeoSonnar now scans complete recordings, follows measured moving-tone ridges,
joins unambiguous interrupted fragments and groups harmonics into possible
flyovers. Only audio and an explicit channel are supplied to the detector.
No date, flight identity, location, annotated band or event time is an input.

```powershell
.\dist\geosonnar.exe discover --input "your-recording.mp4" --out output/discovery/new-case --channel 0
```

Each run requires a new directory and writes:

- `discovery.json`: measured ridges, groups, settings, source SHA-256, channel,
  uncertainty diagnostics, chunk counts and fragment decisions.
- One CSV per measured ridge, including time, frequency and contrast weight.
- `review.html`: offline overview and group charts, with Keep/Reject/Uncertain
  review choices and a separate `review-selection.json` download.

Every group starts unreviewed. “Keep” means suitable for further matching, not a
confirmed aircraft. Selection does not alter the frozen detections. Schema-2
review exports now bind the exact discovery file SHA-256, source and channel,
and can be consumed by `prepare-reviewed-search`. The pair matcher requires an
explicit choice of one measured ridge from each of two kept groups, together
with emitted-frequency grids and search/timing assumptions. See
[reviewed search](REVIEWED_SEARCH.md) for the connection and comparison results.
Discovery neither recovers a UTC clock nor verifies a location. The earlier
37 m regional result still uses manually reviewed traces.

For an existing discovery, regenerate a schema-2 review page without rescanning:

```powershell
.\dist\geosonnar.exe review-discovery --input output/benchmarks/discovery/original-hour-001/discovery.json --out output/review-new.html
```

Use a new HTML path. Choices start unreviewed; older schema-1 selections are
rejected by the bridge because they do not bind the detection file itself.
CSV downloads use the embedded measurements and work after moving the review page.

## Fixed method and limits

The Rust algorithm adapts the MIT-licensed DOPPLERGEIST discovery implementation
at `70626ad44fb9c7f4e2f5b225956c1f9281f91c13`: frequency-wise median whitening,
per-frame median/MAD contrast, local spectral peaks, sub-bin measurement,
one-to-one minimum-cost path assignment, distributed downward-motion checks,
overlap deduplication, reciprocal-unique fragment stitching and rational harmonic
ratios including missing fundamentals.

Defaults are 40–1,600 Hz, 120 s chunks with 60 s overlap, 0.5 s hops, at least
12 s duration and 65% measured support. The native radix-2 FFT uses a **2.048 s**
periodic Hann window at 4 kHz, compared with upstream's 2.0 s window. There is no
zero-padding, and uncertainties use the actual window. Group comparisons do not
interpolate across unsupported gaps. Competing continuations remain separate.

FFmpeg performs channel selection and anti-aliased decoding; all discovery
calculations run in Rust. Channels are never averaged. The program hashes the
source before and after decoding, bounds working/output memory, rejects invalid
settings and refuses existing output folders. Strict decoder error handling
prevents a damaged file's valid prefix from masquerading as a completed scan.
Failures retain diagnostics without completed-looking JSON/HTML artifacts.

These are **moving-tone proposals**. Stationary tones, upward sweeps and the
declared noise controls are rejected, but an electronic downward sweep can
resemble an aircraft. Weak, masked or nontonal aircraft can be missed. Proposal
counts and sensitivity diagnostics are not calibrated aircraft probabilities.

## Validation performed

The frozen `full-003` discovery run used the static-CRT executable SHA-256
`53a13a1e7001dbdf4c1fae9a03a76236131dfc373c8e47048d7fd3b807b7f44a`.

These are the preserved discovery-release receipts. The newer audio-prior build
has 149 passing native tests in `validation-005`; its discovery implementation
is unchanged. Current packaging identity is in `validation-005/package-receipt.json`.

That release's full core regression profile includes **132 Rust tests**, including 16
group/fragment tests, eight real-FFmpeg discovery input/output tests and 12
reviewed-search integration tests, plus
the existing physics, data, decision and audio checks. Formatting and Clippy
pass. Core receipt: `output/benchmarks/validation-004/summary.json`.

The discovery benchmark contract was frozen in `discovery_contract.json` before
production evaluation. Its control generator was independently checked to
produce byte-identical audio for all 14 pinned upstream controls. In
`output/benchmarks/discovery/full-003/report.json`:

| Lane | Result | Meaning |
| --- | --- | --- |
| 14 primary controls | 8/8 expected events; 0 misses, 0 extra groups | All fixed noise/stationary/upward/overlap/MP3/truncated controls pass |
| Primary ridge measurement | Median error 0.0128–0.0248 Hz; interval IoU at least 0.938 | Compared with independent known source laws; gates were 5 Hz and IoU 0.3 |
| Additional semantic controls | Missing fundamental, interruption and stationary switching pass | Gaps contain no invented samples; a weak partial event remains diagnostic |
| 40 real clips | 40/40 complete; 34 with proposals, 6 without; 136 groups, 169 ridges | Technical completion and descriptive proposal counts, not precision/recall |

The 40 real recordings are from Praveen Pathak's CC BY 4.0 corpus at
[Zenodo record 23424278](https://zenodo.org/records/23424278). All source and
selected archive-member hashes were checked. Original WAVs remain unchanged;
benchmark inputs are metadata-stripped 192 kbps MP3s with opaque filenames.
The native detector receives no annotations. Attribution, original audio-only
manifest and annotation timing audit are under `data/research/zenodo-23424278/`.

The publisher's `received_start_utc` is the first annotated frequency time,
not an established audio sample-zero timestamp. Therefore there is **no honest
event-level precision or recall score** available from those annotations alone.
Upstream proposals are never substituted for ground truth, and upstream's clip
count is not a target that this implementation was tuned to match.

The final discovery run took 136.5 s including all 18 control cases, 40 real clips,
encoding, hashing and reports. Source, executable, compiled review template and
contract remained unchanged. Runtime is descriptive for this machine; other
validation work ran concurrently. The v2 review renderer also completed on every clip.

## Your original recording

The complete one-hour recording was scanned without event hints in approximately
six seconds, yielding **18 groups and 20 ridges**. Review it at
`output/benchmarks/discovery/original-hour-001/review.html`.

After prediction, frozen manual ridges were compared at the same recording time,
with no time shift, harmonic rescaling or interpolation over gaps longer than 2 s.
Compatibility required frequency error at most 3 Hz:

| Existing reference | Compatible samples | Median supported frequency error |
| --- | --- | --- |
| Main event 1 | 102/189 (54.0%) | 0.119 Hz |
| Main event 2 | 65/105 (61.9%) | 0.120 Hz |
| Faint event | 0/233 | No matching proposal |

Both main signals are found partially; the faint signal **still needs manual
marking**. These are acoustic diagnostics, not a new geolocation result. The
comparison rule was specified before prediction; its implementation was frozen
after the prediction file existed but before that file was read. That chronology
and hashes are explicit in `original-hour-evaluation-001/source-evaluation.json`.
`source-ridge-comparison.png` shows the preserved measurements and missing support.

## Reproduce

From the project root, after the core build:

```powershell
python analysis/benchmark_discovery.py --exe dist/geosonnar.exe --output output/benchmarks/discovery/new-run --corpus-manifest data/research/zenodo-23424278/recording-manifest.json
python -m unittest discover -s benchmarks -p test_discovery_harness.py -v
node analysis/validate_discovery_review.cjs --input output/benchmarks/discovery/original-hour-001 --out output/benchmarks/discovery/review-new.json
```

The harness needs Python, numpy, scipy, soundfile and FFmpeg, plus the already
restored corpus. It saves labels separately, produces all control predictions
before parsing labels, enforces case counts and records failures without skipping.
The separation is procedural on one filesystem, not a sealed holdout.

The review page was tested at the code level using a minimal DOM: chart coordinates
and gap segments, default unreviewed state, review export, unchanged source/channel
identity, and Unicode/script-text escaping. Intentional bad acceptance, gap
bridging and network mutations are rejected. **Browser visual QA was unavailable**
because browser policy blocked local-file navigation; these tests do not claim
layout/accessibility verification. Receipts are in `review-code-checks-003/`.

The initial `controls-001` is preserved. `full-001` was interrupted and explicitly
excluded after the partial-decode bug was identified; `full-002` reran every case
with the corrected decoder. `full-003` repeats every case with the reviewed-search
release and v2 review export, with unchanged proposal counts. No detection
threshold was changed to force the real recordings to pass.
