//! Independent numerical contracts motivated by DOPPLERGEIST tests at commit
//! 70626ad44fb9c7f4e2f5b225956c1f9281f91c13. These are model tests, not evidence
//! of measured geolocation accuracy. The oracle below generates arrival times
//! from known emission times, independently of the production inverse solver.

use geosonnar::physics::*;

const C: f64 = 343.0;

#[test]
fn roundoff_at_an_ecef_track_vertex_does_not_duplicate_the_same_closest_approach() {
    // Independent synthetic straight-line samples. ECEF subtraction puts the
    // two computed roots about 4e-11 seconds apart around the shared vertex.
    let observer = [-3209143.2380034938, 4729054.570462998, 2822306.229488748];
    for epoch in [0.0, 1_790_000_000.0] {
        let track = Track {
            flight_id: "roundoff-vertex".into(),
            points: vec![
                TrackPoint {
                    t_s: epoch + 150.0,
                    xyz_m: [-3210318.618409764, 4731215.521342871, 2823175.4049459444],
                },
                TrackPoint {
                    t_s: epoch + 155.0,
                    xyz_m: [-3210391.6030816697, 4730983.262839338, 2823479.539210591],
                },
                TrackPoint {
                    t_s: epoch + 160.0,
                    xyz_m: [-3210464.5877535753, 4730751.004335804, 2823783.673475238],
                },
            ],
        };
        let model = PreparedTrack::new(&track, observer, ModelConfig::default()).unwrap();
        assert_eq!(model.local_closest_approaches().len(), 1);
        let closest = model.local_closest_approaches()[0];
        assert!(!closest.at_track_boundary);
        close(closest.emission_time_s, epoch + 155.0, 1e-8);
    }
}

#[test]
fn numerical_vertex_repair_preserves_two_resolved_nearby_minima() {
    let observer = [6_378_137.0, 0.0, 0.0];
    let track = Track {
        flight_id: "two-resolved-minima".into(),
        points: [
            (0.0, 2.0),
            (1.0, 1.0),
            (1.0001, 1.01),
            (1.0002, 1.0),
            (2.0, 2.0),
        ]
        .into_iter()
        .map(|(t_s, y)| TrackPoint {
            t_s,
            xyz_m: [observer[0] + 1000.0, y, 0.0],
        })
        .collect(),
    };
    let model = PreparedTrack::new(&track, observer, ModelConfig::default()).unwrap();
    let minima = model.local_closest_approaches();
    assert_eq!(minima.len(), 2);
    close(minima[0].emission_time_s, 1.0, 1e-12);
    close(minima[1].emission_time_s, 1.0002, 1e-12);
}

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} != {expected} (tolerance {tolerance})"
    );
}

fn straight(id: &str, center: f64, speed: f64, slant: f64) -> Track {
    Track {
        flight_id: id.into(),
        points: (-120..=120)
            .step_by(20)
            .map(|t| TrackPoint {
                t_s: center + t as f64,
                xyz_m: [speed * t as f64, slant, 0.0],
            })
            .collect(),
    }
}

fn trace(center: f64, speed: f64, slant: f64, clock: f64, f0: f64, step: usize) -> EventTrace {
    EventTrace {
        points: (-30..=30)
            .step_by(step)
            .map(|emission| {
                let elapsed = emission as f64;
                let range = (speed * elapsed).hypot(slant);
                TracePoint {
                    t_rec_s: center + elapsed + range / C - clock,
                    frequency_hz: f0 * C / (C + speed * speed * elapsed / range),
                }
            })
            .collect(),
        f0_hz: vec![f0],
    }
}

fn pair_fixture() -> ([Track; 2], [EventTrace; 2], PairConfig) {
    let tracks = [
        straight("first", 500.0, 200.0, 1500.0),
        straight("second", 1420.0, 170.0, 2000.0),
    ];
    let events = [
        trace(500.0, 200.0, 1500.0, 400.0, 110.0, 2),
        trace(1420.0, 170.0, 2000.0, 400.0, 140.0, 3),
    ];
    let config = PairConfig {
        event1_cpa_recording_s: 100.0 + 1500.0 / C,
        shared_shifts_s: (-40..=40).map(|i| i as f64 * 0.25).collect(),
        clock_offset_bounds_s: None,
        model: ModelConfig::default(),
    };
    (tracks, events, config)
}

