# Analyst-guided regional search

The regional workflow searches explicit uncertainty in **one shared recording clock**, reviewed aircraft/strike alternatives and emitted aircraft tones. It keeps the original observations unchanged and returns competing location/time/source hypotheses for inspection. It never identifies a sound source, confirms an event or accepts a location automatically.

## Operator flow

1. In **Audio review**, inspect measured audio, edit traces/arrival intervals, confirm the events you intend to use and select them for scoring. Confirmation is the analyst's interpretation, not independent identification of an aircraft or strike.
2. Import and verify the required trajectories and lightning in **Data**. Set their UTC origins, interpolation-gap policies and vertical references. Missing historical lightning access can leave thunder unavailable; it must not be replaced by invented strikes.
3. In **Search**, configure the regional workflow from left to right: region/time, evidence and alternatives, then estimate/run. Declare an approximate recording-start UTC, a minimum/maximum clock adjustment and step. Adjustments are absolute relative to that UTC reference, replacing the fixed comparison's clock adjustment rather than adding another offset.
4. Choose the evidence terms and use **Refresh event list**. Select which sources may be considered for each event. Newly listed sources start unapproved. Filtering the list or inspecting a map source does not approve it. The explicit include/clear actions change the approved list, which is saved in the review.
5. For frequency evidence, supply a positive emitted-tone interval. Equal endpoints mean a fixed tone. Search analytically fits the best constant emitted tone within that interval for each aircraft/receiver/time combination. Timing-only and thunder events have no tone fit.
6. **Estimate regional search** reports the amount of work before execution. Optionally select independent Doppler-only, thunder-only and combined runs; their aggregate work must fit a separate explicit total budget. Review the estimate and then **Run reviewed regional plan**. The background operation reports progress and can be cancelled without opening another application. Cancellation preserves a clearly incomplete result containing fully evaluated receiver/time cells only, and requested modes that were not started remain explicitly unstarted.
7. In **Results**, sort the retained hypotheses and inspect their coordinates, inferred recording-start UTC, shared clock adjustment, source assignments, fitted tones and boundary flags. Geographic summaries separately retain the best supported assignment at **every declared receiver**, even when many source combinations at one location crowd the global top-K list. Inspecting a hypothesis or receiver does not apply it. Inspection uses a temporary review to reproduce the full fixed-score residuals and predicted evidence overlays for a selected complete hypothesis or receiver's best assignment.
8. Explicitly apply a selected complete hypothesis only when you want its associations, emitted tones and clock to become the review's fixed assumptions. This is one undoable edit. Measured points and event review decisions are unchanged. Applying a specific independent mode also explicitly switches the selected scoring-term flags to that mode; it does not silently claim a combined result. A changed review or changed loaded inputs invalidates application of an older result.
9. Use **Prepare refinement plan** to narrow the geographic/time ranges around a chosen hypothesis, with explicitly selected spacing and time step. This edits the next plan; it does not run or accept the hypothesis automatically. Other source alternatives remain available unless the analyst changes them. Review competing coarse hypotheses before narrowing around only one.
10. Save the investigation dossier when a result is worth retaining. It contains a self-contained HTML explanation, retained rows in CSV, the original review and full stored regional report, and file hashes. Cancelled searches can be exported with prominent partial-work labels.

## What is searched

Before fixed scoring, regional work estimation or prediction inspection, each confirmed, selected event whose modality is enabled needs a separate **acoustic source assessment**. Confirming measured points or an arrival does not establish its sound class. New manual, detected and imported events start **Unassessed**. Choose **Compatible** when the sound plausibly belongs to the selected aircraft/thunder class, or explicitly permit an **Uncertain source hypothesis** with a written rationale. Compatible is not an aircraft/strike identification. Unassessed, incompatible and blank-rationale uncertain events block execution with named reasons; none is silently removed from the score. Disabled modalities and unselected events do not block other evidence.

