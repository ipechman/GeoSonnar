//! Fabricated spectral-band envelopes; no claim that RMS identifies thunder.
use geosonnar::thunder::*;

fn envelope() -> ThunderEnvelope {
    ThunderEnvelope {
        samples: (0..201)
            .map(|i| EnvelopeSample {
                time_s: i as f64 / 10.0,
                rms: 1.0,
            })
            .collect(),
        band_low_hz: 23.4375,
        band_high_hz: 246.09375,
        method: "fabricated band RMS for threshold controls".into(),
    }
}

fn burst(e: &mut ThunderEnvelope, start: usize, end: usize, rms: f64) {
    for sample in &mut e.samples[start..=end] {
        sample.rms = rms;
    }
}

#[test]
fn bounded_burst_remains_unreviewed_with_exact_diagnostics() {
    let mut e = envelope();
    burst(&mut e, 40, 60, 6.0);
    let report = propose(&e, &ThunderConfig::default()).unwrap();
    assert_eq!(report.proposals.len(), 1);
    let p = &report.proposals[0];
    assert_eq!((p.start_s, p.end_s), (4.0, 6.0));
    assert_eq!(p.above_threshold_samples, 21);
    assert_eq!(report.noise_rms, 1.0);
    assert_eq!(report.threshold_rms, 3.0);
    assert_eq!(p.peak_to_threshold_ratio, 2.0);
    assert!(p.needs_review);
    assert!(!report.location_verified);
    assert_eq!(report.band_low_hz, e.band_low_hz);
    assert!(report.calibration.contains("not probabilities"));
}

#[test]
fn silence_steady_energy_and_single_impulse_do_not_become_events() {
    for level in [0.0, 1.0, 100.0] {
        let mut e = envelope();
        for s in &mut e.samples {
            s.rms = level;
        }
        assert!(
            propose(&e, &ThunderConfig::default())
                .unwrap()
                .proposals
                .is_empty()
        );
    }
    let mut e = envelope();
    e.samples[50].rms = 1000.0;
    let r = propose(&e, &ThunderConfig::default()).unwrap();
    assert!(r.proposals.is_empty());
    assert_eq!(r.rejected_short_intervals, 1);
}

#[test]
fn sensitivity_changes_threshold_without_changing_samples() {
    let mut e = envelope();
    burst(&mut e, 40, 60, 4.0);
    let mut config = ThunderConfig::default();
    assert_eq!(propose(&e, &config).unwrap().proposals.len(), 1);
    config.threshold_ratio = 5.0;
    assert!(propose(&e, &config).unwrap().proposals.is_empty());
    config.threshold_ratio = 3.0;
    config.minimum_rms = 5.0;
    assert!(propose(&e, &config).unwrap().proposals.is_empty());
}

#[test]
fn gap_joining_is_bounded_and_does_not_bridge_missing_samples() {
    let mut e = envelope();
    burst(&mut e, 40, 45, 5.0);
    burst(&mut e, 48, 53, 5.0);
    let config = ThunderConfig::default();
    assert_eq!(propose(&e, &config).unwrap().proposals.len(), 1);
    let mut narrow = config.clone();
    narrow.max_gap_s = 0.15;
    assert_eq!(propose(&e, &narrow).unwrap().proposals.len(), 2);
    e.samples.drain(46..48);
    let r = propose(&e, &config).unwrap();
    assert_eq!(r.sampling_gap_count, 1);
    assert_eq!(r.proposals.len(), 2);
}

#[test]
fn sustained_intervals_are_rejected_whole_not_split_into_events() {
    let mut e = envelope();
    burst(&mut e, 40, 150, 5.0);
    let config = ThunderConfig {
        max_duration_s: 5.0,
        ..Default::default()
    };
    let r = propose(&e, &config).unwrap();
    assert!(r.proposals.is_empty());
    assert_eq!(r.rejected_long_intervals, 1);
    assert!(r.limitations.iter().any(|s| s.contains("wind")));
}

#[test]
fn invalid_envelopes_and_settings_fail_instead_of_inventing_detections() {
    let e = envelope();
    for value in [0.0, 1.0, f64::NAN, f64::INFINITY] {
        let config = ThunderConfig {
            threshold_ratio: value,
            ..Default::default()
        };
        assert!(propose(&e, &config).is_err());
    }
    let mut bad = e.clone();
    bad.samples[10].time_s = bad.samples[9].time_s;
    assert!(propose(&bad, &ThunderConfig::default()).is_err());
    let mut bad = e.clone();
    bad.samples[10].rms = -1.0;
    assert!(propose(&bad, &ThunderConfig::default()).is_err());
    let mut bad = e;
    bad.samples[10].rms = f64::NAN;
    assert!(propose(&bad, &ThunderConfig::default()).is_err());
}

#[test]
fn manual_annotations_can_adjust_arrival_but_must_keep_it_inside_reviewed_interval() {
    let mut a = ThunderAnnotation {
        id: "manual-1".into(),
        start_s: 4.0,
        end_s: 8.0,
        arrival_s: 4.6,
        uncertainty_s: 0.3,
        reviewed_as_thunder: true,
        source_proposal_id: None,
        note: "Fabricated manual timing".into(),
    };
    validate_annotation(&a).unwrap();
    a.arrival_s = 8.1;
    assert!(validate_annotation(&a).is_err());
    a.arrival_s = 4.6;
    a.uncertainty_s = 0.0;
    assert!(validate_annotation(&a).is_err());
}

#[test]
fn extreme_dynamic_range_does_not_emit_nonfinite_diagnostics() {
    let mut e = envelope();
    for sample in &mut e.samples {
        sample.rms = 0.0;
    }
    burst(&mut e, 40, 60, 1e300);
    let config = ThunderConfig {
        minimum_rms: 1e-100,
        ..Default::default()
    };
    assert!(propose(&e, &config).unwrap_err().contains("overflowed"));
}
