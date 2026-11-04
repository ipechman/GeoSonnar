# Analyst-guided regional investigation

## Shared location/time evidence pilot

The frozen [48-case joint-audio experiment](JOINT_ENVELOPE_VALIDATION.md) now
compares measured Doppler and thunder envelopes on identical receiver/clock
cells. All cases completed with unchanged 0.9.1 executables. Weak-case combined
first-place containment was 5/12 versus Doppler 4/12 and envelope 2/12, while
top-five containment was 10/12 versus 12/12 and 7/12. Wrong lightning data could
degrade a useful Doppler shortlist. Synthetic gains do not establish field
accuracy or a default combination policy.
At the predefined 31-cell joint location/time budget, weak-case containment was
11/12 combined versus 7/12 Doppler and 8/12 envelope, showing a useful candidate
prioritization benefit that should be preserved alongside the geographic losses.

Version 0.10.0 implements a distinct, optional shared-cell comparison: freeze an
envelope interval in Audio review, preview the common search domain, preserve
individual and combined results with location/time disagreement, and save full
requests and statuses through run history. The native wrapper composes the
existing engines with aggregate work/memory limits. It retains the complete
receiver/clock domain, including unavailable and unfinished cells, rather than
inferring a full surface from truncated top-K reports. Headless validation passed
793 native tests, nine explicit integrations and 132 Python controls (one fixture
skipped). The new native wrapper exactly reproduced all 72,912 frozen pilot cells,
their scores, source/tone assignments, ranks and boundary flags. Five CLI failure
and cancellation controls passed. Reviewed arrival timing remains a separate
method; no sound is counted again as a timing vote. Predicted joint-cell curves,
independent field evidence and false-match calibration remain open work.

## Version 0.9.1 and post-0.9 evidence

The patch fixes default FFmpeg/FFprobe discovery for command-line envelope
measurement. Four isolated resolver tests and a real decoder/CLI regression
cover PATH lookup, bundled-helper priority and explicit missing selections.
The final source passed 745 native tests (154 headless GUI/launcher checks),
nine explicit media/data integrations, 132 Python controls with one unsupported
link-fixture skip, formatting, Clippy and the portable build. Six older real
hypotheses retained all predictions through 324 replay checks; the built CLI
passed 188 workflow checks. Original null-helper measurement and all nine new
storm-reference results replayed byte for byte. The default GUI renderer remains
the setup the user reported working in 0.7; no GUI was opened by the agent.

The preserved 0.9.0 program completed a source-informed Thunderprint reference
and a separate 72-case synthetic stress experiment. The reference chose one
coarse receiver across all three source heights, 7.334 km from the disclosed
location, with higher full-search similarity than both wrong-date controls.
The synthetic favorable group reached 12/12 top-five containment, while weak
cases reached 4/12 despite higher median similarity. Neither experiment proves
independent field accuracy, a reliable acceptance threshold or hybrid benefit.
See [storm evidence](STORM_VALIDATION.md) and
[synthetic stress evidence](THUNDER_ENVELOPE_STRESS.md).

The next comparison needs the same receiver-and-clock candidates for Doppler
and envelope evidence, with weak cases and misleading alternatives frozen
before scoring. Separately profiled best-clock result tables must not be fused
as if they represented shared timing support. Existing picked-thunder timing
and envelope evidence from the same sound must not become independent votes.
Independent source curation and false-match calibration remain release-readiness
gaps; the broader useful/validated OSINT objective is still active.

## Scope

This is the active development and acceptance plan following version 0.3.0.
Its scope is a useful, tested investigation workflow, including unresolved and
misleading cases. Passing numerical tests alone does not complete field validation.

## Operator contract

1. Preserve the original recording and review explicit measured events. Detection
   proposes events; only the analyst confirms and selects them.
2. Declare a region, receiver-height assumptions, and a bounded recording-start
   uncertainty. One shared clock applies to all selected observations.
