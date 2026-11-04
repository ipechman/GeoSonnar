# Doppler/thunder scoring ablation

Run `hybrid-ablation-001` completed headlessly on 2026-11-04 UTC. **The current combined score did not improve the primary synthetic accuracy result over Doppler-only.** It helped in a separate single-aircraft geometry challenge, but did not reliably overcome wrong associations or clocks. No atmospheric features or scoring weights were changed.

## Primary result

These are the same 24 held-out nominal/noisy scenes, with two aircraft, three lightning arrivals and a shared 250 m grid. Every mode returned a rankable winner for every primary scene.

| Mode | Mean error | Median error | Within 250 m | Within 500 m |
| --- | ---: | ---: | ---: | ---: |
| Doppler-only | 107.81 m | 111.57 m | 24/24 | 24/24 |
| Thunder-only | 661.89 m | 531.75 m | 4/24 | 11/24 |
| Combined | 115.92 m | 112.43 m | 23/24 | 24/24 |

Combined selected exactly the same cell as Doppler-only in 22 scenes, worsened two and improved none. Its mean error difference was **+8.11 m**, with a paired scene-bootstrap 95% interval of **[0.00, 21.46] m**. The difference of marginal medians was **+0.86 m**, interval **[0.00, 25.51] m**. Positive differences favor Doppler-only. This establishes neither improvement nor a statistically decisive general deterioration.

The predeclared improvement gate required complete paired coverage, upper bounds below zero for both differences, and no decline in all-case 500 m success. **It did not pass.** Combined clearly outperformed thunder-only in this synthetic primary set, but adding thunder did not improve the already strong Doppler baseline. The 250 m grid and known aircraft/clock/tone assumptions limit sensitivity to small improvements; this run does not establish continuous-space accuracy.

The nearest available grid cells already average 100.01 m from truth. Doppler selected those cells in 20/24 scenes and combined in 19/24, leaving limited room for improvement at this resolution. A finer-grid comparison would need its own frozen experiment and fresh holdouts.

## Separate stress results

The following strata were declared before scoring and are not pooled into the primary claim.

| Held-out stratum | Cases | Doppler median | Thunder median | Combined median | Within 500 m: Doppler / thunder / combined |
| --- | ---: | ---: | ---: | ---: | --- |
| Single-aircraft geometry challenge | 12 | 414.45 m | 426.54 m | 297.99 m | 7/12 / 8/12 / 12/12 |
| Sound-speed/source-position mismatch | 12 | 149.37 m | 541.40 m | 134.05 m | 12/12 / 5/12 / 12/12 |

In the single-aircraft stratum, combined improved five scenes, tied six and worsened one relative to Doppler-only. It corrected all four Doppler errors exceeding 1 km. The paired mean difference interval excludes zero, but the median-difference interval reaches zero; this is descriptive evidence of benefit for some weak geometries, not the primary success criterion or a field-accuracy claim. The protocol's internal name `ambiguous` denotes this geometry challenge; it does not guarantee every scene is ambiguous.

Mismatch used a true uniform sound speed of 335 or 351 m/s while scoring at 343 m/s, plus a 500 m displacement of each actual thunder source from its catalog position. Combined and Doppler chose the same cell in 11 of 12 scenes. This limited stress does not validate atmospheric profiles, wind, refraction, storm-channel extent or terrain propagation.

## Misleading alternatives and missing evidence

There are **32 correlated control variants of four nominal holdouts**, not 32 independent new scenes. Wrong-aircraft and wrong-strike controls deliberately select translated alternative sources. The scorer does not search for associations; every mode receives those same supplied associations and candidate coordinates.

| Control | Doppler outcome | Thunder outcome | Combined outcome |
| --- | --- | --- | --- |
| Wrong aircraft | Median 2.56 km; 0/4 within 500 m | Median 350 m; 2/4 within 500 m | Median 2.50 km; 0/4 within 500 m |
| Wrong strikes | Median 140 m; 4/4 within 500 m | Median 2.61 km; 0/4 within 500 m | Median 157 m; 4/4 within 500 m |
| Clock shifted 20 seconds | Median 2.42 km; 0/4 within 500 m | Median 4.03 km; 0/4 within 500 m | Median 2.42 km; 0/4 within 500 m |
| Date shifted one day | No rankable result, 4/4 | Median 4.47 km; 0/4 within 500 m | No rankable result, 4/4 |
| True location outside region | Median 4.65 km; 0/4 within 500 m | Median 4.82 km; 0/4 within 500 m | Median 4.65 km; 0/4 within 500 m |
| Lightning absent | 4/4 within 500 m | No rankable result, 4/4 | No rankable result, 4/4 |
| A required track absent | No rankable result, 4/4 | 2/4 within 500 m | No rankable result, 4/4 |
| Events unreviewed | No rankable result, 4/4 | No rankable result, 4/4 | No rankable result, 4/4 |

