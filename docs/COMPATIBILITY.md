# Compatibility checklist for 0.13

This checklist records the released 0.12 behavior that an organization-only change must preserve. It does not mark the [0.13 milestones](RELEASE_0_13_PLAN.md) complete. Source/build hashes will change after a refactor; scientific inputs, saved-format semantics and numerical outcomes must not change accidentally.

## Public entry points and saved formats

The Rust definitions and validators remain authoritative. This is an inventory of the main persisted surfaces, not a replacement JSON specification.

| Surface | Current contract and owner | Required compatibility |
| --- | --- | --- |
| Rust library and CLI | Public exports in [`src/lib.rs`](../src/lib.rs); commands, argument/path handling and exit codes in [`src/main.rs`](../src/main.rs) | Preserve callers through facades where files move. Preserve command names/options, relative-path bases, overwrite refusal and per-command exit statuses; do not standardize all partial/cancelled outcomes into success. |
| Desktop launcher | `src/bin/geosonnar-gui.rs`, `gui/startup.rs`, `gui/renderer.rs`, `gui/shutdown.rs` | Preserve renderer choices, the safe-launcher alias, headless startup checks and ownership of children. No GUI launch is required for automated refactor validation. |
| Case and review | Schema 1 `SavedCase` / `CaseHint` in `casework.rs`; schema 1 `ReviewWorkspace` in `workspace.rs` | Preserve recording/channel/source bindings, narrative clues versus scoring constraints, optional-field defaults and omission rules. Reviews are new revisions under `workspace-reviews`, not in-place replacement of old evidence. |
| Managed datasets | Schema 1 catalog entries in `data_catalog/model.rs`, re-exported through `data_catalog.rs`, with provider-specific plans and status receipts | Preserve provider metadata, inventories, checksums, partial/empty/cancelled states and complete-index reuse. Only registered owned copies may be deleted. |
| Indexed trajectories | `track_store.rs` reads v1 and v2; the packed writer emits v2 | Preserve little-endian f64 samples without resampling, time origin, pass boundaries, source provenance and per-range identities. Verify declared spatial bounds before using them to exclude tracks. |
| Multi-step jobs | Schema 1 plan/manifest/state/attempt receipts in `jobs.rs` | Preserve explicit resume/recovery, frozen dependencies and runtime identity, immutable previous attempts and failure evidence. This API is not a general arbitrary-command runner. |
| Regional and joint history | Schema 1 history receipts in `regional_history.rs`; `ReportKind` and `JointSnapshot` in `investigation_archive.rs` | Preserve the original workspace, normalized inputs and full outcomes. Regional, modality-comparison and joint-envelope variants remain distinct. Opening an archive never automatically follows external paths or starts scoring. |
| Wide-area searches | Schema 1 request/report and terminal receipt in `wide_area_pair.rs` / `wide_area_journal.rs` | Preserve request hashing, exact grid/source bindings, contiguous complete receiver pages and fresh-only output. Legacy reports may omit newer optional fields. Unsupported nonempty scoring-implementation identifiers are refused. |
| Envelope/joint workflow artifacts | `envelope_workflow.rs`, `joint_workflow.rs`, `joint_evidence.rs` | Keep measured audio, preview, request, result and failure artifacts distinct; completed execution is not equivalent to available scores or a verified location. |

Do not change serde field names, enum tags, defaults, optional-field omission or float round-trip behavior merely to improve source layout. A necessary format change needs its own migration decision and old/new fixtures. Preserve exact request bytes and hashes wherever existing regression contracts require them; compare exported paths/metadata only under explicitly declared normalization.

The initial extraction keeps the existing `data_catalog` public exports, `gui::studio::{Studio, Tools, Action}` facade and headless test namespaces. Private helper visibility is restricted to the coordinating parent module. The GUI's relocated calls to outer GUI helpers must retain their original targets; changing a file's parent must not silently change name resolution. Presence of these files does not replace compilation, test-inventory and runtime compatibility checks.

## Numerical invariants