3. Inspect plausible aircraft/strike alternatives and explicitly choose which
   associations may be searched. Keep emitted-tone uncertainty bounded and visible.
4. Preview work and data support before starting. Search only the declared space,
   with progress and cancellation. A cap or cancellation must never imply a complete
   search or a verified location.
5. Review multiple location/time/association hypotheses, individual residuals,
   modality disagreement and boundary warnings. Applying a hypothesis to the manual
   workspace is a separate undoable action; it does not verify a location.
6. Save the input identity, declared search space, all analyst choices, work coverage,
   retained alternatives, limitations and reproducible output. Changed evidence or
   data makes an earlier result stale.

## Implementation and acceptance evidence

| Requirement | Evidence required before completion |
| --- | --- |
| Shared uncertain time and reviewable associations | Production backend plus headless tests with unknown clock, misleading alternatives and unchanged input snapshots |
| Useful regional throughput | Measured realistic regional replay, bounded memory/work, cancellation and explicit retained-versus-searched counts |
| GUI preserves manual control | Headless interaction tests for inclusion, preview, run/cancel, stale results, explicit application, Undo and save/reopen |
| Ambiguous evidence remains visible | Separate modality support, competing hypotheses, boundary flags and missing-data outcomes; no invented confidence |
| Terrain and data readiness | Explicit vertical-datum handling, real coverage/gap diagnostics and tested receiver-height sampling before fine search |
| Investigation handoff | Portable saved settings and a readable evidence dossier with source identities, assumptions, competing results and reproduction instructions |
| Numerical validity | Independent analytic/synthetic cases, including weak individual evidence, wrong associations, wrong clocks and out-of-region truth |
| Field validity | Independently supported recording location/time and matching data, with prediction committed before evaluation; source-informed replays labeled separately |
| Windows delivery | Headless application/media checks, clean lint/format, corresponding GPL source package, build identity and preserved prior release |
| Cleanup | Remove owned build/scratch artifacts after preserving test receipts and runnable delivery; retain original/private recordings and downloaded evidence |

## Validation boundaries

The frozen 0.3.0 ablation remains historical evidence and is not a tuning set.
New search settings and rejection criteria need fresh validation. Broad weak
evidence is useful when its intersection narrows the candidates; precision on
strong-Doppler cases is not the only measure of usefulness. Record shortlist
coverage, false matches, unresolved cases, runtime and required analyst input.

Frost access is deferred at the user's request. Seek accessible independently
supported recordings and permitted data alternatives without creating accounts
or inventing missing truth. Public reference coordinates that informed method
development do not become blind ground truth. If independent field evidence is
unavailable, report the unmet requirement and keep the overall goal incomplete
while other meaningful work can continue.

Atmospheric profiles remain optional local refinement. More atmospheric detail
does not replace correct associations, clock handling, data support or field tests.
All automated validation must run headlessly without opening or controlling the
user's desktop applications.

## Current evidence, November 3, 2026

The regional backend and report export passed 27 and 12 targeted checks in
`output/benchmarks/regional-workflow-001/core-dossier-tests-003.log`.
All 84 headless GUI checks passed in `gui-tests-006.log`, including independent
modality review, all-receiver selection, explicit refinement, resizing and
bounded rendering of a two-million-point trajectory. These are implementation
checks; the newest workflow is not yet in the 0.3.0 portable release.

The frozen regional synthetic experiment passed 231 functional/analytic checks
after the documented numerical CPA repair. Its original failed run remains
preserved. Noisy accuracy remains descriptive rather than an acceptance gate.

