# Real storm validation: current evidence

The real-recording lane checks acquisition integrity and runs the actual audio backend. **It does not yet establish independent receiver-location accuracy or improvement from combining thunder with Doppler.** The Figshare instrument sample lacks independent microphone geometry and strike metadata. A separate source-informed Thunderprint reference comparison is now recorded below; it supplements, rather than replaces, those limitations and the historical synthetic hybrid benchmark.

## Acquired field recordings

Eight approximately 30-second infrasound recordings, plus two companion seismic records, were acquired from Heting Hong's [Triggered-Lightning DAS Dataset](https://figshare.com/articles/dataset/Triggered-Lightning_DAS_Dataset/34020543), version 1, DOI `10.6084/m9.figshare.34020543.v1`, licensed CC BY 4.0. The release describes triggered-lightning multisensor observations. Files are from August 12, 2024, flashes 4–7, stations 203 and 204. They are real instrument recordings, not simulated thunder or ordinary full-band video audio.

The reproducible fetch downloads only selected ZIP members using verified HTTP byte ranges. The acquisition used **173,621 payload bytes**, instead of downloading the complete 485,024,746-byte archive. Each selected member is checked against its ZIP CRC and recorded with SHA-256. The provider's whole-archive MD5 is recorded but is **not** claimed as verified. Original recordings are preserved under `data/validation/storm-figshare-34020543-001/` and are ignored by Git.

The SAC files are 100 Hz, so the available signal stops at 50 Hz. Conversion to float WAV preserves sample count, rate and order, with a documented normalization gain per clip. The native adapter runs `review_media::analyze_window` and `thunder::propose`, the same measured-spectrum/envelope and proposal code used by the GUI. Internal resampling to 4 kHz adds no information above the source Nyquist limit. Although the backend's nominal thunder band is 20–250 Hz, these files only exercise its 20–50 Hz content. Peaks are unreviewed energy proposals, not confirmed lightning associations.

All infrasound receiver coordinates, heights and picked arrivals are undefined in the supplied SAC headers. The two companion seismometers have coordinates, but the folder layout does not independently prove exact microphone co-location. Lightning coordinates are absent; origin field zero is not independent evidence of strike UTC. SAC reference times have negative begin offsets, which the parser applies when deriving sample zero. Independent clock synchronization/uncertainty is not documented in the release. No aircraft evidence is supplied.

