# Bounded frequency and spatial sensitivity

This experiment tests whether the original audio-derived regional estimate
stabilizes when its frequency and spatial limits widen. It uses the existing
source recording and regional aircraft archive. The newer private clip has no
location estimate and is not an input.

The policy is fixed in `search_sensitivity_contract.json` before running the
expanded searches. All stages run, including when an earlier result appears
stable. The original `audio-priors/pipeline-001` artifacts remain unchanged.

## Fixed schedule

Frequency allowances are added symmetrically to both original emitted-tone
ranges. Every earlier frequency value, including exact interval endpoints,
remains eligible; additional frequencies use the existing 0.5 Hz spacing.

| Stage | Additional allowance at each frequency end | Regional outer rings | Local radius / spacing | Fine radius / spacing |
| --- | --- | --- | --- | --- |
| Original | 0 Hz | 0 | 600 / 100 m | 150 / 25 m |
| Widen 1 | 2 Hz | 3, about 0.6 km per side | 1,200 / 100 m | 300 / 25 m |
| Widen 2 | 4 Hz | 6, about 1.2 km per side | 1,800 / 100 m | 450 / 25 m |
| Widen 3 | 8 Hz | 12, about 2.4 km per side | 2,400 / 100 m | 600 / 25 m |

The expanded regional lattices contain 3,534, 4,284 and 6,000 observers. They
preserve the original 2,856 observers exactly. Added points use the same terrain,
geoid and 3 m microphone-height policy. All 6,661 supplied tracks are ranked
again at every stage; expanding the observer region does not download or prove
complete aircraft coverage for the added area.

Each stage retains its new top three distinct ordered flight pairs and every
previously retained pair. Each pair gets its own local and fine search. Previous
per-pair observer points remain in the search when grids recenter, including the
original local and fine points. This prevents an apparent improvement or shift
caused by discarding previously sampled locations. Boundary checks account for
the separate grid footprints instead of treating gaps in their union as sampled
interior coverage. Fine-grid closure specifically requires a surrounding
footprint at 25 m spacing or finer; coarse coverage cannot hide a fine-grid edge.

All 320 measured points and their gaps, aircraft tracks, clock bounds and shift
grid, separation tolerance, coarse speed/slant filters, sound speed and native
executable remain fixed. The new outputs are explicit sensitivity inputs; they
do not rewrite the original acoustic estimates or claim that widened ranges
were inferred from the audio.

## Stability rule

Both final successive transitions must retain the same leading ordered pair
and meet every declared threshold:

- Horizontal location change at most 50 m.
- Each emitted frequency changes by at most 0.5 Hz.
- Absolute clock offset changes by at most 1 s and shared shift by at most 0.5 s.
- Absolute combined frequency-RMSE change at most 0.05 Hz.
- The leading estimate does not reach any tested observer, frequency, clock or
  coarse-filter boundary.

These are engineering tolerances fixed before the expanded predictions, not
confidence intervals or accuracy thresholds. Boundary problems for alternative
pairs remain separate findings even if the leading estimate settles. The
frequency and spatial limits widen together, so this experiment does not
isolate their individual contributions. Fixed timing, trajectory, acoustic and
atmospheric assumptions can still bias an apparently stable location.

## Reproduce

```powershell
python -m unittest discover -s benchmarks -p test_search_sensitivity.py -v
python analysis/benchmark_search_sensitivity.py --contract benchmarks/search_sensitivity_contract.json --output output/benchmarks/search-sensitivity/new-run
```

Use a fresh output directory. The runner records the contract, input and source
hashes, native commands, all regional results, retained alternatives, expanded
grids and stability comparisons. Evaluation coordinate files are not inputs.
Scientific stability or incomplete coverage remains separate from software
integrity; `location_verified` remains false.

## Executed result

`output/benchmarks/search-sensitivity/widening-001/report.json` completed all
three prescribed stages in **1,711.51 seconds (28.53 minutes)**. Software
integrity passed. The leading estimate satisfies the declared numerical
stability rule, while overall alternative-pair coverage remains incomplete.

