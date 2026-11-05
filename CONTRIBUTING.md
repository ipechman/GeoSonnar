# Contributing to GeoSonnar

Start with the [architecture map](docs/ARCHITECTURE.md) and [compatibility checklist](docs/COMPATIBILITY.md). They connect the investigation workflow to code owners and tests. The [0.13 plan](docs/RELEASE_0_13_PLAN.md) defines the current organization work; the [roadmap](docs/ROADMAP.md) tracks subsequent capabilities.

## Set up

Use Windows, Rust 1.94.1 or a compatible toolchain, and Python 3.11 or newer for development utilities. Install FFmpeg and FFprobe for synthetic media integration checks. FFplay is optional for interactive playback. Python research dependencies are listed separately in `requirements-research.txt`; ordinary numerical execution does not require Python.

Clone the public repository into a new directory. Cargo may need network access to obtain the locked dependencies the first time. Install `requirements-research.txt` in your Python environment before requesting the full Python or release tier; it supplies the numerical/plotting packages used by existing research-tool controls. Media, downloaded datasets and prior experiment outputs are not required for the public software checks.

## Check a change

From the repository root, choose a **new output directory** for each run:

```powershell
python tools/validate.py quick --output output/check-001
python tools/validate.py native --output output/native-001
python tools/validate.py python --output output/python-001
python tools/validate.py integration --output output/integration-001
```

`quick` checks formatting and compilation. `native` adds Clippy and executes the native suite, including headless GUI controls. `integration` explicitly runs the synthetic media and large-storage checks. Missing required tools fail their requested tier; optional historical-data checks are reported separately. See the [tool guide](tools/README.md) and [test map](tests/README.md) for exact scope and prerequisites.

The tooling runs heavy commands serially, records command/output/source identities, and limits Windows jobs to 20% aggregate CPU at below-normal priority. It never opens the GUI. A failed run retains its evidence; inspect it before choosing a fresh run directory.

For focused edits, use the appropriate existing test directly, for example:

```powershell
cargo test --locked --test data_catalog_validation
cargo test --locked --test score_pruning_validation
cargo test --locked --features gui --bin geosonnar-gui gui::studio
```

Keep heavy direct checks serial and resource-limited on a shared machine. A focused test is useful feedback; the release workflow still checks the full required inventory.

## Make reviewable changes

- Work on one responsibility at a time. Keep mechanical moves separate from behavior changes.
- Preserve public Rust paths, CLI commands and saved formats through compatibility facades. Explain an intentional contract change and provide migration checks.
- Keep numerical oracles independent. Preserve iteration order, tie rules, cancellation and incomplete outcomes.
- Add a meaningful regression test for a changed contract; reuse fixture construction where appropriate. Do not weaken existing assertions to accommodate a refactor.
- Update the architecture map when ownership changes. Describe the problem, resulting behavior and checks in the commit or pull request.

## Build and hand off a release

```powershell
python tools/validate.py release --output output/release-001
python tools/validate.py verify --run output/release-001
python tools/validate.py cleanup --run output/release-001
```

Review the cleanup preview before adding `--execute`. Cleanup only removes the marked scratch directory owned by that run after retained artifacts are verified. The Windows package and matching source package must share the tested source identity. A release needs its validation receipt, source manifest, dependency notices and explicit limitations; software tests do not establish field accuracy.

Keep personal recordings, credentials and downloaded data outside the source tree. The source exporter has an explicit allowlist; verify new tooling and contributor files are included. Historical evidence is retained locally and documented as history rather than becoming a dependency of ordinary checks.
