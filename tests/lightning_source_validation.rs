//! Fabricated source-format and physics controls; no field-accuracy claims.
use geosonnar::{
    evidence_score::{
        self, AssociationKind, CandidateEvidence, EvidenceSet, LightningAltitudeDatum,
        LightningCatalog, LightningDocument, LightningProvenance, LightningStrike,
        ObserverLocation, ScoreConfig, TermSettings, ThunderTimingObservation,
    },
    lightning_source::{self, FmiRecord, FmiSourceMetadata, LightningSourcePolicy},
    map_data,
    thunder::ThunderAnnotation,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

const ORIGIN: f64 = 1_700_000_000.0;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "geosonnar-lightning-source-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn provenance() -> LightningProvenance {
    LightningProvenance {
        provider: "Synthetic FMI-format control".into(),
        source: "Fabricated positions and clock; not an acquired observation".into(),
        license: Some("Synthetic fixture; real FMI attribution is CC BY 4.0".into()),
        notes: None,
        altitude_datum: LightningAltitudeDatum::Unknown,
        source_metadata: Some(FmiSourceMetadata::default()),
    }
}
fn strike() -> LightningStrike {
    LightningStrike {
        id: "fmi-synthetic-0".into(),
        time_utc_s: ORIGIN + 10.0,
        latitude_deg: 65.0,
        longitude_deg: 14.0,
        altitude_m: None,
        intensity: None,
        source_record: Some(FmiRecord {
            response_sha256: "b".repeat(64),
            row_ordinal: 0,
            multiplicity: 0,
            peak_current_ka: -10.5,
            cloud_indicator: 1,
            ellipse_major_raw: 10.0,
        }),
    }
}
fn catalog() -> LightningCatalog {
    LightningCatalog {
        schema_version: 1,
        input_path: PathBuf::from("fabricated-fmi.json"),
        input_bytes: 0,
        input_sha256: "a".repeat(64),
        provenance: provenance(),
        strikes: vec![strike()],
    }
}
fn policy(catalog: &LightningCatalog) -> LightningSourcePolicy {
    LightningSourcePolicy {catalog_sha256:catalog.input_sha256.clone(),
        etrs89_as_wgs84_rationale:"Synthetic geometry control only; exact coordinates are fabricated in WGS84, retaining the declared ETRS89 approximation for the API test".into(),
        time_representation_allowance_s:1.0}
}
fn evidence(catalog: LightningCatalog) -> EvidenceSet {
    EvidenceSet {
        thunder: vec![ThunderTimingObservation {
            annotation: ThunderAnnotation {
                id: "thunder".into(),
                start_s: 15.0,
                end_s: 25.0,
                arrival_s: 21.25,
                uncertainty_s: 0.5,
                reviewed_as_thunder: true,
                source_proposal_id: None,
                note: "Fabricated arrival".into(),
            },
            strike_id: catalog.strikes[0].id.clone(),
            association: AssociationKind::UserReviewed,
        }],
        lightning: Some(catalog),
        ..Default::default()
    }
}
fn candidate() -> CandidateEvidence {
    CandidateEvidence {
        id: "synthetic-receiver".into(),
        observer: ObserverLocation {
            latitude_deg: 65.0,
            longitude_deg: 14.0,
            ellipsoid_height_m: 0.0,
        },
        recording_start_utc_s: ORIGIN,
        frequency: vec![],
        doppler_timing: vec![],
    }
}
fn config(catalog: &LightningCatalog) -> ScoreConfig {
    ScoreConfig {
        frequency: TermSettings {
            enabled: false,
            weight: 1.0,
            residual_scale: 1.0,
        },
        thunder_timing: TermSettings {
            enabled: true,
            weight: 1.0,
            residual_scale: 1.0,
        },
        fallback_strike_ellipsoid_height_m: Some(3430.0),
        lightning_source_policy: Some(policy(catalog)),
        ..Default::default()
    }
}
fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-8, "{actual} != {expected}");
}

#[test]
fn explicit_fmi_policy_preserves_raw_timestamp_and_existing_residual_math() {
    let catalog = catalog();
    let configuration = config(&catalog);
    let evidence = evidence(catalog);
    let before = serde_json::to_value((&evidence, &configuration)).unwrap();
    let report =
        evidence_score::score_candidates(&evidence, &configuration, &[candidate()]).unwrap();
    let residual = &report.candidates[0].terms[0].residuals[0];
    // Same latitude/longitude and a 3430m height difference gives ten seconds
    // travel at343m/s. Source UTC is nominal+10s; no quantization shift is fitted.
    near(residual.predicted, 20.0);
    near(residual.residual, -1.25);
    near(residual.observation_uncertainty, 0.5);
    near(residual.effective_scale, 1.25_f64.sqrt());
    near(residual.normalized_residual, -1.25 / 1.25_f64.sqrt());
    let interpreted = residual
        .thunder_prediction
        .as_ref()
        .unwrap()
        .source_interpretation
        .as_ref()
        .unwrap();
    assert_eq!(interpreted.source_time_resolution_s, 1.0);
    assert_eq!(interpreted.declared_time_representation_allowance_s, 1.0);
    assert!(!interpreted.datum_transform_applied && !interpreted.source_clock_accuracy_calibrated);
    assert!(
        residual
            .thunder_prediction
            .as_ref()
            .unwrap()
            .strike_height_assumed
    );
    assert!(
        report
            .limitations
            .iter()
            .any(|text| text.contains("not a calibrated Gaussian sigma"))
    );
    assert!(
        report
            .limitations
            .iter()
            .any(|text| text.contains("ellipse_major"))
    );
    assert_eq!(
        before,
        serde_json::to_value((&evidence, &configuration)).unwrap()
    );
}

