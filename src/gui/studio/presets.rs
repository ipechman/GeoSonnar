//! Explicit, uncalibrated sensitivity choices. Applying one changes settings,
//! never starts analysis or accepts evidence.
use geosonnar::{discovery::DiscoverySettings, thunder::ThunderConfig};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DopplerPreset {
    Broad,
    Balanced,
    Strict,
}
impl DopplerPreset {
    pub const ALL: [Self; 3] = [Self::Broad, Self::Balanced, Self::Strict];
    pub fn label(self) -> &'static str {
        match self {
            Self::Broad => "Broad",
            Self::Balanced => "Balanced",
            Self::Strict => "Strict",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Broad => {
                "Include weaker, shorter and less continuous falling tones; expect more candidates to review."
            }
            Self::Balanced => {
                "Use the standard contrast, duration, continuity and frequency-drop thresholds."
            }
            Self::Strict => {
                "Require stronger, longer, more continuous tones with a larger frequency drop."
            }
        }
    }
    pub fn settings(self) -> DiscoverySettings {
        match self {
            Self::Broad => DiscoverySettings {
                min_contrast: 1.75,
                min_support: 0.45,
                min_duration_s: 8.,
                min_drop_hz: 2.,
                min_fractional_drop: 0.015,
                ..Default::default()
            },
            Self::Balanced => DiscoverySettings::default(),
            Self::Strict => DiscoverySettings {
                min_contrast: 3.5,
                min_support: 0.8,
                min_duration_s: 18.,
                min_drop_hz: 6.,
                min_fractional_drop: 0.05,
                ..Default::default()
            },
        }
    }
    pub fn matching(settings: &DiscoverySettings) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| same(settings, &preset.settings()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ThunderPreset {
    Sensitive,
    Balanced,
    StrongBursts,
}
impl ThunderPreset {
    pub const ALL: [Self; 3] = [Self::Sensitive, Self::Balanced, Self::StrongBursts];
    pub fn label(self) -> &'static str {
        match self {
            Self::Sensitive => "Sensitive",
            Self::Balanced => "Balanced",
            Self::StrongBursts => "Strong bursts",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Sensitive => {
                "Include quieter and shorter low-frequency bursts; wind and handling noise can also trigger proposals."
            }
            Self::Balanced => {
                "Use the standard low-frequency noise ratio, RMS floor and duration thresholds."
            }
            Self::StrongBursts => {
                "Require louder low-frequency bursts that persist longer above the threshold."
            }
        }
    }
    pub fn settings(self) -> ThunderConfig {
        match self {
            Self::Sensitive => ThunderConfig {
                threshold_ratio: 1.8,
                minimum_rms: 0.00005,
                min_duration_s: 0.15,
                ..Default::default()
            },
            Self::Balanced => ThunderConfig::default(),
            Self::StrongBursts => ThunderConfig {
                threshold_ratio: 5.,
                minimum_rms: 0.0002,
                min_duration_s: 0.5,
                ..Default::default()
            },
        }
    }
    pub fn matching(settings: &ThunderConfig) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| same(settings, &preset.settings()))
    }
}

// Compare every serialized setting, including fields not exposed in the simple
// controls. A preset label must never conceal an edited/custom configuration.
fn same<T: Serialize>(left: &T, right: &T) -> bool {
    serde_json::to_value(left).is_ok_and(|a| serde_json::to_value(right).is_ok_and(|b| a == b))
}

/// Preserve the existing spacing adaptation, but share it between the display
/// and execution path so coarse loaded audio never conceals the effective gap.
pub(super) fn effective_thunder(settings: &ThunderConfig, loaded_hop_s: f64) -> ThunderConfig {
    let mut effective = settings.clone();
    effective.max_sample_gap_s = (loaded_hop_s * 1.1).max(settings.max_sample_gap_s);
    effective.max_gap_s = effective.max_gap_s.max(effective.max_sample_gap_s);
    effective
}

#[cfg(test)]
mod tests {
    use super::*;
    use geosonnar::thunder::{self, EnvelopeSample, ThunderEnvelope};

