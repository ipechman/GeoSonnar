//! Explicit bounded regional thunder-envelope comparisons, separate from the
//! reviewed event scorer. Every result is an unverified hypothesis.
//!
//! Equal source ENERGY is attenuated by inverse square. A finite causal energy
//! response is integrated over sample bins, summed, then square-rooted into
//! relative RMS. This is an assumed point-source model, not a measured thunder
//! transfer function. Provider current/intensity/multiplicity never supplies
//! acoustic strength. The declared source interval is queried support, not a
//! certificate that every lightning discharge was detected.

use crate::{
    evidence_score::{LightningAltitudeDatum, LightningCatalog, ObserverLocation},
    lightning_source::{self, LightningInterpretationDiagnostic, LightningSourcePolicy},
    physics::geodetic_to_ecef,
    thunder::ThunderEnvelope,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};

pub const MAX_OBSERVATION_S: f64 = 300.0;
pub const MAX_OFFSET_SPAN_S: f64 = 3600.0;
pub const MAX_SAMPLES: usize = 8192;
pub const MAX_RECEIVERS: usize = 10_000;
pub const MAX_WORK_UNITS: u64 = 500_000_000;
pub const MAX_MEMORY_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_KEEP_BEST: usize = 40;
const MIN_CADENCE_S: f64 = 0.05;
const MAX_CADENCE_S: f64 = 5.0;
const MAX_FFT_SIZE: usize = 262_144;
const NUMERIC_SCORE_TOLERANCE: f64 = 1e-7;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ObservedEnvelope {
    pub recording_sha256: String,
    pub channel: usize,
    pub envelope: ThunderEnvelope,
    /// Explicit analyst selection; neither this nor a match identifies thunder.
    pub reviewed_selection_note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeReceiver {
    pub id: String,
    pub location: ObserverLocation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeModel {
    pub sound_speed_mps: f64,
    pub fallback_strike_ellipsoid_height_m: Option<f64>,
    pub minimum_distance_m: f64,
    pub maximum_distance_m: f64,
    pub kernel_decay_s: f64,
    pub kernel_duration_s: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeLimits {
    pub max_work_units: u64,
    pub max_memory_bytes: u64,
}
impl Default for EnvelopeLimits {
    fn default() -> Self {
        Self {
            max_work_units: 200_000_000,
            max_memory_bytes: MAX_MEMORY_BYTES,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeSearchPlan {
    pub schema_version: u32,
    pub catalog_sha256: String,
    /// Explicitly declared complete queried interval, half-open Unix UTC.
    /// This is NOT an assertion of complete physical lightning detection.
    pub catalog_utc_interval: [f64; 2],
    pub receivers: Vec<EnvelopeReceiver>,
    pub nominal_recording_start_utc_s: f64,
    /// Positive means the first decoded recording sample occurred later.
    pub min_clock_offset_s: f64,
    pub max_clock_offset_s: f64,
    pub model: EnvelopeModel,
    pub source_policy: Option<LightningSourcePolicy>,
    pub keep_best: usize,
    pub limits: EnvelopeLimits,
}
impl EnvelopeSearchPlan {
    pub fn with_observation(self, observation: ObservedEnvelope) -> EnvelopeSearchRequest {
        EnvelopeSearchRequest {
            plan: self,
            observation,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeSearchRequest {
    pub plan: EnvelopeSearchPlan,
    pub observation: ObservedEnvelope,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnvelopeEstimate {
    pub request_sha256: String,
    pub catalog_data_sha256: String,
    pub sample_count: usize,
    pub cadence_s: f64,
    pub receiver_count: usize,
    pub offsets_per_receiver: usize,
    pub total_offsets: u64,
    pub source_rows: usize,
    pub predicted_sample_count: usize,
    pub fft_size: usize,
    pub estimated_work_units: u64,
    pub estimated_memory_bytes: u64,
    pub required_catalog_utc_interval: [f64; 2],
    pub source_interpretation: Option<LightningInterpretationDiagnostic>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EnvelopeProgress {
    pub completed_receivers: usize,
    pub total_receivers: usize,
    pub evaluated_offsets: u64,
    pub total_offsets: u64,
    pub zero_energy_offsets: u64,
    pub numerically_unresolved_offsets: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnvelopeHypothesis {
    pub request_sha256: String,
    pub catalog_data_sha256: String,
    pub receiver_index: usize,
    pub receiver_id: String,
    pub observer: ObserverLocation,
    pub offset_index: usize,
    pub clock_offset_s: f64,
    pub recording_start_utc_s: f64,
    pub cosine_similarity: f64,
    pub clock_at_boundary: bool,
    /// Rows supplying any energy over the whole requested clock range, not an
    /// asserted event association or count confined to this particular offset.
    pub contributing_source_rows: usize,
    pub distance_excluded_source_rows: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnvelopeReceiverResult {
    pub receiver_index: usize,
    pub receiver_id: String,
    pub best: Option<EnvelopeHypothesis>,
    pub rankable_offsets: usize,
    pub zero_energy_offsets: usize,
    pub numerically_unresolved_offsets: usize,
    pub contributing_source_rows: usize,
    pub distance_excluded_source_rows: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnvelopeReport {
    pub schema_version: u32,
    pub estimate: EnvelopeEstimate,
    pub progress: EnvelopeProgress,
    pub hypotheses: Vec<EnvelopeHypothesis>,
    pub receivers: Vec<EnvelopeReceiverResult>,
    pub complete: bool,
    pub cancelled: bool,
    pub location_verified: bool,
    pub independent_field_validation: bool,
    pub numerical_ordering_tolerance: f64,
    pub retained_near_tie_count: usize,
    pub retention_boundary_ambiguous: bool,
    pub exact_top_k_guaranteed: bool,
    pub score_meaning: String,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnvelopeOverlay {
    pub request_sha256: String,
    pub catalog_data_sha256: String,
    pub hypothesis: EnvelopeHypothesis,
    pub times_s: Vec<f64>,
    pub observed_rms: Vec<f64>,
    pub predicted_relative_rms: Vec<f64>,
    pub model: EnvelopeModel,
    pub location_verified: bool,
}

fn hash_ok(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn bounded_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max
}
fn valid_height(value: f64) -> bool {
    value.is_finite() && (-1000.0..=100_000.0).contains(&value)
}
struct HashWriter(Sha256);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn hash<T: Serialize>(value: &T) -> Result<String, String> {
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", writer.0.finalize()))
}
fn checked_add(a: u64, b: u64) -> Result<u64, String> {
    a.checked_add(b)
        .ok_or_else(|| "Envelope resource estimate overflowed".into())
}
fn checked_mul(a: u64, b: u64) -> Result<u64, String> {
    a.checked_mul(b)
        .ok_or_else(|| "Envelope resource estimate overflowed".into())
}

/// Validates and estimates before any FFT allocation or scoring. All values are
/// explicit; no date, area, source strength or response kernel is fitted here.
pub fn preflight(
    request: &EnvelopeSearchRequest,
    catalog: &LightningCatalog,
) -> Result<EnvelopeEstimate, String> {
    let plan = &request.plan;
    let observation = &request.observation;
    let samples = &observation.envelope.samples;
    if plan.schema_version != 1
        || !hash_ok(&observation.recording_sha256)
        || observation.channel >= 64
        || !bounded_text(&observation.reviewed_selection_note, 4000)
        || !bounded_text(&observation.envelope.method, 4000)
        || !(2..=MAX_SAMPLES).contains(&samples.len())
    {
        return Err("Envelope request needs schema1, recording hash, reviewed selection/method, channel0..63 and2..8192 samples".into());
    }
    let low = observation.envelope.band_low_hz;
    let high = observation.envelope.band_high_hz;
    if !low.is_finite() || !high.is_finite() || low < 0.0 || high <= low || high > 2000.0 {
        return Err(
            "Envelope must declare finite actual frequency-bin bounds within0..2000Hz".into(),
        );
    }
    let cadence = samples[1].time_s - samples[0].time_s;
    if !cadence.is_finite() || !(MIN_CADENCE_S - 1e-9..=MAX_CADENCE_S + 1e-9).contains(&cadence) {
        return Err("Envelope requires uniform complete sampling at0.05..5 seconds; no masks or gap filling".into());
    }
    let mut peak = 0.0_f64;
    for (index, sample) in samples.iter().enumerate() {
        if !sample.time_s.is_finite()
            || !(0.0..=86401.0).contains(&sample.time_s)
            || !sample.rms.is_finite()
            || sample.rms < 0.0
            || (sample.time_s - (samples[0].time_s + index as f64 * cadence)).abs() > cadence * 1e-7
        {
            return Err("Envelope samples must be finite, nonnegative and uniformly spaced without masks, gaps or reordering".into());
        }
        peak = peak.max(sample.rms);
    }
    if peak == 0.0 {
        return Err("Observed envelope has zero energy; silence cannot produce a match".into());
    }
    if samples.len() as f64 * cadence > MAX_OBSERVATION_S + 1e-7 {
        return Err(
            "Envelope comparison permits at most300 seconds of complete sample-bin support".into(),
        );
    }
    if plan.receivers.is_empty()
        || plan.receivers.len() > MAX_RECEIVERS
        || !(1..=MAX_KEEP_BEST).contains(&plan.keep_best)
    {
        return Err(
            "Envelope comparison needs1..10000 explicit receivers and1..40 retained hypotheses"
                .into(),
        );
    }
    let mut receiver_ids = BTreeSet::new();
    for receiver in &plan.receivers {
        if !bounded_text(&receiver.id, 512)
            || !receiver_ids.insert(&receiver.id)
            || !valid_height(receiver.location.ellipsoid_height_m)
        {
            return Err("Receiver IDs must be unique and bounded; explicit ellipsoid heights must be within-1000..100000m".into());
        }
        geodetic_to_ecef(
            receiver.location.latitude_deg,
            receiver.location.longitude_deg,
            receiver.location.ellipsoid_height_m,
        )?;
    }
    let model = &plan.model;
    if !model.sound_speed_mps.is_finite()
        || !(250.0..=400.0).contains(&model.sound_speed_mps)
        || !model.minimum_distance_m.is_finite()
        || !(1.0..=500_000.0).contains(&model.minimum_distance_m)
        || !model.maximum_distance_m.is_finite()
        || model.maximum_distance_m < model.minimum_distance_m
        || model.maximum_distance_m > 500_000.0
        || !model.kernel_decay_s.is_finite()
        || model.kernel_decay_s < cadence
        || !model.kernel_duration_s.is_finite()
        || model.kernel_duration_s < model.kernel_decay_s
        || model.kernel_duration_s > 120.0
        || model.kernel_duration_s / model.kernel_decay_s > 32.0
        || model
            .fallback_strike_ellipsoid_height_m
            .is_some_and(|h| !valid_height(h))
    {
        return Err("Explicit envelope model requires250..400m/s,1..500000m ordered distance bounds, cadence<=decay<=duration<=120s (duration/decay<=32), and valid optional source height".into());
    }
    if !plan.nominal_recording_start_utc_s.is_finite()
        || !(0.0..253_402_300_800.0).contains(&plan.nominal_recording_start_utc_s)
        || !plan.min_clock_offset_s.is_finite()
        || !plan.max_clock_offset_s.is_finite()
        || plan.min_clock_offset_s > plan.max_clock_offset_s
        || plan.max_clock_offset_s - plan.min_clock_offset_s > MAX_OFFSET_SPAN_S
        || !(0.0..253_402_300_800.0)
            .contains(&(plan.nominal_recording_start_utc_s + plan.min_clock_offset_s))
        || !(0.0..253_402_300_800.0)
            .contains(&(plan.nominal_recording_start_utc_s + plan.max_clock_offset_s))
    {
        return Err("Recording UTC and ordered clock bounds must be finite with at most3600 seconds of offset span".into());
    }
    let offset_steps = (plan.max_clock_offset_s - plan.min_clock_offset_s) / cadence;
    if (offset_steps - offset_steps.round()).abs() > 1e-6 {
        return Err("Clock span must be an integer multiple of the measured envelope cadence; endpoints are never silently rounded".into());
    }
    let offsets = offset_steps.round() as usize + 1;
    let predicted_count = samples
        .len()
        .checked_add(offsets - 1)
        .ok_or("Envelope sample count overflowed")?;
    let fft_size = predicted_count
        .checked_add(samples.len() - 1)
        .and_then(usize::checked_next_power_of_two)
        .ok_or("Envelope FFT size overflowed")?;
    if fft_size > MAX_FFT_SIZE {
        return Err("Envelope FFT exceeds262144 samples; explicitly coarsen sampling or narrow the declared interval".into());
    }
    let required = [
        plan.nominal_recording_start_utc_s + plan.min_clock_offset_s + samples[0].time_s
            - cadence / 2.0
            - model.maximum_distance_m / model.sound_speed_mps
            - model.kernel_duration_s,
        plan.nominal_recording_start_utc_s
            + plan.max_clock_offset_s
            + samples[samples.len() - 1].time_s
            + cadence / 2.0,
    ];
    let declared = plan.catalog_utc_interval;
    if required
        .iter()
        .chain(declared.iter())
        .any(|v| !v.is_finite() || !(0.0..253_402_300_800.0).contains(v))
        || declared[0] >= declared[1]
        || declared[0] > required[0]
        || declared[1] < required[1]
    {
        return Err(format!(
            "Declared catalog UTC interval must cover every clock/receive bin and full distance/kernel lookback, including [{:.6},{:.6}); query completion is not lightning completeness",
            required[0], required[1]
        ));
    }
    if catalog.schema_version != 1
        || !hash_ok(&catalog.input_sha256)
        || plan.catalog_sha256 != catalog.input_sha256
        || !bounded_text(&catalog.provenance.provider, 512)
        || !bounded_text(&catalog.provenance.source, 8192)
        || catalog
            .provenance
            .notes
            .as_ref()
            .is_some_and(|s| s.len() > 20_000)
        || catalog
            .provenance
            .license
            .as_ref()
            .is_some_and(|s| s.len() > 2048)
    {
        return Err(
            "Envelope requires an unchanged source catalog hash and bounded retained provenance"
                .into(),
        );
    }
    lightning_source::validate_policy(catalog, plan.source_policy.as_ref(), model.kernel_decay_s)
        .map_err(|e| format!("Envelope source interpretation (kernel decay must be at least the declared representation allowance; not calibrated accuracy): {e}"))?;
    if let Some(policy) = &plan.source_policy {
        policy.validate_shape()?;
    }
    for strike in &catalog.strikes {
        if strike.time_utc_s < declared[0] || strike.time_utc_s >= declared[1] {
            return Err(
                "Catalog rows fall outside the explicitly declared catalog UTC interval".into(),
            );
        }
        let known_height = catalog.provenance.altitude_datum
            == LightningAltitudeDatum::Wgs84Ellipsoid
            && strike.altitude_m.is_some();
        if !known_height && model.fallback_strike_ellipsoid_height_m.is_none() {
            return Err("Each lightning row needs a known ellipsoid height or an explicit fallback source-height assumption".into());
        }
    }
    let log = fft_size.ilog2() as u64;
    let receiver_count = plan.receivers.len() as u64;
    let kernel_bins = (model.kernel_duration_s / cadence).ceil() as u64 + 2;
    let fft_work = checked_mul(3 * fft_size as u64, log + 1)?;
    let synthesis_work = checked_mul(catalog.strikes.len() as u64, kernel_bins + 64)?;
    let range_work = checked_mul(offsets as u64, log * 2 + plan.keep_best as u64 * 2 + 8)?;
    let verification_work = checked_mul(plan.keep_best as u64 * samples.len() as u64, 4)?;
    let work = checked_mul(
        receiver_count,
        checked_add(
            checked_add(fft_work, synthesis_work)?,
            checked_add(range_work, verification_work)?,
        )?,
    )?;
    // Includes two complex FFT arrays, RMS curve and nonnegative energy tree,
    // prepared sources, retained candidates and an allowance for loaded inputs.
    let memory = checked_add(
        1_048_576 + plan.keep_best as u64 * 2048,
        checked_add(
            checked_mul(fft_size as u64, 64)?,
            checked_add(
                checked_mul(catalog.strikes.len() as u64, 1024)?,
                checked_add(
                    checked_mul(samples.len() as u64, 64)?,
                    checked_mul(receiver_count, 2048)?,
                )?,
            )?,
        )?,
    )?;
    if plan.limits.max_work_units == 0
        || plan.limits.max_work_units > MAX_WORK_UNITS
        || plan.limits.max_memory_bytes == 0
        || plan.limits.max_memory_bytes > MAX_MEMORY_BYTES
        || work > plan.limits.max_work_units
        || memory > plan.limits.max_memory_bytes
    {
        let (required, allowed) =
            crate::data_size::format_gib_pair(memory, plan.limits.max_memory_bytes);
        return Err(format!(
            "Envelope preflight needs {work} bounded work units ({} allowed) and {required} estimated memory ({allowed} allowed); declared limits must cover both within hard caps of {MAX_WORK_UNITS} units and {}",
            plan.limits.max_work_units,
            crate::data_size::format_gib(MAX_MEMORY_BYTES)
        ));
    }
    Ok(EnvelopeEstimate {
        request_sha256: hash(request)?,
        catalog_data_sha256: hash(&(&catalog.provenance, &catalog.strikes))?,
        sample_count: samples.len(),
        cadence_s: cadence,
        receiver_count: plan.receivers.len(),
        offsets_per_receiver: offsets,
        total_offsets: checked_mul(receiver_count, offsets as u64)?,
        source_rows: catalog.strikes.len(),
        predicted_sample_count: predicted_count,
        fft_size,
        estimated_work_units: work,
        estimated_memory_bytes: memory,
        required_catalog_utc_interval: required,
        source_interpretation: lightning_source::interpretation(
            catalog,
            plan.source_policy.as_ref(),
        ),
    })
}

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f64,
    im: f64,
}

/// Private, bounded linear-correlation transform. Cancellation is observed
/// inside long stages as well as between receivers; no detached work exists.
fn fft(values: &mut [Complex], inverse: bool, cancelled: &AtomicBool) -> bool {
    let count = values.len();
    let mut reversed = 0;
    for index in 1..count {
        if index & 4095 == 0 && cancelled.load(Ordering::Relaxed) {
            return false;
        }
        let mut bit = count >> 1;
        while reversed & bit != 0 {
            reversed ^= bit;
            bit >>= 1;
        }
        reversed ^= bit;
        if index < reversed {
            values.swap(index, reversed);
        }
    }
    let mut length = 2;
    while length <= count {
        if cancelled.load(Ordering::Relaxed) {
            return false;
        }
        let angle = if inverse { 2.0 } else { -2.0 } * std::f64::consts::PI / length as f64;
        let (step_im, step_re) = angle.sin_cos();
        for start in (0..count).step_by(length) {
            let (mut phase_re, mut phase_im) = (1.0, 0.0);
            for index in 0..length / 2 {
                if index & 4095 == 0 && cancelled.load(Ordering::Relaxed) {
                    return false;
                }
                let left = values[start + index];
                let right = values[start + index + length / 2];
                let re = right.re * phase_re - right.im * phase_im;
                let im = right.re * phase_im + right.im * phase_re;
                values[start + index] = Complex {
                    re: left.re + re,
                    im: left.im + im,
                };
                values[start + index + length / 2] = Complex {
                    re: left.re - re,
                    im: left.im - im,
                };
                (phase_re, phase_im) = (
                    phase_re * step_re - phase_im * step_im,
                    phase_re * step_im + phase_im * step_re,
                );
            }
        }
        length *= 2;
    }
    if inverse {
        for value in values {
            value.re /= count as f64;
            value.im /= count as f64;
        }
    }
    true
}

/// Addition-only range sums avoid loss of quiet window energy when subtracting
/// two large prefix sums. No cancellation or negative squared norms occur.
struct EnergyTree {
    base: usize,
    values: Vec<f64>,
}
impl EnergyTree {
    fn new(samples: &[f64]) -> Self {
        let base = samples.len().next_power_of_two();
        let mut values = vec![0.0; base * 2];
        for (index, value) in samples.iter().enumerate() {
            values[base + index] = value * value;
        }
        for index in (1..base).rev() {
            values[index] = values[2 * index] + values[2 * index + 1];
        }
        Self { base, values }
    }
    fn sum(&self, start: usize, end: usize) -> f64 {
        let (mut left, mut right) = (self.base + start, self.base + end);
        let mut total = 0.0;
        while left < right {
            if left & 1 != 0 {
                total += self.values[left];
                left += 1;
            }
            if right & 1 != 0 {
                right -= 1;
                total += self.values[right];
            }
            left /= 2;
            right /= 2;
        }
        total
    }
}

struct Source {
    xyz: [f64; 3],
    time_relative_s: f64,
}
fn prepare_sources(
    request: &EnvelopeSearchRequest,
    catalog: &LightningCatalog,
    cancelled: &AtomicBool,
) -> Result<Option<Vec<Source>>, String> {
    let mut sources = Vec::with_capacity(catalog.strikes.len());
    for (index, strike) in catalog.strikes.iter().enumerate() {
        if index & 255 == 0 && cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let height = if catalog.provenance.altitude_datum == LightningAltitudeDatum::Wgs84Ellipsoid
        {
            strike.altitude_m
        } else {
            None
        }
        .or(request.plan.model.fallback_strike_ellipsoid_height_m)
        .ok_or("Source height was not approved")?;
        sources.push(Source {
            xyz: geodetic_to_ecef(strike.latitude_deg, strike.longitude_deg, height)?,
            // Epoch subtraction precedes acoustic delay for reproducible subsecond math.
            time_relative_s: strike.time_utc_s - request.plan.nominal_recording_start_utc_s,
        });
    }
    Ok(Some(sources))
}

struct Curve {
    rms: Vec<f64>,
    contributing: usize,
    distance_excluded: usize,
}
fn curve(
    request: &EnvelopeSearchRequest,
    estimate: &EnvelopeEstimate,
    sources: &[Source],
    receiver: &EnvelopeReceiver,
    cancelled: &AtomicBool,
) -> Result<Option<Curve>, String> {
    let model = &request.plan.model;
    let observer = geodetic_to_ecef(
        receiver.location.latitude_deg,
        receiver.location.longitude_deg,
        receiver.location.ellipsoid_height_m,
    )?;
    let cadence = estimate.cadence_s;
    let origin = request.plan.min_clock_offset_s + request.observation.envelope.samples[0].time_s;
    let mut energy = vec![0.0; estimate.predicted_sample_count];
    let mut contributing = 0;
    let mut distance_excluded = 0;
    let normalization = -(-model.kernel_duration_s / model.kernel_decay_s).exp_m1();
    for (source_index, source) in sources.iter().enumerate() {
        if source_index & 63 == 0 && cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let distance = (source.xyz[0] - observer[0])
            .hypot(source.xyz[1] - observer[1])
            .hypot(source.xyz[2] - observer[2]);
        if distance > model.maximum_distance_m {
            distance_excluded += 1;
            continue;
        }
        let arrival = source.time_relative_s + distance / model.sound_speed_mps;
        // Half-open sample bins overlap the finite kernel. Extra endpoint bins
        // are harmless: their exact integrated energy is zero.
        let first = ((arrival - origin) / cadence - 0.5).floor().max(0.0) as usize;
        let last = ((arrival + model.kernel_duration_s - origin) / cadence + 0.5)
            .ceil()
            .max(0.0) as usize;
        let upper = last.min(energy.len());
        let attenuation = (1000.0 / distance.max(model.minimum_distance_m)).powi(2);
        let mut supplied = false;
        for (index, bin) in energy
            .iter_mut()
            .enumerate()
            .take(upper)
            .skip(first.min(upper))
        {
            if index & 4095 == 0 && cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let center = origin + index as f64 * cadence;
            let start = (center - cadence / 2.0 - arrival).max(0.0);
            let end = (center + cadence / 2.0 - arrival).min(model.kernel_duration_s);
            if end <= start {
                continue;
            }
            let integral = (-start / model.kernel_decay_s).exp()
                * -(-(end - start) / model.kernel_decay_s).exp_m1()
                / normalization;
            *bin += attenuation * integral / cadence;
            supplied = true;
        }
        if supplied {
            contributing += 1;
        }
    }
    for value in &mut energy {
        if !value.is_finite() || *value < 0.0 {
            return Err("Predicted energy overflowed or became negative".into());
        }
        *value = value.sqrt();
    }
    Ok(Some(Curve {
        rms: energy,
        contributing,
        distance_excluded,
    }))
}

fn normalized(values: &[f64]) -> Vec<f64> {
    let peak = values.iter().copied().fold(0.0_f64, f64::max);
    if peak == 0.0 {
        return vec![0.0; values.len()];
    }
    values.iter().map(|value| value / peak).collect()
}

fn direct_cosine(observed: &[f64], predicted: &[f64]) -> Option<f64> {
    let peak = predicted.iter().copied().fold(0.0_f64, f64::max);
    if peak == 0.0 {
        return None;
    }
    let (mut dot, mut left_energy, mut right_energy) = (0.0, 0.0, 0.0);
    for (left, right) in observed.iter().zip(predicted) {
        let right = right / peak;
        dot += left * right;
        left_energy += left * left;
        right_energy += right * right;
    }
    let score = dot / (left_energy * right_energy).sqrt();
    score.is_finite().then_some(score.clamp(0.0, 1.0))
}

struct LagScores {
    ranked: Vec<(usize, f64)>,
    zero: usize,
    unresolved: usize,
    rankable: usize,
    discarded_upper: Option<f64>,
}
fn lag_scores(
    observed: &[f64],
    observed_fft: &[Complex],
    curve: &[f64],
    estimate: &EnvelopeEstimate,
    keep: usize,
    cancelled: &AtomicBool,
) -> Result<Option<LagScores>, String> {
    let predicted = normalized(curve);
    let energy = EnergyTree::new(&predicted);
    let observed_energy: f64 = observed.iter().map(|x| x * x).sum();
    let total_predicted_energy = energy.sum(0, predicted.len());
    if total_predicted_energy == 0.0 {
        return Ok(Some(LagScores {
            ranked: Vec::new(),
            zero: estimate.offsets_per_receiver,
            unresolved: 0,
            rankable: 0,
            discarded_upper: None,
        }));
    }
    let mut transformed = vec![Complex::default(); estimate.fft_size];
    for (value, sample) in transformed.iter_mut().zip(&predicted) {
        value.re = *sample;
    }
    if !fft(&mut transformed, false, cancelled) {
        return Ok(None);
    }
    for (value, reference) in transformed.iter_mut().zip(observed_fft) {
        *value = Complex {
            re: value.re * reference.re - value.im * reference.im,
            im: value.re * reference.im + value.im * reference.re,
        };
    }
    if !fft(&mut transformed, true, cancelled) {
        return Ok(None);
    }
    // Conservative engineering roundoff guard, not a confidence interval. Very
    // weak windows relative to the complete transform are unavailable instead
    // of allowing FFT leakage to manufacture matches. Retained scores are also
    // recomputed directly from their full windows.
    let absolute_guard = 128.0
        * f64::EPSILON
        * (estimate.fft_size.ilog2() as f64 + 1.0)
        * (observed_energy * total_predicted_energy).sqrt();
    let mut ranked: Vec<(usize, f64)> = Vec::new();
    let mut discarded_upper: Option<f64> = None;
    let (mut zero, mut unresolved, mut rankable) = (0, 0, 0);
    for offset in 0..estimate.offsets_per_receiver {
        if offset & 255 == 0 && cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let window_energy = energy.sum(offset, offset + observed.len());
        if window_energy == 0.0 {
            zero += 1;
            continue;
        }
        let denominator = (observed_energy * window_energy).sqrt();
        let score_guard = absolute_guard / denominator;
        if score_guard > NUMERIC_SCORE_TOLERANCE {
            unresolved += 1;
            continue;
        }
        let score = transformed[observed.len() - 1 + offset].re / denominator;
        if !score.is_finite()
            || !(-NUMERIC_SCORE_TOLERANCE..=1.0 + NUMERIC_SCORE_TOLERANCE).contains(&score)
        {
            return Err(
                "FFT cosine exceeded its numerical range; comparison is unavailable".into(),
            );
        }
        rankable += 1;
        let score = score.clamp(0.0, 1.0);
        let position = ranked
            .iter()
            .position(|(_, other)| score > *other)
            .unwrap_or(ranked.len());
        if position < keep {
            ranked.insert(position, (offset, score));
            if ranked.len() > keep {
                let (_, discarded) = ranked.pop().unwrap();
                discarded_upper = Some(
                    discarded_upper
                        .unwrap_or(0.0)
                        .max(discarded + NUMERIC_SCORE_TOLERANCE),
                );
            }
        } else {
            discarded_upper = Some(
                discarded_upper
                    .unwrap_or(0.0)
                    .max(score + NUMERIC_SCORE_TOLERANCE),
            );
        }
    }
    for (offset, score) in &mut ranked {
        *score = direct_cosine(observed, &curve[*offset..*offset + observed.len()])
            .ok_or("Retained full window lost its model energy")?;
    }
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(Some(LagScores {
        ranked,
        zero,
        unresolved,
        rankable,
        discarded_upper,
    }))
}

fn hypothesis(
    request: &EnvelopeSearchRequest,
    estimate: &EnvelopeEstimate,
    receiver_index: usize,
    offset_index: usize,
    score: f64,
    curve: &Curve,
) -> EnvelopeHypothesis {
    let receiver = &request.plan.receivers[receiver_index];
    let offset = request.plan.min_clock_offset_s + offset_index as f64 * estimate.cadence_s;
    EnvelopeHypothesis {
        request_sha256: estimate.request_sha256.clone(),
        catalog_data_sha256: estimate.catalog_data_sha256.clone(),
        receiver_index,
        receiver_id: receiver.id.clone(),
        observer: receiver.location.clone(),
        offset_index,
        clock_offset_s: offset,
        recording_start_utc_s: request.plan.nominal_recording_start_utc_s + offset,
        cosine_similarity: score,
        clock_at_boundary: offset_index == 0 || offset_index + 1 == estimate.offsets_per_receiver,
        contributing_source_rows: curve.contributing,
        distance_excluded_source_rows: curve.distance_excluded,
    }
}

fn report(estimate: EnvelopeEstimate) -> EnvelopeReport {
    EnvelopeReport {
        schema_version: 1,
        progress: EnvelopeProgress { total_receivers: estimate.receiver_count, total_offsets: estimate.total_offsets, ..Default::default() },
        estimate, hypotheses: Vec::new(), receivers: Vec::new(), complete: false, cancelled: false,
        location_verified: false, independent_field_validation: false,
        numerical_ordering_tolerance: NUMERIC_SCORE_TOLERANCE,
        retained_near_tie_count: 0, retention_boundary_ambiguous: false, exact_top_k_guaranteed: false,
        score_meaning: "Higher full-window cosine means a closer shape match under the explicit assumed energy kernel; it is not confidence, source identification or location verification.".into(),
        limitations: vec![
            "This is a bounded regional comparison, not a worldwide or multiweek lightning-envelope search. Every time offset uses the complete same observed segment.".into(),
            "Each original lightning row has equal unit emitted energy. Inverse-square attenuation applies to energy; energy contributions are summed before converting to relative RMS. Peak current, intensity and multiplicity are ignored.".into(),
            "The finite exponential response, range floor/cutoff, point-source height and constant still-air slant propagation are explicit assumptions. They are not fitted acoustic source physics. Channel extent, wind, refraction, terrain, microphone AGC, clipping and non-thunder noise may invalidate them.".into(),
            "Catalog interval is declared queried support, not complete lightning detection or spatial coverage. Missing or out-of-area strikes can mislead this comparison. No original records are merged or deduplicated.".into(),
            "Source timestamp resolution and explicit FMI interpretation remain unchanged. A kernel wider than the representation allowance is smoothing, not a calibrated timing uncertainty or correction.".into(),
            "All offsets are evaluated by a linear FFT with full-window nonnegative energy sums. A conservative roundoff guard withholds numerically weak windows; retained scores are recomputed directly. Near ties within1e-7 are numerically unresolved, and retained ordering is not a confidence ranking.".into(),
            "No independent field-location accuracy, false-match decision or hybrid improvement is established. Do not count this envelope and picked arrivals from the same thunder as independent evidence. No reviewed events, associations or existing scores are changed.".into(),
        ],
    }
}

/// Searches only the explicit receivers and sample-cadence offsets. Cancellation
/// retains completed receivers; an interrupted receiver contributes no partial
/// offset result. The callback is observational and never approves evidence.
pub fn search(
    request: &EnvelopeSearchRequest,
    catalog: &LightningCatalog,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(&EnvelopeProgress),
) -> Result<EnvelopeReport, String> {
    let estimate = preflight(request, catalog)?;
    let mut result = report(estimate);
    progress(&result.progress);
    if cancelled.load(Ordering::Relaxed) {
        result.cancelled = true;
        return Ok(result);
    }
    let Some(sources) = prepare_sources(request, catalog, cancelled)? else {
        result.cancelled = true;
        return Ok(result);
    };
    let observed = normalized(
        &request
            .observation
            .envelope
            .samples
            .iter()
            .map(|s| s.rms)
            .collect::<Vec<_>>(),
    );
    let mut observed_fft = vec![Complex::default(); result.estimate.fft_size];
    for (value, sample) in observed_fft.iter_mut().zip(observed.iter().rev()) {
        value.re = *sample;
    }
    if !fft(&mut observed_fft, false, cancelled) {
        result.cancelled = true;
        return Ok(result);
    }
    let mut discarded_upper: Option<f64> = None;
    for (index, receiver) in request.plan.receivers.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
        let Some(curve) = curve(request, &result.estimate, &sources, receiver, cancelled)? else {
            result.cancelled = true;
            break;
        };
        let Some(scores) = lag_scores(
            &observed,
            &observed_fft,
            &curve.rms,
            &result.estimate,
            request.plan.keep_best,
            cancelled,
        )?
        else {
            result.cancelled = true;
            break;
        };
        if let Some(upper) = scores.discarded_upper {
            discarded_upper = Some(discarded_upper.unwrap_or(0.0).max(upper));
        }
        let candidates = scores
            .ranked
            .iter()
            .map(|(offset, score)| {
                hypothesis(request, &result.estimate, index, *offset, *score, &curve)
            })
            .collect::<Vec<_>>();
        result.receivers.push(EnvelopeReceiverResult {
            receiver_index: index,
            receiver_id: receiver.id.clone(),
            best: candidates.first().cloned(),
            rankable_offsets: scores.rankable,
            zero_energy_offsets: scores.zero,
            numerically_unresolved_offsets: scores.unresolved,
            contributing_source_rows: curve.contributing,
            distance_excluded_source_rows: curve.distance_excluded,
        });
        result.hypotheses.extend(candidates);
        result.hypotheses.sort_by(|a, b| {
            b.cosine_similarity
                .total_cmp(&a.cosine_similarity)
                .then(a.receiver_index.cmp(&b.receiver_index))
                .then(a.offset_index.cmp(&b.offset_index))
        });
        for omitted in result.hypotheses.iter().skip(request.plan.keep_best) {
            discarded_upper = Some(
                discarded_upper
                    .unwrap_or(0.0)
                    .max(omitted.cosine_similarity),
            );
        }
        result.hypotheses.truncate(request.plan.keep_best);
        result.progress.completed_receivers += 1;
        result.progress.evaluated_offsets += result.estimate.offsets_per_receiver as u64;
        result.progress.zero_energy_offsets += scores.zero as u64;
        result.progress.numerically_unresolved_offsets += scores.unresolved as u64;
        progress(&result.progress);
    }
    result.complete = result.progress.completed_receivers == result.estimate.receiver_count;
    result.retained_near_tie_count = result
        .hypotheses
        .windows(2)
        .filter(|pair| {
            (pair[0].cosine_similarity - pair[1].cosine_similarity).abs()
                <= 2.0 * NUMERIC_SCORE_TOLERANCE
        })
        .count();
    result.retention_boundary_ambiguous = discarded_upper
        .zip(result.hypotheses.last())
        .is_some_and(|(upper, last)| upper + NUMERIC_SCORE_TOLERANCE >= last.cosine_similarity);
    // Only an untruncated, fully evaluated collection contains every supported
    // result directly. Otherwise FFT near-cutoff alternatives remain disclosed.
    result.exact_top_k_guaranteed = result.complete
        && discarded_upper.is_none()
        && result.progress.numerically_unresolved_offsets == 0;
    if !result.complete && cancelled.load(Ordering::Relaxed) {
        result.cancelled = true;
    }
    Ok(result)
}

/// Reproduces a chosen retained hypothesis for inspection, with identity and
/// score checks. This does not apply its location/time to any workspace.
pub fn overlay(
    request: &EnvelopeSearchRequest,
    catalog: &LightningCatalog,
    chosen: &EnvelopeHypothesis,
) -> Result<EnvelopeOverlay, String> {
    overlay_with_cancel(request, catalog, chosen, &AtomicBool::new(false))?
        .ok_or_else(|| "Unexpected overlay cancellation".into())
}

/// Interruptible inspection variant. `None` means cancellation; no partial
/// curve is returned as if it were a complete prediction.
pub fn overlay_with_cancel(
    request: &EnvelopeSearchRequest,
    catalog: &LightningCatalog,
    chosen: &EnvelopeHypothesis,
    cancelled: &AtomicBool,
) -> Result<Option<EnvelopeOverlay>, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let estimate = preflight(request, catalog)?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    if chosen.request_sha256 != estimate.request_sha256
        || chosen.catalog_data_sha256 != estimate.catalog_data_sha256
        || chosen.receiver_index >= request.plan.receivers.len()
        || chosen.offset_index >= estimate.offsets_per_receiver
    {
        return Err(
            "Envelope hypothesis belongs to changed inputs or lies outside the declared search"
                .into(),
        );
    }
    let receiver = &request.plan.receivers[chosen.receiver_index];
    let Some(sources) = prepare_sources(request, catalog, cancelled)? else {
        return Ok(None);
    };
    let Some(curve) = curve(request, &estimate, &sources, receiver, cancelled)? else {
        return Ok(None);
    };
    let predicted =
        curve.rms[chosen.offset_index..chosen.offset_index + estimate.sample_count].to_vec();
    let observed: Vec<_> = request
        .observation
        .envelope
        .samples
        .iter()
        .map(|s| s.rms)
        .collect();
    let score = direct_cosine(&normalized(&observed), &predicted)
        .ok_or("Chosen envelope window has zero energy")?;
    let expected = hypothesis(
        request,
        &estimate,
        chosen.receiver_index,
        chosen.offset_index,
        score,
        &curve,
    );
    if hash(chosen)? != hash(&expected)? {
        return Err(
            "Envelope hypothesis fields or full-window score do not reproduce exactly".into(),
        );
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    Ok(Some(EnvelopeOverlay {
        request_sha256: estimate.request_sha256,
        catalog_data_sha256: estimate.catalog_data_sha256,
        hypothesis: expected,
        times_s: request
            .observation
            .envelope
            .samples
            .iter()
            .map(|s| s.time_s)
            .collect(),
        observed_rms: observed,
        predicted_relative_rms: predicted,
        model: request.plan.model.clone(),
        location_verified: false,
    }))
}

#[cfg(test)]
mod numerical_controls {
    use super::*;

    #[test]
    fn raw_fft_linear_correlation_matches_naive_dot_at_every_full_lag() {
        // These checks inspect the raw FFT numerator BEFORE the separately
        // recomputed retained scores, so a wrong sign/crop cannot be hidden.
        let cancelled = AtomicBool::new(false);
        for (n, m) in [(3_usize, 11_usize), (7, 23), (16, 31), (31, 47)] {
            let observed: Vec<_> = (0..n).map(|i| ((i * 7 + 3) % 11) as f64 / 13.0).collect();
            let predicted: Vec<_> = (0..m).map(|i| ((i * 3 + 5) % 17) as f64 / 19.0).collect();
            let length = (n + m - 1).next_power_of_two();
            let mut left = vec![Complex::default(); length];
            let mut right = vec![Complex::default(); length];
            for (value, sample) in left.iter_mut().zip(observed.iter().rev()) {
                value.re = *sample;
            }
            for (value, sample) in right.iter_mut().zip(&predicted) {
                value.re = *sample;
            }
            assert!(fft(&mut left, false, &cancelled));
            assert!(fft(&mut right, false, &cancelled));
            for (a, b) in left.iter_mut().zip(right) {
                *a = Complex {
                    re: a.re * b.re - a.im * b.im,
                    im: a.re * b.im + a.im * b.re,
                };
            }
            assert!(fft(&mut left, true, &cancelled));
            for offset in 0..=m - n {
                let expected: f64 = observed
                    .iter()
                    .zip(&predicted[offset..offset + n])
                    .map(|(a, b)| a * b)
                    .sum();
                assert!(
                    (left[n - 1 + offset].re - expected).abs() < 2e-11,
                    "n{n} m{m} lag{offset}"
                );
                assert!(left[n - 1 + offset].im.abs() < 2e-11);
            }
        }
    }

    #[test]
    fn energy_range_tree_preserves_quiet_window_after_loud_energy() {
        let tree = EnergyTree::new(&[1.0, 1e-15, 2e-15, 0.0]);
        assert!((tree.sum(1, 3) - 5e-30).abs() < 1e-44);
        assert_eq!(tree.sum(3, 4), 0.0);
    }

    #[test]
    fn fft_roundoff_does_not_manufacture_a_quiet_or_empty_window_match() {
        let cancelled = AtomicBool::new(false);
        let observed = [1.0, 0.5, 0.25];
        let curve = [1e-15, 1e-15, 1e-15, 0.0, 0.0, 0.0, 1.0, 0.5];
        let estimate = EnvelopeEstimate {
            request_sha256: "a".repeat(64),
            catalog_data_sha256: "b".repeat(64),
            sample_count: 3,
            cadence_s: 1.0,
            receiver_count: 1,
            offsets_per_receiver: 6,
            total_offsets: 6,
            source_rows: 2,
            predicted_sample_count: 8,
            fft_size: 16,
            estimated_work_units: 1,
            estimated_memory_bytes: 1,
            required_catalog_utc_interval: [0.0, 10.0],
            source_interpretation: None,
        };
        let mut transformed = vec![Complex::default(); 16];
        for (value, sample) in transformed.iter_mut().zip(observed.iter().rev()) {
            value.re = *sample;
        }
        assert!(fft(&mut transformed, false, &cancelled));
        let scores = lag_scores(&observed, &transformed, &curve, &estimate, 40, &cancelled)
            .unwrap()
            .unwrap();
        assert!(scores.unresolved >= 1);
        assert!(scores.zero >= 1);
        assert!(
            !scores
                .ranked
                .iter()
                .any(|(offset, _)| *offset == 0 || *offset == 3)
        );
        assert_eq!(scores.rankable + scores.zero + scores.unresolved, 6);
    }

    #[test]
    fn transform_observes_an_owned_cancellation_flag() {
        let mut values = vec![Complex::default(); 8192];
        assert!(!fft(&mut values, false, &AtomicBool::new(true)));
    }
}
