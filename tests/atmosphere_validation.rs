//! Numerical/model contracts, not a claim of improved field geolocation.
use geosonnar::{
    atmosphere::*,
    physics::{ModelConfig, PreparedTrack, Track, TrackPoint, geodetic_to_ecef},
};

const UTC: f64 = 1_790_000_000.0;
const A: f64 = 6_378_137.0;

fn level(
    height: f64,
    temperature: f64,
    pressure: f64,
    rh: f64,
    east: f64,
    north: f64,
) -> AtmosphereLevel {
    AtmosphereLevel {
        ellipsoid_height_m: height,
        temperature_k: temperature,
        pressure_pa: pressure,
        relative_humidity_fraction: rh,
        eastward_wind_mps: east,
        northward_wind_mps: north,
    }
}

fn profile() -> AtmosphereProfile {
    AtmosphereProfile {
        schema_version: 1,
        name: "Synthetic model validation; not weather data".into(),
        provenance: AtmosphereProvenance {
            provider: "Synthetic fixture".into(),
            source: "Analytic control".into(),
            source_sha256: "0".repeat(64),
            notes: "No real recording or weather observation".into(),
        },
        valid_from_utc_s: UTC - 1000.0,
        valid_to_utc_s: UTC + 10_000.0,
        bounds: [-1.0, -1.0, 1.0, 1.0],
        height_datum: AtmosphereHeightDatum::Wgs84Ellipsoid,
        humidity_reference: HumidityReference::LiquidWater,
        levels: vec![
            level(-1000.0, 293.15, 101_325.0, 0.0, 0.0, 0.0),
            level(20_000.0, 293.15, 101_325.0, 0.0, 0.0, 0.0),
        ],
        integration: IntegrationSettings::default(),
    }
}

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} != {expected}, tolerance {tolerance}"
    );
}

#[test]
fn ideal_humid_air_is_close_to_published_reference_but_not_claimed_exact() {
    // Low-frequency values from Gavioso et al., JPCRD (2025), Table 5.
    // Our deliberately simpler ideal-mixture model omits real-gas corrections.
    for (temperature, rh, reference) in [
        (273.15, 0.4, 331.5682),
        (293.15, 0.6, 344.0945),
        (303.15, 0.8, 350.9820),
    ] {
        close(
            humid_sound_speed_mps(temperature, 101_325.0, rh).unwrap(),
            reference,
            0.3,
        );
    }
    let dry = humid_sound_speed_mps(293.15, 101_325.0, 0.0).unwrap();
    let wet = humid_sound_speed_mps(293.15, 101_325.0, 1.0).unwrap();
    close(dry, 343.2, 0.1);
    assert!(wet - dry > 1.0 && wet - dry < 1.5);
}

#[test]
fn humidity_pressure_and_temperature_are_not_silently_clamped() {
    for (temperature, pressure, rh) in [
        (199.0, 101_325.0, 0.5),
        (331.0, 101_325.0, 0.5),
        (293.0, 500.0, 0.5),
        (293.0, 101_325.0, 50.0),
        (293.0, 101_325.0, -0.1),
        (f64::NAN, 101_325.0, 0.5),
        (330.0, 5000.0, 1.0),
    ] {
        assert!(humid_sound_speed_mps(temperature, pressure, rh).is_err());
    }
}

#[test]
fn dry_air_is_pressure_independent_but_moisture_fraction_is_not() {
    close(
        humid_sound_speed_mps(290.0, 50_000.0, 0.0).unwrap(),
        humid_sound_speed_mps(290.0, 100_000.0, 0.0).unwrap(),
        1e-12,
    );
    assert!(
        humid_sound_speed_mps(290.0, 50_000.0, 0.5).unwrap()
            > humid_sound_speed_mps(290.0, 100_000.0, 0.5).unwrap()
    );
}

#[test]
fn ecef_inverse_preserves_wgs84_height_and_position() {
    for coordinates in [
        [0.0, 0.0, 0.0],
        [46.8, -71.2, 1234.0],
        [-51.4, 170.2, -200.0],
        [84.0, 179.0, 12000.0],
    ] {
        let xyz = geodetic_to_ecef(coordinates[0], coordinates[1], coordinates[2]).unwrap();
        let actual = ecef_to_geodetic(xyz).unwrap();
        close(actual[0], coordinates[0], 1e-9);
        close(actual[1], coordinates[1], 1e-9);
        close(actual[2], coordinates[2], 1e-6);
    }
    assert!(ecef_to_geodetic([0.0; 3]).is_err());
    assert!(ecef_to_geodetic([f64::NAN, 0.0, 0.0]).is_err());
}

