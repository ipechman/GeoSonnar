# Upstream comparison

Reviewed November 3, 2026. This compares implemented capabilities, source tests and published results. Upstream code was inspected, not executed; upstream benchmark and field-test numbers below are their reported results, not independent reproductions or measurements against GeoSonnar.

This comparison table preserves the GeoSonnar 0.2 snapshot. Later version 0.3.0 added optional atmospheric profiles and modality diagnostics; the active [regional workflow](REGIONAL_WORKFLOW_PLAN.md) is adding uncertain-time association searches and terrain-derived receiver heights. See the main README and current validation receipts for their status. The upstream snapshots below are unchanged.

## Revisions reviewed

| Project | Current snapshot | Progress since our previous review |
| --- | --- | --- |
| DOPPLERGEIST | [`8a8e85bb19e0215089608c22d43733930d9dc261`](https://github.com/1337isnot1337/DOPPLERGEIST/tree/8a8e85bb19e0215089608c22d43733930d9dc261), November 3, 23:55:59 UTC | 200 commits and 217 changed files since `70626ad` |
| franken_doppler_finder | [`065a548ddd7ec015a53d3178ff1efbc2a9d36d32`](https://github.com/Dicklesworthstone/franken_doppler_finder/tree/065a548ddd7ec015a53d3178ff1efbc2a9d36d32), November 3, 22:20:23 UTC | 9 commits since `2d0362f13b7634eb9594fba34b166279fc53b0eb` |

## Capability comparison

| Area | GeoSonnar | DOPPLERGEIST | franken_doppler_finder |
| --- | --- | --- | --- |
| Operator workflow | Native Windows GUI; pipeline tabs, detection/grid presets, spectrogram event review and manual traces, map, sortable results and data manager | Python CLI and browser jobs; upload, progress, cancellation/resume and static diagnostics; manual ridge CSV input, no inspected browser tracing editor | Automated Rust CLI with generated figures, maps and HTML reports; no inspected interactive trace editor |
| Aircraft data/search | ADSB.lol acquisition/import and bounded searches with reviewed evidence | Historical acquisition, SQLite spatial/time index, optional C scoring, parallel coarse/fine grids, checkpoints and retained alternatives | Historical acquisition/cache and flight-centric hypothesis joins; automated propeller-flyover detection and matching |
| Terrain | Bounded Copernicus GLO-30/GLO-90 requests; automatic receiver-height sampling in the GUI remains unfinished | Automated Copernicus GLO-90 DEM and EGM2008 geoid acquisition; terrain-conforming scoring | Automated Terrarium terrain and geoid handling; explicit fallback warnings |
| Atmosphere | Declared sound speed; downloaded atmospheric profiles remain unfinished | Configurable constant sound speed and sensitivity checks; no acquired profiles | Acquired temperature/wind profiles and path-averaged propagation; straight rays, no refraction |
| Thunder/lightning | Experimental thunder proposals and reviewed strike timing combined with selected Doppler timing/frequency evidence | No implemented joint lightning/thunder pipeline found | No implemented joint lightning/thunder pipeline found |
| Validation status | 395 native tests, including 63 GUI tests, plus 4 media integration tests; no independently established hybrid localization accuracy | Extensive synthetic/real-data checks; documentation does not claim an independently accepted blind automatic real-world localization | 74 reported offline tests; original recording plus nine public-camera field clips, with substantial failures and abstentions |

Implementation references: DOPPLERGEIST's [worker scoring](https://github.com/1337isnot1337/DOPPLERGEIST/blob/8a8e85bb19e0215089608c22d43733930d9dc261/src/dopplergeist/parallel_tiles.py#L222), [terrain acquisition](https://github.com/1337isnot1337/DOPPLERGEIST/blob/8a8e85bb19e0215089608c22d43733930d9dc261/src/dopplergeist/data_access.py#L977), [browser](https://github.com/1337isnot1337/DOPPLERGEIST/blob/8a8e85bb19e0215089608c22d43733930d9dc261/src/dopplergeist/web/app.js) and [methodology](https://github.com/1337isnot1337/DOPPLERGEIST/blob/8a8e85bb19e0215089608c22d43733930d9dc261/docs/METHODOLOGY.md); Franken's [workflow](https://github.com/Dicklesworthstone/franken_doppler_finder/blob/065a548ddd7ec015a53d3178ff1efbc2a9d36d32/README.md), [weather acquisition](https://github.com/Dicklesworthstone/franken_doppler_finder/blob/065a548ddd7ec015a53d3178ff1efbc2a9d36d32/crates/fdop-data/src/weather.rs) and [propagation](https://github.com/Dicklesworthstone/franken_doppler_finder/blob/065a548ddd7ec015a53d3178ff1efbc2a9d36d32/crates/fdop-core/src/track.rs#L434).

GeoSonnar does **not** yet implement the proposed worldwide, multiweek lightning-envelope search using FFT correlation. Reviewed strike timing is a narrower capability. Test counts measure different scopes and do not establish comparative accuracy.

## Meaningful recent progress and limitations

**DOPPLERGEIST:** search infrastructure and reproducibility advanced substantially. Its latest integrated regional benchmark reports median runtime falling from **57.824 to 48.853 seconds** across three alternating matched pairs, with identical scientific outputs and arrays. Downloads and initial indexing were excluded. Every result remained **inconclusive / search_incomplete**. This supports a runtime improvement on that benchmark, not increased localization accuracy. Its Livermore demonstration searched 466 nearby aircraft but used city-sized bounds and an approximately ten-second clock interval; the 476 m distance was to an inferred reference, not surveyed microphone truth. The result remained inconclusive. [Performance record](https://github.com/1337isnot1337/DOPPLERGEIST/blob/8a8e85bb19e0215089608c22d43733930d9dc261/docs/PERFORMANCE.md), [Livermore limitations](https://github.com/1337isnot1337/DOPPLERGEIST/blob/8a8e85bb19e0215089608c22d43733930d9dc261/docs/LIVERMORE_DEMO.md#L98).

**Franken:** the nine new commits primarily improve stalled-download recovery, detection-limit disclosure, failed-run recording, state-boundary warnings and diagnostics. Weather propagation already existed at the previous review. It uses at most a 3-by-3 weather grid, five pressure levels and approximated upper-air humidity, with warned fallbacks. No separate weather ablation establishes its accuracy benefit. [Changelog](https://github.com/Dicklesworthstone/franken_doppler_finder/blob/065a548ddd7ec015a53d3178ff1efbc2a9d36d32/CHANGELOG.md).

Franken reports one precise high-confidence field success at the original site's camera on another day; its nine-clip set also includes five wrong low-confidence locations, two abstentions and one low-confidence correct-township result without precise ground truth. Crucially, an uncommitted experiment increasing detection/measurement limits upgraded a **wrong 103 km result from low to medium confidence**: its threshold did not account for the larger search. The limits remain. Its confidence labels and modeled radius should not be treated as empirically calibrated probabilities. [Field tests and experiment](https://github.com/Dicklesworthstone/franken_doppler_finder/blob/065a548ddd7ec015a53d3178ff1efbc2a9d36d32/CHANGELOG.md#field-tests), [implemented confidence gates](https://github.com/Dicklesworthstone/franken_doppler_finder/blob/065a548ddd7ec015a53d3178ff1efbc2a9d36d32/src/pipeline.rs#L793).

## Next priorities

1. **Establish a truth-separated hybrid benchmark.** Compare Doppler-only, thunder-only and combined evidence under identical priors. Include wrong-date/location controls, missing-data cases and wider detection presets; retain competing locations and report false confident answers and abstentions.
2. **Finish terrain/geoid integration.** Make source, coverage, vertical datum and fallback state visible in the data manager; verify receiver-height sampling before finer searches.
3. **Add optional atmospheric profiles.** Start with temperature, humidity and wind over height, cache provenance and explicit fallback selection. Compare constant-speed and profile-enabled results before claiming better precision; defer ray tracing until evidence justifies it.
4. **Improve resilient, reviewable jobs.** Expose discarded/capped events and incomplete work, support resumable searches, and verify streamed-download idle recovery. Preserve manual review and explicit evidence selection.

## Licensing and reuse

Current DOPPLERGEIST uses **PolyForm Noncommercial 1.0.0** and explicitly preserves earlier MIT grants. Keep GeoSonnar's existing `70626ad` provenance and historical MIT attribution; do not apply that historical license to current source. [Licensing history](https://github.com/1337isnot1337/DOPPLERGEIST/blob/8a8e85bb19e0215089608c22d43733930d9dc261/COMMERCIAL-LICENSING.md).

Franken uses **MIT with an OpenAI/Anthropic rider**, not ordinary MIT. This comparison imported no upstream implementation. Further work should use independent implementations and preserve accurate third-party notices. [Pinned license](https://github.com/Dicklesworthstone/franken_doppler_finder/blob/065a548ddd7ec015a53d3178ff1efbc2a9d36d32/LICENSE).
