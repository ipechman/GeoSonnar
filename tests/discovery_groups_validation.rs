//! Independent acoustic-ridge controls. These prove algorithm contracts, not
//! aircraft identity, real event completeness, or geographic accuracy.
use geosonnar::discovery::{DiscoverySettings, Ridge, RidgePoint};
use geosonnar::discovery_groups::{
    harmonic_groups, merge_duplicates, quality, stitch_fragments, usable,
};

fn sampled(start: f64, end: f64, frequency: impl Fn(f64) -> f64) -> Ridge {
    (0..=((end - start) * 2.0).round() as usize)
        .map(|i| {
            let time_s = start + i as f64 * 0.5;
            RidgePoint {
                time_s,
                frequency_hz: frequency(time_s),
                weight: 5.0,
            }
        })
        .collect()
}

fn line(start: f64, end: f64, offset: f64) -> Ridge {
    sampled(start, end, |t| 180.0 - 0.7 * t + offset)
}

// Closed-form constant-velocity retarded-time model is an independent positive
// shape oracle. No production physics or discovery helper generates its samples.
fn doppler(center: f64, f0: f64) -> Ridge {
    sampled(center - 40.0, center + 40.0, |t| {
        let c = 343.0_f64;
        let v = 80.0_f64;
        let distance = 1000.0_f64;
        let arrival = t - center + distance / c;
        let emission = (c * c * arrival
            - (c * c * v * v * arrival * arrival + (c * c - v * v) * distance * distance).sqrt())
            / (c * c - v * v);
        let radial = v * v * emission / distance.hypot(v * emission);
        f0 * c / (c + radial)
    })
}

fn scaled(path: &[RidgePoint], multiplier: f64) -> Ridge {
    path.iter()
        .map(|p| RidgePoint {
            frequency_hz: p.frequency_hz * multiplier,
            ..*p
        })
        .collect()
}

#[test]
fn independently_generated_doppler_shape_passes_with_reported_resolution() {
    let settings = DiscoverySettings::default();
    let ridge = doppler(60.0, 120.0);
    assert!(usable(&ridge, &settings));
    let q = quality(&ridge, &settings).unwrap();
    assert_eq!(q.support_fraction, 1.0);
    assert_eq!(q.fragment_count, 1);
    assert_eq!(q.max_observed_gap_s, 0.5);
    assert_eq!(q.median_contrast, 5.0);
    assert!(q.frequency_drop_hz > 20.0);
    assert_eq!(q.frequency_sigma_hz, q.frequency_uncertainty_hz);
    assert!((q.frequency_sigma_hz - 1.0 / settings.window_s).abs() < 1e-12);
    assert_eq!(q.time_uncertainty_s, settings.window_s / 2.0);
}

#[test]
fn stationary_upward_switching_and_random_wandering_ridges_fail() {
    let settings = DiscoverySettings::default();
    assert!(!usable(&sampled(0.0, 80.0, |_| 180.0), &settings));
    assert!(!usable(&sampled(0.0, 80.0, |t| 100.0 + t), &settings));
    assert!(!usable(
        &sampled(0.0, 80.0, |t| if t < 40.0 { 180.0 } else { 150.0 }),
        &settings
    ));
    for seed in 1..=8_u64 {
        let mut state = seed;
        let wandering: Ridge = (0..161)
            .map(|i| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                RidgePoint {
                    time_s: i as f64 * 0.5,
                    frequency_hz: 120.0 + (state >> 32) as f64 / u32::MAX as f64 * 60.0,
                    weight: 6.0,
                }
            })
            .collect();
        assert!(!usable(&wandering, &settings), "noise seed {seed}");
    }
}

#[test]
fn insufficient_duration_drop_or_observation_support_does_not_force_a_proposal() {
    let settings = DiscoverySettings::default();
    assert!(!usable(&line(0.0, 5.0, 0.0), &settings));
    assert!(!usable(
        &sampled(0.0, 80.0, |t| 180.0 - 0.02 * t),
        &settings
    ));
    let dense = doppler(60.0, 120.0);
    let sparse: Ridge = dense.into_iter().step_by(4).collect();
    assert!(quality(&sparse, &settings).unwrap().support_fraction < 0.3);
    assert!(!usable(&sparse, &settings));
}

