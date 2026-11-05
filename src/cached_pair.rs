//! Exact first-event reuse for one immutable first CPA.
//! The public scalar physics scorer remains the independent reference.
use crate::physics::{EventTrace, ModelResult, PairConfig, PairScore, PreparedTrack, Track};

pub(crate) const RESOURCE_ERROR_PREFIX: &str = "First-event cache allocation refused: ";
pub(crate) fn allocation_error(error: impl std::fmt::Display) -> String {
    format!("{RESOURCE_ERROR_PREFIX}{error}")
}
pub(crate) fn is_resource_error(error: &str) -> bool {
    error.starts_with(RESOURCE_ERROR_PREFIX)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStats {
    pub first_model_preparations: u64,
    pub completed_first_fits: u64,
    pub first_fit_hits: u64,
    pub first_factor_calls: u64,
    pub second_factor_calls: u64,
}

#[derive(Clone, Copy)]
struct Fit {
    clock_bits: u64,
    f0_hz: f64,
    mse: f64,
}

/// Immutable inputs and the first local-CPA index define the complete key.
/// The second model/CPA is never part of a cached first-event fit. Shift indices
/// remain distinct, even for equal values, preserving the original first tie.
pub struct FirstEventCache<'a> {
    first: &'a Track,
    observer: [f64; 3],
    events: [&'a EventTrace; 2],
    config: &'a PairConfig,
    first_cpa_index: usize,
    first_model: Option<PreparedTrack<'a>>,
    fits: Vec<Option<Fit>>,
    stats: CacheStats,
}

impl<'a> FirstEventCache<'a> {
    /// Binding allocates no model/cache and evaluates no source before the
    /// caller's ordinary pair reservation and second-track load have succeeded.
    pub fn new(
        first: &'a Track,
        observer: [f64; 3],
        events: [&'a EventTrace; 2],
        config: &'a PairConfig,
        first_cpa_index: usize,
    ) -> Self {
        Self {
            first,
            observer,
            events,
            config,
            first_cpa_index,
            first_model: None,
            fits: Vec::new(),
            stats: CacheStats::default(),
        }
    }

    #[cfg(test)]
    pub fn stats(&self) -> CacheStats {
        self.stats
    }

    pub fn score(
        &mut self,
        second_track: &Track,
        second_cpa_index: usize,
        mut checkpoint: impl FnMut() -> ModelResult<()>,
    ) -> ModelResult<PairScore> {
        let events = self.events;
        let config = self.config;
        let observer = self.observer;
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
        if self.first_model.is_none() {
            self.first_model = Some(PreparedTrack::new(self.first, observer, config.model)?);
            self.stats.first_model_preparations += 1;
        }
        checkpoint()?;
        let second = PreparedTrack::new(second_track, observer, config.model)?;
        checkpoint()?;
        let models = [self.first_model.as_ref().unwrap(), &second];
        let tracks = [self.first, second_track];
        let indices = [self.first_cpa_index, second_cpa_index];
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
        let cpa = [select(0)?, select(1)?];
        checkpoint()?;
        if cpa.iter().any(|c| c.at_track_boundary) {
            return Err(
                "A selected closest approach is at a track boundary; supply a complete pass".into(),
            );
        }
        if cpa[1].arrival_time_s <= cpa[0].arrival_time_s {
            return Err("The selected flights do not preserve the observed event order".into());
        }
        // This is a new internal wide-search API, whose existing preflight admits
        // at most1001 shifts. No public legacy scorer is restricted or redirected.
        if config.shared_shifts_s.len() > 1001 {
            return Err("First-event cache exceeds the explicit1001-shift memory bound".into());
        }
        if self.fits.is_empty() {
            self.fits
                .try_reserve_exact(config.shared_shifts_s.len())
                .map_err(allocation_error)?;
            self.fits.resize(config.shared_shifts_s.len(), None);
        }
        let mut best: Option<PairScore> = None;
        'shifts: for (shift_index, &shared_shift) in config.shared_shifts_s.iter().enumerate() {
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
            let cached = self.fits[shift_index];
            if cached.is_some_and(|fit| fit.clock_bits != clock.to_bits()) {
                return Err("Immutable first-event cache clock identity changed".into());
            }
            let mut first_factors = Vec::new();
            if cached.is_none() {
                first_factors = Vec::with_capacity(events[0].points.len());
                for (index, point) in events[0].points.iter().enumerate() {
                    if index.is_multiple_of(64) {
                        checkpoint()?;
                    }
                    self.stats.first_factor_calls += 1;
                    match models[0].factor_at_arrival(point.t_rec_s + clock) {
                        Ok(factor) => first_factors.push(factor),
                        Err(_) => continue 'shifts,
                    }
                }
                checkpoint()?;
            }
            let mut second_factors = Vec::with_capacity(events[1].points.len());
            for (index, point) in events[1].points.iter().enumerate() {
                if index.is_multiple_of(64) {
                    checkpoint()?;
                }
                self.stats.second_factor_calls += 1;
                match models[1].factor_at_arrival(point.t_rec_s + clock) {
                    Ok(factor) => second_factors.push(factor),
                    Err(_) => continue 'shifts,
                }
            }
            checkpoint()?;
            // A miss reaches this point only after BOTH complete factor arrays.
            // A hit is a previously completed first fit with identical inputs;
            // second-event support is still checked before that fit is used.
            let fit1 = if let Some(fit) = cached {
                checkpoint()?;
                self.stats.first_fit_hits += 1;
                (fit.f0_hz, fit.mse)
            } else {
                let fit = fit_frequency(events[0], &first_factors, &mut checkpoint)?;
                self.fits[shift_index] = Some(Fit {
                    clock_bits: clock.to_bits(),
                    f0_hz: fit.0,
                    mse: fit.1,
                });
                self.stats.completed_first_fits += 1;
                fit
            };
            let fit2 = fit_frequency(events[1], &second_factors, &mut checkpoint)?;
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
            "No shared clock shift provides complete track coverage for both fixed trace windows"
                .into()
        })
    }
}

// Byte-for-byte mathematical expressions and validation/error text copied from
// the frozen public scorer. The candidate does not approximate the tone sweep.
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

#[cfg(test)]
#[path = "cached_pair_tests.rs"]
mod tests;
