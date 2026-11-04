//! Fabricated composition, cancellation and integrity controls. These do not
//! measure field accuracy or reuse the held-out pilot's observations/truth.
use geosonnar::{
    casework::{CaseMedia, CaseRecording, SavedCase},
    evidence_score::{
        LightningAltitudeDatum, LightningCatalog, LightningProvenance, LightningStrike,
        ObserverLocation,
    },
    joint_evidence::{
        self as joint, DopplerState, EnvelopeState, JointLimits, JointRequest, JointStage,
        JointStatus,
    },
    map_data,
    regional_search::{self, ClockRange, EventAlternatives, RegionalPlan},
    thunder::{EnvelopeSample, ThunderEnvelope},
    thunder_envelope::{
        self, EnvelopeLimits, EnvelopeModel, EnvelopeReceiver, EnvelopeSearchPlan, ObservedEnvelope,
    },
    workspace::{
        AcousticSourceAssessment, EventKind, EvidenceEvent, ReviewState, ReviewWorkspace,
        SourceBinding, TracePoint,
    },
};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

const ORIGIN: f64 = 1_788_825_600.0;

fn request() -> JointRequest {
    let case = SavedCase {
        schema_version: 1,
        case_id: "joint-core-control".into(),
        title: "Fabricated arithmetic control".into(),
        created_unix_s: 1,
        local_only: true,
        recording: CaseRecording {
            path: PathBuf::from("fabricated.wav"),
            sha256: "a".repeat(64),
            bytes: 1,
            media: CaseMedia {
                duration_s: 120.0,
                sample_rate_hz: 4000,
                channels: 1,
                audio_codec: "fabricated".into(),
            },
        },
    };
    let mut document = ReviewWorkspace::new(&case, 0);
    document.recording_start_utc = "2026-09-08 00:00:00".into();
    document.map.bounds = [-0.1, -0.1, 0.1, 0.1];
    document.mix.doppler_frequency = true;
    document.mix.doppler_timing = false;
    document.mix.thunder_timing = false;
    let mut event = EvidenceEvent::manual("tone".into(), EventKind::Doppler, 19.0, 25.0);
    event.review = ReviewState::Confirmed;
    event.selected_for_scoring = true;
    event.acoustic_source = AcousticSourceAssessment::Compatible;
    event.points = [104.0, 102.0, 100.0, 99.0]
        .iter()
        .enumerate()
        .map(|(i, &frequency_hz)| TracePoint {
            time_s: 20.2 + i as f64,
            frequency_hz,
        })
        .collect();
    document.events.push(event);
    let mut map = map_data::parse_value(&json!({"config":{"time_origin_utc_s":ORIGIN,"max_gap_s":11.0},
        "tracks":[
            {"flight_id":"a","points":(-6..=12).map(|i|json!({"t_s":i as f64*10.0,"xyz_m":[6379137.0,500.0*i as f64,0.0]})).collect::<Vec<_>>()},
            {"flight_id":"b","points":(-6..=12).map(|i|json!({"t_s":i as f64*10.0,"xyz_m":[6379137.0,500.0*i as f64+800.0,0.0]})).collect::<Vec<_>>()}
        ]})).unwrap();
    map.source_path = PathBuf::from("fabricated-tracks.json");
    map.source_sha256 = "b".repeat(64);
    let lightning = LightningCatalog {
        schema_version: 1,
        input_path: PathBuf::from("fabricated-lightning.json"),
        input_bytes: 1,
        input_sha256: "c".repeat(64),
        provenance: LightningProvenance {
            provider: "Fabricated controls".into(),
            source: "Not a measurement or field data".into(),
            license: None,
            notes: None,
            altitude_datum: LightningAltitudeDatum::Wgs84Ellipsoid,
            source_metadata: None,
        },
        strikes: (-14..=24)
            .map(|i| LightningStrike {
                id: format!("strike-{i}"),
                time_utc_s: ORIGIN + i as f64 * 3.0,
                latitude_deg: 0.01,
                longitude_deg: 0.0,
                altitude_m: Some(343.0),
                intensity: None,
                source_record: None,
            })
            .collect(),
    };
    document.datasets = vec![
        SourceBinding {
            path: map.source_path.clone(),
            sha256: map.source_sha256.clone(),
        },
        SourceBinding {
            path: lightning.input_path.clone(),
            sha256: lightning.input_sha256.clone(),
        },
    ];
    let observation = ObservedEnvelope {
        recording_sha256: document.recording_sha256.clone(),
        channel: 0,
        envelope: ThunderEnvelope {
            samples: [1.0, 2.0, 0.3, 1.0]
                .iter()
                .enumerate()
                .map(|(i, &rms)| EnvelopeSample {
                    time_s: 20.128 + i as f64,
                    rms,
                })
                .collect(),
            band_low_hz: 20.0,
            band_high_hz: 250.0,
            method: "Fabricated arithmetic values, not recorded audio".into(),
        },
        reviewed_selection_note: "Fixed fabricated interval; no source fit or truth access".into(),
    };
    let regional_plan = RegionalPlan {
        schema_version: 1,
        clock: ClockRange {
            min_offset_s: -1.0,
            max_offset_s: 1.0,
            step_s: 1.0,
        },
        events: vec![EventAlternatives {
            event_id: "tone".into(),
            source_ids: vec!["a".into(), "b".into()],
            emitted_frequency_hz: Some([80.0, 160.0]),
        }],
        max_evaluations: 1000,
        retain_hypotheses: 1,
    };
    let envelope_plan = EnvelopeSearchPlan {
        schema_version: 1,
        catalog_sha256: lightning.input_sha256.clone(),
        catalog_utc_interval: [ORIGIN - 200.0, ORIGIN + 200.0],
        receivers: vec![
            EnvelopeReceiver {
                id: "first".into(),
                location: ObserverLocation {
                    latitude_deg: 0.0,
                    longitude_deg: 0.0,
                    ellipsoid_height_m: 0.0,
                },
            },
            EnvelopeReceiver {
                id: "second".into(),
                location: ObserverLocation {
                    latitude_deg: 0.005,
                    longitude_deg: 0.002,
                    ellipsoid_height_m: 0.0,
                },
            },
        ],
        nominal_recording_start_utc_s: ORIGIN,
        min_clock_offset_s: -1.0,
        max_clock_offset_s: 1.0,
        model: EnvelopeModel {
            sound_speed_mps: 343.0,
            fallback_strike_ellipsoid_height_m: None,
            minimum_distance_m: 1.0,
            maximum_distance_m: 5000.0,
            kernel_decay_s: 2.0,
            kernel_duration_s: 10.0,
        },
        source_policy: None,
        keep_best: 1,
        limits: EnvelopeLimits::default(),
    };
    JointRequest {
        schema_version: 1,
        document,
        map,
        lightning,
        observation,
        regional_plan,
        envelope_plan,
        limits: JointLimits::default(),
    }
}

