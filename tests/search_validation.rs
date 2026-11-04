//! Native search/preflight contracts. Analytic fixtures test behavior only;
//! they are not measurements or a claim of real recording localization.
use geosonnar::physics::{EventTrace, ModelConfig, PairConfig, TracePoint, Track, TrackPoint};
use geosonnar::verification::{
    self, CaseManifest, CoarseBounds, EventWindow, Observer, SearchInput,
};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn repeated_pass_input() -> SearchInput {
    let track = Track {
        flight_id: "return-flight".into(),
        points: (-6..=26)
            .map(|i| {
                let t = i as f64 * 10.0;
                let along = if t <= 100.0 {
                    100.0 * t
                } else {
                    100.0 * (200.0 - t)
                };
                TrackPoint {
                    t_s: t,
                    xyz_m: [6_378_137.0 + 1500.0, along, 0.0],
                }
            })
            .collect(),
    };
    let trace = |center: f64, f0: f64| EventTrace {
        points: (-30..=30)
            .step_by(3)
            .map(|t| {
                let t = t as f64;
                let range = (100.0 * t).hypot(1500.0);
                TracePoint {
                    t_rec_s: center + t + range / 343.0 + 100.0,
                    frequency_hz: f0 * 343.0 / (343.0 + 10000.0 * t / range),
                }
            })
            .collect(),
        f0_hz: vec![f0 - 1.0, f0, f0 + 1.0],
    };
    SearchInput {
        schema_version: 1,
        evidence_kind: "synthetic".into(),
        provenance: vec![
            "Independent analytic forward-time fixture, not recording evidence".into(),
        ],
        tracks: vec![track],
        observers: vec![Observer {
            id: "observer".into(),
            latitude_deg: 0.0,
            longitude_deg: 0.0,
            ellipsoid_height_m: 0.0,
        }],
        events: [trace(0.0, 110.0), trace(200.0, 140.0)],
        pair_config: PairConfig {
            event1_cpa_recording_s: 100.0 + 1500.0 / 343.0,
            shared_shifts_s: vec![0.0],
            clock_offset_bounds_s: None,
            model: ModelConfig::default(),
        },
        coarse_bounds: [
            CoarseBounds {
                speed_mps: [90.0, 110.0],
                slant_distance_m: [1000.0, 2000.0],
            },
            CoarseBounds {
                speed_mps: [90.0, 110.0],
                slant_distance_m: [1000.0, 2000.0],
            },
        ],
        measured_separation_s: 200.0,
        separation_tolerance_s: 0.01,
    }
}

#[test]
fn search_retains_two_passes_from_one_continuous_track_and_exports_their_clock() {
    let input = repeated_pass_input();
    let report = verification::rank(&input).unwrap();
    assert_eq!(report["status"], "hypotheses_ranked");
    assert_eq!(report["location_verified"], false);
    assert_eq!(report["local_cpa_count"], 2);
    assert_eq!(report["timing_candidates"], 1);
    assert_eq!(report["ranked_count"], 1);
    let score = &report["candidates"][0]["score"];
    assert_eq!(
        score["flight_ids"],
        json!(["return-flight", "return-flight"])
    );
    assert!(score["combined_rmse_hz"].as_f64().unwrap() < 1e-10);
    assert_eq!(score["cpa"][0]["emission_time_s"], 0.0);
    assert_eq!(score["cpa"][1]["emission_time_s"], 200.0);
    assert_eq!(score["clock_offset_s"], -100.0);
    let overlays = geosonnar::overlay::export(&input, &report, 1).unwrap();
    assert_eq!(overlays["status"], "overlays_exported");
    assert_eq!(overlays["candidates"][0]["flight_ids"], score["flight_ids"]);
}

#[test]
fn chronological_event_order_is_independent_of_the_order_of_input_tracks() {
    let mut input = repeated_pass_input();
    let mut first = input.tracks[0].clone();
    first.flight_id = "first-chronologically".into();
    first.points.retain(|p| p.t_s <= 60.0);
    let mut second = input.tracks[0].clone();
    second.flight_id = "second-chronologically".into();
    second.points.retain(|p| p.t_s >= 140.0);
    input.tracks = vec![second, first];
    let report = verification::rank(&input).unwrap();
    assert_eq!(report["ranked_count"], 1);
    assert_eq!(
        report["candidates"][0]["score"]["flight_ids"],
        json!(["first-chronologically", "second-chronologically"])
    );
    input.tracks.reverse();
    let forward = verification::rank(&input).unwrap();
    assert_eq!(report["candidates"], forward["candidates"]);
}