Missing evidence was never silently dropped from combined scoring. Conversely, ranking a candidate does not mean the input association, date or region is correct. Wrong-aircraft Doppler and wrong-strike thunder each produced four winners more than 1 km wrong despite scores at or below the predeclared diagnostic threshold of 1.0. Five primary thunder-only winners also met that diagnostic. **Scores are uncalibrated residuals, not confidence.** All outputs remained `location_verified: false`; no false-confidence rate or automatic acceptance accuracy is claimed.

## What was tested

- Production `workspace_scoring::score_observers`, the backend used by the manual GUI comparison, through a small native adapter. Only the three term-enable flags vary. Candidate order, observations, associations, emitted tones, clock, heights, uncertainties, scales and weights remain identical.
- Doppler-only includes frequency and closest-approach arrival timing. Thunder-only uses reviewed arrival times with fixed lightning IDs. Combined assigns weight 1 to each of these three terms: two Doppler terms and one thunder term, not equal modality weighting. Samples are averaged within events before the score averages terms.
- Twelve development scenes, 48 independent synthetic holdouts and 32 correlated controls. The primary set is the 24 nominal/noisy holdouts. No seeds, weights, thresholds, scenarios or bounds were tuned after predictions. Development results were not used to modify this run.
- An independent Python WGS84/ECEF forward oracle generated retarded arrival times and frequencies. It never calls production Rust physics. Truth is off-grid. All modes search the same 625 explicit receivers across a 6 km region, with a known receiver height.
- Inputs, labels, protocol and source hashes were frozen before building/scoring. The native adapter receives no evaluation labels. All predictions were committed before evaluation opened truth; input/binary/source identities, unchanged shared terms, candidate coverage and ranking arithmetic were checked first. This is procedural synthetic separation, not an independently administered blind field trial.
- Errors are horizontal great-circle distances to truth. All-case success denominators retain missing/error outcomes as failures. Bootstrap summaries resample independent base scenes; correlated controls receive descriptive summaries only.

The 22 Python harness checks, three native adapter tests and warning-free adapter Clippy check passed. The frozen experiment completed all 276 mode attempts; 240 returned rankable winners and 36 had no rankable result or an explicit unavailable error. An independent recount from raw scores and truth matched every rankable winner and all distances within 0.000001 m. No GUI was launched, no recordings were modified and no network data was requested. Existing application release binaries were unchanged.

This tests supplied reviewed measurements and associations. It does **not** test raw-audio detection, manual tracing quality, emitted-frequency fitting, automatic association discovery, unknown-clock recovery, worldwide search, lightning-envelope cross-correlation or provider coverage.

## Real-world readiness and next step

No available real recording currently combines an independently verified microphone location and sample-zero clock, reviewed Doppler and thunder measurements, and matching aircraft/lightning coverage. The reference location is a source-informed acoustic estimate; the private clip has no independently verified receiver coordinate. Neither can currently support a defensible real-world hybrid accuracy comparison. These synthetic results do not establish that combining evidence improves real-world accuracy.

**Keep atmospheric complexity deferred.** The next validation priority is an independently located and timestamped storm recording with matched tracks and lightning data, with its receiver truth withheld from scoring and the same three modes compared. Before trusting combined ranking, show the separate modality locations and require review when they disagree or when a winner lies at the search boundary. Any future rejection threshold, weighting change or grid-refinement policy should be developed separately and tested on fresh holdouts; do not tune to this saved test set.

## Reproduce

Use a new destination from the project root:

```powershell
python benchmarks/test_hybrid_ablation.py -v
cargo test --offline --locked --example hybrid_ablation
python analysis/benchmark_hybrid_ablation.py --output output/benchmarks/hybrid-ablation-new
```

Python uses its standard library. Rust dependencies must already be available for the locked offline build. The runner builds only the headless native example, refuses existing output directories and preserves failures. `--scorer PATH` can use an existing adapter executable; its compiled source hashes must match the checkout.

The checked-in [protocol](hybrid_ablation_protocol.json), [aggregate results](hybrid_ablation_results.json), [generator/evaluator](../analysis/benchmark_hybrid_ablation.py), [native adapter](../examples/hybrid_ablation.rs) and [harness controls](test_hybrid_ablation.py) are included by the source exporter. Full local inputs, truth, predictions, hashes, logs, validation receipt and exact scorer executable are preserved in `output/benchmarks/hybrid-ablation-001/`. Generated artifacts and private data remain excluded from source uploads. The existing 0.2.0 release packages retain their original snapshots; this benchmark does not repackage the GUI.
