# Reviewed discovery to flight matching

The Rust `prepare-reviewed-search` stage now transfers explicitly selected,
reviewed automatic ridges into the existing two-event flight matcher. It checks
the recording, discovery report, review decisions and group memberships before
copying the original time/frequency samples. There is no clipping, gap filling,
harmonic rescaling or selection based on the resulting location score.

## Review and run

For an existing discovery report, generate a fresh review page with the updated
executable. This does not rerun discovery or change the measurements:

```powershell
.\dist\geosonnar.exe review-discovery --input output/benchmarks/discovery/original-hour-001/discovery.json --out output/benchmarks/discovery/original-hour-001/review-v2-new.html
```

Open that page, review the acoustic groups, choose **Keep for matching** for the
groups to use, and save `review-selection.json`. Every new page starts unreviewed.
Its schema-version-2 export binds the exact bytes of `discovery.json` as well as
the recording SHA-256, channel and complete group/event membership. The page
records `review_actor: "human"`; the benchmark below separately declares its
source-informed `"agent"` review. A group kept for matching is not a confirmed
aircraft or a verified location.

Old schema-version-1 review files are rejected: regenerate the page and make the
review choices again. There is no automatic migration or implicit acceptance of
old selections. The review page's chart/export logic is code-tested; browser
visual verification remains unavailable because local-file navigation was blocked.

The generated benchmark bridge file is a complete, runnable example. To repeat
the **same source-informed agent selection**, copy it to a new config and run:

```powershell
Copy-Item -LiteralPath output/benchmarks/reviewed-search/comparison-001/regional-detailed-bridge.json -Destination cases/reviewed-main-pair-new.json
.\dist\geosonnar.exe prepare-reviewed-search --config cases/reviewed-main-pair-new.json --out output/reviewed-main-pair-new
.\dist\geosonnar.exe rank --input output/reviewed-main-pair-new/search_input.json --out output/reviewed-main-pair-new/results.json
```

The copied config uses absolute paths to the frozen discovery, original search
template and explicitly labelled **agent** review. To use your own decisions,
change `review_selection` to the new exported file and describe the actual basis
in `selection_basis`. Select exactly two distinct groups, with one explicit
`event_id` from each. Both groups must be marked `keep_for_matching`. Supply the
emitted-frequency candidates in `f0_hz` for that measured harmonic; the program
does not infer a fundamental or scale its samples.

Each run requires a new output directory and writes:

- `search_input.json`, ready for the existing `rank` and `overlays` commands.
- `event-1.json` and `event-2.json`, with all selected measured samples.
- `receipt.json`, including source/review/config hashes, reviewer, selection
  basis, original quality and contrast weights, measured gaps, and other
  harmonics in the selected groups that were **not** scored.

A failed preparation cannot leave a completed-looking search input/receipt.
`rank` exit 3 means no candidate; exit 2 means an operational or input error.
Neither becomes a verified location. Matching uses equal sample weights within
each event and equal event weights. Contrast and quality/uncertainty are retained
as evidence; they are not calibrated likelihood weights in this matcher.

## Other recordings and locations

Instead of `template_search_input`, a bridge config may specify `search_config`
pointing to the normal geometry/data configuration used by `prepare-search`.
Exactly one of the two is required. This route reads normalized tracks, observer
bounds/terrain and the timing/model settings; `event_files` may be omitted, and
any old event file paths are not read. Reviewed discovery supplies both traces.

For example, the structure is:

```json
{
  "schema_version": 1,
  "discovery": "../output/my-recording/discovery.json",
  "review_selection": "../output/my-recording/review-selection.json",
  "search_config": "my-search-geometry.json",
  "selected_ridges": [
    {"harmonic_group_id": "harmonics-0001", "event_id": "event-0001", "f0_hz": [110, 110.5, 111]},
    {"harmonic_group_id": "harmonics-0002", "event_id": "event-0002", "f0_hz": [120, 120.5, 121]}
  ],
  "selection_basis": "Describe the actual acoustic review and supplied frequency assumptions."
}
```

The names and frequency lists above are illustrative, not settings suitable for
every recording. Config-relative paths resolve relative to their config file.
Optional `source_override` may point to a relocated copy of the recording; it
must have the identical SHA-256. This does not permit replacing the source with
another recording.

