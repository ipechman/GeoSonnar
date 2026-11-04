# Evaluation corpus and separation

Current reports use the updated 1,000 m acceptance target and show the 100, 500
and 1,000 m tiers separately. Historical runs retain their original 100 m target.
Uncalibrated sensitivity and diagnostic candidates are distinct from measured
error of an accepted location.

`inventory.json` pins input hashes from the published checkpoint manifest, source,
license status, clock evidence and coordinate precision. The 40-passage record is
CC BY 4.0 (Praveen Pathak). The public reference recording has no independently
established redistribution licence in the collected evidence; this change adds
neither that audio nor the source video. Synthetic fixtures use the repository's
independent emission-domain renderer. Historical outputs remain untouched.

`frozen-manifest.json` contains case identity, split and scope only.
`truth/frozen-labels.json` contains post-run evaluation references. None of these
historical cases is held out: all have already been inspected during development.
The exact coordinates, known aircraft, manual event windows/bands and regional
search boxes in historical artifacts must never become normal search inputs.

## Reproduce the frozen audit

```bash
PYTHONPATH=src .venv/bin/python scripts/evaluate_frozen.py --output results/frozen-audit-new
```

This evaluates every one of the 48 readable frozen cases. Two independent-renderer
synthetic searches return 21.8855 m and 40.8744 m errors. These searches used manual
event settings. The real regional two-event estimate is 43.8585 m from a rounded
mapped reference; the faint all-types result is 357.4687 m away and misses the
100 m target. Neither is eligible for independent real localization accuracy.
Three exploratory directories have no completed report and are counted as
execution failures. The 40 feature cases include all seven misses (33 detections),
using supplied harmonic-band annotations. Their case-level detection recall is
not automatic event-window recall. Peak memory was not recorded and remains null.
Feature-only results are represented as `inconclusive` for localization. Use the
`by_scope` aggregates to distinguish these from localization abstention.

## Run new audio controls with truth unavailable

```bash
PYTHONPATH=src .venv/bin/python scripts/make_benchmark_controls.py --output results/controls-new
# Place results/controls-new/truth outside the search worker's filesystem/mount.
# The runner reads only inputs/manifest.json, requests, and checksummed audio.
PYTHONPATH=src .venv/bin/python scripts/run_benchmark.py results/controls-new/inputs/manifest.json \
  --output results/controls-run-new -- python run_one.py '{request}' '{output}'
# After the worker exits, join the separately held labels:
PYTHONPATH=src .venv/bin/python scripts/evaluate_benchmark.py \
  results/controls-new/inputs/manifest.json results/controls-run-new/predictions.json \
  results/controls-new/truth/labels.json --output results/controls-evaluation-new.json
```

`run_one.py` is an adapter to the actual producer entry point and must write a
shared `JobResult` to `OUTPUT/result.json`. The runner uses an argument vector,
validates the normal request contract (no manual hints), verifies audio hashes,
records every process exit and per-process Linux `wait4` runtime/peak memory, and
refuses existing output directories. It does not pass manifest labels to the
executable. It is not an adversarial filesystem sandbox: the operator must remove
or unmount truth for an isolation claim. A protocol test executes with no truth
directory at all; that test does not claim audio/localization accuracy.

The generator makes seven development and seven public deterministic held-out
controls with distinct seeds: noise, stationary tone, ascending non-aircraft
electronic sweep, moving-source harmonic event, overlapping nonharmonic events,
truncated short clip and MP3. Audio filenames are opaque. Truth contains event
intervals and negative labels; requests contain only audio, an explicit synthetic
clock and CONUS coverage. These discovery controls have no historical track corpus,
so precise locations from them are unsupported. The renderer transports emission
phase into reception time without calling the inverse search physics. MP3 bytes
are hashed per generation because encoder versions can change bytes. Freeze
detector settings on development controls before examining held-out predictions.
These small public synthetic splits are regression controls, not independent real
validation or an estimate of performance over arbitrary sound distributions.

