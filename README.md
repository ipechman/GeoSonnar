# GeoSonnar

Download the portable Windows ZIP from [GitHub Releases](https://github.com/ipechman/GeoSonnar/releases), extract it, and run `geosonnar-gui.exe`. See the [Windows guide](docs/WINDOWS_GUI.txt) for media-tool setup and the indexed flight-data workflow.

GeoSonnar 0.11.2 is an experimental Rust investigation workbench for aircraft and thunder audio on Windows. The GUI follows **Case → Audio review → Data → Search → Results**, with editable measured traces, video/audio preview, an interactive map, case-owned data and explicitly selected evidence. Guided regional searches consider a shared uncertain recording time and analyst-approved aircraft/strike alternatives. Independent Doppler/thunder searches expose disagreement in location and time. The optional shared-cell method introduced in 0.10.0 compares measured Doppler frequencies and a reviewed thunder envelope on the same complete location/time grid. Optional atmospheric profiles support experimental local refinement of the existing event method. Processing jobs and the command-line pipeline remain available.

GeoSonnar is copyright (c) 2026 GeoSonnar contributors and is free software under
the **GNU General Public License, version 3 or (at your option) any later version**
(`GPL-3.0-or-later`). It is distributed without any warranty. See [LICENSE](LICENSE)
for the full terms. Third-party code, assets and external datasets retain their
own notices and terms; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Status and validation

Version **0.11.2** adds a separate **Wide-area two-pass Doppler** path for day archives and nationwide receiver grids. In **Data → Stored datasets**, select a complete downloaded archive, set its **Extraction region** and aircraft-area padding, then choose **Use complete archive UTC day → Build indexed flight data**. Select the new indexed copy and choose **Use indexed flight data**. This retains exact samples and provenance in bounded disk blocks instead of the legacy two-million-point map buffer. Widening Search later does not expand the imported source coverage. In Search, explicitly choose the wide-area method, copy two reviewed Doppler traces, review the CPA/tone/clock/physical bounds, estimate the complete grid, then run. Receiver counts above 10,000 do not require allocating the complete grid or changing the declared spacing.

The wide-area method derives the common recording clock from candidate closest approaches and joins their heard-time separation. It uses a spatial index, a bounded exact-track cache and paged receiver results. Flight samples and source provenance are packed into bounded files, with paged metadata, so short flight segments do not each create two separate files. Provenance compression is buffered while preserving exact source records. **Advanced memory limit** defaults to 512 MiB and can be adjusted before import, loading or search; the estimate reports its source/index reservation separately from the 256 MiB track cache. For the tested complete-USA day source, set this to **1024 MiB before loading and estimating**: the recorded national preflight required about **531 MiB**, exceeding the default. That check verified source loading and planning, not completed nationwide scoring. This is an index allowance, not a limit on all application memory. Work, elapsed-time and output budgets remain explicit; cancellation or exhaustion produces a partial result with completed/remaining counts. Fixed ellipsoid receiver height and constant sound speed are explicit assumptions in this first path. Terrain, atmosphere and thunder remain available through the existing refinement methods. Interrupted wide-area runs can be reopened for review but cannot yet resume.

The full-day import comparison used the **same archive and selection**, sequentially on one machine under the same **20% aggregate CPU cap**. Elapsed time fell from **17 min 25 s to 3 min 58 s** (about **4.40× faster**). All **54,788,942 retained samples and 461,344 passes** matched the baseline, with identical index, metadata, point and provenance hashes; a fresh native open verified the stored data. The USA 10 km grid's **196,647-receiver Estimate** also passed with a 1024 MiB source/index allowance. These are measured import and planning results, not completed nationwide scoring or a location-accuracy result.

**A work-budget stop is incomplete, not “no match.”** In a separate 50-cell diagnostic, the historical native configuration completed 50/50 cells in 24.39 s; broad GUI copied suggestions reached the **250000000 work-unit limit at 0/50 completed cells**. Its 102 provisional pair scores were excluded from completed results because the first receiver was unfinished. The historical configuration used distinct physical bounds for each event; the GUI currently shares one set across both, so these are different workloads. Review evidence-supported tone and physical ranges, choose an explicit work budget, then re-estimate. Work units are a raw integer budget, separate from runtime and memory; raising only the time limit does not remove this limit. A sufficient larger budget for those broad GUI suggestions has not been tested, and no nationwide scoring runtime is established.

Version **0.11.2** adds frequency panning in **Inspect / seek**. In a zoomed spectrogram, drag vertically with the left mouse button to move the visible frequency band. The band keeps its width, stays within the loaded frequencies, and responds immediately when you reverse direction at an edge. Horizontal dragging scrubs time; clicking still selects a trace or seeks. Panning changes the view without editing annotations, measurements, source approvals or undo history.

The **0.11.2** source passed **888 native tests**, including **218 headless GUI/launcher checks**, a separate **2.1-million-point** storage control, and all **nine media/cached-data integrations**. Python controls passed **132 tests**, with one unsupported fixture skipped; all **seven release-collector controls** passed. Checks include exact small-grid score parity, more than 10,000 completed receivers, packed-file boundaries and source provenance, memory-budget refusal, cancellation during verification/indexing/scoring, partial indexed-source deletion, and large-catalog/results scrolling. New controls verify exact provenance and point values across compression-buffer boundaries, refusal to publish a completed index after a final buffered-write failure, and unchanged source counts for accepted and rejected rows. These establish the listed software behavior, not nationwide throughput or independent location accuracy. Receipts are under `output/benchmarks/frequency-pan-001/`.

Version **0.11.1** corrects data-library status and deletion behavior. Stored FMI requests distinguish planned, partial, failed, cancelled, complete and complete-empty outcomes; imported datasets have a separate status. Invalid or altered status metadata is unavailable. These labels do not certify provider coverage or verify the raw data.

Deleting a managed copy now waits for the operation's outcome before unloading affected inputs. Unrelated copies, review choices and numerical results remain. A failed deletion preserves loaded state unless files actually became unavailable; an interrupted deletion reports the remaining recovery action. Results whose sources were removed remain historical, with source-dependent inspection withheld until the matching inputs are restored and checked. Default rendering and scoring are unchanged.

The **0.11.1** source passed **821 native tests**, including **196 headless GUI/launcher checks**, all **nine media/cached-data integrations**, formatting and Clippy. Python controls passed **132 tests**, with one unsupported fixture skipped; all **seven release-collector controls** passed. Eleven new GUI controls cover selective deletion, historical-only dependencies, interrupted removal, unchanged state on refusal and exact source restoration. Release receipts are under `output/benchmarks/data-library-001/`. No interactive GUI or graphics device was opened, and no new field-accuracy result or scientific replay is claimed.

Version **0.11.0** adds a visible media-setup check before case creation. FFprobe enables intake; FFmpeg enables decoding; FFplay remains optional. Shared discovery keeps automatic choices portable, preserves explicit paths, and checks local file locations in the background without launching helpers. Settings offers Browse and Auto-detect even while an earlier file check is waiting.

The **0.11.0** source passed **808 native tests**, including **185 headless GUI/launcher checks**, all **nine media/cached-data integrations**, formatting and Clippy. Python controls passed **132 tests**, with one unsupported fixture skipped. The new checks cover portable tool discovery, preservation of manual choices, queued refreshes, stale replies and closing without waiting for a file lookup. No interactive GUI or graphics device was opened. Release receipts are under `output/benchmarks/media-readiness-001/`; the numerical-science checks below remain labeled with the versions that executed them.

A separately frozen storm-data exercise completed all **1,248 planned FMI minute queries**, with one explicit retry after a preserved connection timeout. Six required comparison windows returned no records, so the declared prerequisite withheld **all 126 planned location searches**. This is an unavailable field trial, with no new location-accuracy result. Its evidence and refusal receipts are retained under `output/benchmarks/nordic-field-005a-002/`.

Version **0.10.0** introduced the explicitly selected **Doppler + envelope (experimental)** method, which remains available. Choose and measure a thunder interval in Audio review, review aircraft alternatives and the shared location/time grid, declare lightning query coverage, then estimate and run. Results retain separate Doppler error, envelope cosine, ordinal combination, coordinates, clock offsets and unavailable cells. Selecting a row pins the map without applying its clock or source assignments. Cancellation and durable run history preserve the complete declared domain; missing cells prevent combined ranking. See [the workflow and limits](docs/THUNDER_ENVELOPE.md).

The **0.10.0** source passed **793 native tests**, including **177 headless GUI/launcher checks**, all **nine media/cached-data integrations**, formatting, Clippy and the portable build. Python controls passed **132 tests**, with one unsupported fixture skipped; native Windows link checks passed. The built program passed **188 CLI checks**, **324 checks across six historical hypotheses**, and remeasured/replayed the storm reference with byte-identical observation and all nine numerical results. All **48 frozen joint cases and 72,912 cells** matched the previous Doppler errors, envelope cosines, fitted tones, source assignments, ranks and boundary flags exactly; five CLI failure/cancellation controls passed. Receipts are under `output/benchmarks/joint-workspace-001/`. No GUI or graphics device was opened. These are software and numerical regression checks; independent field accuracy, reliable false-match rejection and hybrid field improvement remain unproved.

Version **0.9.1** fixes default FFmpeg/FFprobe discovery in command-line thunder-envelope measurement. Leaving those configuration fields empty now resolves bundled helpers first, then installed helpers through absolute PATH entries. Explicit selections retain their config-relative meaning and report an error when missing.

The 0.9.1 source passed **745 native tests**, including **154 headless GUI/launcher checks**, all **nine media/cached-data integrations**, formatting, Clippy, and the portable static-C-runtime build. Python controls passed **132 tests**, with one unsupported symbolic-link fixture skipped; native hard-link and junction checks passed. The built CLI passed **188 workflow checks** and **324 checks** across six historical replays. It also remeasured the full storm reference using the original default-helper configuration and repeated all nine searches: the observation and every result were **byte-identical** to the preserved 0.9.0 outputs. No GUI or graphics device was opened. Receipts are under `output/benchmarks/default-tools-001/`.

Two separate experiments used the preserved **0.9.0** program. A [source-informed storm reference](docs/STORM_VALIDATION.md) selected the same location under three declared source-height assumptions, **7.334 km** from the exposed reference; the correct-day maximum exceeded both neighboring-day controls. The [72-case synthetic stress experiment](docs/THUNDER_ENVELOPE_STRESS.md) found that weak evidence and wrong catalogs can still score highly. These results support explicit review and ambiguity reporting; they do not establish an acceptance threshold, blind field accuracy or hybrid improvement.

A subsequent [48-case joint-audio pilot](docs/JOINT_ENVELOPE_VALIDATION.md) used unchanged **0.9.1** programs to compare Doppler, thunder envelopes and combined ranking on identical location/time candidates. In weak cases, combined first-place containment improved to **5/12**, versus **4/12 Doppler** and **2/12 envelope**; its top-five containment was **10/12**, versus **12/12** and **7/12** respectively. Wrong lightning data also degraded Doppler results. This supports preserving individual views and explicit disagreement, rather than making combined ranking the default. It is synthetic evidence, not field validation; its separate optional method is implemented in 0.10.0.

The strongest weak-case benefit was the predefined shortlist of **31 joint location/time cells**: combined ranking retained a candidate within **2 km and 1 second in 11/12 cases**, versus **7/12 Doppler** and **8/12 envelope**. The report preserves geometric support volume and paired gains/losses; these results do not supply calibrated confidence.

Version **0.9.0** adds **Results → Run history**. Explicit regional and comparison runs retain their original review, source bindings, status and numerical result. Later edits do not replace earlier runs. Open, Save copy and Delete are separate actions; interrupted or damaged entries remain visible. A failed write retains the result in memory, warns before closing, and blocks another run until retry or explicit discard. Compact history says **Saved result; replay requires original data**. Full input archives remain a separate export.

The [thunder-envelope command-line workflow](docs/THUNDER_ENVELOPE.md) measures an explicitly chosen audio interval, previews work, and compares it against a declared lightning energy model over bounded receiver/clock alternatives. It preserves input hashes and outputs reviewable curves. The optional shared-cell method combines it with Doppler frequency, without adding picked-arrival timing. Field accuracy, false-match thresholds and hybrid improvement have not been established for this method.

The final 0.9.0 source passed **741 native tests**, including **154 headless GUI/launcher checks**, plus all **eight media/cached-data integrations**, formatting, Clippy and the portable static-C-runtime build. Python controls passed **132 tests**, with one unsupported symbolic-link fixture skipped; actual native hard-link and junction checks passed. The built companion passed **188 end-to-end checks** across measurement, preview, search, cancellation, receipts, finite curve output, invalid arguments and overwrite refusal. Six preserved real hypotheses passed **324 replay checks** with every prediction and score unchanged. These are application/numerical checks, not new field-accuracy results. No GUI was opened and no live provider request was repeated. The aggregate receipt is `output/benchmarks/history-envelope-001/summary.json`; failed development attempts remain preserved.

The default **Guided view** focuses on the current step. **All controls** restores the dense workspace without changing evidence, detection thresholds, search settings or source approvals. Basic sound-finding presets stay visible; review file actions, secondary map controls and detailed settings are grouped. Data separates **Get data** from **Stored datasets**, while progress, cancellation and source interpretation remain available. This is a presentation choice, not an automatic analysis mode.

Spectrogram wheel zoom now changes both time and frequency around the pointer. Ctrl-wheel changes time only, Alt-wheel frequency only, and Shift-wheel pans time. Visible frequency controls and **Fit loaded frequencies** restore the measured band. Loading audio uses a separate frequency limit, so zooming cannot silently narrow the next decode. Zoom does not add FFT resolution or alter annotations.

Closing during work offers **Cancel all and exit** with an explicit unsaved-edits notice. It signals cancellation, gives workers a bounded two-second grace, then closes without waiting indefinitely. A Windows process group owns this GUI's helper processes so remaining downloads, decoding, playback and CLI descendants stop at exit; unrelated processes are outside that group. Saved originals remain intact, while interrupted work can leave partial output or require inspection before resumption. Ordinary close requests do not discard unsaved edits without confirmation.
Version **0.8.0** adds explicit FMI/NORDLIS historical lightning requests, native XML import, resumable managed storage and separate source interpretation before thunder scoring. Plan, download, import, load and review remain separate actions. Timestamps, coordinate datum and source attributes survive filtering, saving and reopening. FMI covers Northern Europe; it does not provide worldwide or Quebec coverage. See [the FMI workflow and limits](docs/FMI_LIGHTNING.md).

The final 0.8.0 source passed **689 native tests**, including **143 headless GUI/launcher tests**, plus all **seven media/cached-data integrations**, formatting, Clippy and the portable static-C-runtime build. Python controls passed **132 tests**, with one actual symbolic-link fixture unavailable; native Windows junction controls passed. A live one-minute FMI request matched all **36 observations and 144 source attributes**, then reused cached responses and imported them. Six frozen real-audio hypotheses passed **324 arithmetic/integrity checks**, preserving every prior prediction and score. The receipt is `output/benchmarks/fmi-pipeline-001/summary.json`. Tests include stalled playback cleanup and owned-process shutdown while an unrelated process survives. No GUI window was opened. This validates application and acquisition behavior; independent hybrid location accuracy and calibrated confidence remain unestablished. The historical 17-stage scientific profile was not repeated.

Version **0.7.0** adds a collapsed **Fit comparison** panel after **Inspect fit in audio**. It compares the unchanged Doppler hypothesis with fitted flat and linearly drifting tones at the same measured timestamps. Ordinary Hz error and error divided by the original scoring scales are reported separately. Missing support withholds comparison instead of dropping points. These are descriptive fits on the measured data, with different parameter counts and no correction for previous search size; they do not identify a source or provide location confidence. Scores, selections and source approvals remain unchanged.

The 0.7.0 diagnostics profile passed **618 native tests**, including **124 headless GUI/launcher tests**, all **seven scoped validation stages**, Clippy and the portable static-C-runtime build. Six preserved real-audio hypotheses passed **324 independent arithmetic/integrity checks** with every previous prediction and score unchanged. Seven installed-media/cached integrations and the historical scientific/Python profiles were not repeated for this diagnostics-only change; their previous results remain bound to 0.6.0 or earlier. The receipt is `output/benchmarks/fit-diagnostics-001/summary.json`. The WSU trace favors a drifting-tone baseline, while the previously unsuccessful fresh-B estimate beats both baselines: this demonstrates why fit advantage alone cannot become geographic confidence. No GUI window was opened.

Version **0.6.0** adds reopening complete saved investigations and explicit restoration of their original normalized inputs. Results remain readable when original media or datasets are unavailable; audio inspection separately checks the recording, and applying results or scoring again still verifies original sources. Opening a dossier never runs a search or applies a hypothesis. A background library refresh now preserves the preceding operation's explanation.

Windows startup defaults to **Direct3D 12**. The separate **`geosonnar-safe.exe`** launcher selects software Direct3D rendering and refuses hardware fallback; explicit `--opengl` remains available only as a legacy option on the normal launcher. The user confirmed that the **0.6.0 software launcher stays responsive on the initial Case screen**, after reporting a whole-computer stall with 0.5.0. The user subsequently reported using the **0.7.0 default GUI without issues**. Specific workflows were not identified, so this feedback does not certify every media/search path or the new build. The original 0.5.0 freeze cause remains unestablished. Automated validation opens no native graphics device or GUI window. Startup logs and settings reads are bounded, and a 120-frame headless check covers idle work. Software adapter discovery still enumerates Windows graphics adapters.

The 0.6.0 application profile passed **609 native tests**, including **122 headless GUI/launcher tests**, four explicit media integrations and three cached-data integrations. Python controls passed **132 tests**, with one actual symbolic-link fixture unavailable; native junction controls passed. All **11 application-validation stages** passed, including Clippy, a static-C-runtime build, unchanged source checks, and exact export/reopen/inspection replay of the preserved unsuccessful field hypothesis. Three built-launcher configuration checks and safe-launcher rejection of OpenGL passed without initializing graphics. The receipt is `output/benchmarks/investigation-reopen-001/summary.json`. The historical 17-stage scientific profile was **not rerun** for 0.6.0; its prior results remain bound to their earlier builds.

The GPS-linked Western Sydney reference minute was also acquired and checked separately. Default discovery found no traces. A frozen manual trace supplied 30 measurements; at the known receiver, nominal Doppler RMSE was **4.53 Hz**, versus **4.49 Hz** for a flat tone. Five fixed-clock scenarios and all original measurements were retained, with 259 verification checks. Unknown tone identity, recording/telemetry alignment and approximate height assumptions remain material; raw GPS altitude steps also affect predicted velocities. This is a source-informed consistency check, not location recovery or hybrid validation. See [the field-validation record](docs/FIELD_VALIDATION_PLAN.md).

The Windows GUI supports manual spectrogram traces, reviewed Doppler/thunder events, bounded regional ADSB.lol history requests and whole-day archives, native track import, Copernicus terrain requests, FMI/NORDLIS history and authorized lightning-export requests, selectable scoring terms and sortable location results. Receiver heights can use explicitly applied terrain and a compatible geoid. Thunder detection proposes low-frequency energy bursts; the hybrid score uses reviewed strike-arrival timing. Optional altitude profiles model temperature, pressure, humidity and directional wind along straight paths. Global multiweek lightning-envelope correlation, weather-provider downloads and refracted propagation are not implemented.

The new regional workflow is described in [REGIONAL_SEARCH.md](docs/REGIONAL_SEARCH.md), and the small aircraft-data request in [ADSBLOL_HISTORY.md](docs/ADSBLOL_HISTORY.md). Every declared receiver retains its best supported source/time hypothesis independently of the truncated combination shortlist. Applying a result, preparing refinement, importing incomplete data and approving a source are explicit analyst actions. Self-contained investigation dossiers preserve assumptions, competing results, source identities and partial-work status.

Acoustic-source assessment is separate from confirming a measurement. For each active scoring event, explicitly assess whether its sound is compatible with the selected class, or permit an uncertain hypothesis with a reason. Unassessed and incompatible active events block scoring; disabled evidence does not. An uncertain interpretation stays visible in predictions and reports and does not change the numerical score. This is analyst review, not a new aircraft/thunder classifier. Legacy reviews retain their measurements and selections but require source assessment before a new workspace search. New and imported events start unassessed; the application does not infer compatibility from an old confirmation.

**Independent field accuracy remains unestablished.** Two real Doppler-only automatic-proposal trials returned leading estimates 102.1 km and 56.1 km from a published deployment marker. A longer, source-informed spectral review improved the first shortlist but still placed its leader 34.7 km away. A fresh deployment, with the audio review frozen before viewing aircraft identities or exact coordinates, returned an 81.0 km leading error; neither its top 10 nor top 20 distinct receivers met the declared 25 km tolerance. These tones lack independent aircraft-source identification, and deployment-position and recorder-clock errors are unquantified. The failed candidate-location experiments are preserved in [FIELD_VALIDATION_PLAN.md](docs/FIELD_VALIDATION_PLAN.md) and [the active acceptance plan](docs/REGIONAL_WORKFLOW_PLAN.md); low scores and many assignments must not be treated as geographic confidence. The private recording still has no independently verified microphone location.

The preserved regional experiment recovers the reference aircraft pair and a stable local optimum, but it is **source-informed, not an independently verified location**. Wider alternatives, coverage gaps and model assumptions remain material. See [the benchmark overview](benchmarks/README.md), [audio-derived refinement](benchmarks/AUDIO_PRIORS.md), and [bounded sensitivity](benchmarks/SEARCH_SENSITIVITY.md).

Version **0.5.0** passed **584 native tests**, including **105 headless GUI tests**, four explicit media integrations and three cached terrain/legacy-review integrations. All **17 implemented validation stages** passed in `output/benchmarks/validation-015/summary.json`; Python controls passed 130 tests with one actual symbolic-link fixture unavailable on this Windows account. The native junction check passed. A new synthetic regional replay passed 231 checks. Five source-assessment variants of the preserved failed field inspection verified blocking behavior and unchanged numerical predictions; this was a compatibility replay, not a new location trial. The aggregate receipt is `output/benchmarks/source-assessment-001/summary.json`. No GUI window was opened. Source review does not establish a sound's identity or calibrated confidence.

Version **0.4.0** passed **560 native tests**, including **97 headless GUI tests**, four explicit media integrations and three cached terrain/legacy-review integrations. The full **17-stage implemented validation profile** passed in `output/benchmarks/validation-014/summary.json`, including historical scientific replays, formatting, Clippy and a static-C-runtime release build. The Python controls passed 126 tests with one actual symbolic-link test skipped because Windows did not permit its fixture; the separate native junction check passed. The aggregate receipt is `output/benchmarks/regional-workflow-001/summary.json`. These historical receipts certify their own snapshots.

The unchanged detector's [two annotated source-presence audits](docs/DETECTION_VALIDATION.md) produced sharply different results. Hour one had 64 harmonic groups: four wholly inside aircraft-audible intervals, 51 wholly inside intervals labelled no salient aircraft, and nine mixed or uncertain. The next untouched hour had 21 groups: 14 wholly inside aircraft-audible intervals, zero wholly inside negative intervals and seven mixed or uncertain. These are temporal overlap measures, not independently identified Doppler sources or a measured benefit from the new source-review controls. No thresholds were tuned on either recording. **Inspect fit in audio** displays native predicted curves and arrival markers against measured evidence without applying associations; changed evidence, inputs, modes or receivers invalidate the inspection.

The previous **0.3.0** release passed **446 distinct native checks, including 69 headless GUI controls and four explicit media integrations**, plus 31 Python controls. Its application receipt is `output/benchmarks/hybrid-atmosphere-001/summary.json`. Eight real infrasound recordings also passed native audio processing, but lack the independent geometry, strike and clock metadata needed to measure location accuracy. Historical receipts certify their own snapshots only.

Historically, version **0.2.0** passed `validation-013`: 395 native tests, four explicit media integrations, 15 harness controls and all 17 implemented validation stages, with two cached-terrain comparisons separately. The full 17-stage scientific replay was not repeated for 0.3.0. Provider downloads have offline controls; this release does not claim a new live provider-download check. Local receipts, recordings and generated results are excluded from source uploads. Automated checks do not establish independent hybrid accuracy, calibrated confidence or full parity with other projects.

See [the current comparison with DOPPLERGEIST and Franken](docs/UPSTREAM_COMPARISON.md) for pinned revisions, recent progress, evidence limits and implementation priorities.

The [headless hybrid ablation](benchmarks/HYBRID_ABLATION.md) compares Doppler-only, thunder-only and combined scoring on identical synthetic holdouts and misleading alternatives. Combined scoring did **not** improve the primary strong-Doppler baseline; it helped some single-aircraft geometries and still failed with wrong aircraft or clocks. Its historical results are preserved. The new [evidence comparison](docs/EVIDENCE_COMPARISON.md) makes complementary weak evidence and disagreement visible without automatically rejecting different peaks. [Atmospheric modeling](docs/ATMOSPHERE.md) is experimental and opt-in; real-world hybrid improvement remains unestablished.

[Real storm validation](docs/STORM_VALIDATION.md) records acquired research waveforms and the Thunderprint reference case. Neither raw Frost lightning logs nor the reference audio are distributed by Thunderprint; Frost acquisition remains pending. A waveform integration pass is distinct from independently verified location accuracy.

## Run on Windows

For a packaged build, extract the complete portable folder. **If you experienced the 0.5.0 startup freeze, try `geosonnar-safe.exe` first**; it uses software rendering. The normal **`geosonnar-gui.exe`** uses Direct3D 12. Keep `geosonnar.exe` beside both launchers. Check `package-receipt.json` and its referenced validation report for the exact build. See [Windows GUI instructions and a short manual checklist](docs/WINDOWS_GUI.txt).

**Media setup:** the Case screen shows whether **FFprobe** and **FFmpeg** were found. FFprobe is required to create a case; FFmpeg is needed when decoding audio or video in Audio review. **FFplay** is optional for playback and is listed in Settings. These external programs are not bundled. Put the actual executables in the portable folder's `tools` directory, or choose **Browse tools / Settings** and browse to installed executables, then **Save settings**. Package-manager launch shortcuts are not supported.

If they are not installed, [FFmpeg's download page](https://ffmpeg.org/download.html#build-windows) links to Windows executable builds. Extract a Windows x64 static build, keep its license and README, and copy `ffmpeg.exe`, `ffprobe.exe` and optionally `ffplay.exe` from its `bin` folder into GeoSonnar's `tools` folder. Then choose **Auto-detect**. Downloading or installing these tools is a separate user action.

Blank tool paths stay automatic: GeoSonnar checks `tools/`, beside the program, the known actual Chocolatey `ffmpeg-full` installation, then local absolute PATH entries. Automatic checks skip direct network locations and detected symbolic/reparse links; an explicitly chosen path retains its meaning. **Auto-detect** deliberately clears saved manual selections and checks again. This is useful after moving a portable copy whose older settings contain obsolete absolute paths; missing manual paths are otherwise preserved. The background check reads file locations only and launches no media tool. **Found** does not certify a tool's version or compatibility. Existing cases and data work remain available when a decoder is missing.

1. **Case:** create or reopen a checked recording and retain uncertain date/region clues.
2. **Audio review:** use the full-recording navigator to select visible audio, load a measured channel spectrogram, wheel-zoom at the cursor or Shift-wheel pan, and draw a Doppler ridge by dragging. Seek decoded video frames, play visible audio, and add/move/erase precise points or thunder intervals. Detection presets and thresholds propose events. Confirm measurements, assess the possible source separately, and explicitly select evidence; uncertain hypotheses require a reason. Editing a measurement resets its review/source assessment, and Undo restores the previous choices.
3. **Data:** import owned copies and verify/load layers. Choose a small ADSB.lol time/area request or a global day archive, review its limits, then explicitly fetch and import. For the wide-area method, use **Build indexed flight data → Use indexed flight data** as described above; **Import bounded tracks into library** remains the separate smaller map/regional path. Missing files, capped candidates and incomplete coverage remain visible. Terrain and authorized lightning-export requests are also available. Import an optional normalized JSON atmospheric profile under **Atmospheric profile**; the complete profile is saved in the review.
4. **Search:** for **Reviewed events**, follow **Area & time → Evidence → Run**. Declare the region and shared clock uncertainty, approve source alternatives for each selected event, and bound the emitted aircraft tones. Estimate work, then explicitly run or cancel. Optionally compare independently searched Doppler, thunder and combined modes. For **Wide-area two-pass Doppler**, select exactly two confirmed, source-assessed Doppler events with nonoverlapping intervals and at least three measured points each, then choose **Copy the two selected reviewed Doppler events**. Review its separate physical assumptions and Estimate before Run. Terrain/geoid height application belongs to the existing regional methods and requires complete declared coverage.
5. **Results:** regional results can sort the best hypothesis at every receiver or inspect retained source/time combinations. Wide-area results show a sortable retained shortlist; the saved run folder also contains processed-receiver records. Review unavailable support, residuals, boundary flags and any modality disagreement. **Prepare local area** makes a smaller wide-area grid without running it or transferring aircraft associations to another method. Re-estimate explicitly. Reopen with **Open saved wide-area run…**, selecting the run folder; keep its matching `.review.json` file beside it to retain review context. Existing regional methods retain their separate investigation-dossier export.

Delete removes the selected audio event; Ctrl+Z and Ctrl+Y/Ctrl+Shift+Z undo and redo, and Ctrl+S saves. Text fields retain their own editing keys. The window adapts down to 760 × 560 logical pixels, with event/inspector tabs below the audio canvas at narrower widths. Jobs and Settings are separate utilities. The bottom sun toggle persists the light/dark theme; measured spectra and video use dark panels. The interface retains native file dialogs, background work, immutable review revisions, source hashes and unsaved-edit protection. Scores and low-score areas are uncalibrated. Fixed comparison holds associations, tones and clock fixed; regional search considers only the explicitly approved alternatives. Spatial overlap after independent clock fitting is not common-time confirmation. Loading terrain alone does not apply heights. The existing regional methods allow up to 10,000 fixed-speed receivers; the separate indexed wide-area method is not subject to that cap. Optional atmospheric refinement is limited to 400 receivers with tighter work bounds. Date-line-crossing receiver grids are unsupported.

Natural Earth supplies coarse offline country outlines. Optional OpenStreetMap tiles are off by default; enabling them sends requests for the viewed map area and retains a local cache with visible attribution. There is no bulk map-download feature. Video is a periodic frame preview, and the FFplay-driven cursor estimates playback position. Use measured evidence for arrival times. Thunder proposals are experimental low-frequency energy soundprints, not a trained thunder recognizer or an established accuracy improvement.

Build with Rust 1.94.1 or compatible. Before running the tests, put real **FFmpeg
and FFprobe executables on PATH**: the audio integration tests invoke them, and
GUI Settings do not configure test tools.

```powershell
cargo test --locked --features gui
cargo build --release --locked --features gui
```

The executables are `target/release/geosonnar-gui.exe` and its command-line companion `target/release/geosonnar.exe`; delivery copies are in `dist/`. Select actual **FFmpeg and FFprobe** executables in Settings, plus **FFplay** for audio playback. The external tools are not bundled. Numerical search needs no Python. Windows downloads use the system `curl.exe`; native code checks recorded content hashes. The packaging/validation workflow creates static-C-runtime delivery builds; a plain Cargo build above does not itself certify a portable release.

For a short manual check, launch the new build yourself, load a brief recording interval, test seek/Play/Pause/Stop, edit and confirm an event, save/reopen its review, import a small dataset, and run a bounded comparison with explicit associations and scoring terms. Sort the Results table and restart after changing the sun toggle. Missing evidence should remain visible as unavailable support. Large downloads and the online map are separate explicit actions. The validation-013 receipt identifies the 0.2.0 build and its checks. Visual usability and hands-on interaction are left for user testing.

Historical CLI examples below use the preserved **0.8.0** package layout and local research inputs. They are not a bundled demo in the 0.11.2 download. From the matching project directory:

```powershell
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe inspect --project . --out output/input-inspection.json
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe preflight --case cases/colsto.json --out output/preflight.json
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe analyze --input "Known Videos with coordinates/Unknown location - raw audio - colsto (144p, h264).mp4" --channel 0 --out output/new-audio-analysis
```

`inspect` only establishes that the source file is readable. `preflight` checks input readiness. Neither claims to locate a camera. Analysis requires a new output directory so previous spectral evidence is preserved. Exit codes: 0 completed, 2 invalid command/input or operational failure, 3 blocked preflight, no search candidate or incomplete regional-history acquisition (partial, cancelled or budget exhausted).

## Source checkout and GitHub upload

Commit source, tests, examples, configuration templates, pinned benchmark fixtures and license notices. `Cargo.lock` is intentional. The folders `data/`, `output/`, `target/`, `tmp/`, `dist/`, original/private recording folders and local settings are excluded. The supplied replication PDF is a local input, not a redistribution asset. The machine-bound native sensitivity contract also remains local; portable benchmark contracts are included.

For an upload folder and ZIP containing only approved source files:

```powershell
python analysis/package_source.py --output output/github-source --zip dist/GeoSonnar-source.zip
```

Choose fresh destinations on subsequent exports. The exporter checks included files, rejects links and common embedded credentials, and records file hashes. Inspect that source folder before publishing; `.gitignore` alone does not filter a browser drag-and-drop upload. No GitHub repository or upload is created by the exporter.

Build from a clean source checkout using the Rust commands above. Runtime audio/video tools are separate inputs. Optional research scripts additionally use NumPy, SciPy, pandas, PyArrow, Pillow, matplotlib and soundfile; install the libraries required by the particular analysis. Full real-recording replays require separately acquired, licensed data and the matching local artifacts. They are not promised to run from source alone.

Portable executable packaging now uses checked-in notices, so an existing `dist/` directory is unnecessary:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release --locked --features gui
python analysis/package_windows.py --output dist/GeoSonnar-Windows --readme docs/WINDOWS_GUI.txt
```

Earlier local validation receipts and the shipped binary describe their exact historical snapshots; regenerating a package from edited source requires new checks.

## Request data by date and area

The GUI's **Data** step offers small regional historical requests and whole-day archive planning, explicit download/resume and native import. A small request selects candidate identities from sampled map replay and downloads their detailed traces; it is bounded to one hour within one UTC day, 256 traces and a declared byte budget up to 250 MB. It can miss aircraft between index samples or outside provider coverage. The whole-day archive route remains available for broader acquisition. Geometric altitude is the default, and a barometric approximation requires a declared offset, uncertainty and rationale. See [the small-request commands](docs/ADSBLOL_HISTORY.md) and the archive commands below. The older adsb.aero adapter remains available as follows.

Edit `cases/regional_request.json` or create another request:

```json
{
  "date_utc": "2026-09-08",
  "bounds": [40.65, -73.85, 41.1, -73.2],
  "max_flights": 5000,
  "refresh": false
}
```

Bounds are **south, west, north, east** for aircraft coverage, in WGS84 degrees. Pad the intended microphone search area to include nearby flight paths. Dates are UTC. Then run:

```powershell
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe fetch --request cases/regional_request.json --out data/adsb_aero/2026-09-08-oyster-region
```

The request for this project retrieved 3,161 flights in 32 pages and exhausted the API cursor. Matching cached requests work offline after checking SHA-256 for the aggregate and every raw page. An explicit refresh preserves old runs. The report records raw requests/responses, the provider schema, observed date availability, limits, and completeness. A completed query does not mean every aircraft was observed. The initial cache predates hashing; its migration first verified aggregate equality with deduplicated raw pages, recorded in `cases/acquisition_integrity_receipt.json`.

The public [adsb.aero API](https://adsb.aero/api/docs) currently supplies paths simplified to 50 m horizontally and 100 ft vertically, with pressure altitude. They support candidate discovery, but proved insufficient for this detailed replication: the correct first flight lost full event coverage across a 257-second vertex interval. The attempt remains in `output/regional/`: the correct pair was rejected, while other unverified numerical hypotheses remained. The program did not shorten the trace or invent missing positions to force a match. Large areas require tiling and deduplication; this initial adapter is for bounded regional requests.

## Repeat the detailed regional experiment

### Native ADSB.lol acquisition and import

The native pipeline can plan and resume an official [ADSB.lol daily archive](https://www.adsb.lol/docs/open-data/historical), verify every part against its published SHA-256, stream the split TAR, and import detailed readsb traces directly. These stages need no external Parquet export or Python. The public archive is global: the date selects a download, while the requested UTC interval and area filter it locally. September 8, 2026 requires 3,874,212,864 bytes from the preferred `staging-0` release. Preferred means the provider's selected archive, not proven complete aircraft coverage.

```powershell
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe plan-adsblol --request cases/adsblol_request.json --out data/adsblol/new-plan
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe fetch-adsblol --plan data/adsblol/new-plan/download-plan.json --cache data/adsblol/new-cache
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe prepare-adsblol-import --manifest data/adsblol/new-cache/archive-manifest.json --config cases/adsblol_trace_import.json --out data/adsblol/new-import-request.json
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe import-adsblol --request data/adsblol/new-import-request.json --out output/new-native-tracks
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe terrain-grid --config cases/native_terrain.json --out output/new-native-terrain
```

Use new planning/import/terrain output paths. Inspect the plan's size before fetching. Acquisition uses bounded HTTP ranges, preserves partial files for retry, checks exact response ranges and full final hashes, and reuses a matching completed cache offline. Conflicting caches fail rather than replacing evidence. `max_download_bytes` is a hard budget. Raw provider metadata, selected original traces, rejection counts and input hashes remain in receipts. Corruption or exhausted import budgets fail explicitly; processing a whole archive does not prove every flight was received.

The example import explicitly selects **20:00–22:00 UTC** and the existing development region. It is an adapter validation with source-informed date/time/area, not a new blind search. Change `time_window_utc_s`, `time_origin_utc_s` (the selected day's midnight), and south/west/north/east `bounds` for another case. Use one request per day. Geometric altitude is converted from feet above the WGS84 ellipsoid. Missing geometric heights, stale observations, disallowed sources, ground points and broken passes are recorded. Barometric approximation requires a separate explicit policy; sparse aircraft metadata never fills missing heights.

Native terrain reading supports local north-up WGS84 GeoTIFFs with bounded tile caching, bilinear sampling, nodata checks and recorded vertical-datum assumptions. Point `normalized_tracks` and `observers_file` in existing search geometry at the new `normalized.json` and `observers.json`. Explicit bounded terrain downloads are available in the Data step; seamless mosaics and global coverage remain unfinished. The historical example retains the previous Copernicus/EGM96 approximation for comparison; it does not claim an exact vertical-datum conversion.

The complete September 8 archive was scanned: 79,044 aircraft traces and 106,492,920 source rows. The explicit two-hour regional selection retains 428 aircraft, 1,008 passes and 228,061 measured points, without barometric fallback. Independent checks compare every retained point against its original raw row and all 2,856 observer heights against the previous independent sampler. Timestamp collisions and reversed-time overlaps exclude all ambiguous rows and break support; rows are never sorted or selected arbitrarily to hide ambiguity. See `output/native-data/replay-002/retained-points-audit.json` and `terrain-comparison.json`. Real HTTP-range resume is verified in `output/adsblol-research/native-resume/receipt.json`.

The complete native replay finished in 335.66 seconds on this machine, recovering `a1dfeb` / `ace391` from 13,810 ranked hypotheses across 95 ordered pass pairs. Its initial 200 m grid leader was **40.876800, -73.523818**, RMSE **0.6771 Hz**, with event two at its frequency limit. The subsequent fresh refinement and three widening stages in `output/benchmarks/search-sensitivity/native-001/` now establish the conditional stability described above. `output/native-data/replay-002/report.json` and `results.json` retain the frozen validation-006 executable identity. The earlier replay-001 preserved the same predictions but correctly failed its final input-immutability check because its original build path was rebuilt concurrently; replay-002 is the completed native import run.

### Preserved reference replay

The [DOPPLERGEIST checkpoint](https://github.com/1337isnot1337/DOPPLERGEIST/releases/tag/checkpoint-2026-11-02) provides detailed regional ADSB.lol trajectories and terrain. The downloaded archive and selected members were verified against its pinned SHA-256 manifest. See `data/dopplergeist/2026-09-08/import-provenance.json` and `data/dopplergeist/provenance/`.

The preserved reference Parquet/GeoTIFF import is a research helper using Python with numpy, pandas, pyarrow and Pillow. It creates portable JSON inputs for Rust search. The native route above provides a raw-archive alternative; older replay artifacts remain unchanged for regression comparisons.

```powershell
python analysis/import_reference.py --parquet data/dopplergeist/2026-09-08/reference-region.parquet --out-dir output/reference-data-new --time-origin-utc-s 1788825600 --max-gap-s 30 --max-speed-mps 330 --barometric-policy approximate --barometric-offset-m 0 --barometric-uncertainty-m 150 --bounds 40.83 -73.59 40.92 -73.46 --grid-spacing-m 200 --microphone-height-m 3 --dem data/dopplergeist/terrain/Copernicus_DSM_COG_10_N40_00_W074_00_DEM.tif --geoid data/dopplergeist/terrain/us_nga_egm96_15.tif
```

Update the normalized/observer file paths in a copy of `cases/reference_search.json` when using a new import directory. The existing verified import is already configured:

```powershell
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe prepare-search --config cases/reference_search.json --out output/reference/search_input.json
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe rank --input output/reference/search_input.json --out output/reference/results.json
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe overlays --input output/reference/search_input.json --results output/reference/results.json --out output/reference/overlays.json --top 3
python analysis/plot_results.py --overlays output/reference/overlays.json --results output/reference/results.json --out output/reference/figures --reference-file cases/reported_reference.json
```

The reference coordinate is loaded only by post-search evaluation/plotting. Search bounds and the choice of audio events are acknowledged prior information. Every supplied aircraft remains eligible subject to documented data-quality exclusions. The source regional dataset itself is externally prepared and is not proof of complete national coverage.

## What was established from the source recording

- The raw recording is 3,600.2075 seconds of 44.1 kHz stereo AAC. The channels are effectively identical; the program still selects channels explicitly rather than averaging.
- Original timeline = downloaded recording timeline + 3,600 seconds. Both matching curve shapes and the displayed `60to120min` source filename support this mapping.
- Guide anchors 5,752 / 6,673 seconds therefore become 2,152 / 3,073 seconds. Independently traced ideal-pass fits give approximately 2,156.54 / 3,077.54 seconds, a 920.994-second separation. Guide anchors remain unchanged in the fitting configuration.
- September 8, 2026 is the user's working date, supported by the external historical replay and the actual flight match. The September 13 timestamp in a displayed filename was not treated as a capture date.
- The Rust numeric spectrogram matches an independent SciPy calculation to within 0.000003815 dB at the checked frames. Files under `output/audio-validation/` document sampling, tracing, uncertainty, and the comparison.

## Scientific model and limits

The Rust core uses WGS84 ECEF positions, continuous segment CPA, subsonic piecewise-linear trajectories, and solves each sample's emission time from its arrival time. It rejects extrapolation, invalid motion and uncovered event samples. One shared recording clock applies to both events; each aircraft has its own emitted-tone sweep.

The preserved regional replication uses equal-event weighted **traced-ridge RMSE in Hz**, lower is better. This is an explicit reconstruction choice; it is not the creator's unpublished brightness normalization and cannot reproduce the displayed 20.48 score numerically. Traces were selected from the recording before evaluating historical models. Their nominal 1 Hz measurement uncertainty is not a calibrated confidence interval. The interactive workspace separately reports residuals normalized by its selected uncertainty floors and user weights.

Ground heights use the regional DSM plus EGM96 geoid and an assumed 3 m microphone height. Source height types and unused velocity fields are retained. A small set of barometric fallback rows receives an explicitly declared approximate treatment. The leading pair uses source-labelled ellipsoidal heights. Native position jitter, interpolation, terrain/geoid differences, atmosphere, propeller RPM and clock calibration remain material uncertainties.

Tracks are split at gaps and implausible jumps. Pair search enumerates continuous local closest approaches, including distinct return passes by the same aircraft, and rejects truncated boundary passes. Silence rejection is disabled: incomplete reception or different acoustic audibility makes it unsafe to treat every missing pass as certain exclusion. No numeric score is a calibrated location probability.

## Repeat the DOPPLERGEIST-derived validations

```powershell
python benchmarks/run_validation.py --output output/benchmarks/validation-new
```

Use a new output folder. The suite builds the portable Rust executable, runs native tests and checks, exercises all 16 pinned decision fixtures, audits codec/audio timing, and reruns the real regional and faint-event cases. It preserves inputs, hashes, exact commands, timings and a `summary.json`. See [benchmarks/README.md](benchmarks/README.md) for prerequisites, frozen thresholds and scope; [benchmarks/upstream_catalog.json](benchmarks/upstream_catalog.json) maps all upstream test files and explicitly identifies unsupported stages.

The `rank-single` command adds a known-clock single-event search with full fixed-ridge support. The `decide` command evaluates supplied coverage/ambiguity/sensitivity evidence using the pinned engineering rules. Passing these constructed decision controls does not turn a raw search score into calibrated confidence: both native search commands still return unverified hypotheses. Discovery has its own independently recorded [benchmark and workflow](benchmarks/DISCOVERY.md); it does not automatically replace the manually reviewed traces used in the regional location experiment.

## Discover and review events

```powershell
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe discover --input "Known Videos with coordinates/Unknown location - raw audio - colsto (144p, h264).mp4" --out output/discovery/new-recording
```

Open `review.html` in the new output folder. Inspect each group and its measured ridges, then mark it **Keep for matching**, **Reject**, or **Uncertain** and save a separate review JSON. Harmonics remain one possible flyover; missing measurements stay missing. The review file records human selection; the [reviewed-search workflow](benchmarks/REVIEWED_SEARCH.md) describes the explicit preparation needed for matching. The report and per-ridge CSV files preserve the source hash, selected channel, exact measurements and settings. A new output folder is required on every run.

Discovery scans the complete recording with no event windows, flight data, dates or location hints. FFmpeg decodes the selected channel and performs resampling; the spectral analysis, tracking, quality checks, stitching and harmonic grouping run in Rust. The source is never modified. Corrupt or partial decoding fails explicitly.

## Saved cases and resumable jobs

Saved cases identify a recording by SHA-256 and retain local source references,
audio metadata and append-only narrative clues. A clue never silently changes
the search box, capture clock, event selection or a verification verdict.
Recording bytes are not copied or moved. Relative references remain usable when
the complete case/source folder layout moves together.

```powershell
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe case-create --input "recording.mp4" --title "My recording" --out data/cases/new-case
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe case-status --case data/cases/new-case/case.json
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe case-add-hint --case data/cases/new-case/case.json --hint hint.json
```

A hint file contains `kind` (`region`, `date_time` or `other`), `text`, `source`
(`user`, `embedded_metadata` or `analyst`) and `certainty` (`uncertain` or
`reported`). There is no `verified` hint category. Changing the immutable case
manifest invalidates its old hint bindings; create another case revision instead.

The initial job stages are inspection, discovery, native terrain grids, pair or
single-event ranking, and checked overlays. The supplied
`cases/original_native_job.json` reruns the already prepared native regional
search, then passes that attempt's results to overlay generation:

```powershell
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe job-create --plan cases/original_native_job.json --out output/jobs/new-replay
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe job-run --job output/jobs/new-replay
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe job-status --job output/jobs/new-replay
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe job-cancel --job output/jobs/new-replay
.\dist\GeoSonnar-Windows-0.8.0\geosonnar.exe job-run --job output/jobs/new-replay --resume true
```

Cancellation is requested from another terminal while a job runs. Each job
freezes its stage executable, relevant settings and existing case clues.
Every attempt has its own directory, commands, logs, input identities and output
hashes. Resume reuses complete, unchanged work; damaged or unfinished output
gets a new attempt, and subsequent stages rerun. Previous attempt files remain.
Changed recordings, terrain or bound configuration inputs require a new job.
New case clues do not rewrite an older job's context. Job completion remains
separate from scientific support, including a valid `no_candidate` result.

Only one runner owns a job. After an interrupted process, `job-recover --job DIR`
retires its lock only when the recorded runner and stage processes are proven
dead. A live or uncertain process identity prevents recovery. A later explicit
resume starts unfinished stages again; it does not restart halfway through a
numerical search. Exit code 4 means a cancelled job.

Media jobs require explicit real executable paths under `media_tools.ffprobe`
and, for discovery, `media_tools.ffmpeg` in the plan. Paths are plan-relative or
absolute. The runner copies and hashes these tools and checks their version
command; Chocolatey launch shims are not the actual decoder and are rejected.
Prefer static FFmpeg builds: dynamic libraries and operating-system components
are not bundled. Pure ranking, terrain and overlay jobs need no media helpers.

The GUI uses this job foundation for discovery and prepared processing plans.
Its manual workspace additionally handles media review, managed data downloads
and regional import, map inspection and explicit evidence comparisons.
Acoustic-prior preparation and broader automatic pair refinement retain their
existing command workflows. A supplied search JSON must belong to the intended
recording; attaching it to a case does not independently prove its provenance.

**Save review + traces** creates a new `CASE/workspace-reviews` revision with
`workspace.json` and trace exports. Decisions, source hashes, event/term choices,
UTC/altitude interpretation and model assumptions are retained.
**Save comparison + evidence snapshot** also stores the report. Reopening verifies bound sources;
relative case-owned layouts can move together. The earlier harmonic-group
reviews under `CASE/reviews` remain a separate input to their existing commands.

The real job demonstration in `output/jobs-validation/live-001/` cancelled a
running native regional search, preserved its first attempt, resumed into a
second attempt, generated dependent overlays, and then reused both completed
stages without new attempts. All 13,810 candidate rows matched the original
native replay. A separate media inspection job used a copied real FFprobe.
The 16 fabricated job controls also test crash recovery, startup ownership,
source/output/log tampering, failed retries and artifact dependencies.
Jobs retain their original external input paths; relocating those inputs may
require a new job even when a saved case's relative layout remains valid.

## Project structure and next milestones

- `src/audio.rs`: streaming channel-specific decoding and FFT; numeric `.gspec`, metadata and preview.
- `src/discovery.rs`, `src/discovery_groups.rs`: automatic measured-ridge proposals, fragment handling and harmonic grouping; `src/discovery_review.html`: standalone review/export report.
- `src/acquisition.rs`: date/area requests, pagination, limits and cache.
- `src/adsblol_acquisition.rs`, `src/adsblol.rs`, `src/detailed_data.rs`: official daily archive planning, resumable fetching, source-bound streaming import and case preparation.
- `src/terrain.rs`: native GeoTIFF sampling and observer-height provenance.
- `src/data_catalog.rs`, `src/map_data.rs`: case-owned copies/downloads, safe deletion, native geographic layers and explicit timeline support.
- `src/review_media.rs`, `src/workspace.rs`, `src/gui/studio/`: measured spectral/video review, manual traces, immutable assumptions, maps and the data manager.
- `src/thunder.rs`, `src/evidence_score.rs`, `src/workspace_scoring.rs`: experimental energy proposals and explicitly selected aircraft/strike residual comparisons.
- `src/casework.rs`, `src/jobs.rs`: local saved cases, append-only clues, frozen job context, cancellation, recovery and verified stage reuse.
- `src/normalization.rs`: explicit units, height assumptions, gap splitting and provenance.
- `src/physics.rs`: propagation and shared-clock scoring.
- `src/single.rs`: fixed-clock single-event search; `src/decision.rs`: engineering evidence checks.
- `src/search_setup.rs`, `src/verification.rs`, `src/overlay.rs`: regional grids, hypothesis ranking and checked model overlays.
- `cases/`: measured event inputs and reproducible request/search configurations.
- `analysis/`: independent inspection, reference-data import and plots. These helpers are not required by the Rust numerical executable.

Reviewed automatic ridges now connect to the flight search through `prepare-reviewed-search`; see the [reviewed-search workflow and comparison](benchmarks/REVIEWED_SEARCH.md). The bridge requires an explicit ridge from each of two kept groups, preserves measured gaps, checks recording/review hashes, and keeps clock, frequency and region assumptions visible. Missing or incomplete signals still need manual review. Automatic terrain-height use in scoring, the guide's spectral scoring and independently timed/surveyed recordings remain work for the broader pipeline.

The newer `derive-audio-priors` / `prepare-audio-search` route estimates the
acoustic priors from those reviewed ridges. `analysis/refine_automatic.py`
refines its newly ranked pairs rather than inheriting the manual fine grid.
The bounded frequency/spatial sensitivity runner adds fixed outer grid rings,
reranks every supplied track and retains prior refinement points and competing
pairs. It keeps numerical stability separate from location accuracy; see
[the sensitivity policy and results](benchmarks/SEARCH_SENSITIVITY.md) and
[the original audio-prior workflow](benchmarks/AUDIO_PRIORS.md).

The Windows workspace connects recording review, data acquisition, map selection
and explicit evidence comparisons. Version 0.3.0 includes optional straight-path
atmospheric profiles; weather-provider downloads and refracted propagation remain
unfinished. The subsequent [guided regional workflow](docs/REGIONAL_WORKFLOW_PLAN.md)
is adding shared uncertain-clock searches, reviewed alternative associations and
source-bound terrain/geoid receiver heights. Its validation and independent field
evidence are separate from the historical regional replication. Date-line support
and worldwide performance remain unestablished. A finer search grid does not turn
coarse source data into finer location accuracy.

## Experimental lightning/thunder evidence

The workspace proposes low-frequency energetic soundprints, allows manual
thunder intervals/arrival estimates and imports timestamped strike exports.
These proposals are threshold-based energy diagnostics, not a trained thunder
recognizer. A selected strike and reviewed arrival can contribute a timing
residual using the same recording UTC clock as aircraft evidence. Missing strike
height needs an explicit fallback; supplied heights need a declared datum.
Per-term uncertainty floors and user weights normalize the comparison. These
scores are not calibrated probabilities, and combined scoring has not established
an accuracy improvement on independent recordings.

Use an authorized export with provider/coverage/terms information. The Data
page accepts native JSON/CSV and bounded HTTPS export downloads. CSV requires
`id,time_utc_s,latitude_deg,longitude_deg`, with optional `altitude_m,intensity`;
JSON uses `{schema_version:1,strikes:[...]}` with the same fields. Times are Unix
UTC seconds. A provider's original format may require an adapter.
[Blitzortung's computed-strike archives](https://www.blitzortung.org/en/compendium.php)
have participant access and use restrictions. This interface does not promise
an anonymous global historical archive or start continuous collection.

The present timing model treats strikes as points and uses a declared sound
speed. Extended channels, weather, gain/clipping and unrelated low-frequency
sounds remain limitations. Next validation should compare Doppler-only,
thunder-only and combined results on consented recordings with independent
time/location truth withheld until predictions are saved. Broad searches and
weather-profile propagation need separate controls and resource budgets.
The private validation clip is not assumed to contain usable thunder or aircraft
evidence; missing provider coverage is not negative evidence.

## Sources and attribution

The supplied `Aircraft_Audio_Replication_Guide.pdf` is a reconstruction of colsto's explanatory video, not a supplied scientific paper or original implementation. Its referenced `doppler_core.py` was not present in this folder; this Rust core was independently implemented from the described equations.

- [colsto explanation](https://www.youtube.com/watch?v=UytYrhYLOKI) and [raw recording](https://www.youtube.com/watch?v=4CxVxuMVkMc).
- [DOPPLERGEIST](https://github.com/1337isnot1337/DOPPLERGEIST): methodology, checksum-verified external reference data, and adapted MIT-licensed validation/decision rules. Its own claims distinguish configured regional replay from blind validation. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the pinned revision, adapted areas and license.
- [franken_doppler_finder](https://github.com/Dicklesworthstone/franken_doppler_finder/tree/2d0362f13b7634eb9594fba34b166279fc53b0eb), reviewed at `2d0362f13b7634eb9594fba34b166279fc53b0eb`: independent reference for staged caches/reports, Doppler-template proposals, alternative explanations and historical atmosphere acquisition. No code was copied or run. Its nonstandard license includes an additional restriction; it is not treated as an ordinary MIT dependency. Its same-site successes and wrong low-confidence field results motivate held-out validation, not an accuracy guarantee. Proposed follow-ups are an independently implemented second detector and measured weather sensitivity after the case/job foundation. Preserve our explicit altitude policies rather than adopting its barometric/geoid defaults; its conditional confidence calculations are not calibrated worldwide coverage. It supplies no lightning pipeline.
- [ADSB.lol historical archive](https://www.adsb.lol/docs/open-data/historical), [adsb.aero documentation](https://adsb.aero/api/docs) and [provider guide](https://adsb.aero/llms.txt). Flight data retains its ODbL terms; see the saved source licenses.
- Terrain and geoid sources/terms are retained under `data/dopplergeist/provenance/` and in each observer-height report.
- The embedded [Natural Earth](docs/NATURAL_EARTH.txt) v5.1.2 country outlines are public-domain map context. Optional street tiles credit [OpenStreetMap contributors](https://www.openstreetmap.org/copyright) and follow the [tile usage policy](https://operations.osmfoundation.org/policies/tiles/).

No source recording, flight/strike/terrain dataset or FFmpeg-family binary is
bundled. The small Natural Earth context map is embedded, with its notice.
