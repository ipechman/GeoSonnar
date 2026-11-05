//! Exact synthetic software controls for the bounded cache, not field accuracy.
use super::{FirstEventCache, PairOutcome};
use crate::physics::{
    self, EventTrace, ModelConfig, ModelResult, PairConfig, PairScore, PreparedTrack, TracePoint,
    Track, TrackPoint,
};

#[derive(Clone)]
struct Case {
    tracks: [Track; 2],
    events: [EventTrace; 2],
    config: PairConfig,
    observer: [f64; 3],
}

fn straight(id: &str, center: f64, direction: usize, lateral: f64, height: f64) -> Track {
    Track {
        flight_id: id.into(),
        points: (-120..=120)
            .step_by(5)
            .map(|t| {
                let mut xyz_m = [lateral, lateral, height];
                xyz_m[direction] = t as f64 * 100.0;
                TrackPoint {
                    t_s: center + t as f64,
                    xyz_m,
                }
            })
            .collect(),
    }
}

fn fixture() -> Case {
    let observer = [200., -150., 20.];
    let tracks = [
        straight("first", 0., 0, 0., 3000.),
        straight("second", 920., 1, 2500., 2000.),
    ];
    let config = PairConfig {
        event1_cpa_recording_s: 100.,
        shared_shifts_s: vec![-2., -1., 0., 1.25, 2.],
        clock_offset_bounds_s: None,
        model: ModelConfig::default(),
    };
    let first = PreparedTrack::new(&tracks[0], observer, config.model).unwrap();
    let clock = first.closest_approach().arrival_time_s - 101.25;
    let events = std::array::from_fn(|index| {
        let model = PreparedTrack::new(&tracks[index], observer, config.model).unwrap();
        let f0 = [130., 145.][index];
        let center = model.closest_approach().arrival_time_s - clock;
        EventTrace {
            points: (-30..=30)
                .step_by(3)
                .map(|offset| {
                    let t_rec_s = center + offset as f64;
                    TracePoint {
                        t_rec_s,
                        frequency_hz: model.factor_at_arrival(t_rec_s + clock).unwrap() * f0,
                    }
                })
                .collect(),
            f0_hz: vec![f0 - 1., f0, f0 + 1.],
        }
    });
    Case {
        tracks,
        events,
        config,
        observer,
    }
}

fn reference(case: &Case, second: &Track, indices: [usize; 2]) -> ModelResult<PairScore> {
    physics::score_pair_at_cpa_with_checkpoint(
        [&case.tracks[0], second],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        indices,
        || Ok(()),
    )
}

fn float_bits(score: &PairScore) -> Vec<u64> {
    let mut numbers = Vec::new();
    numbers.extend(score.observer_xyz_m);
    numbers.extend(score.rmse_hz);
    numbers.push(score.combined_rmse_hz);
    numbers.extend(score.fitted_f0_hz);
    numbers.extend([
        score.clock_offset_s,
        score.shared_shift_s,
        score.predicted_event2_cpa_recording_s,
    ]);
    for cpa in score.cpa {
        numbers.extend([
            cpa.emission_time_s,
            cpa.arrival_time_s,
            cpa.slant_distance_m,
            cpa.speed_mps,
        ]);
    }
    numbers.into_iter().map(f64::to_bits).collect()
}

