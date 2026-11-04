use geosonnar::{
    evidence_score::{
        LightningAltitudeDatum, LightningCatalog, LightningProvenance, LightningStrike,
        ObserverLocation,
    },
    investigation_archive as archive, investigation_report,
    map_data::{self, MapData},
    regional_search::{
        self, ClockRange, EventAlternatives, RegionalPlan, RegionalReport, SearchStatus,
    },
    workspace::{
        EventKind, EvidenceEvent, EvidenceMix, MapSelection, ReviewState, ReviewWorkspace,
    },
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture {
    doc: ReviewWorkspace,
    map: MapData,
    catalog: Option<LightningCatalog>,
    observer: ObserverLocation,
    plan: RegionalPlan,
}
fn fixture() -> Fixture {
    let mut event = EvidenceEvent::manual("thunder".into(), EventKind::Thunder, 5.0, 15.0);
    event.label = "Reviewed storm <script>alert('unsafe')</script>".into();
    event.arrival_s = 10.0;
    event.selected_for_scoring = true;
    event.review = ReviewState::Confirmed;
    // Fabricated physics control: source class is specified by its generator.
    event.acoustic_source = geosonnar::workspace::AcousticSourceAssessment::Compatible;
    let doc = ReviewWorkspace {
        schema_version: 1,
        case_id: "report-case".into(),
        recording_sha256: "0".repeat(64),
        channel: 0,
        duration_s: 30.0,
        recording_start_utc: "2026-09-08 00:00:00".into(),
        sound_speed_mps: 343.0,
        atmosphere: None,
        regional_plan: None,
        regional_comparison: None,
        envelope_review: None,
        receiver_terrain: None,
        events: vec![event],
        notes: "Private clue <img src=x onerror=alert('unsafe')> & uncertainty".into(),
        mix: EvidenceMix {
            thunder_timing: true,
            ..Default::default()
        },
        map: MapSelection {
            bounds: [-0.1, -0.1, 0.1, 0.1],
            grid_spacing_m: 1000.0,
            receiver_height_m: 0.0,
            clock_adjustment_s: 0.0,
        },
        datasets: Vec::new(),
        dataset_options: Vec::new(),
        strike_fallback_height_m: None,
    };
    let map=map_data::parse_value(&serde_json::json!({"schema_version":1,"strikes":[{"id":"unused-map-strike","time_utc_s":1788825600.0,"latitude_deg":0.0,"longitude_deg":0.0}]})).unwrap();
    let catalog = Some(LightningCatalog {
        schema_version: 1,
        input_path: PathBuf::from("private-source.csv"),
        input_bytes: 0,
        input_sha256: "1".repeat(64),
        provenance: LightningProvenance {
            provider: "Synthetic fixture".into(),
            source: "Independent invented source".into(),
            license: None,
            notes: None,
            altitude_datum: LightningAltitudeDatum::Wgs84Ellipsoid,
            source_metadata: None,
        },
        strikes: vec![LightningStrike {
            id: "strike,\"quoted\"\nline".into(),
            time_utc_s: 1788825606.75,
            latitude_deg: 0.01,
            longitude_deg: 0.0,
            altitude_m: Some(0.0),
            intensity: None,
            source_record: None,
        }],
    });
    let plan = RegionalPlan {
        schema_version: 1,
        clock: ClockRange {
            min_offset_s: -1.0,
            max_offset_s: 1.0,
            step_s: 1.0,
        },
        events: vec![EventAlternatives {
            event_id: "thunder".into(),
            source_ids: vec![catalog.as_ref().unwrap().strikes[0].id.clone()],
            emitted_frequency_hz: None,
        }],
        max_evaluations: 100,
        retain_hypotheses: 3,
    };
    Fixture {
        doc,
        map,
        catalog,
        observer: ObserverLocation {
            latitude_deg: 0.0,
            longitude_deg: 0.0,
            ellipsoid_height_m: 0.0,
        },
        plan,
    }
}
fn report(f: &Fixture) -> RegionalReport {
    regional_search::run(
        &f.doc,
        &f.map,
        &f.catalog,
        std::slice::from_ref(&f.observer),
        &f.plan,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap()
}
struct TestDirectory(PathBuf);
impl TestDirectory {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("output/benchmarks/investigation-report-tests");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        // Only this new fixture subtree; reject links before recursive cleanup.
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        let path = self.0.canonicalize().unwrap();
        assert!(path.starts_with(workspace.join("output/benchmarks/investigation-report-tests")));
        let mut stack = vec![path.clone()];
        while let Some(entry) = stack.pop() {
            let metadata = fs::symlink_metadata(&entry).unwrap();
            assert!(!metadata.file_type().is_symlink());
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                assert_eq!(metadata.file_attributes() & 0x400, 0);
            }
            if metadata.is_dir() {
                for child in fs::read_dir(entry).unwrap() {
                    stack.push(child.unwrap().path());
                }
            }
        }
        fs::remove_dir_all(path).unwrap();
    }
}

