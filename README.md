# GeoSonnar

**A portable Windows workbench for investigating where a recording was made using aircraft Doppler and thunder evidence.**

Written in Rust. You review the sounds, choose the evidence and inspect competing locations. Detection and search assist your investigation; they do not certify a location.

[Download for Windows](https://github.com/ipechman/GeoSonnar/releases) · [Windows guide](docs/WINDOWS_GUI.txt) · [Roadmap](docs/ROADMAP.md) · [GPL-3.0-or-later](LICENSE)

## Get started

1. Download the Windows ZIP from **Releases** and extract it.
2. Run **`geosonnar-gui.exe`**. No installer is required.
3. Configure **FFmpeg and FFprobe** in Settings; add **FFplay** for playback. These tools are separate downloads.
4. Create a case, load a recording and review its events before searching.

See the [Windows guide](docs/WINDOWS_GUI.txt) for media setup, data import and troubleshooting. A software-rendering launcher, `geosonnar-safe.exe`, is also included.

## Follow the evidence

| Step | What you do |
| --- | --- |
| **Case** | Preserve the recording and enter known date, time and location clues. |
| **Audio review** | Inspect the spectrogram, adjust detection presets, trace Doppler sounds and confirm thunder events. |
| **Data** | Download or import available flight, lightning and terrain data; reuse stored indexes. |
| **Search** | Set the region and uncertainty, choose evidence and compare candidate locations. |
| **Results** | Sort coordinates and scores, inspect competing explanations and save reproducible runs. |

**Guided view** keeps common choices visible. **All controls** exposes detailed parameters without changing your evidence.

## What it includes

- Editable Doppler traces and event overlays on the spectrogram, with time/frequency zoom and panning.
- An interactive map with aircraft paths, lightning observations and search grids.
- Separate Doppler and thunder scores, explicit disagreement, and experimental combined evidence.
- Managed data, reusable flight indexes, background jobs and cancellation.
- Saved cases, input identities, run history and a command-line pipeline.

## Status: experimental 0.12

This release focuses on usability and search throughput. It adds clearer per-location progress and reuses or safely excludes repeated Doppler calculations. **A lower score is a better fit under the selected model—not a calibrated probability or a verified location.**

Broad searches remain expensive. The next performance goal is **worldwide discovery followed by local refinement within 30 minutes**. That goal is **not achieved by 0.12**; see its [acceptance criteria and next steps](docs/ROADMAP.md).

Historical lightning coverage varies by provider and region. Independent field accuracy and reliable false-match thresholds remain open work. Private recordings and downloaded datasets are not included in this repository.

## Documentation

| Topic | Guide |
| --- | --- |
| Windows setup and controls | [Windows guide](docs/WINDOWS_GUI.txt) |
| Flight data | [ADSB.lol acquisition and import](docs/ADSBLOL_HISTORY.md) |
| Lightning data | [FMI coverage and limitations](docs/FMI_LIGHTNING.md) |
| Search methods | [Regional search](docs/REGIONAL_SEARCH.md) · [Thunder envelopes](docs/THUNDER_ENVELOPE.md) |
| Evidence and validation | [Modality comparison](docs/EVIDENCE_COMPARISON.md) · [Field-validation plan](docs/FIELD_VALIDATION_PLAN.md) |
| Research and provenance | [Upstream comparison](docs/UPSTREAM_COMPARISON.md) · [Detailed release/reference notes](docs/PROJECT_REFERENCE.md) |

## Build from source

Use Rust **1.94.1 or compatible**. For media integration tests, put real FFmpeg and FFprobe executables on PATH.

```powershell
cargo test --locked --features gui
cargo build --release --locked --features gui
```

The GUI and CLI are written to `target/release/`. Numerical searches do not require Python; optional research and packaging scripts do. A portable release also needs the packaging checks described in the [technical reference](docs/PROJECT_REFERENCE.md).

## License and credits

GeoSonnar is licensed under **GPL-3.0-or-later**. External datasets and tools retain their own licenses. See [third-party notices](THIRD_PARTY_NOTICES.md) for attribution and the precise licensing of adapted material.

Methodology and comparisons draw on [DOPPLERGEIST](https://github.com/1337isnot1337/DOPPLERGEIST), [franken_doppler_finder](https://github.com/Dicklesworthstone/franken_doppler_finder) and [Thunderprint](https://github.com/Hexay/thunderprint). Their results are not claims of GeoSonnar's accuracy.