Old saved reviews load with their selections intact and missing assessments remain unassessed. The default field is omitted on serialization, preserving the previous canonical document identity when otherwise unchanged. No detector or import migrates old events to compatible. An unfinished uncertainty rationale can be saved as a draft (up to 4000 UTF-8 bytes), but cannot be scored. Changing an assessment changes the review identity and invalidates earlier results for application. Applying a source/time hypothesis never changes the acoustic assessment.

Uncertain-source rationales appear in result limitations, inspected prediction assumptions and the investigation dossier before the ranked alternatives. Allowing an uncertain interpretation changes no score weight or physical prediction; it records the conditional assumption. For thunder, the legacy internal `reviewed_as_thunder` flag authorizes evaluating the reviewed sound as thunder, including an explicitly uncertain hypothesis; it does not assert independent source identification. Native workspace entry points enforce this gate. Legacy generic physics/event-trace inputs have separate contracts and do not acquire a source assessment merely by being exported.

Every receiver is evaluated at every declared clock sample. Both time endpoints are included, even when the final interval is shorter than the step. All selected events share this same clock and receiver. Each event evaluates every approved source; no extra source is inferred or silently added.

Aircraft predictions use the existing continuous, gap-bounded propagation model. A frequency sample outside unique supported reception-time coverage remains unavailable; observations are not shortened to improve a fit. Doppler timing uses one supported, non-boundary geometric closest approach inside the reviewed interval. Thunder uses the selected point-strike timing model and its explicit height assumptions. An optional atmospheric profile affects both propagation paths without a fixed-speed fallback outside its coverage.

The frequency objective is quadratic. If `g_i` is the predicted Doppler factor, `y_i` the measured frequency and `s_i` its effective residual scale, the bounded emitted tone is:

```text
f0 = clamp(sum(g_i*y_i/s_i^2) / sum(g_i^2/s_i^2), lower, upper)
```

The engine reuses the production scorer's factors and residual scales. It averages samples within events, events within each term, and then the selected term weights. It does not fit a separate clock for each event or drop an unavailable selected event. Replaying an applied hypothesis through the fixed scorer must reproduce the same score.

Source choices factor under this score: after evaluating the individual alternatives, a best-first combination search returns the exact best retained joint assignments for each sampled receiver/time. It does not need to evaluate every Cartesian assignment with repeated physics. There is **no one-to-one source constraint**: two reviewed events can refer to the same aircraft or strike. Correlated harmonics and multiple arrivals are not independent confirmations, and their inclusion remains an analyst decision.

The global result is a bounded top-K list, not every supported hypothesis. The report records supported assignment counts (or an explicit integer-overflow state), retained truncation, evaluated cells, unavailable source evaluations and capped examples of missing support. A separate geographic summary contains every receiver's evaluated, rankable and unavailable clock counts, best score/clock and compact source/tone assignments. Fully unavailable and not-yet-evaluated receivers remain visible. Scores are uncalibrated; comparing a larger set of sources or fitting additional parameters can improve a residual by chance.

Optional mode comparison performs **independent searches** for each explicitly requested modality. It preserves the observations, receiver list, shared-clock range and approved alternatives within that modality; only irrelevant term flags and plan rows are disabled. Original weights and residual scales are retained. It does not infer Doppler-only or thunder-only global peaks from the truncated combined list. A modality can choose different aircraft/strikes and a different shared clock, so disagreement must be reviewed in both geography and time. Mode scores have different selected-term normalizations and are not likelihood ratios.

Spatial overlap after independent clock fitting is **not shared spatiotemporal support**. Geographic summaries retain only each receiver's best clock. They cannot establish whether additional low-score clock alternatives intersect or do not intersect. Inspect the combined search's common-clock residuals before interpreting agreement.

## Bounds and computational limits