The two real ESID103 automatic-proposal trials failed their 25 km comparison,
with leading estimates 102.1 km and 56.1 km from the publisher marker. A separate
exploratory assistant spectral review extended the first trace from 13 to about
70 seconds, with 32 accepted measurements and explicit local peak rejection.
Its leading location was 34.7 km away; the nearest grid point ranked 11th among
distinct receivers, compared with 129th for the automatic fragment. This remains
a failed leading-location comparison and a source-informed exploration, not an
independent accuracy success. Measurement uncertainty also changed from the
native bin scale to the helper's explicit 1 Hz scale, so it is not a pure
trace-length ablation. The region, clock and emitted-tone bounds, approved
aircraft alternatives, assumed height and sound speed stayed unchanged.

`output/audio-review-reference-search-001/exploratory-evaluation.json` preserves
these outcomes. All 1,000 original baseline hypotheses replayed identically with
the new geographic-reporting code. `analysis/audio_review_measurements.py`
supports reproducible audio-only figures and measurements along explicitly
supplied anchors; its four independent signal/input checks passed. It never
confirms an aircraft or reads a location. The pending review and the experiment's
scoring-eligibility flags remain distinct.

Native managed regional ADSB.lol acquisition has since been implemented and
checked against the separately acquired reference data: 19 detailed traces and
615 indexed positions matched, with a zero-transfer cached resume. Acquisition
coverage remains incomplete by construction; this does not identify a sound.

The fresh deployment B experiment froze 40 audio-only measurements before
reviewer access to aircraft identities or exact receiver coordinates. Its 95
eligible native passes and 130 receivers completed in 34.45 seconds. The leading
receiver was 80.984 km from the publisher marker; the closest top-10 or top-20
receiver was 26.187 km away, outside the frozen 25 km tolerance. The nearest
available grid point ranked 85 of 130. These are failed candidate-location
results; the tone has no independently established aircraft identity. Predictions
were committed before evaluation, and no measurements or bounds were retuned.
See `output/field-validation-fresh-B-search-001/publisher-evaluation.json`.

The current 0.4.0 code passed all 17 implemented validation stages, with 560
native tests including 97 GUI tests after the explicit media and cached-data
integrations. The new prediction overlay reproduced the committed fresh-field
winner's score exactly without changing measurements. The separate one-hour
source-presence audit exposed a high proposal burden in negative-labelled audio;
see `docs/DETECTION_VALIDATION.md`. No thresholds were tuned on that hour.

The 0.4.0 portable Windows folder/ZIP and corresponding GPL source ZIP have been
assembled and verified. `output/benchmarks/regional-workflow-001/delivery.json`
binds the artifacts and all 157 compiled-source snapshot entries. The separate
cleanup receipt records 6,291,373,387 bytes removed from owned build/test scratch;
original recordings, downloaded datasets, frozen experiments and older releases
remain. This post-delivery status note does not modify the frozen release files.

Still required for the overall investigation-tool goal: independent field
success, reliable false-match/insufficient-evidence evaluation, a supported real
hybrid recording and hands-on analyst usability feedback. Prioritize explicit
acoustic-source assessment and evaluation on unseen recordings; do not tune the
new source-presence hour and call it a fresh holdout. Acquisition research and
real-data exclusions must remain visible rather than being replaced with
favorable examples.

## Source assessment increment, version 0.5.0

Source assessment is now separate from measurement confirmation. Compatible
source classes and explicitly reasoned uncertain hypotheses may be scored;
unassessed/incompatible active events block, without removing selections or
blocking disabled modes. Measurement edits reset review and source assessment.
Legacy reviews retain their values and default to unassessed. Uncertain-source
assumptions remain visible in Results, prediction overlays and exported dossiers.
This is an analyst control, not an improved automatic source classifier.

All 17 implemented validation stages passed in `validation-015`, with 584 native
tests including 105 GUI tests, 130 Python controls plus one unavailable symbolic-
link fixture, 231 regional synthetic checks and five frozen field-inspection
source-gate variants. The original failed field fit's physical predictions and
score remain identical when explicitly evaluated as an uncertain hypothesis.
The aggregate receipt is `output/benchmarks/source-assessment-001/summary.json`.