#[test]
fn uniform_dry_atmosphere_reduces_to_range_over_sound_speed() {
    let p = profile();
    let atmosphere = PreparedAtmosphere::new(&p).unwrap();
    let source = [A + 2000.0, -1500.0, 1000.0];
    let receiver = [A + 100.0, 500.0, 0.0];
    let distance = (1900.0_f64.powi(2) + 2000.0_f64.powi(2) + 1000.0_f64.powi(2)).sqrt();
    let result = atmosphere.travel_time(source, receiver, UTC).unwrap();
    close(result.path_length_m, distance, 1e-9);
    close(
        result.travel_time_s,
        distance / humid_sound_speed_mps(293.15, 101_325.0, 0.0).unwrap(),
        1e-10,
    );
    assert!(result.integration_error_s <= p.integration.absolute_tolerance_s);
}

#[test]
fn linear_temperature_vertical_integral_matches_closed_form() {
    let mut p = profile();
    p.levels = vec![
        level(-100.0, 290.0, 101_325.0, 0.0, 0.0, 0.0),
        level(0.0, 290.0, 101_325.0, 0.0, 0.0, 0.0),
        level(2000.0, 270.0, 101_325.0, 0.0, 0.0, 0.0),
        level(3000.0, 270.0, 101_325.0, 0.0, 0.0, 0.0),
    ];
    let atmosphere = PreparedAtmosphere::new(&p).unwrap();
    let expected = 4000.0
        / (humid_sound_speed_mps(290.0, 101_325.0, 0.0).unwrap()
            + humid_sound_speed_mps(270.0, 101_325.0, 0.0).unwrap());
    close(
        atmosphere
            .travel_time([A + 2000.0, 0.0, 0.0], [A, 0.0, 0.0], UTC)
            .unwrap()
            .travel_time_s,
        expected,
        2e-7,
    );
}

#[test]
fn tailwind_headwind_and_crosswind_have_directional_effects() {
    let mut p = profile();
    for l in &mut p.levels {
        l.eastward_wind_mps = 20.0;
    }
    let atmosphere = PreparedAtmosphere::new(&p).unwrap();
    let source = [A + 1000.0, -500.0, 0.0];
    let receiver = [A + 1000.0, 500.0, 0.0];
    let c = humid_sound_speed_mps(293.15, 101_325.0, 0.0).unwrap();
    let tail = atmosphere
        .travel_time(source, receiver, UTC)
        .unwrap()
        .travel_time_s;
    let head = atmosphere
        .travel_time(receiver, source, UTC)
        .unwrap()
        .travel_time_s;
    close(tail, 1000.0 / (c + 20.0), 1e-7);
    close(head, 1000.0 / (c - 20.0), 1e-7);
    assert!(tail < 1000.0 / c && head > 1000.0 / c);
    // With wind along local north, this equatorial east/west path is crosswind.
    for l in &mut p.levels {
        l.eastward_wind_mps = 0.0;
        l.northward_wind_mps = 20.0;
    }
    let cross = PreparedAtmosphere::new(&p)
        .unwrap()
        .travel_time(source, receiver, UTC)
        .unwrap()
        .travel_time_s;
    close(cross, 1000.0 / c, 1e-10);
}

#[test]
fn altitude_sampling_interpolates_pressure_logarithmically_without_extrapolation() {
    let mut p = profile();
    p.levels = vec![
        level(0.0, 290.0, 100_000.0, 0.1, -10.0, 5.0),
        level(2000.0, 270.0, 25_000.0, 0.5, 10.0, -5.0),
    ];
    let atmosphere = PreparedAtmosphere::new(&p).unwrap();
    let middle = atmosphere.sample_at_height(1000.0).unwrap();
    close(middle.temperature_k, 280.0, 1e-10);
    close(middle.pressure_pa, 50_000.0, 1e-8);
    close(middle.relative_humidity_fraction, 0.3, 1e-10);
    close(middle.eastward_wind_mps, 0.0, 1e-10);
    close(middle.northward_wind_mps, 0.0, 1e-10);
    assert!(atmosphere.sample_at_height(-0.01).is_err());
    assert!(atmosphere.sample_at_height(2000.01).is_err());
}

