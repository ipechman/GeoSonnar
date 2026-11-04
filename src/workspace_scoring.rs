//! Manual workspace to physical predictions. All aircraft/strike associations,
//! emitted frequencies, clock shifts and selected terms come from the workspace.
//! This bounded comparison performs no hidden parameter or association fitting.

use crate::{
    atmosphere::{PreparedAtmosphere, PreparedAtmosphericTrack},
    evidence_score::{
        self, AssociationKind, CandidateEvidence, EvidenceSet, FrequencyObservation,
        FrequencyPrediction, LightningCatalog, ObserverLocation, ScoreConfig, ScoreReport,
        TermSettings, ThunderTimingObservation, TimingObservation, TimingPrediction,
    },
    map_data::{GeoTrack, MapData},
    physics::{ClosestApproach, ModelConfig, PreparedTrack, Track, geodetic_to_ecef},
    thunder::ThunderAnnotation,
    workspace::{EventKind, EvidenceEvent, MapSelection, ReviewState, ReviewWorkspace, parse_utc},
};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_OBSERVERS: usize = 10_000;
pub const MAX_RESIDUALS: usize = 250_000;
pub const MAX_ATMOSPHERE_OBSERVERS: usize = 400;
pub const MAX_ATMOSPHERE_RESIDUALS: usize = 20_000;
pub const MAX_ATMOSPHERE_TRACK_POINT_VISITS: usize = 150_000;
pub const MAX_TRACK_POINT_VISITS: usize = 5_000_000;

/// A bounded equal-angle grid including all four edges. Receiver height is the
/// user-declared WGS84 ellipsoid height, not terrain supplied automatically.
pub fn grid_observers(selection: &MapSelection) -> Result<Vec<ObserverLocation>, String> {
    let [south, west, north, east] = selection.bounds;
    if !selection.bounds.iter().all(|n| n.is_finite())
        || south < -85.0
        || north > 85.0
        || south >= north
        || west < -180.0
        || east > 180.0
        || west >= east
        || !selection.grid_spacing_m.is_finite()
        || selection.grid_spacing_m < 10.0
        || !selection.receiver_height_m.is_finite()
    {
        return Err("Grid needs ordered geographic bounds, at least 10 m spacing and an explicit ellipsoid height".into());
    }
    // Conservative radius for converting angular spans into subdivisions. This
    // is an angular grid, not an equal-area confidence region.
    let radius = 6_400_000.0;
    let nearest_equator = if south <= 0.0 && north >= 0.0 {
        0.0
    } else {
        south.abs().min(north.abs())
    };
    let lat_steps = ((north - south).to_radians() * radius / selection.grid_spacing_m)
        .ceil()
        .max(1.0);
    let lon_steps = ((east - west).to_radians() * radius * nearest_equator.to_radians().cos()
        / selection.grid_spacing_m)
        .ceil()
        .max(1.0);
    if lat_steps > MAX_OBSERVERS as f64
        || lon_steps > MAX_OBSERVERS as f64
        || (lat_steps + 1.0) * (lon_steps + 1.0) > MAX_OBSERVERS as f64
    {
        return Err(format!(
            "Grid exceeds {MAX_OBSERVERS} points; narrow its bounds or increase spacing"
        ));
    }
    let (rows, columns) = (lat_steps as usize, lon_steps as usize);
    let mut points = Vec::with_capacity((rows + 1) * (columns + 1));
    for row in 0..=rows {
        for column in 0..=columns {
            points.push(ObserverLocation {
                latitude_deg: south + (north - south) * row as f64 / rows as f64,
                longitude_deg: west + (east - west) * column as f64 / columns as f64,
                ellipsoid_height_m: selection.receiver_height_m,
            });
        }
    }
    Ok(points)
}

pub fn score_grid(
    document: &ReviewWorkspace,
    map: &MapData,
    lightning: &Option<LightningCatalog>,
    fallback_strike_height: Option<f64>,
) -> Result<ScoreReport, String> {
    document.validate()?;
    document.validate_acoustic_sources_for_scoring()?;
    let heights = crate::receiver_heights::grid(document)?;
    let observers = heights.complete_observers()?;
    let mut report = score_observers(document, map, lightning, fallback_strike_height, &observers)?;
    report.limitations.extend(heights.limitations);
    Ok(report)
}

pub(crate) struct SelectedTrack {
    pub(crate) origin: f64,
    pub(crate) max_gap: f64,
    pub(crate) segments: Vec<Track>,
}

