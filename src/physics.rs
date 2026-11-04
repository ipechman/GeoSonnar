//! Propagation-aware aircraft Doppler models and shared-clock ridge scoring.
//!
//! Positions must use one common Cartesian frame in metres (ECEF or local ENU),
//! with a reconciled vertical datum. Times use one common origin in seconds.
//! The model assumes a stationary receiver, piecewise-linear subsonic motion,
//! still air, and a stable emitted tone within each pass. It does not estimate
//! terrain, correct pressure altitudes, or establish a real recording's location.

use serde::{Deserialize, Serialize};

pub type ModelResult<T> = Result<T, String>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackPoint {
    pub t_s: f64,
    pub xyz_m: [f64; 3],
}

/// One continuous, gap-checked trajectory. For a looping flight, enumerate local
/// closest approaches and use `score_pair_at_cpa`; `score_pair` is the legacy
/// single-pass API and anchors only the global closest approach.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub flight_id: String,
    pub points: Vec<TrackPoint>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    pub sound_speed_mps: f64,
    pub max_gap_s: f64,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            sound_speed_mps: 343.0,
            max_gap_s: 30.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ClosestApproach {
    pub emission_time_s: f64,
    pub arrival_time_s: f64,
    pub slant_distance_m: f64,
    pub speed_mps: f64,
    pub segment_index: usize,
    /// A minimum at the first/last track point may be a truncated pass.
    pub at_track_boundary: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TracePoint {
    pub t_rec_s: f64,
    pub frequency_hz: f64,
}

/// A measured ridge and explicit candidate emitted frequencies. These samples
/// are fixed across observers; incomplete coverage is rejected, not shortened.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventTrace {
    pub points: Vec<TracePoint>,
    pub f0_hz: Vec<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairConfig {
    pub event1_cpa_recording_s: f64,
    pub shared_shifts_s: Vec<f64>,
    /// Optional recording-start interval in the tracks' common time origin.
    /// Restricts the day/time without shifting individual events independently.
    #[serde(default)]
    pub clock_offset_bounds_s: Option<[f64; 2]>,
    #[serde(default)]
    pub model: ModelConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairScore {
    pub flight_ids: [String; 2],
    pub observer_xyz_m: [f64; 3],
    pub rmse_hz: [f64; 2],
    /// sqrt((event1_mse + event2_mse) / 2); lower is better. Equal event
    /// weights prevent denser tracing of one event from dominating the other.
    pub combined_rmse_hz: f64,
    pub fitted_f0_hz: [f64; 2],
    /// Absolute arrival time = recording time + clock_offset_s.
    pub clock_offset_s: f64,
    pub shared_shift_s: f64,
    pub cpa: [ClosestApproach; 2],
    pub predicted_event2_cpa_recording_s: f64,
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

fn advance(p: [f64; 3], velocity: [f64; 3], dt: f64) -> [f64; 3] {
    [
        p[0] + velocity[0] * dt,
        p[1] + velocity[1] * dt,
        p[2] + velocity[2] * dt,
    ]
}

fn finite_xyz(xyz: [f64; 3]) -> bool {
    xyz.iter().all(|x| x.is_finite())
}

fn validate_config(config: ModelConfig) -> ModelResult<()> {
    if !config.sound_speed_mps.is_finite() || config.sound_speed_mps <= 0.0 {
        return Err("Sound speed must be a finite positive number in m/s".into());
    }
    if !config.max_gap_s.is_finite() || config.max_gap_s <= 0.0 {
        return Err("Maximum track gap must be a finite positive number in seconds".into());
    }
    Ok(())
}

/// Convert WGS84 geodetic coordinates to Earth-centred, Earth-fixed metres.
/// Height MUST be ellipsoidal, not uncorrected barometric, NAVD88 or MSL height.
/// This conversion alone does not reconcile those vertical references.
pub fn geodetic_to_ecef(
    latitude_deg: f64,
    longitude_deg: f64,
    ellipsoid_height_m: f64,
) -> ModelResult<[f64; 3]> {
    if !latitude_deg.is_finite()
        || !(-90.0..=90.0).contains(&latitude_deg)
        || !longitude_deg.is_finite()
        || !(-180.0..=180.0).contains(&longitude_deg)
        || !ellipsoid_height_m.is_finite()
    {
        return Err("WGS84 latitude, longitude and ellipsoidal height must be finite and latitude/longitude in range".into());
    }
    let semi_major = 6_378_137.0_f64;
    let flattening = 1.0 / 298.257_223_563;
    let eccentricity_sq = flattening * (2.0 - flattening);
    let lat = latitude_deg.to_radians();
    let lon = longitude_deg.to_radians();
    let radius = semi_major / (1.0 - eccentricity_sq * lat.sin().powi(2)).sqrt();
    let xyz = [
        (radius + ellipsoid_height_m) * lat.cos() * lon.cos(),
        (radius + ellipsoid_height_m) * lat.cos() * lon.sin(),
        (radius * (1.0 - eccentricity_sq) + ellipsoid_height_m) * lat.sin(),
    ];
    if !finite_xyz(xyz) {
        return Err("Coordinate conversion overflowed".into());
    }
    Ok(xyz)
}

/// An observer-specific validated trajectory. Arrival times are monotonically
/// increasing because every segment's speed is strictly less than sound speed.
/// Reuse this for many frequency queries without revalidating the track.
pub struct PreparedTrack<'a> {
    track: &'a Track,
    observer: [f64; 3],
    config: ModelConfig,
    velocities: Vec<[f64; 3]>,
    arrivals_s: Vec<f64>,
    cpa: ClosestApproach,
    local_cpas: Vec<ClosestApproach>,
}

impl<'a> PreparedTrack<'a> {
    pub fn new(track: &'a Track, observer: [f64; 3], config: ModelConfig) -> ModelResult<Self> {
        validate_config(config)?;
        if !finite_xyz(observer) {
            return Err("Observer coordinates must be finite metres".into());
        }
        if track.flight_id.trim().is_empty() {
            return Err("A track must have a nonempty flight ID".into());
        }
        if track.points.len() < 2 {
            return Err(format!(
                "Track {} requires at least two positions",
                track.flight_id
            ));
        }
        if track
            .points
            .iter()
            .any(|p| !p.t_s.is_finite() || !finite_xyz(p.xyz_m))
        {
            return Err(format!(
                "Track {} contains nonfinite time or coordinates",
                track.flight_id
            ));
        }
        let mut velocities = Vec::with_capacity(track.points.len() - 1);
        let mut arrivals_s = Vec::with_capacity(track.points.len());
        let mut best: Option<ClosestApproach> = None;
        for (i, pair) in track.points.windows(2).enumerate() {
            let dt = pair[1].t_s - pair[0].t_s;
            if !dt.is_finite() || dt <= 0.0 {
                return Err(format!(
                    "Track {} timestamps must be strictly increasing (segment {i})",
                    track.flight_id
                ));
            }
            if dt > config.max_gap_s {
                return Err(format!(
                    "Track {} gap of {dt:.3} s exceeds {:.3} s at segment {i}; split or obtain better data",
                    track.flight_id, config.max_gap_s
                ));
            }
            let displacement = sub(pair[1].xyz_m, pair[0].xyz_m);
            let velocity = displacement.map(|v| v / dt);
            let speed = norm(velocity);
            if !speed.is_finite() || speed >= config.sound_speed_mps {
                return Err(format!(
                    "Track {} segment {i} is not subsonic; inspect units, timestamps and gaps",
                    track.flight_id
                ));
            }
            velocities.push(velocity);
            // A stationary segment has no unique interior CPA; its first point
            // explicitly represents the tied minimum.
            let duration_to_cpa = if speed > 0.0 {
                (dot(sub(observer, pair[0].xyz_m), velocity) / speed.powi(2)).clamp(0.0, dt)
            } else {
                0.0
            };
            let position = advance(pair[0].xyz_m, velocity, duration_to_cpa);
            let distance = norm(sub(position, observer));
            if !distance.is_finite() || distance <= 1.0e-6 {
                return Err(format!(
                    "Track {} passes through the observer or has invalid range",
                    track.flight_id
                ));
            }
            let emission_time_s = pair[0].t_s + duration_to_cpa;
            let arrival_time_s = emission_time_s + distance / config.sound_speed_mps;
            if !arrival_time_s.is_finite() {
                return Err("Propagation time overflowed".into());
            }
            if best.as_ref().is_none_or(|c| distance < c.slant_distance_m) {
                best = Some(ClosestApproach {
                    emission_time_s,
                    arrival_time_s,
                    slant_distance_m: distance,
                    speed_mps: speed,
                    segment_index: i,
                    at_track_boundary: (i == 0 && duration_to_cpa == 0.0)
                        || (i == track.points.len() - 2 && duration_to_cpa == dt),
                });
            }
        }
        for p in &track.points {
            let arrival = p.t_s + norm(sub(p.xyz_m, observer)) / config.sound_speed_mps;
            if !arrival.is_finite()
                || arrivals_s
                    .last()
                    .is_some_and(|previous| arrival <= *previous)
            {
                return Err(format!(
                    "Track {} arrival times are not finite and strictly increasing",
                    track.flight_id
                ));
            }
            arrivals_s.push(arrival);
        }
        let local_cpas = enumerate_local_cpas(track, observer, config, &velocities);
        Ok(Self {
            track,
            observer,
            config,
            velocities,
            arrivals_s,
            cpa: best.expect("at least one segment was checked"),
            local_cpas,
        })
    }

    pub fn closest_approach(&self) -> ClosestApproach {
        self.cpa
    }

    /// All local range minima, ordered by emission time. Constrained minima at
    /// the ends are retained but flagged so truncated passes cannot be scored.
    /// A stationary minimum plateau has one representative at its first point;
    /// such a plateau has no uniquely identifiable geometric CPA time.
    pub fn local_closest_approaches(&self) -> &[ClosestApproach] {
        &self.local_cpas
    }

    pub fn arrival_bounds_s(&self) -> [f64; 2] {
        [
            self.arrivals_s[0],
            self.arrivals_s[self.arrivals_s.len() - 1],
        ]
    }

    /// Native interpolation boundaries, for diagnostic rendering without joining
    /// velocity discontinuities. Values use this track's unchanged time origin.
    pub(crate) fn arrival_knots_s(&self) -> &[f64] {
        &self.arrivals_s
    }

    /// Solve t_arrival = t_emission + range(t_emission)/c inside the correct
    /// segment, then evaluate its velocity. No constant-delay approximation,
    /// interpolation across gaps, sorting invalid tracks, or extrapolation.
    /// At a velocity-discontinuous knot the outgoing segment is used, except
    /// at the final endpoint where only the incoming segment exists.
    pub fn factor_at_arrival(&self, arrival_s: f64) -> ModelResult<f64> {
        let [first, last] = self.arrival_bounds_s();
        if !arrival_s.is_finite() || arrival_s < first || arrival_s > last {
            return Err(format!(
                "Arrival {arrival_s} is outside track {} coverage [{first}, {last}]",
                self.track.flight_id
            ));
        }
        let i = self
            .arrivals_s
            .partition_point(|&a| a <= arrival_s)
            .saturating_sub(1)
            .min(self.velocities.len() - 1);
        let start = &self.track.points[i];
        let dt = self.track.points[i + 1].t_s - start.t_s;
        let velocity = self.velocities[i];
        let target = arrival_s - start.t_s;
        let mut low = 0.0;
        let mut high = dt;
        // Use segment-relative seconds so Unix-epoch inputs do not repeatedly
        // lose precision during root refinement. Strictly subsonic motion
        // makes this a monotonic scalar equation.
        for _ in 0..60 {
            let mid = (low + high) * 0.5;
            let range = norm(sub(advance(start.xyz_m, velocity, mid), self.observer));
            if mid + range / self.config.sound_speed_mps < target {
                low = mid;
            } else {
                high = mid;
            }
        }
        let emission_offset = (low + high) * 0.5;
        let relative = sub(
            advance(start.xyz_m, velocity, emission_offset),
            self.observer,
        );
        let range = norm(relative);
        let radial = dot(velocity, relative.map(|r| r / range));
        let factor = self.config.sound_speed_mps / (self.config.sound_speed_mps + radial);
        if !factor.is_finite() || factor <= 0.0 {
            return Err("Invalid Doppler factor".into());
        }
        Ok(factor)
    }
}

// Range squared is convex within each linear segment. Its interior minimum is
// analytic; a knot is a minimum only when neither adjacent side descends away
// from it. Checking both sides avoids manufacturing a CPA at every segment end.
fn enumerate_local_cpas(
    track: &Track,
    observer: [f64; 3],
    config: ModelConfig,
    velocities: &[[f64; 3]],
) -> Vec<ClosestApproach> {
    let mut minima = Vec::new();
    let make = |segment_index: usize, emission_time_s: f64, position: [f64; 3], boundary: bool| {
        let distance = norm(sub(position, observer));
        ClosestApproach {
            emission_time_s,
            arrival_time_s: emission_time_s + distance / config.sound_speed_mps,
            slant_distance_m: distance,
            speed_mps: norm(velocities[segment_index]),
            segment_index,
            at_track_boundary: boundary,
        }
    };
    for (i, pair) in track.points.windows(2).enumerate() {
        let velocity = velocities[i];
        let speed_squared = dot(velocity, velocity);
        if speed_squared == 0.0 {
            continue;
        }
        let offset = -dot(sub(pair[0].xyz_m, observer), velocity) / speed_squared;
        if offset > 0.0 && offset < pair[1].t_s - pair[0].t_s {
            minima.push(make(
                i,
                pair[0].t_s + offset,
                advance(pair[0].xyz_m, velocity, offset),
                false,
            ));
        }
    }
    let last = track.points.len() - 1;
    let mut first_equal = 0;
    while first_equal <= last {
        let mut last_equal = first_equal;
        while last_equal < last
            && track.points[last_equal + 1].xyz_m == track.points[first_equal].xyz_m
        {
            last_equal += 1;
        }
        let point = &track.points[first_equal];
        let relative = sub(point.xyz_m, observer);
        let left_descends_to_knot =
            first_equal == 0 || dot(relative, velocities[first_equal - 1]) <= 0.0;
        let right_ascends_from_knot =
            last_equal == last || dot(relative, velocities[last_equal]) >= 0.0;
        if left_descends_to_knot && right_ascends_from_knot {
            minima.push(make(
                first_equal.min(last - 1),
                point.t_s,
                point.xyz_m,
                first_equal == 0 || last_equal == last,
            ));
        }
        first_equal = last_equal + 1;
    }
    minima.sort_by(|a, b| a.emission_time_s.total_cmp(&b.emission_time_s));
    // ECEF subtraction can perturb the two adjacent velocities enough to put
    // the same vertex CPA just inside both segments. Preserve distinct physical
    // minima, but coalesce roundoff-level duplicates at a shared track vertex.
    let mut unique: Vec<ClosestApproach> = Vec::with_capacity(minima.len());
    for minimum in minima {
        if let Some(previous) = unique.last_mut()
            && let Some(vertex) =
                same_numerical_vertex(previous, &minimum, track, observer, velocities)
        {
            *previous = make(
                vertex.min(last - 1),
                track.points[vertex].t_s,
                track.points[vertex].xyz_m,
                vertex == 0 || vertex == last,
            );
        } else {
            unique.push(minimum);
        }
    }
    unique
}

fn same_numerical_vertex(
    a: &ClosestApproach,
    b: &ClosestApproach,
    track: &Track,
    observer: [f64; 3],
    velocities: &[[f64; 3]],
) -> Option<usize> {
    for vertex in [a.segment_index, a.segment_index + 1] {
        if vertex != b.segment_index && vertex != b.segment_index + 1 {
            continue;
        }
        let point = &track.points[vertex];
        let speed = norm(velocities[a.segment_index]).min(norm(velocities[b.segment_index]));
        if speed <= 0.0 {
            continue;
        }
        let shortest_dt = [a.segment_index, b.segment_index]
            .into_iter()
            .map(|i| track.points[i + 1].t_s - track.points[i].t_s)
            .fold(f64::INFINITY, f64::min);
        let coordinate_scale = point
            .xyz_m
            .iter()
            .chain(observer.iter())
            .fold(1.0_f64, |scale, value| scale.max(value.abs()));
        let range = norm(sub(point.xyz_m, observer));
        // Includes position/velocity subtraction and timestamp rounding. The
        // strict caps prevent this numerical repair merging resolved nearby passes.
        let tolerance = (128.0 * f64::EPSILON * coordinate_scale / speed
            * (1.0 + range / (speed * shortest_dt))
            + 4.0 * f64::EPSILON * point.t_s.abs())
        .min(1e-6)
        .min(shortest_dt * 1e-6);
        if (a.emission_time_s - point.t_s).abs() <= tolerance
            && (b.emission_time_s - point.t_s).abs() <= tolerance
        {
            return Some(vertex);
        }
    }
    None
}

pub fn closest_approach(
    track: &Track,
    observer: [f64; 3],
    config: ModelConfig,
) -> ModelResult<ClosestApproach> {
    Ok(PreparedTrack::new(track, observer, config)?.closest_approach())
}

pub fn local_closest_approaches(
    track: &Track,
    observer: [f64; 3],
    config: ModelConfig,
) -> ModelResult<Vec<ClosestApproach>> {
    Ok(PreparedTrack::new(track, observer, config)?
        .local_closest_approaches()
        .to_vec())
}

/// Observer-independent input checks, performed once before a search. Bad
/// timestamps or missing positions are input errors, not scientific rejections.
pub fn validate_track(track: &Track, config: ModelConfig) -> ModelResult<()> {
    validate_config(config)?;
    if track.flight_id.trim().is_empty() || track.points.len() < 2 {
        return Err("Each track needs an identity and at least two positions".into());
    }
    if track
        .points
        .iter()
        .any(|p| !p.t_s.is_finite() || !finite_xyz(p.xyz_m))
    {
        return Err(format!(
            "Track {} contains nonfinite values",
            track.flight_id
        ));
    }
    for (index, pair) in track.points.windows(2).enumerate() {
        let dt = pair[1].t_s - pair[0].t_s;
        if !dt.is_finite() || dt <= 0. || dt > config.max_gap_s {
            return Err(format!(
                "Track {} has unordered timestamps or an excessive gap at segment {index}",
                track.flight_id
            ));
        }
        let speed = norm(sub(pair[1].xyz_m, pair[0].xyz_m)) / dt;
        if !speed.is_finite() || speed >= config.sound_speed_mps {
            return Err(format!(
                "Track {} has invalid or non-subsonic motion at segment {index}",
                track.flight_id
            ));
        }
    }
    Ok(())
}

pub fn frequency_at_arrival(
    track: &Track,
    observer: [f64; 3],
    arrival_s: f64,
    f0_hz: f64,
    config: ModelConfig,
) -> ModelResult<f64> {
    if !f0_hz.is_finite() || f0_hz <= 0.0 {
        return Err("Emitted tone frequency must be finite and positive".into());
    }
    let frequency =
        PreparedTrack::new(track, observer, config)?.factor_at_arrival(arrival_s)? * f0_hz;
    if !frequency.is_finite() {
        return Err("Predicted frequency overflowed".into());
    }
    Ok(frequency)
}

fn validate_event(event: &EventTrace, event_number: usize) -> ModelResult<()> {
    if event.points.len() < 3 {
        return Err(format!(
            "Event {event_number} requires at least three fixed trace samples"
        ));
    }
    if event
        .points
        .iter()
        .any(|p| !p.t_rec_s.is_finite() || !p.frequency_hz.is_finite() || p.frequency_hz <= 0.0)
    {
        return Err(format!(
            "Event {event_number} needs finite times and positive finite frequencies"
        ));
    }
    if event
        .points
        .windows(2)
        .any(|p| p[1].t_rec_s <= p[0].t_rec_s)
    {
        return Err(format!(
            "Event {event_number} trace times must be strictly increasing"
        ));
    }
    if event.f0_hz.is_empty() || event.f0_hz.iter().any(|f| !f.is_finite() || *f <= 0.0) {
        return Err(format!(
            "Event {event_number} needs a nonempty sweep of positive finite emitted frequencies"
        ));
    }
    Ok(())
}

fn fit_frequency(
    event: &EventTrace,
    factors: &[f64],
    checkpoint: &mut impl FnMut() -> ModelResult<()>,
) -> ModelResult<(f64, f64)> {
    let mut best: Option<(f64, f64)> = None;
    for &f0 in &event.f0_hz {
        checkpoint()?;
        let mse = event
            .points
            .iter()
            .zip(factors)
            .map(|(p, factor)| (p.frequency_hz - f0 * factor).powi(2))
            .sum::<f64>()
            / event.points.len() as f64;
        if mse.is_finite() && best.is_none_or(|(_, previous)| mse < previous) {
            best = Some((f0, mse));
        }
        checkpoint()?;
    }
    best.ok_or_else(|| "Frequency fitting overflowed; inspect input values".into())
}

/// Score an ordered pair of passes against independently traced ridges.
/// This RMSE scorer is an explicit reproducible alternative to the author's
/// unpublished spectrogram-brightness normalization; its numbers are not the
/// video's score of 20.48. A single B (clock_offset_s) is shared by both events:
/// B = first_model_heard_CPA - measured_first_CPA - shared_shift.
/// Event 2 never receives its own timing adjustment.
pub fn score_pair(
    tracks: [&Track; 2],
    observer: [f64; 3],
    events: [&EventTrace; 2],
    config: &PairConfig,
) -> ModelResult<PairScore> {
    score_pair_selected(tracks, observer, events, config, None, &mut || Ok(()))
}

/// Score explicit local minima from `PreparedTrack::local_closest_approaches`.
/// The same continuous track may supply both events when distinct minima occur
/// in the observed order. Indices are checked against computed minima, never
/// accepted as arbitrary caller-supplied times. Both events still share one B.
pub fn score_pair_at_cpa(
    tracks: [&Track; 2],
    observer: [f64; 3],
    events: [&EventTrace; 2],
    config: &PairConfig,
    cpa_indices: [usize; 2],
) -> ModelResult<PairScore> {
    score_pair_at_cpa_with_checkpoint(tracks, observer, events, config, cpa_indices, || Ok(()))
}

/// The exact local-CPA scorer with cooperative cancellation or work-budget
/// checks. Callback errors propagate unchanged and are never treated as missing
/// model coverage. Arithmetic, candidate order and first-tie selection match
/// `score_pair_at_cpa`; a track preparation and a tone fit each remain one
/// native call between checkpoints, subject to the caller's input limits.
pub fn score_pair_at_cpa_with_checkpoint(
    tracks: [&Track; 2],
    observer: [f64; 3],
    events: [&EventTrace; 2],
    config: &PairConfig,
    cpa_indices: [usize; 2],
    mut checkpoint: impl FnMut() -> ModelResult<()>,
) -> ModelResult<PairScore> {
    score_pair_selected(
        tracks,
        observer,
        events,
        config,
        Some(cpa_indices),
        &mut checkpoint,
    )
}

fn score_pair_selected(
    tracks: [&Track; 2],
    observer: [f64; 3],
    events: [&EventTrace; 2],
    config: &PairConfig,
    cpa_indices: Option<[usize; 2]>,
    checkpoint: &mut impl FnMut() -> ModelResult<()>,
) -> ModelResult<PairScore> {
    checkpoint()?;
    validate_event(events[0], 1)?;
    validate_event(events[1], 2)?;
    checkpoint()?;
    if !config.event1_cpa_recording_s.is_finite()
        || config.shared_shifts_s.is_empty()
        || config.shared_shifts_s.iter().any(|s| !s.is_finite())
    {
        return Err("The event-1 CPA and a nonempty shared-shift sweep must be finite".into());
    }
    if events[1].points[0].t_rec_s <= events[0].points.last().unwrap().t_rec_s {
        return Err("The ordered event trace windows must not overlap".into());
    }
    if let Some([low, high]) = config.clock_offset_bounds_s
        && (!low.is_finite() || !high.is_finite() || low > high)
    {
        return Err("Recording-start bounds must be finite and ordered".into());
    }
    checkpoint()?;
    let first = PreparedTrack::new(tracks[0], observer, config.model)?;
    checkpoint()?;
    let second = PreparedTrack::new(tracks[1], observer, config.model)?;
    checkpoint()?;
    let models = [first, second];
    let cpa = if let Some(indices) = cpa_indices {
        let mut select = |event: usize| {
            checkpoint()?;
            let result = models[event]
                .local_closest_approaches()
                .get(indices[event])
                .copied()
                .ok_or_else(|| {
                    format!(
                        "Local CPA index {} is outside track {} minima",
                        indices[event], tracks[event].flight_id
                    )
                });
            checkpoint()?;
            result
        };
        [select(0)?, select(1)?]
    } else {
        [models[0].closest_approach(), models[1].closest_approach()]
    };
    checkpoint()?;
    if cpa.iter().any(|c| c.at_track_boundary) {
        return Err(
            "A selected closest approach is at a track boundary; supply a complete pass".into(),
        );
    }
    if cpa[1].arrival_time_s <= cpa[0].arrival_time_s {
        return Err("The selected flights do not preserve the observed event order".into());
    }
    let mut best: Option<PairScore> = None;
    'shifts: for &shared_shift in &config.shared_shifts_s {
        checkpoint()?;
        let clock = cpa[0].arrival_time_s - config.event1_cpa_recording_s - shared_shift;
        if !clock.is_finite() {
            return Err("Shared clock overflowed".into());
        }
        if config
            .clock_offset_bounds_s
            .is_some_and(|[low, high]| clock < low || clock > high)
        {
            continue;
        }
        let mut factors = Vec::with_capacity(2);
        for (event, model) in events.iter().zip(models.iter()) {
            let mut values = Vec::with_capacity(event.points.len());
            for (index, point) in event.points.iter().enumerate() {
                if index.is_multiple_of(64) {
                    checkpoint()?;
                }
                match model.factor_at_arrival(point.t_rec_s + clock) {
                    Ok(factor) => values.push(factor),
                    Err(_) => continue 'shifts,
                }
            }
            checkpoint()?;
            factors.push(values);
        }
        let fit1 = fit_frequency(events[0], &factors[0], checkpoint)?;
        let fit2 = fit_frequency(events[1], &factors[1], checkpoint)?;
        let combined = (0.5 * fit1.1 + 0.5 * fit2.1).sqrt();
        if best
            .as_ref()
            .is_none_or(|score| combined < score.combined_rmse_hz)
        {
            best = Some(PairScore {
                flight_ids: [tracks[0].flight_id.clone(), tracks[1].flight_id.clone()],
                observer_xyz_m: observer,
                rmse_hz: [fit1.1.sqrt(), fit2.1.sqrt()],
                combined_rmse_hz: combined,
                fitted_f0_hz: [fit1.0, fit2.0],
                clock_offset_s: clock,
                shared_shift_s: shared_shift,
                cpa,
                predicted_event2_cpa_recording_s: cpa[1].arrival_time_s - clock,
            });
        }
        checkpoint()?;
    }
    checkpoint()?;
    best.ok_or_else(|| {
        "No shared clock shift provides complete track coverage for both fixed trace windows".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn straight(id: &str, center: f64, direction: usize, lateral: f64, height: f64) -> Track {
        Track {
            flight_id: id.into(),
            points: (-120..=120)
                .step_by(5)
                .map(|t| {
                    let mut xyz = [lateral, lateral, height];
                    xyz[direction] = t as f64 * 100.0;
                    TrackPoint {
                        t_s: center + t as f64,
                        xyz_m: xyz,
                    }
                })
                .collect(),
        }
    }

    fn assert_near(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() < tolerance,
            "actual {actual}, expected {expected}"
        );
    }

    #[test]
    fn cpa_is_continuous_not_snapped_to_a_track_sample() {
        let track = Track {
            flight_id: "coarse".into(),
            points: vec![
                TrackPoint {
                    t_s: -10.0,
                    xyz_m: [-1000.0, 0.0, 3000.0],
                },
                TrackPoint {
                    t_s: 10.0,
                    xyz_m: [1000.0, 0.0, 3000.0],
                },
            ],
        };
        let cpa = closest_approach(&track, [0.0; 3], ModelConfig::default()).unwrap();
        assert_near(cpa.emission_time_s, 0.0, 1e-10);
        assert_near(cpa.slant_distance_m, 3000.0, 1e-10);
        assert_near(cpa.arrival_time_s, 3000.0 / 343.0, 1e-10);
        assert_near(cpa.speed_mps, 100.0, 1e-10);
        assert!(!cpa.at_track_boundary);
    }

    #[test]
    fn arrival_model_matches_independent_analytic_inverse_and_doppler_sign() {
        let track = straight("level", 0.0, 0, 0.0, 3000.0);
        let model = PreparedTrack::new(&track, [0.0; 3], ModelConfig::default()).unwrap();
        let c = 343.0_f64;
        let v = 100.0_f64;
        let d = 3000.0_f64;
        for arrival in [-40.0_f64, 0.0, 30.0, 70.0] {
            // Analytic solution for a straight constant-speed flight is an
            // independent oracle for the segment-wise inverse implementation.
            let emission = (c * c * arrival
                - (c * c * v * v * arrival * arrival + (c * c - v * v) * d * d).sqrt())
                / (c * c - v * v);
            let radial = v * v * emission / (d * d + v * v * emission * emission).sqrt();
            assert_near(
                model.factor_at_arrival(arrival).unwrap(),
                c / (c + radial),
                1e-12,
            );
        }
        assert!(model.factor_at_arrival(-40.0).unwrap() > 1.0);
        assert!(model.factor_at_arrival(70.0).unwrap() < 1.0);
        assert_near(model.factor_at_arrival(d / c).unwrap(), 1.0, 1e-12);
        let constant_delay_emission = 70.0 - d / c;
        let wrong_radial = v * v * constant_delay_emission
            / (d * d + v * v * constant_delay_emission.powi(2)).sqrt();
        assert!((model.factor_at_arrival(70.0).unwrap() - c / (c + wrong_radial)).abs() > 0.005);
    }

    #[test]
    fn rejects_bad_tracks_and_refuses_extrapolation() {
        let good = straight("valid", 0.0, 0, 0.0, 3000.0);
        let config = ModelConfig::default();
        let model = PreparedTrack::new(&good, [0.0; 3], config).unwrap();
        let [first, last] = model.arrival_bounds_s();
        assert!(model.factor_at_arrival(first).is_ok());
        assert!(model.factor_at_arrival(last).is_ok());
        assert!(model.factor_at_arrival(first - 0.001).is_err());
        assert!(model.factor_at_arrival(last + 0.001).is_err());
        assert!(model.factor_at_arrival(f64::NAN).is_err());
        let mut invalid = good.clone();
        invalid.points[1].t_s = invalid.points[0].t_s;
        assert!(PreparedTrack::new(&invalid, [0.0; 3], config).is_err());
        let mut invalid = good.clone();
        invalid.points[1].xyz_m[0] += 100_000.0;
        assert!(PreparedTrack::new(&invalid, [0.0; 3], config).is_err());
        assert!(
            PreparedTrack::new(
                &good,
                [0.0; 3],
                ModelConfig {
                    max_gap_s: 4.0,
                    ..config
                }
            )
            .is_err()
        );
        assert!(PreparedTrack::new(&good, [f64::NAN, 0.0, 0.0], config).is_err());
        assert!(
            PreparedTrack::new(
                &good,
                [0.0; 3],
                ModelConfig {
                    sound_speed_mps: 0.0,
                    ..config
                }
            )
            .is_err()
        );
        let crossing = straight("crossing", 0.0, 0, 0.0, 0.0);
        assert!(PreparedTrack::new(&crossing, [0.0; 3], config).is_err());
    }

    #[test]
    fn wgs84_known_axes_and_input_validation() {
        let equator = geodetic_to_ecef(0.0, 0.0, 0.0).unwrap();
        assert_near(equator[0], 6_378_137.0, 1e-6);
        assert_near(equator[1], 0.0, 1e-6);
        let pole = geodetic_to_ecef(90.0, 0.0, 0.0).unwrap();
        assert_near(pole[2], 6_356_752.314_245, 1e-6);
        assert!(geodetic_to_ecef(91.0, 0.0, 0.0).is_err());
        assert!(geodetic_to_ecef(0.0, -181.0, 0.0).is_err());
        assert!(geodetic_to_ecef(0.0, 0.0, f64::NAN).is_err());
    }

    fn synthetic_fixture() -> ([Track; 2], [EventTrace; 2], PairConfig, [f64; 3]) {
        let observer = [200.0, -150.0, 20.0];
        let tracks = [
            straight("first", 0.0, 0, 0.0, 3000.0),
            straight("second", 920.0, 1, 2500.0, 2000.0),
        ];
        let config = PairConfig {
            event1_cpa_recording_s: 100.0,
            shared_shifts_s: vec![-2.0, -1.0, 0.0, 1.25, 2.0],
            clock_offset_bounds_s: None,
            model: ModelConfig::default(),
        };
        let m1 = PreparedTrack::new(&tracks[0], observer, config.model).unwrap();
        let m2 = PreparedTrack::new(&tracks[1], observer, config.model).unwrap();
        let clock = m1.closest_approach().arrival_time_s - 101.25;
        let make_event = |m: &PreparedTrack, f0: f64| {
            let center = m.closest_approach().arrival_time_s - clock;
            EventTrace {
                points: (-60..=60)
                    .step_by(2)
                    .map(|offset| {
                        let t_rec_s = center + offset as f64;
                        TracePoint {
                            t_rec_s,
                            frequency_hz: m.factor_at_arrival(t_rec_s + clock).unwrap() * f0,
                        }
                    })
                    .collect(),
                f0_hz: vec![f0 - 1.0, f0, f0 + 1.0],
            }
        };
        let events = [make_event(&m1, 130.0), make_event(&m2, 145.0)];
        (tracks, events, config, observer)
    }

    #[test]
    fn checkpointed_pair_scoring_preserves_exact_scores_and_tie_order() {
        let (tracks, events, mut config, observer) = synthetic_fixture();
        // Duplicate the best shift: strict comparisons must keep its first
        // occurrence, and checkpoints must not perturb any arithmetic.
        config.shared_shifts_s.extend([1.25, 1.25]);
        let legacy = score_pair(
            [&tracks[0], &tracks[1]],
            observer,
            [&events[0], &events[1]],
            &config,
        )
        .unwrap();
        let explicit = score_pair_at_cpa(
            [&tracks[0], &tracks[1]],
            observer,
            [&events[0], &events[1]],
            &config,
            [0, 0],
        )
        .unwrap();
        let mut visits = 0;
        let checked = score_pair_at_cpa_with_checkpoint(
            [&tracks[0], &tracks[1]],
            observer,
            [&events[0], &events[1]],
            &config,
            [0, 0],
            || {
                visits += 1;
                Ok(())
            },
        )
        .unwrap();
        assert!(visits > config.shared_shifts_s.len() * 2);
        assert_eq!(
            serde_json::to_vec(&checked).unwrap(),
            serde_json::to_vec(&legacy).unwrap()
        );
        assert_eq!(
            serde_json::to_vec(&checked).unwrap(),
            serde_json::to_vec(&explicit).unwrap()
        );
        assert_eq!(checked.fitted_f0_hz, [130., 145.]);
        assert_eq!(checked.shared_shift_s.to_bits(), 1.25f64.to_bits());

        let tie = EventTrace {
            points: (0..3)
                .map(|index| TracePoint {
                    t_rec_s: index as f64,
                    frequency_hz: 2.,
                })
                .collect(),
            f0_hz: vec![1., 3.],
        };
        assert_eq!(
            fit_frequency(&tie, &[1.; 3], &mut || Ok(())).unwrap(),
            (1., 1.)
        );
    }

    #[test]
    fn frequency_checkpoint_stops_mid_sweep_without_returning_an_earlier_fit() {
        let event = EventTrace {
            points: (0..3)
                .map(|index| TracePoint {
                    t_rec_s: index as f64,
                    frequency_hz: 2.,
                })
                .collect(),
            f0_hz: vec![1., 2., 3.],
        };
        for stop_at in [3, 4] {
            let mut visits = 0;
            let error = fit_frequency(&event, &[1.; 3], &mut || {
                visits += 1;
                if visits == stop_at {
                    Err("stop within emitted-frequency sweep".into())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert_eq!(visits, stop_at);
            assert_eq!(error, "stop within emitted-frequency sweep");
        }
        assert_eq!(
            fit_frequency(&event, &[1.; 3], &mut || Ok(())).unwrap(),
            (2., 0.)
        );
    }

    #[test]
    fn trace_factor_batch_checkpoint_errors_are_never_missing_model_coverage() {
        let (tracks, mut events, mut config, observer) = synthetic_fixture();
        config.shared_shifts_s = vec![1.25];
        let clock = PreparedTrack::new(&tracks[0], observer, config.model)
            .unwrap()
            .closest_approach()
            .arrival_time_s
            - config.event1_cpa_recording_s
            - 1.25;
        for (track, event) in tracks.iter().zip(events.iter_mut()) {
            let model = PreparedTrack::new(track, observer, config.model).unwrap();
            let first = event.points.first().unwrap().t_rec_s;
            let last = event.points.last().unwrap().t_rec_s;
            let f0 = event.f0_hz[1];
            event.points = (0..129)
                .map(|index| {
                    let t_rec_s = first + (last - first) * index as f64 / 128.;
                    TracePoint {
                        t_rec_s,
                        frequency_hz: model.factor_at_arrival(t_rec_s + clock).unwrap() * f0,
                    }
                })
                .collect();
        }
        let mut checkpoints = 0;
        score_pair_at_cpa_with_checkpoint(
            [&tracks[0], &tracks[1]],
            observer,
            [&events[0], &events[1]],
            &config,
            [0, 0],
            || {
                checkpoints += 1;
                Ok(())
            },
        )
        .unwrap();
        // This spans multiple64-sample batches in both events. Interrupt every
        // reachable checkpoint so a stop in any batch cannot become a skipped
        // shift, an incomplete best fit, or a missing-coverage error.
        for stop_at in 1..=checkpoints {
            let mut visits = 0;
            let error = score_pair_at_cpa_with_checkpoint(
                [&tracks[0], &tracks[1]],
                observer,
                [&events[0], &events[1]],
                &config,
                [0, 0],
                || {
                    visits += 1;
                    if visits == stop_at {
                        Err("requested pair stop".into())
                    } else {
                        Ok(())
                    }
                },
            )
            .unwrap_err();
            assert_eq!(visits, stop_at);
            assert_eq!(error, "requested pair stop");
        }
    }

    #[test]
    fn pair_checkpoint_stops_at_later_shift_without_returning_completed_best() {
        let (tracks, events, mut config, observer) = synthetic_fixture();
        config.shared_shifts_s = vec![1.25];
        let mut first_shift_calls = 0;
        score_pair_at_cpa_with_checkpoint(
            [&tracks[0], &tracks[1]],
            observer,
            [&events[0], &events[1]],
            &config,
            [0, 0],
            || {
                first_shift_calls += 1;
                Ok(())
            },
        )
        .unwrap();
        // The completed one-shift run's final checkpoint occupies the same
        // position as entry to the next shift in this two-shift run.
        config.shared_shifts_s.push(2.0);
        let mut visits = 0;
        let error = score_pair_at_cpa_with_checkpoint(
            [&tracks[0], &tracks[1]],
            observer,
            [&events[0], &events[1]],
            &config,
            [0, 0],
            || {
                visits += 1;
                if visits == first_shift_calls {
                    Err("stop before second shared shift".into())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(visits, first_shift_calls);
        assert_eq!(error, "stop before second shared shift");
    }

    #[test]
    fn shared_clock_recovers_frequencies_shift_and_known_observer_rank() {
        let (tracks, events, config, observer) = synthetic_fixture();
        let score = score_pair(
            [&tracks[0], &tracks[1]],
            observer,
            [&events[0], &events[1]],
            &config,
        )
        .unwrap();
        assert!(score.combined_rmse_hz < 1e-9);
        assert_eq!(score.fitted_f0_hz, [130.0, 145.0]);
        assert_eq!(score.shared_shift_s, 1.25);
        assert_near(
            score.predicted_event2_cpa_recording_s,
            events[1].points[30].t_rec_s,
            1e-9,
        );
        for offset in [
            [200.0, 0.0, 0.0],
            [-200.0, 0.0, 0.0],
            [0.0, 200.0, 0.0],
            [0.0, -200.0, 0.0],
        ] {
            let candidate = [
                observer[0] + offset[0],
                observer[1] + offset[1],
                observer[2] + offset[2],
            ];
            let other = score_pair(
                [&tracks[0], &tracks[1]],
                candidate,
                [&events[0], &events[1]],
                &config,
            )
            .unwrap();
            assert!(other.combined_rmse_hz > score.combined_rmse_hz + 0.01);
        }
    }

    #[test]
    fn independent_second_event_time_shift_cannot_hide_wrong_pair() {
        let (mut tracks, events, config, observer) = synthetic_fixture();
        for point in &mut tracks[1].points {
            point.t_s += 20.0;
        }
        let bad = score_pair(
            [&tracks[0], &tracks[1]],
            observer,
            [&events[0], &events[1]],
            &config,
        )
        .unwrap();
        assert!(bad.combined_rmse_hz > 1.0);
        // Giving event 2 a private +20 s clock would fit exactly, which is why
        // score_pair deliberately has no independent event-2 shift parameter.
        let second = PreparedTrack::new(&tracks[1], observer, config.model).unwrap();
        let first = PreparedTrack::new(&tracks[0], observer, config.model).unwrap();
        let true_clock = first.closest_approach().arrival_time_s - 101.25;
        for p in &events[1].points {
            assert_near(
                second
                    .factor_at_arrival(p.t_rec_s + true_clock + 20.0)
                    .unwrap()
                    * 145.0,
                p.frequency_hz,
                1e-9,
            );
        }
    }

    #[test]
    fn recording_start_window_rejects_an_otherwise_good_pair() {
        let (tracks, events, mut config, observer) = synthetic_fixture();
        config.clock_offset_bounds_s = Some([10_000., 20_000.]);
        assert!(
            score_pair(
                [&tracks[0], &tracks[1]],
                observer,
                [&events[0], &events[1]],
                &config
            )
            .is_err()
        );
        config.clock_offset_bounds_s = Some([20_000., 10_000.]);
        assert!(
            score_pair(
                [&tracks[0], &tracks[1]],
                observer,
                [&events[0], &events[1]],
                &config
            )
            .unwrap_err()
            .contains("bounds")
        );
    }

    #[test]
    fn rejects_partial_coverage_invalid_trace_and_boundary_cpa() {
        let (tracks, mut events, config, observer) = synthetic_fixture();
        events[1].points.last_mut().unwrap().t_rec_s += 1000.0;
        assert!(
            score_pair(
                [&tracks[0], &tracks[1]],
                observer,
                [&events[0], &events[1]],
                &config
            )
            .is_err()
        );
        let (tracks, mut events, config, observer) = synthetic_fixture();
        events[0].points[1].t_rec_s = events[0].points[0].t_rec_s;
        assert!(
            score_pair(
                [&tracks[0], &tracks[1]],
                observer,
                [&events[0], &events[1]],
                &config
            )
            .is_err()
        );
        let (mut tracks, events, config, observer) = synthetic_fixture();
        tracks[0].points.retain(|p| p.t_s >= 20.0);
        assert!(
            score_pair(
                [&tracks[0], &tracks[1]],
                observer,
                [&events[0], &events[1]],
                &config
            )
            .is_err()
        );
    }

    #[test]
    fn common_epoch_shift_preserves_frequency() {
        let track = straight("relative", 0.0, 0, 0.0, 3000.0);
        let mut absolute = track.clone();
        for p in &mut absolute.points {
            p.t_s += 1_700_000_000.0;
        }
        let relative =
            frequency_at_arrival(&track, [0.0; 3], 32.0, 130.0, ModelConfig::default()).unwrap();
        let epoch = frequency_at_arrival(
            &absolute,
            [0.0; 3],
            1_700_000_032.0,
            130.0,
            ModelConfig::default(),
        )
        .unwrap();
        assert_near(relative, epoch, 1e-7);
    }

    #[test]
    fn turning_climbing_track_preserves_each_samples_own_propagation_delay() {
        let track = Track {
            flight_id: "maneuver".into(),
            points: vec![
                TrackPoint {
                    t_s: 0.0,
                    xyz_m: [0.0, 0.0, 3000.0],
                },
                TrackPoint {
                    t_s: 10.0,
                    xyz_m: [1000.0, 200.0, 3500.0],
                },
                TrackPoint {
                    t_s: 20.0,
                    xyz_m: [1700.0, 1000.0, 3300.0],
                },
            ],
        };
        let observer = [200.0, 400.0, 10.0];
        let model = PreparedTrack::new(&track, observer, ModelConfig::default()).unwrap();
        // Independently construct a forward arrival/frequency pair from a
        // known point inside each segment and recover it with the inverse.
        for pair in track.points.windows(2) {
            let velocity = sub(pair[1].xyz_m, pair[0].xyz_m).map(|x| x / 10.0);
            for offset in [1.0, 5.0, 9.0] {
                let relative = sub(advance(pair[0].xyz_m, velocity, offset), observer);
                let distance = norm(relative);
                let arrival = pair[0].t_s + offset + distance / 343.0;
                let radial = dot(velocity, relative) / distance;
                assert_near(
                    model.factor_at_arrival(arrival).unwrap(),
                    343.0 / (343.0 + radial),
                    1e-12,
                );
            }
        }
    }
}