#[test]
fn corrupt_or_unordered_measurements_are_not_sorted_into_apparent_evidence() {
    let settings = DiscoverySettings::default();
    let original = doppler(60.0, 120.0);
    let mut bad = original.clone();
    bad[1].time_s = bad[0].time_s;
    assert!(quality(&bad, &settings).is_none());
    let mut bad = original.clone();
    bad.swap(0, 1);
    assert!(!usable(&bad, &settings));
    let mut bad = original.clone();
    bad[0].frequency_hz = f64::NAN;
    assert!(!usable(&bad, &settings));
    let mut bad = original.clone();
    bad[0].frequency_hz = 0.0;
    assert!(!usable(&bad, &settings));
    let mut bad = original;
    bad[0].weight = -1.0;
    assert!(!usable(&bad, &settings));
    assert!(!usable(&[], &settings));
    assert!(merge_duplicates(vec![bad, Vec::new()]).is_empty());
}

#[test]
fn overlapping_chunks_keep_one_original_strongest_measurement_at_each_time() {
    let base = doppler(40.0, 120.0);
    let left: Ridge = base.iter().copied().filter(|p| p.time_s <= 60.0).collect();
    let right: Ridge = base
        .iter()
        .copied()
        .filter(|p| p.time_s >= 20.0)
        .map(|p| RidgePoint {
            time_s: p.time_s + 1e-12,
            frequency_hz: p.frequency_hz + 0.1,
            weight: 8.0,
        })
        .collect();
    let expected = right
        .iter()
        .find(|p| (p.time_s - 30.0).abs() < 1e-8)
        .copied()
        .unwrap();
    let merged = merge_duplicates(vec![right, left]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].len(), base.len());
    assert!(
        merged[0]
            .windows(2)
            .all(|pair| pair[0].time_s < pair[1].time_s)
    );
    let actual = merged[0]
        .iter()
        .find(|p| (p.time_s - 30.0).abs() < 1e-8)
        .unwrap();
    assert_eq!(*actual, expected);
}

#[test]
fn independent_crossing_downward_paths_are_neither_duplicates_nor_harmonics() {
    let first = sampled(0.0, 100.0, |t| 230.0 - t);
    let second = sampled(0.0, 100.0, |t| 170.0 - 0.2 * t);
    let merged = merge_duplicates(vec![first, second]);
    assert_eq!(merged.len(), 2);
    assert_eq!(harmonic_groups(merged).len(), 2);
}

#[test]
fn three_simultaneous_harmonics_share_one_group_and_no_fundamental_is_invented() {
    let base = doppler(60.0, 120.0);
    let groups = harmonic_groups(vec![scaled(&base, 3.0), base.clone(), scaled(&base, 2.0)]);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].len(), 3);
    assert_eq!(groups[0][0], base);
    let groups = harmonic_groups(vec![scaled(&base, 3.0), scaled(&base, 2.0)]);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].len(), 2);
    assert_eq!(groups[0][0], scaled(&base, 2.0));
    assert_eq!(
        groups[0].iter().map(Vec::len).sum::<usize>(),
        base.len() * 2
    );
}

#[test]
fn overlapping_but_differently_timed_flyovers_remain_independent() {
    let first = doppler(60.0, 120.0);
    let second = doppler(76.0, 210.0);
    let groups = harmonic_groups(vec![scaled(&first, 2.0), first, second]);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), 3);
    assert_eq!(groups.iter().filter(|g| g.len() == 2).count(), 1);
}

#[test]
fn a_harmonic_compatible_with_two_existing_groups_is_not_assigned_arbitrarily() {
    let first = doppler(60.0, 120.0);
    // These nearby bases do not meet the declared >=1.2 harmonic ratio. The
    // upper ridge fits both (7/3 of the first, 2 of the second), so its group
    // cannot be selected from acoustic proportionality alone.
    let second = scaled(&first, 7.0 / 6.0);
    let ambiguous = scaled(&first, 7.0 / 3.0);
    let groups = harmonic_groups(vec![ambiguous, second, first]);
    assert_eq!(groups.len(), 3);
    assert!(groups.iter().all(|group| group.len() == 1));
}

#[test]
fn brief_ratio_coincidence_or_insufficient_overlap_cannot_merge_events() {
    let first = line(0.0, 80.0, 0.0);
    let second = scaled(&line(70.0, 150.0, 0.0), 2.0);
    assert_eq!(harmonic_groups(vec![first, second]).len(), 2);
    let first = sampled(0.0, 80.0, |t| 180.0 - 0.7 * t);
    let second = sampled(0.0, 80.0, |t| 360.0 - 0.3 * t);
    assert_eq!(harmonic_groups(vec![first, second]).len(), 2);
}

