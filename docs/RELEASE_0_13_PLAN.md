# 0.13 plan: organization and maintainability

**Status: implementation and validation in progress.** Version 0.12 remains the downloadable experimental release until the 0.13 release gate is verified.

The goal is to let a contributor locate a responsibility, understand its inputs and outputs, make a bounded change, and verify it from the public repository. Success means clearer ownership and reproducible work, not just smaller files or more folders.

The user subsequently requested [issue #1](https://github.com/ipechman/GeoSonnar/issues/1) and investigation of background GUI GPU load. Those usability fixes are tracked separately from the mechanical refactor in the [release notes](RELEASE_0_13.md) and receive additional regression checks.

## What needs attention

The 0.12 code already has useful modules and substantial tests, but some responsibilities have grown together:

- [`src/data_catalog.rs`](../src/data_catalog.rs) combines dataset ownership, download/import workflows, history and index reuse.
- [`src/gui/studio.rs`](../src/gui/studio.rs) combines shared application state, worker coordination and result dispatch. Views such as [`wide_ui.rs`](../src/gui/studio/wide_ui.rs) also contain substantial inline test code.
- [`src/lib.rs`](../src/lib.rs) exposes a largely flat module list spanning physics, acquisition, persistence and application workflows. The intended dependency direction is not documented.
- The latest release checks use local controllers under ignored `output/` directories. A fresh public checkout cannot reproduce that exact procedure without those files.
- Current instructions, research methods and historical release evidence need clearer entry points. The public source tree is distinct from the much larger local collection of recordings, datasets and experiment outputs.

These are starting points for inspection, not a mandate to split every large file.

## Milestones, in order

| Step | Work | Completion evidence |
| --- | --- | --- |
| **1. Map the system and its contracts** | Add an architecture guide showing Case → Audio review → Data → Search → Results, the modules responsible for each step, and the allowed dependencies. Inventory saved formats, public entry points, job lifecycle and numerical ordering rules. | Each responsibility has a named module and a test entry point. Proposed boundaries are documented before files move. |
| **2. Make verification reproducible** | Put maintained validation and release commands in the public repository. Replace dependencies on historical local controller chains with explicit, versioned utilities. Document prerequisites and separate quick checks, integration tests and optional research runs. | A fresh Windows checkout can run headless checks and produce verified Windows/source packages using documented dependencies. Missing optional data is reported explicitly. No personal paths or ignored historical receipts are required for ordinary builds and tests. |
| **3. Separate data responsibilities** | Extract provider adapters, dataset catalog operations, storage/index access and import coordination behind clear interfaces. Start with `data_catalog.rs` and its immediate dependencies. Keep the Rust crate layout simple; add crates only if a demonstrated boundary needs one. | Each extraction has one responsibility and accompanying tests. Existing callers remain supported through compatible exports where needed. Dataset identities, index reuse and saved cases behave as before. |
| **4. Separate GUI coordination from views** | Move case state, request construction, background-job lifecycle and result application into focused modules. Keep widgets responsible for presentation and user actions, with orchestration testable without a window. | Existing interaction tests pass. Cancellation, stale-result rejection, event editing, undo and close-while-busy behavior retain their contracts. No rendering changes are bundled into the refactor. |
| **5. Organize tests and shared helpers** | Give unit, integration, numerical regression and optional data-heavy checks clear homes and commands. Move large inline test sections where this improves navigation. Consolidate duplicated setup without merging independent numerical oracles into production code. | A contributor can identify the appropriate test tier for a change. Existing coverage is mapped to its new location; no test is silently deleted, weakened or turned into an ignored test. |
| **6. Simplify the documentation and release handoff** | Provide short user, contributor and research entry points. Keep historical evidence in an explicit archive/reference section. Document packaging, source manifests, resource limits and cleanup. | Links resolve in the exported source, the documented commands run from a fresh checkout, and the 0.13 release notes distinguish structural changes from any separately reviewed fixes. |

The [architecture map](ARCHITECTURE.md), [compatibility checklist](COMPATIBILITY.md), [contributor guide](../CONTRIBUTING.md) and [test map](../tests/README.md) describe the implemented boundaries and how to verify them. Completion still requires the release gate below; files and documentation alone do not prove it.

## Rules for the refactor

- Make small changes around one responsibility at a time. Separate file moves from behavior changes so reviewers can see both clearly.
- Preserve numerical scores, ranking and tie ordering on the frozen regression cases. Compare output formats and progress semantics as well as test pass counts.
- Preserve existing cases and saved requests. Any unavoidable format change needs a documented migration and compatibility tests before it is accepted.
- Preserve data provenance, evidence disagreement, cancellation and resource limits. Do not hide incomplete search outcomes during cleanup.
- Keep original recordings, private validation data and historical evidence out of the public source export. Historical evidence is retained, not deleted to make the folder look smaller.
- Run automated checks headlessly. Keep heavy local jobs serial and resource-limited; interactive Windows testing remains a separate user activity.
- Record structural decisions briefly: the responsibility being separated, why the boundary helps, and how compatibility was checked. Avoid adding abstraction layers without a concrete caller or problem.

## Release gate

0.13 is ready when all six milestones have evidence, the documented public workflow works from a fresh checkout, and the existing numerical and saved-case regressions pass. Review the final dependency map against the actual code, rather than accepting a renamed folder tree as proof of completion.

The release does **not** require new atmospheric models, additional providers, a GUI redesign, or completion of worldwide search within 30 minutes. Those remain separate work in the [roadmap](ROADMAP.md). Independent field accuracy also remains an open validation goal; reorganizing the code cannot establish it.