#[test]
fn date_region_height_and_complete_propagation_interval_are_required() {
    let p = profile();
    let atmosphere = PreparedAtmosphere::new(&p).unwrap();
    let source = [A + 2000.0, 0.0, 0.0];
    let receiver = [A + 100.0, 0.0, 0.0];
    assert!(
        atmosphere
            .travel_time(source, receiver, UTC - 1001.0)
            .is_err()
    );
    assert!(
        atmosphere
            .travel_time(source, receiver, p.valid_to_utc_s - 1.0)
            .is_err()
    );
    let outside = geodetic_to_ecef(1.01, 0.0, 2000.0).unwrap();
    assert!(atmosphere.travel_time(outside, receiver, UTC).is_err());
    let high = geodetic_to_ecef(0.0, 0.0, 20_001.0).unwrap();
    assert!(atmosphere.travel_time(high, receiver, UTC).is_err());
    assert!(atmosphere.travel_time(receiver, receiver, UTC).is_err());
}

#[test]
fn profile_rejects_wrong_units_unsorted_levels_and_implicit_datum() {
    let mut p = profile();
    p.levels.swap(0, 1);
    assert!(p.validate().is_err());
    let mut p = profile();
    p.levels[1].ellipsoid_height_m = p.levels[0].ellipsoid_height_m;
    assert!(p.validate().is_err());
    let mut p = profile();
    p.levels[0].pressure_pa = 1013.25;
    assert!(p.validate().is_err());
    let mut p = profile();
    p.levels[0].relative_humidity_fraction = 75.0;
    assert!(p.validate().is_err());
    let mut p = profile();
    p.bounds = [-1.0, 179.0, 1.0, -179.0];
    assert!(p.validate().is_err());
    let mut p = profile();
    p.valid_to_utc_s = p.valid_from_utc_s + 86401.0;
    assert!(p.validate().is_err());
    let mut p = profile();
    p.provenance.source_sha256 = "not-a-hash".into();
    assert!(p.validate().is_err());
    let value = serde_json::to_value(profile()).unwrap();
    for (field, replacement) in [
        ("height_datum", "mean_sea_level"),
        ("humidity_reference", "mixed_phase"),
    ] {
        let mut changed = value.clone();
        changed[field] = replacement.into();
        assert!(serde_json::from_value::<AtmosphereProfile>(changed).is_err());
    }
    let mut changed = value;
    changed["unknown"] = true.into();
    assert!(serde_json::from_value::<AtmosphereProfile>(changed).is_err());
}

#[test]
fn quadrature_budget_is_explicit_and_failure_does_not_fall_back() {
    let mut p = profile();
    p.integration.max_step_m = 10.0;
    p.integration.max_segments = 32;
    let atmosphere = PreparedAtmosphere::new(&p).unwrap();
    assert!(
        atmosphere
            .travel_time([A + 5000.0, 0.0, 0.0], [A + 100.0, 0.0, 0.0], UTC)
            .unwrap_err()
            .contains("budget")
    );
    p.integration.max_segments = 33;
    assert!(p.validate().is_err());
}

#[test]
fn stratified_profile_converges_under_tighter_quadrature() {
    let mut p = profile();
    p.levels = vec![
        level(-100.0, 290.0, 101_000.0, 0.6, 1.0, 2.0),
        level(1200.0, 282.0, 87_000.0, 0.4, 8.0, -2.0),
        level(5000.0, 255.0, 54_000.0, 0.1, 20.0, 3.0),
    ];
    let source = [A + 4000.0, -2000.0, 1000.0];
    let receiver = [A + 100.0, 0.0, 0.0];
    let normal = PreparedAtmosphere::new(&p)
        .unwrap()
        .travel_time(source, receiver, UTC)
        .unwrap();
    p.integration.absolute_tolerance_s = 1e-8;
    p.integration.max_step_m = 100.0;
    let tighter = PreparedAtmosphere::new(&p)
        .unwrap()
        .travel_time(source, receiver, UTC)
        .unwrap();
    close(normal.travel_time_s, tighter.travel_time_s, 2e-6);
}

#[test]
fn stationary_source_has_unit_doppler_factor() {
    let p = profile();
    let atmosphere = PreparedAtmosphere::new(&p).unwrap();
    close(
        atmosphere
            .frequency_factor([A + 2000.0, 0.0, 0.0], [0.0; 3], [A + 100.0, 0.0, 0.0], UTC)
            .unwrap(),
        1.0,
        1e-12,
    );
}

fn track() -> Track {
    Track {
        flight_id: "synthetic-moving-source".into(),
        points: (-2..=2)
            .map(|i| TrackPoint {
                t_s: f64::from(i) * 10.0,
                xyz_m: [A + 2000.0, f64::from(i) * 1000.0, 500.0],
            })
            .collect(),
    }
}

