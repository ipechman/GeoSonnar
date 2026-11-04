# Detection proposals on independently labelled recordings

The default ridge detector proposes tonal contours for review. It has not been
validated as an aircraft-source classifier. A frozen run on a complete hour of
real annotated audio found many proposals in intervals labelled as containing
no salient aircraft. A second, previously unseen hour had no clean-negative
proposals but still missed audible-aircraft runs. This variation limits how
detection output can be used in a location investigation, even when subsequent
numerical fitting succeeds.

## Input and separation

The recording is the first environmental hour, `env_audio/1_AUDIO.wav`, from
[AeroSonicDB YPAD0523 v1.1.2](https://zenodo.org/records/10215080). The publisher's
[methodology](https://arxiv.org/html/2311.06368v1) and accompanying mapping notebook
define five-second aircraft-audibility labels. Class 0 means no salient aircraft,
class 1 means aircraft audible, and ignore labels cover uncertain boundaries.
They do not identify the source of each tonal ridge or annotate Doppler curves.

The complete hour was selected by its original filename before inspecting our
detector output. Channel 0 is the sole channel: mono PCM16, 22,050 Hz. The complete
ZIP member CRC and retained audio SHA-256 were verified. Acquisition, failed
attempts and a conservative allowance for prior metadata research totalled
94,001,054 bytes, below the declared 100 MB cap. The original dataset is CC BY-NC
4.0 and remains separate from the GPL application/source distribution.

The production 0.4.0 executable ran the complete recording with unchanged native
discovery defaults, including the 12-second minimum ridge duration. No manual
proposal removal, trimming, gain adjustment or threshold tuning was performed.
The ordinary dataset's ten-second negative clips would have been too short for
that default duration criterion; using the continuous hour avoids that trivial
negative test. Predictions were committed before the evaluator scored labels.
The audio, executable, tools, protocol, evaluator and 157 build-source files were
bound by hashes. A separate endpoint-sweep calculation reproduced the metrics.

## First-hour result

Native discovery completed in 7.11 seconds and produced 68 ridges in 64 harmonic
groups. Related harmonics count once per group. Classification uses the union
of a group's member intervals; groups crossing labelled classes or ignore bins
remain explicitly ambiguous rather than being removed without accounting.

| Measure | Result |
| --- | --- |
| Groups entirely within aircraft-audible intervals | 4 |
| Groups entirely within negative intervals | 51 |
| Mixed/uncertain groups, excluded from clean-group precision | 9 |
| Clean-group temporal precision | 4 / 55 = 7.27% |
| Negative-interval proposal rate | 51 in 42.25 minutes = 72.43 per negative hour |
| Aircraft-audibility runs overlapped by proposals for at least five seconds | 8 / 12 = 66.7% |
| Five-second-bin temporal precision | 38 / 98 = 38.78% |
| Five-second-bin temporal recall | 38 / 117 = 32.48% |
| Negative bins with at least 2.5 seconds of proposal coverage | 60 / 507 = 11.83% |

The bin counts are TP 38, FP 60, TN 447 and FN 79 after excluding 96 uncertain
bins (eight minutes). Proposal-union coverage inside negative intervals is
274.576 seconds out of 2,535 seconds. Bins, ridges and runs within this one
recording are correlated; they are not independent sites or independent trials.

These are **temporal correspondence measures**. A contour overlapping an audible
aircraft could still be another sound, and a negative label does not independently
identify every faint source. The run does not establish Doppler-source precision,
usable-trace recall, geolocation accuracy or calibrated confidence. Published
microphone coordinates are privacy-shifted, and the environmental hour's exact
UTC and external session identity were not independently resolved.

## Unseen second hour with unchanged settings

The next numeric complete environmental recording, `env_audio/2_AUDIO.wav`, was
selected before inspecting its waveform, spectrum or our detector output.
Public label metadata had already been inspected during feasibility research;
this is not strict experiment-wide blinding. The protocol froze the complete
hour, original mono channel 0, the same production 0.4.0 executable and settings,
and the same metric definitions. No thresholds, intervals or proposals were
changed after either result. The five metric functions are structurally identical
to the first-hour evaluator, and their five arithmetic controls pass.

The complete ZIP member passed CRC verification and its original PCM waveform
is bound by SHA-256. Acquisition used 73,464,209 HTTP payload bytes in 37 requests
with no failed attempts. A later provenance check retained the exact 422,693-byte
publisher mapping notebook matching the Git blob pinned before execution. It
confirms that environmental hour 2 uses label column `1`; this later preservation
did not change the predictions or evaluator. Both transfers remain below 100 MB.

Discovery completed in **7.03 seconds**, producing **21 ridges in 21 groups**.
Predictions were committed before label scoring. The saved executable, original
157-file compiled-source snapshot, inputs, evaluator and results were rechecked;
an independent endpoint-sweep calculation reproduced every group classification,
run overlap, exposure and bin count.

| Measure | First hour | Unseen second hour |
| --- | --- | --- |
| Clean-positive / clean-negative / mixed groups | 4 / 51 / 9 | 14 / 0 / 7 |
| Clean-group temporal precision | 4 / 55 = 7.27% | 14 / 14 = 100% |
| Negative exposure | 2,535 s | 2,800 s |
| Clean-negative proposals per negative hour | 72.43 | 0 |
| Audible runs with at least five seconds of proposal coverage | 8 / 12 = 66.7% | 6 / 9 = 66.7% |
| Five-second bins TP / FP / TN / FN | 38 / 60 / 447 / 79 | 40 / 0 / 560 / 48 |
| Bin temporal precision | 38 / 98 = 38.78% | 40 / 40 = 100% |
| Bin temporal recall | 38 / 117 = 32.48% | 40 / 88 = 45.45% |
| Negative-bin false-positive fraction | 60 / 507 = 11.83% | 0 / 560 = 0% |

Second-hour exposure is 440 seconds labelled aircraft-audible, 2,800 seconds
labelled negative and 360 seconds marked ignore. Its seven mixed/uncertain groups
and 72 ignore bins remain separately accounted for. The proposal union overlaps
**zero seconds** of clean-negative intervals. Temporal overlap still does not
prove a particular ridge is emitted by the aircraft, and three of nine audible
runs were missed. The sharply different hours must both remain visible.

The second hour is an untouched recording, but its external site/session identity
and exact UTC remain unresolved; it is not established as a new independent site
or session. Do not treat its 720 bins or 21 groups as independent validation cases.
These are results for the **unchanged 0.4.0 detector**, not measured accuracy
improvements from the 0.5.0 source-review interface.

The second frozen run is under `output/source-presence-002/`, with original media
under `data/validation/source-presence-002/`. Its protocol SHA-256 is
`5afb9a0b097540abc65a7b661181de79818c303cb169464559f834385b72ff60`;
`evaluation-001/evaluation.json` is
`180007228764ab173c3e315017b7c414d96bf8849a0510bbef671cec0ad3c12a`.
The independent calculation and binding checks are in
`evaluation-001/verification.json`. These local research assets remain separate
from source uploads and the application distribution.

## Operational consequence and retained evidence

Inspect and confirm the measurements, review the possible acoustic source, and
explicitly approve aircraft alternatives before interpreting a regional fit.
Detection sensitivity controls change proposals, not the probability that a
sound is an aircraft. A visually plausible predicted curve alone cannot identify
the source. Keep unknown-source hypotheses and misleading alternatives visible.

The complete local run is preserved under `output/source-presence-001/`, including
the frozen protocol, acquisition/runner/evaluator scripts, source snapshot,
native predictions and `evaluation-001/verification.json`. Original media and
provider data remain under `data/validation/source-presence-001/`; neither folder
is part of source uploads. This is a local research artifact, not a claim that a
source-only checkout supplies the external dataset or all frozen run artifacts.

Any source-review assistance or detector changes developed using these hours need
evaluation on separate, previously unseen recordings. Do not retune either run and
present the resulting performance as a new held-out result. Location validation
remains governed by [the field-validation plan](FIELD_VALIDATION_PLAN.md).
