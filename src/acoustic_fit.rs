//! Audio-only descriptive straight-pass fitting. No flight paths, locations,
//! UTC clock, manually supplied CPA or emitted-frequency seed enter this module.
//!
//! With M=v/c, x=t_arrival-t_emission_CPA, and s=(d/v)*sqrt(1-M²),
//! exact retarded-time Doppler is f=A*(1-M*x/hypot(x,s)), A=f0/(1-M²).
//! Thus only the emission-CPA center and log(s) need nonlinear optimization;
//! linear least squares solves A and M, including the physical-domain edges.
use crate::physics::TracePoint;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FitSettings {
    pub sound_speed_mps: f64,
    pub speed_bounds_mps: [f64; 2],
    pub slant_bounds_m: [f64; 2],
    pub width_bounds_s: [f64; 2],
    pub center_padding_spans: f64,
    pub min_samples: usize,
    pub min_duration_s: f64,
    pub min_drop_hz: f64,
    pub min_drop_sigma: f64,
    pub profile_sigma_multiplier: f64,
    pub profile_grid_size: usize,
    pub max_iterations: usize,
}

impl Default for FitSettings {
    fn default() -> Self {
        Self {
            sound_speed_mps: 343.,
            speed_bounds_mps: [10., 280.],
            slant_bounds_m: [100., 30_000.],
            width_bounds_s: [0.5, 1000.],
            center_padding_spans: 1.,
            min_samples: 20,
            min_duration_s: 12.,
            min_drop_hz: 4.,
            min_drop_sigma: 4.,
            profile_sigma_multiplier: 1.,
            profile_grid_size: 65,
            max_iterations: 300,
        }
    }
}