#[test]
fn html_is_self_contained_escapes_case_text_and_disclaims_accuracy() {
    let f = fixture();
    let report = report(&f);
    let html = investigation_report::render_html(&f.doc, &report).unwrap();
    assert!(html.contains("&lt;script&gt;alert(&#39;unsafe&#39;)&lt;/script&gt;"));
    assert!(html.contains("&lt;img src=x onerror=alert(&#39;unsafe&#39;)&gt; &amp; uncertainty"));
    assert!(!html.contains("<script"));
    assert!(!html.contains("<img"));
    assert!(!html.contains("<link"));
    assert!(html.contains("default-src 'none'"));
    assert!(html.contains("Location verified: <strong>NO</strong>"));
    assert!(html.contains("2026-09-07 23:59:59.000 UTC"));
    assert!(html.contains("2026-09-08 00:00:01.000 UTC"));
    assert!(html.contains("not probabilities"));
    assert!(html.contains("Modality-specific global best locations cannot be inferred"));
}

#[test]
fn completed_export_preserves_json_and_hashes_every_artifact() {
    let f = fixture();
    let report = report(&f);
    let directory = TestDirectory::new();
    let dest = directory.0.join("dossier");
    let receipt = investigation_report::export(&dest, &f.doc, &report).unwrap();
    assert_eq!(receipt.status, "completed_search_exported");
    assert!(!receipt.location_verified);
    assert_eq!(receipt.artifacts.len(), 5);
    let names: std::collections::BTreeSet<_> = receipt.artifacts.iter().map(|a| &a.path).collect();
    assert_eq!(names.len(), 5);
    for artifact in &receipt.artifacts {
        let bytes = fs::read(dest.join(&artifact.path)).unwrap();
        assert_eq!(artifact.bytes, bytes.len() as u64);
        assert_eq!(artifact.sha256, format!("{:x}", Sha256::digest(&bytes)));
    }
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &fs::read(dest.join("workspace.json")).unwrap()
        )
        .unwrap(),
        serde_json::to_value(&f.doc).unwrap()
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &fs::read(dest.join("regional-report.json")).unwrap()
        )
        .unwrap(),
        serde_json::to_value(&report).unwrap()
    );
    assert!(dest.join("receipt.json").is_file());
    assert!(!dest.join("INCOMPLETE.txt").exists());
}