#[test]
fn forward_oracle_matches_inverse_at_large_epoch_and_arbitrary_ecef_origin() {
    let origin = geodetic_to_ecef(40.0, -100.0, 100.0).unwrap();
    for epoch in [0.0, 1_790_000_000.0] {
        let mut track = straight("epoch", epoch, 80.0, 1300.0);
        for p in &mut track.points {
            p.xyz_m = [
                origin[0] + p.xyz_m[0],
                origin[1] + p.xyz_m[1],
                origin[2] + 700.0,
            ];
        }
        let model = PreparedTrack::new(&track, origin, ModelConfig::default()).unwrap();
        let slant = 1300.0_f64.hypot(700.0);
        for i in -400..=400 {
            let emission = i as f64 * 0.2;
            let range = (80.0 * emission).hypot(slant);
            let arrival = epoch + emission + range / C;
            let expected = C / (C + 80.0 * 80.0 * emission / range);
            close(model.factor_at_arrival(arrival).unwrap(), expected, 4e-9);
        }
        let cpa = model.closest_approach();
        close(cpa.emission_time_s, epoch, 1e-8);
        close(cpa.slant_distance_m, slant, 1e-8);
    }
}

#[test]
fn nearly_sonic_motion_still_solves_the_propagation_equation() {
    let speed = 342.9999;
    let track = Track {
        flight_id: "near-sonic".into(),
        points: [-1000.0, 0.0, 1000.0]
            .into_iter()
            .map(|t| TrackPoint {
                t_s: t,
                xyz_m: [speed * t, 100.0, 0.0],
            })
            .collect(),
    };
    let model = PreparedTrack::new(
        &track,
        [0.0; 3],
        ModelConfig {
            sound_speed_mps: C,
            max_gap_s: 1000.0,
        },
    )
    .unwrap();
    for t in [-990.0_f64, -700.0, -30.0, 0.0, 30.0, 700.0, 990.0] {
        let range = (speed * t).hypot(100.0);
        let expected = C / (C + speed * speed * t / range);
        close(
            model.factor_at_arrival(t + range / C).unwrap(),
            expected,
            expected * 2e-7,
        );
    }
}

#[test]
fn doppler_is_inverse_arrival_time_derivative_and_has_no_constant_delay() {
    let model_track = straight("derivative", 0.0, 210.0, 1800.0);
    let model = PreparedTrack::new(&model_track, [0.0; 3], ModelConfig::default()).unwrap();
    let arrival = |t: f64| t + (210.0 * t).hypot(1800.0) / C;
    for t in [-35.0, -8.0, 0.0, 17.0, 42.0] {
        let numerical = 2e-4 / (arrival(t + 1e-4) - arrival(t - 1e-4));
        close(
            model.factor_at_arrival(arrival(t)).unwrap(),
            numerical,
            1e-9,
        );
    }
    assert!((arrival(42.0) - 42.0 - (arrival(0.0) - 0.0)).abs() > 20.0);
}

#[test]
fn knot_derivative_uses_outgoing_motion_and_final_endpoint_uses_incoming() {
    let track = Track {
        flight_id: "corner".into(),
        points: vec![
            TrackPoint {
                t_s: 0.0,
                xyz_m: [0.0, 1000.0, 0.0],
            },
            TrackPoint {
                t_s: 10.0,
                xyz_m: [1000.0, 1000.0, 0.0],
            },
            TrackPoint {
                t_s: 20.0,
                xyz_m: [1000.0, 1500.0, 0.0],
            },
        ],
    };
    let model = PreparedTrack::new(&track, [0.0; 3], ModelConfig::default()).unwrap();
    close(
        model
            .factor_at_arrival(10.0 + 1000.0_f64.hypot(1000.0) / C)
            .unwrap(),
        C / (C + 50.0 / 2.0_f64.sqrt()),
        1e-12,
    );
    close(
        model
            .factor_at_arrival(model.arrival_bounds_s()[1])
            .unwrap(),
        C / (C + 50.0 * 1500.0 / 1000.0_f64.hypot(1500.0)),
        1e-12,
    );
    assert!(
        model
            .factor_at_arrival(model.arrival_bounds_s()[0] - 1e-8)
            .is_err()
    );
    assert!(
        model
            .factor_at_arrival(model.arrival_bounds_s()[1] + 1e-8)
            .is_err()
    );
    assert!(model.factor_at_arrival(f64::NAN).is_err());
}