#[test]
fn missing_support_is_not_interpolated_to_create_duplicate_or_harmonic_evidence() {
    let interrupted: Ridge = line(0.0, 10.0, 0.0)
        .into_iter()
        .chain(line(70.0, 80.0, 0.0))
        .collect();
    let missing_middle = line(20.0, 60.0, 0.0);
    assert_eq!(
        merge_duplicates(vec![interrupted.clone(), missing_middle]).len(),
        2
    );
    let continuous_harmonic = scaled(&line(0.0, 80.0, 0.0), 2.0);
    assert_eq!(
        harmonic_groups(vec![interrupted, continuous_harmonic]).len(),
        2
    );
}

#[test]
fn reciprocal_unique_fragments_join_without_generating_a_single_gap_sample() {
    let settings = DiscoverySettings::default();
    let first = line(0.0, 23.5, 0.0);
    let second = line(30.0, 54.5, 0.0);
    let expected: Ridge = first.iter().chain(&second).copied().collect();
    let (stitched, links) = stitch_fragments(vec![second, first], &settings);
    assert_eq!(stitched, vec![expected]);
    assert_eq!(links.len(), 1);
    assert!(links[0].accepted);
    assert_eq!(links[0].reason, "reciprocal_unique_continuation");
    assert_eq!(links[0].gap_s, 6.5);
    assert!(
        !stitched[0]
            .iter()
            .any(|p| p.time_s > 23.5 && p.time_s < 30.0)
    );
    let q = quality(&stitched[0], &settings).unwrap();
    assert_eq!(q.fragment_count, 2);
    assert_eq!(q.max_observed_gap_s, 6.5);
    assert!(q.support_fraction < 1.0);
}

#[test]
fn chains_keep_all_original_measurements_and_report_every_join() {
    let settings = DiscoverySettings::default();
    let paths = vec![
        line(0.0, 20.0, 0.0),
        line(25.0, 45.0, 0.0),
        line(50.0, 70.0, 0.0),
    ];
    let expected: Ridge = paths.iter().flatten().copied().collect();
    let (stitched, links) = stitch_fragments(paths, &settings);
    assert_eq!(stitched, vec![expected]);
    assert_eq!(links.iter().filter(|link| link.accepted).count(), 2);
    assert_eq!(quality(&stitched[0], &settings).unwrap().fragment_count, 3);
}

#[test]
fn competing_outgoing_or_incoming_continuations_remain_separate() {
    let settings = DiscoverySettings::default();
    for paths in [
        vec![
            line(0.0, 23.5, 0.0),
            line(30.0, 54.5, 0.0),
            line(31.0, 55.5, 0.5),
        ],
        vec![
            line(0.0, 23.5, 0.0),
            line(1.0, 24.5, 0.5),
            line(30.0, 55.0, 0.0),
        ],
    ] {
        let (stitched, links) = stitch_fragments(paths, &settings);
        assert_eq!(stitched.len(), 3);
        assert_eq!(links.len(), 2);
        assert!(
            links
                .iter()
                .all(|link| !link.accepted && link.reason == "ambiguous_continuation")
        );
    }
}

#[test]
fn long_gaps_large_missing_fraction_and_bad_endpoint_motion_do_not_stitch() {
    let settings = DiscoverySettings::default();
    for (first, second) in [
        (line(0.0, 23.5, 0.0), line(50.0, 80.0, 0.0)),
        (line(0.0, 12.0, 0.0), line(30.0, 42.0, 0.0)),
        (
            line(0.0, 23.5, 0.0),
            sampled(30.0, 55.0, |t| 150.0 + (t - 30.0)),
        ),
        (line(0.0, 23.5, 0.0), sampled(30.0, 55.0, |_| 155.0)),
        (line(0.0, 23.5, 0.0), line(30.0, 55.0, -70.0)),
    ] {
        let (stitched, links) = stitch_fragments(vec![first, second], &settings);
        assert_eq!(stitched.len(), 2);
        assert!(links.is_empty());
    }
}

#[test]
fn all_four_and_six_independent_passes_survive_without_a_global_event_limit() {
    for count in [4, 6] {
        let mut ridges = Vec::new();
        for i in 0..count {
            let base = doppler(
                60.0 + i as f64 * 100.0,
                [120.0, 170.0, 230.0, 140.0, 260.0, 190.0][i],
            );
            assert!(usable(&base, &DiscoverySettings::default()));
            ridges.push(scaled(&base, 2.0));
            ridges.push(base);
        }
        let groups = harmonic_groups(ridges);
        assert_eq!(groups.len(), count);
        assert!(groups.iter().all(|group| group.len() == 2));
    }
}