The [isolated live receipt](reports/discovery-isolation-20261102/receipt.json) and
[all-case report](reports/discovery-isolation-20261102/report.json) exercise the
actual W1 intake/discovery source frozen at `62066ac` (discovery `4a49646`). Each
subprocess had both truth directories masked with bubblewrap, a read-only root,
private `/tmp`, disabled network and only its own output directory writable.
A separate probe verified label files were absent inside that namespace. All
14 cases completed: 8/8 labelled events, zero misses and zero false alarms, with
4/4 events in each split. All cases remain discovery-only inconclusive without
coordinates. Total per-case elapsed time was 120.1 s; maximum per-case peak RSS
was 240,021,504 bytes, including plotting/import/codec overhead. The receipt pins
source/input/prediction hashes and preserves the two failed isolation-harness
attempts; their failures are not detector outcomes. These results do not measure
historical flight association or real-world localization accuracy.

Detection evaluation merges harmonic groups and uses maximum-cardinality temporal
IoU matching at 0.3 (predeclared). It reports TP, misses and false alarms. Localization
uses WGS84 geodesic error with a 100 m threshold, marks wrong flight associations as
false localizations, and reports all statuses separately. The false-localization
denominator is explicit: eligible location cases plus known negative cases. Candidate
coordinates retained under `inconclusive` are diagnostic alternatives, not fixes.

## Unresolved independent real-data acceptance

The requested three independently timed recordings across two known public or
consented locations have **not** been established. No existing real case qualifies
for that gate. The 40 Livermore passages have UTC frequency annotations but no
established audio-sample-zero clock or precise recorder coordinate in the collected
evidence. Their labels cannot be promoted from flight proximity or fitted clocks.

