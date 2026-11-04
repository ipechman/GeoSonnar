# Executable field-reference validation

Reviewed November 3, 2026, America/New_York. This work acquires real audio and independent flight observations for a regional replay. It does not substitute another program's estimated coordinates for receiver truth, and it does not turn a successful download into an accuracy claim.

## Selected primary recording

[Eclipse Soundscapes ESID 103](https://zenodo.org/records/22049202), version 2024.0.0A, supplies raw AudioMoth audio, a device identifier, publisher deployment metadata and a UTC clock convention under CC BY 4.0. The publisher reports a deployment at **43.79568, -72.18011**, independently of aircraft matching. The metadata links that position to ESID 103, attributed to a Nationwide Eclipse Ballooning Project volunteer scientist. This is a primary deployment claim, rather than a coordinate inferred by DOPPLERGEIST or GeoSonnar.

The predeclared excerpt is the first 600 seconds of `ESID_103_2024_04_08/20240408_042453.WAV`. Its preserved RIFF comment explicitly says it was recorded at **04:24:53 on April 8, 2024 UTC** by AudioMoth `242A26046037F967`. Thus the clock claim exists in the original waveform header as well as its filename. Publisher metadata reports automated AudioMoth time-chime setup. The supplied field temperature is a device reading, not an independently calibrated atmospheric profile.

The complete excerpt is `data/validation/field-esid103-004/audio/field.wav`: 57,600,044 bytes, 48 kHz mono 16-bit PCM, SHA-256 `9738d99a2881233d4f040c8431fd5a238a1c1d5c721307bfef0ddadd12a0e56f`. Its receipt and original header are adjacent. Acquisition reused verified ranges retained in `field-esid103-003/source/ranges/`; that cache and its original range journal are part of the provenance. Earlier interrupted prefix outputs are not validation inputs.

Two uncertainties remain: the publisher does not quantify the microphone-coordinate registration error or the actual recorder clock error. [Project registration instructions](https://eclipsesoundscapes.org/my-audiomoth-location/) permit registering nearby when cellular service is unavailable. The [record description](https://zenodo.org/records/22049202) warns that raw records may contain clock failures. Compare a finished result with the published deployment point, retaining both uncertainties explicitly; do not advertise a surveyed sub-100-m accuracy test.

This is a **source-informed reference replay**: current DOPPLERGEIST [candidate notes](https://github.com/1337isnot1337/DOPPLERGEIST/blob/main/docs/ECLIPSE_FIELD_CORPUS.md) reported a descending band at this site. Its event windows, fitted curves, aircraft IDs and estimated coordinates are not inputs to our discovery. A different interval or deployment, selected before observing local detector results, is required for a new held-out trial. No upstream implementation was copied.

## Small matching aircraft download

The public ADSB.lol map publishes the same historical routes it uses for playback:

```text
https://adsb.lol/globe_history/YYYY/MM/DD/heatmap/NN.bin.ttf
https://adsb.lol/globe_history/YYYY/MM/DD/traces/XX/trace_full_ICAO.json
```

`NN = 2 * UTC hour + floor(UTC minute / 30)`; `XX` is the final two hexadecimal characters of the aircraft ID. These routes were read from the actual [provider map JavaScript](https://adsb.lol/script_22b772ac55265ba79eeeca99ab34165c.js), and then successfully requested. No account, API key or Frost access was used.

The completed acquisition at `data/validation/field-esid103-tracks-002/` used **13,223,253 HTTP payload bytes**, including two half-hour replay files, source-format evidence and **19 full-day aircraft traces containing 42,604 points**. The candidate index contained 615 position records during 04:20–04:40 UTC in the predeclared broad rectangle `[42.5, -73.5, 45.5, -70.0]`. No aircraft-type filter or distance-to-truth filter was applied. The exact deployment coordinate was not used to select identities. Original per-aircraft JSON lives in `traces/`, ready for GeoSonnar's detailed-track importer. Original gzip replay responses and a source-code snapshot remain under `source/`; the receipt includes hashes.

The April 2024 replay files contain 180 ten-second time slices and a leading table of slice offsets. The decoder validates that legacy table, handles signed longitude/altitude and source-type flags, and checks timestamps against the requested half-hour. Replay's quantized barometric altitude is used only for candidate indexing; detailed traces retain their original fields, including geometric altitude when present. The source data retains its [ODbL 1.0 attribution](https://www.adsb.lol/docs/open-data/historical).

This avoids downloading the matching GitHub daily archive's **2,047,846,400 bytes**. No April 8, 2024 archive was already cached locally. The alternative adsb.aero service reported coverage beginning April 28, 2025, so it cannot serve this recording.

**Replay is not complete-sky evidence.** Ten-second sampling, receiver gaps, absent positions, untracked aircraft and non-ICAO targets can omit relevant candidates. No provider guarantee of low-altitude coverage was found. An empty index or missing trace must not become an unheard-aircraft penalty or proof of no flight. Raw source codes and the incomplete-coverage flag must survive import. Finding a flight in the area is not an acoustic association.

## Reproduction and validation steps

The isolated research helpers are `analysis/field_validation_acquire.py` and `analysis/field_validation_tracks.py`. They use fresh output directories, preserve provenance, cap transfers below 100 MB per run and do not open a GUI. The audio helper uses exact byte ranges and refuses full archive responses. It preserves source PCM sample rate, sample order and sample zero. It does not claim verification of a whole compressed member's CRC or SHA-512 when only a leading excerpt was fetched. It uses Windows' native downloader for archive ranges because this machine's Python HTTPS reader returned truncated bodies; failed attempts are retained separately.

```powershell
python -B analysis/field_validation_acquire.py --record 22049202 --duration 600 --output data/validation/field-esid103-new
python -B analysis/field_validation_tracks.py --output data/validation/field-esid103-tracks-new
python -B benchmarks/field_validation_tests.py -v
```

The eight acquisition checks cover byte-preserving ZIP access, bounded reads, cached-range tampering, RIFF metadata, legacy/current replay layouts, malformed markers, region/time filtering and bounded gzip inflation. Six additional artifact controls in `benchmarks/field_validation_artifact_tests.py` verify unchanged native measurements, ECEF points, pending review state, source segments, complete tiled receiver coverage and post-score distance calculations. All **14** passed headlessly; the optional artifact checks skip cleanly in source checkouts without the separately acquired recordings.

The actual shipped native CLI has now processed the entire excerpt with frozen defaults, without publisher coordinates, aircraft IDs, event windows or a manually selected frequency band. `output/field-validation-reference-001/summary.json` records one proposed ridge containing 20 measured points between 372.024 and 385.024 seconds, falling from about 558.69 to 518.49 Hz. All 19 acquired traces passed the native geometric-altitude importer, producing 42 separately supported passes with 1,901 points. These are successful real audio/import integrations, not a location verdict. The original native executable (SHA-256 `cef773b17145e4547c5c9f74844f40f79fa81cd48d6994cfe2c232d129c5c010`), command arguments, installed media-tool hashes and logs are retained. `spectrogram.png` is a separate waveform-derived review figure with the unchanged native ridge overlaid; no desktop was opened.

`benchmarks/field_validation_policy.json` was frozen before native discovery. `benchmarks/field_validation_search_policy.json` was frozen before fitting. The first regional request is `output/field-validation-reference-prepared-001/batch.json`, with all 42 passes, 288 receivers in the broad rectangle, seven shared clock offsets from −15 to +15 seconds, 20 km spacing, a 20–3,200 Hz analytic emitted-tone bound, fixed 343 m/s sound speed and an explicit assumed 200 m ellipsoid receiver height. The short descending fragment does not provide an independently reviewed closest-approach time, so only frequency evidence is used. `pending-workspace.json` retains the unreviewed/unselected operator annotations. The separate batch explicitly includes every proposal for a numerical experiment; its API eligibility flag is not a claim of human review or a confirmed aircraft identity.

The prepared request has no receiver-truth input and copies native ECEF positions, timestamps and segment boundaries without fitting or editing them. No-event and rejected-fit outcomes must be retained. Compare the final ranking against the separate publisher deployment marker only after scoring. Include misleading source alternatives and record ambiguity/abstention. Aircraft association must be earned from the audio, not supplied by this acquisition. A ±15-second clock window is a declared experiment, not a measured device-clock error bound; the assumed receiver height and constant sound speed remain material approximations.

The fresh time holdout is the first 600 seconds of `20240408_165034.WAV`, a different daytime interval at the same deployment. This selection and the identical detector/search rules were frozen before either local detector output or any fit. The complete waveform is `data/validation/field-esid103-holdout-001/audio/field.wav`, SHA-256 `0cd43968bdbf5fe89429cc3dad3437f2617feb9020187541f619d74a8183e017`. Its original header explicitly confirms 16:50:34 UTC. The daytime replay index has 204 regular ICAO candidates, plus four non-ICAO IDs. The first acquisition hit a declared 100-trace cap; `field-esid103-holdout-tracks-002/acquisition.json` completes all 204 regular candidates using the cached index, preserving the capped receipt. Audio plus all track acquisition used **82,231,718 HTTP payload bytes**. Even completion of that index cannot establish exhaustive aircraft coverage. This is a within-site time holdout, not a held-out deployment or a surveyed microphone benchmark.

## Completed real regional results

The frozen automatic-proposal baseline did **not** recover the publisher deployment point in either recording. This is useful negative validation: low residual scores and many retained assignments are insufficient to accept a location. Neither recording has matched lightning evidence; this lane therefore does not establish hybrid Doppler/thunder accuracy.

| Case | Native audio proposals | Native geometric observations | Top result distance to publisher marker | Result |
| --- | --- | --- | --- | --- |
| Source-informed 04:24:53 reference | 1 ridge, 20 points | 19 downloaded aircraft; 42 passes, 1,901 points | 102.068 km | Failed the explicitly reported 25 km regional comparison |
| Frozen 16:50:34 time holdout | 10 ridges, 388 points | 204 downloaded aircraft; 182 with retained geometric support, 546 passes, 11,956 points | 56.059 km | Failed the same comparison |

The native regional adapter is SHA-256 `b9a422609a0b2c04798ee04a3852503642b7ca64f5ed9efb9b01848a684d5271`, preserved under `output/benchmarks/regional-workflow-001/regional_search-fixed.exe`. The reference result is `output/field-validation-reference-search-001/eclipse-reference.json`; its separate `publisher-evaluation.json` verifies input/report hashes before reading the publisher coordinate. The best row is at 44.296622, −73.250240, with score 2.70865 and a +5-second clock adjustment. The nearest retained grid point is 7.104 km from the publisher marker, ranked 425; 7.104 km is also the nearest point available on the declared grid. Refining only the distant top result would therefore risk reinforcing an incorrect association.

The holdout's unchanged all-proposal request exceeds one native job's source/residual limits. `analysis/field_validation_tiles.py` stores each aircraft's exact native passes as disjoint segments, reducing 546 pass identities to 182 ICAO alternatives without joining gaps or changing coordinates. **Grouping changes association scope:** one event may now use supported samples from multiple observed passes of the same aircraft; a pass-level alternative required one pass to support the entire event. Thus exact point preservation does not imply identical hypothesis spaces. The failed holdout result belongs to this explicitly grouped policy; original frozen receipts are retained unchanged.

The grouped request is then partitioned across the **same 288 receivers** into 18 disjoint groups of 16. Every tile retains all ten ridges, all clock samples, all aircraft and the original frequency limits. This latter tiling changes scheduling only. The research adapter does not claim that the GUI currently accepts the aggregate workload as one job or groups sources automatically. All original requests, transformations, native reports, source hashes and global receiver indices are retained under `output/field-validation-holdout-tiles-001/` and `output/field-validation-holdout-search-001/`.

All 18 searches completed in 118.26 seconds of native runtime, totaling 3,669,120 event/source/receiver/clock evaluations. A separate research merge computes the global lowest-score 1,000 rows from each tile's retained 1,000. `analysis/field_validation_evaluate_tiled.py` verifies every native input/report, original observations, receiver coverage and merged scores before reading truth. The best row is at 43.398311, −71.751681, score 3.67178, with a +5-second clock adjustment.

**All 1,000 retained holdout rows occupy one receiver cell**, differing in their source assignments. This is assignment crowding, not evidence of narrow geographic certainty. Native supported-assignment counts exceeded the integer range and remain explicitly unknown; no count is reinterpreted as confidence. Retaining spatially distinct receiver/time alternatives is an important next reporting improvement. The automatic ridges can also include non-aircraft sources or correlated harmonics; they were deliberately included unchanged for this baseline, with human-review flags still false in their diagnostics.

The appropriate next real-data experiment is an **audio-only review**: independently inspect/trace sustained tonal evidence and reject non-aircraft-looking proposals before consulting trajectory fits or truth. Record that as a separate exploratory revision, retain this failed baseline, and validate the review rules on another deployment or untouched interval. Tuning traces, clock limits, frequencies or map bounds until this known site wins would not be an independent validation. Receiver-height and clock uncertainty should also be assessed before claiming precise accuracy.

Cleanup removed five incomplete generated-waveform or hash-identical failed-attempt duplicate files, reclaiming 41,638,169 bytes. The deletion receipt is `output/field-validation-reference-001/cleanup.json`. Canonical audio, primary metadata, original range cache/journals, raw flight observations, all native results and the independently stored truth remain intact. No application windows were opened.

## Fresh deployment: audio review before aircraft matching

A separately selected deployment is now ready for an audio-only review. Selection started with the next indexed site IDs after ESID103; it did not use upstream aircraft fits or inspect spectra. [ESID105](https://zenodo.org/records/20835101) was excluded from recorder metadata before audio review: its CONFIG declares a 6–18 kHz band-pass and amplitude-triggered `T.WAV` files. Official [AudioMoth documentation](https://www.openacousticdevices.info/config-app-guide) explains that triggered recordings remove quiet intervals and require expansion to recover their original timing. Treating those files as ordinary continuous audio would make reception-time matching invalid. The excluded attempt and original policy remain under `data/validation/field-newsite-001/` and `benchmarks/field_validation_fresh_policy.json`.

The primary catalog identified [ESID108](https://zenodo.org/records/20835484) as the next available numeric deployment; ESID106 and 107 were absent from the queried catalog. This is a different deployment from ESID103. The approved metadata-only interval amendment uses the first 600 seconds of the earliest April8 ordinary WAV, starting **2024-04-08 12:11:56 UTC**. The initial after 16:00 file-start rule found no eligible member: the only April8 source file begins earlier and lasts over 12 hours. Seeking 16:00 inside its compressed stream would exceed the transfer budget. The amendment was frozen before waveform review, detections or flight matching in `benchmarks/field_validation_fresh_site108_policy.json`.

The original CONFIG describes 55-second recording/5-second sleep, but the actual long files have ordinary PCM headers and file-size termination comments. Original headers immediately before, at and after the selected file each declare 44,739 seconds of PCM; adjacent header timestamps differ 44,741 seconds. The official [firmware 1.8.1 source](https://github.com/OpenAcousticDevices/AudioMoth-Firmware-Basic/blob/1.8.1/src/main.c) sets unlimited recording duration in DEFAULT switch mode, then closes and restarts at the WAV size limit. These independently acquired boundaries support a continuous source timeline with 2-second inter-file restarts, rather than omitted 5-second gaps every minute. DEFAULT mode is an inference, not a logged switch position. Absolute clock error and sample-clock drift remain unquantified. The selected excerpt contains only one original file, so no restart boundary or concatenation enters its samples. Header bytes, sample counts, source hash and reasoning are preserved in `field-newsite-108-metadata-001/clock-continuity.json`.

The isolated review directory is `data/validation/field-newsite-B-001/review/`. It contains:

- `recording.wav`: 600 seconds, 48 kHz mono 16-bit original PCM, SHA-256 `2433d3067183154b5cb8b9fa410ac4f1a105092e9b255c570477084368f6ced7`.
- `audio-only.json` and `clock-continuity-review.json`: original UTC claim, unquantified errors and the coarse regional rectangle, with no exact publisher coordinate or aircraft identity.
- `spectrograms/overview.png` plus five 120-second pages: raw power and separately labeled frequency-median contrast, without detector, flight or fitted-curve overlays. `review-assets.json` records display settings and file hashes.

The acquisition agent derived only a coarse region for the reviewer from primary metadata. This is **reviewer-blind audio interpretation with source-informed acquisition**, not strict experiment-wide blinding. The root reviewer must freeze accepted/rejected intervals and trace anchors before opening matching aircraft identities or fitting results. Mapping metadata and exact deployment truth remain separate. The predeclared interval will not be replaced if it lacks usable evidence.

Matching historical observations were acquired in parallel for the frozen coarse rectangle and 11:55–12:25 UTC; all 101 regular ICAO candidates from the sampled replay index are preserved outside the review directory. That does not establish complete sky coverage. The recorded scientific payload, including metadata exclusions, original headers, waveform and aircraft data, is **78,475,447 bytes**, below the authorized 150 MB budget. Review selection was frozen before aircraft identities were opened; predictions were subsequently committed before reading the exact publisher location.

### Frozen reviewed trial result

The assistant reviewed all audio-only pages and froze one sustained, approximately 150 Hz tonal ridge with **40 measured points**. Related bands were explicitly treated as possible harmonics rather than independent evidence. Aircraft identity remained unknown; no closest-approach or thunder timing was asserted. `benchmarks/reviewed_field_protocol.json` (SHA-256 `e06bfef70129f803488c12108d2551055b789c580a116b93576da0ce4250c3bc`) binds the review, measurements, exclusions and search assumptions before aircraft or exact-location access. The pending operator annotation remains unconfirmed; execution uses an explicitly labeled experimental copy, not a human-review claim.

All 101 raw aircraft traces were imported with the preserved native CLI. Of 114 geometric passes, 95 overlapped the declared observation/clock interval plus the fixed 600-second acoustic look-back. The other 19 failed that timestamp-only eligibility rule. Every eligible pass retains its original ECEF points, timestamps and segment boundary; **no ICAO grouping** was used. The frozen batch is `output/field-validation-fresh-B-prepared-001/batch.json`, SHA-256 `bfd693363f68ce9a0b2d77a52412436a48e6055f0e38df93ddadaac644a5b648`.

The search evaluated 130 receivers at 20 km spacing, 25 common clock adjustments across ±120 seconds, all 95 eligible passes and the unchanged 40 points. Its declared emitted-tone range was 20–3,200 Hz, sound speed 343 m/s and receiver ellipsoid height 200 m. These remain explicit engineering assumptions. The preserved location-reporting native adapter (SHA-256 `e4de3107ee3321fea28ff45f2e69d700db576c099033dcd033da597de964718d`) completed 308,750 source evaluations and 12.35 million frequency residual evaluations in **34.45 seconds**, within one job's native limits.

`output/field-validation-fresh-B-search-001/prediction-commit.json` binds the completed inputs and native report before the separate evaluator opens publisher truth. Its `publisher-evaluation.json` records the result:

| Measure | Fresh reviewed deployment B |
| --- | --- |
| Top distinct receiver error | **80.984 km** |
| Top distinct receiver score / clock | 1.1511 / +60 seconds |
| Closest of top 10 distinct receivers | 26.187 km; outside the frozen 25 km tolerance |
| Closest of top 20 distinct receivers | 26.187 km; outside the same tolerance |
| Closest available grid point | 9.177 km from publisher, ranked **85 of 130** |
| Native supported hypotheses | 206,437; the retained assignment list is truncated |
| Supported receiver/clock combinations | All 3,250; 102,313 individual source alternatives unavailable |
| Declared rectangle area | 36,169 km² |
| Area within best score +1 | 19,519 km²; a descriptive threshold, **not** a confidence region |

The top receiver is 43.257636, −79.508686, compared with the independently published deployment marker at 42.866523, −78.667772. The full distinct-location summaries prevent assignment crowding from hiding geographic alternatives, but they do not resolve this recording. The single tone's source identity, recorder-clock error, receiver-height assumption, source coverage and publisher coordinate uncertainty remain material. There is **no independent proof that the selected tone is an aircraft**. This is a failed candidate-location experiment, not a verified positive aircraft-detection trial; its failure also does not establish that the sound is non-aircraft. Preserve it as a misleading-signal challenge with unresolved source identity. The reviewed trial **failed** its declared regional comparison; no location is accepted, and its measurements or bounds were not retuned after truth was revealed.

The initial **source-recognition** recommendation was a balanced public [AeroSonicDB v1.1.2](https://zenodo.org/records/10215080) test-split subset. Inspection found that its ordinary negative clips last ten seconds, shorter than our unchanged default twelve-second minimum ridge duration. They therefore cannot provide a meaningful default-detector negative test as-is. Two complete environmental hours have instead been acquired and evaluated with frozen native defaults and the same five-second audibility metrics. The first produced 51 clean-negative proposal groups; the previously unseen second hour produced zero, while both covered only two-thirds of annotated audible runs. [Detection validation](DETECTION_VALIDATION.md) records all denominators, excluded groups, byte budgets and limitations. These are temporal source-presence controls, not proof of each ridge's source. External site/session independence is unresolved; privacy-shifted coordinates and missing exact UTC prevent geolocation validation. The ordinary session-disjoint split remains useful for a separately declared review/classification test with duration-appropriate criteria, not a retroactive replacement for either frozen hour. Dataset assets remain separately licensed CC BY-NC 4.0. A location-accuracy acceptance test still needs confirmed acoustic evidence, independently bounded microphone coordinates and a supported sample-zero clock.

## Concrete telemetry-aligned reference lead

The [Western Sydney University UAV Acoustic Localization Dataset](https://researchers.westernsydney.edu.au/en/datasets/uav-acoustic-localization-dataset-24-channel-beamformed-recording/) supplies multi-day DJI Air 3 recordings, GPS flight logs and per-recording alignment parameters. Its [repository record](https://research-data.westernsydney.edu.au/published/1f8cceb0801b11f189a547c836cf5bee/) declares open access under **CC BY-SA 4.0**. The [companion paper](https://arxiv.org/html/2508.00307v1) independently describes a stationary reference log recorded at the microphone array, rather than a receiver coordinate inferred from an acoustic fit. This makes it a substantially stronger reference-integration lead than the unverified-source Eclipse tones. The initial metadata-only plan is retained; the subsequent frozen audio integration and known-receiver consistency check are recorded below. No location search has been run for this lead.

Primary files have been inspected for `dataset/test_1/Oct_11_2024/flight_5/`:

- The 579,075-byte flight CSV has **1,631 positions**, every adjacent elapsed-time increment exactly 100 ms, explicit `datetime(utc)`, latitude, longitude, altitude above sea level and velocity fields. Its UTC strings span 2024-10-11 04:49:13–04:51:56 and have only whole-second resolution.
- The separate 402,253-byte reference CSV has 376 rows. The provider's published loader uses its mean latitude/longitude as the array origin and its first sea-level altitude. This is a documented GNSS reference, not a surveyed microphone accuracy certificate.
- `alignment_params.json` maps original audio second **82.0** to flight row **39** (elapsed flight time 3.9 seconds); device 1's sample correction is zero. The published loader advances one 100-ms audio block per GPS row. Absolute audio/GPS alignment error is not quantified. The paper's ±1-sample claim concerns synchronization **between recorders**, not absolute GPS-to-audio accuracy.
- Verified HTTP ranges establish that `device_1_nosync.wav` is **284,313,644 bytes**, containing 246.8 seconds of six-channel, 48 kHz signed PCM32. The PCM begins at byte 44. Headers, hashes, alignment code and original metadata are retained under `output/ground-truth-leads-001/wsu/`.

The executable acquisition plan is `future-acquisition-plan.json` in that folder.
It selects the first 60 seconds after the provider's alignment, original interval
**[82,142) seconds**, with device 1/channel 0 fixed before any waveform inspection.
The exact interleaved PCM range is **94,464,044–163,584,043 inclusive**:
**69,120,000 bytes**, leaving room for metadata and bounded retries below 100 MB.
All six channels must be fetched because the frames are interleaved; retain only
the predeclared channel for analysis, preserving its 2,880,000 samples. Require
HTTP 206, exact ranges, payload lengths and retained hashes; refuse a whole-file
response. No 1.8 GB reference video or full multidevice download is needed for
this initial integration check.

This would be a **source-informed reference trial**, with receiver truth kept
separate from an audio-only reviewer and scorer. Do not select another interval
if the frozen one lacks a usable tone. GPS elapsed time supports relative
alignment; the whole-second UTC field and unquantified synchronization error
must remain explicit. Sea-level altitude requires a declared vertical-datum and
microphone-height treatment before fine-grid claims. Rotor-speed variation may
dominate Doppler, so documented drone presence does not establish a constant
emitted tone or identify each narrowband ridge. A known-receiver overlay can
first test physical consistency; it is not location recovery. A subsequent
search must freeze bounds, clock assumptions and wrong-source/wrong-clock
controls, commit predictions, then read the separate reference point.

The provider's `test_2/Mar_18_2025/flight_2` offers a different day/site for a later
untouched trial and explicitly labeled post-landing ambient intervals. Its two
negative halves share one recording and are not independent sessions. The
published README also identifies a later continuation unused in the paper, but
its own clock/telemetry eligibility must be checked before selection. No new
UAV waveform was downloaded while preparing this plan. Neither this lead nor
the two AeroSonicDB audits establish real hybrid Doppler/thunder accuracy.

### Frozen first-minute audio integration

The predeclared `[82,142)` second interval has now been acquired without changing
the file, device or channel selection. **33 exact HTTP ranges transferred
69,120,000 bytes with zero failed attempts**, below the 100 MB cap. All original
six-channel range payloads remain under `data/validation/wsu-reference-001/raw/`.
The separate review waveform is `review/recording.wav`: exactly 2,880,000 original
channel-0 samples, mono PCM32 at 48 kHz, 60 seconds, SHA-256
`0202d45173c8e68fc3429376c66108275c572362f54cfae2b86769e4e679fbaf`.
No gain adjustment, resampling, denoising or post-selection crop was applied to
this waveform. An independent integer-array reconstruction compared every
output sample with the original interleaved channel, and verified all range,
metadata, executable and **159 compiled-source file** hashes.

The protocol was frozen before PCM acquisition as
`output/wsu-reference-001/protocol.json`, SHA-256
`96f54f60cf887a37f94cb8d43a8b72864adc967ccd81eebac3d3a2fe3cf0d59d`.
It binds the preserved **0.5.0** native CLI, metadata, source snapshot and analysis
scripts. Native `analyze` completed in **2.42 seconds** and unchanged-default
`discover` in **1.82 seconds**. Discovery produced **zero ridges and zero groups**.
This zero outcome is retained; no threshold changes or replacement interval were
used. Documented aircraft presence is not a guarantee of a usable default
Doppler trace.

Full-minute audio-only figures cover 0–4,000 Hz, 40–800 Hz and 800–1,600 Hz. Each
shows raw spectral power and a separately labeled time-median-contrast view;
neither includes receiver coordinates, aircraft geometry, detector overlays or
predicted curves. The source channel has low absolute level: peak 0.00180 and
RMS 0.000117 of full scale, with no endpoint clipping. Its original PCM uses 98
distinct values with zero lower 16 bits. These properties are observed in the
downloaded channel, not introduced by PCM16 conversion: the native decoder uses
floating-point output. Their upstream cause is unresolved, and no corrective
gain or quantization treatment was applied.

A predeclared, geometry-free necessary condition checks whether an entire
unchanged trace could fit one constant emitted frequency. All 1,631 telemetry
rows were checked for finite mph velocity components and scalar speed; the
largest recorded speed is **11.4000 m/s**. The declared 25% plus 1 m/s margin gives
a conditional speed cap of **15.2500 m/s**. For a stationary receiver, no wind and
sound speed at least 330 m/s, the corresponding maximum observed-frequency ratio
is `(330 + 15.2500) / (330 - 15.2500) = 1.096902`. The speed cap and atmospheric
conditions are assumptions, not a measured absolute physical limit. Passing
this necessary bound would not prove constant source frequency or identity;
exceeding it could indicate rotor-speed changes, another sound or a mixed trace.

**There are no automatic traces to test against that bound**, so no compatibility
verdict follows from the zero exceedance count. Independent audio-only review
and any explicit anchors must precede geometry-based interpretation. The native
zero result, original data and diagnostics remain frozen. `phase1-completed.json`,
`frequency-variability.json`, `pcm-quality-supplement.json` and `verification.json`
under `output/wsu-reference-001/` preserve the evidence. The provider's nominal
sample/telemetry mapping is recorded; its physical synchronization accuracy is
still unquantified. No location fit, accepted receiver estimate or hybrid
accuracy claim results from this integration, and no application window opened.

### Frozen known-receiver consistency check

Following the unchanged zero automatic detections, audio-only review froze one
uncertain narrowband trace near 1,320 Hz, before inspecting aircraft predictions.
The original review retains **30 accepted points and two rejected points** and
remains pending/unselected, with no human-review or source-identification claim.
Its SHA-256 is `3f82333de2a81521ffeec40be9ac19064fb2bd54943f513dde3c1dd526d528dc`.
Only a separate experimental copy enables scoring, explicitly marked as an
**uncertain source hypothesis**. Documented UAV presence does not identify this
particular tone.

The phase-two protocol was frozen before reading those frequencies, under
`output/wsu-reference-001/reference-fit-001/protocol.json`, SHA-256
`787563097259dc8820735f79af506a02f5ecc4de30641cd9d79786feb4978eae`.
It supplies one known receiver and one original trajectory; this is deliberately
**source-informed consistency testing**, not blind recovery. All 1,631 GPS rows
are retained without smoothing. An explicitly documented Python conversion,
not a native provider importer, maps `elapsed_ms / 1000 - 3.9` to excerpt time.
The nominal UTC origin is `2024-10-11T04:49:17Z`, preserving the provider's relative
alignment without claiming subsecond absolute accuracy. Both source ASL altitude
and the reference origin's 32.6136 m height are treated as ellipsoid heights with
an assumed zero geoid offset. The reference origin approximates the selected
microphone, with no surveyed channel offset. Sound speed is fixed at 343 m/s.

Five separately fixed shared-clock scenarios were declared in advance: nominal,
−1, +1, −2 and +2 seconds. These are sensitivity scenarios, not calibrated timing
error bounds. The preserved 0.5 native production model fits only emitted
frequency within the fixed 500–2,500 Hz range. No receiver, source, clock, height,
trace point or frequency bound is optimized after seeing the result. All five
scenarios support all 30 measurements, without a fitted-frequency boundary hit.
A separately frozen descriptive baseline fits one constant observed frequency
to exactly the same points.

| Fixed scenario | Fitted emitted frequency (Hz) | Frequency RMSE (Hz) |
| --- | ---: | ---: |
| Flat observed-tone baseline | 1,320.5261 observed | **4.49059** |
| Nominal 0 s | 1,320.5306 | 4.52763 |
| −1 s | 1,320.9169 | 5.14865 |
| +1 s | 1,320.8144 | 4.46918 |
| −2 s | 1,320.9352 | 5.05215 |
| +2 s | 1,322.0122 | 6.28733 |

The nominal Doppler curve does **not** improve on the flat tone. The +1 s scenario
differs by only 0.02142 Hz RMSE (0.48%); no winner, statistical significance or
acceptance threshold is asserted. The complete trace's frequency ratio,
1.010793, passes the predeclared conditional necessary limit of 1.096902. That
weak condition does not establish source identity or a constant emitted tone.

A post-result, read-only GPS diagnostic identifies an important limitation:
there are **no exact repeated horizontal coordinate pairs or complete 3D
positions**, but altitude is quantized in approximately **0.1 m increments**.
Within source-elapsed seconds 6–23, 157 of 170 adjacent heights are unchanged;
the native finite-difference speed has median 0.0460 m/s and maximum 3.0004 m/s.
Vertical steps create velocity jumps and corresponding curve discontinuities.
Fine decimal latitude/longitude output is not a position-accuracy certificate.
This source-time subset is descriptive, not the exact emission-time support of
every clock scenario. No smoothing or refit followed the diagnostic. The mismatch
cannot be attributed solely to the acoustic source: timing, position/velocity
quality, datum and microphone approximations also limit the comparison.

An independent arithmetic and binding audit passed **259 checks**, including all
159 preserved compiled-source files, the native adapter's 11 embedded source
hashes, the 16 frozen input/code bindings, all 150 residuals and the five scalar
least-squares optima. It does not independently reimplement propagation physics.
`verification.json` has SHA-256
`2f9148ea7c6bd60cfd6ce2b4caaddb257735db65356a0843b9f3edd613009c4f`.
The corrected final report is `verified-summary.json`, SHA-256
`722675be2de6d1d575dbdd590842d893502f009ceefba122bd03b4fceb8e7825`.
It preserves the original summary and records two pre-physics adapter invocation
corrections; only a descriptive observer ID was removed for API compatibility,
with all scientific inputs unchanged. `known-receiver-curves.png` and
`gps-quality-supplement.json` retain the curves and trajectory diagnostic.
No location was searched or verified, no source was identified, no hybrid
accuracy was established, and no application window opened.

## Other investigated sources

- **Franken's public cameras:** the current [field-test report](https://github.com/Dicklesworthstone/franken_doppler_finder/blob/main/CHANGELOG.md#field-tests) identifies nine recordings but does not ship their media or stream timestamp manifests. The [PSEG camera operator](https://www.psegliny.com/wildlife/osprey/ospreycams) independently confirms the Oyster Bay camera, but a geocoded business address or aggregator pin is not surveyed microphone truth. Stream arrival time also differs from capture time unless tied to the first audio sample.
- **AeroSonicDB:** the [primary release](https://zenodo.org/records/10215080) explicitly substitutes nearby public-place coordinates for exact microphones; its filename time has no explicit timezone in the inspected metadata. Useful for detection controls, but it does not remove the current location/time uncertainty.
- **Livermore:** the existing local corpus and timing audit remain useful for discovery. Generic WAV filenames and publisher frequency annotations do not independently establish sample-zero UTC; its inferred reference coordinate must not be relabeled as surveyed truth.
- **Thunderprint/Frost:** remains a source-informed lead without its external raw inputs. The user requested continuing without Frost; no registration, credentials or messages were attempted.

The Eclipse path is executable with real audio and small independently sourced flight data. A successful regional recovery and an independent precision estimate remain separate outcomes to measure.

## Additive fit-diagnostic replay, version 0.7.0

The version 0.7.0 additive fit-diagnostic replay also retained all five WSU
scenarios and the unsuccessful fresh-B leader without refitting any hypothesis.
All original overlay fields and scores match exactly; 324 independent arithmetic
and binding checks passed. The fitted observed-frequency line has **4.36955 Hz**
RMSE on the WSU trace, lower than all five fixed Doppler cases (4.46918–6.28733 Hz).
For fresh-B, however, the Doppler curve has **1.51729 Hz** RMSE versus **4.16686 Hz**
flat and **3.15199 Hz** drift, despite its failed location-tolerance result.
These post-hoc regression examples show both the diagnostic's usefulness and its
limit: beating simple tones does not establish a correct source or location.
The line fits two parameters, the flat tone one; prior Doppler search freedom is
not penalized, and neighboring trace samples can be correlated. All 30/40 points
remain included. No significance, confidence threshold or new holdout claim is
made. The replay is `output/benchmarks/fit-diagnostics-001/real-replay-003/summary.json`.

## Frost-free Nordic lightning lead, November 2026

A bounded live request to [FMI's historical lightning service](https://opendata.fmi.fi/wfs?service=WFS&version=2.0.0&request=describeStoredQueries&storedquery_id=fmi::observations::lightning::multipointcoverage)
returned 36 strikes near the published Thunderprint reference region for
2025-07-19 16:00:00–16:01:00 UTC. All positions and 144 parameter values agree
between the service's two output formats. No credentials were used. FMI describes
[Northern European coverage](https://www.ilmatieteenlaitos.fi/salamahavainnot) and
licenses its open data under [CC BY 4.0](https://en.ilmatieteenlaitos.fi/open-data-licence).
This is useful for Nordic cases; it does not provide Quebec coverage.

Both inspected formats expose whole-second times. Their rounding/truncation and
physical timing accuracy are not certified by this sample. A future importer
must preserve that resolution and expose an explicit timing allowance. The
location-error major-axis field has conflicting units in official metadata and
the FMI-linked client documentation, so its raw value must not become a guessed
calibrated error radius. The response provides no acoustic-source altitude.

This removes Frost credentials as a necessary obstacle to a Nordic reference
reproduction. It does not supply an independent microphone recording or clock:
Thunderprint's published start was aligned using visible flashes and lightning
records, and its reference was developed with the receiver location known. The
original media, microphone accuracy and sample-zero audio timing remain
unverified here. Rechecking the Figshare directory found the same already-tested
infrasound archive, without new geometry or strike tables. No new audio or
location-accuracy result follows from either check.

Exact queries, raw replies, cross-checks and provenance are preserved in
`output/ground-truth-leads-002/findings.json`. The next provider increment can add
an analyst-requested time/region download and native import, with bounded chunks,
cache reuse, cancellation and visible source limitations. It is not implemented
in version 0.7.0. A real hybrid trial still requires aircraft evidence in the same
independently documented recording.