- **Native tracks:** interpolation gaps, altitude/datum policies, source times and full-precision positions remain explicit. Display sampling must not feed scoring. Preserve rejection order for malformed or unsupported inputs.
- **Doppler:** the public scalar scorer in [`physics.rs`](../src/physics.rs) remains an independent reference for optimized code. Both events use one shared recording clock. Keep declared tone/shift iteration order, residual summation and first-tie selection; algebraically equivalent expressions are not automatically IEEE-equivalent.
- **Wide-area enumeration:** preserve receiver order, immutable pass order, native local-CPA indices, inclusive timing/physical bounds and ordered event pairing. Distinct later CPAs of the same aircraft remain eligible. Count and scoring paths must describe the same pair inventory.
- **Retention/pruning:** ordinary TopK and distinct ordered-flight-pair TopK use RMSE `total_cmp` followed by encounter order. A score-bound exclusion requires the existing strict certificate for both retainers; equality is not exclusion. Cache only proven first-event results tied to immutable inputs and exact clock bits. Never cache cancellation as lack of support.
- **Thunder and joint evidence:** lower Doppler error and higher envelope cosine have different directions. Preserve audio frame-center timing, measured band and source interpretation. Joint fusion uses the same complete receiver/clock domain, declared adjacent-transitive ties and midpoint ranks; unavailable cells cannot be silently substituted or dropped.
- **Budgets:** preserve logical work reservation, finite counters, memory/output bounds, deadlines and checkpoints. A faster implementation does not authorize widening, truncating or changing the scientific domain. Disabled work caps do not disable other limits.

## Progress, cancellation and historical results

For a committed wide-area receiver:

```text
timing candidates = scored candidates + scoring rejections + score-bound exclusions
```

Global attempted/scored/excluded counters may include the active, uncommitted receiver. Report rejection totals cover committed receivers. Therefore an interrupted pair is not an acoustic rejection, and global counters must not be forced into the completed-row identity.

The active receiver has an unknown eligible-pair total until counting finishes. Attempting its last pair is not committing its result. Clear active progress only after the receiver is written and included in completed counts. A missing terminal receipt remains an interrupted run, not a completed one.

GUI responses must still match their case generation and operation identifier. Late replies cannot clear newer work or replace current review/results. Preserve unsaved terminal outcomes when persistence fails, including their original case ownership. Rendering, selecting a saved result and toggling presentation must not change evidence or start workers.

Preserve bounded reads, hash checks, source rechecks, exclusive publication and path/link confinement while moving storage code. Hash receipts detect changed bytes; they are not authentication or field-truth certificates. Keep original sources and damaged/incomplete evidence available for diagnosis. Do not replace a missing declared height/source/model with an implicit default.

## Evidence required for an extraction

1. Name the moved responsibility and list affected exported APIs, formats and tests. Separate mechanical moves from fixes or numerical changes.
2. Map existing tests to their new homes. Do not silently remove assertions, ignore failures or merge independent numerical oracles into production helpers.
3. Run the applicable headless checks through the maintained public workflow. Record actual executed names, skips and prerequisites; missing private/media fixtures are not passes.
4. Compare the frozen numerical and persistence controls appropriate to the change. Preserve exact scores, ties, complete page inventories and canonical requests; include cancellation, stale replies, malformed input and rehashed-tamper controls for affected boundaries.
5. Recheck source/package manifests after path changes. Include every compiled fixture/helper and required license; exclude private recordings, credentials and generated research data.
6. Record fresh source/build identities and limitations. Keep earlier releases and receipts immutable. Refactoring success does not establish field accuracy or the worldwide 30-minute performance goal.

Useful starting controls are [`score_pruning_validation.rs`](../tests/score_pruning_validation.rs), [`wide_area_pair_validation.rs`](../tests/wide_area_pair_validation.rs), [`data_catalog_validation.rs`](../tests/data_catalog_validation.rs), [`jobs_validation.rs`](../tests/jobs_validation.rs), [`regional_history_validation.rs`](../tests/regional_history_validation.rs) and [`joint_history_validation.rs`](../tests/joint_history_validation.rs). GUI lifecycle controls live beside the relevant studio features. Use the [architecture map](ARCHITECTURE.md) to select the rest; a single passing test count is not proof that a boundary was preserved.