fn exact(actual: ModelResult<PairScore>, expected: ModelResult<PairScore>) {
    match (actual, expected) {
        (Ok(actual), Ok(expected)) => {
            assert_eq!(
                serde_json::to_vec(&actual).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
            assert_eq!(float_bits(&actual), float_bits(&expected));
        }
        (Err(actual), Err(expected)) => assert_eq!(actual, expected),
        (actual, expected) => panic!("different outcome: actual={actual:?}, expected={expected:?}"),
    }
}

#[test]
fn repeated_and_distinct_partners_preserve_every_score_bit_and_reuse_only_first_fits() {
    let case = fixture();
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    assert_eq!(cache.stats().first_model_preparations, 0);
    assert_eq!(cache.stats().first_factor_calls, 0);
    assert_eq!(cache.stats().completed_first_fits, 0);
    exact(
        cache.score(&case.tracks[1], 0, || Ok(())),
        reference(&case, &case.tracks[1], [0, 0]),
    );
    let first = cache.stats();
    assert_eq!(first.first_model_preparations, 1);
    assert_eq!(
        first.completed_first_fits,
        case.config.shared_shifts_s.len() as u64
    );
    assert_eq!(first.first_fit_hits, 0);
    let mut alternative = case.tracks[1].clone();
    alternative.flight_id = "different-partner".into();
    for point in &mut alternative.points {
        point.xyz_m[0] += 17.;
    }
    for partner in [&case.tracks[1], &alternative, &case.tracks[1]] {
        exact(
            cache.score(partner, 0, || Ok(())),
            reference(&case, partner, [0, 0]),
        );
    }
    let final_stats = cache.stats();
    assert_eq!(final_stats.first_model_preparations, 1);
    assert_eq!(final_stats.completed_first_fits, first.completed_first_fits);
    assert_eq!(final_stats.first_factor_calls, first.first_factor_calls);
    assert_eq!(final_stats.first_fit_hits, 3 * first.completed_first_fits);
    assert!(final_stats.second_factor_calls > first.second_factor_calls);
}

#[test]
fn first_ties_preserve_unsorted_tones_duplicate_shifts_and_signed_zero() {
    // The two interior stationary plateaus yield factors exactly 1, producing
    // a real distinct-tone tie, rather than just repeating the same candidate.
    let plateau = |id: &str, center: f64| Track {
        flight_id: id.into(),
        points: [
            (-30., 2000.),
            (-20., 1000.),
            (20., 1000.),
            (30., 1000.),
            (40., 2000.),
        ]
        .into_iter()
        .map(|(t, x)| TrackPoint {
            t_s: center + t,
            xyz_m: [x, 0., 1000.],
        })
        .collect(),
    };
    let tracks = [
        plateau("plateau-first", 0.),
        plateau("plateau-second", 200.),
    ];
    let observer = [0.; 3];
    let mut config = PairConfig {
        event1_cpa_recording_s: 100.,
        shared_shifts_s: vec![0., -0., 0.],
        clock_offset_bounds_s: None,
        model: ModelConfig {
            max_gap_s: 50.,
            ..Default::default()
        },
    };
    let events = [102., 302.].map(|first| EventTrace {
        points: (0..3)
            .map(|i| TracePoint {
                t_rec_s: first + 2. * i as f64,
                frequency_hz: 2.,
            })
            .collect(),
        f0_hz: vec![3., 1., 3.],
    });
    for signed_first in [0.0_f64, -0.0_f64] {
        config.shared_shifts_s[0] = signed_first;
        let case = Case {
            tracks: tracks.clone(),
            events: events.clone(),
            config: config.clone(),
            observer,
        };
        let expected = reference(&case, &case.tracks[1], [0, 0]).unwrap();
        assert_eq!(expected.fitted_f0_hz, [3., 3.]);
        assert_eq!(expected.shared_shift_s.to_bits(), signed_first.to_bits());
        let mut cache = FirstEventCache::new(
            &case.tracks[0],
            observer,
            [&case.events[0], &case.events[1]],
            &case.config,
            0,
        );
        for _ in 0..2 {
            exact(
                cache.score(&case.tracks[1], 0, || Ok(())),
                Ok(expected.clone()),
            );
        }
        assert_eq!(cache.stats().completed_first_fits, 3);
        assert_eq!(cache.stats().first_fit_hits, 3);
    }
}

#[test]
fn same_continuous_aircraft_and_distinct_local_cpa_indices_match() {
    let mut case = fixture();
    let mut combined = case.tracks[0].clone();
    combined.flight_id = "one-continuous-aircraft".into();
    let a = combined.points.last().unwrap().clone();
    let b = case.tracks[1].points[0].clone();
    let steps = ((b.t_s - a.t_s) / 20.).ceil() as usize;
    for i in 1..steps {
        let fraction = i as f64 / steps as f64;
        combined.points.push(TrackPoint {
            t_s: a.t_s + (b.t_s - a.t_s) * fraction,
            xyz_m: std::array::from_fn(|axis| {
                a.xyz_m[axis] + (b.xyz_m[axis] - a.xyz_m[axis]) * fraction
            }),
        });
    }
    combined.points.extend(case.tracks[1].points.clone());
    case.tracks = [combined.clone(), combined];
    let model = PreparedTrack::new(&case.tracks[0], case.observer, case.config.model).unwrap();
    let cpas = model.local_closest_approaches();
    let first = cpas
        .iter()
        .position(|c| c.emission_time_s < 10. && !c.at_track_boundary)
        .unwrap();
    let second = cpas
        .iter()
        .position(|c| c.emission_time_s > 900. && !c.at_track_boundary)
        .unwrap();
    assert_ne!(first, second);
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        first,
    );
    assert!(reference(&case, &case.tracks[1], [first, second]).is_ok());
    for index in [second, first, cpas.len(), second] {
        exact(
            cache.score(&case.tracks[1], index, || Ok(())),
            reference(&case, &case.tracks[1], [first, index]),
        );
    }
}

#[test]
fn validation_and_model_error_precedence_matches_without_cached_failures() {
    for variant in 0..15 {
        let mut case = fixture();
        let mut indices = [0, 0];
        match variant {
            0 => case.events[0].points.truncate(2),
            1 => case.events[1].points[0].frequency_hz = f64::NAN,
            2 => case.events[0].points[1].t_rec_s = case.events[0].points[0].t_rec_s,
            3 => case.events[0].f0_hz.clear(),
            4 => case.config.shared_shifts_s.clear(),
            5 => case.config.shared_shifts_s[0] = f64::INFINITY,
            6 => case.events[1] = case.events[0].clone(),
            7 => case.config.clock_offset_bounds_s = Some([2., 1.]),
            8 => case.config.model.sound_speed_mps = -1.,
            9 => case.tracks[0].points[1].t_s = case.tracks[0].points[0].t_s,
            10 => case.tracks[1].points.truncate(1),
            11 => indices[0] = 999,
            12 => indices[1] = 999,
            13 => {
                case.config.event1_cpa_recording_s = -f64::MAX;
                case.config.shared_shifts_s = vec![-f64::MAX];
            }
            14 => case.config.clock_offset_bounds_s = Some([1e6, 1e6 + 1.]),
            _ => unreachable!(),
        }
        let expected = reference(&case, &case.tracks[1], indices);
        assert!(expected.is_err(), "variant {variant}");
        let mut cache = FirstEventCache::new(
            &case.tracks[0],
            case.observer,
            [&case.events[0], &case.events[1]],
            &case.config,
            indices[0],
        );
        for _ in 0..2 {
            exact(
                cache.score(&case.tracks[1], indices[1], || Ok(())),
                expected.clone(),
            );
        }
        assert_eq!(cache.stats().completed_first_fits, 0, "variant {variant}");
        assert_eq!(cache.stats().first_fit_hits, 0, "variant {variant}");
    }
}