#[test]
fn cancelled_search_exports_as_partial_with_unevaluated_work_visible() {
    let f = fixture();
    let cancel = AtomicBool::new(false);
    let report = regional_search::run(
        &f.doc,
        &f.map,
        &f.catalog,
        std::slice::from_ref(&f.observer),
        &f.plan,
        &cancel,
        |p| {
            if p.completed_cells > 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    )
    .unwrap();
    assert_eq!(report.status, SearchStatus::Cancelled);
    let html = investigation_report::render_html(&f.doc, &report).unwrap();
    assert!(html.contains("PARTIAL — search cancelled"));
    assert!(html.contains("1/3"));
    assert!(html.contains("Other planned locations"));
    let directory = TestDirectory::new();
    let receipt =
        investigation_report::export(&directory.0.join("partial"), &f.doc, &report).unwrap();
    assert_eq!(receipt.status, "partial_search_exported");
}

#[test]
fn completely_unsupported_run_is_not_presented_as_no_event_evidence() {
    let mut f = fixture();
    f.catalog.as_mut().unwrap().provenance.altitude_datum = LightningAltitudeDatum::Unknown;
    let report = report(&f);
    assert!(report.hypotheses.is_empty());
    assert_eq!(report.unavailable_count, 3);
    let html = investigation_report::render_html(&f.doc, &report).unwrap();
    assert!(html.contains("no rankable hypotheses"));
    assert!(html.contains("does not prove that the event did not occur"));
    assert!(html.contains("Unavailable event/source evaluations: 3"));
}

#[test]
fn stale_snapshot_is_rejected_before_a_directory_is_created() {
    let mut f = fixture();
    let report = report(&f);
    f.doc.notes.push_str("edited after run");
    let directory = TestDirectory::new();
    let destination = directory.0.join("stale");
    assert!(investigation_report::export(&destination, &f.doc, &report).is_err());
    assert!(!destination.exists());
}

#[test]
fn impossible_completion_and_tampered_scores_cannot_make_a_dossier() {
    let f = fixture();
    let original = report(&f);
    let mut report = original.clone();
    report.progress.completed_cells -= 1;
    assert!(investigation_report::render_html(&f.doc, &report).is_err());
    let mut report = original.clone();
    report.location_verified = true;
    assert!(investigation_report::render_html(&f.doc, &report).is_err());
    let mut report = original.clone();
    report.hypotheses[0].score += 1.0;
    assert!(investigation_report::render_html(&f.doc, &report).is_err());
    let mut report = original.clone();
    report.hypotheses[0].recording_start_utc_s += 0.01;
    assert!(investigation_report::render_html(&f.doc, &report).is_err());
    let mut report = original;
    report.hypotheses[0].assignments[0].terms[0].mean_normalized_squared_residual = None;
    assert!(investigation_report::render_html(&f.doc, &report).is_err());
}

#[test]
fn csv_quotes_multiline_fields_and_neutralizes_formula_text() {
    let mut f = fixture();
    f.plan.clock = ClockRange::default();
    let mut report = report(&f);
    report.hypotheses[0].id = "=HYPERLINK(\"https://invalid.example\",\"click\")".into();
    let directory = TestDirectory::new();
    let destination = directory.0.join("csv");
    investigation_report::export(&destination, &f.doc, &report).unwrap();
    let csv = fs::read_to_string(destination.join("hypotheses.csv")).unwrap();
    assert!(csv.contains("\"'=HYPERLINK(\"\"https://invalid.example\"\",\"\"click\"\")\""));
    assert!(csv.contains("strike,\"\"quoted\"\"\nline"));
    assert!(csv.ends_with(",false\r\n"));
}

#[test]
fn existing_destinations_and_parent_traversal_are_refused_without_overwrite() {
    let f = fixture();
    let report = report(&f);
    let directory = TestDirectory::new();
    let destination = directory.0.join("existing");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep.txt"), "unchanged").unwrap();
    assert!(
        investigation_report::export(&destination, &f.doc, &report)
            .unwrap_err()
            .contains("never overwritten")
    );
    assert_eq!(
        fs::read_to_string(destination.join("keep.txt")).unwrap(),
        "unchanged"
    );
    assert!(
        investigation_report::export(&directory.0.join("existing/../escape"), &f.doc, &report)
            .is_err()
    );
    assert!(!directory.0.join("escape").exists());
}

#[test]
fn truncated_shortlist_is_labeled_and_full_report_retains_counts() {
    let mut f = fixture();
    f.plan.retain_hypotheses = 1;
    let report = report(&f);
    assert_eq!(report.supported_hypotheses, Some(3));
    assert!(report.retained_truncated);
    let html = investigation_report::render_html(&f.doc, &report).unwrap();
    assert!(html.contains("Retained <strong>1</strong> hypotheses from <strong>3</strong>"));
    assert!(html.contains("Retained-list truncation: <strong>YES</strong>"));
}

fn comparison_fixture() -> (Fixture, regional_search::ModalityPlan) {
    let mut f = fixture();
    f.doc.mix.doppler_timing = true;
    let mut event = EvidenceEvent::manual("aircraft".into(), EventKind::Doppler, 0.0, 30.0);
    event.review = ReviewState::Confirmed;
    // Fabricated physics control: source class is specified by its generator.
    event.acoustic_source = geosonnar::workspace::AcousticSourceAssessment::Compatible;
    event.selected_for_scoring = true;
    event.arrival_s = 10.0 + 1000.0 / 343.0;
    f.doc.events.push(event);
    let points: Vec<_> = (-2..=6)
        .map(|i| {
            let t = i as f64 * 5.0;
            serde_json::json!({"t_s":t,"xyz_m":[6379137.0,200.0*(t-10.0),0.0]})
        })
        .collect();
    f.map=map_data::parse_value(&serde_json::json!({"config":{"time_origin_utc_s":1788825600.0,"max_gap_s":6.0},"tracks":[{"flight_id":"aircraft-source","points":points}]})).unwrap();
    f.plan.events.push(EventAlternatives {
        event_id: "aircraft".into(),
        source_ids: vec!["aircraft-source".into()],
        emitted_frequency_hz: None,
    });
    let request = regional_search::ModalityPlan {
        regional: f.plan.clone(),
        modes: vec![
            regional_search::SearchMode::DopplerOnly,
            regional_search::SearchMode::ThunderOnly,
            regional_search::SearchMode::Combined,
        ],
        max_total_evaluations: 1000,
    };
    (f, request)
}

#[test]
fn uncertain_source_dossier_is_prominent_escaped_and_preserved_in_json() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    let mut f = fixture();
    let rationale = "Possible handling noise <script>alert(1)</script> & not an identified strike";
    f.doc.events[0].acoustic_source = Assessment::UncertainHypothesis {
        rationale: rationale.into(),
    };
    let result = report(&f);
    let html = investigation_report::render_html(&f.doc, &result).unwrap();
    assert!(html.contains("Acoustic source assessments"));
    assert!(html.contains("UNCERTAIN ACOUSTIC SOURCE"));
    assert!(html.contains("Possible handling noise &lt;script&gt;alert(1)&lt;/script&gt; &amp;"));
    assert!(!html.contains("<script>"));
    assert!(
        html.find("UNCERTAIN ACOUSTIC SOURCE").unwrap()
            < html.find("Retained competing hypotheses").unwrap()
    );
    let root = TestDirectory::new();
    let destination = root.0.join("uncertain-source");
    investigation_report::export(&destination, &f.doc, &result).unwrap();
    let saved: ReviewWorkspace =
        serde_json::from_slice(&fs::read(destination.join("workspace.json")).unwrap()).unwrap();
    assert_eq!(
        saved.events[0].acoustic_source,
        f.doc.events[0].acoustic_source
    );
    let saved_report: RegionalReport =
        serde_json::from_slice(&fs::read(destination.join("regional-report.json")).unwrap())
            .unwrap();
    assert!(
        saved_report
            .limitations
            .iter()
            .any(|warning| warning.contains(rationale))
    );
}