#[test]
fn rejects_supersonic_full_vector_even_when_radial_speed_is_small() {
    let track = Track {
        flight_id: "unsupported".into(),
        points: vec![
            TrackPoint {
                t_s: 0.0,
                xyz_m: [0.0, 0.0, 3000.0],
            },
            TrackPoint {
                t_s: 1.0,
                xyz_m: [340.0, 100.0, 3000.0],
            },
        ],
    };
    assert!(validate_track(&track, ModelConfig::default()).is_err());
    assert!(PreparedTrack::new(&track, [0.0; 3], ModelConfig::default()).is_err());
    let track = straight("sound-sensitivity", 0.0, 300.0, 1000.0);
    assert!(PreparedTrack::new(&track, [0.0; 3], ModelConfig::default()).is_ok());
    assert!(
        PreparedTrack::new(
            &track,
            [0.0; 3],
            ModelConfig {
                sound_speed_mps: 290.0,
                max_gap_s: 30.0
            }
        )
        .is_err()
    );
}

#[test]
fn invalid_units_order_gaps_and_zero_range_never_become_finite_predictions() {
    let original = straight("valid", 0.0, 100.0, 3000.0);
    let mut cases = Vec::new();
    let mut changed = original.clone();
    changed.points[1].t_s = changed.points[0].t_s;
    cases.push(changed);
    let mut changed = original.clone();
    changed.points.swap(0, 1);
    cases.push(changed);
    let mut changed = original.clone();
    changed.points[1].xyz_m[0] = f64::NAN;
    cases.push(changed);
    let mut changed = original.clone();
    changed.points[1].t_s = f64::INFINITY;
    cases.push(changed);
    let mut changed = original.clone();
    changed.points.remove(1);
    cases.push(changed);
    let mut changed = original.clone();
    changed.flight_id = " ".into();
    cases.push(changed);
    let mut changed = original.clone();
    changed.points.truncate(1);
    cases.push(changed);
    for track in &cases {
        assert!(validate_track(track, ModelConfig::default()).is_err());
        assert!(PreparedTrack::new(track, [0.0; 3], ModelConfig::default()).is_err());
    }
    assert!(PreparedTrack::new(&original, [f64::NAN, 0.0, 0.0], ModelConfig::default()).is_err());
    assert!(
        PreparedTrack::new(
            &straight("collision", 0.0, 100.0, 0.0),
            [0.0; 3],
            ModelConfig::default()
        )
        .is_err()
    );
    for bad in [0.0, -1.0, f64::INFINITY, f64::NAN] {
        assert!(
            frequency_at_arrival(&original, [0.0; 3], 10.0, bad, ModelConfig::default()).is_err()
        );
        assert!(
            validate_track(
                &original,
                ModelConfig {
                    sound_speed_mps: bad,
                    max_gap_s: 30.0
                }
            )
            .is_err()
        );
        assert!(
            validate_track(
                &original,
                ModelConfig {
                    sound_speed_mps: C,
                    max_gap_s: bad
                }
            )
            .is_err()
        );
    }
}

#[test]
fn global_coordinate_conversion_handles_poles_antimeridian_and_negative_heights() {
    let a = 6_378_137.0;
    let b = a * (1.0 - 1.0 / 298.257_223_563);
    for sign in [-1.0, 1.0] {
        let pole = geodetic_to_ecef(sign * 90.0, 123.0, 80.0).unwrap();
        close(pole[2], sign * (b + 80.0), 1e-8);
        assert!(pole[0].hypot(pole[1]) < 1e-8);
    }
    let west = geodetic_to_ecef(0.0, -180.0, -30.0).unwrap();
    let east = geodetic_to_ecef(0.0, 180.0, -30.0).unwrap();
    close(west[0], -a + 30.0, 1e-8);
    close(west[1], east[1], 2e-9);
    for (lat, lon, h) in [
        (90.1, 0.0, 0.0),
        (0.0, -180.1, 0.0),
        (f64::NAN, 0.0, 0.0),
        (0.0, 0.0, f64::INFINITY),
    ] {
        assert!(geodetic_to_ecef(lat, lon, h).is_err());
    }
}

#[test]
fn last_interval_interior_cpa_is_retained_even_when_last_sample_is_nearest() {
    let track = Track {
        flight_id: "edge-regression".into(),
        points: [-20.0, -10.0, 1.0]
            .into_iter()
            .map(|t| TrackPoint {
                t_s: t,
                xyz_m: [100.0 * t, 3000.0, 0.0],
            })
            .collect(),
    };
    let minima = local_closest_approaches(&track, [0.0; 3], ModelConfig::default()).unwrap();
    assert_eq!(minima.len(), 1);
    close(minima[0].emission_time_s, 0.0, 1e-12);
    assert_eq!(minima[0].segment_index, 1);
    assert!(!minima[0].at_track_boundary);
}