#[test]
fn cropped_boundary_pass_is_excluded_with_an_explicit_no_candidate_result() {
    let mut input = repeated_pass_input();
    input.tracks[0].points.retain(|p| p.t_s >= 0.0);
    let report = verification::rank(&input).unwrap();
    assert_eq!(report["status"], "no_candidate");
    assert_eq!(report["boundary_cpa_rejections"], 1);
    assert_eq!(report["timing_candidates"], 0);
    assert_eq!(report["location_verified"], false);
    assert_eq!(report["candidates"], json!([]));
    assert_eq!(report["best_per_flight_pair"], json!([]));
}

#[test]
fn incompatible_separation_or_incomplete_ridge_never_produces_a_candidate() {
    let mut input = repeated_pass_input();
    input.measured_separation_s = 300.0;
    let report = verification::rank(&input).unwrap();
    assert_eq!(report["status"], "no_candidate");
    assert_eq!(report["timing_candidates"], 0);
    let mut input = repeated_pass_input();
    input.tracks[0].points.retain(|p| p.t_s <= 220.0);
    let report = verification::rank(&input).unwrap();
    assert_eq!(report["status"], "no_candidate");
    assert_eq!(report["timing_candidates"], 1);
    assert_eq!(report["scoring_rejections"], 1);
    assert!(
        report["diagnostic_examples"][0]
            .as_str()
            .unwrap()
            .contains("complete track coverage")
    );
}

#[test]
fn duplicate_track_and_observer_identities_are_input_errors() {
    let mut input = repeated_pass_input();
    input.tracks.push(input.tracks[0].clone());
    assert!(
        verification::rank(&input)
            .unwrap_err()
            .contains("Duplicate flight ID")
    );
    let mut input = repeated_pass_input();
    input.observers.push(input.observers[0].clone());
    assert!(
        verification::rank(&input)
            .unwrap_err()
            .contains("identities")
    );
    let mut input = repeated_pass_input();
    input.observers[0].id = " ".into();
    assert!(verification::rank(&input).is_err());
}