Date/flight coverage, observer region/heights, the first event's recording-time
CPA anchor, event separation/tolerance, shared clock sweep, coarse speed/distance
bounds and both emitted-frequency grids still require explicit assumptions.
Automatic discovery does not supply them. This stage connects reviewed
measurements to an existing experiment; it is not a blind worldwide search or
the planned native Windows GUI.

## Executed comparison

`reviewed_search_contract.json` fixed the source hashes, selected groups/ridges,
frequency grids, two search scopes and evaluation policy **before** the new
matching runs. The benchmark makes an explicitly source-informed agent selection
of `harmonics-0011/event-0011` and `harmonics-0013/event-0013`, already associated
acoustically with the two main reference events. It transfers **201 + 119 = 320
samples**, including the original 8 s and 11 s maximum gaps, unchanged.

The final executable SHA-256 used here is
`53a13a1e7001dbdf4c1fae9a03a76236131dfc373c8e47048d7fd3b807b7f44a`.
The report is
`output/benchmarks/reviewed-search/comparison-001/report.json`.
Both native predictions completed before the evaluator parsed the old result
files or external coordinate references. This is procedural separation on one
filesystem, not a sealed holdout. Templates contain manual traces, but the bridge
replaces those traces completely before ranking; they contribute no new score.

| Search scope | Result from reviewed automatic ridges | Comparison with manual traces |
| --- | --- | --- |
| Original regional search: 6,661 continuous tracks, 2,856 terrain observers on the 200 m grid | Same winning pair, `a1dfeb#part1` / `ace391#part3`; 115 ranked hypotheses | Same winning grid point, **0 m displacement**; 110.22 m from the video's reported coordinate; fitted clock differs by +1.00 s |
| Existing fine comparison: the manual baseline's two tracks and 169 observers on its 25 m grid | **40.8763497586, -73.5249954611**; 169 ranked hypotheses | **50.00 m south** of the manual result; 13.42 m from the video coordinate and 37.32 m from the rounded platform coordinate; fitted clock differs by +0.758 s |

Both new winners lie inside their observer grid and shared-shift sweep, rather
than on a boundary. The fine pair and grid were inherited from the manual
baseline; this is **conditional comparison, not independent aircraft recovery or
automatic refinement**. The video coordinate is a reported acoustic estimate,
and the platform reference is rounded public data, neither surveyed microphone
truth. The manual fine result was 37.32 m from the video estimate and 33.37 m from
the platform reference. Being closer to one reference is not an established
accuracy improvement.

The [location comparison figure](../output/benchmarks/reviewed-search/comparison-001/location-comparison.png)
shows both scopes in local WGS84 metres. Its source-report and image hashes are
recorded in `comparison-001/plot-receipt.json`; the rendered image was inspected.

Combined RMSE is 0.639 Hz regionally and 0.564 Hz on the fine comparison, versus
1.388/1.361 Hz for the manual traces. The sample sets differ: the automatic ridges
are partial and cannot establish full-pass coverage. Lower RMSE does not prove
better geolocation or better measurements. Other harmonics in the first group
remain untested by the pair scorer, and the faint event is still missed by
discovery and needs manual marking.

All declared **software integrity** gates pass: exact sample transfer, unchanged
candidate data/settings/frequency grids, explicit untested harmonics, preserved
quality/weights/gaps, source/review binding, finite coherent outputs and stable
source/input/executable hashes. No geographic threshold or identity requirement
was adjusted to force this result to pass. `location_verified` remains false.
The benchmark took 87.29 s on this machine, including preparation, both searches,
hashing and evaluation; runtime is descriptive, with other audits running
concurrently. Ten independent Python failure controls check the evaluator.

## Reproduce the complete comparison

```powershell
python -m unittest discover -s benchmarks -p test_reviewed_search_harness.py -v
python analysis/benchmark_reviewed_search.py --exe dist/geosonnar.exe --output output/benchmarks/reviewed-search/new-comparison
```

The harness verifies all frozen source hashes, snapshots the executable, writes
the labelled agent review and both bridge configs, and preserves command logs,
input receipts, predictions and evaluation. A damaged input, changed search
space, missing result or execution failure fails the audit. A coherent
`no_candidate` result would remain an explicit inconclusive scientific outcome,
not a claimed successful location. No matching settings are retuned from results.

Audio-derived timing/frequency priors and refinement of newly ranked candidates
are now implemented in the separate [audio-prior experiment](AUDIO_PRIORS.md).
Its first run retains an incomplete-coverage flag at a frequency limit; the
earlier fixed-prior comparison above is preserved unchanged.