#[test]
fn unsupported_second_window_precedes_first_fit_overflow_and_does_not_fill_cache() {
    let mut case = fixture();
    for point in &mut case.events[0].points {
        point.frequency_hz = f64::MAX;
    }
    let mut short = case.tracks[1].clone();
    short.points.retain(|point| (point.t_s - 920.).abs() <= 5.);
    let missing = reference(&case, &short, [0, 0]);
    assert!(
        missing
            .as_ref()
            .unwrap_err()
            .starts_with("No shared clock shift")
    );
    let overflow = reference(&case, &case.tracks[1], [0, 0]);
    assert_eq!(
        overflow.as_ref().unwrap_err(),
        "Frequency fitting overflowed; inspect input values"
    );
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    for partner in [&short, &case.tracks[1], &short, &case.tracks[1]] {
        exact(
            cache.score(partner, 0, || Ok(())),
            reference(&case, partner, [0, 0]),
        );
        assert_eq!(cache.stats().completed_first_fits, 0);
        assert_eq!(cache.stats().first_fit_hits, 0);
    }
}

#[test]
fn warm_cache_still_checks_second_support_and_second_fit_overflow() {
    let case = fixture();
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    exact(
        cache.score(&case.tracks[1], 0, || Ok(())),
        reference(&case, &case.tracks[1], [0, 0]),
    );
    let before = cache.stats();
    let mut short = case.tracks[1].clone();
    short.points.retain(|point| (point.t_s - 920.).abs() <= 5.);
    exact(
        cache.score(&short, 0, || Ok(())),
        reference(&case, &short, [0, 0]),
    );
    assert_eq!(cache.stats().first_fit_hits, before.first_fit_hits);
    assert_eq!(
        cache.stats().completed_first_fits,
        before.completed_first_fits
    );
    exact(
        cache.score(&case.tracks[1], 0, || Ok(())),
        reference(&case, &case.tracks[1], [0, 0]),
    );

    let mut overflow_case = fixture();
    for point in &mut overflow_case.events[1].points {
        point.frequency_hz = f64::MAX;
    }
    let mut cache = FirstEventCache::new(
        &overflow_case.tracks[0],
        overflow_case.observer,
        [&overflow_case.events[0], &overflow_case.events[1]],
        &overflow_case.config,
        0,
    );
    for _ in 0..2 {
        let expected = reference(&overflow_case, &overflow_case.tracks[1], [0, 0]);
        assert_eq!(
            expected.as_ref().unwrap_err(),
            "Frequency fitting overflowed; inspect input values"
        );
        exact(
            cache.score(&overflow_case.tracks[1], 0, || Ok(())),
            expected,
        );
    }
    // The completed first fit is a valid reusable value; the second error is not.
    assert_eq!(cache.stats().completed_first_fits, 1);
    assert_eq!(cache.stats().first_fit_hits, 1);
}