#[test]
fn malformed_experiment_is_rejected_before_any_candidate_filtering() {
    let mut input = repeated_pass_input();
    input.observers[0].latitude_deg = 91.0;
    assert!(verification::rank(&input).is_err());
    let mut input = repeated_pass_input();
    input.events[0].points[0].t_rec_s = -1.0;
    assert!(verification::rank(&input).is_err());
    let mut input = repeated_pass_input();
    input.provenance = vec![String::new()];
    assert!(verification::rank(&input).is_err());
    let mut value = serde_json::to_value(repeated_pass_input()).unwrap();
    value["unexpected_clock_override_s"] = json!(10.0);
    assert!(serde_json::from_value::<SearchInput>(value).is_err());
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

struct PreflightFixture {
    root: PathBuf,
    case: CaseManifest,
    search: SearchInput,
}

impl PreflightFixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("output/benchmarks/search/integration")
            .join(format!(
                "preflight-{}-{}",
                std::process::id(),
                UNIQUE.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        // Ten seconds of PCM silence suffice to test media/container contracts.
        // No audio inference or timing claim is made from this fixture.
        let data_size = 8000_u32 * 10 * 2;
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + data_size).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16_u32.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(8000_u32.to_le_bytes());
        wav.extend(16000_u32.to_le_bytes());
        wav.extend(2_u16.to_le_bytes());
        wav.extend(16_u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(data_size.to_le_bytes());
        wav.resize(44 + data_size as usize, 0);
        fs::write(root.join("media.wav"), wav).unwrap();
        let mut search = repeated_pass_input();
        search.evidence_kind = "real_recording".into();
        search.provenance =
            vec!["Artificial preflight input only; no claimed geolocation result".into()];
        search.events = [
            EventTrace {
                points: [1.0, 2.0, 3.0]
                    .into_iter()
                    .map(|t| TracePoint {
                        t_rec_s: t,
                        frequency_hz: 120.0,
                    })
                    .collect(),
                f0_hz: vec![120.0],
            },
            EventTrace {
                points: [6.0, 7.0, 8.0]
                    .into_iter()
                    .map(|t| TracePoint {
                        t_rec_s: t,
                        frequency_hz: 120.0,
                    })
                    .collect(),
                f0_hz: vec![120.0],
            },
        ];
        search.pair_config.event1_cpa_recording_s = 2.0;
        search.measured_separation_s = 5.0;
        let case = CaseManifest {
            schema_version: 1,
            case_id: "preflight-test".into(),
            input: "media.wav".into(),
            recording_date: Some("2024-02-29".into()),
            date_status: "Fixture date for validation only".into(),
            original_time_offset_s: 0.0,
            events: vec![
                EventWindow {
                    event_id: "first".into(),
                    start_s: 1.0,
                    end_s: 3.0,
                    heard_cpa_s: 2.0,
                },
                EventWindow {
                    event_id: "second".into(),
                    start_s: 6.0,
                    end_s: 8.0,
                    heard_cpa_s: 7.0,
                },
            ],
            search_input: Some("search.json".into()),
            notes: vec![],
        };
        Self { root, case, search }
    }

    fn write_case(&self) -> PathBuf {
        let path = self.root.join("case.json");
        verification::write_json(&path, &self.case).unwrap();
        path
    }

    fn check(&self) -> Value {
        verification::write_json(&self.root.join("search.json"), &self.search).unwrap();
        verification::preflight(&self.write_case()).unwrap()
    }
}

fn blocked_for(report: &Value, text: &str) {
    assert_eq!(report["status"], "blocked");
    assert_eq!(report["location_verified"], false);
    assert!(
        report["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b.as_str().unwrap().contains(text)),
        "{report}"
    );
}

#[test]
fn preflight_requires_an_actual_search_contract_not_merely_an_existing_file() {
    let fixture = PreflightFixture::new();
    let report = fixture.check();
    assert_eq!(report["status"], "ready_for_search");
    assert_eq!(report["location_verified"], false);
    assert_eq!(report["search_contract"]["status"], "validated");
    fs::write(fixture.root.join("search.json"), b"{}").unwrap();
    blocked_for(
        &verification::preflight(&fixture.write_case()).unwrap(),
        "Search input is not ready",
    );
    let mut fixture = fixture;
    fixture.search.tracks[0].points[1].t_s = fixture.search.tracks[0].points[0].t_s;
    blocked_for(&fixture.check(), "timestamps");
}

#[test]
fn preflight_validates_calendar_dates_without_silently_rewriting_them() {
    let mut fixture = PreflightFixture::new();
    for valid in ["2024-02-29", "2000-02-29", "2026-09-08"] {
        fixture.case.recording_date = Some(valid.into());
        assert_eq!(fixture.check()["status"], "ready_for_search");
    }
    for invalid in [
        "2026-02-29",
        "1900-02-29",
        "2026-13-01",
        "2026-00-01",
        "2026-04-31",
        "2026-9-8",
        " 2026-09-08",
        "2026-09-08T00:00:00Z",
        "",
    ] {
        fixture.case.recording_date = Some(invalid.into());
        blocked_for(&fixture.check(), "valid YYYY-MM-DD");
    }
    fixture.case.recording_date = None;
    blocked_for(&fixture.check(), "has not been supplied");
}

#[test]
fn preflight_rejects_mismatched_ridges_anchors_separation_and_synthetic_searches() {
    let mut fixture = PreflightFixture::new();
    fixture.search.events[0].points[0].t_rec_s = 0.5;
    blocked_for(&fixture.check(), "outside the case");
    fixture.search.events[0].points[0].t_rec_s = 1.0;
    fixture.search.pair_config.event1_cpa_recording_s = 2.5;
    blocked_for(&fixture.check(), "clock anchor differs");
    fixture.search.pair_config.event1_cpa_recording_s = 2.0;
    fixture.search.measured_separation_s = 6.0;
    blocked_for(&fixture.check(), "measured separation differs");
    fixture.search.measured_separation_s = 5.0;
    fixture.search.evidence_kind = "synthetic".into();
    blocked_for(&fixture.check(), "not a synthetic demo");
}

#[test]
fn preflight_rejects_ambiguous_event_identity_and_reversed_case_windows() {
    let mut fixture = PreflightFixture::new();
    fixture.case.events[1].event_id = fixture.case.events[0].event_id.clone();
    blocked_for(&fixture.check(), "identities");
    fixture.case.events[1].event_id = "second".into();
    fixture.case.events.swap(0, 1);
    blocked_for(&fixture.check(), "ordered and nonoverlapping");
}