Source investigation on 2026-11-02 found
[AeroSonicDB v1.1](https://zenodo.org/records/8368882), which supplies recordings
from three Adelaide locations and filenames with dates/times. Its published
coordinates deliberately identify a nearby road/public space for privacy, rather
than the recorder. Precision adequate for a 100 m recorder-error claim and a UTC
sample-zero clock are unestablished. It is also outside the current CONUS contract.
The [3K version](https://zenodo.org/records/12775560) has restricted files and
trimmed events. These are potential detection corpora, not eligible location
truth. No access request or author message was sent. Additional real recordings
must establish coordinate uncertainty, consent/public site, source licence,
sample-zero UTC/time window and a flight archive independent of the known target.
Until those exist, do not close the real-corpus or nationwide-accuracy gate.

## Automatic discovery on the 40 real Livermore recordings

On 2026-11-03, all 40 published smartphone recordings were encoded to 192 kbps
MP3 and passed through the normal automatic CLI at source `e8cbb08`. Inputs had
opaque names, no event windows/bands, no flight hints and no supplied UTC. Workers
had no network or access to the annotations, KML files or original case mapping.
Every case and outcome is in [the compact table](real-discovery-20261103.csv).

**36 clips produced proposals; four produced none; 136 groups were proposed.**
All 40 jobs returned `needs_input / recording_time_required`; none returned a
location. Processing took 241.88 s, excluding MP3 staging. These are real positive
recordings, but a proposal in a positive clip is not a confirmed target detection.
There is no event-level false-alarm rate or location-accuracy result here.

The missed clips were N958JX_R50, N1387U_R44, N451EE_R51 and N9658K_R24. Their
annotated bands, inspected only after outputs were terminal, all lie inside the
frozen 40–1600 Hz discovery range. The historical manually banded analysis detected
N9658K_R24; the other three were historical misses too. N563FX_R21 produced 25
groups and N60RS_R11 produced 19, requiring inspection for fragments, harmonics
and unrelated proposals. Extra groups remain unconfirmed rather than being
automatically counted as false alarms. These cases are handed to W1 in issue #5.

To repeat with the restored public source and a new output directory:

```sh
PYTHONPATH=src .venv/bin/python scripts/benchmark_real_discovery.py \
  data/research/zenodo-23424278 --output results/NEW-UNUSED-DIRECTORY
```

The script requires the automatic CLI, ffmpeg and bwrap. Its runner is made
portable from the local script used for this experiment; only `--help` was run
on the committed helper, without repeating the unchanged 40-clip experiment.
The missing microphone/UTC evidence described above still prevents these cases
from qualifying as real geolocation acceptance. Further validation must use real
recordings and geolocations under the user's real-data-only directive.

## Real NASA supplied-trajectory session benchmark

`scripts/benchmark_nasa_session.py` uses original NASA pressure samples and measured
GPS trajectories. The [case manifest](nasa-session-cases-20261103.json) pins four
original MAT files from the
[public NASA archive](https://data.nasa.gov/docs/datasets/rfk401li/small_uav_acoustics.zip).
Retrieve only those original members, without downloading the complete archive:

```sh
python scripts/download_nasa_cases.py --output data/nasa-originals
```

The download directory must be new. If the original files already exist, add
`--verify-only` to check them without downloading or modifying anything.
The downloader verifies ZIP lengths and CRCs plus the manifest's byte sizes and
SHA256 values. Preparation also checks the original identities before reading them.

The runner preserves channel 2 samples and original acoustic/GPS timestamps.
It uses synchronized seconds of day; calendar dates remain unresolved. It does
not invent an absolute UTC epoch. The local search domain comes from the full
GPS footprint with fixed padding. Microphone truth is read only during preparation
and later evaluation, not by search. One supplied trajectory does not imply one
encounter or one independent acoustic event.

With the package installed, run each stage into a new output directory:

```sh
PYTHONPATH=src .venv/bin/python scripts/benchmark_nasa_session.py prepare \
  --source-directory data/nasa-originals \
  --case-manifest benchmarks/nasa-session-cases-20261103.json \
  --output results/nasa-new/prepared
PYTHONPATH=src .venv/bin/python scripts/benchmark_nasa_session.py search \
  --inputs results/nasa-new/prepared/search-inputs.json \
  --domain results/nasa-new/prepared/search-domain.json \
  --output results/nasa-new/search
PYTHONPATH=src .venv/bin/python scripts/benchmark_nasa_session.py evaluate \
  --search-directory results/nasa-new/search \
  --truth results/nasa-new/prepared/evaluation-truth.json \
  --output results/nasa-new/evaluation.json
```

Defaults use automatic discovery with a 2 s spectral window and 0.5 s hop,
50 m grid cells, zero clock offset, and separate constant-tone and 2% drift fits.
The [result table](nasa-session-results-20261103.csv) preserves every development
case and both discovery settings. These are supplied-track experiments, not an
automatic aircraft-association or nationwide validation corpus.

| Recording | Proposals at 2 s / 0.5 s | Proposals at 0.5 s / 0.125 s | Primary result |
|---|---:|---:|---|
| Cub 107 | 0 | 0 | No fit or location |
| Edge 038 | 0 | 0 | No fit or location |
| Edge 065 | 1 | 0 | Conditional best-mode error 474.0 m; source identity unconfirmed |
| Edge 066 | 0 | 0 | No fit or location |

Edge 065's proposal covers 1.5–19.0 s near 94–102 Hz. The strong harmonic flyover
and measured-track closest approach occur near 30.6 s. The publisher notes a
background propeller aircraft at the start, so the proposal may belong to it.
Both source-tone models rank the same point, but this does not validate drone
localization. Each grid has 3,632 supported points and 568 unknown points.
A competing mode is 1,899 m from truth, and similar losses extend over kilometres.
The CSV's 100/500/1,000 m fields describe only the conditional best-mode error;
they do not indicate accepted or correctly associated locations. The high-speed
pair's primary discovery and grid search took 6.56 s in the recorded run.

The single shorter-window development comparison returned zero proposals in all
four recordings, with no fitting or truth access. It did not improve discovery.
To reproduce that setting, use a new search directory and add these search options:

```sh
--discovery-window-s 0.5 --discovery-hop-s 0.125
```

Every other default remains unchanged. No detector threshold was relaxed, no
synthetic data were generated, and all original negative results are retained.
The reusable CLI was also run against all four originals with the isolated
installed package. Preparation, default search, evaluation and the shorter-window
search all completed and reproduced the proposal counts above.

## Real WSU supplied-trajectory session benchmark

`scripts/benchmark_wsu_session.py` processes the complete 246.8-second recording
from WSU's October 11, 2024 flight 5, using device 1/channel 1 selected before
acoustic inspection. The [dataset](https://doi.org/10.26183/07nm-4p35) and
[publisher README](https://rds.westernsydney.edu.au/Centres/ICNS/2026/Jahir_Rodriguez/README.md)
credit Belman Jahir Rodríguez, Sergio F. Chevtchenko, Marcelo Herrera Martínez,
Yeshwanth Bethi and Saeed Afshar. External recordings and metadata retain their
CC BY-SA 4.0 license. This repository's adapter is independently written.

Retrieve these four original public inputs into a new directory. The adapter
checks their pinned lengths and hashes; these identify the versions downloaded
for the experiment and are not publisher-issued checksums.

```sh
mkdir -p data
mkdir data/wsu-test1-originals
wsu_source='https://rds.westernsydney.edu.au/Centres/ICNS/2026/Jahir_Rodriguez'
wsu_flight="$wsu_source/dataset/test_1/Oct_11_2024/flight_5"
curl -fL "$wsu_flight/device_1_nosync.wav" -o data/wsu-test1-originals/device_1_nosync.wav
curl -fL "$wsu_flight/Oct-11th-2024-03-49PM-Flight-Airdata.csv" -o data/wsu-test1-originals/flight.csv
curl -fL "$wsu_flight/alignment_params.json" -o data/wsu-test1-originals/alignment_params.json
curl -fL "$wsu_source/dataset/test_1/Oct_11_2024/ref/DJIFlightRecord_2024-10-11_%5B14-32-34%5D.csv" -o data/wsu-test1-originals/reference.csv

PYTHONPATH=src .venv/bin/python scripts/benchmark_wsu_session.py discover \
  --audio data/wsu-test1-originals/device_1_nosync.wav \
  --output results/wsu-new/discovery
PYTHONPATH=src .venv/bin/python scripts/benchmark_wsu_session.py search \
  --discovery results/wsu-new/discovery \
  --flight-csv data/wsu-test1-originals/flight.csv \
  --alignment data/wsu-test1-originals/alignment_params.json \
  --output results/wsu-new/search
PYTHONPATH=src .venv/bin/python scripts/benchmark_wsu_session.py evaluate \
  --search results/wsu-new/search \
  --reference-csv data/wsu-test1-originals/reference.csv \
  --output results/wsu-new/evaluation.json
```

The original six-channel PCM32 WAV is 284,313,644 bytes at 48 kHz. The published
alignment maps audio time 82.0 s to flight-log row 39 (elapsed time 3,900 ms).
Every trajectory row is retained, using
`audio_seconds = 82 + (flight_milliseconds - 3900) / 1000`. The resulting
trajectory support is 78.1–241.1 s. Reference coordinates are opened only by
`evaluate`; discovery reads only audio, and search uses only the supplied flight.
This process separation is not a sealed holdout.

| Stage | Recorded result |
| --- | --- |
| Complete-recording discovery, unchanged defaults | Four proposals, all at 60.0–82.0 s; 2.80 s, 505 MiB peak RSS |
| Conditional search | All four proposals retained; 624 cells per scenario; constant-tone and 2% drift fits |
| Motion support | Zero fully supported fits in 4,992 attempted fits; every proposal has ridge samples before the first measured flight state |
| Final outcome | `no_supported_fit`; no coordinate, location error, or accepted localization |

Discovery ran at `1aa763b`; final search ran at `a661424` in 2.31 s with 157 MiB
peak RSS. The direct discovery/matching modules were unchanged between these
revisions. No detector thresholds or event windows were adjusted after inspection.
The four proposals remain unconfirmed sounds; missing trajectory support does
not establish their source identity or a false-alarm rate.

This low-speed multirotor case has limited Doppler variation. The measured log's
maximum reported horizontal speed is 11.40 m/s. A post-result calculation using
the published array reference and nominal 343 m/s sound speed gives radial speeds
of -7.66 to +7.41 m/s and a 4.30% relative decline from the maximum modeled
frequency factor to its minimum. The largest modeled decline among sampled
12-second intervals is 3.35%, close to
discovery's fixed 3% minimum. Discovery also requires at least 12 seconds,
4 Hz decline and sustained ridge support. Some modeled intervals cross the
fractional threshold, so low speed
alone does not explain why frozen discovery and the supplied trajectory yielded
no supported match.
These telemetry calculations establish a difficult operating regime, not a
diagnosed implementation defect or a measured rotor-frequency model.

The camera-based audio/GPS alignment has no measured uncertainty. The WAV has no
recording-clock tags; rounded UTC labels in the flight CSV do not establish the
audio clock. Published MSL heights are used as a common-datum approximation, with
flat receiver height derived from the initial aircraft altitude plus microphone
height. The grid covers the supplied flight footprint plus 500 m, rounded outward
to 100 m and sampled at 50 m cell centers. Clock error, altitude datum, weather,
background aircraft, and microphone GPS uncertainty remain unresolved. This
Australian supplied-flight experiment does not verify national acquisition,
automatic flight identification, or the independent real-location acceptance gate.

## Real reference full HTTP pipeline

This development replay verifies actual intake, automatic discovery, input
acquisition, tiled search, physical refinement and decision through the local
HTTP service. It uses the complete real recording, no supplied acoustic windows,
frequency bands or flight IDs, and the default CONUS request. Its restored flight
index and terrain are regional and incomplete. Its 20-second recording-start
interval surrounds a historical fitted clock, not an independently measured one.
It cannot satisfy independent real-location acceptance.

Restore the published inputs, build the partial index, and encode the entire
recording. The helper refuses an existing output directory. FFmpeg's `-n` flag
preserves an existing MP3.

```sh
.venv/bin/python scripts/restore_artifacts.py \
  --bundle data-source --bundle data-adsb-derived --bundle data-terrain
.venv/bin/python scripts/prepare_reference_upload.py \
  --data-root data --output results/reference-upload-setup-new
ffmpeg -n -nostdin -hide_banner -loglevel error \
  -i data/source/reference-recording.wav -map 0:a:0 \
  -codec:a libmp3lame -q:a 2 results/reference-whole-new.mp3
.venv/bin/python scripts/verify_upload_http.py \
  --audio results/reference-whole-new.mp3 \
  --operator-config results/reference-upload-setup-new/operator.json \
  --start-utc 2026-09-08T20:17:20Z --end-utc 2026-09-08T20:17:40Z \
  --time-source 'Development interval around the historical fitted clock; not independently measured recording time' \
  --expect-status inconclusive --require-full-pipeline \
  --timeout-s 1800 --poll-interval-s 1 \
  --output results/reference-http-new
```

At `a661424`, the actual upload completed in 253.71 seconds, with 16 independent
groups, 18 ridge records and 23 returned hypotheses. All seven operational
stages passed the strict verifier. The scientific decision remained
`inconclusive / search_incomplete`; `supported_decision` was false.
A fresh run after integration with upstream at `7df7c04` completed in 223.75
seconds, again passing the strict verifier with the same leading hypothesis,
counts and scientific status. The new audio resume checkpoint was created;
this run did not test restarting an interrupted analysis.
The leading diagnostic hypothesis was 65,998.78 m from the unsurveyed public
reference, measured after outputs were frozen. It misses all three distance
tiers, and this source-informed replay is ineligible for independent accuracy.

The search completed 19,190 of 19,278 tiles. This count includes empty tiles:
256 coarse points and 243 refinement points were actually scored, with possible
overlap between the grids. Eighty-six tiles lacked terrain coverage and two
exhausted the association budget. All 16 independent groups remained eligible;
there was no two- or three-event cap. Separate real-point diagnosis found
56 points in one failed tile that individually exceed 20,000 associations.
Smaller tiles alone cannot remove that failure.

The strict undated control returned `needs_input / recording_time_required`
and correctly failed the full-pipeline requirement. A separate Chromium check
loaded and reloaded the completed timed job, displayed its hypotheses and
incomplete-coverage warnings, and reported no page errors. It did not rerun the
analysis. Source revisions, measured runtimes and limits for these experiments
are kept together in [the compact result](real-http-2026-11-03/summary.json).