fn wide_request() -> JointRequest {
    let mut request = request();
    request.regional_plan.clock.min_offset_s = -20.0;
    request.regional_plan.clock.max_offset_s = 20.0;
    request.envelope_plan.min_clock_offset_s = -20.0;
    request.envelope_plan.max_clock_offset_s = 20.0;
    request
}

// Independent all-pairs connected-component oracle, rather than the native
// sorted-adjacency algorithm. Transitive epsilon connections remain one tie.
fn rank(values: &[f64], index: usize, tolerance: f64) -> f64 {
    let mut member = vec![false; values.len()];
    member[index] = true;
    loop {
        let before = member.iter().filter(|&&v| v).count();
        for a in 0..values.len() {
            for b in 0..values.len() {
                if member[a] && (values[a] - values[b]).abs() <= tolerance {
                    member[b] = true;
                }
            }
        }
        if member.iter().filter(|&&v| v).count() == before {
            break;
        }
    }
    let minimum = values
        .iter()
        .zip(&member)
        .filter(|(_, m)| **m)
        .map(|(v, _)| *v)
        .fold(f64::INFINITY, f64::min);
    let less = values.iter().filter(|&&v| v < minimum).count();
    let count = member.iter().filter(|&&m| m).count();
    less as f64 + (count as f64 + 1.0) / 2.0
}

