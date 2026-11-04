//! Fabricated locations, times and source files only. No private evidence.
use geosonnar::{evidence_score::*, physics::geodetic_to_ecef, thunder::ThunderAnnotation};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(0);
fn file(extension: &str, bytes: &[u8]) -> PathBuf {
    let root = ROOT.get_or_init(|| {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("output/benchmarks/evidence-score/integration")
            .join(format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&p).unwrap();
        p
    });
    let p = root.join(format!(
        "fabricated-{}.{}",
        NEXT.fetch_add(1, Ordering::Relaxed),
        extension
    ));
    fs::write(&p, bytes).unwrap();
    p
}
fn provenance() -> LightningProvenance {
    LightningProvenance {
        provider: "Synthetic test".into(),
        source: "Fabricated controls, not an observational network".into(),
        license: None,
        notes: None,
        altitude_datum: LightningAltitudeDatum::Wgs84Ellipsoid,
        source_metadata: None,
    }
}
fn strike() -> LightningStrike {
    LightningStrike {
        id: "strike-a".into(),
        time_utc_s: 1_788_825_600.0,
        latitude_deg: 40.01,
        longitude_deg: -75.0,
        altitude_m: Some(120.0),
        intensity: Some(-10.0),
        source_record: None,
    }
}
fn catalog() -> LightningCatalog {
    let p = file(
        "json",
        &serde_json::to_vec(&LightningDocument {
            schema_version: 1,
            strikes: vec![strike()],
            provenance: None,
        })
        .unwrap(),
    );
    import_lightning(&p, &provenance()).unwrap()
}
fn candidate(id: &str) -> CandidateEvidence {
    CandidateEvidence {
        id: id.into(),
        observer: ObserverLocation {
            latitude_deg: 40.0,
            longitude_deg: -75.0,
            ellipsoid_height_m: 120.0,
        },
        recording_start_utc_s: 1_788_825_590.0,
        frequency: vec![
            FrequencyPrediction {
                observation_id: "f1".into(),
                time_s: 2.0,
                predicted_hz: 100.0,
            },
            FrequencyPrediction {
                observation_id: "f2".into(),
                time_s: 3.0,
                predicted_hz: 99.0,
            },
        ],
        doppler_timing: vec![TimingPrediction {
            observation_id: "cpa".into(),
            predicted_arrival_utc_s: 1_788_825_595.0,
        }],
    }
}
fn evidence() -> EvidenceSet {
    let c = candidate("truth");
    let strike = strike();
    let p = geodetic_to_ecef(
        strike.latitude_deg,
        strike.longitude_deg,
        strike.altitude_m.unwrap(),
    )
    .unwrap();
    let o = geodetic_to_ecef(
        c.observer.latitude_deg,
        c.observer.longitude_deg,
        c.observer.ellipsoid_height_m,
    )
    .unwrap();
    let arrival = (strike.time_utc_s - c.recording_start_utc_s)
        + (p[0] - o[0]).hypot(p[1] - o[1]).hypot(p[2] - o[2]) / 343.0;
    EvidenceSet {
        frequency: vec![
            FrequencyObservation {
                id: "f1".into(),
                event_id: "pass-a".into(),
                time_s: 2.0,
                frequency_hz: 100.0,
                uncertainty_hz: 1.0,
            },
            FrequencyObservation {
                id: "f2".into(),
                event_id: "pass-a".into(),
                time_s: 3.0,
                frequency_hz: 99.0,
                uncertainty_hz: 1.0,
            },
        ],
        doppler_timing: vec![TimingObservation {
            id: "cpa".into(),
            event_id: "pass-a".into(),
            time_s: 5.0,
            uncertainty_s: 0.2,
        }],
        thunder: vec![ThunderTimingObservation {
            annotation: ThunderAnnotation {
                id: "rumble-a".into(),
                start_s: arrival - 0.5,
                end_s: arrival + 2.0,
                arrival_s: arrival,
                uncertainty_s: 0.3,
                reviewed_as_thunder: true,
                source_proposal_id: None,
                note: "Fabricated reviewed onset".into(),
            },
            strike_id: "strike-a".into(),
            association: AssociationKind::UserReviewed,
        }],
        lightning: Some(catalog()),
    }
}
fn all_terms() -> ScoreConfig {
    ScoreConfig {
        doppler_timing: TermSettings {
            enabled: true,
            weight: 1.0,
            residual_scale: 0.1,
        },
        thunder_timing: TermSettings {
            enabled: true,
            weight: 1.0,
            residual_scale: 0.2,
        },
        ..Default::default()
    }
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

#[test]
fn known_location_and_shared_clock_beat_wrong_location_and_clock() {
    let evidence = evidence();
    let truth = candidate("correct");
    let mut wrong_location = truth.clone();
    wrong_location.id = "wrong-location".into();
    wrong_location.observer.latitude_deg += 0.08;
    let mut wrong_clock = truth.clone();
    wrong_clock.id = "wrong-clock".into();
    wrong_clock.recording_start_utc_s += 5.0;
    let r = score_candidates(
        &evidence,
        &all_terms(),
        &[wrong_location, wrong_clock, truth],
    )
    .unwrap();
    assert_eq!(r.ranked_candidate_ids[0], "correct");
    assert_eq!(r.candidates[2].rank, Some(1));
    close(r.candidates[2].score.unwrap(), 0.0);
    assert!(r.candidates[..2].iter().all(|c| c.score.unwrap() > 10.0));
    assert!(!r.location_verified);
    assert!(r.candidates.iter().all(|c| !c.location_verified));
    let diagnostic = r.candidates[2].terms[2].residuals[0]
        .thunder_prediction
        .as_ref()
        .unwrap();
    assert!(diagnostic.slant_distance_m > 1000.0);
    assert!(!diagnostic.strike_height_assumed);
}

#[test]
fn missing_selected_evidence_is_unrankable_never_a_better_zero() {
    let mut incomplete = candidate("incomplete");
    incomplete.frequency.pop();
    let mut complete = candidate("complete");
    complete.frequency[0].predicted_hz += 2.0;
    let r = score_candidates(&evidence(), &all_terms(), &[incomplete, complete]).unwrap();
    assert_eq!(r.ranked_candidate_ids, vec!["complete"]);
    assert_eq!(r.candidates[0].score, None);
    assert_eq!(r.candidates[0].rank, None);
    assert!(!r.candidates[0].terms[0].available);
    assert_eq!(r.candidates[0].terms.len(), 3);
    assert!(r.candidates[1].score.unwrap() > 0.0);
}

#[test]
fn disabling_term_removes_its_requirement_for_every_candidate() {
    let mut c = candidate("without-ridge");
    c.frequency.clear();
    let mut config = all_terms();
    config.frequency.enabled = false;
    let r = score_candidates(&evidence(), &config, &[c]).unwrap();
    close(r.candidates[0].score.unwrap(), 0.0);
    assert_eq!(
        r.selected_terms,
        vec![EvidenceTerm::DopplerTiming, EvidenceTerm::ThunderTiming]
    );
    assert_eq!(r.candidates[0].terms.len(), 2);
}

#[test]
fn normalized_residuals_and_user_weights_account_for_the_whole_score() {
    let e = evidence();
    let mut c = candidate("offset");
    for p in &mut c.frequency {
        p.predicted_hz += 2.0;
    }
    c.doppler_timing[0].predicted_arrival_utc_s += 1.0;
    let mut config = all_terms();
    config.frequency.weight = 3.0;
    config.thunder_timing.enabled = false;
    let r = score_candidates(&e, &config, &[c]).unwrap();
    let c = &r.candidates[0];
    close(c.terms[0].mean_normalized_squared_residual.unwrap(), 2.0);
    close(c.terms[1].mean_normalized_squared_residual.unwrap(), 20.0);
    close(c.terms[0].weighted_contribution.unwrap(), 1.5);
    close(c.terms[1].weighted_contribution.unwrap(), 5.0);
    close(c.score.unwrap(), 6.5);
    let d = &c.terms[0].residuals[0];
    close(d.residual, 2.0);
    close(d.effective_scale, 2.0_f64.sqrt());
}

#[test]
fn duplicating_samples_in_one_event_does_not_increase_that_events_weight() {
    let mut e = evidence();
    let mut c = candidate("equal-events");
    e.frequency[1].event_id = "pass-b".into();
    c.frequency[0].predicted_hz += 2.0;
    let config = ScoreConfig::default();
    let original = score_candidates(&e, &config, std::slice::from_ref(&c))
        .unwrap()
        .candidates[0]
        .score
        .unwrap();
    for n in 0..40 {
        let mut o = e.frequency[0].clone();
        o.id = format!("dense-{n}");
        let mut p = c.frequency[0].clone();
        p.observation_id = o.id.clone();
        e.frequency.push(o);
        c.frequency.push(p);
    }
    let dense = score_candidates(&e, &config, &[c]).unwrap().candidates[0]
        .score
        .unwrap();
    close(original, dense);
}

#[test]
fn mismatched_times_duplicate_predictions_and_foreign_ids_cannot_shorten_evidence() {
    for variation in 0..3 {
        let mut c = candidate("invalid");
        match variation {
            0 => c.frequency[0].time_s += 0.1,
            1 => c.frequency[1].observation_id = "f1".into(),
            _ => c.frequency[1].observation_id = "foreign".into(),
        }
        let r = score_candidates(&evidence(), &ScoreConfig::default(), &[c]).unwrap();
        assert_eq!(r.candidates[0].score, None);
        assert!(r.ranked_candidate_ids.is_empty());
    }
}

#[test]
fn unknown_height_requires_explicit_assumption_and_records_it() {
    let mut e = evidence();
    e.lightning.as_mut().unwrap().provenance.altitude_datum = LightningAltitudeDatum::Unknown;
    let mut config = all_terms();
    let c = candidate("candidate");
    assert_eq!(
        score_candidates(&e, &config, std::slice::from_ref(&c))
            .unwrap()
            .candidates[0]
            .score,
        None
    );
    config.fallback_strike_ellipsoid_height_m = Some(120.0);
    let r = score_candidates(&e, &config, &[c]).unwrap();
    close(r.candidates[0].score.unwrap(), 0.0);
    assert!(
        r.candidates[0].terms[2].residuals[0]
            .thunder_prediction
            .as_ref()
            .unwrap()
            .strike_height_assumed
    );
}

#[test]
fn unreviewed_or_unknown_associations_fail_and_exploration_is_explicit() {
    let mut e = evidence();
    let c = candidate("candidate");
    let mut config = all_terms();
    e.thunder[0].annotation.reviewed_as_thunder = false;
    assert!(score_candidates(&e, &config, std::slice::from_ref(&c)).is_err());
    e.thunder[0].annotation.reviewed_as_thunder = true;
    e.thunder[0].strike_id = "missing".into();
    assert!(score_candidates(&e, &config, std::slice::from_ref(&c)).is_err());
    e.thunder[0].strike_id = "strike-a".into();
    e.thunder[0].association = AssociationKind::ExploratoryUserSelected;
    assert!(score_candidates(&e, &config, std::slice::from_ref(&c)).is_err());
    config.allow_exploratory_associations = true;
    let r = score_candidates(&e, &config, &[c]).unwrap();
    assert_eq!(
        r.candidates[0].terms[2].residuals[0]
            .thunder_prediction
            .as_ref()
            .unwrap()
            .association,
        AssociationKind::ExploratoryUserSelected
    );
    assert!(!r.location_verified);
}

#[test]
fn invalid_selected_configuration_and_empty_evidence_fail_closed() {
    let e = evidence();
    let c = candidate("candidate");
    for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut config = ScoreConfig::default();
        config.frequency.weight = value;
        assert!(score_candidates(&e, &config, std::slice::from_ref(&c)).is_err());
    }
    let mut config = ScoreConfig::default();
    config.frequency.enabled = false;
    assert!(score_candidates(&e, &config, std::slice::from_ref(&c)).is_err());
    assert!(score_candidates(&EvidenceSet::default(), &ScoreConfig::default(), &[c]).is_err());
}