The next untouched AeroSonicDB environmental hour was evaluated using the same
preserved detector and defaults. It had no proposal overlap with labelled
negative intervals, unlike the first hour, and covered six of nine audible runs.
Both results remain in `DETECTION_VALIDATION.md`; neither independently identifies
every source or measures the efficacy of the new manual review controls.

Next implementation priority: reopening complete saved investigations into
interactive Results, preserving exact review/input identities and readable
partial results even when external data are unavailable. Input restoration must
retain original lightning provenance and ordered layer interpretations; generic
replacement metadata currently changes the full input fingerprint. Do not bypass
that mismatch. Loading must not apply a hypothesis or start scoring/downloading.

The Western Sydney UAV lead now has an executable bounded acquisition plan with
GPS/UTC logs, a separate microphone-reference log and an audio-alignment record.
Use it first as a declared reference trial, qualifying clock/height/position
uncertainty and variable rotor frequency. No UAV waveform or successful location
result is claimed yet; the exact plan is linked from `FIELD_VALIDATION_PLAN.md`.


## Version 0.6.0 application and startup compatibility increment

Complete saved investigations now reopen into interactive Results with explicit
restoration of receipt-covered normalized inputs. Archived inspection checks the
matching recording and unchanged review; Apply and new scoring retain original
source verification. The real failed-field hypothesis replay preserved every
prediction and measured residual, including after removing only owned fixture
copies of its raw sources. This is persistence validation, not a new location trial.

A user-reported 0.5.0 startup stall redirected work to Windows rendering. The
new default Direct3D12 path, separate software launcher, bounded settings reads
and startup logs passed headless controls; no graphics context or application
window was opened, so responsiveness on the affected desktop remains unverified.
The application profile passed 609 native tests including 122 GUI/launcher tests,
132 Python controls with one unavailable symbolic-link fixture, and all 11 scoped
stages. Native junction tests passed. Historical 17-stage scientific replays were
not rerun. The aggregate receipt is
`output/benchmarks/investigation-reopen-001/summary.json`.

The acquired WSU reference minute produced no default proposals. A separately
frozen manual trace did not favor nominal Doppler over a flat tone at the supplied
known receiver. All five fixed-clock outcomes, limitations and 259 independent
arithmetic/binding checks are retained in `FIELD_VALIDATION_PLAN.md`. No geography
was optimized and no alternative interval was substituted after the outcome.

The user subsequently confirmed that the 0.6.0 software launcher remains
responsive on the initial Case screen. This does not establish normal-renderer
compatibility or media/search usability; the exact response and binary hash are
preserved in `output/benchmarks/investigation-reopen-001/user-startup-feedback.json`.
Scientific priorities remain usable-source assessment, false-match and
insufficient-evidence evaluation, and an independently supported real hybrid case.

## Version 0.7.0 descriptive fit diagnostics

Candidate inspection now adds per-event flat-tone and linear-drift comparisons.
Both use the exact measured timestamps and residual scales of the unchanged
inspected Doppler curve. Every measurement needs one valid prediction; missing
support withholds the comparison rather than selecting a favorable subset.
Ordinary Hz RMSE and normalized RMSE are reported separately. No score, source
approval, evidence selection, emitted frequency or result ordering is changed.

These are in-sample descriptive checks with unequal parameter counts and no
accounting for previous search size or correlated trace samples. They do not
establish a source identity, location confidence or a false-match threshold.
The additive replay protocol fixes all five previous WSU clock scenarios and the
unsuccessful fresh-B leading hypothesis before calculating the new diagnostics;
these known outcomes are regression examples, not new holdouts. The protocol is
`output/benchmarks/fit-diagnostics-001/protocol.json`.