#[test]
fn full_cell_composition_matches_native_engines_and_independent_rank_oracle() {
    let request = request();
    let original = serde_json::to_vec(&request).unwrap();
    let report = joint::run(&request, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(report.status, JointStatus::Completed);
    assert!(report.all_cells_available && report.ranking_available);
    assert_eq!(report.cells.len(), 6);
    let observers = request
        .envelope_plan
        .receivers
        .iter()
        .map(|r| r.location.clone())
        .collect::<Vec<_>>();
    for (clock_index, &clock) in report.clock_offsets_s.iter().enumerate() {
        let mut plan = request.regional_plan.clone();
        plan.clock.min_offset_s = clock;
        plan.clock.max_offset_s = clock;
        let native = regional_search::run(
            &request.document,
            &request.map,
            &None,
            &observers,
            &plan,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        for (receiver, location) in native.location_summaries.iter().enumerate() {
            let DopplerState::Available {
                score, assignments, ..
            } = &report.cells[receiver * 3 + clock_index].doppler
            else {
                panic!("missing Doppler")
            };
            assert_eq!(Some(*score), location.best_score);
            assert_eq!(
                serde_json::to_value(assignments).unwrap(),
                serde_json::to_value(&location.best_assignments).unwrap()
            );
        }
    }
    for receiver in 0..2 {
        let mut plan = request.envelope_plan.clone();
        plan.receivers = vec![plan.receivers[receiver].clone()];
        plan.keep_best = 3;
        let native = thunder_envelope::search(
            &plan.with_observation(request.observation.clone()),
            &request.lightning,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        for h in native.hypotheses {
            let EnvelopeState::Available {
                cosine_similarity, ..
            } = report.cells[receiver * 3 + h.offset_index].envelope
            else {
                panic!("missing envelope")
            };
            assert_eq!(cosine_similarity, h.cosine_similarity);
        }
    }
    let d = report
        .cells
        .iter()
        .map(|c| match c.doppler {
            DopplerState::Available { score, .. } => score,
            _ => panic!(),
        })
        .collect::<Vec<_>>();
    let e = report
        .cells
        .iter()
        .map(|c| match c.envelope {
            EnvelopeState::Available {
                cosine_similarity, ..
            } => -cosine_similarity,
            _ => panic!(),
        })
        .collect::<Vec<_>>();
    for (index, cell) in report.cells.iter().enumerate() {
        let ranks = cell.ranks.as_ref().unwrap();
        let d_rank = rank(&d, index, 1e-10);
        let e_rank = rank(&e, index, 1e-7);
        assert_eq!(ranks.doppler, d_rank);
        assert_eq!(ranks.envelope, e_rank);
        assert_eq!(ranks.combined, d_rank.max(e_rank));
    }
    assert_eq!(serde_json::to_vec(&request).unwrap(), original);
    joint::validate_report(&request, &report).unwrap();
    assert!(!report.location_verified && !report.independent_field_validation);
}

#[test]
fn forty_one_clocks_retain_every_cell_and_charge_every_tile() {
    let request = wide_request();
    let estimate = joint::estimate(&request).unwrap();
    assert_eq!(
        (estimate.clocks, estimate.cells, estimate.envelope_runs),
        (41, 82, 4)
    );
    let report = joint::run(&request, &AtomicBool::new(false), |_| {}).unwrap();
    assert!(report.ranking_available);
    for receiver in 0..2 {
        for clock in [0, 39, 40] {
            let mut plan = request.envelope_plan.clone();
            plan.receivers = vec![plan.receivers[receiver].clone()];
            plan.min_clock_offset_s = report.clock_offsets_s[clock];
            plan.max_clock_offset_s = plan.min_clock_offset_s;
            plan.keep_best = 1;
            let native = thunder_envelope::search(
                &plan.with_observation(request.observation.clone()),
                &request.lightning,
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap();
            let EnvelopeState::Available {
                cosine_similarity, ..
            } = report.cells[receiver * 41 + clock].envelope
            else {
                panic!()
            };
            assert!((cosine_similarity - native.hypotheses[0].cosine_similarity).abs() < 1e-10);
        }
    }
    joint::validate_report(&request, &report).unwrap();
    let mut capped = request.clone();
    capped.limits.max_envelope_work_units = estimate.envelope_work_units - 1;
    assert!(
        joint::estimate(&capped)
            .unwrap_err()
            .contains("aggregate work")
    );
    capped = request.clone();
    capped.envelope_plan.limits.max_work_units = estimate.envelope_work_units - 1;
    assert!(joint::estimate(&capped).is_err());
}

#[test]
fn cancellation_keeps_completed_values_without_partial_fusion() {
    let request = wide_request();
    let initial = joint::run(&request, &AtomicBool::new(true), |_| {}).unwrap();
    assert_eq!(initial.status, JointStatus::Cancelled);
    assert!(
        initial
            .cells
            .iter()
            .all(|c| matches!(c.doppler, DopplerState::NotStarted)
                && matches!(c.envelope, EnvelopeState::NotStarted))
    );
    joint::validate_report(&request, &initial).unwrap();
    let cancel = AtomicBool::new(false);
    let midway = joint::run(&request, &cancel, |p| {
        if p.stage == JointStage::Doppler && p.completed_doppler_runs == 1 {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert_eq!(midway.progress.completed_doppler_cells, 2);
    assert_eq!(midway.progress.completed_envelope_cells, 0);
    assert_eq!(midway.status, JointStatus::Cancelled);
    assert!(!midway.ranking_available);
    joint::validate_report(&request, &midway).unwrap();
    let cancel = AtomicBool::new(false);
    let midway = joint::run(&request, &cancel, |p| {
        if p.stage == JointStage::Envelope && p.completed_envelope_runs == 1 {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert_eq!(midway.progress.completed_doppler_cells, 82);
    assert_eq!(midway.progress.completed_envelope_cells, 40);
    assert!(!midway.ranking_available);
    joint::validate_report(&request, &midway).unwrap();
}

#[test]
fn unavailable_envelope_is_completed_execution_without_fused_ranks() {
    let mut request = request();
    for (i, strike) in request.lightning.strikes.iter_mut().enumerate() {
        strike.time_utc_s = ORIGIN - 150.0 + i as f64;
    }
    let report = joint::run(&request, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(report.status, JointStatus::Completed);
    assert!(!report.ranking_available && !report.all_cells_available);
    assert!(
        report
            .cells
            .iter()
            .all(|c| matches!(c.doppler, DopplerState::Available { .. })
                && matches!(c.envelope, EnvelopeState::Unavailable { .. })
                && c.ranks.is_none())
    );
    joint::validate_report(&request, &report).unwrap();
}

#[test]
fn identity_domain_selection_and_resource_mismatches_fail_before_work() {
    let original = request();
    let reject = |request: &JointRequest| {
        let mut callbacks = 0;
        assert!(
            joint::run(request, &AtomicBool::new(false), |_| {
                callbacks += 1;
            })
            .is_err()
        );
        assert_eq!(callbacks, 0);
    };
    let mut changed = original.clone();
    changed.observation.recording_sha256 = "d".repeat(64);
    reject(&changed);
    changed = original.clone();
    changed.observation.channel = 1;
    reject(&changed);
    changed = original.clone();
    changed.envelope_plan.nominal_recording_start_utc_s += 1.0;
    reject(&changed);
    changed = original.clone();
    changed.envelope_plan.model.sound_speed_mps = 342.0;
    reject(&changed);
    changed = original.clone();
    changed.regional_plan.clock.step_s = 0.5;
    reject(&changed);
    changed = original.clone();
    changed.envelope_plan.max_clock_offset_s = 0.0;
    reject(&changed);
    changed = original.clone();
    changed.envelope_plan.receivers[1].location.latitude_deg = 0.2;
    reject(&changed);
    changed = original.clone();
    changed.observation.envelope.samples[1].time_s += 0.1;
    reject(&changed);
    changed = original.clone();
    changed.document.mix.doppler_timing = true;
    reject(&changed);
    changed = original.clone();
    changed.document.mix.thunder_timing = true;
    reject(&changed);
    changed = original.clone();
    changed.document.datasets.pop();
    reject(&changed);
    changed = original.clone();
    changed.limits.max_cells = 5;
    reject(&changed);
    changed = original.clone();
    changed.limits.max_serialized_report_bytes = 100;
    reject(&changed);
    changed = original.clone();
    changed.limits.max_working_memory_bytes = 100;
    reject(&changed);
    changed = original.clone();
    changed.map.tracks[0].points[0].latitude_deg += 0.01;
    reject(&changed);
    changed = original.clone();
    changed.document.regional_plan = Some(original.regional_plan.clone());
    changed.regional_plan.events[0].source_ids.pop();
    reject(&changed);
}

#[test]
fn canonical_merged_tracks_need_original_bindings_without_a_fabricated_hash() {
    let mut request = request();
    request.map.source_path = PathBuf::new();
    request.map.source_sha256.clear();
    joint::estimate(&request).unwrap();
    request.document.datasets.remove(0);
    assert!(joint::estimate(&request).is_err());
}

#[test]
fn receiver_ids_cannot_duplicate_physical_positions_to_weight_the_ranking() {
    let mut request = request();
    request.envelope_plan.receivers[1].location =
        request.envelope_plan.receivers[0].location.clone();
    request.envelope_plan.receivers[1].location.latitude_deg = -0.0;
    assert!(
        joint::estimate(&request)
            .unwrap_err()
            .contains("distinct physical positions")
    );
    request.envelope_plan.receivers[1]
        .location
        .ellipsoid_height_m = 1.0;
    joint::estimate(&request).unwrap();
    request.envelope_plan.receivers[1]
        .location
        .ellipsoid_height_m = 0.0;
    request.document.map.bounds = [-0.1, -180.0, 0.1, 180.0];
    request.envelope_plan.receivers[0].location.longitude_deg = -180.0;
    request.envelope_plan.receivers[1].location.longitude_deg = 180.0;
    assert!(
        joint::estimate(&request)
            .unwrap_err()
            .contains("distinct physical positions")
    );
}

#[test]
fn offline_validation_rejects_domain_rank_assignment_and_completion_tampering() {
    let request = request();
    let report = joint::run(&request, &AtomicBool::new(false), |_| {}).unwrap();
    let mut changed = report.clone();
    changed.cells[0].ranks.as_mut().unwrap().combined += 1.0;
    assert!(joint::validate_report(&request, &changed).is_err());
    changed = report.clone();
    changed.cells[0].clock_offset_s += 0.5;
    assert!(joint::validate_report(&request, &changed).is_err());
    changed = report.clone();
    changed.cells[0].receiver_index = 1;
    assert!(joint::validate_report(&request, &changed).is_err());
    changed = report.clone();
    changed.progress.completed_envelope_cells -= 1;
    assert!(joint::validate_report(&request, &changed).is_err());
    changed = report.clone();
    changed.inputs_sha256 = "f".repeat(64);
    assert!(joint::validate_report(&request, &changed).is_err());
    changed = report.clone();
    changed.cells[0].doppler = DopplerState::NotStarted;
    assert!(joint::validate_report(&request, &changed).is_err());
    changed = report.clone();
    changed.location_verified = true;
    assert!(joint::validate_report(&request, &changed).is_err());
    changed = report.clone();
    if let DopplerState::Available { assignments, .. } = &mut changed.cells[0].doppler {
        assignments[0].source_id = "unapproved".into();
    }
    assert!(joint::validate_report(&request, &changed).is_err());
    changed = report.clone();
    if let EnvelopeState::Available {
        cosine_similarity, ..
    } = &mut changed.cells[0].envelope
    {
        *cosine_similarity = f64::NAN;
    }
    assert!(joint::validate_report(&request, &changed).is_err());
    let bytes = serde_json::to_vec(&report).unwrap();
    let reopened = serde_json::from_slice(&bytes).unwrap();
    joint::validate_report(&request, &reopened).unwrap();
}