#[test]
fn json_and_quoted_csv_import_preserve_native_records_and_provenance() {
    let csv=b"id,time_utc_s,latitude_deg,longitude_deg,altitude_m,intensity\r\n\"strike, \"\"one\"\"\",1788825600.25,40.01,-75,120,-10\r\n";
    let p = file("csv", csv);
    let catalog = import_lightning(&p, &provenance()).unwrap();
    assert_eq!(catalog.strikes[0].id, "strike, \"one\"");
    assert_eq!(catalog.strikes[0].time_utc_s, 1_788_825_600.25);
    assert_eq!(catalog.strikes[0].intensity, Some(-10.0));
    assert_eq!(catalog.input_bytes, csv.len() as u64);
    assert_eq!(catalog.input_sha256.len(), 64);
    let json = file(
        "json",
        &serde_json::to_vec(&LightningDocument {
            schema_version: 1,
            strikes: catalog.strikes.clone(),
            provenance: None,
        })
        .unwrap(),
    );
    let imported = import_lightning(&json, &provenance()).unwrap();
    assert_eq!(
        serde_json::to_value(imported.strikes).unwrap(),
        serde_json::to_value(catalog.strikes).unwrap()
    );
    assert_eq!(imported.provenance.provider, "Synthetic test");
}

#[test]
fn optional_altitude_and_intensity_stay_missing_not_zero() {
    let p = file(
        "csv",
        b"id,time_utc_s,latitude_deg,longitude_deg\na,1788825600,1,2\n",
    );
    let imported = import_lightning(&p, &provenance()).unwrap();
    assert_eq!(imported.strikes[0].altitude_m, None);
    assert_eq!(imported.strikes[0].intensity, None);
}

#[test]
fn malformed_lightning_never_silently_drops_rows_or_columns() {
    for csv in [
        "id,time_utc_s,latitude_deg,longitude_deg\na,1788825600,91,2\n",
        "id,time_utc_s,latitude_deg,longitude_deg\na,1788825600,1,2\na,1788825601,1,2\n",
        "id,time_utc_s,latitude_deg,longitude_deg\na,1788825600000,1,2\n",
        "id,time_utc_s,latitude_deg,longitude_deg\na,1788825600,NaN,2\n",
        "id,time_utc_s,latitude_deg,longitude_deg,unknown\na,1788825600,1,2,3\n",
        "id,time_utc_s,latitude_deg,longitude_deg\n\"a\"oops,1788825600,1,2\n",
        "id,time_utc_s,latitude_deg,longitude_deg\n\"a,1788825600,1,2\n",
        "id,time_utc_s,latitude_deg,longitude_deg\na,1788825600,1\n",
    ] {
        assert!(
            import_lightning(&file("csv", csv.as_bytes()), &provenance()).is_err(),
            "accepted {csv}"
        );
    }
    let bad = file("json", br#"{"schema_version":2,"strikes":[]}"#);
    assert!(import_lightning(&bad, &provenance()).is_err());
}
