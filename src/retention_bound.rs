//! Retention-only certificate for whole-pair score-bound pruning.
//! This does not establish acoustic support or classify an unevaluated pair.

use super::WideCandidate;

/// Return a threshold that a proved pair lower bound must STRICTLY exceed.
///
/// The caller supplies the lists maintained by `retain_candidate`: both are
/// sorted by `order`, have at most `retain` rows, and `pairs` has unique ordered
/// flight IDs. Ordinary hypotheses may share flight IDs. These private caller
/// invariants are tested below, not re-audited or allocated on every pair.
///
/// Full local ordinary TopK protects the receiver winner and global ordinary
/// TopK. A better existing representative, or K better distinct local keys,
/// protects global distinct-pair TopK. Their maximum protects both destinations.
/// Equality is deliberately not a pruning certificate; encounter ties survive.
pub(super) fn cutoff(
    candidates: &[WideCandidate],
    pairs: &[WideCandidate],
    flight_ids: [&str; 2],
    retain: usize,
) -> Option<f64> {
    if retain == 0 || candidates.len() != retain {
        return None;
    }
    let ordinary = candidates.last()?.score.combined_rmse_hz;
    let representative = pairs.iter().find(|row| {
        row.score.flight_ids[0].as_str() == flight_ids[0]
            && row.score.flight_ids[1].as_str() == flight_ids[1]
    });
    let distinct = match representative {
        Some(row) => row.score.combined_rmse_hz,
        None if pairs.len() == retain => pairs.last()?.score.combined_rmse_hz,
        None => return None,
    };
    if !ordinary.is_finite() || ordinary < 0.0 || !distinct.is_finite() || distinct < 0.0 {
        return None;
    }
    Some(ordinary.max(distinct))
}

#[cfg(test)]
mod tests {
    use super::super::{order, retain_candidate};
    use super::{WideCandidate, cutoff};
    use crate::evidence_score::ObserverLocation;
    use crate::physics::{ClosestApproach, PairScore};

    // Fabricated retention scores test ordering only, not physical accuracy.
    fn row(ids: [&str; 2], score: f64, encounter: u64) -> WideCandidate {
        let cpa = ClosestApproach {
            emission_time_s: 0.0,
            arrival_time_s: 1.0,
            slant_distance_m: 343.0,
            speed_mps: 80.0,
            segment_index: 0,
            at_track_boundary: false,
        };
        WideCandidate {
            receiver_index: encounter / 20,
            observer: ObserverLocation {
                latitude_deg: 0.0,
                longitude_deg: 0.0,
                ellipsoid_height_m: 0.0,
            },
            score: PairScore {
                flight_ids: ids.map(str::to_owned),
                observer_xyz_m: [0.0; 3],
                rmse_hz: [score; 2],
                combined_rmse_hz: score,
                fitted_f0_hz: [100.0; 2],
                clock_offset_s: 0.0,
                shared_shift_s: 0.0,
                cpa: [cpa; 2],
                predicted_event2_cpa_recording_s: 1.0,
            },
            encounter_index: encounter,
        }
    }

    fn ranked(rows: &[WideCandidate], retain: usize, distinct: bool) -> Vec<WideCandidate> {
        let mut result = Vec::new();
        for candidate in rows {
            retain_candidate(&mut result, candidate.clone(), retain, distinct);
        }
        result
    }

    fn exact(rows: &[WideCandidate]) -> Vec<u8> {
        serde_json::to_vec(rows).unwrap()
    }

    #[test]
    fn ordinary_topk_must_be_full_even_when_pair_representative_exists() {
        let a = row(["A", "B"], 1.0, 0);
        assert_eq!(cutoff(&[], &[], ["A", "B"], 0), None);
        assert_eq!(cutoff(&[], std::slice::from_ref(&a), ["A", "B"], 1), None);
        assert_eq!(
            cutoff(
                std::slice::from_ref(&a),
                std::slice::from_ref(&a),
                ["A", "B"],
                2
            ),
            None
        );
        let ordinary = vec![a.clone(), row(["A", "B"], 2.0, 1)];
        assert_eq!(cutoff(&ordinary, &[a], ["A", "B"], 2), Some(2.0));
    }