#[test]
fn every_cold_and_warm_checkpoint_stop_propagates_and_retry_is_exact() {
    let mut case = fixture();
    case.config.shared_shifts_s = vec![1.25];
    // Cross two64-point boundaries in both event factor vectors.
    let clock = PreparedTrack::new(&case.tracks[0], case.observer, case.config.model)
        .unwrap()
        .closest_approach()
        .arrival_time_s
        - 101.25;
    for index in 0..2 {
        let model =
            PreparedTrack::new(&case.tracks[index], case.observer, case.config.model).unwrap();
        let start = case.events[index].points[0].t_rec_s;
        let end = case.events[index].points.last().unwrap().t_rec_s;
        let f0 = case.events[index].f0_hz[1];
        case.events[index].points = (0..129)
            .map(|i| {
                let t_rec_s = start + (end - start) * i as f64 / 128.;
                TracePoint {
                    t_rec_s,
                    frequency_hz: model.factor_at_arrival(t_rec_s + clock).unwrap() * f0,
                }
            })
            .collect();
    }
    let expected = reference(&case, &case.tracks[1], [0, 0]).unwrap();
    for warm in [false, true] {
        let make = || {
            FirstEventCache::new(
                &case.tracks[0],
                case.observer,
                [&case.events[0], &case.events[1]],
                &case.config,
                0,
            )
        };
        let mut counter = make();
        if warm {
            counter.score(&case.tracks[1], 0, || Ok(())).unwrap();
        }
        let mut total = 0;
        counter
            .score(&case.tracks[1], 0, || {
                total += 1;
                Ok(())
            })
            .unwrap();
        assert!(total > 15);
        for stop_at in 1..=total {
            let mut cache = make();
            if warm {
                cache.score(&case.tracks[1], 0, || Ok(())).unwrap();
            }
            let mut calls = 0;
            let error = cache
                .score(&case.tracks[1], 0, || {
                    calls += 1;
                    if calls == stop_at {
                        Err("exact requested cache stop".into())
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();
            assert_eq!(error, "exact requested cache stop");
            assert_eq!(calls, stop_at);
            exact(
                cache.score(&case.tracks[1], 0, || Ok(())),
                Ok(expected.clone()),
            );
            assert_eq!(cache.stats().completed_first_fits, 1);
            assert_eq!(cache.stats().first_model_preparations, 1);
        }
    }
}

#[test]
fn immutable_contexts_do_not_cross_receiver_configuration_or_tone_boundaries() {
    let original = fixture();
    let mut cases = vec![original.clone(); 6];
    cases[1].observer[0] += 55.;
    cases[2].config.model.sound_speed_mps = 340.;
    cases[3].config.shared_shifts_s = vec![2., 0., -1.];
    cases[4].events[0].f0_hz = vec![129., 128.];
    cases[5].config.event1_cpa_recording_s += 0.2;
    let mut scores = Vec::new();
    for case in &cases {
        let expected = reference(case, &case.tracks[1], [0, 0]).unwrap();
        let mut cache = FirstEventCache::new(
            &case.tracks[0],
            case.observer,
            [&case.events[0], &case.events[1]],
            &case.config,
            0,
        );
        assert_eq!(cache.stats().completed_first_fits, 0);
        for _ in 0..2 {
            exact(
                cache.score(&case.tracks[1], 0, || Ok(())),
                Ok(expected.clone()),
            );
        }
        scores.push(serde_json::to_vec(&expected).unwrap());
    }
    for score in &scores[1..] {
        assert_ne!(score, &scores[0]);
    }
}

#[test]
fn shift_storage_bound_is_explicit_and_does_not_grow_with_partner_count() {
    let mut case = fixture();
    case.events.iter_mut().for_each(|event| {
        event.points.truncate(3);
        event.f0_hz.truncate(1);
    });
    case.config.shared_shifts_s = vec![1.25; 1001];
    let expected = reference(&case, &case.tracks[1], [0, 0]).unwrap();
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    for _ in 0..3 {
        exact(
            cache.score(&case.tracks[1], 0, || Ok(())),
            Ok(expected.clone()),
        );
    }
    assert_eq!(cache.stats().completed_first_fits, 1001);
    assert_eq!(cache.stats().first_fit_hits, 2002);
    assert_eq!(cache.stats().first_factor_calls, 3003);
    assert_eq!(cache.stats().first_model_preparations, 1);
    drop(cache);
    case.config.shared_shifts_s.push(1.25);
    let mut refused = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    assert_eq!(
        refused.score(&case.tracks[1], 0, || Ok(())).unwrap_err(),
        "First-event cache exceeds the explicit1001-shift memory bound"
    );
    assert_eq!(refused.stats().completed_first_fits, 0);
    assert_eq!(refused.stats().first_factor_calls, 0);
    // This verifies logical storage bounds, not an allocator/RSS measurement.
}

fn scored(value: ModelResult<PairOutcome>) -> ModelResult<PairScore> {
    value.map(|value| match value {
        PairOutcome::Scored(score) => score,
        PairOutcome::Pruned {
            lower_bound_rmse_hz,
        } => {
            panic!("Expected an exact score, got bound {lower_bound_rmse_hz}")
        }
    })
}

fn poor_first_fit_case() -> Case {
    let mut case = fixture();
    case.events[0].f0_hz = vec![89., 90., 88.];
    case
}

#[test]
fn bounded_cold_and_disabled_calls_are_exact_scalar_scores() {
    let case = poor_first_fit_case();
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    let expected = reference(&case, &case.tracks[1], [0, 0]).unwrap();
    // Even a cutoff below every eventual score cannot prune unknown first fits.
    exact(
        scored(cache.score_bounded(&case.tracks[1], 0, Some(0.), || Ok(()))),
        Ok(expected.clone()),
    );
    let mut ordinary_calls = 0;
    exact(
        cache.score(&case.tracks[1], 0, || {
            ordinary_calls += 1;
            Ok(())
        }),
        Ok(expected.clone()),
    );
    let mut disabled_calls = 0;
    exact(
        scored(cache.score_bounded(&case.tracks[1], 0, None, || {
            disabled_calls += 1;
            Ok(())
        })),
        Ok(expected),
    );
    assert_eq!(ordinary_calls, disabled_calls);
}

#[test]
fn completed_cache_bound_excludes_real_scalar_scores_for_distinct_partners() {
    let case = poor_first_fit_case();
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    cache.score(&case.tracks[1], 0, || Ok(())).unwrap();
    assert_eq!(
        cache.stats().completed_first_fits,
        case.config.shared_shifts_s.len() as u64
    );
    let before = cache.stats();
    for displacement in [0., 17., -81., 260.] {
        let mut partner = case.tracks[1].clone();
        partner.flight_id = format!("oracle-partner-{displacement}");
        for point in &mut partner.points {
            point.xyz_m[0] += displacement;
        }
        let oracle = reference(&case, &partner, [0, 0]).unwrap();
        let PairOutcome::Pruned {
            lower_bound_rmse_hz,
        } = cache
            .score_bounded(&partner, 0, Some(0.), || Ok(()))
            .unwrap()
        else {
            panic!("Complete poor first fits should exclude cutoff zero");
        };
        assert!(lower_bound_rmse_hz.is_finite() && lower_bound_rmse_hz > 0.);
        assert!(lower_bound_rmse_hz <= oracle.combined_rmse_hz);
        // Independently ask the public scalar scorer about every shared shift.
        for &shift in &case.config.shared_shifts_s {
            let mut one = case.clone();
            one.config.shared_shifts_s = vec![shift];
            let single = reference(&one, &partner, [0, 0]).unwrap();
            assert!(lower_bound_rmse_hz <= single.combined_rmse_hz);
        }
    }
    assert_eq!(cache.stats().first_factor_calls, before.first_factor_calls);
    assert_eq!(
        cache.stats().second_factor_calls,
        before.second_factor_calls
    );
    assert_eq!(
        cache.stats().completed_first_fits,
        before.completed_first_fits
    );
}

#[test]
fn equal_cutoff_is_scored_and_only_strictly_worse_bound_is_pruned() {
    let case = poor_first_fit_case();
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    let expected = cache.score(&case.tracks[1], 0, || Ok(())).unwrap();
    let PairOutcome::Pruned {
        lower_bound_rmse_hz: bound,
    } = cache
        .score_bounded(&case.tracks[1], 0, Some(0.), || Ok(()))
        .unwrap()
    else {
        panic!("Expected a positive completed-fit bound");
    };
    exact(
        scored(cache.score_bounded(&case.tracks[1], 0, Some(bound), || Ok(()))),
        Ok(expected.clone()),
    );
    exact(
        scored(cache.score_bounded(&case.tracks[1], 0, Some(f64::MAX), || Ok(()))),
        Ok(expected),
    );
    assert!(matches!(
        cache
            .score_bounded(&case.tracks[1], 0, Some(bound.next_down()), || Ok(()))
            .unwrap(),
        PairOutcome::Pruned { .. }
    ));
}

#[test]
fn unknown_later_shift_preserves_later_overflow_instead_of_returning_partial_winner() {
    let mut case = poor_first_fit_case();
    case.config.shared_shifts_s = vec![1.25, -40.];
    for point in &mut case.events[1].points {
        point.frequency_hz *= 1e158;
    }
    case.events[1].f0_hz = vec![145. * 1e158];
    let mut short = case.tracks[1].clone();
    short.points.retain(|point| point.t_s <= 955.);
    let mut first_only = case.clone();
    first_only.config.shared_shifts_s.truncate(1);
    assert!(reference(&first_only, &case.tracks[1], [0, 0]).is_ok());
    let expected = reference(&case, &case.tracks[1], [0, 0]);
    assert_eq!(
        expected.as_ref().unwrap_err(),
        "Frequency fitting overflowed; inspect input values"
    );
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    exact(
        cache.score(&short, 0, || Ok(())),
        reference(&case, &short, [0, 0]),
    );
    assert_eq!(cache.stats().completed_first_fits, 1);
    assert!(cache.fits[0].is_some() && cache.fits[1].is_none());
    // A per-shift skip could discard the later overflow and return the earlier
    // score. An unknown shift instead forces the complete original evaluation.
    exact(
        scored(cache.score_bounded(&case.tracks[1], 0, Some(0.), || Ok(()))),
        expected,
    );
}

#[test]
fn clock_bits_must_match_for_every_admissible_cached_shift() {
    let case = poor_first_fit_case();
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    cache.score(&case.tracks[1], 0, || Ok(())).unwrap();
    cache.fits[0].as_mut().unwrap().clock_bits ^= 1;
    assert_eq!(
        cache
            .score_bounded(&case.tracks[1], 0, Some(0.), || Ok(()))
            .unwrap_err(),
        "Immutable first-event cache clock identity changed"
    );
}

#[test]
fn inclusive_clock_endpoints_and_signed_zero_keep_equal_ties() {
    let plateau = |id: &str, center: f64| Track {
        flight_id: id.into(),
        points: [
            (-30., 2000.),
            (-20., 1000.),
            (20., 1000.),
            (30., 1000.),
            (40., 2000.),
        ]
        .into_iter()
        .map(|(t, x)| TrackPoint {
            t_s: center + t,
            xyz_m: [x, 0., 1000.],
        })
        .collect(),
    };
    for signed_first in [0.0_f64, -0.0_f64] {
        let tracks = [plateau("a", 0.), plateau("b", 200.)];
        let model = ModelConfig {
            max_gap_s: 50.,
            ..Default::default()
        };
        let arrival = PreparedTrack::new(&tracks[0], [0.; 3], model)
            .unwrap()
            .closest_approach()
            .arrival_time_s;
        let config = PairConfig {
            event1_cpa_recording_s: 100.,
            shared_shifts_s: vec![signed_first, -0., 1., 50.],
            clock_offset_bounds_s: Some([arrival - 100. - 1., arrival - 100. - signed_first]),
            model,
        };
        let events = [102., 302.].map(|start| EventTrace {
            points: (0..3)
                .map(|i| TracePoint {
                    t_rec_s: start + 2. * i as f64,
                    frequency_hz: 2.,
                })
                .collect(),
            f0_hz: vec![3., 1., 3.],
        });
        let case = Case {
            tracks,
            events,
            config,
            observer: [0.; 3],
        };
        let expected = reference(&case, &case.tracks[1], [0, 0]).unwrap();
        let mut cache = FirstEventCache::new(
            &case.tracks[0],
            case.observer,
            [&case.events[0], &case.events[1]],
            &case.config,
            0,
        );
        cache.score(&case.tracks[1], 0, || Ok(())).unwrap();
        assert_eq!(cache.stats().completed_first_fits, 3);
        assert!(cache.fits[3].is_none()); // Outside clock bounds, not an unknown admissible shift.
        let PairOutcome::Pruned {
            lower_bound_rmse_hz: bound,
        } = cache
            .score_bounded(&case.tracks[1], 0, Some(-0.), || Ok(()))
            .unwrap()
        else {
            panic!("Positive bound should exclude signed zero cutoff");
        };
        assert_eq!(bound.to_bits(), (0.5_f64).sqrt().to_bits());
        let actual =
            scored(cache.score_bounded(&case.tracks[1], 0, Some(bound), || Ok(()))).unwrap();
        assert_eq!(actual.shared_shift_s.to_bits(), signed_first.to_bits());
        exact(Ok(actual), Ok(expected));
    }
}

#[test]
fn all_bound_checkpoints_propagate_cancellation_without_caching_a_stop() {
    let case = poor_first_fit_case();
    let make = || {
        FirstEventCache::new(
            &case.tracks[0],
            case.observer,
            [&case.events[0], &case.events[1]],
            &case.config,
            0,
        )
    };
    let mut counted = make();
    counted.score(&case.tracks[1], 0, || Ok(())).unwrap();
    let mut total = 0;
    assert!(matches!(
        counted
            .score_bounded(&case.tracks[1], 0, Some(0.), || {
                total += 1;
                Ok(())
            })
            .unwrap(),
        PairOutcome::Pruned { .. }
    ));
    assert!(total > case.config.shared_shifts_s.len());
    for stop in 1..=total {
        let mut cache = make();
        cache.score(&case.tracks[1], 0, || Ok(())).unwrap();
        let before = cache.stats();
        let mut calls = 0;
        let result = cache.score_bounded(&case.tracks[1], 0, Some(0.), || {
            calls += 1;
            if calls == stop {
                Err("exact bounded-pair cancellation".into())
            } else {
                Ok(())
            }
        });
        assert_eq!(result.unwrap_err(), "exact bounded-pair cancellation");
        assert_eq!(calls, stop);
        assert_eq!(
            cache.stats().completed_first_fits,
            before.completed_first_fits
        );
        assert_eq!(
            cache.stats().second_factor_calls,
            before.second_factor_calls
        );
        exact(
            scored(cache.score_bounded(&case.tracks[1], 0, None, || Ok(()))),
            reference(&case, &case.tracks[1], [0, 0]),
        );
    }
}

#[test]
fn bounded_api_rejects_invalid_cutoffs_and_preserves_model_cpa_and_no_clock_errors() {
    let case = poor_first_fit_case();
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    cache.score(&case.tracks[1], 0, || Ok(())).unwrap();
    for cutoff in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.] {
        assert_eq!(
            cache
                .score_bounded(&case.tracks[1], 0, Some(cutoff), || Ok(()))
                .unwrap_err(),
            "Pair pruning cutoff must be finite and nonnegative"
        );
    }
    for variant in 0..4 {
        let mut second = case.tracks[1].clone();
        let index = if variant == 3 { 999 } else { 0 };
        match variant {
            0 => second.points.truncate(1),
            1 => second
                .points
                .iter_mut()
                .for_each(|point| point.t_s -= 1000.),
            2 => second.points.retain(|point| point.t_s >= 960.),
            _ => (),
        }
        let expected = reference(&case, &second, [0, index]);
        assert!(expected.is_err());
        exact(
            scored(cache.score_bounded(&second, index, Some(0.), || Ok(()))),
            expected,
        );
    }
    for variant in 0..3 {
        let mut invalid = case.clone();
        match variant {
            0 => invalid.config.model.sound_speed_mps = -1.,
            1 => invalid.config.clock_offset_bounds_s = Some([1e6, 1e6 + 1.]),
            2 => invalid.events[0].points[0].frequency_hz = f64::NAN,
            _ => unreachable!(),
        }
        let expected = reference(&invalid, &invalid.tracks[1], [0, 0]);
        assert!(expected.is_err());
        let mut bounded = FirstEventCache::new(
            &invalid.tracks[0],
            invalid.observer,
            [&invalid.events[0], &invalid.events[1]],
            &invalid.config,
            0,
        );
        exact(
            scored(bounded.score_bounded(&invalid.tracks[1], 0, Some(0.), || Ok(()))),
            expected,
        );
    }
}

fn missing_first_support_case() -> Case {
    let mut case = fixture();
    case.config.shared_shifts_s = vec![1.25];
    // The first CPA remains interior; later event samples exceed this pass.
    case.tracks[0].points.retain(|point| point.t_s <= 10.);
    case
}

fn failed_sample(case: &Case, event: usize, track: &Track, shift: f64) -> usize {
    let first = PreparedTrack::new(&case.tracks[0], case.observer, case.config.model).unwrap();
    let clock = first.local_closest_approaches()[0].arrival_time_s
        - case.config.event1_cpa_recording_s
        - shift;
    let model = PreparedTrack::new(track, case.observer, case.config.model).unwrap();
    case.events[event]
        .points
        .iter()
        .position(|point| model.factor_at_arrival(point.t_rec_s + clock).is_err())
        .expect("Fixture must have an actual factor failure")
}

#[test]
fn first_factor_unavailable_is_reused_with_exact_failure_query_counts() {
    let case = missing_first_support_case();
    let failed = failed_sample(&case, 0, &case.tracks[0], 1.25);
    assert!(failed > 0);
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    let mut alternative = case.tracks[1].clone();
    alternative.flight_id = "different-valid-second".into();
    for point in &mut alternative.points {
        point.xyz_m[0] += 17.;
    }
    for partner in [&case.tracks[1], &alternative, &case.tracks[1]] {
        let expected = reference(&case, partner, [0, 0]);
        assert!(
            expected
                .as_ref()
                .unwrap_err()
                .starts_with("No shared clock shift")
        );
        exact(cache.score(partner, 0, || Ok(())), expected);
    }
    let stats = cache.stats();
    assert_eq!(stats.first_factor_calls, failed as u64 + 1);
    assert_eq!(stats.first_factor_failures, 1);
    assert_eq!(stats.first_factor_samples_before_failure, failed as u64);
    assert_eq!(stats.second_factor_calls, 0);
    assert_eq!(stats.second_factor_failures, 0);
    assert_eq!(stats.first_support_cache_hits, 2);
    assert_eq!(stats.reused_first_factor_queries, 2 * (failed as u64 + 1));
    assert_eq!(stats.completed_first_fits, 0);
    assert!(matches!(cache.fits[0].unwrap().result,
        super::FirstResult::Unavailable { failed_sample_index } if failed_sample_index == failed));
}

#[test]
fn second_factor_failures_are_counted_but_never_negative_cached() {
    let mut case = fixture();
    case.config.shared_shifts_s = vec![1.25];
    let mut short = case.tracks[1].clone();
    short.points.retain(|point| point.t_s <= 930.);
    let failed = failed_sample(&case, 1, &short, 1.25);
    assert!(failed > 0);
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    for _ in 0..2 {
        let expected = reference(&case, &short, [0, 0]);
        assert!(
            expected
                .as_ref()
                .unwrap_err()
                .starts_with("No shared clock shift")
        );
        exact(cache.score(&short, 0, || Ok(())), expected);
        assert!(cache.fits[0].is_none());
    }
    assert_eq!(
        cache.stats().first_factor_calls,
        2 * case.events[0].points.len() as u64
    );
    assert_eq!(cache.stats().second_factor_failures, 2);
    exact(
        cache.score(&case.tracks[1], 0, || Ok(())),
        reference(&case, &case.tracks[1], [0, 0]),
    );
    let after_fit = cache.stats();
    exact(
        cache.score(&short, 0, || Ok(())),
        reference(&case, &short, [0, 0]),
    );
    let stats = cache.stats();
    assert_eq!(stats.first_factor_calls, after_fit.first_factor_calls);
    assert_eq!(stats.first_fit_hits, after_fit.first_fit_hits);
    assert_eq!(stats.second_factor_failures, 3);
    assert_eq!(
        stats.second_factor_samples_before_failure,
        3 * failed as u64
    );
    assert_eq!(stats.first_factor_failures, 0);
    assert_eq!(stats.first_support_cache_hits, 0);
    assert_eq!(stats.reused_first_factor_queries, 0);
    exact(
        cache.score(&case.tracks[1], 0, || Ok(())),
        reference(&case, &case.tracks[1], [0, 0]),
    );
    assert_eq!(cache.stats().first_fit_hits, after_fit.first_fit_hits + 1);
}

#[test]
fn known_unavailable_shifts_allow_only_a_finite_complete_remaining_bound() {
    let mut case = poor_first_fit_case();
    case.config.shared_shifts_s = vec![1.25, 200., -200.];
    let expected = reference(&case, &case.tracks[1], [0, 0]).unwrap();
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    // Initially all shifts are unknown: the first bounded call must still score.
    exact(
        scored(cache.score_bounded(&case.tracks[1], 0, Some(0.), || Ok(()))),
        Ok(expected.clone()),
    );
    assert_eq!(cache.stats().completed_first_fits, 1);
    assert_eq!(cache.stats().first_factor_failures, 2);
    let before = cache.stats();
    let PairOutcome::Pruned {
        lower_bound_rmse_hz: bound,
    } = cache
        .score_bounded(&case.tracks[1], 0, Some(0.), || Ok(()))
        .unwrap()
    else {
        panic!("Every shift is either a completed fit or a proved first-support failure");
    };
    assert!(bound.is_finite() && bound > 0. && bound <= expected.combined_rmse_hz);
    assert_eq!(cache.stats().first_factor_calls, before.first_factor_calls);
    assert_eq!(
        cache.stats().second_factor_calls,
        before.second_factor_calls
    );
    // The strict boundary must return the entire ordinary scalar score.
    exact(
        scored(cache.score_bounded(&case.tracks[1], 0, Some(bound), || Ok(()))),
        Ok(expected),
    );
    assert_eq!(
        cache.stats().first_support_cache_hits,
        before.first_support_cache_hits + 2
    );
}

#[test]
fn all_unavailable_duplicate_signed_zero_shifts_keep_coverage_error_and_clock_identity() {
    let mut case = missing_first_support_case();
    case.config.shared_shifts_s = vec![-0., 0.];
    let arrival = PreparedTrack::new(&case.tracks[0], case.observer, case.config.model)
        .unwrap()
        .local_closest_approaches()[0]
        .arrival_time_s;
    let clock = arrival - case.config.event1_cpa_recording_s;
    case.config.clock_offset_bounds_s = Some([clock, clock]);
    let expected = reference(&case, &case.tracks[1], [0, 0]);
    assert!(
        expected
            .as_ref()
            .unwrap_err()
            .starts_with("No shared clock shift")
    );
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    for cutoff in [None, Some(-0.), Some(f64::MAX)] {
        exact(
            scored(cache.score_bounded(&case.tracks[1], 0, cutoff, || Ok(()))),
            expected.clone(),
        );
    }
    assert_eq!(cache.fits.len(), 2);
    assert_eq!(cache.stats().first_factor_failures, 2);
    assert_eq!(cache.stats().completed_first_fits, 0);
    assert_eq!(cache.stats().first_support_cache_hits, 4);
    cache.fits[1].as_mut().unwrap().clock_bits ^= 1;
    assert_eq!(
        cache
            .score_bounded(&case.tracks[1], 0, Some(0.), || Ok(()))
            .unwrap_err(),
        "Immutable first-event cache clock identity changed"
    );
}

#[test]
fn first_support_cache_never_bypasses_second_model_cpa_or_order_validation() {
    let case = missing_first_support_case();
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    exact(
        cache.score(&case.tracks[1], 0, || Ok(())),
        reference(&case, &case.tracks[1], [0, 0]),
    );
    let before = cache.stats();
    for variant in 0..4 {
        let mut partner = case.tracks[1].clone();
        let index = if variant == 3 { 999 } else { 0 };
        match variant {
            0 => partner.points.truncate(1),
            1 => partner
                .points
                .iter_mut()
                .for_each(|point| point.t_s -= 1000.),
            2 => partner.points.retain(|point| point.t_s >= 960.),
            _ => (),
        }
        let expected = reference(&case, &partner, [0, index]);
        assert!(
            !expected
                .as_ref()
                .unwrap_err()
                .starts_with("No shared clock shift")
        );
        exact(
            scored(cache.score_bounded(&partner, index, Some(0.), || Ok(()))),
            expected,
        );
    }
    assert_eq!(
        cache.stats().first_support_cache_hits,
        before.first_support_cache_hits
    );
    assert_eq!(cache.stats().first_factor_calls, before.first_factor_calls);
}

#[test]
fn cancellation_is_never_published_as_first_support_failure() {
    let case = missing_first_support_case();
    let expected = reference(&case, &case.tracks[1], [0, 0]);
    for warm in [false, true] {
        for bounded in [false, true] {
            let make = || {
                FirstEventCache::new(
                    &case.tracks[0],
                    case.observer,
                    [&case.events[0], &case.events[1]],
                    &case.config,
                    0,
                )
            };
            let mut counter = make();
            if warm {
                assert!(counter.score(&case.tracks[1], 0, || Ok(())).is_err());
            }
            let cutoff = bounded.then_some(0.);
            let mut total = 0;
            exact(
                scored(counter.score_bounded(&case.tracks[1], 0, cutoff, || {
                    total += 1;
                    Ok(())
                })),
                expected.clone(),
            );
            for stop in 1..=total {
                let mut cache = make();
                if warm {
                    assert!(cache.score(&case.tracks[1], 0, || Ok(())).is_err());
                }
                let mut calls = 0;
                let result = cache.score_bounded(&case.tracks[1], 0, cutoff, || {
                    calls += 1;
                    if calls == stop {
                        Err("exact first-support cancellation".into())
                    } else {
                        Ok(())
                    }
                });
                assert_eq!(result.unwrap_err(), "exact first-support cancellation");
                assert_eq!(calls, stop);
                // A real deterministic failure proved before a later stop may
                // remain cached; a checkpoint error itself must create no fact.
                if cache.stats().first_factor_failures == 0 {
                    assert!(cache.fits.iter().all(Option::is_none));
                }
                exact(cache.score(&case.tracks[1], 0, || Ok(())), expected.clone());
                assert_eq!(cache.stats().first_factor_failures, 1);
                assert_eq!(cache.stats().completed_first_fits, 0);
            }
        }
    }
}

#[test]
fn unavailable_shift_storage_is_bounded_and_does_not_grow_with_partners() {
    let mut case = missing_first_support_case();
    case.config.shared_shifts_s = vec![1.25; 1001];
    let expected = reference(&case, &case.tracks[1], [0, 0]);
    let mut cache = FirstEventCache::new(
        &case.tracks[0],
        case.observer,
        [&case.events[0], &case.events[1]],
        &case.config,
        0,
    );
    exact(cache.score(&case.tracks[1], 0, || Ok(())), expected.clone());
    assert_eq!(cache.fits.len(), 1001);
    let capacity = cache.fits.capacity();
    let before = cache.stats();
    for _ in 0..3 {
        exact(cache.score(&case.tracks[1], 0, || Ok(())), expected.clone());
        assert_eq!(cache.fits.capacity(), capacity);
    }
    assert_eq!(cache.stats().first_factor_failures, 1001);
    assert_eq!(cache.stats().first_factor_calls, before.first_factor_calls);
    assert_eq!(cache.stats().first_support_cache_hits, 3003);
    assert_eq!(
        cache.stats().reused_first_factor_queries,
        3 * before.first_factor_calls
    );
}