impl FitSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !self.sound_speed_mps.is_finite()
            || self.sound_speed_mps <= 0.
            || !self.center_padding_spans.is_finite()
            || !(0.1..=5.).contains(&self.center_padding_spans)
            || !(8..=100_000).contains(&self.min_samples)
            || !(17..=161).contains(&self.profile_grid_size)
            || !(50..=2000).contains(&self.max_iterations)
        {
            return Err("Invalid acoustic fit domain or computation budget".into());
        }
        for value in [
            self.min_duration_s,
            self.min_drop_hz,
            self.min_drop_sigma,
            self.profile_sigma_multiplier,
        ] {
            if !value.is_finite() || value <= 0. {
                return Err("Acoustic duration, motion and sensitivity scales must be positive finite numbers".into());
            }
        }
        for range in [
            self.speed_bounds_mps,
            self.slant_bounds_m,
            self.width_bounds_s,
        ] {
            if range.iter().any(|v| !v.is_finite() || *v <= 0.) || range[0] >= range[1] {
                return Err(
                    "Acoustic fit physical and width ranges must be finite, positive and ordered"
                        .into(),
                );
            }
        }
        if self.speed_bounds_mps[1] >= self.sound_speed_mps * 0.95 {
            return Err("Acoustic fit speed domain must remain below Mach 0.95".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FitParameters {
    pub emitted_frequency_hz: f64,
    pub speed_mps: f64,
    pub slant_distance_m: f64,
    /// Arrival of sound emitted at geometric closest approach, not inflection.
    pub heard_cpa_recording_s: f64,
    pub emission_cpa_recording_s: f64,
    pub rmse_hz: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FitEnvelope {
    pub emitted_frequency_hz: [f64; 2],
    pub speed_mps: [f64; 2],
    pub slant_distance_m: [f64; 2],
    pub heard_cpa_recording_s: [f64; 2],
    pub basis: String,
}

impl FitEnvelope {
    fn new(p: &FitParameters) -> Self {
        Self {
            emitted_frequency_hz: [p.emitted_frequency_hz; 2],
            speed_mps: [p.speed_mps; 2],
            slant_distance_m: [p.slant_distance_m; 2],
            heard_cpa_recording_s: [p.heard_cpa_recording_s; 2],
            basis: "Union of sampled near-optimal profile models and four contiguous-time-block omission fits; engineering sensitivity, not a confidence interval or guaranteed bound".into(),
        }
    }
    fn include(&mut self, p: &FitParameters) {
        for (range, value) in [
            (&mut self.emitted_frequency_hz, p.emitted_frequency_hz),
            (&mut self.speed_mps, p.speed_mps),
            (&mut self.slant_distance_m, p.slant_distance_m),
            (&mut self.heard_cpa_recording_s, p.heard_cpa_recording_s),
        ] {
            range[0] = range[0].min(value);
            range[1] = range[1].max(value);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FitDiagnostics {
    pub sample_count: usize,
    pub observed_duration_s: f64,
    pub frequency_drop_hz: f64,
    pub nominal_frequency_sigma_hz: f64,
    pub maximum_observed_gap_s: f64,
    pub evaluated_models: usize,
    pub optimization_starts: usize,
    pub profile_rmse_limit_hz: Option<f64>,
    pub accepted_profile_models: usize,
    pub best_on_physical_boundary: bool,
    pub best_on_numerical_boundary: bool,
    pub profile_touches_physical_boundary: bool,
    pub profile_touches_numerical_boundary: bool,
    pub samples_before_heard_cpa: usize,
    pub samples_after_heard_cpa: usize,
    pub leave_time_block_out_fits: Vec<FitParameters>,
    pub leave_time_block_out_failures: usize,
    pub sensitivity_is_confidence_interval: bool,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FitReport {
    pub schema_version: u32,
    pub status: String,
    pub usable_for_search: bool,
    pub settings: FitSettings,
    pub fit: Option<FitParameters>,
    pub envelope: Option<FitEnvelope>,
    pub diagnostics: FitDiagnostics,
    pub limitations: Vec<String>,
}

#[derive(Clone, Copy)]
struct Model {
    q: [f64; 2],
    amplitude: f64,
    mach: f64,
    mse: f64,
}

struct Search<'a> {
    samples: &'a [(f64, f64)],
    settings: &'a FitSettings,
    span: f64,
    origin: f64,
    bounds: [[f64; 2]; 2],
    evaluations: usize,
}

impl Search<'_> {
    fn clamp(&self, mut q: [f64; 2]) -> [f64; 2] {
        for (value, bounds) in q.iter_mut().zip(self.bounds) {
            *value = value.clamp(bounds[0], bounds[1]);
        }
        q
    }

    fn evaluate(&mut self, q: [f64; 2]) -> Option<Model> {
        self.evaluations += 1;
        let center = q[0] * self.span;
        let width = q[1].exp();
        let n = self.samples.len() as f64;
        let mean_y = self.samples.iter().map(|p| p.1).sum::<f64>() / n;
        let mean_z = self
            .samples
            .iter()
            .map(|p| {
                let x = p.0 - center;
                x / x.hypot(width)
            })
            .sum::<f64>()
            / n;
        let mut var_z = 0.;
        let mut cov = 0.;
        for &(t, y) in self.samples {
            let x = t - center;
            let z = x / x.hypot(width) - mean_z;
            var_z += z * z;
            cov += z * (y - mean_y);
        }
        if !var_z.is_finite() || var_z < 1e-15 {
            return None;
        }
        let slope = cov / var_z;
        let intercept = mean_y - slope * mean_z;
        let c = self.settings.sound_speed_mps;
        // For a fixed width, distance is monotone in Mach. Its bounds can be
        // converted to Mach limits and handled by exact constrained LS.
        let [d_min, d_max] = self.settings.slant_bounds_m;
        let min_m = (self.settings.speed_bounds_mps[0] / c).max(d_min / (c * width).hypot(d_min));
        let max_m = (self.settings.speed_bounds_mps[1] / c).min(d_max / (c * width).hypot(d_max));
        if min_m > max_m {
            return None;
        }
        let unconstrained = -slope / intercept;
        let mut best = None;
        for mach in [min_m, max_m, unconstrained] {
            if !mach.is_finite() || mach < min_m || mach > max_m {
                continue;
            }
            let mut yg = 0.;
            let mut gg = 0.;
            for &(t, y) in self.samples {
                let x = t - center;
                let g = 1. - mach * x / x.hypot(width);
                yg += y * g;
                gg += g * g;
            }
            let amplitude = yg / gg;
            if !amplitude.is_finite() || amplitude <= 0. {
                continue;
            }
            let mse = self
                .samples
                .iter()
                .map(|&(t, y)| {
                    let x = t - center;
                    (y - amplitude * (1. - mach * x / x.hypot(width))).powi(2)
                })
                .sum::<f64>()
                / n;
            if mse.is_finite() && best.is_none_or(|b: Model| mse < b.mse) {
                best = Some(Model {
                    q,
                    amplitude,
                    mach,
                    mse,
                });
            }
        }
        best
    }

    fn parameters(&self, m: Model) -> FitParameters {
        let distance =
            self.settings.sound_speed_mps * m.mach * m.q[1].exp() / (1. - m.mach * m.mach).sqrt();
        let center = self.origin + m.q[0] * self.span;
        FitParameters {
            emitted_frequency_hz: m.amplitude * (1. - m.mach * m.mach),
            speed_mps: self.settings.sound_speed_mps * m.mach,
            slant_distance_m: distance,
            emission_cpa_recording_s: center,
            heard_cpa_recording_s: center + distance / self.settings.sound_speed_mps,
            rmse_hz: m.mse.sqrt(),
        }
    }

    fn physical_boundary(&self, m: Model) -> bool {
        let p = self.parameters(m);
        [
            (p.speed_mps, self.settings.speed_bounds_mps),
            (p.slant_distance_m, self.settings.slant_bounds_m),
        ]
        .iter()
        .any(|(x, b)| (*x - b[0]).abs().min((*x - b[1]).abs()) <= 1e-5 * (b[1] - b[0]))
    }

    fn numerical_boundary(&self, m: Model) -> bool {
        m.q.iter()
            .zip(self.bounds)
            .any(|(x, b)| (*x - b[0]).abs().min((*x - b[1]).abs()) <= 1e-5 * (b[1] - b[0]))
    }

    fn grid(&mut self, n: usize, bounds: [[f64; 2]; 2]) -> Vec<Model> {
        let mut models = Vec::new();
        for i in 0..n {
            for j in 0..n {
                let q = self.clamp([
                    bounds[0][0] + (bounds[0][1] - bounds[0][0]) * i as f64 / (n - 1) as f64,
                    bounds[1][0] + (bounds[1][1] - bounds[1][0]) * j as f64 / (n - 1) as f64,
                ]);
                if let Some(m) = self.evaluate(q) {
                    models.push(m);
                }
            }
        }
        models
    }

    fn optimize(&mut self, seed: Model) -> Model {
        let shifts = [0.04, 0.14];
        let mut simplex = vec![seed];
        for dimension in 0..2 {
            let mut q = seed.q;
            q[dimension] += if q[dimension] + shifts[dimension] <= self.bounds[dimension][1] {
                shifts[dimension]
            } else {
                -shifts[dimension]
            };
            simplex.push(self.evaluate(self.clamp(q)).unwrap_or(seed));
        }
        for _ in 0..self.settings.max_iterations {
            simplex.sort_by(|a, b| a.mse.total_cmp(&b.mse));
            let spread = (0..2)
                .map(|k| {
                    (simplex[2].q[k] - simplex[0].q[k])
                        .abs()
                        .max((simplex[1].q[k] - simplex[0].q[k]).abs())
                })
                .fold(0., f64::max);
            if spread < 1e-9 {
                break;
            }
            let centroid = [
                (simplex[0].q[0] + simplex[1].q[0]) / 2.,
                (simplex[0].q[1] + simplex[1].q[1]) / 2.,
            ];
            let transform = |factor: f64| {
                [
                    centroid[0] + factor * (centroid[0] - simplex[2].q[0]),
                    centroid[1] + factor * (centroid[1] - simplex[2].q[1]),
                ]
            };
            let reflected = self.evaluate(self.clamp(transform(1.)));
            if let Some(reflected) = reflected {
                if reflected.mse < simplex[0].mse {
                    let expanded = self.evaluate(self.clamp(transform(2.)));
                    simplex[2] = expanded
                        .filter(|m| m.mse < reflected.mse)
                        .unwrap_or(reflected);
                    continue;
                }
                if reflected.mse < simplex[1].mse {
                    simplex[2] = reflected;
                    continue;
                }
                let factor = if reflected.mse < simplex[2].mse {
                    0.5
                } else {
                    -0.5
                };
                let contracted = self.evaluate(self.clamp(transform(factor)));
                if let Some(contracted) =
                    contracted.filter(|m| m.mse < simplex[2].mse.min(reflected.mse))
                {
                    simplex[2] = contracted;
                    continue;
                }
            }
            for i in 1..3 {
                let q = [
                    (simplex[0].q[0] + simplex[i].q[0]) / 2.,
                    (simplex[0].q[1] + simplex[i].q[1]) / 2.,
                ];
                simplex[i] = self.evaluate(self.clamp(q)).unwrap_or(simplex[0]);
            }
        }
        simplex
            .into_iter()
            .min_by(|a, b| a.mse.total_cmp(&b.mse))
            .unwrap()
    }

    fn multistart(&mut self) -> Option<(Model, usize)> {
        let mut coarse = self.grid(25, self.bounds);
        coarse.sort_by(|a, b| a.mse.total_cmp(&b.mse));
        let mut seeds = Vec::<Model>::new();
        for model in coarse {
            if seeds
                .iter()
                .all(|s| (s.q[0] - model.q[0]).hypot((s.q[1] - model.q[1]) * 0.25) > 0.08)
            {
                seeds.push(model);
                if seeds.len() == 12 {
                    break;
                }
            }
        }
        let count = seeds.len();
        seeds
            .into_iter()
            .map(|m| self.optimize(m))
            .min_by(|a, b| a.mse.total_cmp(&b.mse))
            .map(|m| (m, count))
    }
}

/// Evaluate the same exact arrival-time model for diagnostics. This is not a
/// moving-aircraft historical-trajectory model or a geolocation computation.
pub fn predict_frequency(
    p: &FitParameters,
    time_s: f64,
    sound_speed_mps: f64,
) -> Result<f64, String> {
    if !time_s.is_finite()
        || !sound_speed_mps.is_finite()
        || sound_speed_mps <= 0.
        || !p.emitted_frequency_hz.is_finite()
        || p.emitted_frequency_hz <= 0.
        || !p.speed_mps.is_finite()
        || p.speed_mps <= 0.
        || p.speed_mps >= sound_speed_mps
        || !p.slant_distance_m.is_finite()
        || p.slant_distance_m <= 0.
        || !p.emission_cpa_recording_s.is_finite()
    {
        return Err("Invalid acoustic prediction parameters".into());
    }
    let mach = p.speed_mps / sound_speed_mps;
    let width = p.slant_distance_m / p.speed_mps * (1. - mach * mach).sqrt();
    let x = time_s - p.emission_cpa_recording_s;
    let frequency = p.emitted_frequency_hz / (1. - mach * mach) * (1. - mach * x / x.hypot(width));
    if !frequency.is_finite() {
        return Err("Acoustic prediction overflow".into());
    }
    Ok(frequency)
}

fn median(values: impl Iterator<Item = f64>) -> f64 {
    let mut v = values.collect::<Vec<_>>();
    v.sort_by(f64::total_cmp);
    if v.len().is_multiple_of(2) {
        (v[v.len() / 2 - 1] + v[v.len() / 2]) / 2.
    } else {
        v[v.len() / 2]
    }
}

/// Fit fixed observed samples without filling gaps or using contrast weights.
/// Invalid numeric inputs are errors; insufficient/degenerate evidence is a
/// completed diagnostic report with `usable_for_search=false`.
pub fn fit(
    points: &[TracePoint],
    frequency_sigma_hz: f64,
    settings: &FitSettings,
) -> Result<FitReport, String> {
    settings.validate()?;
    if points.len() < 3
        || points.len() > 100_000
        || !frequency_sigma_hz.is_finite()
        || frequency_sigma_hz <= 0.
        || points.iter().any(|p| {
            !p.t_rec_s.is_finite()
                || p.t_rec_s < 0.
                || !p.frequency_hz.is_finite()
                || p.frequency_hz <= 0.
        })
        || points.windows(2).any(|p| p[0].t_rec_s >= p[1].t_rec_s)
    {
        return Err("Acoustic fitting needs ordered finite positive measured samples, a positive frequency scale and at most 100000 points".into());
    }
    let span = points.last().unwrap().t_rec_s - points[0].t_rec_s;
    let endpoint_count = (points.len() / 5).max(1);
    let drop = median(points[..endpoint_count].iter().map(|p| p.frequency_hz))
        - median(
            points[points.len() - endpoint_count..]
                .iter()
                .map(|p| p.frequency_hz),
        );
    let mut report=FitReport {schema_version:1,status:"insufficient_acoustic_information".into(),usable_for_search:false,settings:settings.clone(),fit:None,envelope:None,
        diagnostics:FitDiagnostics {sample_count:points.len(),observed_duration_s:span,frequency_drop_hz:drop,nominal_frequency_sigma_hz:frequency_sigma_hz,maximum_observed_gap_s:points.windows(2).map(|p|p[1].t_rec_s-p[0].t_rec_s).fold(0.,f64::max),evaluated_models:0,optimization_starts:0,profile_rmse_limit_hz:None,accepted_profile_models:0,best_on_physical_boundary:false,best_on_numerical_boundary:false,profile_touches_physical_boundary:false,profile_touches_numerical_boundary:false,samples_before_heard_cpa:0,samples_after_heard_cpa:0,leave_time_block_out_fits:Vec::new(),leave_time_block_out_failures:0,sensitivity_is_confidence_interval:false,reasons:Vec::new()},
        limitations:vec!["A straight, constant-speed, stable-tone model is a descriptive approximation; curved paths, RPM changes and atmosphere can bias its parameters".into(),"Acoustic geometry estimates are correlated and engineering sensitivity ranges are not confidence intervals, exhaustive bounds or calibrated probabilities".into(),"Heard CPA is arrival of sound emitted at geometric closest approach, not the inflection time of the observed frequency curve".into(),"Times remain relative to this recording; absolute recording UTC, aircraft identity and observer location cannot be determined from this fit".into(),"Only supplied measured samples are fitted with equal weights; no missing segment is reconstructed or claimed observed".into()]};
    if points.len() < settings.min_samples {
        report
            .diagnostics
            .reasons
            .push("Too few measured samples for four-parameter acoustic fitting".into());
    }
    if span < settings.min_duration_s {
        report
            .diagnostics
            .reasons
            .push("Measured duration is too short".into());
    }
    if drop
        < settings
            .min_drop_hz
            .max(settings.min_drop_sigma * frequency_sigma_hz)
    {
        report.diagnostics.reasons.push("No sufficiently strong downward frequency motion relative to the declared measurement scale".into());
    }
    if !report.diagnostics.reasons.is_empty() {
        return Ok(report);
    }
    let samples = points
        .iter()
        .map(|p| (p.t_rec_s - points[0].t_rec_s, p.frequency_hz))
        .collect::<Vec<_>>();
    let bounds = [
        [
            -settings.center_padding_spans,
            1. + settings.center_padding_spans,
        ],
        [
            settings.width_bounds_s[0].ln(),
            settings.width_bounds_s[1].ln(),
        ],
    ];
    let mut search = Search {
        samples: &samples,
        settings,
        span,
        origin: points[0].t_rec_s,
        bounds,
        evaluations: 0,
    };
    let Some((best, starts)) = search.multistart() else {
        report
            .diagnostics
            .reasons
            .push("No finite model in the declared acoustic domain".into());
        return Ok(report);
    };
    let parameters = search.parameters(best);
    let mut envelope = FitEnvelope::new(&parameters);
    report.diagnostics.optimization_starts = starts;
    report.diagnostics.best_on_physical_boundary = search.physical_boundary(best);
    report.diagnostics.best_on_numerical_boundary = search.numerical_boundary(best);
    let limit_squared = best.mse + (settings.profile_sigma_multiplier * frequency_sigma_hz).powi(2);
    report.diagnostics.profile_rmse_limit_hz = Some(limit_squared.sqrt());
    // Full domain plus dense neighborhood. The profile threshold and domains
    // are declared settings, not chosen from a known location or aircraft.
    let mut profile = search.grid(settings.profile_grid_size, bounds);
    profile.extend(search.grid(
        settings.profile_grid_size,
        [
            [best.q[0] - 0.35, best.q[0] + 0.35],
            [best.q[1] - 0.8, best.q[1] + 0.8],
        ],
    ));
    profile.push(best);
    for model in profile.into_iter().filter(|m| m.mse <= limit_squared) {
        envelope.include(&search.parameters(model));
        report.diagnostics.accepted_profile_models += 1;
        report.diagnostics.profile_touches_physical_boundary |= search.physical_boundary(model);
        report.diagnostics.profile_touches_numerical_boundary |= search.numerical_boundary(model);
    }
    // Removing contiguous time blocks probes dependence on partial support;
    // this is a stability diagnostic, never a statistical jackknife interval.
    for block in 0..4 {
        let subset = samples
            .iter()
            .copied()
            .filter(|(t, _)| {
                let bucket = ((*t / span * 4.).floor() as usize).min(3);
                bucket != block
            })
            .collect::<Vec<_>>();
        if subset.len() < settings.min_samples {
            report.diagnostics.leave_time_block_out_failures += 1;
            continue;
        }
        let mut omitted = Search {
            samples: &subset,
            settings,
            span,
            origin: search.origin,
            bounds,
            evaluations: 0,
        };
        if let Some((model, _)) = omitted.multistart() {
            let p = omitted.parameters(model);
            envelope.include(&p);
            if omitted.physical_boundary(model) || omitted.numerical_boundary(model) {
                report.diagnostics.leave_time_block_out_failures += 1;
            }
            report.diagnostics.leave_time_block_out_fits.push(p);
        } else {
            report.diagnostics.leave_time_block_out_failures += 1;
        }
        search.evaluations += omitted.evaluations;
    }
    report.diagnostics.evaluated_models = search.evaluations;
    report.diagnostics.samples_before_heard_cpa = points
        .iter()
        .filter(|p| p.t_rec_s < parameters.heard_cpa_recording_s)
        .count();
    report.diagnostics.samples_after_heard_cpa = points
        .iter()
        .filter(|p| p.t_rec_s > parameters.heard_cpa_recording_s)
        .count();
    let d = &mut report.diagnostics;
    if d.best_on_physical_boundary || d.best_on_numerical_boundary {
        d.reasons
            .push("Best fit reaches the declared physical or numerical domain boundary".into());
    }
    if d.profile_touches_physical_boundary || d.profile_touches_numerical_boundary {
        d.reasons.push(
            "Near-optimal models reach a fit-domain boundary; the acoustic range is not closed"
                .into(),
        );
    }
    if d.leave_time_block_out_failures > 0 {
        d.reasons
            .push("Removing a measured time block produces an unsupported or boundary fit".into());
    }
    let minimum_side = (points.len() / 10).max(5);
    if d.samples_before_heard_cpa < minimum_side || d.samples_after_heard_cpa < minimum_side {
        d.reasons
            .push("Closest-approach timing lacks measured support on both sides".into());
    }
    if parameters.rmse_hz > (3. * frequency_sigma_hz).max(0.03 * drop) {
        d.reasons
            .push("Residuals are large relative to measurement scale and observed motion".into());
    }
    if envelope.emitted_frequency_hz[1] - envelope.emitted_frequency_hz[0]
        > 0.3 * parameters.emitted_frequency_hz
        || envelope.speed_mps[1] - envelope.speed_mps[0] > 0.9 * parameters.speed_mps
        || envelope.slant_distance_m[1] - envelope.slant_distance_m[0]
            > 1.5 * parameters.slant_distance_m
        || envelope.heard_cpa_recording_s[1] - envelope.heard_cpa_recording_s[0] > 0.5 * span
    {
        d.reasons
            .push("Parameter sensitivity is too broad for a useful bounded search prior".into());
    }
    report.usable_for_search = d.reasons.is_empty();
    report.status = if report.usable_for_search {
        "usable_audio_priors"
    } else {
        "weakly_identified_acoustic_fit"
    }
    .into();
    report.fit = Some(parameters);
    report.envelope = Some(envelope);
    Ok(report)
}