#[test]
fn repeated_flyovers_preserve_every_local_cpa_without_spurious_segment_minima() {
    let track = Track {
        flight_id: "repeated".into(),
        points: (-40..=200)
            .map(|i| {
                let t = i as f64 * 0.5;
                TrackPoint {
                    t_s: t,
                    xyz_m: [
                        4000.0 * (std::f64::consts::TAU * t / 80.0).sin(),
                        1300.0,
                        700.0,
                    ],
                }
            })
            .collect(),
    };
    let minima = local_closest_approaches(&track, [0.0; 3], ModelConfig::default()).unwrap();
    assert_eq!(minima.len(), 3);
    for (cpa, time) in minima.iter().zip([0.0, 40.0, 80.0]) {
        close(cpa.emission_time_s, time, 1e-10);
        close(cpa.slant_distance_m, 1300.0_f64.hypot(700.0), 1e-10);
        assert!(!cpa.at_track_boundary);
    }
}

#[test]
fn corner_minima_are_distinguished_from_turning_away_and_plateaus_are_deduplicated() {
    let mk = |xs: &[f64]| Track {
        flight_id: "knots".into(),
        points: xs
            .iter()
            .enumerate()
            .map(|(i, &x)| TrackPoint {
                t_s: i as f64 * 10.0,
                xyz_m: [x, 1000.0, 0.0],
            })
            .collect(),
    };
    let track = mk(&[2000.0, 1000.0, 2000.0, 1500.0, 1500.0, 2000.0]);
    let minima = local_closest_approaches(&track, [0.0; 3], ModelConfig::default()).unwrap();
    assert_eq!(minima.len(), 2);
    close(minima[0].emission_time_s, 10.0, 0.0);
    close(minima[1].emission_time_s, 30.0, 0.0);
    let constant = mk(&[1000.0, 1000.0, 1000.0]);
    let minima = local_closest_approaches(&constant, [0.0; 3], ModelConfig::default()).unwrap();
    assert_eq!(minima.len(), 1);
    assert!(minima[0].at_track_boundary);
    let descending_plateau = mk(&[2000.0, 1000.0, 1000.0, 500.0]);
    let minima =
        local_closest_approaches(&descending_plateau, [0.0; 3], ModelConfig::default()).unwrap();
    assert_eq!(minima.len(), 1);
    assert!(minima[0].at_track_boundary);
}

#[test]
fn one_track_can_supply_two_distinct_passes_under_one_locked_clock() {
    let track = Track {
        flight_id: "return-flight".into(),
        points: (-6..=26)
            .map(|i| {
                let t = i as f64 * 10.0;
                let x = if t <= 100.0 {
                    100.0 * t
                } else {
                    100.0 * (200.0 - t)
                };
                TrackPoint {
                    t_s: t,
                    xyz_m: [x, 1500.0, 0.0],
                }
            })
            .collect(),
    };
    let events = [
        trace(0.0, 100.0, 1500.0, -100.0, 110.0, 3),
        trace(200.0, 100.0, 1500.0, -100.0, 140.0, 3),
    ];
    let config = PairConfig {
        event1_cpa_recording_s: 100.0 + 1500.0 / C,
        shared_shifts_s: vec![0.0],
        clock_offset_bounds_s: None,
        model: ModelConfig::default(),
    };
    let score = score_pair_at_cpa(
        [&track, &track],
        [0.0; 3],
        [&events[0], &events[1]],
        &config,
        [0, 1],
    )
    .unwrap();
    assert!(score.combined_rmse_hz < 1e-11);
    close(score.clock_offset_s, -100.0, 1e-10);
    close(score.cpa[1].emission_time_s, 200.0, 1e-10);
    assert!(
        score_pair_at_cpa(
            [&track, &track],
            [0.0; 3],
            [&events[0], &events[1]],
            &config,
            [0, 99]
        )
        .is_err()
    );
    assert!(
        score_pair_at_cpa(
            [&track, &track],
            [0.0; 3],
            [&events[0], &events[1]],
            &config,
            [0, 0]
        )
        .is_err()
    );
    assert!(
        score_pair_at_cpa(
            [&track, &track],
            [0.0; 3],
            [&events[0], &events[1]],
            &config,
            [1, 0]
        )
        .is_err()
    );
}

