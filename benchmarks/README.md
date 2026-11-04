# GeoSonnar validation

The [Doppler/thunder ablation](HYBRID_ABLATION.md) is a separate headless accuracy experiment against the production workspace scorer: 48 synthetic holdouts, 32 correlated misleading/missing-data controls and a frozen protocol. The primary combined score did not improve Doppler-only; the report distinguishes single-aircraft benefits, misleading evidence, missing support and the absence of a verified real-world hybrid holdout. Run it with `python analysis/benchmark_hybrid_ablation.py --output output/benchmarks/hybrid-ablation-new`. It needs no recordings, GUI or live downloads.

This suite adapts applicable [DOPPLERGEIST validations](https://github.com/1337isnot1337/DOPPLERGEIST/tree/70626ad44fb9c7f4e2f5b225956c1f9281f91c13/benchmarks)
at commit `70626ad44fb9c7f4e2f5b225956c1f9281f91c13` to the native Rust program.
It does not execute the upstream Python application or treat its saved results
as GeoSonnar predictions. See `upstream_catalog.json` for all upstream test files,
the semantic mapping and explicit capability gaps; `upstream_notes.json` records
evaluation rules. Static catalog entries are not test passes.

From the project root, use a **new output folder** for each run:

```powershell
python benchmarks/run_validation.py --output output/benchmarks/validation-new
```

For the Windows workbench release, add `--gui`. This also tests and builds the
native GUI, records both executable identities, and runs an explicit installed
FFmpeg/FFprobe integration through case intake, discovery and saved event review.
The historical 0.2.0 application profile, `output/benchmarks/validation-013/`, passed
395 native tests including 63 GUI controls, four explicit media integrations,
and all 17 implemented stages with no omissions. Earlier native Windows
walkthrough evidence remains in `output/gui-validation/live-001/`. Automated
checks establish interface and pipeline behavior, not independent location
accuracy or hands-on usability of the latest interface.

The 0.3.0 application checks are recorded separately in
`output/benchmarks/hybrid-atmosphere-001/summary.json`: 446 distinct native
checks (69 headless GUI controls and four explicit media integrations included),
plus 31 Python controls. The full native run was followed by targeted checks
of the final changes. Three optional legacy/terrain integrations and the full
17-stage scientific replay were not repeated. Atmospheric contracts use
independent analytic comparisons; they do not establish field accuracy.
Eight real infrasound recordings passed native audio processing in
`output/storm-validation-002/`, with location and hybrid accuracy still blocked
by missing independent metadata. See [storm validation](../docs/STORM_VALIDATION.md).

The full run requires the existing source recording, restored reference tracks
and terrain, Rust, FFmpeg/FFprobe, Python, numpy, scipy and soundfile. Its build
uses the locked Rust dependencies offline. It preserves command logs, timings,
binary/source/input hashes and a machine-readable `summary.json`. A missing
required fixture or an unexpected result fails the run. Historical results are
not overwritten. There are no live data downloads or paid requests in this run.

Local recordings, downloaded data and generated receipts are intentionally
excluded from the source upload. The source-only checkout supports native unit
tests and the fabricated publication controls below; it does not contain every
input needed for a real-recording replay. The machine-bound historical
`native_search_sensitivity_contract.json` is also excluded. During publication
cleanup, reports, logs and executable snapshots from older validation runs were
retained, while bulky media and scoring intermediates from runs 001 through 011
were pruned. Run 012 was retained complete. Version 0.2.0 subsequently passed run 013; two
additional cached-terrain controls passed separately. Its dataset requests have
offline validation, without a live provider-download claim.

Publication controls use no private recordings:

```powershell
python benchmarks/test_runner.py -v
python benchmarks/test_source_package.py -v
```

These separately check packaging from canonical source notices, failure
preservation, source-export exclusions, credential detection and manifest/ZIP
integrity. Their results do not replace an application validation run.

### Native detailed-data validation

Saved-case and job controls are also included in the native suite: immutable
source/clue bindings, interrupted writes, cancellation, startup ownership,
stale-lock recovery, dependency reuse, failed retries and output/log tampering.
The real Windows demonstration in `output/jobs-validation/live-001/` cancels
and resumes the native regional rank-to-overlays chain, verifies all 13,810
candidate rows unchanged, and confirms completed stages reuse their existing
attempts. Its media inspection stage uses a copied actual FFprobe executable.

Native ADSB.lol acquisition, trace import, streaming archive integrity and
GeoTIFF controls are included in `cargo test`. They use fabricated data and
offline provider responses. Two additional cached-terrain comparisons are
explicitly optional because the downloaded tiles are not distributed:

```powershell
cargo test --locked --test terrain_validation -- --ignored
python analysis/validate_native_data.py --output output/native-data/replay-new
```

The second command requires the official archive manifest/cache, the example
terrain and previously reviewed audio priors. It runs the entire native import,
terrain grid, audio-search preparation and regional ranking path. It saves an
executable snapshot, input hashes, commands, failures and predictions without
reading an evaluation coordinate. The example is explicitly source-informed:
September 8, 20:00–22:00 UTC and the existing regional box. It is an integration
replay, not blind validation or a test of complete world coverage. Native live
download/resume evidence is retained separately in `output/adsblol-research/`.

For the core Rust checks:

```powershell
cargo test --offline --locked
cargo clippy --offline --locked --all-targets -- -D warnings
```

## What is checked

| Area | Validation |
| --- | --- |
| Propagation | Independent forward-time oracle, large timestamps, near-sonic motion, continuous CPA, multiple passes, no extrapolation or gap bridging |
| Search | Shared recording clock, fixed full-trace support, independent emitted tones, equal event weights, pass ordering, same-aircraft return passes, malformed inputs |
| Normalization/preflight | Explicit altitude assumptions and units, QNH, coordinate wrapping, duplicates, gaps, UTC calendar dates, actual linked search file and event timing |
| Audio | Channel selection and phase cancellation, anti-aliasing, FFT bins/centers/PSD, invalid files/channels, full-hour source compared with independent SciPy |
| Codec timing | Four exact upstream gapless MP3 configurations; duration and nonperiodic pilot offsets each within 2 ms. The no-Xing MP3 is a separate diagnostic, not a gapless pass |
| Decision rules | All 16 frozen development/held-out evidence cases: two supported and 14 inconclusive. Additional tests cover harmonic independence, ambiguity, sensitivity, clock windows and malformed evidence |
| Finite candidate evidence | Completeness, pruning/retention bounds, mode-to-candidate correspondence, omitted alternatives and honest unions of nearby uncertainty envelopes |
| Real replay | Fresh native searches against frozen GeoSonnar pair results, plus the pinned real faint-event trace and all-types tracks; geographic tiers reported separately |
| Failure preservation | Sparse data may return unverified alternatives; an explicit known-pair fixture with missing event coverage must return no candidate. Neither is accepted as a location fix |

Pair replay tolerances are at most 2 m displacement, 0.0001 Hz score drift and
0.05 s clock drift relative to GeoSonnar's existing frozen result. These use
upstream replay tolerances with **our own** score baseline. Upstream weighted
robust scores and measured-velocity interpolation do not have the same units or
behavior as our equal-weight ridge RMSE and piecewise-linear positions.

The faint-event case declares its parameter grids before evaluation. Its current
development gate requires the same aircraft, full trace support and error at most
1,000 m from the upstream platform reference. All 100/500/1,000 m outcomes are
also reported; the upstream historical 100 m miss is preserved. The video-reported
acoustic coordinate and rounded platform reference are evaluated separately.
Neither is surveyed microphone truth. No threshold is fitted to the new result.

## Historical numerical baseline

`output/benchmarks/validation-005/summary.json` records an earlier complete core
profile passing in 228.0 seconds on this machine, with no omitted stages. The
separate discovery profile and all 40 recordings are documented in [DISCOVERY.md](DISCOVERY.md).

- 149 native Rust tests and 10 harness failure controls passed; formatting,
  Clippy and the static-CRT release build passed.
- Ten new acoustic-model tests and five audio-prior pipeline tests pass, with two additional native padding/grid controls. The separate [audio-prior experiment](AUDIO_PRIORS.md) passes software checks while retaining an incomplete-coverage scientific result.
- The 12 reviewed-search integration tests cover identity binding, exact points and gaps, source relocation, generic geometry setup, and a known-answer physical search.
- All 16 upstream decision controls returned the expected status, reason and
  acceptance flag; all four gapless codec variants met the 2 ms limits.
- Detailed and fine pair replays have zero coordinate, score or clock drift
  from the frozen native baselines. Fine-grid error is 37.32 m from the video
  estimate, separately 33.37 m from the rounded platform reference.
- The full simplified archive remains unverified and cannot recover the
  unsupported reference pair. Restricting to all available parts of that pair
  preserves the evidence gap and correctly returns no candidate.
- The faint case recovers `a38237` at 184.34 m from the platform reference and
  177.27 m from the video estimate. Both 500/1,000 m tiers pass; 100 m fails.

The earlier `validation-002/cache-equivalence.json` additionally compares the
original uncached faint run with that release: all 29,922 candidate rows and all per-flight winners are identical,
on identical input bytes. Native elapsed time fell from 343.69 to 37.37 seconds
(9.20x on this machine). `package-receipt.json` verifies the delivered executable
is identical to the audited binary. Runtime is descriptive, not a portable speed
guarantee. That historical packaging identity is in
`validation-005/package-receipt.json`; the latest application package is bound by
the retained `validation-013` receipts.

The interrupted `validation-001` receipt is retained. Review caught an incorrect
test expectation before real replay: incomplete data can produce unverified
alternative hypotheses. The corrected suite separates archive nonacceptance
from the explicitly unsupported-pair no-candidate control; search parameters and
the original data were not changed to force a pass.

## Interpretation

Decision fixtures are constructed producer evidence. Passing them verifies the
policy logic, not the false-localization rate on real recordings. The native
`decide` command accepts the pinned evidence contract; `rank` and `rank-single`
continue returning **unverified hypotheses**. Their raw RMSE output is deliberately
not relabelled as the upstream robust-score contract. Search must first produce
actual coverage, alternatives and sensitivity evidence before a policy can be
used to accept its result.

The decision port checks every selected harmonic, so a good harmonic cannot hide
a bad measurement of the same aircraft. It also requires uncertainty evidence for
every near-tied finite mode included in an envelope. These are conservative
extensions to the pinned policy. A finite grid does not establish uniqueness
between grid points, and sensitivity radii are not calibrated probabilities.

Reference searches use known event windows, regional bounds and, for the faint
case, an inherited clock window. They are source-informed development regressions.
Evaluation labels are opened after predictions are written. Both remain on the
same filesystem; this is procedural separation, not an isolated blind trial.

## Remaining upstream capabilities

Measured-velocity Hermite interpolation, national tiled/indexed searches, parallel retention, interrupted
job resume and the web service are not implemented in GeoSonnar. Their upstream
tests are catalogued as gaps and are not counted as passed or silently skipped.
The 40-recording Livermore **automatic discovery** lane is now executed and
reported separately. Its annotation-band-assisted features/localization lanes
and the NASA/WSU empirical lanes remain unrun. Several upstream empirical cases
themselves correctly remain inconclusive.

Reviewed automatic proposals now connect to the existing flight matcher; the
[frozen comparison](REVIEWED_SEARCH.md) reports exact transfer checks and its
scientific outcome separately. Weak/missed events still require manual marking. Before a worldwide accuracy claim,
obtain independent recordings with surveyed microphone positions and reliable
capture clocks. The GUI should expose those tested stages and their uncertainty.

The pinned upstream MIT license and fixture receipts are under `upstream/`.
See the project's `THIRD_PARTY_NOTICES.md` for adapted code and data licenses.
