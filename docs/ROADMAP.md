# Roadmap

GeoSonnar 0.12 is a usable experimental release, not a completed research program. The priorities below remain explicit after publication.

## Goal: worldwide discovery and local refinement in 30 minutes

**Status: not achieved.** The intended workflow finds promising locations worldwide, then refines those candidates locally. It does not promise exhaustive scoring of every worldwide cell at an arbitrary resolution.

The target is at most **1,800 seconds of search execution** on a declared Windows reference machine, starting with reviewed evidence and available indexed data. Include global candidate generation, local refinement and result generation. Measure downloads/import separately; do not hide them in the search timing. Human review time is also separate.

Before claiming this goal is met:

- Freeze representative recording cases, data coverage, time uncertainty, candidate/refinement budgets, final resolution and reference hardware before timing.
- Search the declared worldwide domain; report missing provider coverage. A smaller region, silent coarsening or a timeout with unfinished work does not satisfy the target.
- Preserve multiple plausible candidates, evidence disagreement, source identities and reproducible settings. Describe any approximate proposal stage separately from exact final scoring.
- Measure candidate recall and false matches alongside runtime using independent known-location cases, weak evidence and misleading alternatives. A fast wrong or unsupported result is not success.
- Run headless Windows benchmarks, record peak resources and cancellation behavior, and publish the workload and results. The development machine's 20% CPU test limit must be stated when used; it is not a hidden hardware assumption.

## Next steps

| Priority | Deliverable | Done when |
| --- | --- | --- |
| **1** | A fixed worldwide performance benchmark | Cases, search bounds, data coverage, hardware and acceptance criteria are committed before execution. |
| **2** | Worldwide candidate discovery | A bounded spatial/time index proposes candidates from flight and lightning evidence without an exhaustive fine global grid; proposal coverage and omissions are measured. |
| **3** | Local refinement within the shared time budget | Promising regions are refined with reviewable limits, ranked alternatives and explicit incomplete outcomes; the complete benchmark stays within 1,800 seconds. |
| **4** | Resume interrupted searches | Completed and partial work can be resumed only with matching evidence, settings and data identities; cancellation and restart reproduce uninterrupted results. |
| **5** | Independent storm validation | An openly usable recording with independently supported location/time uncertainty and matching lightning data is evaluated under a protocol frozen before scoring. |
| **6** | Usability and packaging follow-up | User testing covers intake through results; build paths are remapped in new delivery binaries and regressions remain headless. |

Atmospheric refinement remains optional. It follows evidence association, data coverage, performance and field validation rather than being a prerequisite for publishing useful releases.

## What 0.12 does not claim

- Worldwide discovery within 30 minutes.
- Calibrated location confidence or proven accuracy on arbitrary recordings.
- Worldwide historical lightning availability.
- Automatic recovery of a search interrupted inside a location.

The earlier detailed validation and release record is retained in [PROJECT_REFERENCE.md](PROJECT_REFERENCE.md). Passing software tests and replaying a historical result do not resolve these research limitations.
