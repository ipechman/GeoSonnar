# Bounded thunder-envelope comparison

The command-line companion can compare a complete, explicitly selected audio
interval against an assumed acoustic curve from a lightning catalog. Version
0.10.0 also exposes an optional GUI comparison with Doppler frequency on the
same complete receiver/clock grid. Both remain separate from the existing
reviewed thunder-arrival timing method.
It produces hypotheses and inspection curves, not a location verdict or a
probability. Independent field accuracy and hybrid improvement remain unproved.

The separate [shared-location/time joint-audio pilot](JOINT_ENVELOPE_VALIDATION.md)
compares this method with measured Doppler evidence using unchanged 0.9.1
programs. Its weak-case results include both gains and losses; the native
shared-cell implementation preserves its full-domain ordinal comparison without
changing either physical engine or the GUI's picked-arrival comparison.

## Optional GUI workflow

1. Under **Audio review → Thunder envelope**, choose **Use displayed time
   interval**, adjust its start/end, and write a review note. The selected interval
   remains fixed when you zoom or pan. **All controls** exposes FFT size, cadence
   and the requested frequency band. Click **Measure reviewed envelope**; this
   does not confirm events or classify their source.
2. Under **Search**, choose **Doppler + envelope (experimental)**. In Evidence,
   explicitly select Doppler frequency and disable picked Doppler/thunder timing.
   Review each Doppler event and its approved aircraft/tone alternatives. This
   method currently uses constant sound speed; an atmospheric profile must be
   removed explicitly. No evidence settings are changed by switching methods.
3. Declare the same area, receiver heights and clock range for both methods.
   Clock spacing must equal the measured envelope cadence, with matching
   endpoints. Enter the lightning catalog's actual requested UTC coverage;
   first/last strike times alone are not evidence of query coverage. Declare
   source-height fallback and distance/kernel assumptions where needed.
4. **Estimate shared-cell comparison** checks aggregate cost and source bindings.
   **Run reviewed shared-cell comparison** first preserves the exact request in
   case history. Cancel stops further computation and retains unfinished cells.
5. In **Results**, inspect separate Doppler and envelope views and their combined
   ordinal ordering. Sort coordinates, clock, raw error/cosine or availability.
   Read the individual leaders' spatial and clock disagreement. A selected row
   pins the map; it does not apply source assignments or change the recording
   clock. Export a saved copy for full JSON/CSV cells and a bounded HTML preview.

Raw scores retain their distinct meanings: lower Doppler residual is better;
higher envelope cosine is better. On the entire shared domain, adjacent Doppler
scores within `1e-10` and envelope scores within `1e-7` form transitive tie groups
with midpoint ranks. The combined cost is the worse of the two ranks. No fitted
weight or confidence probability is implied. A misleading catalog can harm the
Doppler result, and combination always has a best row when the domain is fully
rankable. Any unavailable or unfinished cell withholds full-domain combined
ranking; the program never silently removes candidates to obtain a result.

The first GUI version provides cell tables, source/tone assignments and map pins.
Predicted Doppler/envelope curve overlays for these joint cells are not yet wired
into its audio view. The separate envelope CLI still exports inspection curves.

## Shared-cell command line

The same native request can be previewed or run without a GUI:

```powershell
geosonnar preview-joint-evidence --request joint-request.json --out preview.json
geosonnar search-joint-evidence --request joint-request.json --recording recording.wav --out new-run --cancel-file cancel.flag
```

The request stores the frozen review, normalized aircraft/lightning inputs,
measured envelope, both plans and explicit work limits. Source paths are bound
by content hashes and verified before scoring. Run output keeps the request,
start/source receipts, numerical result, full report export and terminal receipt.
Failures are recorded without overwriting existing outputs. Exit 4 means
cancelled; exit 3 means computation completed with unavailable combined ranking;
exit 2 means invalid input or a failed operation. Removing a cancellation file
does not resume a saved partial run: start a new explicit run instead.

Limits include 100,000 shared cells, a 16 MiB serialized request, 32 MiB numerical
report, 256 MiB declared working-memory bound, and aggregate physical-engine
work caps. Envelope tiles retain at most 40 clocks each but account for every
declared cell and the total work across tiles. Estimates do not reset budgets
for each tile. Smaller areas, shorter clock ranges and fewer approved aircraft
alternatives reduce work; narrowing after seeing results is exploratory.

