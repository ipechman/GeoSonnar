//! Experimental proposals from a low-frequency RMS envelope. These thresholds
//! separate energetic intervals, not thunder from wind, handling noise or engines.
//! A proposal is never an accepted event or a location measurement.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeSample {
    pub time_s: f64,
    pub rms: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThunderEnvelope {
    pub samples: Vec<EnvelopeSample>,
    /// Actual included frequency-bin bounds, not an ideal filter specification.
    pub band_low_hz: f64,
    pub band_high_hz: f64,
    pub method: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThunderConfig {
    /// Higher values are less sensitive. Threshold is max(noise * ratio, floor).
    pub threshold_ratio: f64,
    pub noise_quantile: f64,
    pub minimum_rms: f64,
    pub min_duration_s: f64,
    pub max_duration_s: f64,
    /// Maximum time between above-threshold samples in one proposal.
    pub max_gap_s: f64,
    /// Larger sampling gaps break proposals; no missing audio is bridged.
    pub max_sample_gap_s: f64,
}

impl Default for ThunderConfig {
    fn default() -> Self {
        Self {
            threshold_ratio: 3.0,
            noise_quantile: 0.2,
            minimum_rms: 0.0001,
            min_duration_s: 0.2,
            max_duration_s: 30.0,
            max_gap_s: 0.4,
            max_sample_gap_s: 0.25,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThunderProposal {
    pub id: String,
    pub start_s: f64,
    pub end_s: f64,
    pub peak_s: f64,
    pub peak_rms: f64,
    pub above_threshold_samples: usize,
    pub peak_to_threshold_ratio: f64,
    pub needs_review: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThunderProposalReport {
    pub schema_version: u32,
    pub config: ThunderConfig,
    pub band_low_hz: f64,
    pub band_high_hz: f64,
    pub envelope_method: String,
    pub noise_rms: f64,
    pub threshold_rms: f64,
    pub proposals: Vec<ThunderProposal>,
    pub rejected_short_intervals: usize,
    pub rejected_long_intervals: usize,
    pub sampling_gap_count: usize,
    pub calibration: String,
    pub location_verified: bool,
    pub limitations: Vec<String>,
}

/// A manually placed or adjusted event. `reviewed_as_thunder` records a user's
/// interpretation; it does not establish its association with any lightning.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThunderAnnotation {
    pub id: String,
    pub start_s: f64,
    pub end_s: f64,
    pub arrival_s: f64,
    pub uncertainty_s: f64,
    pub reviewed_as_thunder: bool,
    pub source_proposal_id: Option<String>,
    pub note: String,
}

pub fn validate_annotation(annotation: &ThunderAnnotation) -> Result<(), String> {
    if annotation.id.trim().is_empty()
        || !annotation.start_s.is_finite()
        || !annotation.end_s.is_finite()
        || !annotation.arrival_s.is_finite()
        || annotation.start_s < 0.0
        || annotation.end_s <= annotation.start_s
        || !(annotation.start_s..=annotation.end_s).contains(&annotation.arrival_s)
        || !annotation.uncertainty_s.is_finite()
        || annotation.uncertainty_s <= 0.0
    {
        return Err("Thunder annotation needs an ID, an ordered nonnegative interval, an arrival within it and positive uncertainty".into());
    }
    Ok(())
}

pub fn propose(
    envelope: &ThunderEnvelope,
    config: &ThunderConfig,
) -> Result<ThunderProposalReport, String> {
    if !config.threshold_ratio.is_finite()
        || config.threshold_ratio <= 1.0
        || !config.noise_quantile.is_finite()
        || !(0.0..=0.5).contains(&config.noise_quantile)
        || !config.minimum_rms.is_finite()
        || config.minimum_rms <= 0.0
        || !config.min_duration_s.is_finite()
        || config.min_duration_s <= 0.0
        || !config.max_duration_s.is_finite()
        || config.max_duration_s < config.min_duration_s
        || !config.max_gap_s.is_finite()
        || config.max_gap_s <= 0.0
        || !config.max_sample_gap_s.is_finite()
        || config.max_sample_gap_s <= 0.0
    {
        return Err("Invalid thunder thresholds, quantile, duration or gap settings".into());
    }
    if !envelope.band_low_hz.is_finite()
        || !envelope.band_high_hz.is_finite()
        || envelope.band_low_hz < 0.0
        || envelope.band_high_hz <= envelope.band_low_hz
        || envelope.method.trim().is_empty()
        || envelope.samples.len() < 2
        || envelope.samples.len() > 5_000_000
    {
        return Err(
            "Thunder envelope needs documented frequency bounds, method and 2..5000000 samples"
                .into(),
        );
    }
    for (i, sample) in envelope.samples.iter().enumerate() {
        if !sample.time_s.is_finite()
            || sample.time_s < 0.0
            || !sample.rms.is_finite()
            || sample.rms < 0.0
            || (i > 0 && sample.time_s <= envelope.samples[i - 1].time_s)
        {
            return Err("Envelope samples must have increasing finite nonnegative times and finite nonnegative RMS".into());
        }
    }
    let mut sorted: Vec<f64> = envelope.samples.iter().map(|s| s.rms).collect();
    sorted.sort_unstable_by(f64::total_cmp);
    let noise_rms = sorted[((sorted.len() - 1) as f64 * config.noise_quantile).floor() as usize];
    let threshold_rms = (noise_rms * config.threshold_ratio).max(config.minimum_rms);
    if !threshold_rms.is_finite() {
        return Err("Thunder threshold overflowed".into());
    }
    if !(sorted[sorted.len() - 1] / threshold_rms).is_finite() {
        return Err("Thunder peak-to-threshold diagnostic overflowed".into());
    }
    let mut report = ThunderProposalReport {
        schema_version: 1,
        config: config.clone(),
        band_low_hz: envelope.band_low_hz,
        band_high_hz: envelope.band_high_hz,
        envelope_method: envelope.method.clone(),
        noise_rms,
        threshold_rms,
        proposals: Vec::new(),
        rejected_short_intervals: 0,
        rejected_long_intervals: 0,
        sampling_gap_count: 0,
        calibration: "experimental threshold diagnostics; not probabilities".into(),
        location_verified: false,
        limitations: vec![
            "Low-frequency energy is not specific to thunder; wind, handling noise and engines may trigger proposals.".into(),
            "Boundaries are above-threshold sample centers; manually review and adjust the acoustic arrival and its uncertainty.".into(),
            "A global noise quantile can miss sustained or quiet events. Absence of a proposal is not absence of thunder.".into(),
        ],
    };
    // Keep only active sample indices; reject overlong intervals as a whole rather
    // than splitting sustained wind/noise into apparently independent events.
    let mut active: Vec<usize> = Vec::new();
    let flush = |active: &mut Vec<usize>, report: &mut ThunderProposalReport| {
        if active.is_empty() {
            return;
        }
        let first = &envelope.samples[active[0]];
        let last = &envelope.samples[*active.last().unwrap()];
        let duration = last.time_s - first.time_s;
        if duration + 1e-10 < config.min_duration_s {
            report.rejected_short_intervals += 1;
        } else if duration > config.max_duration_s + 1e-10 {
            report.rejected_long_intervals += 1;
        } else {
            let peak = active
                .iter()
                .map(|&i| &envelope.samples[i])
                .max_by(|a, b| a.rms.total_cmp(&b.rms))
                .unwrap();
            report.proposals.push(ThunderProposal {
                id: format!("thunder-proposal-{:04}", report.proposals.len() + 1),
                start_s: first.time_s,
                end_s: last.time_s,
                peak_s: peak.time_s,
                peak_rms: peak.rms,
                above_threshold_samples: active.len(),
                peak_to_threshold_ratio: peak.rms / threshold_rms,
                needs_review: true,
            });
        }
        active.clear();
    };
    for (i, sample) in envelope.samples.iter().enumerate() {
        if i > 0 && sample.time_s - envelope.samples[i - 1].time_s > config.max_sample_gap_s {
            report.sampling_gap_count += 1;
            flush(&mut active, &mut report);
        }
        if let Some(&last) = active.last()
            && sample.time_s - envelope.samples[last].time_s > config.max_gap_s + 1e-10
        {
            flush(&mut active, &mut report);
        }
        if sample.rms > threshold_rms {
            active.push(i);
        }
    }
    flush(&mut active, &mut report);
    Ok(report)
}
