# Reviewing complementary and conflicting evidence

The purpose of combined scoring is also to narrow clips whose Doppler and thunder evidence are individually ambiguous. A different single best cell does not necessarily mean the evidence conflicts. GeoSonnar therefore exposes both the best locations and the larger low-score sets on the exact same searched grid.

Measurement review, sound-class assessment and particular source associations are separate decisions. A clear measured ridge is not necessarily an aircraft, and a low-frequency burst is not necessarily thunder. Confirmed selected observations require a compatible source-class assessment or an explicitly uncertain hypothesis with a reason before they can contribute to a workspace comparison. An unassessed or incompatible observation blocks the affected mode instead of disappearing from its score. A requested Doppler-only comparison can still run when excluded thunder observations are unassessed; combined scoring requires both active classes to pass. Uncertainty reasons remain visible and do not secretly downweight or otherwise improve a fit.

After a comparison, open **Results → Doppler, thunder and their combined search**. It shows each modality's coordinates, score, available receiver count, low-score cell count, distance between the two best cells, distance from the combined estimate to each, and search-edge flags. **Inspect Doppler/Thunder/Combined** selects that receiver in the residual table and pins it on the Search map.

The map uses blue for Doppler, orange for thunder and purple for combined estimates. A connecting line makes their separation visible. Green rings identify cells present in both low-score sets. Separate map controls toggle these estimate markers and the overlap.

## Interpreting overlap

The display includes cells with a modality score no more than **best score + delta**, initially **1.0**. The adjustable delta is an additive residual-score tolerance, not a probability, confidence level or automatically learned threshold. Changing it recalculates only the display diagnostics: observations, physics, scores, weights and winners stay unchanged. Saved comparison reports include the chosen tolerance and complete diagnostic coordinates.

- **Same best cell:** both modalities prefer the same grid point. Broad or multiple low-score areas may still remain.
- **Different peaks with overlap:** the separate best locations differ, but their wider sets share cells. This can be useful complementary evidence; it is not automatically rejected.
- **Disjoint sets:** no shared cell exists at the displayed tolerance. Review associations, shared clock, search extent, heights and propagation assumptions. Combined ranking remains available; this diagnostic does not accept or reject a location.
- **Not comparable:** one modality is disabled or lacks rankable support. Missing selected terms are never silently removed to improve a score.

A best cell or low-score set reaching the search boundary may indicate that the region excludes better alternatives. Overlap is a discrete grid intersection, not a continuous-space uncertainty region. Adjusting a tolerance until two sets overlap is not independent evidence of correctness.

## Calculation and evidence identity

Diagnostics derive from the already-computed residuals; they do not rerun propagation or change associations. Doppler combines whichever frequency/timing terms the user selected, normalizing their declared weights within Doppler. Thunder uses the selected strike-arrival term. Combined retains the actual complete selected-term score. Scores from different modalities are therefore descriptive values with different normalizations, not comparable likelihoods.

The default combined configuration gives one weight to each of frequency, Doppler timing and thunder timing: two Doppler terms and one thunder term. This is not equal modality weighting or a claim of statistical independence. Event samples remain averaged within events before aggregation.

Unavailable terms remain unavailable. If an input is invalid before scoring—for example, a selected aircraft has no imported trajectory—the request returns its explicit error rather than manufacturing a partial combined comparison. If individual candidates lack support in a produced report, diagnostics can still show a complete available modality while keeping combined unavailable.

## Evidence so far

The frozen [synthetic ablation](../benchmarks/HYBRID_ABLATION.md) did not show improvement over strong two-aircraft Doppler evidence. In its separate single-aircraft stratum, combined scoring brought all 12 cases within 500 m, versus seven for Doppler alone. Wrong aircraft and clocks still produced large errors. These findings motivate visible complementary regions and disagreement review, not automatic acceptance or universal fusion.

New native controls test overlapping broad sets whose separate peaks differ but whose combined estimate narrows the common support; disjoint alternatives, missing terms, declared boundaries, deterministic ties and serialization. Headless GUI controls exercise display, profile editing and resizing without operating the user's desktop. Real hybrid accuracy still requires independently verified recordings and matching datasets; see [storm validation](STORM_VALIDATION.md).
