//! Descriptive comparison of evidence modalities on one unchanged receiver grid.
//! A broad or multimodal estimate can still complement another modality even
//! when their best cells differ. Nothing here accepts/rejects a location, fits
//! weights, assigns confidence, or calls the propagation model again.
use crate::evidence_score::{
    CandidateScore, EvidenceTerm, ObserverLocation, ScoreConfig, ScoreReport, TermSettings,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

// Match the underlying evidence scorer; the interactive workspace imposes its
// own smaller grid limit before reaching this read-only summary.
const MAX_CANDIDATES: usize = 100_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticOptions {
    /// Include cells whose modality score is at most this amount above its best
    /// score. This is a displayed residual-score tolerance, not a confidence
    /// interval, hypothesis test, or automatic acceptance threshold.
    pub near_best_score_delta: f64,
}

impl Default for DiagnosticOptions {
    fn default() -> Self {
        Self {
            near_best_score_delta: 1.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RankedObserver {
    pub candidate_id: String,
    pub observer: ObserverLocation,
    pub rank: usize,
    pub score: f64,
    /// Unknown when the caller has supplied no search bounds.
    pub on_search_boundary: Option<bool>,
    pub outside_search_bounds: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModalitySummary {
    pub selected: bool,
    pub selected_terms: Vec<EvidenceTerm>,
    pub rankable_count: usize,
    /// Zero when this modality is not selected; otherwise every receiver without
    /// all of its selected terms is counted, never silently reweighted.
    pub unavailable_count: usize,
    pub unavailable_reason: Option<String>,
    pub ranked: Vec<RankedObserver>,
    pub best: Option<RankedObserver>,
    pub near_best_candidate_ids: Vec<String>,
    pub near_best_touches_boundary: Option<bool>,
    pub near_best_includes_outside_bounds: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonStatus {
    NotComparable,
    SharedBest,
    DifferentBestWithOverlap,
    DisjointNearBest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModalityComparison {
    pub status: ComparisonStatus,
    pub best_separation_m: Option<f64>,
    pub combined_to_doppler_m: Option<f64>,
    pub combined_to_thunder_m: Option<f64>,
    /// Intersection of discrete near-best cells on the exact same receiver grid.
    /// This is not a continuous spatial confidence region.
    pub overlap_candidate_ids: Vec<String>,
    pub overlap_count: usize,
    pub combined_best_in_overlap: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModalityDiagnostics {
    pub schema_version: u32,
    pub candidate_count: usize,
    pub options: DiagnosticOptions,
    pub bounds: Option<[f64; 4]>,
    pub doppler: ModalitySummary,
    pub thunder: ModalitySummary,
    pub combined: ModalitySummary,
    pub comparison: ModalityComparison,
    pub location_verified: bool,
    pub notes: Vec<String>,
}

fn settings(config: &ScoreConfig, term: EvidenceTerm) -> &TermSettings {
    match term {
        EvidenceTerm::DopplerFrequency => &config.frequency,
        EvidenceTerm::DopplerTiming => &config.doppler_timing,
        EvidenceTerm::ThunderTiming => &config.thunder_timing,
    }
}

fn checked_mean(candidate: &CandidateScore, term: EvidenceTerm) -> Option<f64> {
    let contribution = candidate.terms.iter().find(|t| t.term == term)?;
    contribution
        .available
        .then_some(contribution.mean_normalized_squared_residual)
        .flatten()
        .filter(|mean| mean.is_finite() && *mean >= 0.0)
}

fn finite_position(point: &ObserverLocation) -> bool {
    point.latitude_deg.is_finite()
        && point.longitude_deg.is_finite()
        && point.ellipsoid_height_m.is_finite()
        && (-90.0..=90.0).contains(&point.latitude_deg)
        && (-180.0..=180.0).contains(&point.longitude_deg)
}

fn modality_score(
    candidate: &CandidateScore,
    terms: &[EvidenceTerm],
    config: &ScoreConfig,
) -> Option<f64> {
    let total = terms
        .iter()
        .map(|&t| settings(config, t).weight)
        .sum::<f64>();
    let mut score = 0.0;
    for &term in terms {
        // All selected terms are required. Do not turn a missing timing term
        // into an implicit frequency-only estimate or vice versa.
        score += checked_mean(candidate, term)? * (settings(config, term).weight / total);
    }
    score.is_finite().then_some(score)
}

fn boundary(point: &ObserverLocation, bounds: Option<[f64; 4]>) -> (Option<bool>, Option<bool>) {
    let Some([south, west, north, east]) = bounds else {
        return (None, None);
    };
    let tolerance = 1e-9;
    let outside = point.latitude_deg < south - tolerance
        || point.latitude_deg > north + tolerance
        || point.longitude_deg < west - tolerance
        || point.longitude_deg > east + tolerance;
    let edge = !outside
        && ((point.latitude_deg - south).abs() <= tolerance
            || (point.latitude_deg - north).abs() <= tolerance
            || (point.longitude_deg - west).abs() <= tolerance
            || (point.longitude_deg - east).abs() <= tolerance);
    (Some(edge), Some(outside))
}

fn summarize_modality(
    report: &ScoreReport,
    terms: Vec<EvidenceTerm>,
    bounds: Option<[f64; 4]>,
    options: &DiagnosticOptions,
    use_report_score: bool,
    not_selected_reason: &str,
) -> ModalitySummary {
    let selected = !terms.is_empty();
    let mut ranked = Vec::new();
    if selected {
        for candidate in &report.candidates {
            if !finite_position(&candidate.candidate.observer) {
                continue;
            }
            let Some(score) = (if use_report_score {
                candidate.score
            } else {
                modality_score(candidate, &terms, &report.config)
            }) else {
                continue;
            };
            let (edge, outside) = boundary(&candidate.candidate.observer, bounds);
            ranked.push(RankedObserver {
                candidate_id: candidate.candidate.id.clone(),
                observer: candidate.candidate.observer.clone(),
                rank: 0,
                score,
                on_search_boundary: edge,
                outside_search_bounds: outside,
            });
        }
    }
    ranked.sort_by(|a, b| {
        a.score
            .total_cmp(&b.score)
            .then_with(|| a.candidate_id.cmp(&b.candidate_id))
    });
    for (i, candidate) in ranked.iter_mut().enumerate() {
        candidate.rank = i + 1;
    }
    let best = ranked.first().cloned();
    let near: Vec<_> = best
        .as_ref()
        .map(|best| {
            ranked
                .iter()
                .filter(|c| c.score - best.score <= options.near_best_score_delta)
                .collect()
        })
        .unwrap_or_default();
    let unavailable_reason = if !selected {
        Some(not_selected_reason.into())
    } else if ranked.is_empty() {
        Some("No receiver has a score with all selected terms available; inspect missing evidence and candidate reasons.".into())
    } else {
        None
    };
    ModalitySummary {
        selected,
        selected_terms: terms,
        rankable_count: ranked.len(),
        unavailable_count: if selected {
            report.candidates.len() - ranked.len()
        } else {
            0
        },
        unavailable_reason,
        best,
        near_best_candidate_ids: near.iter().map(|c| c.candidate_id.clone()).collect(),
        near_best_touches_boundary: bounds
            .filter(|_| !near.is_empty())
            .map(|_| near.iter().any(|c| c.on_search_boundary == Some(true))),
        near_best_includes_outside_bounds: bounds
            .filter(|_| !near.is_empty())
            .map(|_| near.iter().any(|c| c.outside_search_bounds == Some(true))),
        ranked,
    }
}

/// Horizontal great-circle separation on a mean-radius Earth. Height is shown
/// separately in each estimate; separation is not a slant propagation distance.
pub fn horizontal_separation_m(a: &ObserverLocation, b: &ObserverLocation) -> f64 {
    let p = a.latitude_deg.to_radians();
    let q = b.latitude_deg.to_radians();
    let dl = (a.longitude_deg - b.longitude_deg).to_radians();
    let hav = ((p - q) / 2.0).sin().powi(2) + p.cos() * q.cos() * (dl / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * hav.clamp(0.0, 1.0).sqrt().asin()
}

/// Derive the separate estimates from already computed term residuals. Disabled
/// terms stay disabled and missing selected terms never silently disappear.
/// `bounds` describes the actual search rectangle, not an inferred confidence
/// area. Passing None leaves edge diagnostics unknown.
pub fn summarize(
    report: &ScoreReport,
    bounds: Option<[f64; 4]>,
    options: &DiagnosticOptions,
) -> Result<ModalityDiagnostics, String> {
    if !options.near_best_score_delta.is_finite() || options.near_best_score_delta < 0.0 {
        return Err("Near-best residual-score delta must be finite and nonnegative".into());
    }
    if report.candidates.is_empty() || report.candidates.len() > MAX_CANDIDATES {
        return Err(format!(
            "Diagnostics require 1..{MAX_CANDIDATES} receiver candidates"
        ));
    }
    if let Some([s, w, n, e]) = bounds
        && (![s, w, n, e].iter().all(|x| x.is_finite())
            || s < -90.0
            || n > 90.0
            || w < -180.0
            || e > 180.0
            || s >= n
            || w >= e)
    {
        return Err("Diagnostic bounds must be an ordered geographic rectangle".into());
    }
    let selected: Vec<_> = [
        EvidenceTerm::DopplerFrequency,
        EvidenceTerm::DopplerTiming,
        EvidenceTerm::ThunderTiming,
    ]
    .into_iter()
    .filter(|&term| settings(&report.config, term).enabled)
    .collect();
    if selected.is_empty() || report.selected_terms != selected {
        return Err("Report selected terms disagree with its scoring configuration".into());
    }
    if selected.iter().any(|&term| {
        let term = settings(&report.config, term);
        !term.weight.is_finite() || term.weight <= 0.0
    }) || !selected
        .iter()
        .map(|&t| settings(&report.config, t).weight)
        .sum::<f64>()
        .is_finite()
    {
        return Err("Diagnostic term weights must be positive and finite".into());
    }
    let mut ids = BTreeSet::new();
    for candidate in &report.candidates {
        let point = &candidate.candidate.observer;
        if candidate.candidate.id.is_empty()
            || !ids.insert(&candidate.candidate.id)
            || (!finite_position(point)
                && (candidate.score.is_some() || candidate.terms.iter().any(|t| t.available)))
            || candidate.terms.len() != selected.len()
            || selected.iter().any(|&term| {
                let matching: Vec<_> = candidate.terms.iter().filter(|t| t.term == term).collect();
                matching.len() != 1 || matching[0].weight != settings(&report.config, term).weight
            })
        {
            return Err("Diagnostic candidates need unique IDs, finite coordinates and exactly the selected terms/weights".into());
        }
        if let Some(score) = candidate.score {
            let expected = modality_score(candidate, &selected, &report.config);
            if !score.is_finite()
                || score < 0.0
                || expected.is_none_or(|value| (score - value).abs() > 1e-10 * score.abs().max(1.0))
            {
                return Err(
                    "Report score is inconsistent with complete selected term means".into(),
                );
            }
        }
    }
    let doppler_terms: Vec<_> = selected
        .iter()
        .copied()
        .filter(|&t| t != EvidenceTerm::ThunderTiming)
        .collect();
    let thunder_terms: Vec<_> = selected
        .iter()
        .copied()
        .filter(|&t| t == EvidenceTerm::ThunderTiming)
        .collect();
    let both = !doppler_terms.is_empty() && !thunder_terms.is_empty();
    let doppler = summarize_modality(
        report,
        doppler_terms,
        bounds,
        options,
        false,
        "No Doppler term is selected.",
    );
    let thunder = summarize_modality(
        report,
        thunder_terms,
        bounds,
        options,
        false,
        "Thunder timing is not selected.",
    );
    let combined = summarize_modality(
        report,
        if both { selected } else { Vec::new() },
        bounds,
        options,
        true,
        "Select Doppler and thunder terms to compare their combined estimate.",
    );
    let thunder_near: BTreeSet<_> = thunder.near_best_candidate_ids.iter().collect();
    let overlap: Vec<_> = doppler
        .near_best_candidate_ids
        .iter()
        .filter(|id| thunder_near.contains(id))
        .cloned()
        .collect();
    let comparable = doppler.best.as_ref().zip(thunder.best.as_ref());
    let status = match comparable {
        None => ComparisonStatus::NotComparable,
        Some((d, t)) if d.candidate_id == t.candidate_id => ComparisonStatus::SharedBest,
        Some(_) if overlap.is_empty() => ComparisonStatus::DisjointNearBest,
        Some(_) => ComparisonStatus::DifferentBestWithOverlap,
    };
    let separation = |a: &ModalitySummary, b: &ModalitySummary| {
        a.best
            .as_ref()
            .zip(b.best.as_ref())
            .map(|(a, b)| horizontal_separation_m(&a.observer, &b.observer))
    };
    let comparison = ModalityComparison {
        status,
        best_separation_m: separation(&doppler, &thunder),
        combined_to_doppler_m: separation(&combined, &doppler),
        combined_to_thunder_m: separation(&combined, &thunder),
        overlap_count: overlap.len(),
        combined_best_in_overlap: comparable
            .and(combined.best.as_ref())
            .map(|best| overlap.contains(&best.candidate_id)),
        overlap_candidate_ids: overlap,
    };
    Ok(ModalityDiagnostics {
        schema_version: 1,
        candidate_count: report.candidates.len(),
        options: options.clone(),
        bounds,
        doppler,
        thunder,
        combined,
        comparison,
        location_verified: false,
        notes: vec![
            "All estimates use the same receiver candidates, selected evidence, clock, heights, atmospheric assumptions and associations; no propagation model is rerun.".into(),
            "Each modality renormalizes only its selected term weights. Missing selected terms stay unavailable; the combined estimate preserves the original score availability.".into(),
            "Near-best cells are within the displayed additive residual-score tolerance of each modality's own best. They are discrete support sets, not confidence or acceptance regions.".into(),
            "Separated best cells can still represent complementary weak evidence when their near-best sets overlap. Disjoint sets suggest reviewing associations, clock, coverage and model assumptions, but do not automatically reject a location.".into(),
            "Agreement does not establish correctness: shared errors or a restricted search can move every estimate. Boundary flags describe declared bounds, not global uniqueness.".into(),
        ],
    })
}