    #[test]
    fn doppler_presets_are_valid_and_have_ordered_selection_thresholds() {
        let values = DopplerPreset::ALL.map(|preset| {
            let settings = preset.settings();
            settings.validate().unwrap();
            assert_eq!(DopplerPreset::matching(&settings), Some(preset));
            settings
        });
        for pair in values.windows(2) {
            assert!(pair[0].min_contrast < pair[1].min_contrast);
            assert!(pair[0].min_support < pair[1].min_support);
            assert!(pair[0].min_duration_s < pair[1].min_duration_s);
            assert!(pair[0].min_drop_hz < pair[1].min_drop_hz);
            assert!(pair[0].min_fractional_drop < pair[1].min_fractional_drop);
            assert_eq!(pair[0].min_frequency_hz, pair[1].min_frequency_hz);
            assert_eq!(pair[0].max_frequency_hz, pair[1].max_frequency_hz);
        }
        assert_eq!(
            DopplerPreset::matching(&DiscoverySettings::default()),
            Some(DopplerPreset::Balanced)
        );
    }

    #[test]
    fn thunder_presets_change_real_backend_proposals_without_accepting_them() {
        let envelope = ThunderEnvelope {
            samples: (0..160)
                .map(|index| EnvelopeSample {
                    time_s: index as f64 * 0.125,
                    rms: match index {
                        24..=32 => 0.00025,
                        64..=72 => 0.0004,
                        104..=112 => 0.0008,
                        _ => 0.0001,
                    },
                })
                .collect(),
            band_low_hz: 20.,
            band_high_hz: 250.,
            method: "synthetic RMS envelope control".into(),
        };
        let mut thresholds = vec![];
        for (preset, count) in ThunderPreset::ALL.into_iter().zip([3, 2, 1]) {
            let settings = preset.settings();
            assert_eq!(ThunderPreset::matching(&settings), Some(preset));
            let report = thunder::propose(&envelope, &settings).unwrap();
            assert_eq!(report.proposals.len(), count);
            assert!(
                report
                    .proposals
                    .iter()
                    .all(|proposal| proposal.needs_review)
            );
            assert!(!report.location_verified);
            thresholds.push(report.threshold_rms);
        }
        assert!(thresholds.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            ThunderPreset::matching(&ThunderConfig::default()),
            Some(ThunderPreset::Balanced)
        );
    }

    #[test]
    fn numeric_edits_including_hidden_fields_are_custom_until_a_preset_is_reapplied() {
        let mut doppler = DopplerPreset::Balanced.settings();
        doppler.min_support += 0.01;
        assert_eq!(DopplerPreset::matching(&doppler), None);
        doppler = DopplerPreset::Balanced.settings();
        doppler.max_peaks_per_frame += 1;
        assert_eq!(DopplerPreset::matching(&doppler), None);
        let mut thunder = ThunderPreset::Balanced.settings();
        thunder.threshold_ratio += 0.1;
        assert_eq!(ThunderPreset::matching(&thunder), None);
        thunder = ThunderPreset::Balanced.settings();
        thunder.noise_quantile = 0.25;
        assert_eq!(ThunderPreset::matching(&thunder), None);
        thunder = ThunderPreset::Balanced.settings();
        assert_eq!(
            ThunderPreset::matching(&thunder),
            Some(ThunderPreset::Balanced)
        );
    }

    #[test]
    fn effective_gap_tracks_loaded_audio_without_overwriting_requested_sensitivity() {
        let requested = ThunderPreset::Balanced.settings();
        let fine = effective_thunder(&requested, 0.125);
        assert!(same(&fine, &requested));
        let coarse = effective_thunder(&requested, 1.0);
        assert_eq!(coarse.max_gap_s, 1.1);
        assert_eq!(coarse.max_sample_gap_s, 1.1);
        assert_eq!(coarse.threshold_ratio, requested.threshold_ratio);
        assert_eq!(coarse.minimum_rms, requested.minimum_rms);
        assert_eq!(requested.max_gap_s, 0.4);
        assert_eq!(ThunderPreset::matching(&coarse), None);
        assert_eq!(
            ThunderPreset::matching(&requested),
            Some(ThunderPreset::Balanced)
        );
    }
}
