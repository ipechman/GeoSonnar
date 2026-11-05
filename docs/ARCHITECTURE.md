# Architecture and ownership

This map follows the [0.13 organization plan](RELEASE_0_13_PLAN.md). The data-catalog and GUI responsibility extractions and public validation tooling described below are present in source; their validation is a separate release gate. The numerical engines and saved contracts still follow the released 0.12 baseline. The [compatibility checklist](COMPATIBILITY.md) defines what must survive each change.

## Runtime boundaries

GeoSonnar is one Cargo package with a library, the `geosonnar` CLI in [`src/main.rs`](../src/main.rs), and the optional `geosonnar-gui` binary in [`src/bin/geosonnar-gui.rs`](../src/bin/geosonnar-gui.rs). The GUI feature owns desktop dependencies. Numerical searches do not require Python. FFmpeg, FFprobe, FFplay and the download transport are external process dependencies, not numerical models.

The intended dependency direction is:

```text
CLI / desktop presentation
          |
explicit workflow and job coordination
          |
reviewed domain inputs, acquisition/storage adapters, search engines
          |
audio / geometry / propagation / numerical scoring
```

This is an ownership rule, not a claim that the current Rust module graph is acyclic. For example, `ReviewWorkspace` stores regional-search settings while regional search consumes reviewed workspaces. Move related data types deliberately; do not manufacture a crate split to hide those references.

- Presentation gathers actions and displays state. Painting a panel must not download, decode, select evidence or start a search.
- Workflow code validates an explicit request, binds inputs, estimates resources, runs the requested method and records its outcome.
- Acquisition and storage own provider semantics, file identities, bounded reads/writes and recovery. A successful download does not establish scientific coverage.
- Numerical code consumes declared inputs. It must not consult GUI state, provider services or evaluation truth.

## Pipeline map

Paths identify current owners; tests are entry points, not an exhaustive inventory.

| User step / responsibility | Current implementation | Verification entry points |
| --- | --- | --- |
| Case intake and recording identity | [`casework.rs`](../src/casework.rs), `media.rs`, `media_tools.rs`, `gui_bridge.rs` | [`casework_validation.rs`](../tests/casework_validation.rs), `media_tools_validation.rs`, `gui_bridge_validation.rs` |
| Audio measurement and event review | `audio.rs`, `review_media.rs`, `discovery.rs`, `event_review.rs`, [`workspace.rs`](../src/workspace.rs); GUI `spectrogram.rs`, `studio/acoustic_review.rs` | `audio_validation.rs`, `review_media_validation.rs`, `workspace_review_validation.rs`; spectrogram/studio headless tests |
| Flight acquisition and normalization | `adsblol_acquisition.rs`, `adsblol_history.rs`, `adsblol.rs`, `detailed_data.rs`, `normalization.rs` | `adsblol_acquisition_validation.rs`, `adsblol_history_validation.rs`, `adsblol_validation.rs`, `detailed_data_validation.rs` |
| Lightning, terrain and atmosphere | `fmi_lightning.rs`, `lightning_source.rs`, `terrain.rs`, `receiver_heights.rs`, `atmosphere.rs` | `fmi_lightning_validation.rs`, `lightning_source_validation.rs`, `terrain_validation.rs`, `atmosphere_validation.rs` |
| Managed data and exact indexed tracks | [`data_catalog.rs`](../src/data_catalog.rs) facade and private `data_catalog/` modules; existing `fmi_catalog.rs` / `dataset_requests.rs` provider adapters; [`track_store.rs`](../src/track_store.rs), `track_store_packed.rs` | `data_catalog_validation.rs`, `fmi_catalog_recovery_validation.rs`, `dataset_request_validation.rs`; track-store unit controls |
| Search and evidence comparison | `physics.rs`, `evidence_score.rs`, `regional_search.rs`, `thunder_envelope.rs`, `joint_evidence.rs`; [`wide_area_pair.rs`](../src/wide_area_pair.rs), `cached_pair.rs`, `retention_bound.rs` | `physics_validation.rs`, `regional_search_validation.rs`, `thunder_envelope_validation.rs`, `joint_evidence_validation.rs`, [`score_pruning_validation.rs`](../tests/score_pruning_validation.rs), `wide_area_pair_validation.rs` |
| Durable outcomes and explicit replay | `jobs.rs`, `regional_history.rs`, `wide_area_journal.rs`, `investigation_report.rs`, `investigation_archive.rs` | `jobs_validation.rs`, `regional_history_validation.rs`, `joint_history_validation.rs`, `investigation_report_validation.rs`, wide-area journal controls |
| Desktop coordination and lifecycle | [`gui/studio.rs`](../src/gui/studio.rs) facade; `studio/{state,workers,editing,views}.rs`, feature panels and `wide_ui/`; `gui/mod.rs`, `gui/shutdown.rs` | `gui::studio::*` headless controls, `gui::shutdown::*`, launcher controls |