- Up to 32 selected events, 256 approved sources per event and 1,000 retained hypotheses.
- At most 10,001 shared clock samples over a range no wider than 31 days. Step size is 0.001 to 86,400 seconds. Broad date uncertainty therefore requires coarse steps or separate reviewed passes.
- Up to 10,000 receiver locations under fixed sound speed; optional atmospheric refinement allows at most 400.
- User-declared budget up to 2,000,000 event/source/receiver/time evaluations, with hard limits of 20,000,000 residual calculations and 500,000,000 trajectory-point visits. The default evaluation budget is 200,000.
- The plan is rejected before running if its declared work exceeds these limits. Work is not silently capped or reported as a complete search after truncation.
- Independent mode comparison additionally checks the sum of all requested mode work against the operator's total budget (at most 6,000,000 evaluations). The per-mode regional plan budget and physical limits still apply. It does not automatically enable an evidence term that the analyst left disabled.
- All receivers must lie inside the declared non-wrapping rectangle. The underlying grid excludes the polar caps beyond ±85° and requires at least 10 m spacing.

A coarse clock or geographic grid can miss a narrow fit between samples. A retained location at the geographic/time/tone boundary suggests reviewing the declared limits. A low score away from a boundary is still not proof of uniqueness. Cancellation preserves completed cells and separately records evaluations already spent on an incomplete discarded batch.

## Inspecting predicted evidence

The native inspection model supplies a sampled Doppler curve over each selected reviewed interval, with every exact measured timestamp preserved. Its measured-point predictions are checked against the fixed scorer, and its other display points use the same prepared propagation models. Display samples are not new observations and do not change the score, source association, emitted tone or shared clock.

Each polyline belongs to one original continuous source segment and one trajectory edge. Source gaps, ambiguous reception-time overlap, velocity knots and failed model samples split the displayed curve; a renderer must never join separate polylines. Unsupported intervals and measured points retain explicit reasons. A curve stops at actual support limits instead of extrapolating across the rest of a reviewed event. Samples do not certify continuous model validity between them or provide a numerical curve-approximation error bound.

Doppler closest-approach and thunder arrival markers use exact fixed-score residual diagnostics in recording-relative seconds. The uncertainty belongs to the reviewed observation; the displayed effective residual scale also includes the configured model-error scale. Neither quantity is a calibrated prediction interval or location confidence. Lightning height fallback, when explicitly chosen, remains labeled as an assumption. Missing height or atmospheric support produces an unavailable marker reason, not a replacement arrival.

Inspection records the applied temporary review hash, loaded source-buffer hash, receiver, clock and propagation-model label. The user-facing operation remains a preview; changing observations, sources or the inspected hypothesis must invalidate its display. Applying the hypothesis remains a separate explicit action.

Display work is bounded independently of search: default target spacing is 0.5 seconds with at most 4,000 frequency samples across all events. The sampling step can grow to fit this display budget, with a visible coarsening flag; exact measured timestamps and native support/velocity boundaries are never dropped. If those mandatory points alone exceed the budget, inspection reports that limit. Hard limits are 20,000 samples and 100,000 selected trajectory points for fixed-speed propagation, or 4,000 samples and 10,000 trajectory points for an atmospheric profile. There are at most 32 selected events and 5,000,000 estimated support-boundary comparisons; highly fragmented source inventories can otherwise require disproportionate work. These are rendering-work limits, not evidence-quality thresholds.

## Receiver heights

The optional receiver-height configuration samples registered terrain and a declared compatible geoid. A WGS84 ellipsoid surface receives no geoid correction; EGM96 or EGM2008 surfaces need the corresponding registered geoid. The declared microphone height is above the selected raster surface, which may be a digital surface model containing buildings or vegetation.

Missing surface/geoid samples remain missing. Completing a regional grid requires height coverage for its declared receivers; there is no silent fixed-height substitution. Terrain sampling does not model acoustic occlusion, reflection or refracted paths. Dataset hashes, datum choices and height assumptions are preserved in the review and result identity.

## Reproducibility and export

`RegionalReport` records the original review hash, full loaded track/lightning buffer hash, receiver-list hash, plan, progress and compact assignment contributions. The GUI rejects stale evidence/input buffers before applying a hypothesis. Draft plans are saveable even while incomplete, but execution requires every active selected event to have explicit source alternatives and appropriate tone bounds.

