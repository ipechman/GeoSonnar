# GeoSonnar 0.13

This release concentrates on making the project easier to understand, verify and maintain. The investigation workflow remains Case → Audio review → Data → Search → Results.

## Structural changes

- Data management has focused modules for catalog records, owned storage, imports, index reuse, generic downloads, ADSB acquisition and history. The existing public catalog APIs remain available.
- GUI state, background work, editing and views have separate owners. Wide-search settings, request preparation, operations, presentation helpers and tests have their own modules.
- The public library module list is grouped by responsibility. The project remains one Rust crate.
- The public development tools provide explicit quick, native, integration, Python, release, verification and owned-cleanup commands without historical local controller dependencies.
- Contributor instructions, an architecture map, compatibility contracts and a test map provide current entry points. Older Windows instructions and validation narratives are preserved in historical documents.
- Audio and discovery checks share a PCM fixture writer; their signal construction, assertions and numerical oracles remain independent.
- A legacy provider-replay comparison is now explicitly ignored unless requested with its external inputs. Missing data can no longer make that comparison report a pass; its scientific assertions remain intact.

## Behavior and compatibility

This work preserves the saved case/review/request formats, source identities, numerical scoring, iteration and tie ordering, cancellation, stale-response rejection and explicit incomplete outcomes. Small extraction corrections concern internal import paths, visibility and text encoding. They do not introduce a new search method or alter scoring assumptions.

## Separately requested usability fixes

[Issue #1](https://github.com/ipechman/GeoSonnar/issues/1) adds consistent GiB storage/memory inputs and displays, specific required-versus-allowed budget feedback, and four wide-search sections with visible estimate/run guidance. Exact byte defaults and machine-readable formats remain intact. A preliminary output reservation is labeled as a minimum; it does not predict that a search will finish within its resource allowances. Original source JSON and saved diagnostics remain available for copying while their presentation uses GiB.

Background busy indicators use static status marks with timed refreshes. Progress notifications and input still request immediate redraws. This removes an animation-driven repaint loop identified after a report of high integrated-GPU use. It does not move search calculations onto the GPU, and a change in actual hardware utilization still needs measurement on the user's machine.

Lower Doppler RMSE indicates a better model fit; higher thunder-envelope cosine similarity indicates a better envelope match. Neither is a calibrated location probability. This documentation clarification also appears in the introduction.

## Verification and limitations

Use the [public validation workflow](../tools/README.md) to reproduce software checks and packaging from the source. Each run records actual executed tests, optional or unavailable integrations, source identities and artifacts. Verification rechecks retained evidence, and cleanup only removes the marked scratch directory owned by that run.

The [0.13 plan](RELEASE_0_13_PLAN.md) defines the completion gate. Software validation does not establish independent field accuracy, worldwide historical lightning coverage, interrupted-location resume or worldwide discovery within 30 minutes. Those remain explicit [roadmap](ROADMAP.md) items.