pub(crate) enum ReceiverModel<'a, 'b> {
    Constant(PreparedTrack<'b>),
    Profile(PreparedAtmosphericTrack<'a, 'b>),
}
impl ReceiverModel<'_, '_> {
    pub(crate) fn arrival_bounds_s(&self) -> [f64; 2] {
        match self {
            Self::Constant(m) => m.arrival_bounds_s(),
            Self::Profile(m) => m.arrival_bounds_s(),
        }
    }
    pub(crate) fn factor_at_arrival(&self, arrival: f64) -> Result<f64, String> {
        match self {
            Self::Constant(m) => m.factor_at_arrival(arrival),
            Self::Profile(m) => m.factor_at_arrival(arrival),
        }
    }
    pub(crate) fn arrival_knots_s(&self) -> &[f64] {
        match self {
            Self::Constant(m) => m.arrival_knots_s(),
            Self::Profile(m) => m.arrival_knots_s(),
        }
    }
    fn local_closest_approaches(&self) -> &[ClosestApproach] {
        match self {
            Self::Constant(m) => m.local_closest_approaches(),
            Self::Profile(m) => m.local_closest_approaches(),
        }
    }
}

pub(crate) fn selected_track(track: &GeoTrack) -> Result<SelectedTrack, String> {
    let origin = track.time_origin_utc_s.ok_or_else(|| {
        format!(
            "Flight {} needs an explicit UTC origin before scoring",
            track.id
        )
    })?;
    let max_gap = track.max_gap_s.ok_or_else(|| {
        format!(
            "Flight {} needs an explicit maximum interpolation gap before scoring",
            track.id
        )
    })?;
    if !origin.is_finite()
        || !(0.0..253_402_300_800.0).contains(&origin)
        || !max_gap.is_finite()
        || max_gap <= 0.0
    {
        return Err(format!(
            "Flight {} has invalid UTC origin or interpolation gap",
            track.id
        ));
    }
    let mut end = 0;
    let mut segments = Vec::new();
    for &[start, next] in &track.segments {
        if start != end || next <= start || next > track.physics_track.points.len() {
            return Err(
                "Flight segment inventory is incomplete, overlapping or out of bounds".into(),
            );
        }
        if next - start >= 2 {
            segments.push(Track {
                flight_id: track.id.clone(),
                points: track.physics_track.points[start..next].to_vec(),
            });
        }
        end = next;
    }
    if end != track.physics_track.points.len() || segments.is_empty() {
        return Err(format!(
            "Flight {} has no complete gap-bounded trajectory segment",
            track.id
        ));
    }
    Ok(SelectedTrack {
        origin,
        max_gap,
        segments,
    })
}

pub(crate) fn prepare_models<'a, 'b>(
    track: &'b SelectedTrack,
    xyz: [f64; 3],
    sound_speed_mps: f64,
    atmosphere: Option<&'b PreparedAtmosphere<'a>>,
) -> Result<Vec<ReceiverModel<'a, 'b>>, String> {
    track
        .segments
        .iter()
        .map(|segment| {
            if let Some(profile) = atmosphere {
                PreparedAtmosphericTrack::new(segment, xyz, track.origin, track.max_gap, profile)
                    .map(ReceiverModel::Profile)
            } else {
                PreparedTrack::new(
                    segment,
                    xyz,
                    ModelConfig {
                        sound_speed_mps,
                        max_gap_s: track.max_gap,
                    },
                )
                .map(ReceiverModel::Constant)
            }
        })
        .collect()
}

fn observation_id(event: &EvidenceEvent, index: usize) -> String {
    format!("{}-sample-{index}", event.id)
}

pub fn score_observers(
    document: &ReviewWorkspace,
    map: &MapData,
    lightning: &Option<LightningCatalog>,
    fallback_strike_height: Option<f64>,
    observers: &[ObserverLocation],
) -> Result<ScoreReport, String> {
    document.validate()?;
    document.validate_acoustic_sources_for_scoring()?;
    let atmosphere = document
        .atmosphere
        .as_ref()
        .map(PreparedAtmosphere::new)
        .transpose()?;
    if observers.is_empty() || observers.len() > MAX_OBSERVERS {
        return Err(format!("Compare 1..{MAX_OBSERVERS} receiver locations"));
    }
    let recording_start =
        parse_utc(&document.recording_start_utc)? + document.map.clock_adjustment_s;
    if !recording_start.is_finite() || !(0.0..253_402_300_800.0).contains(&recording_start) {
        return Err("Adjusted recording start is outside Unix UTC range".into());
    }
    let selected: Vec<_> = document
        .events
        .iter()
        .filter(|e| e.selected_for_scoring && e.review == ReviewState::Confirmed)
        .collect();
    let mut evidence = EvidenceSet {
        lightning: lightning.clone(),
        ..Default::default()
    };
    let mut needed_flights = BTreeSet::new();
    for event in &selected {
        match event.kind {
            EventKind::Doppler if document.mix.doppler_frequency || document.mix.doppler_timing => {
                if event.flight_id.trim().is_empty() {
                    return Err(format!("Select an aircraft for {}", event.label));
                }
                needed_flights.insert(event.flight_id.as_str());
                if document.mix.doppler_frequency {
                    if event.points.is_empty()
                        || !event.emitted_frequency_hz.is_finite()
                        || event.emitted_frequency_hz <= 0.0
                    {
                        return Err(format!(
                            "{} needs measured frequency points and an explicit emitted frequency",
                            event.label
                        ));
                    }
                    for (i, p) in event.points.iter().enumerate() {
                        evidence.frequency.push(FrequencyObservation {
                            id: observation_id(event, i),
                            event_id: event.id.clone(),
                            time_s: p.time_s,
                            frequency_hz: p.frequency_hz,
                            uncertainty_hz: event.frequency_sigma_hz,
                        });
                    }
                }
                if document.mix.doppler_timing {
                    evidence.doppler_timing.push(TimingObservation {
                        id: event.id.clone(),
                        event_id: event.id.clone(),
                        time_s: event.arrival_s,
                        uncertainty_s: event.uncertainty_s,
                    });
                }
            }
            EventKind::Thunder if document.mix.thunder_timing => {
                if event.strike_id.trim().is_empty() {
                    return Err(format!(
                        "Select an explicit lightning strike for {}",
                        event.label
                    ));
                }
                evidence.thunder.push(ThunderTimingObservation {
                    annotation: ThunderAnnotation {
                        id: event.id.clone(),
                        start_s: event.start_s,
                        end_s: event.end_s,
                        arrival_s: event.arrival_s,
                        uncertainty_s: event.uncertainty_s,
                        // Legacy low-level authorization to evaluate this sound
                        // as thunder, not independent source identification.
                        // The separate source-class gate has already passed.
                        reviewed_as_thunder: true,
                        source_proposal_id: None,
                        note: format!(
                            "{}; acoustic source: {}{}",
                            event.label,
                            event.acoustic_source.label(),
                            event
                                .acoustic_source
                                .rationale()
                                .map(|reason| format!("; rationale: {reason}"))
                                .unwrap_or_default()
                        ),
                    },
                    strike_id: event.strike_id.clone(),
                    association: AssociationKind::UserReviewed,
                });
            }
            _ => {}
        }
    }
    let residuals =
        evidence.frequency.len() + evidence.doppler_timing.len() + evidence.thunder.len();
    if atmosphere.is_some()
        && (observers.len() > MAX_ATMOSPHERE_OBSERVERS
            || residuals
                .checked_mul(observers.len())
                .is_none_or(|n| n > MAX_ATMOSPHERE_RESIDUALS))
    {
        return Err(format!(
            "Experimental atmospheric refinement is limited to {MAX_ATMOSPHERE_OBSERVERS} receivers and {MAX_ATMOSPHERE_RESIDUALS} residuals; narrow the area or reduce selected measurements"
        ));
    }
    if residuals
        .checked_mul(observers.len())
        .is_none_or(|n| n > MAX_RESIDUALS)
    {
        return Err(format!(
            "Comparison exceeds {MAX_RESIDUALS} residuals; reduce grid size or selected measurements"
        ));
    }
    let mut tracks = BTreeMap::new();
    for id in needed_flights {
        let matches: Vec<_> = map.tracks.iter().filter(|t| t.id == id).collect();
        if matches.len() != 1 {
            return Err(format!(
                "Selected aircraft {id} must identify exactly one imported trajectory"
            ));
        }
        tracks.insert(id, selected_track(matches[0])?);
    }
    let track_points: usize = tracks
        .values()
        .flat_map(|t| &t.segments)
        .map(|t| t.points.len())
        .sum();
    if atmosphere.is_some()
        && track_points
            .checked_mul(observers.len())
            .is_none_or(|n| n > MAX_ATMOSPHERE_TRACK_POINT_VISITS)
    {
        return Err(format!(
            "Experimental atmospheric refinement exceeds {MAX_ATMOSPHERE_TRACK_POINT_VISITS} trajectory-point visits; use a smaller grid and locally bounded tracks"
        ));
    }
    if track_points
        .checked_mul(observers.len())
        .is_none_or(|n| n > MAX_TRACK_POINT_VISITS)
    {
        return Err(format!(
            "Comparison exceeds {MAX_TRACK_POINT_VISITS} trajectory-point visits; narrow tracks or grid"
        ));
    }
    let config = ScoreConfig {
        frequency: TermSettings {
            enabled: document.mix.doppler_frequency,
            weight: document.mix.frequency_weight,
            residual_scale: document.mix.frequency_scale_hz,
        },
        doppler_timing: TermSettings {
            enabled: document.mix.doppler_timing,
            weight: document.mix.doppler_timing_weight,
            residual_scale: document.mix.doppler_timing_scale_s,
        },
        thunder_timing: TermSettings {
            enabled: document.mix.thunder_timing,
            weight: document.mix.thunder_weight,
            residual_scale: document.mix.thunder_scale_s,
        },
        sound_speed_mps: document.sound_speed_mps,
        atmosphere: document.atmosphere.clone(),
        fallback_strike_ellipsoid_height_m: fallback_strike_height,
        allow_exploratory_associations: false,
        lightning_source_policy: document.mix.lightning_source_policy.clone(),
    };
    let mut candidates = Vec::with_capacity(observers.len());
    let mut failures: Vec<Vec<String>> = Vec::with_capacity(observers.len());
    for (i, observer) in observers.iter().enumerate() {
        let xyz = geodetic_to_ecef(
            observer.latitude_deg,
            observer.longitude_deg,
            observer.ellipsoid_height_m,
        )?;
        let mut candidate = CandidateEvidence {
            id: format!("receiver-{i:05}"),
            observer: observer.clone(),
            recording_start_utc_s: recording_start,
            frequency: Vec::new(),
            doppler_timing: Vec::new(),
        };
        let mut issues = Vec::new();
        let mut prepared = BTreeMap::new();
        for (&id, track) in &tracks {
            let models = prepare_models(track, xyz, document.sound_speed_mps, atmosphere.as_ref());
            match models {
                Ok(models) => {
                    prepared.insert(id, models);
                }
                Err(error) => issues.push(format!("{id}: {error}")),
            }
        }
        for event in &selected {
            if event.kind != EventKind::Doppler
                || !(document.mix.doppler_frequency || document.mix.doppler_timing)
            {
                continue;
            }
            let Some(models) = prepared.get(event.flight_id.as_str()) else {
                continue;
            };
            let track = &tracks[event.flight_id.as_str()];
            let start_from_origin = recording_start - track.origin;
            if document.mix.doppler_frequency {
                for (j, point) in event.points.iter().enumerate() {
                    let arrival = start_from_origin + point.time_s;
                    let covering: Vec<_> = models
                        .iter()
                        .filter(|m| {
                            let [first, last] = m.arrival_bounds_s();
                            arrival >= first && arrival <= last
                        })
                        .collect();
                    if covering.len() != 1 {
                        issues.push(format!(
                            "{} sample {} lacks unique reception-time track coverage",
                            event.id, j
                        ));
                        continue;
                    }
                    match covering[0].factor_at_arrival(arrival) {
                        Ok(factor) => candidate.frequency.push(FrequencyPrediction {
                            observation_id: observation_id(event, j),
                            time_s: point.time_s,
                            predicted_hz: event.emitted_frequency_hz * factor,
                        }),
                        Err(error) => issues.push(format!("{}: {error}", event.id)),
                    }
                }
            }
            if document.mix.doppler_timing {
                let cpas: Vec<_> = models
                    .iter()
                    .flat_map(|m| m.local_closest_approaches())
                    .filter(|c| {
                        !c.at_track_boundary
                            && c.arrival_time_s - start_from_origin >= event.start_s
                            && c.arrival_time_s - start_from_origin <= event.end_s
                    })
                    .collect();
                if cpas.len() != 1 {
                    issues.push(format!("{} needs one non-boundary local closest approach inside the reviewed interval; found {}",event.id,cpas.len()));
                } else {
                    candidate.doppler_timing.push(TimingPrediction {
                        observation_id: event.id.clone(),
                        predicted_arrival_utc_s: track.origin + cpas[0].arrival_time_s,
                    });
                }
            }
        }
        failures.push(issues);
        candidates.push(candidate);
    }
    let mut report = evidence_score::score_candidates(&evidence, &config, &candidates)?;
    report
        .limitations
        .splice(0..0, document.acoustic_source_warnings());
    for (candidate, issues) in report.candidates.iter_mut().zip(failures) {
        candidate.unavailable_reasons.extend(issues);
    }
    report.limitations.push("Workspace comparison uses the explicitly selected aircraft, emitted frequencies and one shared UTC clock adjustment; none are fitted. Receiver ellipsoid heights are taken from the supplied receiver list; terrain resolution, when selected, occurs before this scorer.".into());
    report.limitations.push("Aircraft timing is a uniquely supported non-boundary local closest approach inside each reviewed event. An approximate equal-angle grid covers the selected rectangle only; grid rank is not continuous-space uniqueness.".into());
    if atmosphere.is_some() {
        report.limitations.push("Atmospheric aircraft frequency uses the derivative of the same path-integrated travel time and a bounded emission-time inversion. Aircraft timing remains the geometric closest approach with profile propagation delay; it is not a refracted acoustic intensity peak.".into());
    }
    report.modalities = Some(crate::modality_diagnostics::summarize(
        &report,
        Some(document.map.bounds),
        &crate::modality_diagnostics::DiagnosticOptions::default(),
    )?);
    Ok(report)
}