All seven scoped validation stages passed, with 618 native tests including 124
headless GUI/launcher checks. The six real replays passed 324 independent checks,
preserving every prior prediction and score exactly. Seven external-media/cached
integrations and the historical scientific/Python profiles were not repeated;
their prior execution remains tied to older builds. The portable static-C-runtime
build and launcher configuration probes passed without opening graphics.
The aggregate receipt is `output/benchmarks/fit-diagnostics-001/summary.json`.

The nominal WSU trace has Doppler/flat/drift RMSE 4.528/4.491/4.370 Hz. The failed
fresh-B leading hypothesis has 1.517/4.167/3.152 Hz respectively. Thus a wrong
candidate can beat both baselines. Preserve that counterexample and evaluate
future decisions against searched misleading alternatives, not just simple tones.
No acceptance threshold or renewed location success is claimed.

A live FMI historical-lightning request worked without Frost credentials and
became the next acquisition increment, completed in 0.8.0 as recorded below.

## Version 0.8.0 acquisition and application controls

Bounded analyst-requested FMI acquisition and native import preserve whole-second
timestamps, original ETRS89 coordinates, raw row identity and unresolved location
error units through reload and archive. Explicit source interpretation precedes
scoring. This is Nordic coverage, not worldwide or Quebec data.

The release also adds Guided view without changing analytical settings, two-axis
spectrogram zoom, and Cancel all and exit with owned Windows helper cleanup.
The user reported using the default 0.7.0 GUI without issues. That feedback is
recorded separately from headless testing of 0.8.0; no native GUI was opened by
the agent, and the original 0.5.0 stall's cause remains unestablished.

The 0.8.0 application profile passed 689 native tests, including 143 GUI/launcher
checks, eight ignored entries of which seven external integrations were run
separately (the remaining child entrypoint is exercised by its parent test),
and 133 Python controls with one symbolic-link fixture skipped. Six preserved
real hypotheses passed 324 independent replay checks without score changes.
A live one-minute FMI request matched 36 original rows and 144 source values,
then reused cached data with no transfer. This validates acquisition, not a
recording location. Receipts and cleanup are in
`output/benchmarks/fmi-pipeline-001/`; prior releases remain preserved.

## Version 0.9.0 history and bounded envelope comparison

Each explicitly started regional/comparison run now retains its original review,
source bindings, status and result when storage succeeds. Interrupted runs remain
visible, completed results survive later edits, and storage failures retain an
unsaved outcome with close protection and explicit retry/discard controls.
Compact history is not a complete self-contained input archive.

The originally proposed thunder-pattern comparison now has a separate bounded
native method with explicit energy/RMS units, centered sample bins, full-overlap
clock search, resource preview, cancellation and inspectable curves. Do not silently add
it to picked-arrival timing as independent evidence. Arithmetic controls and
synthetic examples do not satisfy the independent storm/hybrid field gate.

New metadata research found GPS-timed EXAEDRE/HyMeX recordings, but the linked use
policy requires investigator contact; no audio was acquired under the current
no-contact constraint. OSU1 recordings are publicly licensed but their absolute
campaign clock accuracy was not established. Research facts and unresolved issues
are retained in `output/ground-truth-leads-003/findings.json`; no favorable field
outcome is inferred from metadata or nominal timestamps.

The final 0.9.0 source passed 741 native tests including 154 GUI/launcher checks,
eight explicit media/data integrations, 132 Python controls with one unsupported
symbolic-link fixture skipped, formatting, Clippy and a portable static-CRT build.
Actual native hard-link and junction fixtures passed. Built CLI commands passed
188 controls; six frozen real hypotheses passed 324 replay checks with unchanged
predictions and scores. All tests ran headlessly with two Cargo jobs/two Rust test
threads and a 20% Windows child-process CPU cap. Failed attempts remain preserved
beside the successful receipts in `output/benchmarks/history-envelope-001/`.
The historical 17-stage scientific profile and live provider acquisition were
not repeated. Independent storm/hybrid accuracy and false-match calibration
remain unmet requirements for the overall goal.