#[test]
fn profile_track_matches_independent_uniform_doppler_oracle_at_epoch() {
    let p = profile();
    let atmosphere = PreparedAtmosphere::new(&p).unwrap();
    let trajectory = track();
    let receiver = [A + 100.0, 0.0, 0.0];
    let modeled =
        PreparedAtmosphericTrack::new(&trajectory, receiver, UTC, 10.0, &atmosphere).unwrap();
    let c = humid_sound_speed_mps(293.15, 101_325.0, 0.0).unwrap();
    for emission in [-20.0_f64, -12.3, 0.0, 8.4, 20.0] {
        let range = (1900.0_f64.powi(2) + (emission * 100.0).powi(2) + 500.0_f64.powi(2)).sqrt();
        let arrival = emission + range / c;
        let factor = 1.0 / (1.0 + 10000.0 * emission / (range * c));
        close(modeled.factor_at_arrival(arrival).unwrap(), factor, 2e-6);
    }
}

#[test]
fn profile_track_preserves_cpa_and_never_extrapolates_or_bridges_gaps() {
    let p = profile();
    let atmosphere = PreparedAtmosphere::new(&p).unwrap();
    let trajectory = track();
    let receiver = [A + 100.0, 0.0, 0.0];
    let modeled =
        PreparedAtmosphericTrack::new(&trajectory, receiver, UTC, 10.0, &atmosphere).unwrap();
    let constant = PreparedTrack::new(
        &trajectory,
        receiver,
        ModelConfig {
            sound_speed_mps: humid_sound_speed_mps(293.15, 101_325.0, 0.0).unwrap(),
            max_gap_s: 10.0,
        },
    )
    .unwrap();
    let cpas = modeled.local_closest_approaches();
    assert_eq!(cpas.len(), 1);
    assert!(!cpas[0].at_track_boundary);
    close(cpas[0].emission_time_s, 0.0, 1e-12);
    close(
        cpas[0].arrival_time_s,
        constant.local_closest_approaches()[0].arrival_time_s,
        1e-9,
    );
    let [first, last] = modeled.arrival_bounds_s();
    assert!(modeled.factor_at_arrival(first - 0.01).is_err());
    assert!(modeled.factor_at_arrival(last + 0.01).is_err());
    assert!(PreparedAtmosphericTrack::new(&trajectory, receiver, UTC, 9.0, &atmosphere).is_err());
    assert!(
        PreparedAtmosphericTrack::new(&trajectory, receiver, UTC + 10000.0, 10.0, &atmosphere)
            .is_err()
    );
}

#[test]
fn fast_motion_is_rejected_under_profile_not_constant_user_speed() {
    let p = profile();
    let atmosphere = PreparedAtmosphere::new(&p).unwrap();
    assert!(
        atmosphere
            .frequency_factor(
                [A + 2000.0, 0.0, 0.0],
                [0.0, 400.0, 0.0],
                [A + 100.0, 0.0, 0.0],
                UTC
            )
            .is_err()
    );
}

#[test]
fn wind_shear_changes_arrival_and_doppler_with_numerical_convergence() {
    let mut p = profile();
    p.levels = vec![
        level(-100.0, 295.0, 101_325.0, 0.7, 0.0, 0.0),
        level(3000.0, 274.0, 70_000.0, 0.3, 22.0, 5.0),
        level(5000.0, 260.0, 54_000.0, 0.1, 30.0, 8.0),
    ];
    let receiver = [A + 100.0, 0.0, 0.0];
    let source = [A + 2000.0, -2000.0, 500.0];
    let velocity = [0.0, 100.0, 0.0];
    let a = PreparedAtmosphere::new(&p).unwrap();
    let arrival = a.travel_time(source, receiver, UTC).unwrap().travel_time_s;
    let factor = a.frequency_factor(source, velocity, receiver, UTC).unwrap();
    let constant = profile();
    let b = PreparedAtmosphere::new(&constant).unwrap();
    assert!((arrival - b.travel_time(source, receiver, UTC).unwrap().travel_time_s).abs() > 0.05);
    assert!((factor - b.frequency_factor(source, velocity, receiver, UTC).unwrap()).abs() > 1e-4);
    p.integration.max_step_m = 100.0;
    p.integration.absolute_tolerance_s = 1e-8;
    close(
        factor,
        PreparedAtmosphere::new(&p)
            .unwrap()
            .frequency_factor(source, velocity, receiver, UTC)
            .unwrap(),
        1e-4,
    );
}

#[test]
fn profile_json_roundtrip_preserves_every_assumption() {
    let original = profile();
    let bytes = serde_json::to_vec(&original).unwrap();
    let restored: AtmosphereProfile = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(original, restored);
    restored.validate().unwrap();
}