| Stage | Regional observers / ordered pairs | Movement from preceding estimate | Leading emitted tones | Leading RMSE |
| --- | --- | --- | --- | --- |
| Original | 2,856 / 10 | — | 113.0 / 126.229785 Hz | 0.603092 Hz |
| Widen 1 | 3,534 / 13 | 70.71 m | 113.5 / 125.5 Hz | 0.569228 Hz |
| Widen 2 | 4,284 / 16 | 0.00 m | 113.5 / 125.5 Hz | 0.569228 Hz |
| Widen 3 | 6,000 / 21 | 0.00 m | 113.5 / 125.5 Hz | 0.569228 Hz |

All expanded stages select **40.87612463783388, -73.52530112819616**, with the
same leading ordered pair, `a1dfeb#part1 / ace391#part3`. The initial widening
moves the prediction 70.71091489 m southwest of the frozen baseline and removes
its second-tone boundary. Subsequent widening leaves the selected observer,
both frequencies, clock offset (73,049.928548 s), shared shift (-0.5 s) and
combined RMSE exactly unchanged. The winning observer is interior at the
declared 25 m refinement resolution and does not touch the tested frequency,
clock or coarse-filter limits. Reported coordinate digits identify a sampled
prediction; they do not describe its accuracy.

The alternatives matter: a new pair enters the retained set at Widen 2 and
improves substantially in Widen 3. Prior pairs remain in the experiment.

| Final refined ordered pair | Combined RMSE | Final fine-search finding |
| --- | --- | --- |
| `a1dfeb#part1 / ace391#part3` | 0.569228 Hz | Interior; unchanged across all expanded stages |
| `c013c3#part2 / a820a4#part3` | 1.759586 Hz | New runner-up; improved from 4.016247 Hz in Widen 2; fine result interior |
| `ab1162#part0 / ab050a#part5` | 2.867626 Hz | Interior; unchanged |
| `abb40e#part3 / ac3d8b#part0` | 7.846333 Hz | Shared-shift boundary remains unresolved |

The final lead over the runner-up is 1.190358 Hz. This gap is not a probability
or confidence level. The new runner-up's local-search boundary clears in its
fine search; the earlier local diagnostic remains recorded. Seventeen of the
21 final regional pairs did not receive fine refinement. The expanded region
uses the same aircraft archive, and fixed clock, separation, coarse-filter and
physical-model assumptions remain possible sources of bias. No evaluation
coordinate file was read, no surveyed error was measured, and
`location_verified` remains false. The original boundary-limited experiment is
unchanged.

All **15 independent fabricated-data controls passed**. A separate audit checked
all **111 frozen hashes**, exact preservation of the 320 samples and all tracks,
nested frequency/observer domains, cumulative pair retention, score accounting,
resolution-aware boundaries and both zero-change final transitions. Native
Rust source and executable were unchanged; the new code orchestrates and audits
the existing native matcher.

- [Full machine-readable report](../output/benchmarks/search-sensitivity/widening-001/report.json)
- [Independent audit](../output/benchmarks/search-sensitivity/widening-001/independent-audit.json)
- [Control-test receipt](../output/benchmarks/search-sensitivity/widening-001/harness-controls.json)
- [Comparison figure](../output/benchmarks/search-sensitivity/widening-001/figures/search-sensitivity.png),
  [PDF](../output/benchmarks/search-sensitivity/widening-001/figures/search-sensitivity.pdf)
  and [comparison data](../output/benchmarks/search-sensitivity/widening-001/figures/comparison.csv).

The next useful check is a predeclared resolution-convergence experiment with
finer emitted-frequency and observer sampling, retaining these alternatives.
Clock, sound-speed and aircraft-height sensitivity then remain separate checks
before making reliability claims or adopting an automatic GUI decision rule.

## Fresh native ADSB.lol rerun

`output/benchmarks/search-sensitivity/native-001/` repeats the experiment with
the newly downloaded, natively imported ADSB.lol archive from
`output/native-data/replay-002/`. It uses **1,008 detailed track segments from
428 regional aircraft**, the same 320 measured acoustic samples and the native
terrain baseline. Its initial regional ranking contains **95 ordered pairs**.
The fresh top three pairs receive new 600 m / 100 m local and 150 m / 25 m fine
grids. No pair identifier or fine observer grid is inherited from the earlier
experiment. The new contract is `native_search_sensitivity_contract.json`;
the earlier contract and results remain unchanged.

