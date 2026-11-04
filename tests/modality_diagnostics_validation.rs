//! Fabricated score surfaces; these test comparison contracts, not field accuracy.
use geosonnar::{
    evidence_score::{
        CandidateEvidence, CandidateScore, EvidenceTerm, ObserverLocation, ScoreConfig,
        ScoreReport, TermContribution,
    },
    modality_diagnostics::{self, ComparisonStatus, DiagnosticOptions},
};
use std::collections::BTreeMap;

const TERMS: [EvidenceTerm; 3] = [
    EvidenceTerm::DopplerFrequency,
    EvidenceTerm::DopplerTiming,
    EvidenceTerm::ThunderTiming,
];

fn report(means: &[[Option<f64>; 3]], enabled: [bool; 3], weights: [f64; 3]) -> ScoreReport {
    let mut config = ScoreConfig::default();
    for (i, setting) in [
        &mut config.frequency,
        &mut config.doppler_timing,
        &mut config.thunder_timing,
    ]
    .into_iter()
    .enumerate()
    {
        setting.enabled = enabled[i];
        setting.weight = weights[i];
    }
    let selected_terms: Vec<_> = TERMS
        .into_iter()
        .enumerate()
        .filter_map(|(i, term)| enabled[i].then_some(term))
        .collect();
    let total_weight: f64 = weights
        .iter()
        .enumerate()
        .filter_map(|(i, weight)| enabled[i].then_some(weight))
        .sum();
    let mut candidates: Vec<_> = means
        .iter()
        .enumerate()
        .map(|(i, means)| {
            let terms: Vec<_> = TERMS
                .into_iter()
                .enumerate()
                .filter(|(i, _)| enabled[*i])
                .map(|(index, term)| TermContribution {
                    term,
                    weight: weights[index],
                    residual_scale: 1.0,
                    available: means[index].is_some(),
                    unavailable_reason: means[index]
                        .is_none()
                        .then(|| "Fabricated missing evidence".into()),
                    residuals: Vec::new(),
                    event_mean_squared_residuals: means[index]
                        .map(|m| BTreeMap::from([("fabricated-event".into(), m)]))
                        .unwrap_or_default(),
                    mean_normalized_squared_residual: means[index],
                    weighted_contribution: means[index].map(|m| m * weights[index] / total_weight),
                })
                .collect();
            let complete = terms.iter().all(|term| term.available);
            CandidateScore {
                candidate: CandidateEvidence {
                    id: format!("receiver-{i:05}"),
                    observer: ObserverLocation {
                        latitude_deg: 0.01,
                        longitude_deg: i as f64 * 0.01,
                        ellipsoid_height_m: 80.0,
                    },
                    recording_start_utc_s: 1_788_825_600.0,
                    frequency: Vec::new(),
                    doppler_timing: Vec::new(),
                },
                rank: None,
                score: complete.then(|| terms.iter().filter_map(|t| t.weighted_contribution).sum()),
                terms,
                unavailable_reasons: if complete {
                    Vec::new()
                } else {
                    vec!["Fabricated missing term".into()]
                },
                location_verified: false,
            }
        })
        .collect();
    let mut order: Vec<usize> = (0..candidates.len())
        .filter(|&i| candidates[i].score.is_some())
        .collect();
    order.sort_by(|&a, &b| {
        candidates[a]
            .score
            .unwrap()
            .total_cmp(&candidates[b].score.unwrap())
            .then_with(|| candidates[a].candidate.id.cmp(&candidates[b].candidate.id))
    });
    for (i, &index) in order.iter().enumerate() {
        candidates[index].rank = Some(i + 1);
    }
    ScoreReport {
        schema_version: 1,
        config,
        selected_terms,
        evidence_sha256: "0".repeat(64),
        ranked_candidate_ids: order
            .iter()
            .map(|&i| candidates[i].candidate.id.clone())
            .collect(),
        candidates,
        calibration: "uncalibrated fabricated residuals".into(),
        location_verified: false,
        limitations: vec!["Fabricated score surfaces, not recorded audio".into()],
        modalities: None,
    }
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn separate_scores_renormalize_declared_weights_within_modality() {
    let report = report(
        &[[Some(2.0), Some(8.0), Some(12.0)]],
        [true; 3],
        [3.0, 1.0, 2.0],
    );
    let before = serde_json::to_vec(&report).unwrap();
    let result =
        modality_diagnostics::summarize(&report, None, &DiagnosticOptions::default()).unwrap();
    close(result.doppler.best.as_ref().unwrap().score, 3.5);
    close(result.thunder.best.as_ref().unwrap().score, 12.0);
    close(result.combined.best.as_ref().unwrap().score, 38.0 / 6.0);
    assert_eq!(result.comparison.status, ComparisonStatus::SharedBest);
    assert_eq!(result.comparison.best_separation_m, Some(0.0));
    assert_eq!(result.comparison.overlap_count, 1);
    assert!(!result.location_verified);
    assert_eq!(
        serde_json::to_vec(&report).unwrap(),
        before,
        "Diagnostics must not alter the evidence, score or rank"
    );
}

#[test]
fn weak_overlapping_modalities_can_support_a_combined_cell_despite_separated_peaks() {
    let report = report(
        &[
            [Some(0.0), None, Some(3.0)],
            [Some(0.6), None, Some(0.6)],
            [Some(3.0), None, Some(0.0)],
        ],
        [true, false, true],
        [1.0; 3],
    );
    let options = DiagnosticOptions {
        near_best_score_delta: 0.75,
    };
    let result = modality_diagnostics::summarize(&report, None, &options).unwrap();
    assert_eq!(
        result.comparison.status,
        ComparisonStatus::DifferentBestWithOverlap
    );
    assert!(result.comparison.best_separation_m.unwrap() > 2200.0);
    assert_eq!(result.doppler.near_best_candidate_ids.len(), 2);
    assert_eq!(result.thunder.near_best_candidate_ids.len(), 2);
    assert_eq!(result.combined.near_best_candidate_ids, ["receiver-00001"]);
    assert_eq!(result.comparison.overlap_candidate_ids, ["receiver-00001"]);
    assert_eq!(result.combined.best.unwrap().candidate_id, "receiver-00001");
    assert_eq!(result.comparison.combined_best_in_overlap, Some(true));
}

#[test]
fn disjoint_evidence_is_visible_without_automatic_rejection_or_confidence() {
    let report = report(
        &[
            [Some(0.0), Some(0.0), Some(10.0)],
            [Some(10.0), Some(10.0), Some(0.0)],
        ],
        [true; 3],
        [1.0; 3],
    );
    let result =
        modality_diagnostics::summarize(&report, None, &DiagnosticOptions::default()).unwrap();
    assert_eq!(result.comparison.status, ComparisonStatus::DisjointNearBest);
    assert_eq!(result.comparison.overlap_count, 0);
    assert!(result.comparison.best_separation_m.unwrap() > 1100.0);
    assert_eq!(result.comparison.combined_best_in_overlap, Some(false));
    assert_eq!(
        result.combined.rankable_count, 2,
        "Disagreement is a review diagnostic, not a rejection gate"
    );
    assert!(!result.location_verified);
}

#[test]
fn missing_thunder_retains_the_doppler_estimate_but_never_invents_a_combined_score() {
    let report = report(
        &[[Some(0.0), Some(0.0), None], [Some(2.0), Some(2.0), None]],
        [true; 3],
        [1.0; 3],
    );
    let result =
        modality_diagnostics::summarize(&report, None, &DiagnosticOptions::default()).unwrap();
    assert_eq!(result.doppler.rankable_count, 2);
    assert!(result.thunder.selected);
    assert_eq!(result.thunder.unavailable_count, 2);
    assert!(result.thunder.best.is_none());
    assert!(result.combined.selected);
    assert_eq!(result.combined.unavailable_count, 2);
    assert!(result.combined.best.is_none());
    assert_eq!(result.comparison.status, ComparisonStatus::NotComparable);
    assert_eq!(result.comparison.best_separation_m, None);
    assert_eq!(result.comparison.combined_best_in_overlap, None);
}

#[test]
fn missing_selected_doppler_timing_cannot_silently_become_frequency_only() {
    let report = report(
        &[
            [Some(0.0), None, Some(0.0)],
            [Some(2.0), Some(2.0), Some(2.0)],
        ],
        [true; 3],
        [1.0; 3],
    );
    let result =
        modality_diagnostics::summarize(&report, None, &DiagnosticOptions::default()).unwrap();
    assert_eq!(result.doppler.unavailable_count, 1);
    assert_eq!(result.doppler.best.unwrap().candidate_id, "receiver-00001");
    assert_eq!(result.thunder.best.unwrap().candidate_id, "receiver-00000");
    assert_eq!(result.combined.best.unwrap().candidate_id, "receiver-00001");
}

#[test]
fn thunder_only_is_explicit_and_does_not_fabricate_disabled_doppler_terms() {
    let report = report(
        &[[None, None, Some(0.0)], [None, None, Some(1.0)]],
        [false, false, true],
        [1.0; 3],
    );
    let result =
        modality_diagnostics::summarize(&report, None, &DiagnosticOptions::default()).unwrap();
    assert!(!result.doppler.selected);
    assert!(!result.combined.selected);
    assert_eq!(result.doppler.unavailable_count, 0);
    assert_eq!(result.combined.unavailable_count, 0);
    assert_eq!(result.thunder.rankable_count, 2);
    assert!(
        result
            .doppler
            .unavailable_reason
            .unwrap()
            .contains("selected")
    );
}

#[test]
fn boundary_and_outside_flags_use_declared_bounds_without_guessing() {
    let report = report(
        &[[Some(0.0); 3], [Some(0.1); 3], [Some(0.2); 3]],
        [true; 3],
        [1.0; 3],
    );
    let bounds = Some([0.0, 0.0, 0.02, 0.015]);
    let result =
        modality_diagnostics::summarize(&report, bounds, &DiagnosticOptions::default()).unwrap();
    assert_eq!(result.combined.ranked[0].on_search_boundary, Some(true));
    assert_eq!(result.combined.ranked[1].on_search_boundary, Some(false));
    assert_eq!(result.combined.ranked[2].on_search_boundary, Some(false));
    assert_eq!(result.combined.ranked[2].outside_search_bounds, Some(true));
    assert_eq!(result.combined.near_best_touches_boundary, Some(true));
    assert_eq!(
        result.combined.near_best_includes_outside_bounds,
        Some(true)
    );
    let unknown =
        modality_diagnostics::summarize(&report, None, &DiagnosticOptions::default()).unwrap();
    assert_eq!(unknown.combined.best.unwrap().on_search_boundary, None);
    assert_eq!(unknown.combined.near_best_touches_boundary, None);
}

#[test]
fn ties_are_deterministic_and_keep_the_whole_near_best_set() {
    let mut report = report(&[[Some(0.0); 3]; 3], [true; 3], [1.0; 3]);
    report.candidates.reverse();
    let result = modality_diagnostics::summarize(
        &report,
        None,
        &DiagnosticOptions {
            near_best_score_delta: 0.0,
        },
    )
    .unwrap();
    assert_eq!(result.combined.best.unwrap().candidate_id, "receiver-00000");
    assert_eq!(result.comparison.overlap_count, 3);
    assert_eq!(result.combined.near_best_candidate_ids.len(), 3);
}

#[test]
fn near_best_tolerance_is_explicit_and_never_changes_the_underlying_winner() {
    let report = report(
        &[[Some(0.0); 3], [Some(0.5); 3], [Some(2.0); 3]],
        [true; 3],
        [1.0; 3],
    );
    let narrow = modality_diagnostics::summarize(
        &report,
        None,
        &DiagnosticOptions {
            near_best_score_delta: 0.1,
        },
    )
    .unwrap();
    let broad = modality_diagnostics::summarize(
        &report,
        None,
        &DiagnosticOptions {
            near_best_score_delta: 3.0,
        },
    )
    .unwrap();
    assert_eq!(narrow.combined.near_best_candidate_ids.len(), 1);
    assert_eq!(broad.combined.near_best_candidate_ids.len(), 3);
    assert_eq!(
        narrow.combined.best.unwrap().candidate_id,
        broad.combined.best.unwrap().candidate_id
    );
    close(broad.options.near_best_score_delta, 3.0);
}

#[test]
fn inconsistent_reports_and_nonfinite_options_are_rejected() {
    let base = report(&[[Some(0.0); 3], [Some(2.0); 3]], [true; 3], [1.0; 3]);
    for delta in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(
            modality_diagnostics::summarize(
                &base,
                None,
                &DiagnosticOptions {
                    near_best_score_delta: delta
                }
            )
            .is_err()
        );
    }
    for action in 0..6 {
        let mut broken = base.clone();
        match action {
            0 => broken.candidates[0].score = Some(10.0),
            1 => broken.candidates[0].terms[0].available = false,
            2 => broken.candidates[1].candidate.id = broken.candidates[0].candidate.id.clone(),
            3 => {
                broken.selected_terms.pop();
            }
            4 => broken.candidates[0].candidate.observer.latitude_deg = f64::NAN,
            _ => broken.candidates[0].terms[0].weight = 2.0,
        }
        assert!(
            modality_diagnostics::summarize(&broken, None, &DiagnosticOptions::default()).is_err(),
            "Action {action} was not rejected"
        );
    }
    assert!(
        modality_diagnostics::summarize(
            &base,
            Some([1.0, 0.0, 0.0, 1.0]),
            &DiagnosticOptions::default()
        )
        .is_err()
    );
}

