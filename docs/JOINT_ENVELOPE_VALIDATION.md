# Doppler and thunder envelope on shared location/time candidates

The first frozen joint-audio pilot shows a useful tradeoff, rather than a
uniform improvement. In the weak group, combined ranking put a receiver within
2 km first in **5/12 cases**, compared with **4/12 Doppler-only** and **2/12
envelope-only**. At a five-receiver shortlist, it reached **10/12**, compared
with **12/12 Doppler-only** and **7/12 envelope-only**. Both gains and losses
must remain visible to the analyst.

For a shortlist of **31 joint location-and-time cells**, combined ranking
retained a candidate within **2 km and 1 second** in **11/12 weak cases**, versus
**7/12 Doppler** and **8/12 envelope**. This is the clearest benefit in the frozen
pilot; it does not erase the geographic shortlist losses.

This is a synthetic experiment using the unchanged **0.9.1** programs. It does
not establish field accuracy, an acceptance threshold or a generally superior
combined method. The GUI's existing combined mode still uses reviewed arrival
timing; this separate envelope comparison has not been added to that mode.

## What was frozen and measured

Twelve fresh base groups each produced four predefined cases: a long Doppler
trace with dispersed lightning, a short trace with clustered lightning, a wrong
lightning catalog, and aircraft timestamps shifted by six seconds. All **48
planned cases** completed, with no omitted, zero-energy or numerically unresolved
comparison cells. The paired variants share base parameters and are not 48
independent samples.

The experiment generated **24 actual mono pressure recordings**, each 120 seconds
at 4 kHz. Each contains a moving aircraft tone, thunder pressure from displaced
channel segments, and background noise. Wrong-catalog and shifted-clock controls
reuse the corresponding favorable audio. Independent source-phase synthesis
solves retarded emission time; the generator does not hand predicted frequencies
to the scoring engine. Thunder uses unequal source energies and finite pressure
packets, while the production model assumes equal energies and an exponential
energy response.

Doppler observations are measured spectral peaks in a fixed 500–1,400 Hz band.
The fixed intervals are 10–110 seconds for long traces and 45–75 seconds for
short traces. The native program measures the full recording's 20–250 Hz RMS
envelope. This frequency separation and simple source mixture make the experiment
easier than many field recordings. The peak selector is an experimental
measurement proxy, not a validated aircraft recognizer. Neither Doppler onset
timing nor picked thunder-arrival timing contributes an additional vote.

Every case searches the same **49 receivers at 2 km spacing** and **31 recording
start offsets from −7.5 to +7.5 seconds**. That is **1,519 shared cells per case**,
or **72,912 across the experiment**. Every Doppler cell profiles the same three
approved aircraft alternatives and the same 600–1,000 Hz emitted-tone bounds.
The source-generating receiver and clock are continuous random draws, without
grid-center rejection or favorable seed selection.

The native envelope engine runs once per receiver and retains all 31 clocks.
The native regional engine runs once per clock and returns every receiver's
best source/frequency fit. The global top-one hypothesis list is not used as the
complete candidate inventory. Missing scores would make the paired comparison
unavailable, rather than silently substituting the other modality.

The combined rule minimizes the worse of the two full-domain midpoint ranks:
ascending Doppler residual and descending envelope cosine. Adjacent numerical
near ties are joined conservatively. There are no fitted weights or extra tie
breaks. Clock is optimized for each receiver **after** fusion at shared cells.
Separately optimized clocks are never treated as common-time support.

All scripts, parameters and input bytes were frozen before measurement; measured
observations and requests were frozen before scoring. Full predictions were
committed before evaluation truth was parsed. Public generator seeds make the
labels reconstructable: this is prediction-first software separation, not a
curator-blind field test.

## Geographic shortlist results

Each entry counts cases with at least one receiver within 2 km of the generating
location. The denominator is 12 for every row. Ties use exact-budget guaranteed
containment; possible containment and expanded support are retained separately
in the detailed report.

| Case group | Method | First receiver | Top 5 | Top 10 | Top 20 |
| --- | --- | ---: | ---: | ---: | ---: |
| Long trace / dispersed lightning | Doppler | 6 | 12 | 12 | 12 |
| | Envelope | 10 | 12 | 12 | 12 |
| | Combined | 9 | 12 | 12 | 12 |
| Short trace / clustered lightning | Doppler | 4 | 12 | 12 | 12 |
| | Envelope | 2 | 7 | 9 | 12 |
| | Combined | 5 | 10 | 12 | 12 |
| Wrong lightning catalog* | Doppler | 6 | 12 | 12 | 12 |
| | Envelope | 1 | 2 | 4 | 7 |
| | Combined | 2 | 10 | 10 | 12 |
| Aircraft clock shifted +6 s* | Doppler | 4 | 11 | 12 | 12 |
| | Envelope | 10 | 12 | 12 | 12 |
| | Combined | 8 | 12 | 12 | 12 |

