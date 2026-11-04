# Audio-derived timing, tone and candidate refinement

The original frozen pipeline run completes successfully, but its scientific outcome
is **coverage incomplete**: the winning second-event tone reaches the lower
audio-derived frequency bound. The new grids recover the same leading aircraft
pair, but their small distance to a reported reference is not verified accuracy.

GeoSonnar now derives recording-relative timing and emitted-frequency search
ranges from the reviewed acoustic measurements. The regional search no longer
needs the manual traces' closest-approach anchors, tone grids, speed/distance
filters, separation or timing sweep. Refinement starts from the **new regional
candidates**, with a separate local and fine grid for every retained flight pair.

Date, absolute recording-start bounds, region, historical flight coverage,
terrain and sound speed remain supplied assumptions. The two event choices are
still source-informed acoustic review. This is not blind worldwide localization.

## Run the stages

The frozen experiment writes a complete example audio request and geometry file.
To repeat the same explicitly labelled agent selection, copy its request and use
fresh output folders:

```powershell
Copy-Item -LiteralPath output/benchmarks/audio-priors/pipeline-001/audio-config.json -Destination cases/audio-main-pair-new.json
.\dist\geosonnar.exe derive-audio-priors --config cases/audio-main-pair-new.json --out output/audio-main-priors-new
.\dist\geosonnar.exe prepare-audio-search --priors output/audio-main-priors-new/priors.json --geometry output/benchmarks/audio-priors/pipeline-001/geometry.json --out output/audio-main-search-new
.\dist\geosonnar.exe rank --input output/audio-main-search-new/search_input.json --out output/audio-main-search-new/results.json
```

For your own review, regenerate a schema-2 review page as described in
[REVIEWED_SEARCH.md](REVIEWED_SEARCH.md), save its decisions and change the copied
request's `review_selection`, `selected_ridges` and `selection_basis`. The request
selects one existing ridge from each of two distinct kept groups. It supplies
**no emitted-frequency grid or timing anchor**. Default fitting settings and
engineering padding are written into `priors.json`; explicit overrides require
a fresh analysis and remain recorded in the request.

`derive-audio-priors` writes `priors.json` and a companion `receipt.json` binding
the exact report and original recording/review/discovery dependencies. Exit 0
means the acoustic estimates support the declared search; exit 3 means
`inconclusive_audio_priors`. An inconclusive report retains measurements and
diagnostics but has no derived search ranges or frequency grid. It cannot be
passed to `prepare-audio-search`. Exit 2 means an input or operational error.

Geometry is a separate strict schema containing normalized-track and observer
paths, observer bounds/grid/height, clock-offset bounds, the propagation model
and provenance. It has no fields for old event files, acoustic anchors, coarse
filters or a manual search template. Config-relative paths resolve beside their
config. The sound speed must match the acoustic fit. Source copies may be
relocated through `source_override` only when their SHA-256 remains identical.

Refine candidates from that new regional result:

```powershell
python analysis/refine_automatic.py --input output/audio-main-search-new/search_input.json --coarse-results output/audio-main-search-new/results.json --exe dist/geosonnar.exe --out-dir output/audio-main-refinement-new --dem data/dopplergeist/terrain/Copernicus_DSM_COG_10_N40_00_W074_00_DEM.tif --geoid data/dopplergeist/terrain/us_nga_egm96_15.tif
```

Acoustic fitting, event transfer, propagation and ranking run in Rust. Terrain
sampling and refinement orchestration currently use the existing Python
GeoTIFF adapter. Native terrain import and the Windows GUI remain separate work.

## Acoustic model and safeguards

The fitter uses the exact propagation delay for a straight, constant-speed
source with one stable emitted tone. It optimizes emission-CPA time and a
logarithmic passage width; for each proposal it solves the two linear frequency
coefficients subject to the declared physical domain. It converts them to speed,
slant distance, emitted frequency and **heard CPA**: the reception time of sound
emitted at geometric closest approach. Heard CPA is not the observed frequency
curve's inflection time. Absolute UTC cannot be inferred from this shape.

