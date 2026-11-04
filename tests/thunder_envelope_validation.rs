//! Independent arithmetic controls for the explicit assumed model. These are
//! synthetic software checks, not a blind field or hybrid-accuracy experiment.
use geosonnar::{
    evidence_score::{
        LightningAltitudeDatum, LightningCatalog, LightningProvenance, LightningStrike,
        ObserverLocation,
    },
    fmi_lightning,
    lightning_source::LightningSourcePolicy,
    thunder::{EnvelopeSample, ThunderEnvelope},
    thunder_envelope::*,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

const EPOCH: f64 = 1_700_000_000.0;

fn strike(id: &str, time: f64, height: f64) -> LightningStrike {
    LightningStrike {
        id: id.into(),
        time_utc_s: EPOCH + time,
        latitude_deg: 0.0,
        longitude_deg: 0.0,
        altitude_m: Some(height),
        intensity: Some(-300.0),
        source_record: None,
    }
}
fn catalog() -> LightningCatalog {
    LightningCatalog {
        schema_version: 1,
        input_path: PathBuf::from("fabricated-only.json"),
        input_bytes: 1,
        input_sha256: "b".repeat(64),
        provenance: LightningProvenance {
            provider: "Synthetic controls".into(),
            source: "Fabricated equal-radial geometry; no field accuracy".into(),
            license: None,
            notes: None,
            altitude_datum: LightningAltitudeDatum::Wgs84Ellipsoid,
            source_metadata: None,
        },
        strikes: vec![
            strike("first", 13.0, 343.0),
            strike("second", 17.25, 686.0),
            strike("third", 21.0, 343.0),
        ],
    }
}
fn receiver(id: &str) -> EnvelopeReceiver {
    EnvelopeReceiver {
        id: id.into(),
        location: ObserverLocation {
            latitude_deg: 0.0,
            longitude_deg: 0.0,
            ellipsoid_height_m: 0.0,
        },
    }
}
fn plan() -> EnvelopeSearchPlan {
    EnvelopeSearchPlan {
        schema_version: 1,
        catalog_sha256: "b".repeat(64),
        catalog_utc_interval: [EPOCH - 100.0, EPOCH + 100.0],
        receivers: vec![receiver("analytic-radial")],
        nominal_recording_start_utc_s: EPOCH,
        min_clock_offset_s: -2.0,
        max_clock_offset_s: 4.0,
        model: EnvelopeModel {
            sound_speed_mps: 343.0,
            fallback_strike_ellipsoid_height_m: None,
            minimum_distance_m: 10.0,
            maximum_distance_m: 5000.0,
            kernel_decay_s: 1.0,
            kernel_duration_s: 3.0,
        },
        source_policy: None,
        keep_best: 40,
        limits: EnvelopeLimits::default(),
    }
}

// Independent forward oracle: the special collinear equatorial geometry has
// exact source range equal to source height. No production geometry or kernel
// helper is called. Integrals use the analytic CDF, not the implementation's
// stable expm1 expression or its bin-index selection.
fn oracle_rms(
    catalog: &LightningCatalog,
    plan: &EnvelopeSearchPlan,
    time: f64,
    cadence: f64,
    offset: f64,
) -> f64 {
    let model = &plan.model;
    let cdf = |u: f64| {
        let value = u.clamp(0.0, model.kernel_duration_s);
        (1.0 - (-value / model.kernel_decay_s).exp())
            / (1.0 - (-model.kernel_duration_s / model.kernel_decay_s).exp())
    };
    let mut energy = 0.0;
    for source in &catalog.strikes {
        let range = source
            .altitude_m
            .or(model.fallback_strike_ellipsoid_height_m)
            .unwrap()
            .abs();
        if range > model.maximum_distance_m {
            continue;
        }
        let arrival = source.time_utc_s - EPOCH + range / model.sound_speed_mps;
        let left = time + offset - cadence / 2.0 - arrival;
        let right = time + offset + cadence / 2.0 - arrival;
        energy += (1000.0 / range.max(model.minimum_distance_m)).powi(2) * (cdf(right) - cdf(left))
            / cadence;
    }
    energy.sqrt()
}
fn request() -> EnvelopeSearchRequest {
    let plan = plan();
    let data = catalog();
    let samples = (0..20)
        .map(|i| {
            let time_s = 12.5 + i as f64 * 0.5;
            EnvelopeSample {
                time_s,
                rms: oracle_rms(&data, &plan, time_s, 0.5, 2.0),
            }
        })
        .collect();
    plan.with_observation(ObservedEnvelope {
        recording_sha256: "a".repeat(64),
        channel: 0,
        envelope: ThunderEnvelope {
            samples,
            band_low_hz: 20.0,
            band_high_hz: 250.0,
            method: "Synthetic independent integrated energy oracle; not recorded audio".into(),
        },
        reviewed_selection_note: "Fixed fabricated full interval; no fitted parameters".into(),
    })
}
fn run(request: &EnvelopeSearchRequest, catalog: &LightningCatalog) -> EnvelopeReport {
    search(request, catalog, &AtomicBool::new(false), |_| {}).unwrap()
}
fn cosine(a: &[f64], b: &[f64]) -> Option<f64> {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let aa: f64 = a.iter().map(|x| x * x).sum();
    let bb: f64 = b.iter().map(|x| x * x).sum();
    (aa > 0.0 && bb > 0.0).then(|| dot / (aa * bb).sqrt())
}

#[test]
fn every_fft_lag_agrees_with_direct_oracle_and_positive_clock_has_correct_sign() {
    let request = request();
    let catalog = catalog();
    let result = run(&request, &catalog);
    assert!(result.complete && !result.cancelled);
    assert_eq!(result.estimate.offsets_per_receiver, 13);
    assert_eq!(result.progress.evaluated_offsets, 13);
    let observed: Vec<_> = request
        .observation
        .envelope
        .samples
        .iter()
        .map(|x| x.rms)
        .collect();
    for offset_index in 0..13 {
        let offset = -2.0 + offset_index as f64 * 0.5;
        let predicted: Vec<_> = request
            .observation
            .envelope
            .samples
            .iter()
            .map(|x| oracle_rms(&catalog, &request.plan, x.time_s, 0.5, offset))
            .collect();
        let expected = cosine(&observed, &predicted).unwrap();
        let actual = result
            .hypotheses
            .iter()
            .find(|h| h.offset_index == offset_index)
            .unwrap();
        assert!(
            (actual.cosine_similarity - expected).abs() < 2e-12,
            "offset{offset}: {expected} vs{}",
            actual.cosine_similarity
        );
    }
    assert_eq!(result.hypotheses[0].clock_offset_s, 2.0);
    assert_eq!(result.hypotheses[0].recording_start_utc_s, EPOCH + 2.0);
    assert!((result.hypotheses[0].cosine_similarity - 1.0).abs() < 1e-14);
    assert!(!result.location_verified && !result.independent_field_validation);
}

#[test]
fn centered_bins_fractional_arrivals_energy_sum_and_overlay_match_oracle() {
    let request = request();
    let catalog = catalog();
    let result = run(&request, &catalog);
    let view = overlay(&request, &catalog, &result.hypotheses[0]).unwrap();
    assert_eq!(view.times_s[0], 12.5);
    for (time, actual) in view.times_s.iter().zip(&view.predicted_relative_rms) {
        let expected = oracle_rms(&catalog, &request.plan, *time, 0.5, 2.0);
        assert!((actual - expected).abs() < 2e-12, "time{time}");
    }
    // First arrival14s straddles the centered13.75..14.25 receive bin.
    let first = oracle_rms(&catalog, &request.plan, 12.0, 0.5, 2.0);
    assert!(first > 0.0);
    assert!(!view.location_verified);
}

#[test]
fn non_power_lengths_and_exact_edge_peaks_use_complete_windows() {
    for true_offset in [-2.0, 4.0] {
        let mut request = request();
        request.observation.envelope.samples.truncate(17);
        let catalog = catalog();
        for sample in &mut request.observation.envelope.samples {
            sample.rms = oracle_rms(&catalog, &request.plan, sample.time_s, 0.5, true_offset);
        }
        let result = run(&request, &catalog);
        assert_eq!(result.hypotheses[0].clock_offset_s, true_offset);
        assert!(result.hypotheses[0].clock_at_boundary);
        assert_eq!(result.estimate.sample_count, 17);
        assert_eq!(result.estimate.predicted_sample_count, 29);
        assert!((result.hypotheses[0].cosine_similarity - 1.0).abs() < 2e-12);
    }
}

#[test]
fn observed_gain_and_provider_intensity_do_not_change_scores() {
    let mut request = request();
    let mut data = catalog();
    let initial = run(&request, &data);
    for sample in &mut request.observation.envelope.samples {
        sample.rms *= 1e180;
    }
    for strike in &mut data.strikes {
        strike.intensity = Some(1e200);
    }
    let changed = run(&request, &data);
    assert_eq!(initial.hypotheses.len(), changed.hypotheses.len());
    for (a, b) in initial.hypotheses.iter().zip(&changed.hypotheses) {
        assert_eq!(a.offset_index, b.offset_index);
        assert!((a.cosine_similarity - b.cosine_similarity).abs() < 2e-14);
    }
}

#[test]
fn coincident_rows_remain_separate_energy_sources() {
    let request = request();
    let mut data = catalog();
    let initial = run(&request, &data);
    let first = overlay(&request, &data, &initial.hypotheses[0]).unwrap();
    let mut copies = data.strikes.clone();
    for strike in &mut copies {
        strike.id.push_str("-another-original-row");
    }
    data.strikes.extend(copies);
    let changed = run(&request, &data);
    let second = overlay(&request, &data, &changed.hypotheses[0]).unwrap();
    assert_eq!(changed.hypotheses[0].contributing_source_rows, 6);
    for (before, after) in first
        .predicted_relative_rms
        .iter()
        .zip(&second.predicted_relative_rms)
    {
        assert!((after - before * 2.0_f64.sqrt()).abs() < 2e-12);
    }
    assert!(
        (initial.hypotheses[0].cosine_similarity - changed.hypotheses[0].cosine_similarity).abs()
            < 2e-14
    );
}

#[test]
fn silence_is_valid_inside_a_segment_but_zero_observed_energy_is_rejected() {
    let mut request = request();
    request.observation.envelope.samples[0].rms = 0.0;
    preflight(&request, &catalog()).unwrap();
    for sample in &mut request.observation.envelope.samples {
        sample.rms = 0.0;
    }
    assert!(
        preflight(&request, &catalog())
            .unwrap_err()
            .contains("zero energy")
    );
}

#[test]
fn empty_prediction_and_explicit_range_cutoff_are_unavailable_not_matches() {
    let mut request = request();
    request.plan.model.maximum_distance_m = 100.0;
    let result = run(&request, &catalog());
    assert!(result.complete && result.hypotheses.is_empty());
    assert_eq!(result.progress.zero_energy_offsets, 13);
    assert_eq!(result.receivers[0].distance_excluded_source_rows, 3);
    assert_eq!(result.receivers[0].rankable_offsets, 0);
}

#[test]
fn malformed_irregular_masked_and_nonfinite_observations_fail_closed() {
    for change in 0..7 {
        let mut request = request();
        match change {
            0 => request.observation.envelope.samples[4].time_s += 0.01,
            1 => {
                request.observation.envelope.samples.remove(4);
            }
            2 => request.observation.envelope.samples[4].rms = f64::NAN,
            3 => request.observation.envelope.samples[4].rms = -1.0,
            4 => request.observation.envelope.samples[4].time_s = f64::INFINITY,
            5 => request.observation.reviewed_selection_note.clear(),
            _ => request.observation.envelope.method.clear(),
        }
        assert!(preflight(&request, &catalog()).is_err(), "variant{change}");
    }
    let mut json = serde_json::to_value(request()).unwrap();
    json["observation"]["envelope"]["mask"] = serde_json::json!([true, false]);
    assert!(serde_json::from_value::<EnvelopeSearchRequest>(json).is_err());
}

#[test]
fn time_support_requires_full_slant_kernel_and_half_bin_lookback() {
    let mut clipped = request();
    let estimate = preflight(&clipped, &catalog()).unwrap();
    let expected = EPOCH - 2.0 + 12.5 - 0.25 - 5000.0 / 343.0 - 3.0;
    assert!((estimate.required_catalog_utc_interval[0] - expected).abs() < 1e-7);
    clipped.plan.catalog_utc_interval[0] = estimate.required_catalog_utc_interval[0] + 0.01;
    assert!(
        preflight(&clipped, &catalog())
            .unwrap_err()
            .contains("lookback")
    );
    let mut request = request();
    request.plan.catalog_utc_interval[1] = estimate.required_catalog_utc_interval[1] - 0.01;
    assert!(preflight(&request, &catalog()).is_err());
}

#[test]
fn nonintegral_offsets_and_work_memory_overflow_inputs_are_rejected_before_search() {
    for variant in 0..9 {
        let mut request = request();
        match variant {
            0 => request.plan.max_clock_offset_s += 0.1,
            1 => request.plan.limits.max_work_units = 1,
            2 => request.plan.limits.max_memory_bytes = 1,
            3 => request.plan.limits.max_work_units = MAX_WORK_UNITS + 1,
            4 => request.plan.max_clock_offset_s = 1e300,
            5 => request.plan.model.maximum_distance_m = f64::INFINITY,
            6 => request.plan.nominal_recording_start_utc_s = f64::MAX,
            7 => request.plan.receivers.push(receiver("analytic-radial")),
            _ => {
                request.plan.nominal_recording_start_utc_s = 1e30;
                request.plan.min_clock_offset_s = -1e30;
                request.plan.max_clock_offset_s = -1e30;
            }
        }
        assert!(preflight(&request, &catalog()).is_err(), "variant{variant}");
    }
}

#[test]
fn unknown_height_requires_an_explicit_assumption_and_catalog_hash_is_exact() {
    let mut request = request();
    let mut data = catalog();
    data.strikes[0].altitude_m = None;
    assert!(preflight(&request, &data).unwrap_err().contains("height"));
    request.plan.model.fallback_strike_ellipsoid_height_m = Some(343.0);
    preflight(&request, &data).unwrap();
    request.plan.catalog_sha256 = "c".repeat(64);
    assert!(preflight(&request, &data).unwrap_err().contains("hash"));
}

#[test]
fn fmi_interpretation_is_bound_to_exact_catalog_and_kernel_is_not_accuracy() {
    let fmi_request = fmi_lightning::FmiRequest {
        start_utc: "2025-07-19T16:00:00Z".into(),
        end_utc: "2025-07-19T16:01:00Z".into(),
        bounds: [65.0, 13.0, 66.0, 15.0],
        max_total_bytes: fmi_lightning::MAX_TOTAL_BYTES,
    };
    let fmi_plan = fmi_lightning::plan(&fmi_request).unwrap();
    let rows = fmi_lightning::parse_chunk(
        &fmi_plan,
        0,
        include_bytes!("fixtures/fmi/nordlis-2025-07-19-minute.xml"),
    )
    .unwrap();
    let document = fmi_lightning::document(&fmi_plan, &[rows])
        .unwrap()
        .unwrap();
    let base = document.strikes[0].time_utc_s;
    let data = LightningCatalog {
        schema_version: 1,
        input_path: PathBuf::from("frozen-fmi.json"),
        input_bytes: 1,
        input_sha256: "b".repeat(64),
        provenance: document.provenance.unwrap(),
        strikes: document.strikes,
    };
    let mut request = request();
    request.plan.nominal_recording_start_utc_s = base + 30.0;
    request.plan.catalog_utc_interval = [base, base + 60.0];
    request.plan.min_clock_offset_s = -1.0;
    request.plan.max_clock_offset_s = 1.0;
    request.plan.model.kernel_decay_s = 2.0;
    request.plan.model.kernel_duration_s = 4.0;
    request.plan.model.fallback_strike_ellipsoid_height_m = Some(0.0);
    request.observation.envelope.samples = (0..4)
        .map(|i| EnvelopeSample {
            time_s: i as f64,
            rms: 1.0,
        })
        .collect();
    assert!(
        preflight(&request, &data)
            .unwrap_err()
            .contains("interpretation")
    );
    request.plan.source_policy = Some(LightningSourcePolicy {
        catalog_sha256: "b".repeat(64),
        etrs89_as_wgs84_rationale: "Synthetic source-semantics test only".into(),
        time_representation_allowance_s: 1.0,
    });
    preflight(&request, &data).unwrap();
    request.plan.source_policy.as_mut().unwrap().catalog_sha256 = "c".repeat(64);
    assert!(preflight(&request, &data).is_err());
    request.plan.source_policy.as_mut().unwrap().catalog_sha256 = "b".repeat(64);
    request
        .plan
        .source_policy
        .as_mut()
        .unwrap()
        .time_representation_allowance_s = 3.0;
    assert!(preflight(&request, &data).is_err());
}

#[test]
fn cancellation_retains_only_completed_receivers_and_never_claims_full_search() {
    let mut request = request();
    request.plan.receivers.push(receiver("second"));
    request.plan.receivers.push(receiver("third"));
    let cancelled = AtomicBool::new(false);
    let result = search(&request, &catalog(), &cancelled, |progress| {
        if progress.completed_receivers == 1 {
            cancelled.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert!(result.cancelled && !result.complete);
    assert_eq!(result.receivers.len(), 1);
    assert_eq!(result.progress.evaluated_offsets, 13);
    assert!(result.hypotheses.iter().all(|h| h.receiver_index == 0));
    let initial = search(&request, &catalog(), &AtomicBool::new(true), |_| {}).unwrap();
    assert!(initial.cancelled && initial.receivers.is_empty() && !initial.complete);
}

#[test]
fn reports_are_deterministic_inputs_unchanged_and_stale_overlays_rejected() {
    let mut request = request();
    let mut data = catalog();
    let before = serde_json::to_value((&request, &data)).unwrap();
    let first = run(&request, &data);
    let second = run(&request, &data);
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap()
    );
    assert_eq!(before, serde_json::to_value((&request, &data)).unwrap());
    overlay(&request, &data, &first.hypotheses[0]).unwrap();
    let mut altered = first.hypotheses[0].clone();
    altered.clock_offset_s += 0.1;
    assert!(overlay(&request, &data, &altered).is_err());
    data.strikes[0].time_utc_s += 1.0;
    assert!(overlay(&request, &data, &first.hypotheses[0]).is_err());
    data = catalog();
    request.observation.envelope.samples[0].rms += 0.1;
    assert!(overlay(&request, &data, &first.hypotheses[0]).is_err());
}

#[test]
fn a_top_k_boundary_tie_is_explicit_and_never_claims_exact_ordering() {
    let mut request = request();
    request.plan.keep_best = 1;
    request
        .plan
        .receivers
        .push(receiver("identical-geometry-alternative"));
    let result = run(&request, &catalog());
    assert!(result.complete);
    assert_eq!(result.hypotheses.len(), 1);
    assert!(result.retention_boundary_ambiguous);
    assert!(!result.exact_top_k_guaranteed);
    assert_eq!(result.numerical_ordering_tolerance, 1e-7);
}

#[test]
fn single_retained_hypothesis_requires_fft_to_select_the_correct_known_lag() {
    let mut request = request();
    request.plan.keep_best = 1;
    let result = run(&request, &catalog());
    assert_eq!(result.hypotheses.len(), 1);
    assert_eq!(result.hypotheses[0].clock_offset_s, 2.0);
    assert!((result.hypotheses[0].cosine_similarity - 1.0).abs() < 2e-14);
}

#[test]
fn interruptible_overlay_returns_no_partial_curve_and_preserves_normal_result() {
    let request = request();
    let data = catalog();
    let result = run(&request, &data);
    let hypothesis = &result.hypotheses[0];
    assert!(
        overlay_with_cancel(&request, &data, hypothesis, &AtomicBool::new(true))
            .unwrap()
            .is_none()
    );
    let expected = overlay(&request, &data, hypothesis).unwrap();
    let interruptible = overlay_with_cancel(&request, &data, hypothesis, &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(expected).unwrap(),
        serde_json::to_value(interruptible).unwrap()
    );
}
