# Public validation and releases

These tools run from a clean GeoSonnar checkout or source export. They use the
Python standard library and public repository files. They do not import private
benchmark controllers or require an earlier release's output folders.

Use **Python 3.11 or newer**, the Rust toolchain specified by `Cargo.toml`, and
Windows for the capped command runner. Rust dependencies must already be cached:
run `cargo fetch --locked` separately when you want to permit network access.
Validation commands use `--offline --locked`; they do not download data or tools.

The runner creates one hidden, below-normal-priority worker in a named Windows
JobObject with a queried 20% aggregate CPU ceiling. Each serial command uses a
separate kill-on-close ownership job with **CPU rate control disabled**: it
inherits the aggregate ceiling instead of multiplying it by another 20% quota.
Both job memberships and the child's priority are checked before it resumes.
Commands use two Cargo/test worker threads. Timeouts and Ctrl+C
close only the owned process tree. Non-Windows systems receive an explicit
unsupported result rather than an uncapped fallback. Pure Python controls can
still be invoked directly with `unittest` on other systems.

An uncapped host ownership job (for example, a CI host) is allowed and recorded.
A known CPU-controlled immediate host job is refused rather than silently
stacking quotas. More distant external host restrictions cannot be enumerated
reliably and may reduce available CPU further. The 20% value is a ceiling, not
promised CPU availability or a runtime prediction. The tools never break away
from host policy. Aggregate handles opened by the worker for verification are
closed immediately so launcher exit can still terminate its owned tree.

## Choose a tier

Run commands from the checkout root. Every `--output` must name a **new** directory
outside the source folders. Failed attempts are retained; choose a new name to
try again. Example names below are arbitrary and do not identify historical runs.

| Tier | Command | Scope and prerequisites |
| --- | --- | --- |
| Quick | `python tools/validate.py quick --output output/validation/quick-001` | Formatting check and all-target Cargo check with GUI features; no tests or release build. |
| Native | `python tools/validate.py native --output output/validation/native-001` | Formatting, Clippy, all-target native and headless GUI tests. Actual FFmpeg and FFprobe are needed by generated-audio tests. |
| Integration | `python tools/validate.py integration --output output/validation/integration-001` | Explicit 2.1-million-point synthetic store control and six actual media-tool integrations. Requires FFmpeg/FFprobe. |
| Python | `python tools/validate.py python --output output/validation/python-001` | Public tooling controls and repository benchmark controls. The latter require the packages in `requirements-research.txt`. |
| Release | `python tools/validate.py release --output output/validation/release-001` | All required checks above, one static-CRT release build, headless startup checks, portable/source packaging, and exact verification. |
| Verify | `python tools/validate.py verify --run output/validation/release-001` | Rechecks retained test logs/counts, build/source bindings, binaries, ZIP contents and licenses; does not rebuild or rescore. |
| Cleanup preview | `python tools/validate.py cleanup --run output/validation/release-001` | Reports only this run's marked scratch directory. Deletes nothing. |
| Cleanup | `python tools/validate.py cleanup --run output/validation/release-001 --execute` | Removes only the marked scratch after verification. Keeps logs, reports, source export, retained programs and packages. |

`--timeout 1800` sets the maximum seconds per command. `--wall-seconds 7200` sets
the whole-tier deadline. These are explicit resource ceilings, not runtime
predictions. Increase them deliberately for a slower machine; the runner never
automatically retries or silently removes tests. Each command retains at most
0.015625 GiB per output stream and stops if the limit is exceeded.

## External prerequisites are explicit

FFmpeg and FFprobe are external executables. Put them on `PATH` or supply paths:

```powershell
python tools/validate.py native --output output/validation/native-001 --ffmpeg C:\Tools\ffmpeg\bin\ffmpeg.exe --ffprobe C:\Tools\ffmpeg\bin\ffprobe.exe
```

Bare names such as `--ffmpeg ffmpeg.exe` resolve through `PATH`; an explicit
directory path that does not exist is refused. Native and integration stages
bind the selected executable hashes and recheck them after use. No executable
is launched merely by importing a tooling module.

For the Python benchmark controls, install the separately declared research
dependencies if desired:

```powershell
python -m pip install -r requirements-research.txt
```

Missing prerequisites produce **unavailable**, with an explanation. They never
produce a passing test count. Default Rust tests retain their exact ignored-test
inventory. The shutdown test's parent-owned child probe is distinguished from
external integrations. The six media tests and synthetic large-store control
run explicitly in integration/release tiers.

Two old terrain comparisons depend on historical downloaded data and a prior
observer grid. Public tooling records them as `historical_external_terrain` /
`not_requested`; it does not invoke an ignored historical output chain or claim
those comparisons passed. An existing discovery compatibility audit can be
requested separately with `--discovery-report PATH` on integration/release.
That option deliberately opens the supplied report's original media, requires
the media tools, and is then a required stage. It is a compatibility check, not
a new location-accuracy trial.

## Receipts and release contents

`report.json` contains the stage inventory, actual executed test names/counts,
ignored/skipped/unavailable states, command receipts, resource limits and exact
source snapshot. `logs/` contains bounded output and per-command receipts.
The sibling `<output>.launcher/` keeps the outer ownership/timeout result even
if a worker terminates before it can finish its own report.
Resource proof schema 2 records the aggregate's queried 2000/10000 ceiling
separately from each command job's queried disabled rate (flags/rate 0), plus
membership and priority checks. Older receipts with nested rate caps are kept
as historical observations; they do not satisfy this corrected policy.

The release tier builds once into its own `scratch/target`. Its child-only Rust
flags request static CRT and remap the current workspace, home and Cargo paths.
Retained binaries are screened for known personal build prefixes; this is not
a general guarantee that arbitrary private text can be detected.

`release/` contains seven retained programs, the three-program Windows package,
the source export, both ZIPs and a manifest. Existing public packagers provide
their source/privacy screen and complete dependency/license inventory. The
verifier compares the exported bytes against the exact pre-build source
snapshot and checks every package/ZIP member. It follows command/log receipts
and reconstructs actual Rust/Python counts instead of trusting a claimed total.
Retained verification continues to work after this run's scratch is removed.

Cleanup refuses links/junctions, moved or rebound ownership markers, unfinished
runs and unrelated directories. It never deletes the checkout's shared
`target`, `tmp`, older releases or scientific evidence. Interrupted runs remain
marked incomplete and need manual review rather than automatic deletion.

These checks establish software behavior and artifact provenance. They do not
establish location accuracy, search completeness outside the declared domain,
interactive graphics-driver behavior, or the validity of a new field recording.
No historical timing or scientific result is relabeled as fresh execution.

## Small tooling controls

```powershell
python -B -m unittest discover -s tools -p "test_*.py" -v
python -B -m unittest discover -s benchmarks -p "test_source_package.py" -v
```

These fabricated controls cover result counting, unavailable prerequisites,
bounded logs, exact ZIP identities, evidence retention, ownership and cleanup.
The public Python/release tiers run them inside the capped worker automatically.

For a small Windows process-policy probe, without building or opening the GUI:

```powershell
python tools/probe_process.py --output output/validation/job-policy-001
```

It queries the aggregate and command-job policies, terminates its exact launcher,
and checks that its worker and descendant exit while an unrelated idle sibling
survives. The tested worker/command tree has the same aggregate ceiling as normal
validation. The observer and unrelated sibling only wait; this is not a CPU
throughput measurement. The fresh folder retains the actual result and logs.