Default domains are speed 10–280 m/s, slant distance 100–30,000 m, numerical width
0.5–1,000 s and emission-CPA center extending one measured duration before and
after the measured interval. Twelve deterministic optimization starts reduce
dependence on initialization. No flight path, expected aircraft, reported
location, manual frequency or manual CPA enters the fitting stage.

The fitter rejects insufficient motion/support, boundary solutions, excessive
residuals and excessively broad sensitivity. It checks measured support on both
sides of heard CPA, samples near-optimal models on a broad and a local profile
grid, and refits after omitting each of four contiguous time quarters. The union
of those parameter ranges is **engineering sensitivity, not a confidence
interval, exhaustive bound or guaranteed coverage**. Parameters are correlated;
good frequency residuals do not establish accurate speed or distance.

The fixed additional search allowances are 2 Hz in emitted frequency, 2.5 s in
heard CPA, 20% of fitted speed, and the larger of 35% of fitted slant distance or
500 m. Speed/distance bounds remain inside the declared physical domain, with
clipping recorded. Frequency and shared-shift grids use 0.5 Hz/s steps and retain
the exact fitted center and both endpoints. Pair separation and its tolerance
cover both padded timing intervals. These allowances are explicit modeling
choices, not learned or statistically calibrated uncertainty.

All 320 selected measurements and their 8 s/11 s maximum gaps remain unchanged.
No harmonic rescaling, clipping, gap filling or manual fallback occurs. The
original contrast weights and quality estimates are retained; fitting and the
matcher use equal sample weights. Other harmonics remain untested by this pair
score. The faint third event is still missed by discovery and needs manual work.

## Automatic refinement policy

The new regional ranking supplies up to three distinct ordered flight pairs.
Before constructing a grid, the refiner replays each seed with the native matcher
at its exact observer and retained tracks. It checks that the result is compatible
with the declared input and executable. This does not establish that the original
regional search or flight archive was exhaustive.

Each retained pair gets a terrain-sampled local grid of 600 m radius at 100 m spacing,
then its own fine grid of 150 m radius at 25 m spacing around **its local winner**.
The old manual pair, old fine grid and reference coordinates are not inputs.
Same-aircraft return passes retain one copy of their continuous track. Alternative
pairs remain in the report; a poorer pair is not silently removed from coverage
diagnostics.

A winning observer at a grid edge, or a winning clock, tone or coarse parameter
at its search edge, is reported as incomplete coverage. No outcome-dependent
parameter widening occurs. Even an interior sampled optimum remains conditional
on the retained pairs and finite footprints, not proof of global uniqueness.

## Validation and interpretation

`audio_priors_contract.json` pins the source data, selection, region/date/terrain,
padding and refinement policy before the first new fit. The benchmark snapshots
the native executable and source, verifies exact measurement transfer and all
derived grids, then waits for the new regional and refined predictions before
opening prior winners and coordinate references. Separation is procedural on
one filesystem, not a sealed holdout.

Independent checks invert arrival time by bisection in the emission domain,
rather than reusing the fitter's reduced closed form. They reproduce the fit
RMSE and heard CPA and recompute every padded range, emitted-tone grid, timing
sweep and separation tolerance. Eleven evaluator controls exercise wrong CPA,
invented residuals, unsafe fits, false UTC claims, changed source/review/uncertainty,
manual-prior substitution, lost grid endpoints and refinement seed substitution.

Reproduce the complete experiment:

```powershell
python -m unittest discover -s benchmarks -p test_audio_priors_harness.py -v
python analysis/benchmark_audio_priors.py --exe dist/geosonnar.exe --output output/benchmarks/audio-priors/new-pipeline
```

The benchmark's pass flag certifies software integrity. A weak fit, no candidate
or incomplete refinement remains a distinct scientific outcome. No location
threshold is adjusted to make the experiment pass. `location_verified` remains
false throughout; neither the video estimate nor the rounded platform coordinate
is surveyed microphone truth.

## Executed audio estimates

