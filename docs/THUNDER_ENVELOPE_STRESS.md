# Synthetic thunder-envelope stress results

The unchanged GeoSonnar **0.9.0** command-line program completed all **72** cases
in a separately frozen synthetic experiment. Favorable cases produced useful
shortlists, but weak evidence often produced incorrect locations with high
similarity scores. This supports testing complementary evidence; it does **not**
show that combining Doppler and thunder improves results.

## Fixed experiment

Twelve independently seeded base groups each supplied six variants: favorable
storm audio, weak evidence, a wrong lightning catalog, a catalog missing one
third of its rows, a receiver outside the search region, and non-thunder audio.
The variants within each base group are correlated. There are **12 independent
base groups**, not 72 independent recordings; paired summaries preserve that
grouping.

An independent Python generator produced real pressure waveforms using unequal
source energies, varying channel heights and durations, three displaced channel
segments per flash, finite sinusoidal packet envelopes, bandlimited noise and
background sound. The search retained its fixed equal-energy, point-source,
exponential-response model. Weak variants changed only signal-to-noise ratio
or source positions relative to their favorable pair. Wind, engine and
rain/rumble simulations supplied the non-thunder controls.

Every case used a complete 120-second recording, native 20–250 Hz RMS measurement
(actual FFT-bin centers 20.5078125–250 Hz), and the same **121 receivers × 61
clock offsets**. The receiver grid had 2 km spacing over a 20 × 20 km square;
clock offsets covered −15 to +15 seconds at 0.5-second steps. In-region truth
was off grid; outside receivers lay at least 6 km beyond the boundary. All
**531,432** planned receiver-clock comparisons were evaluated, with no zero-energy
or numerically unresolved offsets.

Protocol, code, seeds and inputs were frozen before scoring. Native measurements,
requests and previews were also frozen before the first search, and predictions
were committed before the evaluator opened receiver/clock labels. Public seeds
make those labels reproducible: this is not an independently curated blind field
test. No parameters or acceptance threshold were fitted to the outcomes.

## Results

Each receiver is ranked by its maximum over all 61 clocks. A shortlist hit means
at least one of its **distinct receivers** lies within **2 km** of synthetic
truth. The predeclared numerical-tie treatment gave identical possible and
guaranteed hit counts in this run. These are geometric containment counts, not
confidence levels.

| Variant | Cases | Top 1 hits | Top 5 hits | Top 20 hits | Median full-search cosine |
| --- | ---: | ---: | ---: | ---: | ---: |
| Favorable storm | 12 | 9/12 | 12/12 | 12/12 | 0.8451 |
| Weak evidence | 12 | 1/12 | 4/12 | 6/12 | 0.8590 |
| One-third of catalog rows missing | 12 | 7/12 | 9/12 | 11/12 | 0.8274 |
| Wrong catalog, same favorable audio | 12 | — | — | — | 0.8269 |
| Receiver outside region | 12 | 0/12 | 0/12 | 0/12 | 0.7890 |
| Non-thunder audio | 12 | — | — | — | 0.7937 |

Wrong-catalog and non-thunder outputs are never counted as causal location
successes. Outside-region containment is impossible by construction; its useful
diagnostic is the high score still returned by an entirely incorrect region.

The weak group comprised six low-SNR cases (−8 dB), with **0/6** top-1 and
**2/6** top-5 hits, and six clustered-source cases, with **1/6** top-1 and
**2/6** top-5 hits. Each subgroup reached only **3/6** containment at top 20.
Clustered-source cases had a median maximum of **0.8873** despite poor location
accuracy. Median leading-point error was **0.659 km** for favorable cases and
**10.203 km** for weak cases.

Favorable top-five shortlists had a median geometric area of **18 km²** out of
400 km² and contained truth in all 12 cases. Weak top-20 shortlists covered a
median **65 km²** yet contained truth in only half the cases. A small displayed
area alone therefore does not establish a useful or reliable shortlist.

The wrong catalog outscored its correct-catalog pair in **1/12** groups. Wrong
catalog maxima ranged **0.7018–0.9014**, overlapping favorable maxima of
**0.8107–0.9101**. The median weak score was also higher than the median favorable
score. No reliable acceptance threshold or false-positive rate follows from
these results.

The constant-positive-envelope baseline was calculated directly from measured
RMS as `sum(y) / sqrt(N * sum(y*y))`. Low-SNR cases had a median baseline of
**0.9900**, above their median searched maximum of **0.8222**. Conversely, wrong
catalogs still exceeded their baseline by a median **0.1047**. Neither a high
raw cosine nor improvement over this simple baseline is sufficient to verify a
location.

## Limits and next validation

These are declared simulations with constant sound speed, not natural storm
recordings. They omit terrain, refraction, realistic channel development,
instrument response and codec defects. No aircraft evidence or hybrid score
was evaluated, and no field, worldwide or multiweek accuracy claim is made.
All cases and failures remain in the frozen record; any later tuning needs a
new protocol and fresh evaluation seeds.

Weak-case failures make the case for investigating complementary information,
while warning against confidently accepting the strongest thunder score. The
next validation must compare separate and combined evidence on the **same**
independently located recordings, with misleading alternatives and explicit
disagreement. It must test shortlist containment and ambiguity as well as the
leading-point error. These results do not establish that combination works.

Local evidence is preserved under
`output/benchmarks/thunder-envelope-stress-001/`: `protocol.json`,
`inputs-001/inputs-freeze.json`, `run-001/scoring-inputs-freeze.json`,
`run-001/predictions-commit.json` and `evaluation-001/report.json`.
The report includes every case, top-10 results, tie-expanded areas, boundary
diagnostics and paired base-group summaries. Generation, preparation, scoring
and evaluation ran headlessly under a 20% CPU cap; no GUI was opened.