The unchanged harness runs from an immutable snapshot of its Python modules,
Rust sources, Cargo files and tested executable. All 29 copied code files match
the validation-006 source ledger, and the executable retains SHA-256
`7fe619acecb108b8c809177fb4ae6ed5af6a9d1dcead87fe6c373de8e79adc7b`.
Existing immutability checks therefore cover copied sources throughout the
experiment while subsequent application development can continue independently.
The per-command timeout was set to 3,600 seconds before expanded predictions.
Added terrain points still use the pinned Python GeoTIFF adapter; this rerun
does not claim that refinement orchestration or every terrain sample is native.

All three prescribed widening stages completed in **1,465.36 seconds
(24.42 minutes)**. The leading estimate passes the same declared stability
rule. Alternative-pair coverage remains incomplete.

| Stage | Regional observers / ordered pairs | Retained pairs | Leading RMSE | Movement from preceding estimate |
| --- | --- | --- | --- | --- |
| Fresh baseline | 2,856 / 95 | 3 | 0.603092 Hz | — |
| Widen 1 | 3,534 / 111 | 3 | 0.569228 Hz | 70.71 m |
| Widen 2 | 4,284 / 120 | 4 | 0.569228 Hz | 0.00 m |
| Widen 3 | 6,000 / 151 | 4 | 0.569228 Hz | 0.00 m |

The leading pair is `a1dfeb#part1 / ace391#part0`. All expanded stages select
the sampled observer **40.87612463783388, -73.52530112819616**, emitted tones
113.5 / 125.5 Hz, recording clock offset 73,049.928548 seconds and shared shift
−0.5 seconds. Both final successive transitions are exactly unchanged in these
values and RMSE. The winning point is interior at 25 m refinement spacing and
does not reach the tested frequency, clock or coarse-filter limits. These
digits identify a sampled prediction, not surveyed precision.

| Final retained ordered pair | Combined RMSE | Final fine-search finding |
| --- | --- | --- |
| `a1dfeb#part1 / ace391#part0` | 0.569228 Hz | Interior; unchanged across expanded stages |
| `a1dfeb#part1 / 406832#part0` | 1.404925 Hz | Interior |
| `ac26cf#part0 / a25cd3#part0` | 2.078189 Hz | Shared-shift boundary remains unresolved |
| `a66ce0#part0 / acc040#part0` | 2.548093 Hz | Interior |

The final lead over the refined runner-up is **0.835697 Hz**, not a probability.
**147 of 151 final regional pairs were not finely refined.** The new fourth
pair entered at Widen 2 and retained its clock-shift limitation after widening
cleared its frequency edge. All earlier pairs and their sampled observer points
remained eligible. Fixed timing, acoustic, atmospheric and height assumptions,
the source-informed two-hour interval and region, and incomplete receiver
coverage still limit interpretation. Numerical stability does not establish
location accuracy or global uniqueness; `location_verified` remains false.

The 15 fabricated-data controls passed. A separately reviewed and executed
audit verified **131 frozen hashes**, all 1,008 unchanged tracks and 320 samples,
the exact declared grids and pair-specific centers, cumulative pair retention,
frequency and observer nesting, score accounting, boundaries and both stability
transitions. Independent GeoTIFF sampling checked **26,982 unique locations**,
with maximum height discrepancy **3.55 × 10⁻¹⁴ m** under the same explicit
approximate vertical-datum policy. No private case or evaluation-coordinate
file was an input.

- [Fresh baseline](../output/benchmarks/search-sensitivity/native-001/baseline/report.json)
- [Expanded results](../output/benchmarks/search-sensitivity/native-001/widening/report.json)
- [Independent audit](../output/benchmarks/search-sensitivity/native-001/independent-audit.json)
- [Source binding](../output/benchmarks/search-sensitivity/native-001/source-binding.json)
- [Comparison figure](../output/benchmarks/search-sensitivity/native-001/figures/search-sensitivity.png),
  [PDF](../output/benchmarks/search-sensitivity/native-001/figures/search-sensitivity.pdf)
  and [comparison data](../output/benchmarks/search-sensitivity/native-001/figures/comparison.csv)

The unmodified snapshot can rerun widening into another fresh output directory:

```powershell
python output/benchmarks/search-sensitivity/native-001/snapshot/analysis/benchmark_search_sensitivity.py --contract benchmarks/native_search_sensitivity_contract.json --output output/benchmarks/search-sensitivity/native-rerun
```

The contract records absolute paths to this workspace's frozen baseline and
inputs. Reproduction in another workspace requires a newly pinned contract;
changing its paths or hashes is an explicit new experiment, not a silent reuse.
