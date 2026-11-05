# Test map

The [public validation tool](../tools/README.md) provides repeatable tiers and receipts. Use it for a full check or release. This page helps select focused tests while developing.

| Tier | Location / examples | Inputs and purpose |
| --- | --- | --- |
| Model and request unit controls | Inline module tests; `physics_validation.rs`, `normalization_validation.rs`, `audio_priors_validation.rs` | Synthetic inputs and independent expected values. No location-accuracy claim. |
| Numerical regression | `score_pruning_validation.rs`, `wide_area_pair_validation.rs`, `regional_search_validation.rs`, `joint_evidence_validation.rs` | Preserve scalar-oracle parity, scores, ties, request identities, progress and explicit unsupported outcomes. |
| Persistence and acquisition | `casework_validation.rs`, `data_catalog_validation.rs`, `fmi_catalog_recovery_validation.rs`, `jobs_validation.rs`, `regional_history_validation.rs` | Fabricated files/provider responses; ownership, cancellation, recovery, index reuse and tampering. No live download required. |
| GUI and launcher | `src/gui/**` under `#[cfg(test)]` | Headless egui frames, request preparation, generations, stale replies, editing and shutdown. No desktop window. |
| Synthetic media | `audio_validation.rs`, `discovery_io_validation.rs`, ignored installed-helper checks in `review_media_validation.rs`, `gui_bridge_validation.rs`, `envelope_workflow_validation.rs` | Real FFmpeg/FFprobe with locally generated audio/video. Required when that integration tier is requested. |
| Large storage | The explicitly ignored 2.1-million-point track-store control | Generated data, bounded disk/memory; run explicitly for a release. |
| Historical / external data | Two ignored cached terrain controls; optional discovery-report audit | Separately acquired inputs. Unavailable or unrequested checks are never recorded as passes. |
| Python tooling | `benchmarks/test_*.py`, `analysis/test_*.py`, `tools/test_*.py` | Packaging, runner failure handling, research contracts and public development-tool controls. |

The ignored shutdown child-process probe is invoked by its parent test. It is not a missing optional-data test and should not be run as an ordinary standalone integration.

## Fixtures and helper boundaries

`fixtures/fmi/` contains attributed provider XML and expected parsing values. `support/mod.rs` shares a small PCM WAV fixture writer between audio and discovery tests. It constructs bytes only; signal generation and expected numerical results stay in their owning suites.

Keep model expectations, independent scalar implementations and adversarial test assertions in their original suites. Shared setup must not turn an independent oracle into a call to the production algorithm.

## Scope of evidence

A test's executed name and result identify what was checked. A catalog entry, an ignored test, an unavailable input or an old report is not a current pass. Full validation records ignored tests and requested integrations explicitly.

See [compatibility](../docs/COMPATIBILITY.md) for stable contracts and [historical validation](../benchmarks/HISTORICAL_VALIDATION.md) for older experiments. Private recording replays remain separate from the public software suite.