## What is compared

Each original catalog row supplies equal unit **energy**. Source current,
intensity and multiplicity do not determine acoustic amplitude. The model uses
ECEF slant distance, constant sound speed, an explicit source-height assumption
when necessary, and an inverse-square energy factor with declared distance floor
and cutoff. A finite, normalized exponential energy response is integrated over
each sample bin. Contributions add as energies; their square root gives relative
RMS. This avoids applying inverse-square attenuation directly to pressure RMS.

Bins are centered on the measured sample timestamps. The native measurement uses
the square root of integrated one-sided periodic-Hann PSD in selected bins
(20–250 Hz by default). Bins at the numerical -200 dBFS/Hz display floor contribute
zero rather than turning decoded silence into artificial signal. A wholly silent
or below-floor interval is refused. It is not a brick-wall time-domain filter. Timestamps are
STFT window centers, measured from the first decoded audio sample. The original
sample rate must support the requested upper frequency; resampling does not
recover missing bandwidth. An optional `band_hz` pair explicitly selects a
reduced band within 20–250 Hz, without claiming it is a full-band recording.
At least two FFT-bin centers must lie inside the requested interval.

The comparison searches every declared clock offset at the measured cadence,
using a zero-padded linear FFT numerator and full-window cosine normalization.
Positive clock offset means the recording began later than nominal UTC. Every
offset uses the same complete observed interval; partial overlap, masks and gap
filling are refused. Silent predicted windows are unavailable, not perfect fits.
Numerically weak windows are withheld, retained scores are recomputed directly,
and near-tie/retention ambiguity remains explicit.

## Explicit workflow

All paths inside configuration files resolve relative to that configuration.
Output directories must be new, with an existing parent. No network acquisition
or GUI launch occurs. Example measurement configuration:

```json
{
  "schema_version": 1,
  "recording": "recording.wav",
  "channel": 0,
  "start_s": 30.0,
  "end_s": 90.0,
  "fft_size": 4096,
  "hop_s": 0.1,
  "band_hz": [20.0, 250.0],
  "reviewed_selection_note": "Full interval chosen for thunder-pattern inspection; source identity remains uncertain.",
  "ffmpeg": null,
  "ffprobe": null
}
```

```text
geosonnar measure-thunder-envelope --config measurement.json --out measured
geosonnar preview-thunder-envelope --config run.json --out preview.json
geosonnar search-thunder-envelope --config run.json --out search-001 --cancel-file cancel.request
```

Measurement writes `observation.json` and a receipt with original recording hash,
channel, decoded interval, sample rate, FFT/hop, method and configuration. A source
hash identifies bytes; it does not authenticate a recording, source class or UTC.
To request cooperative cancellation, create the named cancellation file. A file
already present cancels before scoring. Only completed receivers are retained.
Terminating the process directly can leave an interrupted output directory.

`run.json` contains:

| Field | Meaning |
| --- | --- |
| `schema_version` | `1` |
| `recording` | Original recording path; bytes must match the observation |
| `observation` | Path to measured `observation.json` |
| `lightning` | Native lightning JSON/CSV interchange file |
| `lightning_provenance` | Provider, source, license, notes and altitude datum; embedded provenance takes precedence |
| `plan` | Explicit search plan below |
| `preview_curves` | Optional number of curve previews, 0–5; default 1 |

The plan has `schema_version: 1`, `catalog_sha256` (exact lightning input bytes),
`catalog_utc_interval` (two Unix UTC seconds), `receivers` (unique IDs and
`location` with latitude/longitude degrees and ellipsoid height metres),
`nominal_recording_start_utc_s`, `min_clock_offset_s`, `max_clock_offset_s`,
`model`, `source_policy`, `keep_best` and `limits`.

`model` requires `sound_speed_mps`, `fallback_strike_ellipsoid_height_m` (or null
when all sources have known ellipsoid heights), `minimum_distance_m`,
`maximum_distance_m`, `kernel_decay_s` and `kernel_duration_s`. Declare these
before looking at evaluation truth. The kernel is an assumption, not a fitted
or validated thunder transfer function.

`source_policy` is null for ordinary native catalogs. FMI catalogs require the
same exact-hash-bound ETRS89 approximation and timestamp representation allowance
as timing scoring. Kernel decay must be at least that allowance; smoothing does
not establish clock accuracy. See [FMI_LIGHTNING.md](FMI_LIGHTNING.md).

