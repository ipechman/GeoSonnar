# Small regional ADSB.lol history requests

The native Rust downloader uses ADSB.lol's public map replay as a **candidate index**, then downloads the selected aircraft's original full-day readsb traces. It does not require Python, an account or an API key. It uses the Windows system HTTPS downloader; other platforms require `curl` on PATH.

This is an explicit two-step acquisition followed by a separate native import. None of these operations confirms a detected sound, selects an aircraft association, or verifies a location.

## Plan before downloading

Save a request such as:

```json
{
  "utc_start": "2024-04-08T04:20:00Z",
  "utc_end": "2024-04-08T04:40:00Z",
  "bounds": [42.5, -73.5, 45.5, -70.0],
  "max_traces": 100,
  "max_download_bytes": 95000000
}
```

Bounds are south, west, north, east in degrees. They describe **aircraft coverage**, so pad the receiver search region for sound travel and aircraft motion. West greater than east explicitly crosses the date line. The initial implementation allows at most one hour within one UTC calendar day, 20 degrees in each horizontal dimension, 256 full traces and 250 MB of cumulative HTTP payload. Start and end must include the UTC `Z`. Both requested time endpoints are included; an endpoint exactly on a half-hour boundary consequently includes the following replay file.

```powershell
geosonnar plan-adsblol-history --request request.json --out history-plan.json
geosonnar fetch-adsblol-history --plan history-plan.json --cache regional-history
```

Planning makes no network request. It lists exact half-hour URLs, the trace count cap and byte budget. URLs and limits are reconstructed from the request before fetching, so editing a saved plan cannot redirect the downloader or loosen its bounds.

Each replay response is capped at 20 MB and 128 MB after decoding. Each raw full trace is capped at 2 MB and 8 MB after decoding. Each HTTP attempt is bounded to 45 seconds. Failed payloads consume the same cumulative budget as successful payloads; retries cannot silently reset it. The GUI can cancel an owned download process without affecting another application.

## What the receipt means

The command writes an immutable numbered report under `regional-history/reports/` and prints its path and contents. Acquisition outcomes are:

- `complete`: all required replay files and all selected regular-ICAO trace files were downloaded and validated, with no detected replay timing gaps or explicitly omitted candidates.
- `partial`: one or more requested files failed, replay timing gaps were found, non-ICAO targets were omitted, or the declared trace cap omitted candidates.
- `cancelled`: the operator stopped acquisition; completed files and observed candidates remain available for review.
- `budget_exhausted`: the cumulative payload budget was consumed before completing required files.

Partial outcomes return CLI exit code 3. A successful bounded download **always** has `coverage_complete=false`. A replay file is sampled receiver output: aircraft may cross the rectangle between samples, lack positions, fly below receiver coverage, or not transmit. An empty candidate list is not proof that no aircraft flew there. This format does not establish exhaustive coverage even when every requested file is present.

The index preserves raw position-source codes, their documented names, unknown source codes, non-ICAO markers, signed and missing altitudes, and sampling intervals. Callsign/squawk metadata rows are counted separately. Non-ICAO identities remain visible but this downloader does not request their full traces. If a trace cap is reached, regular identities are selected in lexicographic ICAO order; this is not an acoustic or relevance ranking. Every omitted identity has an explicit reason.

Replay altitude is quantized barometric altitude and is never substituted for an aircraft's geometric height. The downloaded full traces retain their original source fields and altitude slots. The separate native importer applies its existing explicit source, altitude, time and continuity policies.

## Resume, integrity and import

Repeating `fetch-adsblol-history` with the same plan and cache verifies all retained response hashes, reuses valid files and retries failed files. New immutable receipts preserve earlier failures and cancelled runs. A changed request requires a different cache directory. SHA-256 detects subsequent local changes; these hashes are not provider signatures or independent certification of the aircraft timestamps.

Only one process can write a cache at a time. A persistent empty `download.lock` is an operating-system lock control file: its existence does not mean a download is running, and process death releases the lock automatically. Resume and managed deletion acquire the same lock, so an active core download cannot be deleted through the Data library. Unreceipted interrupted payloads are retained, charged against the original budget and never silently reused as complete responses. A cache with altered registered bytes is rejected rather than silently refreshed.

Downloaded trace files are ordinary native-import-compatible readsb JSON. To import the set with a reviewed `TraceImportConfig`:

```powershell
geosonnar import-adsblol-history --cache regional-history --report regional-history/reports/00000001.json --config trace-config.json --out normalized-tracks.json
```

The import time window must remain inside the original replay selection interval. The caller can choose the trace time origin needed by the case; the geometric-altitude and accepted-source policies remain explicit. A partial acquisition requires the additional `--allow-partial true` option after reviewing the omissions. Imported quality notes retain the acquisition status and limitations. No partial trace set is relabeled as complete sky coverage.

The Data library registers the request, original HTTP responses, detailed traces, all acquisition snapshots and source hashes as owned case-local data. Fetch/resume and native import are separate actions. Import creates a new normalized data entry; aircraft associations still require analyst approval. Deleting an acquisition removes its registered owned files individually and preserves the original recording and independently imported datasets.

## Provider format and validation

The endpoint layout and wire format were checked against the actual [ADSB.lol map JavaScript](https://adsb.lol/script_22b772ac55265ba79eeeca99ab34165c.js), preserved locally with SHA-256 `8cce1dff88b156e5323dca610ed643b10b60759ec3eff5c0206e9e82089d82dd`. This identifies the implementation inspected for this decoder; it does not assert that every future provider release uses an unchanged format. Original flight observations retain the provider's [ODbL attribution](https://www.adsb.lol/docs/open-data/historical). Acquisition shares native import's row-shape, relative-time and flag validation; malformed traces remain explicit failed responses while other downloaded traces can still be imported with partial-use approval.

Offline controls independently construct legacy indexed and current replay layouts, malformed inputs, date-line regions, source types, cap omissions, network failures, budget exhaustion, cancellation, cache tampering and managed-file deletion. An optional fixture check compares the preserved April 2024 replay against the independent Python index (615 positions and 19 regular ICAO identities) and compares native full-trace import results byte-for-byte at the normalized JSON value level. These establish acquisition and parsing behavior, not recording geolocation accuracy.

A bounded live check on November 3, 2026 local time (November 4 UTC) fetched the same predeclared April 2024 region through the preserved native CLI. It used **12,921,791 payload bytes** of its 25 MB budget, recovered the same 615 indexed positions and 19 full traces, and matched every decoded trace SHA-256 against the earlier independent acquisition. Native import reproduced **42 passes and 1,901 points**, including identical source metadata, timestamps, ECEF coordinates and segment boundaries. A subsequent resume reused all 21 successful files with zero transferred bytes. The initial restricted-network attempt correctly returned partial status and exit code 3; its two zero-byte DNS failures remain in the receipt. Executable/source snapshots, logs, request hashes and comparison receipts are under `output/benchmarks/adsblol-history-001/live/`. The fresh cache is `data/adsblol-history-live-001/`. This verifies acquisition/import consistency and does not validate a recording's location.
