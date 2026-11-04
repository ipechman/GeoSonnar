# Historical Nordic lightning

GeoSonnar can explicitly request historical FMI/NORDLIS observations without an account or Frost credential. The provider describes Northern European coverage; locations outside that region require another source. Data is attributed to FMI/NORDLIS under [CC BY 4.0](https://en.ilmatieteenlaitos.fi/open-data-licence).

In **Data → Get data → Request lightning**, select **FMI/NORDLIS**, enter the UTC interval and south/west/north/east bounds, then plan the request. Copying the search window is a separate action with visible time lookback and spatial padding. Download/resume, import, and load are separate actions; none approves an acoustic event or a strike association. The stored request remains available in the data manager for reopening, cancellation, resumption or owned deletion. A complete response with zero strikes creates no lightning layer.

The equivalent CLI uses the same managed case catalog:

```json
{
  "start_utc": "2025-07-19T16:00:00Z",
  "end_utc": "2025-07-19T16:01:00Z",
  "bounds": [65.0, 13.0, 66.0, 15.0],
  "max_total_bytes": 1048576
}
```

```powershell
geosonnar plan-fmi-lightning --case CASE/case.json --request request.json
geosonnar fetch-fmi-lightning --case CASE/case.json --id CATALOG_ID
geosonnar fmi-lightning-status --case CASE/case.json --id CATALOG_ID
geosonnar import-fmi-lightning --case CASE/case.json --id CATALOG_ID
```

Planning and status inspection make no network requests. Download completion means every bounded query succeeded; it never establishes complete lightning detection or verifies a location. Fetch returns exit code 3 for a partial or cancelled result. Import creates a separate normalized catalog entry and leaves the raw acquisition intact.

The initial implementation allows up to one hour, 30 degrees of latitude and 60 degrees of longitude, with no date-line crossing. These are application limits, not claims about provider coverage. Requests are divided into one-minute queries. Each response is limited to 8 MiB, the cumulative response budget to 64 MiB, and the aggregate source rows to 250,000. Failed and interrupted response bytes consume the same budget. One over-limit byte may be retained to establish that the limit was exceeded; it is charged too. Each transfer has a 45-second network timeout. The five-minute session budget is checked before starting each chunk; an active transfer and local validation can finish afterward. A stored request permits at most 240 attempts. Requests run serially, stop on the first error, and never retry automatically. HTTP 429 or 503 requires at least a one-minute pause before explicit resume; this is an application minimum, not a provider retry guarantee.

Original XML, request URLs, attempt receipts and hashes are retained. Cached completed chunks are revalidated and reused without transferring them again. Adjacent queries use half-open ownership, so a boundary row belongs to the following minute; equal timestamps or coordinates within a response are not deduplicated. XML external entities and remote schema fetching are disabled. Unrecognized or changed files and reparse points stop recovery and deletion rather than being adopted. A process killed during the tiny atomic metadata-staging window can leave an unrecognized `.pending-*` file that requires inspection; automatic recovery does not guess its ownership.

Before using loaded FMI data in thunder scoring, review **FMI source interpretation** in Data or Search:

- Coordinates retain their ETRS89 source label. Record a reason for the explicit ETRS89-as-WGS84 approximation used by the existing geometry. This is not a coordinate transformation.
- Timestamps remain whole seconds with unknown rounding and unestablished physical timing accuracy. Declare at least one second of representation allowance and explicitly set the thunder residual scale at least that large. The allowance is a review floor, not a calibrated Gaussian uncertainty or a correction to the timestamps.
- Strike altitude is unknown. The existing explicit height fallback is still required. Cloud/ground classification does not establish acoustic-source height.
- Peak current, multiplicity, cloud indicator and raw ellipse size remain source attributes. Conflicting published ellipse units prevent using that value as a location-error radius or score weight. Peak current is not converted to acoustic intensity.

Apply and clear are explicit, undoable actions. The interpretation is bound to the exact catalog hash and accompanies saved investigations and exports. Loading a different catalog requires a new review. An inactive thunder term does not require this approval for Doppler-only work.

The public fixture in `tests/fixtures/fmi/` contains 36 observations, independently cross-checked against FMI's simple-feature response. Parser, restart, cancellation, budget, source-policy and archive checks exercise data handling; they are not a known-microphone storm geolocation validation. Independent hybrid location accuracy and a calibrated false-match decision remain unestablished.
