use geosonnar::{
    thunder::{EnvelopeSample, ThunderEnvelope},
    thunder_envelope::ObservedEnvelope,
    workspace::{EnvelopeReview, EnvelopeSelection, ReviewWorkspace},
};

fn document() -> ReviewWorkspace {
    serde_json::from_value(serde_json::json!({
        "schema_version":1,"case_id":"envelope-review-control","recording_sha256":"a".repeat(64),
        "channel":0,"duration_s":10.0,"recording_start_utc":"","sound_speed_mps":343.0,
        "events":[],"notes":"","datasets":[],"dataset_options":[],"strike_fallback_height_m":null,
        "mix":{"doppler_frequency":false,"doppler_timing":false,"thunder_timing":false,
            "frequency_weight":1.0,"doppler_timing_weight":1.0,"thunder_weight":1.0,
            "frequency_scale_hz":1.0,"doppler_timing_scale_s":1.0,"thunder_scale_s":1.0},
        "map":{"bounds":[-1.0,-1.0,1.0,1.0],"grid_spacing_m":1000.0,
            "receiver_height_m":0.0,"clock_adjustment_s":0.0}
    }))
    .unwrap()
}

fn measured() -> EnvelopeReview {
    EnvelopeReview {
        selection: EnvelopeSelection {
            start_s: 0.0,
            end_s: 1.0,
            fft_size: 1024,
            hop_s: 0.128,
            band_hz: [20.0, 250.0],
            reviewed_selection_note: "Fixed interval selected explicitly".into(),
        },
        observation: Some(ObservedEnvelope {
            recording_sha256: "a".repeat(64),
            channel: 0,
            envelope: ThunderEnvelope {
                samples: (0..6)
                    .map(|i| EnvelopeSample {
                        time_s: 0.128 + i as f64 * 0.128,
                        rms: 1.0,
                    })
                    .collect(),
                band_low_hz: 23.4375,
                band_high_hz: 250.0,
                method: "Measured PSD".into(),
            },
            reviewed_selection_note: "Fixed interval selected explicitly".into(),
        }),
    }
}

#[test]
fn legacy_workspace_roundtrip_omits_new_optional_selection() {
    let doc = document();
    doc.validate().unwrap();
    assert!(doc.envelope_review.is_none());
    assert!(
        serde_json::to_value(&doc)
            .unwrap()
            .get("envelope_review")
            .is_none()
    );
}

#[test]
fn exact_measured_selection_survives_save_roundtrip() {
    let mut doc = document();
    doc.envelope_review = Some(measured());
    doc.validate().unwrap();
    let restored: ReviewWorkspace =
        serde_json::from_slice(&serde_json::to_vec(&doc).unwrap()).unwrap();
    assert_eq!(restored, doc);
    restored.validate().unwrap();
}

#[test]
fn changed_recording_channel_interval_band_or_note_requires_remeasurement() {
    for mutation in 0..5 {
        let mut doc = document();
        doc.envelope_review = Some(measured());
        match mutation {
            0 => doc.recording_sha256 = "b".repeat(64),
            1 => doc.channel = 1,
            2 => doc.envelope_review.as_mut().unwrap().selection.start_s = 0.01,
            3 => doc.envelope_review.as_mut().unwrap().selection.band_hz = [30.0, 250.0],
            _ => {
                doc.envelope_review
                    .as_mut()
                    .unwrap()
                    .selection
                    .reviewed_selection_note = "Changed".into()
            }
        }
        assert!(doc.validate().is_err());
    }
}

#[test]
fn truncated_gapped_nonfinite_or_silent_observation_is_refused() {
    for mutation in 0..4 {
        let mut doc = document();
        doc.envelope_review = Some(measured());
        let samples = &mut doc
            .envelope_review
            .as_mut()
            .unwrap()
            .observation
            .as_mut()
            .unwrap()
            .envelope
            .samples;
        match mutation {
            0 => {
                samples.pop();
            }
            1 => samples[2].time_s += 0.01,
            2 => samples[0].rms = f64::NAN,
            _ => samples.iter_mut().for_each(|p| p.rms = 0.0),
        }
        assert!(doc.validate().is_err());
    }
}

#[test]
fn edited_unmeasured_draft_remains_explicit_and_cannot_carry_stale_samples() {
    let mut doc = document();
    let mut review = measured();
    review.observation = None;
    review.selection.start_s = 2.0;
    review.selection.end_s = 4.0;
    review.selection.reviewed_selection_note = String::new();
    doc.envelope_review = Some(review);
    doc.validate().unwrap();
    assert!(doc.active_scoring_events().next().is_none());
    assert!(!doc.mix.thunder_timing && !doc.mix.doppler_frequency);
}