The dossier exports into a **new directory** and refuses overwrites or paths through symbolic links/reparse points. It writes `index.html`, `hypotheses.csv`, `locations.csv`, `workspace.json`, `regional-report.json` and `receipt.json`. A comparison export additionally preserves the original review and complete `comparison.json` at its top level, links to separate mode dossiers, and retains not-started states without fabricating reports. An incomplete overall comparison stays prominently partial even when an earlier mode completed. A failure leaves an `INCOMPLETE.txt` marker. The HTML has no external assets or network dependencies; provider/user text is escaped, and spreadsheet formula-like text is neutralized in CSV. Original recordings and provider datasets are not copied. Local paths and case notes are private case content; inspect the dossier before sharing.

The backend controls compare an independent analytic moving-source oracle with the regional engine, replay every retained hypothesis through the fixed production scorer, and compare top-K results with exhaustive small-case searches. They also exercise endpoint coverage, missing sources, shared-clock semantics, explicit approval, bounds, cancellation, determinism and stale evidence. Export controls cover identity, misleading completion claims, HTML/CSV injection and artifact integrity. These establish software behavior. Independent field accuracy, reliable false-match thresholds and broad lightning-envelope searches require additional evidence and implementation.

### Reopening and optional archived inputs

**0.6.0 application validation:** 609 distinct native tests passed, including 122
headless GUI and launcher checks, four explicit media integrations and three
cached-data integrations. Python controls passed 132 tests with one unavailable
symbolic-link fixture; native junction controls passed. The 11-stage application
profile and static-C-runtime build passed. The historical 17-stage scientific
replay was not repeated. The aggregate receipt is
`output/benchmarks/investigation-reopen-001/summary.json`.

This version changes normal Windows startup to Direct3D 12 and
adds `geosonnar-safe.exe`, an identical executable whose exact filename selects
software Direct3D 12 rendering before a window is created. Software mode requires
a compatible CPU adapter and refuses hardware fallback. It does not change
search calculations, restore data automatically or replace recording/source
verification. Keep `geosonnar.exe` beside either launcher. The explicit `--opengl`
option is only a legacy path on `geosonnar-gui.exe`, and is refused by the safe
launcher. No Windows graphics settings are changed.

A reported 0.5.0 startup freeze remains unresolved until tested on the affected
desktop. The software path still discovers available Windows graphics adapters;
it is not a guarantee that no hardware driver is touched. New startup logs are
limited to 64 KiB per run and are written beside the executable, or in the temporary
folder if necessary. They identify the renderer and milestones before graphics,
at the creation callback, after workspace initialization, and after building the
first UI frame. The final frame marker precedes screen presentation. Logs omit
case paths, arguments, settings values and panic details. Headless configuration
and idle-work tests cannot establish that native graphics startup is responsive.
See the user-operated startup check in [WINDOWS_GUI.txt](WINDOWS_GUI.txt) before
testing reopening with the new package.

The preserved failed field hypothesis replayed with identical physical predictions
and measured residuals after native input loading, export, reopening and archived
inspection. Removing only owned source-file copies still allowed archived
inspection, while live source verification failed as required. Original evidence
remained unchanged. Built executables also passed three headless renderer-selection
checks and refused an OpenGL override through the safe launcher. No graphics
initialization or GUI launch occurred. These checks establish application behavior;
they do not verify a location or resolve the reported desktop freeze.

With the matching case open, choose **Open saved investigation** beside **Open
saved review**. Save or discard current edits first. Select the folder containing
`receipt.json` and `workspace.json`; for an independent comparison select its
top-level folder containing `comparison.json`. Results restores every saved mode
and completion status, with no hypothesis selected, applied or rerun. A changed
review or case invalidates a late background reply instead of replacing newer
work.