\* These inputs deliberately lack one correct joint explanation. Their location
containment is descriptive and must not be counted as acceptance of valid
combined evidence.

For weak cases, combined first-place ranking gained two cases and lost one
against Doppler, and gained four and lost one against the envelope. At top five,
it lost two against Doppler and gained three against the envelope. Median
first-place error was 2.56 km for Doppler, 4.95 km for the envelope and 2.65 km for
combined ranking. These different summaries make the tradeoff clearer than
reporting only the increased first-place hit count.

At the predefined 31-cell joint budget, weak-case combined ranking gained five
cases and lost one against Doppler, and gained three with no losses against the
envelope. Median tie-expanded support volumes were 58.00 km²·s for Doppler,
48.75 km²·s for the envelope and 53.25 km²·s for combined ranking, within the
declared 2,160 km²·s search domain. These are geometric search volumes, not
confidence volumes. The smaller joint shortlist asks a different question from
five receiver locations with their independently optimized clocks.

The wrong-catalog control demonstrates contamination: adding unrelated lightning
reduced Doppler's first-place count from 6 to 2 and top-five count from 12 to 10.
Conversely, a shortlist can still contain the true receiver despite an incorrect
catalog or clock. Geographic consistency cannot authenticate either source.
With the wrong catalog, the 31-cell combined shortlist retained location and
time in only 1/12 cases, versus Doppler's 6/12. The shifted-clock combined
first-place geographic count is 8/12 guaranteed and 9/12 possible under ties;
the table uses the guaranteed count consistently.

The report retains raw scores, constant-envelope baselines, fitted aircraft IDs
and tones, boundary flags, tie-expanded areas, shared-cell volumes and modality
support overlap at identical receiver/clock cells. Spatial overlap after separate
clock optimization is reported separately. These are descriptive diagnostics;
neither a high rank nor overlap is a calibrated disagreement detector or confidence
region.

## Reproduction and implementation checks

Local ignored artifacts are under `output/benchmarks/joint-evidence-001/`:

- `protocol.json` and the frozen generation, measurement, runner and evaluator
  scripts describe the complete method.
- `inputs-001/inputs-freeze.json` binds the 24 WAVs, source catalogs, tracks and
  separate evaluation labels.
- `run-001/prepared-freeze.json` binds measured evidence and native requests;
  `predictions-commit.json` binds all native results and full joint surfaces.
- `evaluation-001/report.json` and `completed.json` contain results and verified
  provenance. `figures-001/` contains standalone PNG/SVG comparisons, including
  all four variants of the fixed first base group.
- Capped execution receipts are `joint-*-001` and `joint-controls-003` under
  `output/benchmarks/envelope-empirical-001/`.

Twenty final controls passed, covering rank direction, conservative ties,
possible/guaranteed shortlist containment, missing/duplicate cells, exact clock
joins, geometric support, actual spectral tone/chirp measurement, silence,
independent retarded-time arithmetic and native result extraction. An earlier
mock fixture omitted a newly required progress counter; its failed receipt is
preserved and the fixture was corrected before the code freeze. Independent
review also checked 4,614 exhaustively enumerated small shortlist cases; its
separate provenance limitations are recorded in
`output/benchmarks/joint-evidence-review-001/review.json`.

Generation took 5.86 seconds, measurement/preparation 77.12 seconds, scoring
335.72 seconds and evaluation 20.83 seconds on this machine, under a 20% Windows
CPU cap. These timings include orchestration and are not general throughput
guarantees. No GUI, graphics device, provider download or private/original
recording was used. Production source and release executables were unchanged.

## Consequence for the tool

Preserve all three views and explicit analyst choice. Version 0.10.0 implements
an optional shared-cell comparison in the existing Search and Results tabs,
with a stable reviewed envelope interval, complete candidate accounting,
visible location/time disagreement and durable saved inputs/results. Its report
type is separate from picked-thunder timing. A bounded native wrapper replaces
the pilot's many command-line invocations while retaining cancellation,
unsaved-result and history protections. The 0.10.0 native wrapper reproduced all
48 cases and 72,912 cells exactly: maximum absolute differences in Doppler score,
envelope cosine and fitted tone were zero; source assignments, full-domain ranks
and boundary flags also matched. Five actual CLI controls passed for changed
source bytes, output overwrite refusal, unknown arguments, pre-cancellation and
completed computation with unavailable ranking. Receipts, complete requests and
results are under `output/benchmarks/joint-workspace-001/joint-replay-001/`.
This replay checks implementation parity and is not another independent holdout.

Fresh independently located field recordings and false-match calibration remain
open requirements. More detailed atmospheric modeling does not resolve the
observed source, clock and ranking ambiguities.