#[test]
fn serialized_diagnostics_preserve_coordinates_selection_and_tolerance() {
    let report = report(&[[Some(0.0); 3], [Some(3.0); 3]], [true; 3], [1.0; 3]);
    let result = modality_diagnostics::summarize(
        &report,
        Some([0.0, 0.0, 0.02, 0.02]),
        &DiagnosticOptions::default(),
    )
    .unwrap();
    let bytes = serde_json::to_vec(&result).unwrap();
    let recovered: modality_diagnostics::ModalityDiagnostics =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(recovered.comparison.status, ComparisonStatus::SharedBest);
    close(recovered.combined.best.unwrap().observer.latitude_deg, 0.01);
    close(recovered.options.near_best_score_delta, 1.0);
    assert_eq!(recovered.doppler.selected_terms.len(), 2);
    assert!(!recovered.location_verified);
}

#[test]
fn horizontal_separation_is_geographic_and_not_height_or_longitude_subtraction() {
    let a = ObserverLocation {
        latitude_deg: 0.0,
        longitude_deg: 179.999,
        ellipsoid_height_m: 0.0,
    };
    let b = ObserverLocation {
        latitude_deg: 0.0,
        longitude_deg: -179.999,
        ellipsoid_height_m: 10000.0,
    };
    let distance = modality_diagnostics::horizontal_separation_m(&a, &b);
    assert!((distance - 222.390_160_467).abs() < 0.00001);
    let above = ObserverLocation {
        ellipsoid_height_m: 2000.0,
        ..a.clone()
    };
    close(
        modality_diagnostics::horizontal_separation_m(&a, &above),
        0.0,
    );
}

#[test]
fn invalid_receivers_already_marked_unavailable_do_not_destroy_other_diagnostics() {
    let mut report = report(&[[None; 3], [Some(0.0); 3]], [true; 3], [1.0; 3]);
    report.candidates[0].candidate.observer.latitude_deg = f64::NAN;
    let result =
        modality_diagnostics::summarize(&report, None, &DiagnosticOptions::default()).unwrap();
    assert_eq!(result.combined.rankable_count, 1);
    assert_eq!(result.combined.unavailable_count, 1);
    assert_eq!(result.combined.best.unwrap().candidate_id, "receiver-00001");
}
