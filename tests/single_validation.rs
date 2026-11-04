use geosonnar::{
    physics::{self, EventTrace, ModelConfig, TracePoint, Track, TrackPoint},
    single::{self, SingleInput},
    verification::Observer,
};

fn fixture(epoch: f64) -> SingleInput {
    let observer = Observer {
        id: "true-grid-point".into(),
        latitude_deg: 40.,
        longitude_deg: -100.,
        ellipsoid_height_m: 100.,
    };
    let xyz = physics::geodetic_to_ecef(40., -100., 100.).unwrap();
    let speed = 80.;
    let clock = epoch - 150.;
    let track = Track {
        flight_id: "known-flight".into(),
        points: (-100..=100)
            .step_by(5)
            .map(|t| TrackPoint {
                t_s: epoch + t as f64,
                xyz_m: [xyz[0] + speed * t as f64, xyz[1] + 1300., xyz[2] + 700.],
            })
            .collect(),
    };
    // Independent forward reception times and radial velocities, not the inverse
    // solver or its interpolation helper. Truth is off most frequency trials.
    let points = (-60..=60)
        .step_by(6)
        .map(|t| {
            let x = speed * t as f64;
            let range = x.hypot(1300.).hypot(700.);
            TracePoint {
                t_rec_s: epoch + t as f64 + range / 343. - clock,
                frequency_hz: 120. * 343. / (343. + speed * x / range),
            }
        })
        .collect();
    let mut wrong = observer.clone();
    wrong.id = "wrong-grid-point".into();
    wrong.longitude_deg += 0.02;
    SingleInput {
        schema_version: 1,
        evidence_kind: "synthetic".into(),
        provenance: vec![
            "Independent analytic straight-line single-event control; not location evidence".into(),
        ],
        tracks: vec![track],
        observers: vec![wrong, observer],
        event: EventTrace {
            points,
            f0_hz: vec![130., 115., 120., 125.],
        },
        model: ModelConfig::default(),
        clock_offsets_s: vec![clock - 2., clock, clock + 2.],
        coarse_bounds: None,
    }
}

#[test]
fn known_clock_single_event_recovers_independent_analytic_signal_at_large_epoch() {
    for epoch in [0., 1_790_000_000.] {
        let input = fixture(epoch);
        let report = single::rank(&input).unwrap();
        let best = &report["candidates"][0];
        assert_eq!(best["observer"]["id"], "true-grid-point");
        assert_eq!(best["score"]["flight_id"], "known-flight");
        assert_eq!(best["score"]["fitted_f0_hz"], 120.);
        assert_eq!(best["score"]["clock_offset_s"], epoch - 150.);
        assert!(best["score"]["rmse_hz"].as_f64().unwrap() < 3e-6);
        assert_eq!(best["score"]["support_fraction"], 1.);
        assert_eq!(report["location_verified"], false);
    }
}

#[test]
fn single_event_never_shortens_ridge_to_available_track_support() {
    let mut input = fixture(0.);
    input.tracks[0].points.retain(|p| p.t_s <= 0.);
    let report = single::rank(&input).unwrap();
    assert_eq!(report["status"], "no_candidate");
    assert_eq!(report["supported_clock_trials"], 0);
    assert_eq!(report["unsupported_clock_trials"], 6);
}

#[test]
fn malformed_single_event_input_is_an_error_not_a_scientific_rejection() {
    let mut input = fixture(0.);
    input.tracks[0].points[1].t_s = input.tracks[0].points[0].t_s;
    assert!(single::rank(&input).is_err());
    let mut input = fixture(0.);
    input.event.points[2].t_rec_s = input.event.points[1].t_rec_s;
    assert!(single::rank(&input).is_err());
    let mut input = fixture(0.);
    input.clock_offsets_s.clear();
    assert!(single::rank(&input).is_err());
}

#[test]
fn declared_clock_trials_are_binding() {
    let mut input = fixture(0.);
    input.clock_offsets_s = vec![10000.];
    let report = single::rank(&input).unwrap();
    assert_eq!(report["status"], "no_candidate");
    assert_eq!(report["supported_clock_trials"], 0);
}

#[test]
fn cached_profile_is_identical_to_independent_clock_evaluations() {
    let mut input = fixture(0.);
    // Include noninteger and uneven shifts as well as repeated reception times.
    // Fixed quarter-second sample times ensure exact overlaps for some shifts;
    // other shifts must remain independent cache keys without rounding.
    let original = input.event.points.clone();
    input.event.points = (0..120)
        .map(|i| TracePoint {
            t_rec_s: 130. + i as f64 * 0.25,
            frequency_hz: original[i % original.len()].frequency_hz,
        })
        .collect();
    input.clock_offsets_s = vec![-151., -150.75, -150.1, -150., -149.5, -149.125];
    let cached = single::rank(&input).unwrap();
    assert!(cached["factor_cache_hits"].as_u64().unwrap() > 0);
    let clocks = input.clock_offsets_s.clone();
    let mut standalone = Vec::new();
    let mut independent_evaluations = 0;
    for clock in clocks {
        input.clock_offsets_s = vec![clock];
        let result = single::rank(&input).unwrap();
        independent_evaluations += result["factor_evaluations"].as_u64().unwrap();
        standalone.extend(result["candidates"].as_array().unwrap().iter().cloned());
    }
    assert!(cached["factor_evaluations"].as_u64().unwrap() < independent_evaluations);
    for row in cached["candidates"].as_array().unwrap() {
        let best = standalone
            .iter()
            .filter(|other| {
                other["observer"]["id"] == row["observer"]["id"]
                    && other["score"]["flight_id"] == row["score"]["flight_id"]
            })
            .min_by(|a, b| {
                a["score"]["rmse_hz"]
                    .as_f64()
                    .unwrap()
                    .total_cmp(&b["score"]["rmse_hz"].as_f64().unwrap())
            })
            .unwrap();
        assert_eq!(
            row, best,
            "Caching must preserve the exact profiled winner and score"
        );
    }
}