    #[test]
    fn existing_ordered_key_and_full_distinct_topk_use_the_larger_threshold() {
        let ordinary = vec![row(["A", "B"], 1.0, 0), row(["A", "B"], 2.0, 1)];
        let one_pair = vec![row(["A", "B"], 1.0, 0)];
        assert_eq!(cutoff(&ordinary, &one_pair, ["A", "B"], 2), Some(2.0));
        assert_eq!(cutoff(&ordinary, &one_pair, ["B", "A"], 2), None);
        assert_eq!(cutoff(&ordinary, &one_pair, ["A", "C"], 2), None);

        let pairs = vec![row(["A", "B"], 1.0, 0), row(["B", "A"], 7.0, 2)];
        assert_eq!(cutoff(&ordinary, &pairs, ["A", "B"], 2), Some(2.0));
        assert_eq!(cutoff(&ordinary, &pairs, ["B", "A"], 2), Some(7.0));
        assert_eq!(cutoff(&ordinary, &pairs, ["C", "D"], 2), Some(7.0));
    }

    #[test]
    fn selected_thresholds_refuse_nonfinite_or_negative_scores() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.5] {
            let invalid_row = row(["A", "B"], invalid, 0);
            let valid_row = row(["A", "B"], 1.0, 1);
            assert_eq!(
                cutoff(
                    std::slice::from_ref(&invalid_row),
                    std::slice::from_ref(&valid_row),
                    ["A", "B"],
                    1
                ),
                None
            );
            assert_eq!(
                cutoff(
                    std::slice::from_ref(&valid_row),
                    std::slice::from_ref(&invalid_row),
                    ["A", "B"],
                    1
                ),
                None
            );
            assert_eq!(cutoff(&[valid_row], &[invalid_row], ["C", "D"], 1), None);
        }
    }

    #[test]
    fn strict_threshold_preserves_equal_scores_signed_zero_and_adjacent_floats() {
        let a = row(["A", "B"], 2.0, 4);
        let limit = cutoff(
            std::slice::from_ref(&a),
            std::slice::from_ref(&a),
            ["A", "B"],
            1,
        )
        .unwrap();
        let would_prune = |lower_bound: f64| lower_bound > limit;
        assert!(!would_prune(limit));
        assert!(!would_prune(f64::from_bits(limit.to_bits() - 1)));
        assert!(would_prune(f64::from_bits(limit.to_bits() + 1)));
        let earlier_tie = row(["A", "B"], 2.0, 3);
        assert!(order(&earlier_tie, &a).is_lt());
        assert_eq!(
            ranked(&[a.clone(), earlier_tie], 1, true)[0].encounter_index,
            3
        );
        assert_eq!(
            ranked(&[a, row(["A", "B"], 2.0, 5)], 1, true)[0].encounter_index,
            4
        );

        for zero in [-0.0, 0.0] {
            let z = row(["A", "B"], zero, 0);
            let bound = cutoff(
                std::slice::from_ref(&z),
                std::slice::from_ref(&z),
                ["A", "B"],
                1,
            )
            .unwrap();
            assert_eq!(bound, 0.0);
            assert!(zero <= bound);
        }
    }

    #[test]
    fn actual_retainer_keeps_sorted_lists_and_unique_ordered_pair_keys() {
        let sequence = [
            row(["A", "B"], 4.0, 0),
            row(["A", "B"], 2.0, 1),
            row(["B", "A"], 2.0, 2),
            row(["A", "B"], 2.0, 3),
            row(["C", "D"], 6.0, 4),
            row(["C", "D"], 1.0, 5),
            row(["E", "F"], 0.5, 6),
        ];
        for retain in 1..=4 {
            let mut ordinary = Vec::new();
            let mut pairs = Vec::new();
            for candidate in &sequence {
                retain_candidate(&mut ordinary, candidate.clone(), retain, false);
                retain_candidate(&mut pairs, candidate.clone(), retain, true);
                assert!(ordinary.len() <= retain && pairs.len() <= retain);
                assert!(ordinary.windows(2).all(|w| !order(&w[0], &w[1]).is_gt()));
                assert!(pairs.windows(2).all(|w| !order(&w[0], &w[1]).is_gt()));
                for (i, pair) in pairs.iter().enumerate() {
                    assert!(
                        pairs[..i]
                            .iter()
                            .all(|other| other.score.flight_ids != pair.score.flight_ids)
                    );
                }
            }
        }
        let ordinary = ranked(&sequence[..4], 4, false);
        assert_eq!(
            ordinary
                .iter()
                .filter(|r| r.score.flight_ids == ["A", "B"])
                .count(),
            3
        );
        let pairs = ranked(&sequence[..4], 4, true);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].encounter_index, 1);
    }

    #[test]
    fn certified_discard_preserves_receiver_best_and_both_global_merges() {
        let keys = [["A", "B"], ["B", "A"], ["A", "C"], ["D", "E"], ["F", "G"]];
        let mut certificates = 0;
        for retain in 1..=3 {
            for seed in 0..8 {
                let local: Vec<_> = (0..10)
                    .map(|i| {
                        row(
                            keys[(i + seed) % 4],
                            ((i * 5 + seed * 3) % 11) as f64 / 2.0,
                            20 + i as u64,
                        )
                    })
                    .collect();
                let ordinary = ranked(&local, retain, false);
                let pairs = ranked(&local, retain, true);
                let prior: Vec<_> = (0..6)
                    .map(|i| {
                        row(
                            keys[(i + seed) % keys.len()],
                            ((i + seed) % 7) as f64 / 2.0,
                            i as u64,
                        )
                    })
                    .collect();
                let future: Vec<_> = (0..6)
                    .map(|i| {
                        row(
                            keys[(2 * i + seed) % keys.len()],
                            ((i + seed) % 5) as f64 / 2.0,
                            200 + i as u64,
                        )
                    })
                    .collect();
                for key in keys {
                    let Some(threshold) = cutoff(&ordinary, &pairs, key, retain) else {
                        continue;
                    };
                    for score in 0..=12 {
                        let candidate = row(key, score as f64 / 2.0, 100);
                        for fraction in [0.5, 1.0] {
                            // A deliberately weaker bound is still valid for this score.
                            let lower_bound = candidate.score.combined_rmse_hz * fraction;
                            if lower_bound <= threshold {
                                continue;
                            }
                            certificates += 1;
                            let mut with_candidate = local.clone();
                            with_candidate.push(candidate.clone());
                            assert_eq!(
                                exact(&ranked(&with_candidate, 1, false)),
                                exact(&ranked(&local, 1, false))
                            );
                            for distinct in [false, true] {
                                let omitted = ranked(&local, retain, distinct);
                                let included = ranked(&with_candidate, retain, distinct);
                                assert_eq!(exact(&omitted), exact(&included));
                                let merge = |middle: &[WideCandidate]| {
                                    let mut sequence = prior.clone();
                                    sequence.extend_from_slice(middle);
                                    sequence.extend_from_slice(&future);
                                    ranked(&sequence, retain, distinct)
                                };
                                assert_eq!(exact(&merge(&omitted)), exact(&merge(&included)));
                                let mut exhaustive = prior.clone();
                                exhaustive.extend_from_slice(&with_candidate);
                                exhaustive.extend_from_slice(&future);
                                assert_eq!(
                                    exact(&merge(&omitted)),
                                    exact(&ranked(&exhaustive, retain, distinct))
                                );
                            }
                        }
                    }
                }
            }
        }
        assert!(
            certificates > 100,
            "The proof fixture must exercise actual discards"
        );
    }
}