Do not borrow GPS timing or geometry from the [2023 experiment's paper](https://www.nature.com/articles/s41612-025-01261-z) to certify these 2024 files. The Figshare record has no linked companion paper or station/strike table establishing that connection.

## Measured integration result

The November 3, 2026 headless run is retained in `output/storm-validation-002/summary.json`. All **8/8** infrasound recordings completed the native backend with matching waveform hashes. Each produced a 290-column, 257-bin spectrogram; together they decoded 960,320 resampled samples. The default threshold produced **16 unreviewed energy proposals**, with zero sampling gaps and one rejected short interval. Counts for stations 203/204 were 1/1, 3/1, 1/1 and 4/4 for flashes 4–7 respectively. These counts are not detection accuracy: there are no independent onset labels in the release.

The exact headless adapter is preserved beside the receipt as `storm_audio.exe`, SHA-256 `3b392287e55317fbd2ce7353dc7a14b4ad88454a0d9500478f94c2496490443a`. Original SAC data and the derived WAV inputs remain separate. All location and hybrid validation flags remain false. The acquisition/parser controls passed all nine tests. A preliminary adapter run failed before decoding because an executable name on PATH needed absolute resolution; its diagnostic is retained in `output/storm-validation-001/` and the adapter now resolves the installed tools explicitly.

## Running it without opening windows

Build the small backend adapter, then inspect a new output directory:

```powershell
cargo build --release --example storm_audio
python -B analysis/validate_storm_recording.py fetch --output data/validation/storm-field-new
python -B analysis/validate_storm_recording.py inspect --input data/validation/storm-field-new --output output/storm-validation-new --native target/release/examples/storm_audio.exe
python -B benchmarks/storm_validation_tests.py -v
```

`inspect` deliberately exits **3** after successful waveform processing because location validation remains blocked. Acquisition/corrupt data/backend errors fail separately. The JSON receipt always reports `known_location_accuracy_validated: false` and `hybrid_accuracy_validated: false`; proposal generation cannot turn these into a pass. The acquisition is pinned to this release and is not a general waveform downloader. Existing input/output directories are never silently replaced.

The parser/integrity tests cover endianness, undefined coordinates, sample-zero offsets, invalid/nonfinite samples, WAV timing, ZIP traversal/truncation/CRC, and a server ignoring the requested byte range. They do not certify physical location accuracy.

## Thunderprint audit

The user-provided [Hexay/thunderprint](https://github.com/Hexay/thunderprint) is a useful source of methods and a reference-case lead. Its complete public Git tree `8f6886b854902348c6f0826e3a854371cff162de` was inspected on November 3, 2026 (America/New_York; November 4 UTC): 116 files, with no bundled audio or raw lightning fixture. Its `.gitignore` excludes `data` and `out`.

Its [truth file](https://github.com/Hexay/thunderprint/blob/main/eval/truth.toml) provides `65.666899, 14.138535` for a northern Norway recording. The [real-case script](https://github.com/Hexay/thunderprint/blob/main/tools/real.sh) uses July 19, 2025 and a flash-inferred start of `15:59:46.43 UTC`. Its [candidate notes](https://github.com/Hexay/thunderprint/blob/main/research/real_clip_candidates.md) identify that reference as YouTube `uD6Rtt2l4Ds`. The audio and Frost cache are external inputs; the repository does not contain them. The other six evaluation coordinate files describe much coarser candidate locations with uncertain dates/editing. Its offline Python smoke test generates synthetic strikes and audio.

The [upstream README](https://github.com/Hexay/thunderprint/blob/main/README.md) explicitly says its reported 1.3 km real-case error was not blind: truth was known during development. [Empirical model notes](https://github.com/Hexay/thunderprint/blob/main/research/empirical_thunder_response.md) also disclose fitting at that truth. A GeoSonnar replay is therefore a source-informed reference comparison, not a new held-out claim. The visible-flash timestamp also needs independent audio/video offset verification. Those coordinates must not be assigned to the unrelated hour-long local colsto recording.

[Frost's official documentation](https://frost.met.no/howto.html) requires the user's own free client ID, obtained through email registration. Its [lightning API](https://frost.met.no/api.html) accepts a UTC range and optional geographic polygon and returns UALF records. No account was registered, credentials borrowed or source contacted during this audit. Raw strikes are not redistributed by Thunderprint. Its MIT code license does not establish a license for third-party video recordings. No Thunderprint code has been copied into the adapter.

`benchmarks/storm_validation_candidates.json` records both acquisition leads and their exact readiness limits. It is deliberately separate from scored cases.

## Frozen Thunderprint reference after 0.9

The unchanged **0.9.0 command-line executable** completed a separately frozen, headless reference experiment in `output/benchmarks/thunderprint-reference-001/`. It used the complete 261.549569-second original AAC clip, channel 0, requested 20–250 Hz, FFT 4096 and a one-second hop. The native measurement produced 261 STFT-center samples from 0.512 to 260.512 seconds. No picked thunder arrivals or Doppler evidence contributed.

The date and region were source-informed: the description says July 19 but does not independently establish the year or UTC hour; 2025 and the reference timing came from upstream context. Before prediction, the protocol fixed a 65–67° N, 12–16° E rectangle, an anchored 0.1° latitude × 0.2° longitude grid, nominal 16:00 UTC and ±300 seconds of clock uncertainty. All nine native previews fit the original **441 receivers × 601 offsets**, so no coarsening occurred. Each of three dates was tested at assumed strike heights of 0, 3,000 and 6,000 m; **3,000 m on July 19 remained the primary variant**. Receiver height was fixed at 0 m, sound speed at 343 m/s, distance limits at 100 m–50 km, and the energy kernel at five-second decay/24-second duration. Heights are assumptions, not measurements.

FMI/NORDLIS supplied 1,076 rows for July 19, 117 for July 18 and 69 for July 20. The identical half-open 15:52–16:10 UTC windows and `[64.5, 10.5, 67.5, 17.5]` bounding box cover the declared receiver grid and propagation margin. The 54 one-minute requests transferred 371,398 bytes; raw responses and import receipts were retained. Wrong-date runs shifted both the nominal recording date and catalog support by one day, preserving all other settings. Query completion does not certify lightning completeness. Whole-second timing, the reviewed ETRS89 approximation and unknown heights remain explicit.

All **9/9** runs completed: **265,041 receiver/clock evaluations each, 2,385,369 total**, with no numerically unresolved offsets. Their whole-search maxima were:

| Assumed strike height | July 18 cosine | July 19 cosine | July 20 cosine | July 19 clock offset |
| --- | ---: | ---: | ---: | ---: |
| 0 m | 0.70364 | 0.76711 | 0.61492 | −32 s |
| **3,000 m — primary** | **0.70177** | **0.78371** | **0.61617** | **−28 s** |
| 6,000 m | 0.70129 | 0.78141 | 0.61998 | −21 s |

All three July 19 variants selected **65.7° N, 14.0° E**, **7.334 km** from the already disclosed upstream coordinate. This met the predeclared **10 km descriptive reference-consistency tolerance**, not an independently validated accuracy target. The nearest available grid receiver was itself 4.633 km from that coordinate. The primary start was 15:59:32 UTC, 14.43 seconds from the upstream flash-inferred start; the three heights changed the preferred clock despite preserving the coarse receiver. No height was chosen afterward to improve that comparison.

The observed envelope's cosine to a constant positive envelope was **0.70461**. The primary score exceeded it by 0.07910 and exceeded the two same-height wrong-date maxima by 0.08194 and 0.16754. These are descriptive contrasts, not significance tests or a calibrated rejection threshold. Both wrong dates had nonempty catalogs and complete searches, but different source populations and many silent predicted windows. Their maxima near 0.70 illustrate why a large cosine alone is insufficient. Global top-40 retention is still truncated; no confidence region or exact full ranking is claimed.

The authoritative local artifacts are `protocol.json`, `frozen-inputs-final.json`, `execution-addendum.json`, `acquisition/report.json`, `preview/selected.json`, all nine `predictions/` folders, and `evaluation-002/evaluation.json` under the experiment directory. The first measurement attempt failed before decoding on default helper-path resolution; the recorded second invocation changed only explicit FFmpeg/FFprobe paths. The first evaluator attempt rejected equivalent Windows path forms; its failure remains in `evaluation/`, and the corrected evaluator verified the native receipts. Original audio stays in the ignored validation-data directory and is not redistributed by these docs.

This experiment used already exposed coordinates, a source-informed regional/date prior and a lightning-derived reference start. There is no independent microphone survey or camera/UTC synchronization, and no matched aircraft evidence. **Independent field validation and hybrid accuracy remain false.** The earlier acquisition failures, Figshare limitations and synthetic/field regressions remain part of the evidence record.

## What a valid location trial still needs

1. An unedited original recording with independently documented microphone coordinates, elevation and uncertainty. Receiver truth is evaluation-only and must not enter fitting, grid centering or parameter tuning.
2. Independently sourced sample-zero UTC, or a declared timing uncertainty/search interval. A timestamp inferred using the same lightning catalog needs a separate timing-validation treatment.
3. A permitted, independently timed/positioned lightning catalog covering the clip and acoustic travel window, with declared position/time uncertainties and enough spatial diversity to identify a receiver.
4. Reviewed thunder intervals and a predeclared broad search region, misleading alternatives, missing-event tests and an accuracy tolerance chosen before scoring. Repeated flashes at one launch point constrain range without necessarily identifying a unique position.
5. For a combined comparison, independently reviewed aircraft tones and flight tracks for the **same** recording. A thunder-only reference cannot demonstrate hybrid accuracy or complementarity.

Additional primary-source leads were checked: [Johnson et al.'s 2011 New Mexico microphone/LMA study](https://agupubs.onlinelibrary.wiley.com/doi/full/10.1029/2011GL049162) supplies station information but its published supplement does not supply the raw waveform/catalog bundle; [AGU's Bogoslof thunder audio](https://news.agu.org/press-release/scientists-capture-sounds-of-volcanic-thunder/) is time-compressed demonstration media, not a ready independent storm validation fixture; public nature-sound recordings generally lack exact sample-zero UTC and matching strike catalogs. None is treated as a successful location test.