#[test]
fn comparison_dossier_does_not_hide_uncertain_source_assumptions() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    let (mut f, request) = comparison_fixture();
    f.doc.events[0].acoustic_source = Assessment::UncertainHypothesis {
        rationale: "Synthetic thunder class remains uncertain".into(),
    };
    let comparison = regional_search::run_modalities(
        &f.doc,
        &f.map,
        &f.catalog,
        std::slice::from_ref(&f.observer),
        &request,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let html = investigation_report::render_modalities_html(&f.doc, &comparison).unwrap();
    assert!(html.contains("UNCERTAIN ACOUSTIC SOURCE"));
    assert!(html.contains("Synthetic thunder class remains uncertain"));
    assert!(
        html.find("UNCERTAIN ACOUSTIC SOURCE").unwrap()
            < html.find("Mode-specific results").unwrap()
    );
    assert!(!comparison.location_verified);
}

#[test]
fn single_mode_dossier_does_not_claim_excluded_source_uncertainty_as_used_evidence() {
    use geosonnar::workspace::AcousticSourceAssessment as Assessment;
    let (mut f, mut request) = comparison_fixture();
    f.doc.events[0].acoustic_source = Assessment::UncertainHypothesis {
        rationale: "EXCLUDED-THUNDER-ASSUMPTION".into(),
    };
    request.modes = vec![regional_search::SearchMode::DopplerOnly];
    let comparison = regional_search::run_modalities(
        &f.doc,
        &f.map,
        &f.catalog,
        std::slice::from_ref(&f.observer),
        &request,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let html = investigation_report::render_modalities_html(&f.doc, &comparison).unwrap();
    assert!(!html.contains("EXCLUDED-THUNDER-ASSUMPTION"));
    assert!(!html.contains("UNCERTAIN ACOUSTIC SOURCE"));
    assert!(html.contains("Compatible source class"));
}

#[test]
fn independent_comparison_export_preserves_all_modes_and_every_artifact_hash() {
    let (f, request) = comparison_fixture();
    let comparison = regional_search::run_modalities(
        &f.doc,
        &f.map,
        &f.catalog,
        std::slice::from_ref(&f.observer),
        &request,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let directory = TestDirectory::new();
    let destination = directory.0.join("comparison");
    let receipt =
        investigation_report::export_modalities(&destination, &f.doc, &comparison).unwrap();
    assert_eq!(receipt.status, "completed_comparison_exported");
    assert_eq!(receipt.artifacts.len(), 21);
    for artifact in &receipt.artifacts {
        let bytes = fs::read(destination.join(&artifact.path)).unwrap();
        assert_eq!(artifact.sha256, format!("{:x}", Sha256::digest(bytes)));
    }
    for name in ["doppler-only", "thunder-only", "combined"] {
        assert!(destination.join(name).join("index.html").is_file());
        assert!(destination.join(name).join("locations.csv").is_file());
    }
    let html = fs::read_to_string(destination.join("index.html")).unwrap();
    assert!(html.contains("Independent Doppler–thunder peak separation"));
    assert!(!html.contains("<script"));
    assert!(html.contains("&lt;img"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &fs::read(destination.join("comparison.json")).unwrap()
        )
        .unwrap(),
        serde_json::to_value(&comparison).unwrap()
    );
}

#[test]
fn cancelled_comparison_export_cannot_hide_unstarted_modes_behind_completed_subreport() {
    let (f, request) = comparison_fixture();
    let cancel = AtomicBool::new(false);
    let comparison = regional_search::run_modalities(
        &f.doc,
        &f.map,
        &f.catalog,
        std::slice::from_ref(&f.observer),
        &request,
        &cancel,
        |p| {
            if p.completed_modes == 1 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    )
    .unwrap();
    let directory = TestDirectory::new();
    let destination = directory.0.join("partial-comparison");
    let receipt =
        investigation_report::export_modalities(&destination, &f.doc, &comparison).unwrap();
    assert_eq!(receipt.status, "partial_comparison_exported");
    assert!(destination.join("doppler-only/index.html").is_file());
    assert!(!destination.join("thunder-only").exists());
    let html = fs::read_to_string(destination.join("index.html")).unwrap();
    assert!(html.contains("PARTIAL — comparison cancelled"));
    assert!(html.contains("NOT STARTED"));
    assert!(html.contains("peak agreement is not established"));
    let saved = fs::read_to_string(destination.join("comparison.json")).unwrap();
    assert!(saved.contains("not_started"));
}

#[test]
fn omitted_modes_different_receivers_and_malformed_geographic_summaries_are_rejected() {
    let (f, request) = comparison_fixture();
    let original = regional_search::run_modalities(
        &f.doc,
        &f.map,
        &f.catalog,
        std::slice::from_ref(&f.observer),
        &request,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let mut changed = original.clone();
    changed.modes.pop();
    assert!(investigation_report::render_modalities_html(&f.doc, &changed).is_err());
    let mut changed = original.clone();
    changed.modes[1].report.as_mut().unwrap().observers_sha256 = "a".repeat(64);
    assert!(investigation_report::render_modalities_html(&f.doc, &changed).is_err());
    let mut changed = original.clone();
    changed.status = SearchStatus::Cancelled;
    changed.modes[2].status = regional_search::ModeStatus::NotStarted;
    changed.modes[2].report = None;
    assert!(investigation_report::render_modalities_html(&f.doc, &changed).is_err());
    let mut changed = original;
    changed.modes[0].report.as_mut().unwrap().location_summaries[0].best_clock_offset_s = None;
    assert!(investigation_report::render_modalities_html(&f.doc, &changed).is_err());
}

fn bind_replay_sources(f: &mut Fixture, layers: &[MapData]) {
    use geosonnar::workspace::{DatasetOptions, SourceBinding};
    for layer in layers {
        f.doc.datasets.push(SourceBinding {
            path: layer.source_path.clone(),
            sha256: layer.source_sha256.clone(),
        });
        f.doc.dataset_options.push(DatasetOptions {
            source_sha256: layer.source_sha256.clone(),
            time_origin_utc_s: layer.time_origin_utc_s,
            lightning_altitude_wgs84: false,
        });
    }
    if let Some(catalog) = &f.catalog {
        f.doc.datasets.push(SourceBinding {
            path: catalog.input_path.clone(),
            sha256: catalog.input_sha256.clone(),
        });
        f.doc.dataset_options.push(DatasetOptions {
            source_sha256: catalog.input_sha256.clone(),
            time_origin_utc_s: None,
            lightning_altitude_wgs84: true,
        });
    }
    f.map = archive::merge_layers(layers).unwrap();
}
fn read_receipt(path: &Path) -> investigation_report::ExportReceipt {
    serde_json::from_slice(&fs::read(path.join("receipt.json")).unwrap()).unwrap()
}
fn save_receipt(path: &Path, receipt: &investigation_report::ExportReceipt) {
    fs::write(
        path.join("receipt.json"),
        serde_json::to_vec_pretty(receipt).unwrap(),
    )
    .unwrap();
}

#[test]
fn fmi_source_interpretation_survives_regional_subsets_and_archive_reopen() {
    use geosonnar::lightning_source::{FmiRecord, FmiSourceMetadata, LightningSourcePolicy};
    let mut f = fixture();
    let catalog = f.catalog.as_mut().unwrap();
    catalog.provenance.source_metadata = Some(FmiSourceMetadata::default());
    catalog.provenance.altitude_datum = LightningAltitudeDatum::Unknown;
    catalog.provenance.license = Some("Synthetic FMI-format fixture".into());
    let strike = &mut catalog.strikes[0];
    strike.id = "fmi-archive-fixture-0".into();
    strike.time_utc_s = 1788825606.0;
    strike.altitude_m = None;
    strike.source_record = Some(FmiRecord {
        response_sha256: "f".repeat(64),
        row_ordinal: 7,
        multiplicity: 0,
        peak_current_ka: -12.0,
        cloud_indicator: 1,
        ellipse_major_raw: 3.2,
    });
    f.plan.events[0].source_ids = vec![strike.id.clone()];
    f.doc.events[0].strike_id = strike.id.clone();
    f.doc.strike_fallback_height_m = Some(0.0);
    let interpretation = LightningSourcePolicy {
        catalog_sha256: catalog.input_sha256.clone(),
        etrs89_as_wgs84_rationale: "Fabricated geometry; test <approximation> & escaped export"
            .into(),
        time_representation_allowance_s: 1.0,
    };
    assert!(
        regional_search::estimate(&f.doc, &f.map, &f.catalog, &[f.observer.clone()], &f.plan)
            .unwrap_err()
            .contains("explicit")
    );
    f.doc.mix.lightning_source_policy = Some(interpretation);
    bind_replay_sources(&mut f, &[]);
    f.doc.dataset_options[0].lightning_altitude_wgs84 = false;
    let result = report(&f);
    assert!(!result.hypotheses.is_empty());
    assert!(
        result
            .limitations
            .iter()
            .any(|text| text.contains("ETRS89"))
    );
    let snapshot = archive::capture_replay_inputs(&f.doc, &[], &f.catalog).unwrap();
    let root = TestDirectory::new();
    let dest = root.0.join("fmi-replay");
    investigation_report::export_with_replay_inputs(&dest, &f.doc, &result, &snapshot).unwrap();
    let html = fs::read_to_string(dest.join("index.html")).unwrap();
    assert!(html.contains("Declared lightning source interpretation"));
    assert!(html.contains("&lt;approximation&gt; &amp; escaped export"));
    assert!(!html.contains("test <approximation>"));
    let loaded = archive::load(&dest).unwrap();
    let restored = archive::load_replay_inputs(&loaded).unwrap();
    assert_eq!(
        loaded.document.mix.lightning_source_policy,
        f.doc.mix.lightning_source_policy
    );
    assert_eq!(
        serde_json::to_value(&restored.lightning).unwrap(),
        serde_json::to_value(&f.catalog).unwrap()
    );
    let mut broken = restored.clone();
    broken
        .lightning
        .as_mut()
        .unwrap()
        .provenance
        .source_metadata = None;
    assert!(archive::validate_replay_inputs(&f.doc, &broken, &result.inputs_sha256).is_err());
    let mut broken = restored.clone();
    broken.lightning.as_mut().unwrap().strikes[0].source_record = None;
    assert!(archive::validate_replay_inputs(&f.doc, &broken, &result.inputs_sha256).is_err());
    let applied =
        regional_search::apply_hypothesis(&loaded.document, &result, &result.hypotheses[0].id)
            .unwrap();
    let fixed = geosonnar::workspace_scoring::score_observers(
        &applied,
        &f.map,
        &restored.lightning,
        Some(0.0),
        &[f.observer],
    )
    .unwrap();
    assert!(fixed.candidates[0].score.is_some());
    assert!(
        fixed.candidates[0].terms[0].residuals[0]
            .thunder_prediction
            .as_ref()
            .unwrap()
            .source_interpretation
            .is_some()
    );
}
fn rehash_artifact(path: &Path, name: &str) {
    let mut receipt = read_receipt(path);
    let bytes = fs::read(path.join(name)).unwrap();
    let item = receipt
        .artifacts
        .iter_mut()
        .find(|item| item.path == name)
        .unwrap();
    item.bytes = bytes.len() as u64;
    item.sha256 = format!("{:x}", Sha256::digest(bytes));
    save_receipt(path, &receipt);
}

#[test]
fn archive_roundtrip_preserves_exact_review_ordered_buffers_and_provenance() {
    let (mut f, _) = comparison_fixture();
    let mut first = f.map.clone();
    first.source_path = PathBuf::from("missing/source-one.json");
    first.source_sha256 = "a".repeat(64);
    let mut second = first.clone();
    second.source_path = PathBuf::from("missing/source-two.json");
    second.source_sha256 = "b".repeat(64);
    second.tracks[0].id = "second-unused-source".into();
    second.tracks[0].physics_track.flight_id = second.tracks[0].id.clone();
    let layers = vec![first, second];
    bind_replay_sources(&mut f, &layers);
    let replay = archive::capture_replay_inputs(&f.doc, &layers, &f.catalog).unwrap();
    let result = report(&f);
    let root = TestDirectory::new();
    let dest = root.0.join("replay");
    investigation_report::export_with_replay_inputs(&dest, &f.doc, &result, &replay).unwrap();
    let loaded = archive::load(&dest).unwrap();
    assert_eq!(loaded.document, f.doc);
    assert!(loaded.has_replay_inputs);
    assert!(
        matches!(&loaded.report, archive::ReportKind::Regional(r) if r.status == SearchStatus::Completed)
    );
    let restored = archive::load_replay_inputs(&loaded).unwrap();
    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(&replay).unwrap()
    );
    assert_eq!(
        regional_search::inputs_sha256(
            &archive::merge_layers(&restored.layers).unwrap(),
            &restored.lightning
        )
        .unwrap(),
        result.inputs_sha256
    );
    let mut reordered = restored.clone();
    reordered.layers.reverse();
    assert!(archive::validate_replay_inputs(&f.doc, &reordered, &result.inputs_sha256).is_err());
    let mut stale = f.doc.clone();
    stale.dataset_options[0].time_origin_utc_s = Some(1788825601.0);
    assert!(archive::capture_replay_inputs(&stale, &layers, &f.catalog).is_err());
    assert_eq!(
        loaded.document.datasets[0].path,
        PathBuf::from("missing/source-one.json")
    );
}

#[test]
fn legacy_results_view_without_external_sources_does_not_promote_acoustic_assessment() {
    let mut f = fixture();
    let mut result = report(&f);
    f.doc.events[0].acoustic_source = geosonnar::workspace::AcousticSourceAssessment::Unassessed;
    result.document_sha256 = regional_search::document_sha256(&f.doc).unwrap();
    let root = TestDirectory::new();
    let dest = root.0.join("legacy");
    investigation_report::export(&dest, &f.doc, &result).unwrap();
    assert!(
        !fs::read_to_string(dest.join("workspace.json"))
            .unwrap()
            .contains("acoustic_source")
    );
    let loaded = archive::load(&dest).unwrap();
    assert_eq!(loaded.document, f.doc);
    assert!(loaded.document.events[0].selected_for_scoring);
    assert!(!loaded.has_replay_inputs);
    assert!(
        archive::load_replay_inputs(&loaded)
            .unwrap_err()
            .contains("older dossier")
    );
    assert!(
        loaded
            .document
            .validate_acoustic_sources_for_scoring()
            .is_err()
    );
}

#[test]
fn archive_missing_media_allows_results_but_inspection_requires_matching_recording() {
    use geosonnar::casework::{CaseMedia, CaseRecording, SavedCase};
    let root = TestDirectory::new();
    let mut f = fixture();
    bind_replay_sources(&mut f, &[]);
    let media = b"synthetic recording bytes, not field evidence";
    f.doc.recording_sha256 = format!("{:x}", Sha256::digest(media));
    let case = SavedCase {
        schema_version: 1,
        case_id: f.doc.case_id.clone(),
        title: "Replay fixture".into(),
        created_unix_s: 0,
        local_only: true,
        recording: CaseRecording {
            path: PathBuf::from("recording.bin"),
            sha256: f.doc.recording_sha256.clone(),
            bytes: media.len() as u64,
            media: CaseMedia {
                duration_s: f.doc.duration_s,
                sample_rate_hz: 48000,
                channels: 1,
                audio_codec: "synthetic".into(),
            },
        },
    };
    let manifest = root.0.join("case.json");
    fs::write(&manifest, serde_json::to_vec_pretty(&case).unwrap()).unwrap();
    let result = report(&f);
    let replay = archive::capture_replay_inputs(&f.doc, &[], &f.catalog).unwrap();
    let dest = root.0.join("saved");
    investigation_report::export_with_replay_inputs(&dest, &f.doc, &result, &replay).unwrap();
    let loaded = archive::load(&dest).unwrap();
    loaded.matches_case(&case, 0).unwrap();
    assert!(loaded.matches_case(&case, 1).is_err());
    for change in 0..3 {
        let mut other = case.clone();
        match change {
            0 => other.case_id = "other-case".into(),
            1 => other.recording.sha256 = "f".repeat(64),
            _ => other.recording.media.duration_s += 1.0,
        }
        assert!(loaded.matches_case(&other, 0).is_err());
    }
    archive::load_replay_inputs(&loaded).unwrap();
    assert!(
        archive::verify_archived_inspection(&manifest, &loaded, &f.doc, &f.map, &f.catalog)
            .is_err()
    );
    fs::write(root.0.join("recording.bin"), media).unwrap();
    archive::verify_archived_inspection(&manifest, &loaded, &f.doc, &f.map, &f.catalog).unwrap();
    // Archived inspection does not weaken the live Apply/new-score source gate.
    assert!(geosonnar::workspace::verify(&manifest, &f.doc).is_err());
    let mut changed = f.doc.clone();
    changed.notes.push('!');
    assert!(
        archive::verify_archived_inspection(&manifest, &loaded, &changed, &f.map, &f.catalog)
            .is_err()
    );
    let mut changed_catalog = f.catalog.clone();
    changed_catalog.as_mut().unwrap().strikes[0].time_utc_s += 1.0;
    assert!(
        archive::verify_archived_inspection(&manifest, &loaded, &f.doc, &f.map, &changed_catalog)
            .is_err()
    );
    fs::write(root.0.join("recording.bin"), b"altered").unwrap();
    assert!(
        archive::verify_archived_inspection(&manifest, &loaded, &f.doc, &f.map, &f.catalog)
            .is_err()
    );
}

#[test]
fn archive_receipt_tampering_path_injection_and_resource_claims_are_rejected() {
    let f = fixture();
    let result = report(&f);
    let root = TestDirectory::new();
    let dest = root.0.join("tampered");
    investigation_report::export(&dest, &f.doc, &result).unwrap();
    let original = read_receipt(&dest);
    for path in [
        "../workspace.json",
        "C:/secret.json",
        "/secret.json",
        "workspace.json:stream",
        "sub\\file",
        "CON",
        "combined/../../escape",
        "unlisted.json",
    ] {
        let mut receipt = original.clone();
        receipt.artifacts[0].path = path.into();
        save_receipt(&dest, &receipt);
        assert!(archive::load(&dest).is_err(), "accepted {path}");
    }
    let mut receipt = original.clone();
    receipt.artifacts.push(receipt.artifacts[0].clone());
    save_receipt(&dest, &receipt);
    assert!(archive::load(&dest).is_err());
    let mut receipt = original.clone();
    receipt.artifacts[0].bytes = archive::MAX_REPORT_BYTES + 1;
    save_receipt(&dest, &receipt);
    assert!(archive::load(&dest).is_err());
    let mut receipt = original.clone();
    receipt.status = "partial_search_exported".into();
    save_receipt(&dest, &receipt);
    assert!(archive::load(&dest).is_err());
    save_receipt(&dest, &original);
    let html = fs::read(dest.join("index.html")).unwrap();
    fs::write(dest.join("index.html"), b"changed").unwrap();
    assert!(archive::load(&dest).is_err());
    fs::write(dest.join("index.html"), html).unwrap();
    fs::write(dest.join("INCOMPLETE.txt"), "unfinished").unwrap();
    assert!(archive::load(&dest).is_err());
    fs::remove_file(dest.join("INCOMPLETE.txt")).unwrap();
    archive::load(&dest).unwrap();
}

#[test]
fn archive_replay_restore_rechecks_original_receipt_and_native_semantics() {
    let (mut f, _) = comparison_fixture();
    let mut layer = f.map.clone();
    layer.source_path = "missing/track.json".into();
    layer.source_sha256 = "a".repeat(64);
    bind_replay_sources(&mut f, std::slice::from_ref(&layer));
    let replay = archive::capture_replay_inputs(&f.doc, &[layer], &f.catalog).unwrap();
    let result = report(&f);
    let root = TestDirectory::new();
    let dest = root.0.join("malformed-replay");
    investigation_report::export_with_replay_inputs(&dest, &f.doc, &result, &replay).unwrap();
    let loaded = archive::load(&dest).unwrap();
    let mut malformed = replay.clone();
    malformed.layers[0].tracks[0].segments[0][1] = usize::MAX;
    fs::write(
        dest.join("replay-inputs.json"),
        serde_json::to_vec(&malformed).unwrap(),
    )
    .unwrap();
    rehash_artifact(&dest, "replay-inputs.json");
    assert!(
        archive::load_replay_inputs(&loaded)
            .unwrap_err()
            .contains("changed")
    );
    let reopened = archive::load(&dest).unwrap(); // Reading results alone never evaluates arbitrary replay arrays.
    assert!(
        archive::load_replay_inputs(&reopened)
            .unwrap_err()
            .contains("support")
    );
    let mut malformed = replay.clone();
    malformed.layers[0].tracks[0].points[0].longitude_deg += 1.0;
    assert!(
        archive::capture_replay_inputs(&f.doc, &malformed.layers, &malformed.lightning)
            .unwrap_err()
            .contains("coordinates")
    );
    let mut malformed = replay.clone();
    malformed.layers[0].location_verified = true;
    assert!(
        archive::capture_replay_inputs(&f.doc, &malformed.layers, &malformed.lightning).is_err()
    );
    let mut malformed = replay;
    malformed
        .lightning
        .as_mut()
        .unwrap()
        .provenance
        .altitude_datum = LightningAltitudeDatum::Unknown;
    assert!(
        archive::capture_replay_inputs(&f.doc, &malformed.layers, &malformed.lightning).is_err()
    );
}

#[test]
fn archive_comparison_preserves_completed_cancelled_and_unstarted_modes() {
    let (mut f, request) = comparison_fixture();
    let mut layer = f.map.clone();
    layer.source_path = "missing/track.json".into();
    layer.source_sha256 = "a".repeat(64);
    bind_replay_sources(&mut f, std::slice::from_ref(&layer));
    let replay = archive::capture_replay_inputs(&f.doc, &[layer], &f.catalog).unwrap();
    let root = TestDirectory::new();
    for state in 0..3 {
        let cancel = AtomicBool::new(state == 2);
        let result = regional_search::run_modalities(
            &f.doc,
            &f.map,
            &f.catalog,
            std::slice::from_ref(&f.observer),
            &request,
            &cancel,
            |p| {
                if state == 1 && p.completed_modes == 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        )
        .unwrap();
        let dest = root.0.join(format!("comparison-{state}"));
        investigation_report::export_modalities_with_replay_inputs(&dest, &f.doc, &result, &replay)
            .unwrap();
        let loaded = archive::load(&dest).unwrap();
        assert_eq!(loaded.document, f.doc);
        let archive::ReportKind::Comparison(saved) = &loaded.report else {
            panic!("wrong report type")
        };
        assert_eq!(
            serde_json::to_value(saved).unwrap(),
            serde_json::to_value(&result).unwrap()
        );
        archive::load_replay_inputs(&loaded).unwrap();
        assert_eq!(saved.status == SearchStatus::Completed, state == 0);
        if state == 2 {
            assert!(
                saved
                    .modes
                    .iter()
                    .all(|m| m.status == regional_search::ModeStatus::NotStarted
                        && m.report.is_none())
            );
        }
    }
}

#[test]
fn archive_child_replay_cannot_escape_parent_byte_inventory() {
    let (f, request) = comparison_fixture();
    let result = regional_search::run_modalities(
        &f.doc,
        &f.map,
        &f.catalog,
        std::slice::from_ref(&f.observer),
        &request,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let root = TestDirectory::new();
    let dest = root.0.join("comparison");
    investigation_report::export_modalities(&dest, &f.doc, &result).unwrap();
    let child = dest.join("doppler-only");
    // Even a tiny coherent unlisted child payload must be refused before reading it.
    let bytes = b"{}";
    fs::write(child.join("replay-inputs.json"), bytes).unwrap();
    let mut receipt = read_receipt(&child);
    receipt
        .artifacts
        .push(investigation_report::ExportArtifact {
            path: "replay-inputs.json".into(),
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        });
    save_receipt(&child, &receipt);
    rehash_artifact(&dest, "doppler-only/receipt.json");
    assert!(archive::load(&dest).unwrap_err().contains("top-level"));
}

#[test]
#[cfg(windows)]
fn archive_root_and_nested_mode_reparse_points_are_rejected() {
    use std::os::windows::process::CommandExt;
    let (f, request) = comparison_fixture();
    let result = regional_search::run_modalities(
        &f.doc,
        &f.map,
        &f.catalog,
        std::slice::from_ref(&f.observer),
        &request,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let root = TestDirectory::new();
    let dest = root.0.join("comparison");
    investigation_report::export_modalities(&dest, &f.doc, &result).unwrap();
    let junction = root.0.join("linked-root");
    let status = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(junction.to_string_lossy().replace('/', "\\"))
        .arg(dest.to_string_lossy().replace('/', "\\"))
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "junction fixture failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let result = archive::load(&junction);
    fs::remove_dir(&junction).unwrap();
    assert!(result.unwrap_err().contains("reparse"));
    let child = dest.join("doppler-only");
    let moved = root.0.join("moved-mode");
    fs::rename(&child, &moved).unwrap();
    let status = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(child.to_string_lossy().replace('/', "\\"))
        .arg(moved.to_string_lossy().replace('/', "\\"))
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "nested junction fixture failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let result = archive::load(&dest);
    fs::remove_dir(&child).unwrap();
    assert!(result.unwrap_err().contains("reparse"));
}