The production audio fit took 0.92 s on this machine. Both selected ridges pass
the declared acoustic usability checks without reaching a fit-domain boundary.

| Ridge | Heard CPA in recording | Emitted frequency | Fitted speed | Fitted slant | Frequency RMSE |
| --- | --- | --- | --- | --- | --- |
| Main 1, 201 points | 2,155.839 s | 112.741 Hz | 140.24 m/s | 6,382 m | 0.165 Hz |
| Main 2, 119 points | 3,076.494 s | 129.406 Hz | 112.74 m/s | 3,651 m | 0.683 Hz |

The second event is less stable under time-block omission: emitted-frequency
sensitivity spans 128.23–136.13 Hz, speed 93.81–165.04 m/s and slant distance
2,306–7,467 m before padding. Its low residual must not be read as precise physical
geometry. The first event has narrower empirical sensitivity.

Final search/refinement results are recorded in
`output/benchmarks/audio-priors/pipeline-001/report.json`.

## Executed search and refinement

The full frozen experiment completed in **135.85 s** with executable SHA-256
`976dfab47845575069186e1740072520f86c3bf00617df9b196df7c40d37ede0`.
All software-integrity gates passed, including the independent physical
reconstruction of both audio fits, exact preservation of the 320 samples and
their gaps, unchanged regional candidate data, independently recomputed padded
ranges and grids, native seed replay, and stable source/executable/input hashes.
Other validation work ran concurrently; runtime is descriptive for this machine.

The regional search retained all **6,661 continuous tracks and 2,856 observers**,
with 1,316 ranked hypotheses across 10 ordered flight pairs. It recovered the
same leading pair, `a1dfeb#part1` / `ace391#part3`. Its new 200 m grid winner is
102.42 m from the video estimate. Three newly ranked pairs received their own
local and fine terrain grids; seven other regional pairs were not refined.

The leading new fine result is **40.8765748793, -73.5247079496**, 34.82 m from the
manual fine winner, 23.78 m from the video estimate and 2.87 m from the rounded
platform reference. Both location and shared-clock winners are interior, but the
second emitted tone is exactly **126.229785 Hz**, its lower search bound, in both
the local and fine runs. Therefore this is an unresolved search/model limit,
**not a closed optimum or a 2.9 m accuracy demonstration**. The video estimate
and platform coordinate are different, nonsurveyed references.

| Retained new pair | Fine-grid combined RMSE | Coverage finding |
| --- | --- | --- |
| `a1dfeb#part1` / `ace391#part3` | 0.603 Hz | Second tone at its lower bound |
| `ab1162#part0` / `ab050a#part5` | 2.868 Hz | Interior sampled optimum |
| `abb40e#part3` / `ac3d8b#part0` | 8.079 Hz | Observer grid and first-tone boundaries |

The second event's broad omission sensitivity and the downstream frequency
boundary are consistent reasons to investigate model sensitivity and coverage.
No parameter was widened, shifted, clipped or selected after seeing the location.
The frozen run and its incomplete status remain unchanged. A smaller RMSE or
reference distance does not establish that a partial ridge is more accurate.

[Audio fit and omission diagnostics](../output/benchmarks/audio-priors/pipeline-001/audio-fit-diagnostics.png)
and [new-candidate refinement comparison](../output/benchmarks/audio-priors/pipeline-001/automatic-refinement-comparison.png)
show the observations and unresolved limit. Their generator and source/output
hashes are beside the report; both exported figures were visually inspected.

The subsequent [bounded frequency/spatial sensitivity experiment](SEARCH_SENSITIVITY.md)
has now run all three predeclared expansions. Initial widening moves the leading
estimate 70.71 m and removes its frequency boundary; the later expansions leave
its location, tones, clock and RMSE exactly unchanged. This establishes numerical
stability under those tested expansions. Alternative-pair coverage and model
assumptions remain unresolved, and the original frozen report above retains its
incomplete status. Finer-resolution convergence and separate timing/physical-model
sensitivity remain work before making the automated workflow the GUI default.