Search methods are separate contracts. The indexed wide-area two-pass method is not the regional event scorer; shared-cell Doppler/envelope comparison is not picked thunder-arrival scoring. A refactor must not silently route one into another.

## Jobs, ownership and persistence

`Studio`, defined in `studio/state.rs` and re-exported through `studio.rs`, owns the reviewed document, undo/redo and case generation. `studio/workers.rs` owns `ResultValue`, `WorkerLane`, reply routing and playback retirement; `editing.rs` owns review mutations, while `views.rs` owns the shared panels. Per-operation identifiers prevent an old response from changing a newer case or job. Feature views request work through this coordination; they must not create competing ownership rules.

There are three different persistence lifecycles:

1. `jobs.rs` freezes an explicit multi-step plan and records attempts. Its resume/recovery path verifies dependencies and retains previous attempts.
2. `regional_history.rs` journals regional/comparison/joint runs with their historical review and inputs. Reopening a result is not permission to run it again.
3. `wide_area_journal.rs` stores fresh-only receiver pages and a terminal report. A partially scored receiver is not committed. It does not implement resume inside a receiver.

Cancellation remains cooperative and owned. Source verification, numerical work and workers carry cancellation checks; GUI shutdown also owns its subprocess cleanup. A resource stop is distinct from missing acoustic support. Saved numerical results may remain readable without originals, but replay requires explicit source verification.

## Responsibility boundaries

Existing exported module paths remain compatibility facades. Private children separate responsibilities without creating another Cargo crate or changing the saved data model.

| Entry point | Internal owner and status | Keep stable |
| --- | --- | --- |
| `data_catalog.rs` | Present: private `data_catalog/{model,storage,catalog,imports,indexed_reuse,downloads,adsb_downloads,history}.rs` | `geosonnar::data_catalog` APIs, owned deletion, source and receipt identities |
| `gui/studio.rs` | Present: `studio/{state,workers,editing,views}.rs`; facade retains shared imports and public types | Case generations, reply ownership, undo and close/cancel behavior |
| `studio/wide_ui.rs` | Present: `wide_ui/{state,preparation,operations,presentation,tests}.rs`; facade retains UI entry points | Exact request bytes, explicit actions, result provenance and existing test identities |
| Public validation workflow | Present: `tools/validate.py`, `process.py`, `evidence.py`, `release.py`, `README.md` and `test_validation.py` | Serial bounded processes, input freezing, actual executed-test accounting, safe exports and artifact identities |

The data extraction keeps public `data_catalog::fmi` and `data_catalog::requests` at their existing `fmi_catalog.rs` and `dataset_requests.rs` paths. Model types do not depend on storage; storage provides ownership/inventory operations to catalog, import, reuse and provider workflows. Generic URL downloads and ADSB archive acquisition have separate owners. File-handle/lock lifetimes across these calls remain part of the contract. The two FMI acceptance controls live in `data_catalog/fmi_tests.rs` under their original `data_catalog::fmi::acceptance_controls` namespace. `track_store.rs` and `track_store_packed.rs` remain in place.

Within wide-area UI code, `state.rs` contains settings and reply types; `preparation.rs` builds requests and reads/writes bound review snapshots; `operations.rs` launches explicit work and applies replies; `presentation.rs` formats controls, progress and rows. Moving the tests to `wide_ui/tests.rs` preserves their `gui::studio::wide_ui::tests` names. These are ownership boundaries, not new runtime services.

The public tooling entry point is `python tools/validate.py`, with `quick`, `native`, `integration`, `python`, `release`, `verify` and `cleanup` phases documented in [tools/README.md](../tools/README.md). `process.py` owns hidden, bounded Windows children and the aggregate CPU cap; `evidence.py` owns source/file identities and executed-test accounting; `release.py` owns build flags, packaging, retained verification and marked-scratch cleanup. The workflow uses the public packagers under `analysis/` and imports no ignored `output/` controllers. Historical external checks are explicitly optional. Available commands are not claims of a successful run; fresh receipts establish that separately.

## Repository boundaries

The 0.12 public export has 340 source files plus its manifest. Local `output/`, `tmp/`, `dist/`, `data/` and recordings are not its source structure. Historical/private evidence must not be deleted or copied into the repository to make a refactor convenient.

[`analysis/package_source.py`](../analysis/package_source.py) explicitly lists exported top-level files and source directories. New contributor/tooling directories require reviewed export-rule changes and packaging controls. [`benchmarks/run_validation.py`](../benchmarks/run_validation.py) and the packager also discover compiled inputs: moving helpers or fixtures requires updating their identity coverage, not merely their imports.

Current user guidance is in the [Windows guide](WINDOWS_GUI.txt). Historical experiments remain in [PROJECT_REFERENCE.md](PROJECT_REFERENCE.md) and the method-specific validation documents; their successful software checks are not independent field-accuracy evidence.