#[test]
fn unresolved_wrong_catalog_small_allowance_and_small_model_scale_block_native_scoring() {
    for bad in 0..7 {
        let catalog = catalog();
        let mut configuration = config(&catalog);
        match bad {
            0 => configuration.lightning_source_policy = None,
            1 => {
                configuration
                    .lightning_source_policy
                    .as_mut()
                    .unwrap()
                    .catalog_sha256 = "c".repeat(64)
            }
            2 => {
                configuration
                    .lightning_source_policy
                    .as_mut()
                    .unwrap()
                    .etrs89_as_wgs84_rationale = "  ".into()
            }
            3 => {
                configuration
                    .lightning_source_policy
                    .as_mut()
                    .unwrap()
                    .time_representation_allowance_s = 0.5
            }
            4 => {
                configuration
                    .lightning_source_policy
                    .as_mut()
                    .unwrap()
                    .time_representation_allowance_s = f64::NAN
            }
            5 => configuration.thunder_timing.residual_scale = 0.75,
            _ => {
                configuration
                    .lightning_source_policy
                    .as_mut()
                    .unwrap()
                    .time_representation_allowance_s = 2.0;
                configuration.thunder_timing.residual_scale = 1.5;
            }
        }
        assert!(
            evidence_score::score_candidates(&evidence(catalog), &configuration, &[candidate()])
                .is_err(),
            "bad={bad}"
        );
    }
}

#[test]
fn inactive_thunder_does_not_block_other_evidence_for_an_unreviewed_fmi_source() {
    let mut data = evidence(catalog());
    data.doppler_timing = vec![evidence_score::TimingObservation {
        id: "cpa".into(),
        event_id: "tone".into(),
        time_s: 10.0,
        uncertainty_s: 0.5,
    }];
    let mut model = candidate();
    model.doppler_timing = vec![evidence_score::TimingPrediction {
        observation_id: "cpa".into(),
        predicted_arrival_utc_s: ORIGIN + 10.0,
    }];
    let configuration = ScoreConfig {
        frequency: TermSettings {
            enabled: false,
            weight: 1.0,
            residual_scale: 1.0,
        },
        doppler_timing: TermSettings {
            enabled: true,
            weight: 1.0,
            residual_scale: 1.0,
        },
        ..Default::default()
    };
    let report = evidence_score::score_candidates(&data, &configuration, &[model]).unwrap();
    assert_eq!(report.candidates[0].score, Some(0.0));
    assert!(
        !report
            .limitations
            .iter()
            .any(|text| text.contains("FMI/NORDLIS"))
    );
}

#[test]
fn fmi_source_metadata_and_rows_cannot_be_dropped_or_reinterpreted_on_import() {
    let original = catalog();
    for bad in 0..10 {
        let mut catalog = original.clone();
        match bad {
            0 => catalog.provenance.source_metadata = None,
            1 => catalog.strikes[0].source_record = None,
            2 => {
                catalog
                    .provenance
                    .source_metadata
                    .as_mut()
                    .unwrap()
                    .time_resolution_s = 0.001
            }
            3 => catalog.strikes[0].time_utc_s += 0.5,
            4 => catalog.provenance.altitude_datum = LightningAltitudeDatum::Wgs84Ellipsoid,
            5 => catalog.strikes[0].altitude_m = Some(0.0),
            6 => catalog.strikes[0].intensity = Some(-10.5),
            7 => {
                catalog.strikes[0]
                    .source_record
                    .as_mut()
                    .unwrap()
                    .response_sha256 = "unbound".into()
            }
            8 => {
                catalog.strikes[0]
                    .source_record
                    .as_mut()
                    .unwrap()
                    .cloud_indicator = 2
            }
            _ => catalog.provenance.license = None,
        }
        assert!(
            lightning_source::validate_catalog_source(&catalog).is_err(),
            "bad={bad}"
        );
    }
    let mut legacy = original;
    legacy.provenance.source_metadata = None;
    legacy.strikes[0].source_record = None;
    assert!(
        lightning_source::validate_catalog_source(&legacy).is_err(),
        "Reserved FMI identity must not masquerade as generic legacy input"
    );
}

#[test]
fn fmi_text_and_saved_policy_have_bounds_before_render_or_replay() {
    for field in 0..4 {
        let mut catalog = catalog();
        match field {
            0 => catalog.provenance.provider = "x".repeat(513),
            1 => catalog.provenance.source = "x".repeat(8193),
            2 => catalog.provenance.license = Some("x".repeat(2049)),
            _ => catalog.provenance.notes = Some("x".repeat(20001)),
        }
        assert!(lightning_source::validate_catalog_source(&catalog).is_err());
    }
    let mut policy = policy(&catalog());
    policy.etrs89_as_wgs84_rationale = "x".repeat(4001);
    assert!(policy.validate_shape().is_err());
}