Use **Restore report data** to load an included input snapshot explicitly. For
older exports, load the original datasets in Data and use **Check loaded data
against report**. Missing or mismatched inputs leave Results readable and block
inspection/application. Once inputs match, **Inspect fit in audio** reconstructs
a completed hypothesis without changing measurements or source approvals. Applying
it is a separate undoable action and still requires the original sources.

The native `investigation_archive::load` reopens a dossier from its receipt, preserving the exact embedded review rather than rewriting its source paths. It checks every listed artifact's size and SHA-256, requires the expected file inventory and coherent mode/progress states, and rejects incomplete exports, traversal, links and Windows reparse points. Missing original recordings or provider files do not prevent viewing saved Results. Receipt hashes detect changes relative to the supplied receipt; they are not signatures or proof that a case is authentic.

New `export_with_replay_inputs` and `export_modalities_with_replay_inputs` exports may include one receipt-covered `replay-inputs.json` at the top level. `capture_replay_inputs` preserves ordered normalized map layers and the lightning catalog, their source identities, original geometry, support segments, UTC interpretation and altitude provenance. It copies no recording or raw provider files. Restoring these buffers is a separate explicit operation, performs no downloads or external-path reads, checks the native input schema and exact original merged-input hash, and never changes source assessments or approves an association. Legacy exports remain viewable; without replay buffers, inspection requires independently loaded inputs with the exact saved hash.

Reads are limited to 1 MiB per receipt, 64 MiB per report artifact, 256 MiB for the optional replay snapshot, and 512 MiB of unique receipt-covered artifacts overall. There are at most 64 artifacts, 128 replay layers and two million trajectory points. Comparison children cannot carry additional replay files outside the top-level byte inventory. Geometry checks allow only numerical roundoff in recomputed displayed coordinates (1e-9 degree and 0.1 mm height); stored physical arrays and input-hash values remain unchanged.

Archived inspection requires the unchanged saved review, receipt-verified matching input buffers and a present, hash-verified recording in the active case. It is labeled **Archived inputs; original source files not rechecked**. Missing recording permits Results viewing only. Applying a result, running a new search and saving a live review continue to require normal external source verification. Reopening does not start a search, apply a hypothesis or promote legacy unassessed sounds. Cancelled and not-started modes retain their original status.

An archive is not a complete portable case. It preserves a historical result and
optionally the normalized inputs used by that result; it does not acquire current
provider data or copy recordings, raw datasets or media tools. The GUI still needs
the matching case. Opening that case reports missing or changed recording bytes
without blocking access to saved Results; actual audio inspection remains blocked.
The standalone `index.html` is also readable. A complete case bundle and source-path relinking are separate future
work. See the short user-operated acceptance sequence in [WINDOWS_GUI.txt](WINDOWS_GUI.txt).

## Frozen regional experiment and numerical repair

The first uncertain-clock/source experiment is preserved in `output/benchmarks/regional-search-001/`. Two of its three noiseless on-grid combined controls failed the exact-recovery requirement. Investigation found that floating-point subtraction at an ECEF track vertex could produce two numerical closest approaches about `4e-11` seconds apart, representing the same physical vertex. The timing scorer correctly refused an apparently ambiguous pair, which made the intended receiver unavailable.

The propagation code now coalesces only roundoff-level roots at the same actual shared track vertex, with strict absolute and segment-duration caps. A regression control preserves genuinely distinct nearby minima. `regional-search-002` repeats the same frozen protocol and inputs after this general numerical repair; no per-case weights, source lists, seeds or search bounds were changed. The first failure remains part of the record. The repeated set is regression evidence after debugging, **not a pristine unseen holdout**.

The experiment's pass/fail gate covers analytic recovery, explicit invalid-input errors, declared search coverage and approved alternatives. Noisy and weak-evidence outcomes are descriptive, including their errors and unresolved cases. A passing software gate must not be presented as an independent field-accuracy result or a calibrated false-match rate. The native adapter receives the separate input batches; evaluation labels remain outside those inputs. This is local procedural separation rather than an independently administered blind trial.