`limits` declares `max_work_units` and `max_memory_bytes`. The preview reports the
estimated resources, all receiver/offset counts and required catalog time support.
The catalog interval must cover all receive bins plus the maximum propagation
and kernel lookback. This is declared query support, not proof of spatial coverage
or complete physical lightning detection. Rows outside that interval are refused.

The present bounds are 300 seconds of observed sample-bin support, 0.05–5 second
cadence, 8,192 samples, a 3,600-second clock span, 10,000 explicit receivers,
40 retained hypotheses, 500 million estimated work units and 256 MiB estimated
working memory. Tighter limits are allowed. A combination can exceed the work
limit even when each individual dimension is valid. This is not the proposed
worldwide or multiweek search.

The file workflow also budgets requested preview generation conservatively as
one whole search per preview. `total_work_upper_bound` includes search and
presentation and must fit the declared work limit. Set `preview_curves` to zero
for a result table without extra curve computation.

## Reviewing and preserving output

`request.json` and `started.json` preserve the exact assembled request and input
identities. `results.json` includes per-receiver outcomes, searched versus retained
counts, silent/numerically unavailable windows, cancellation and clock boundaries.
`overlays.json` and `review.html` show the requested number of retained hypotheses
(one by default, at most five). Each curve
uses its own peak normalization for display; the numerical score uses the full
original shape. HTML is a local file with no scripts, external assets or automatic
browser opening. Cancelled runs may have no curve previews.

`completed.json` is written last and binds every output artifact. Its status can
be completed, cancelled or incomplete. Search and presentation completion are
reported separately, so cancellation after ranking preserves the results without
claiming all previews were generated. An absent terminal receipt is an interrupted
or failed write, even if a partial `results.json` exists. `failure.json` records
ordinary failures when storage is available. Outputs never overwrite an existing
run. Replay requires original input files; they are not duplicated into this folder.

Do not add this envelope score and picked arrivals from the same thunder as
independent votes. Rain, wind, engines, microphone gain control, clipping, missing
lightning, extended channels and wrong source-height assumptions can all create
or obscure apparent agreement. Similarity maxima must be evaluated after the
entire region-by-time search, including misleading alternatives. The frozen
software-control protocol is `benchmarks/thunder_envelope_protocol.json`;
it does not establish independent field or hybrid accuracy.

A separately frozen **72-case synthetic stress experiment** has now completed
using the unchanged 0.9.0 CLI. Favorable cases achieved 9/12 top-1 and 12/12
top-5 containment within 2 km; weak cases achieved only 1/12 and 4/12, despite
higher median similarity. Wrong-catalog scores overlapped favorable scores.
These results support testing complementary evidence, without showing that
combination works or supplying an acceptance threshold. See
[the fixed design, all variant results and limits](THUNDER_ENVELOPE_STRESS.md).
Independent field and hybrid validation remain outstanding.

## Source-informed reference evidence

A frozen experiment after the 0.9 release used the unchanged 0.9.0 CLI on the
complete Thunderprint reference clip. All **nine** combinations of three dates
and three assumed source heights completed on the same **441 receivers × 601
clock offsets**. The correct-date catalog had 1,076 FMI rows; the preceding and
following dates had 117 and 69. All three correct-date heights selected
65.7° N, 14.0° E, **7.334 km** from the already exposed upstream
coordinate. The primary 3,000 m assumption gave cosine **0.78371**, versus
whole-search wrong-date maxima **0.70177** and **0.61617**; the observed
constant-envelope baseline was **0.70461**.

This is descriptive consistency within a predeclared 10 km tolerance on a coarse
grid, not independent accuracy or calibrated confidence. The preferred clock
changed with assumed source height; the upstream reference start was itself
inferred from a flash. Source-informed date/region restrictions, unequal catalog
populations and truncated top-40 output limit interpretation. No picked thunder
timing, aircraft tracks or combined scoring were used. **Independent field and
hybrid validation remain false.**

See [the full reference method, all nine maxima and preserved failures](STORM_VALIDATION.md#frozen-thunderprint-reference-after-09).
Local frozen configs, raw acquisition receipts and predictions are under
`output/benchmarks/thunderprint-reference-001/`; the verified descriptive report
is `evaluation-002/evaluation.json` there. These artifacts supplement the
historical controls and do not redistribute the original video.