#[test]
fn thin_temperature_layer_cannot_hide_between_quadrature_nodes() {
    let mut p = profile();
    // A 20 m anomaly placed between the initial whole-path Simpson nodes.
    p.levels = vec![
        level(-100.0, 290.0, 101_325.0, 0.0, 0.0, 0.0),
        level(101.0, 290.0, 101_325.0, 0.0, 0.0, 0.0),
        level(111.0, 220.0, 101_325.0, 0.0, 0.0, 0.0),
        level(121.0, 290.0, 101_325.0, 0.0, 0.0, 0.0),
        level(1100.0, 290.0, 101_325.0, 0.0, 0.0, 0.0),
    ];
    let c_hot = humid_sound_speed_mps(290.0, 101_325.0, 0.0).unwrap();
    let c_cold = humid_sound_speed_mps(220.0, 101_325.0, 0.0).unwrap();
    let expected = 980.0 / c_hot + 40.0 / (c_hot + c_cold);
    let actual = PreparedAtmosphere::new(&p)
        .unwrap()
        .travel_time([A + 1000.0, 0.0, 0.0], [A, 0.0, 0.0], UTC)
        .unwrap();
    close(actual.travel_time_s, expected, 2e-7);
    assert!(actual.travel_time_s - 1000.0 / c_hot > 0.003);
}

#[test]
fn source_inverse_rejects_unresolved_sharp_atmospheric_gradients() {
    let mut p = profile();
    p.levels = vec![
        level(-100.0, 290.0, 101_325.0, 0.0, 0.0, 0.0),
        level(1000.0, 290.0, 101_325.0, 0.0, 0.0, 0.0),
        level(1001.0, 220.0, 101_325.0, 0.0, 0.0, 0.0),
        level(5000.0, 220.0, 101_325.0, 0.0, 0.0, 0.0),
    ];
    let atmosphere = PreparedAtmosphere::new(&p).unwrap();
    let trajectory = track();
    let error =
        PreparedAtmosphericTrack::new(&trajectory, [A + 100.0, 0.0, 0.0], UTC, 10.0, &atmosphere)
            .err()
            .unwrap();
    assert!(error.contains("monotone arrivals"), "{error}");
}

#[test]
fn high_latitude_path_cannot_leave_coverage_between_quadrature_nodes() {
    let mut p = profile();
    p.bounds = [69.99, -0.51, 70.000_871_355_152_31, 0.51];
    p.integration.max_step_m = 1000.0;
    let source = geodetic_to_ecef(70.0, -0.5, 100.0).unwrap();
    let receiver = geodetic_to_ecef(70.000_321, 0.5, 4000.0).unwrap();
    // Independently solved WGS84 latitude maximum: 70.00087139711869 at
    // path fraction ~0.55703048. It is distinct from the height minimum.
    // The initial 40/80-interval quadrature nodes all remain below this
    // deliberately tight north bound; endpoint-only/node-only tests miss it.
    let atmosphere = PreparedAtmosphere::new(&p).unwrap();
    let error = atmosphere.travel_time(source, receiver, UTC).unwrap_err();
    assert!(error.contains("geographic coverage"), "{error}");
    p.bounds[2] = 70.001;
    let accepted = PreparedAtmosphere::new(&p)
        .unwrap()
        .travel_time(source, receiver, UTC)
        .unwrap();
    let length = ((receiver[0] - source[0]).powi(2)
        + (receiver[1] - source[1]).powi(2)
        + (receiver[2] - source[2]).powi(2))
    .sqrt();
    close(
        accepted.travel_time_s,
        length / humid_sound_speed_mps(293.15, 101_325.0, 0.0).unwrap(),
        1e-9,
    );
}

#[test]
fn geographic_check_handles_southern_extrema_and_equator_crossing() {
    let mut p = profile();
    p.bounds = [-70.000_871_355_152_31, -0.51, -69.99, 0.51];
    let source = geodetic_to_ecef(-70.0, -0.5, 100.0).unwrap();
    let receiver = geodetic_to_ecef(-70.000_321, 0.5, 4000.0).unwrap();
    assert!(
        PreparedAtmosphere::new(&p)
            .unwrap()
            .travel_time(source, receiver, UTC)
            .unwrap_err()
            .contains("geographic coverage")
    );
    let p = profile();
    let source = geodetic_to_ecef(-0.1, -0.1, 1000.0).unwrap();
    let receiver = geodetic_to_ecef(0.1, 0.1, 2000.0).unwrap();
    assert!(
        PreparedAtmosphere::new(&p)
            .unwrap()
            .travel_time(source, receiver, UTC)
            .is_ok()
    );
}