#[test]
fn embedded_provenance_and_same_second_rows_survive_native_import_map_and_subset() {
    let dir = Directory::new();
    let mut second = strike();
    second.id = "fmi-synthetic-1".into();
    second.source_record.as_mut().unwrap().row_ordinal = 1;
    let document = LightningDocument {
        schema_version: 1,
        strikes: vec![strike(), second],
        provenance: Some(provenance()),
    };
    let bytes = serde_json::to_vec(&document).unwrap();
    let path = dir.write("source.json", &bytes);
    let generic = LightningProvenance {
        provider: "Generic UI reload placeholder".into(),
        source: "Local map".into(),
        license: None,
        notes: None,
        altitude_datum: LightningAltitudeDatum::Wgs84Ellipsoid,
        source_metadata: None,
    };
    let catalog = evidence_score::import_lightning(&path, &generic).unwrap();
    assert_eq!(
        serde_json::to_value(&catalog.provenance).unwrap(),
        serde_json::to_value(document.provenance.as_ref().unwrap()).unwrap()
    );
    assert_eq!(catalog.strikes.len(), 2);
    assert_eq!(catalog.strikes[0].time_utc_s, catalog.strikes[1].time_utc_s);
    let mut repeated_source_row = catalog.clone();
    repeated_source_row.strikes[1]
        .source_record
        .as_mut()
        .unwrap()
        .row_ordinal = 0;
    assert!(
        lightning_source::validate_catalog_source(&repeated_source_row).is_err(),
        "Distinct IDs must not duplicate the same raw provider row"
    );
    let map = map_data::load(&path).unwrap();
    assert_eq!(
        serde_json::to_value(&map.lightning_provenance).unwrap(),
        serde_json::to_value(&document.provenance).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&map.lightning).unwrap(),
        serde_json::to_value(&document.strikes).unwrap()
    );
    assert!(map.notes.iter().any(|note| note.contains("ETRS89")));
    let mut subset = catalog.clone();
    subset.strikes.remove(0);
    lightning_source::validate_policy(&subset, Some(&policy(&catalog)), 1.0).unwrap();
    assert_eq!(
        subset.strikes[0]
            .source_record
            .as_ref()
            .unwrap()
            .row_ordinal,
        1
    );
    let encoded = serde_json::to_vec(&subset).unwrap();
    let decoded: LightningCatalog = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(encoded, serde_json::to_vec(&decoded).unwrap());
    assert_eq!(bytes, fs::read(path).unwrap());
}

#[test]
fn original_legacy_json_shape_and_numerical_score_stay_unchanged() {
    let legacy = serde_json::json!({"schema_version":1,"strikes":[{"id":"legacy","time_utc_s":ORIGIN+10.0,"latitude_deg":65.0,"longitude_deg":14.0,"altitude_m":null,"intensity":null}]});
    let document: LightningDocument = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(&document).unwrap(), legacy);
    assert!(document.provenance.is_none() && document.strikes[0].source_record.is_none());
    let fmi = catalog();
    let mut legacy_catalog = fmi.clone();
    legacy_catalog.provenance.source_metadata = None;
    legacy_catalog.strikes[0].source_record = None;
    legacy_catalog.strikes[0].id = "legacy".into();
    let mut legacy_config = config(&fmi);
    legacy_config.lightning_source_policy = None;
    let a = evidence_score::score_candidates(&evidence(fmi.clone()), &config(&fmi), &[candidate()])
        .unwrap();
    let b =
        evidence_score::score_candidates(&evidence(legacy_catalog), &legacy_config, &[candidate()])
            .unwrap();
    assert_eq!(a.candidates[0].score, b.candidates[0].score);
    let mut residual = serde_json::to_value(&a.candidates[0].terms[0].residuals[0]).unwrap();
    residual["thunder_prediction"]
        .as_object_mut()
        .unwrap()
        .remove("source_interpretation");
    residual["thunder_prediction"]["strike_id"] = serde_json::json!("legacy");
    assert_eq!(
        residual,
        serde_json::to_value(&b.candidates[0].terms[0].residuals[0]).unwrap()
    );
    let saved = serde_json::to_value(geosonnar::workspace::EvidenceMix::default()).unwrap();
    assert!(saved.get("lightning_source_policy").is_none());
}

#[test]
fn policy_change_alters_saved_review_identity_and_keeps_source_bytes_unchanged() {
    let original = geosonnar::workspace::EvidenceMix::default();
    let before = serde_json::to_vec(&original).unwrap();
    let mut reviewed = original.clone();
    reviewed.lightning_source_policy = Some(policy(&catalog()));
    let encoded = serde_json::to_vec(&reviewed).unwrap();
    assert_ne!(before, encoded);
    let decoded: geosonnar::workspace::EvidenceMix = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, reviewed);
    assert_eq!(before, serde_json::to_vec(&original).unwrap());
}
