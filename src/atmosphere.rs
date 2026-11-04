//! Optional, bounded stratified-atmosphere propagation along a straight ECEF path.
//!
//! Temperature, pressure, water vapour and horizontal wind vary with ellipsoid
//! height. The profile is horizontally homogeneous and frozen over its declared
//! UTC interval. This is NOT a refracting ray tracer or a weather-data importer.
//! See docs/ATMOSPHERE.md for equations, sources and applicability limits.

use crate::physics::{ClosestApproach, ModelConfig, ModelResult, PreparedTrack, Track};
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read, path::Path};

const MAX_PROFILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PATH_M: f64 = 100_000.0;
const GAS_CONSTANT: f64 = 8.314_462_618_153_24;
const MOLAR_MASS_DRY: f64 = 0.028_965_46;
const MOLAR_MASS_WATER: f64 = 0.018_015_28;
const CP_DRY: f64 = 29.12;
const CP_WATER: f64 = 33.58;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtmosphereHeightDatum {
    Wgs84Ellipsoid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumidityReference {
    LiquidWater,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtmosphereProvenance {
    pub provider: String,
    pub source: String,
    /// Hash of the provider/source input, not a claim of provider authentication.
    pub source_sha256: String,
    pub notes: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtmosphereLevel {
    pub ellipsoid_height_m: f64,
    pub temperature_k: f64,
    pub pressure_pa: f64,
    /// Fraction, not percent. Explicitly relative to liquid water even below 0 C.
    pub relative_humidity_fraction: f64,
    /// Components toward east/north, not the meteorological "from" direction.
    pub eastward_wind_mps: f64,
    pub northward_wind_mps: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationSettings {
    pub max_step_m: f64,
    pub absolute_tolerance_s: f64,
    pub max_segments: usize,
}

impl Default for IntegrationSettings {
    fn default() -> Self {
        Self {
            max_step_m: 250.0,
            absolute_tolerance_s: 1e-7,
            max_segments: 4096,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtmosphereProfile {
    pub schema_version: u32,
    pub name: String,
    pub provenance: AtmosphereProvenance,
    pub valid_from_utc_s: f64,
    pub valid_to_utc_s: f64,
    /// South, west, north, east; date-line crossing is not supported.
    pub bounds: [f64; 4],
    pub height_datum: AtmosphereHeightDatum,
    pub humidity_reference: HumidityReference,
    pub levels: Vec<AtmosphereLevel>,
    #[serde(default)]
    pub integration: IntegrationSettings,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PropagationResult {
    pub travel_time_s: f64,
    pub path_length_m: f64,
    pub effective_speed_mps: f64,
    /// Successive-grid Simpson estimate, not a guaranteed physical error bound.
    pub integration_error_s: f64,
    pub segments: usize,
}

impl AtmosphereProfile {
    pub fn validate(&self) -> ModelResult<()> {
        let [south, west, north, east] = self.bounds;
        if self.schema_version != 1
            || self.name.trim().is_empty()
            || self.name.len() > 200
            || self.provenance.provider.trim().is_empty()
            || self.provenance.source.trim().is_empty()
            || self.provenance.provider.len() > 1000
            || self.provenance.source.len() > 4000
            || self.provenance.notes.len() > 16_000
            || self.provenance.source_sha256.len() != 64
            || !self
                .provenance
                .source_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err("Atmosphere profile needs schema 1, a name and bounded source provenance with SHA-256".into());
        }
        if !self.valid_from_utc_s.is_finite()
            || !self.valid_to_utc_s.is_finite()
            || self.valid_from_utc_s < 0.0
            || self.valid_from_utc_s >= self.valid_to_utc_s
            || self.valid_to_utc_s >= 253_402_300_800.0
            || self.valid_to_utc_s - self.valid_from_utc_s > 86_400.0
        {
            return Err(
                "A frozen atmosphere profile needs an explicit UTC interval of at most 24 hours"
                    .into(),
            );
        }
        if !self.bounds.iter().all(|v| v.is_finite())
            || south < -85.0
            || north > 85.0
            || south >= north
            || west < -180.0
            || east > 180.0
            || west >= east
            || north - south > 5.0
            || east - west > 5.0
        {
            return Err(
                "Atmosphere bounds must be a non-wrapping local box at most 5 degrees per side"
                    .into(),
            );
        }
        let settings = self.integration;
        if !settings.max_step_m.is_finite()
            || !(10.0..=1000.0).contains(&settings.max_step_m)
            || !settings.absolute_tolerance_s.is_finite()
            || !(1e-9..=1e-5).contains(&settings.absolute_tolerance_s)
            || !(32..=4096).contains(&settings.max_segments)
            || !settings.max_segments.is_multiple_of(2)
        {
            return Err("Invalid atmosphere integration bounds (step 10..1000 m, tolerance 1e-9..1e-5 s, even 32..4096 segments)".into());
        }
        if !(2..=256).contains(&self.levels.len()) {
            return Err("Atmosphere profile needs 2..256 ordered height levels".into());
        }
        for (index, level) in self.levels.iter().enumerate() {
            validate_level(level)?;
            if index > 0
                && level.ellipsoid_height_m - self.levels[index - 1].ellipsoid_height_m < 1.0
            {
                return Err("Atmosphere heights must increase by at least 1 m; no implicit sorting or merging".into());
            }
        }
        Ok(())
    }
}

pub fn load_profile(path: &Path) -> ModelResult<AtmosphereProfile> {
    let file = File::open(path).map_err(|e| format!("Read atmosphere profile: {e}"))?;
    let mut data = Vec::new();
    file.take(MAX_PROFILE_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > MAX_PROFILE_BYTES {
        return Err("Atmosphere profile exceeds 4 MiB".into());
    }
    let profile: AtmosphereProfile = serde_json::from_slice(&data)
        .map_err(|e| format!("Invalid atmosphere profile JSON: {e}"))?;
    profile.validate()?;
    Ok(profile)
}

fn validate_level(level: &AtmosphereLevel) -> ModelResult<()> {
    if !level.ellipsoid_height_m.is_finite()
        || !(-1000.0..=20_000.0).contains(&level.ellipsoid_height_m)
        || !level.temperature_k.is_finite()
        || !(200.0..=330.0).contains(&level.temperature_k)
        || !level.pressure_pa.is_finite()
        || !(5000.0..=110_000.0).contains(&level.pressure_pa)
        || !level.relative_humidity_fraction.is_finite()
        || !(0.0..=1.0).contains(&level.relative_humidity_fraction)
        || !level.eastward_wind_mps.is_finite()
        || !level.northward_wind_mps.is_finite()
        || level.eastward_wind_mps.hypot(level.northward_wind_mps) > 100.0
    {
        return Err("Atmosphere level exceeds supported height, temperature (200..330 K), pressure (5000..110000 Pa), RH (0..1) or horizontal wind (<=100 m/s) range".into());
    }
    humid_sound_speed_mps(
        level.temperature_k,
        level.pressure_pa,
        level.relative_humidity_fraction,
    )?;
    Ok(())
}

/// Ideal, calorically perfect dry-air/water-vapour mixture. Pressure enters via
/// vapour mole fraction. Real-gas and molecular-relaxation corrections are absent.
pub fn humid_sound_speed_mps(
    temperature_k: f64,
    pressure_pa: f64,
    relative_humidity_fraction: f64,
) -> ModelResult<f64> {
    if !temperature_k.is_finite()
        || !(200.0..=330.0).contains(&temperature_k)
        || !pressure_pa.is_finite()
        || !(5000.0..=110_000.0).contains(&pressure_pa)
        || !relative_humidity_fraction.is_finite()
        || !(0.0..=1.0).contains(&relative_humidity_fraction)
    {
        return Err("Humid-air thermodynamic inputs are outside supported bounds".into());
    }
    let mole_fraction =
        relative_humidity_fraction * saturation_pressure_pa(temperature_k) / pressure_pa;
    if !mole_fraction.is_finite() || !(0.0..=0.1).contains(&mole_fraction) {
        return Err("Water vapour exceeds the dilute-air mixture limit of 10 mole percent".into());
    }
    let cp = CP_DRY * (1.0 - mole_fraction) + CP_WATER * mole_fraction;
    let gamma = cp / (cp - GAS_CONSTANT);
    let molar_mass = MOLAR_MASS_DRY * (1.0 - mole_fraction) + MOLAR_MASS_WATER * mole_fraction;
    Ok((gamma * GAS_CONSTANT * temperature_k / molar_mass).sqrt())
}

fn saturation_pressure_pa(t: f64) -> f64 {
    // Murphy & Koop (2005), liquid-water saturation pressure, Eq. 10; Pa/K.
    (54.842_763 - 6763.22 / t - 4.210 * t.ln()
        + 0.000_367 * t
        + (0.0415 * (t - 218.8)).tanh()
            * (53.878 - 1331.22 / t - 9.445_23 * t.ln() + 0.014_025 * t))
        .exp()
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: [f64; 3]) -> f64 {
    a[0].hypot(a[1]).hypot(a[2])
}
fn advance(a: [f64; 3], v: [f64; 3], t: f64) -> [f64; 3] {
    [a[0] + v[0] * t, a[1] + v[1] * t, a[2] + v[2] * t]
}

/// WGS84 inverse restricted to the local atmosphere domain (away from poles).
pub fn ecef_to_geodetic(xyz: [f64; 3]) -> ModelResult<[f64; 3]> {
    if !xyz.iter().all(|v| v.is_finite()) {
        return Err("Atmospheric coordinates must be finite ECEF metres".into());
    }
    let [x, y, z] = xyz;
    let p = x.hypot(y);
    if p < 100_000.0 || !(6_300_000.0..=6_410_000.0).contains(&norm(xyz)) {
        return Err(
            "Atmospheric coordinates are outside the supported Earth/altitude domain".into(),
        );
    }
    let a = 6_378_137.0;
    let f = 1.0 / 298.257_223_563;
    let e2 = f * (2.0 - f);
    let mut lat = z.atan2(p * (1.0 - e2));
    for _ in 0..8 {
        let radius = a / (1.0 - e2 * lat.sin().powi(2)).sqrt();
        lat = (z + e2 * radius * lat.sin()).atan2(p);
    }
    let radius = a / (1.0 - e2 * lat.sin().powi(2)).sqrt();
    Ok([
        lat.to_degrees(),
        y.atan2(x).to_degrees(),
        p / lat.cos() - radius,
    ])
}

pub struct PreparedAtmosphere<'a> {
    profile: &'a AtmosphereProfile,
    minimum_speed_mps: f64,
    maximum_wind_mps: f64,
    speed_gradient_bound: f64,
}

impl<'a> PreparedAtmosphere<'a> {
    pub fn new(profile: &'a AtmosphereProfile) -> ModelResult<Self> {
        profile.validate()?;
        let min_temperature = profile
            .levels
            .iter()
            .map(|l| l.temperature_k)
            .fold(f64::INFINITY, f64::min);
        let max_wind = profile
            .levels
            .iter()
            .map(|l| l.eastward_wind_mps.hypot(l.northward_wind_mps))
            .fold(0.0, f64::max);
        // A conservative subsonic bound across the complete declared profile.
        let minimum_speed_mps = humid_sound_speed_mps(min_temperature, 101_325.0, 0.0)? - max_wind;
        let mut speed_gradient_bound = 0.0_f64;
        for pair in profile.levels.windows(2) {
            let [low, high] = [&pair[0], &pair[1]];
            let dh = high.ellipsoid_height_m - low.ellipsoid_height_m;
            let dt = (high.temperature_k - low.temperature_k).abs() / dh;
            let drh = (high.relative_humidity_fraction - low.relative_humidity_fraction).abs() / dh;
            let dlnp = (high.pressure_pa.ln() - low.pressure_pa.ln()).abs() / dh;
            // Conservative derivatives across the supported dilute ideal-mixture
            // domain: |dc/dT|<1, |dc/dx|<80, |d ln(e_sat)/dT|<0.35.
            let max_es_over_p = saturation_pressure_pa(low.temperature_k.max(high.temperature_k))
                / low.pressure_pa.min(high.pressure_pa);
            let dx = max_es_over_p
                * (drh
                    + low
                        .relative_humidity_fraction
                        .max(high.relative_humidity_fraction)
                        * (0.35 * dt + dlnp));
            let wind_gradient = (high.eastward_wind_mps - low.eastward_wind_mps)
                .hypot(high.northward_wind_mps - low.northward_wind_mps)
                / dh;
            speed_gradient_bound = speed_gradient_bound.max(dt + 80.0 * dx + wind_gradient);
        }
        let maximum_latitude = profile.bounds[0]
            .abs()
            .max(profile.bounds[2].abs())
            .to_radians();
        // Rotation of local horizontal wind axes along the ECEF path.
        speed_gradient_bound += 2.0 * max_wind / (6_300_000.0 * maximum_latitude.cos());
        Ok(Self {
            profile,
            minimum_speed_mps,
            maximum_wind_mps: max_wind,
            speed_gradient_bound,
        })
    }

    pub fn profile(&self) -> &AtmosphereProfile {
        self.profile
    }

    pub fn sample_at_height(&self, height: f64) -> ModelResult<AtmosphereLevel> {
        let levels = &self.profile.levels;
        if !height.is_finite()
            || height < levels[0].ellipsoid_height_m - 1e-6
            || height > levels[levels.len() - 1].ellipsoid_height_m + 1e-6
        {
            return Err(format!(
                "Atmosphere has no height coverage at {height:.3} m ellipsoid; extrapolation is disabled"
            ));
        }
        let index = levels
            .partition_point(|l| l.ellipsoid_height_m <= height)
            .saturating_sub(1)
            .min(levels.len() - 2);
        let (low, high) = (levels[index], levels[index + 1]);
        let fraction = ((height - low.ellipsoid_height_m)
            / (high.ellipsoid_height_m - low.ellipsoid_height_m))
            .clamp(0.0, 1.0);
        let lerp = |a: f64, b: f64| a + (b - a) * fraction;
        Ok(AtmosphereLevel {
            ellipsoid_height_m: height,
            temperature_k: lerp(low.temperature_k, high.temperature_k),
            // Pressure is positive and interpolated logarithmically.
            pressure_pa: lerp(low.pressure_pa.ln(), high.pressure_pa.ln()).exp(),
            relative_humidity_fraction: lerp(
                low.relative_humidity_fraction,
                high.relative_humidity_fraction,
            ),
            eastward_wind_mps: lerp(low.eastward_wind_mps, high.eastward_wind_mps),
            northward_wind_mps: lerp(low.northward_wind_mps, high.northward_wind_mps),
        })
    }

    fn check_time(&self, utc: f64) -> ModelResult<()> {
        if !utc.is_finite()
            || utc < self.profile.valid_from_utc_s
            || utc > self.profile.valid_to_utc_s
        {
            return Err(
                "Atmosphere UTC coverage is missing; temporal extrapolation is disabled".into(),
            );
        }
        Ok(())
    }

    fn check_geographic_path(&self, source: [f64; 3], displacement: [f64; 3]) -> ModelResult<()> {
        let sample = |fraction: f64| -> ModelResult<(f64, f64)> {
            let [latitude, longitude, _] =
                ecef_to_geodetic(advance(source, displacement, fraction))?;
            let [south, west, north, east] = self.profile.bounds;
            if latitude < south || latitude > north || longitude < west || longitude > east {
                return Err(
                    "Sound path leaves the atmosphere's declared geographic coverage".into(),
                );
            }
            let (lat, lon) = (latitude.to_radians(), longitude.to_radians());
            let north_axis = [-lat.sin() * lon.cos(), -lat.sin() * lon.sin(), lat.cos()];
            Ok((latitude, dot(north_axis, displacement)))
        };
        sample(0.0)?;
        sample(1.0)?;
        let mut divisions = vec![0.0, 1.0];
        // Split hemispheres explicitly rather than assuming one latitude
        // extremum on an arbitrary global path. Geodetic latitude is zero iff
        // ECEF z is zero, so this split is analytic for a straight chord.
        if displacement[2] != 0.0 {
            let equator = -source[2] / displacement[2];
            if equator > 0.0 && equator < 1.0 {
                divisions.insert(1, equator);
            }
        }
        for interval in divisions.windows(2) {
            let (mut low, mut high) = (interval[0], interval[1]);
            let (_, left) = sample(low)?;
            let (_, right) = sample(high)?;
            if left * right < 0.0 {
                // Latitude derivative has the sign of local_north dot d.
                // Within one hemisphere every nondegenerate zero has the
                // same crossing direction: writing p=hypot(x,y), the slope
                // derivative at a latitude extremum has sign -p''*sin(lat),
                // and p''>=0. There is therefore at most one such root per
                // hemisphere. The radial/constant-latitude degeneracy is
                // already covered by the endpoints. Longitude is monotone
                // on the accepted short non-wrapping chord (x*y'-y*x' is
                // constant, and the path stays away from the polar axis).
                for _ in 0..44 {
                    let middle = (low + high) * 0.5;
                    let (_, slope) = sample(middle)?;
                    if slope == 0.0 {
                        low = middle;
                        high = middle;
                        break;
                    }
                    if slope.is_sign_negative() == left.is_sign_negative() {
                        low = middle;
                    } else {
                        high = middle;
                    }
                }
                sample((low + high) * 0.5)?;
            }
        }
        Ok(())
    }

    fn slowness(&self, point: [f64; 3], direction: [f64; 3]) -> ModelResult<f64> {
        let [latitude, longitude, height] = ecef_to_geodetic(point)?;
        let [south, west, north, east] = self.profile.bounds;
        if latitude < south || latitude > north || longitude < west || longitude > east {
            return Err("Sound path leaves the atmosphere's declared geographic coverage".into());
        }
        let state = self.sample_at_height(height)?;
        let (lat, lon) = (latitude.to_radians(), longitude.to_radians());
        let east_axis = [-lon.sin(), lon.cos(), 0.0];
        let north_axis = [-lat.sin() * lon.cos(), -lat.sin() * lon.sin(), lat.cos()];
        let wind = state.eastward_wind_mps * dot(east_axis, direction)
            + state.northward_wind_mps * dot(north_axis, direction);
        let c = humid_sound_speed_mps(
            state.temperature_k,
            state.pressure_pa,
            state.relative_humidity_fraction,
        )?;
        let effective = c + wind;
        if !effective.is_finite() || effective < 150.0 {
            return Err("Atmospheric effective speed is too small or invalid".into());
        }
        Ok(1.0 / effective)
    }

    fn integrate(
        &self,
        source: [f64; 3],
        displacement: [f64; 3],
        length: f64,
        segments: usize,
        interval: [f64; 2],
    ) -> ModelResult<f64> {
        let direction = displacement.map(|v| v / length);
        let mut weighted = 0.0;
        for i in 0..=segments {
            let weight = if i == 0 || i == segments {
                1.0
            } else if i % 2 == 0 {
                2.0
            } else {
                4.0
            };
            let fraction = interval[0] + (interval[1] - interval[0]) * i as f64 / segments as f64;
            weighted +=
                weight * self.slowness(advance(source, displacement, fraction), direction)?;
        }
        Ok(length * (interval[1] - interval[0]) * weighted / (3.0 * segments as f64))
    }

    fn path_intervals(
        &self,
        source: [f64; 3],
        displacement: [f64; 3],
    ) -> ModelResult<Vec<[f64; 2]>> {
        let height_and_slope = |fraction: f64| -> ModelResult<(f64, f64)> {
            let [lat, lon, height] = ecef_to_geodetic(advance(source, displacement, fraction))?;
            let (lat, lon) = (lat.to_radians(), lon.to_radians());
            Ok((
                height,
                dot(
                    [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()],
                    displacement,
                ),
            ))
        };
        let (start_height, start_slope) = height_and_slope(0.0)?;
        let (end_height, end_slope) = height_and_slope(1.0)?;
        // Signed distance to the convex WGS84 ellipsoid along a straight line
        // has a single height minimum in this near-surface domain. Split at
        // that minimum and every profile-height crossing, so thin layers cannot
        // hide between uniform quadrature samples.
        let minimum_fraction = if start_slope >= 0.0 {
            0.0
        } else if end_slope <= 0.0 {
            1.0
        } else {
            let (mut low, mut high) = (0.0, 1.0);
            for _ in 0..44 {
                let middle = (low + high) * 0.5;
                if height_and_slope(middle)?.1 < 0.0 {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            (low + high) * 0.5
        };
        let minimum_height = height_and_slope(minimum_fraction)?.0;
        // Explicitly inspect the actual height extremum, not only quadrature nodes.
        self.sample_at_height(minimum_height)?;
        self.sample_at_height(start_height)?;
        self.sample_at_height(end_height)?;
        let mut cuts = vec![0.0, minimum_fraction, 1.0];
        for (begin, end, begin_h, end_h) in [
            (0.0, minimum_fraction, start_height, minimum_height),
            (minimum_fraction, 1.0, minimum_height, end_height),
        ] {
            if end - begin <= 1e-13 {
                continue;
            }
            for level in &self.profile.levels {
                let target = level.ellipsoid_height_m;
                if target <= begin_h.min(end_h) || target >= begin_h.max(end_h) {
                    continue;
                }
                let (mut low, mut high) = (begin, end);
                let (mut f_low, mut f_high) = (begin_h - target, end_h - target);
                let mut crossing = None;
                for _ in 0..48 {
                    let secant = low - f_low * (high - low) / (f_high - f_low);
                    let fraction =
                        secant.clamp(low + (high - low) * 0.02, high - (high - low) * 0.02);
                    let value = height_and_slope(fraction)?.0 - target;
                    if value.abs() <= 1e-6 || high - low <= 1e-13 {
                        crossing = Some(fraction);
                        break;
                    }
                    if value.is_sign_negative() == f_low.is_sign_negative() {
                        low = fraction;
                        f_low = value;
                    } else {
                        high = fraction;
                        f_high = value;
                    }
                }
                cuts.push(crossing.ok_or("Atmosphere layer crossing could not be resolved")?);
            }
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-13);
        Ok(cuts.windows(2).map(|w| [w[0], w[1]]).collect())
    }

    pub fn travel_time(
        &self,
        source: [f64; 3],
        receiver: [f64; 3],
        emission_utc_s: f64,
    ) -> ModelResult<PropagationResult> {
        self.check_time(emission_utc_s)?;
        let displacement = sub(receiver, source);
        let length = norm(displacement);
        if !length.is_finite() || !(0.01..=MAX_PATH_M).contains(&length) {
            return Err("Atmospheric path length must be 0.01..100000 m".into());
        }
        let settings = self.profile.integration;
        self.check_geographic_path(source, displacement)?;
        let intervals = self.path_intervals(source, displacement)?;
        let mut segments: Vec<usize> = intervals
            .iter()
            .map(|v| {
                let count = ((length * (v[1] - v[0]) / settings.max_step_m).ceil() as usize).max(2);
                count + count % 2
            })
            .collect();
        let count = segments.iter().sum::<usize>();
        if count < 8 {
            segments[0] += 8 - count;
        }
        if segments.iter().sum::<usize>() * 2 > settings.max_segments {
            return Err("Atmosphere path needs more than the bounded integration budget".into());
        }
        let integrate = |counts: &[usize]| {
            intervals
                .iter()
                .zip(counts)
                .map(|(&interval, &count)| {
                    self.integrate(source, displacement, length, count, interval)
                })
                .collect::<ModelResult<Vec<_>>>()
        };
        let mut previous = integrate(&segments)?;
        loop {
            for count in &mut segments {
                *count *= 2;
            }
            let current_parts = integrate(&segments)?;
            let current = current_parts.iter().sum::<f64>();
            // Sum absolute local estimates so layer errors cannot cancel.
            let error = current_parts
                .iter()
                .zip(&previous)
                .map(|(a, b)| (a - b).abs() / 15.0)
                .sum::<f64>();
            if error <= settings.absolute_tolerance_s {
                self.check_time(emission_utc_s + current)?;
                return Ok(PropagationResult {
                    travel_time_s: current,
                    path_length_m: length,
                    effective_speed_mps: length / current,
                    integration_error_s: error,
                    segments: segments.iter().sum(),
                });
            }
            if segments.iter().sum::<usize>() * 2 > settings.max_segments {
                return Err(format!(
                    "Atmosphere travel-time quadrature did not converge within {} segments",
                    settings.max_segments
                ));
            }
            previous = current_parts;
        }
    }

    fn check_motion_bound(&self, path_length: f64, speed: f64) -> ModelResult<()> {
        let c = self.minimum_speed_mps;
        // For tau=L*integral(q du), changing source position changes length,
        // sampled medium, and wind projection. Bound |grad_source(tau)| using
        // q<=1/c, |grad q|<=G/c^2 and |d q/d direction|<=max_wind/c^2.
        let derivative_bound = speed
            * (1.0 / c
                + (path_length * self.speed_gradient_bound * 0.5 + self.maximum_wind_mps)
                    / (c * c));
        if !derivative_bound.is_finite() || derivative_bound >= 0.95 {
            return Err("Atmospheric gradients/source speed cannot guarantee monotone arrivals within this straight-path model".into());
        }
        Ok(())
    }

    pub fn frequency_factor(
        &self,
        source: [f64; 3],
        velocity: [f64; 3],
        receiver: [f64; 3],
        emission_utc_s: f64,
    ) -> ModelResult<f64> {
        self.factor_bounded(source, velocity, receiver, emission_utc_s, 0.04, 0.04)
    }

    fn factor_bounded(
        &self,
        source: [f64; 3],
        velocity: [f64; 3],
        receiver: [f64; 3],
        emission_utc_s: f64,
        backward: f64,
        forward: f64,
    ) -> ModelResult<f64> {
        if !velocity.iter().all(|v| v.is_finite()) || norm(velocity) >= self.minimum_speed_mps {
            return Err(
                "Source motion is not subsonic under the complete atmospheric profile".into(),
            );
        }
        self.check_motion_bound(
            norm(sub(source, receiver)) + norm(velocity) * backward.max(forward),
            norm(velocity),
        )?;
        if norm(velocity) == 0.0 {
            self.travel_time(source, receiver, emission_utc_s)?;
            return Ok(1.0);
        }
        let derivative = |step: f64| -> ModelResult<f64> {
            let at = |offset: f64| {
                self.travel_time(
                    advance(source, velocity, offset),
                    receiver,
                    emission_utc_s + offset,
                )
                .map(|p| p.travel_time_s)
            };
            if backward >= step && forward >= step {
                Ok((at(step)? - at(-step)?) / (2.0 * step))
            } else if forward >= 2.0 * step {
                Ok((-3.0 * at(0.0)? + 4.0 * at(step)? - at(2.0 * step)?) / (2.0 * step))
            } else if backward >= 2.0 * step {
                Ok((3.0 * at(0.0)? - 4.0 * at(-step)? + at(-2.0 * step)?) / (2.0 * step))
            } else {
                Err(
                    "Track segment is too short for bounded atmospheric Doppler differentiation"
                        .into(),
                )
            }
        };
        let step = 0.02_f64.min(backward.max(forward) / 4.0);
        if step < 1e-4 {
            return Err("Atmospheric Doppler needs at least 0.4 ms of segment support".into());
        }
        let coarse = derivative(step)?;
        let fine = derivative(step / 2.0)?;
        if !fine.is_finite() || (fine - coarse).abs() > 2e-4 || 1.0 + fine <= 0.05 {
            return Err("Atmospheric Doppler derivative is nonmonotonic, near-sonic or numerically unresolved".into());
        }
        Ok(1.0 / (1.0 + fine))
    }
}

/// Moving-source adapter using the same arrival-time and CPA units as PreparedTrack.
/// Input is one continuous gap-checked segment; no track extrapolation is allowed.
pub struct PreparedAtmosphericTrack<'a, 'b> {
    atmosphere: &'b PreparedAtmosphere<'a>,
    track: &'b Track,
    observer: [f64; 3],
    origin_utc_s: f64,
    velocities: Vec<[f64; 3]>,
    arrivals_s: Vec<f64>,
    local_cpas: Vec<ClosestApproach>,
}

impl<'a, 'b> PreparedAtmosphericTrack<'a, 'b> {
    pub fn new(
        track: &'b Track,
        observer: [f64; 3],
        origin_utc_s: f64,
        max_gap_s: f64,
        atmosphere: &'b PreparedAtmosphere<'a>,
    ) -> ModelResult<Self> {
        let geometry = PreparedTrack::new(
            track,
            observer,
            ModelConfig {
                sound_speed_mps: atmosphere.minimum_speed_mps,
                max_gap_s,
            },
        )?;
        let mut local_cpas = geometry.local_closest_approaches().to_vec();
        let mut velocities = Vec::new();
        for pair in track.points.windows(2) {
            let dt = pair[1].t_s - pair[0].t_s;
            let velocity = sub(pair[1].xyz_m, pair[0].xyz_m).map(|v| v / dt);
            atmosphere.check_motion_bound(
                norm(sub(pair[0].xyz_m, observer)).max(norm(sub(pair[1].xyz_m, observer))),
                norm(velocity),
            )?;
            velocities.push(velocity);
        }
        let mut arrivals_s = Vec::new();
        for point in &track.points {
            let delay = atmosphere
                .travel_time(point.xyz_m, observer, origin_utc_s + point.t_s)?
                .travel_time_s;
            let arrival = point.t_s + delay;
            if arrivals_s.last().is_some_and(|&old| arrival <= old) {
                return Err("Atmospheric track arrival times are not strictly increasing".into());
            }
            arrivals_s.push(arrival);
        }
        // Check an interior point in every segment as well as the endpoints;
        // the bounded inverse below also verifies its final residual/derivative.
        for (i, pair) in track.points.windows(2).enumerate() {
            let dt = (pair[1].t_s - pair[0].t_s) / 2.0;
            let p = advance(pair[0].xyz_m, velocities[i], dt);
            let arrival = pair[0].t_s
                + dt
                + atmosphere
                    .travel_time(p, observer, origin_utc_s + pair[0].t_s + dt)?
                    .travel_time_s;
            if arrival <= arrivals_s[i] || arrival >= arrivals_s[i + 1] {
                return Err("Atmospheric segment has unresolved arrival monotonicity".into());
            }
        }
        for cpa in &mut local_cpas {
            let point = &track.points[cpa.segment_index];
            let position = advance(
                point.xyz_m,
                velocities[cpa.segment_index],
                cpa.emission_time_s - point.t_s,
            );
            cpa.arrival_time_s = cpa.emission_time_s
                + atmosphere
                    .travel_time(position, observer, origin_utc_s + cpa.emission_time_s)?
                    .travel_time_s;
        }
        Ok(Self {
            atmosphere,
            track,
            observer,
            origin_utc_s,
            velocities,
            arrivals_s,
            local_cpas,
        })
    }

    pub fn arrival_bounds_s(&self) -> [f64; 2] {
        [
            self.arrivals_s[0],
            self.arrivals_s[self.arrivals_s.len() - 1],
        ]
    }
    pub(crate) fn arrival_knots_s(&self) -> &[f64] {
        &self.arrivals_s
    }
    pub fn local_closest_approaches(&self) -> &[ClosestApproach] {
        &self.local_cpas
    }

    pub fn factor_at_arrival(&self, arrival_s: f64) -> ModelResult<f64> {
        let [first, last] = self.arrival_bounds_s();
        // Independently calculated endpoint arrivals can differ by a few ulps
        // after quadrature. Snap only that numerical boundary fuzz to the
        // endpoint; never evaluate the source outside the supplied segment.
        let endpoint_tolerance =
            (16.0 * f64::EPSILON * first.abs().max(last.abs()).max(1.0)).clamp(1e-10, 1e-6);
        if !arrival_s.is_finite()
            || arrival_s < first - endpoint_tolerance
            || arrival_s > last + endpoint_tolerance
        {
            return Err("Arrival is outside atmospheric track coverage".into());
        }
        let arrival_s = arrival_s.clamp(first, last);
        let i = self
            .arrivals_s
            .partition_point(|&a| a <= arrival_s)
            .saturating_sub(1)
            .min(self.velocities.len() - 1);
        let start = &self.track.points[i];
        let duration = self.track.points[i + 1].t_s - start.t_s;
        let target = arrival_s - start.t_s;
        let mut low = 0.0;
        let mut high = duration;
        let mut f_low = self.arrivals_s[i] - arrival_s;
        let mut f_high = self.arrivals_s[i + 1] - arrival_s;
        let residual = |offset: f64| -> ModelResult<f64> {
            Ok(offset
                + self
                    .atmosphere
                    .travel_time(
                        advance(start.xyz_m, self.velocities[i], offset),
                        self.observer,
                        self.origin_utc_s + start.t_s + offset,
                    )?
                    .travel_time_s
                - target)
        };
        let mut answer = None;
        if f_low.abs() <= 1e-7 {
            answer = Some(low);
        } else if f_high.abs() <= 1e-7 {
            answer = Some(high);
        }
        for _ in 0..48 {
            if answer.is_some() {
                break;
            }
            let secant = low - f_low * (high - low) / (f_high - f_low);
            let offset = secant.clamp(low + (high - low) * 0.02, high - (high - low) * 0.02);
            let value = residual(offset)?;
            if value.abs() <= 1e-7 || high - low <= 1e-7 {
                answer = Some(offset);
                break;
            }
            if value < 0.0 {
                low = offset;
                f_low = value;
            } else {
                high = offset;
                f_high = value;
            }
        }
        let offset = answer.ok_or("Atmospheric arrival inversion did not converge")?;
        if residual(offset)?.abs() > 2e-6 {
            return Err("Atmospheric arrival inversion residual exceeds tolerance".into());
        }
        self.atmosphere.factor_bounded(
            advance(start.xyz_m, self.velocities[i], offset),
            self.velocities[i],
            self.observer,
            self.origin_utc_s + start.t_s + offset,
            offset,
            duration - offset,
        )
    }
}