#[test]
fn independent_tone_fits_cannot_realign_a_wrong_second_event() {
    let (tracks, mut events, config) = pair_fixture();
    events[0].f0_hz = vec![109.0, 110.0, 111.0];
    events[1].f0_hz = vec![139.0, 140.0, 141.0];
    let fit = score_pair(
        [&tracks[0], &tracks[1]],
        [0.0; 3],
        [&events[0], &events[1]],
        &config,
    )
    .unwrap();
    assert!(fit.combined_rmse_hz < 1e-11);
    assert_eq!(fit.fitted_f0_hz, [110.0, 140.0]);
    close(fit.clock_offset_s, 400.0, 1e-12);
    let mut wrong = tracks[1].clone();
    for p in &mut wrong.points {
        p.t_s += 8.0;
    }
    let wrong_fit = score_pair(
        [&tracks[0], &wrong],
        [0.0; 3],
        [&events[0], &events[1]],
        &config,
    )
    .unwrap();
    assert!(
        wrong_fit.combined_rmse_hz > 2.0,
        "{}",
        wrong_fit.combined_rmse_hz
    );
    assert!(wrong_fit.rmse_hz.iter().any(|&error| error > 2.0));
}

#[test]
fn fixed_trace_windows_cannot_shrink_to_make_incomplete_coverage_win() {
    let (mut tracks, events, mut config) = pair_fixture();
    config.shared_shifts_s = vec![0.0];
    tracks[1].points.retain(|p| p.t_s <= 1440.0);
    let error = score_pair(
        [&tracks[0], &tracks[1]],
        [0.0; 3],
        [&events[0], &events[1]],
        &config,
    )
    .unwrap_err();
    assert!(error.contains("complete track coverage"));
}

#[test]
fn common_clock_bounds_and_unordered_or_invalid_traces_are_enforced() {
    let (tracks, events, mut config) = pair_fixture();
    config.shared_shifts_s = vec![0.0];
    config.clock_offset_bounds_s = Some([399.0, 401.0]);
    assert!(
        score_pair(
            [&tracks[0], &tracks[1]],
            [0.0; 3],
            [&events[0], &events[1]],
            &config
        )
        .is_ok()
    );
    config.clock_offset_bounds_s = Some([1000.0, 2000.0]);
    assert!(
        score_pair(
            [&tracks[0], &tracks[1]],
            [0.0; 3],
            [&events[0], &events[1]],
            &config
        )
        .is_err()
    );
    config.clock_offset_bounds_s = None;
    let mut cases = Vec::new();
    let mut changed = events[0].clone();
    changed.points.truncate(2);
    cases.push(changed);
    let mut changed = events[0].clone();
    changed.points.swap(0, 1);
    cases.push(changed);
    let mut changed = events[0].clone();
    changed.points[0].frequency_hz = f64::NAN;
    cases.push(changed);
    let mut changed = events[0].clone();
    changed.points[0].frequency_hz = 0.0;
    cases.push(changed);
    let mut changed = events[0].clone();
    changed.f0_hz = vec![];
    cases.push(changed);
    let mut changed = events[0].clone();
    changed.f0_hz = vec![f64::INFINITY];
    cases.push(changed);
    for invalid in cases {
        assert!(
            score_pair(
                [&tracks[0], &tracks[1]],
                [0.0; 3],
                [&invalid, &events[1]],
                &config
            )
            .is_err()
        );
    }
    assert!(
        score_pair(
            [&tracks[0], &tracks[1]],
            [0.0; 3],
            [&events[1], &events[0]],
            &config
        )
        .is_err()
    );
}

#[test]
fn event_weights_do_not_change_when_one_ridge_is_more_densely_traced() {
    let (tracks, mut events, mut config) = pair_fixture();
    config.shared_shifts_s = vec![0.0];
    for p in &mut events[0].points {
        p.frequency_hz += 3.0;
    }
    for p in &mut events[1].points {
        p.frequency_hz += 4.0;
    }
    let fit = score_pair(
        [&tracks[0], &tracks[1]],
        [0.0; 3],
        [&events[0], &events[1]],
        &config,
    )
    .unwrap();
    close(fit.rmse_hz[0], 3.0, 1e-11);
    close(fit.rmse_hz[1], 4.0, 1e-11);
    close(fit.combined_rmse_hz, 12.5_f64.sqrt(), 1e-11);
    events[0] = trace(500.0, 200.0, 1500.0, 400.0, 110.0, 1);
    for p in &mut events[0].points {
        p.frequency_hz += 3.0;
    }
    let denser = score_pair(
        [&tracks[0], &tracks[1]],
        [0.0; 3],
        [&events[0], &events[1]],
        &config,
    )
    .unwrap();
    close(denser.combined_rmse_hz, fit.combined_rmse_hz, 1e-11);
}
