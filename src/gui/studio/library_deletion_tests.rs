//! Headless state controls with tiny, owned fabricated inputs. No real media,
//! provider requests, graphics context or scientific accuracy claim.
use super::*;
use geosonnar::{
    casework::{CaseMedia, CaseRecording, SavedCase},
    detailed_data::sha256,
};
use serde_json::json;
use std::{fs, path::Path, sync::atomic::AtomicU64};

static NEXT: AtomicU64 = AtomicU64::new(0);
const TRACKS: &[u8] =
    b"{\"tracks\":[],\"config\":{\"time_origin_utc_s\":1788825600,\"max_gap_s\":30}}\n";
const STRIKES: &[u8] = b"{\"schema_version\":1,\"strikes\":[{\"id\":\"synthetic-strike\",\"time_utc_s\":1788825600,\"latitude_deg\":0,\"longitude_deg\":0,\"altitude_m\":1000,\"intensity\":null}]}\n";

struct Fixture {
    root: PathBuf,
    case: CaseOverview,
}
impl Fixture {
    fn new() -> Self {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
        fs::create_dir_all(&base).unwrap();
        let root = base.join(format!(
            "library-deletion-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let recording = root.join("fabricated.raw");
        fs::write(
            &recording,
            b"Synthetic deletion fixture; not recorded audio",
        )
        .unwrap();
        let case = SavedCase {
            schema_version: 1,
            case_id: root.file_name().unwrap().to_str().unwrap().into(),
            title: "Fabricated deletion control".into(),
            created_unix_s: 1,
            local_only: true,
            recording: CaseRecording {
                path: "fabricated.raw".into(),
                sha256: sha256(&recording).unwrap(),
                bytes: fs::metadata(&recording).unwrap().len(),
                media: CaseMedia {
                    duration_s: 60.,
                    sample_rate_hz: 4000,
                    channels: 1,
                    audio_codec: "fabricated".into(),
                },
            },
        };
        let manifest = root.join("case.json");
        fs::write(&manifest, serde_json::to_vec_pretty(&case).unwrap()).unwrap();
        Self {
            root,
            case: CaseOverview {
                manifest,
                case,
                hints: vec![],
                recording_path: recording,
                recording_verified: Some(true),
                recording_problem: None,
            },
        }
    }
    fn source(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
    fn managed(&self, name: &str, bytes: &[u8], kind: DataKind) -> data_catalog::CatalogEntry {
        let source = self.source(name, bytes);
        data_catalog::import_file(
            &self.case.manifest,
            &source,
            &ImportRequest {
                label: name.into(),
                kind,
                date_utc: None,
                bounds: None,
                notes: "Fabricated deletion control".into(),
                max_bytes: 1024 * 1024,
            },
        )
        .unwrap()
    }
    fn primary(&self, entry: &data_catalog::CatalogEntry) -> PathBuf {
        data_catalog::verified_files(&self.case.manifest, &entry.id)
            .unwrap()
            .into_iter()
            .find(|path| path.ends_with(&entry.primary_file))
            .unwrap()
    }
    fn studio(&self) -> Studio {
        let mut studio = Studio::default();
        studio.bind(&self.case);
        studio.data.refresh_requested = false;
        studio.next_frame = Instant::now() + Duration::from_secs(3600);
        studio.next_progress = Instant::now() + Duration::from_secs(3600);
        let event = EvidenceEvent::manual("reviewed-fixture".into(), EventKind::Doppler, 1., 2.);
        studio.document.as_mut().unwrap().events.push(event);
        studio.selected = Some(0);
        studio
    }
    fn layer(&self, studio: &mut Studio, path: &Path) {
        let layer = map_data::load(path).unwrap();
        let binding = SourceBinding {
            path: layer.source_path.clone(),
            sha256: layer.source_sha256.clone(),
        };
        studio.add_layer(layer, binding);
    }
    fn lightning(&self, studio: &mut Studio, path: &Path) {
        let catalog = evidence_score::import_lightning(
            path,
            &evidence_score::LightningProvenance {
                provider: "Synthetic".into(),
                source: "Fabricated deletion control".into(),
                license: None,
                notes: None,
                altitude_datum: evidence_score::LightningAltitudeDatum::Wgs84Ellipsoid,
                source_metadata: None,
            },
        )
        .unwrap();
        studio
            .document
            .as_mut()
            .unwrap()
            .register_dataset(
                SourceBinding {
                    path: catalog.input_path.clone(),
                    sha256: catalog.input_sha256.clone(),
                },
                None,
                true,
            )
            .unwrap();
        studio.lightning = Some(catalog);
    }
    fn retained_result(&self, studio: &mut Studio) {
        studio.score = Some(evidence_score::ScoreReport {
            schema_version: 1,
            config: Default::default(),
            selected_terms: vec![],
            evidence_sha256: "c".repeat(64),
            ranked_candidate_ids: vec![],
            candidates: vec![],
            calibration: "Fabricated state-preservation control".into(),
            location_verified: false,
            limitations: vec![],
            modalities: None,
        });
        studio.score_document = studio.document.clone();
        let summary = geosonnar::regional_history::RunSummary {
            id: "retained-fixture".into(),
            created_unix_ms: 1,
            status: geosonnar::regional_history::RunStatus::Failed,
            inputs_sha256: "d".repeat(64),
            document_sha256: "e".repeat(64),
            has_result: false,
            error: Some("Fabricated retained outcome".into()),
        };
        studio.run_history.rows = vec![summary.clone()];
        studio.run_history.opened = Some(geosonnar::regional_history::LoadedRun {
            summary,
            document: studio.document.clone().unwrap(),
            report: None,
            joint_request: None,
            result_sha256: None,
        });
        studio.run_history.verified_inputs =
            Some(super::super::inspection::InputIdentity::capture(studio));
        studio.run_history.status = Some((false, "Saved original result".into()));
        self.source(
            "immutable-history.json",
            b"Original saved history fixture bytes",
        );
    }
    fn run(&self, studio: &mut Studio, entry: &data_catalog::CatalogEntry) -> DeletionReply {
        studio.data.delete_run_id += 1;
        let run_id = studio.data.delete_run_id;
        studio.data.deleting = Some((run_id, entry.id.clone()));
        let dependencies = studio.deletion_dependencies();
        delete_with_reconciliation(
            self.case.manifest.clone(),
            run_id,
            entry.id.clone(),
            dependencies,
            data_catalog::delete_owned,
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .canonicalize()
            .unwrap();
        assert!(self.root.starts_with(&base) && self.root.parent() == Some(base.as_path()));
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn review_snapshot(studio: &Studio) -> serde_json::Value {
    json!({"document": studio.document, "score": studio.score, "score_document": studio.score_document,
        "event_selection": studio.selected, "library_selection": studio.data.selected,
        "history": format!("{:?}", studio.run_history.opened), "history_rows": format!("{:?}", studio.run_history.rows),
        "history_status": studio.run_history.status})
}
fn data_snapshot(studio: &Studio) -> serde_json::Value {
    json!({"layers": studio.layers, "lightning": studio.lightning, "revision": studio.loaded_data_revision,
        "unavailable": studio.data.unavailable_sources})
}

#[test]
fn deleting_unused_managed_copy_preserves_identical_original_and_all_review_state() {
    let f = Fixture::new();
    let entry = f.managed("tracks.json", TRACKS, DataKind::Adsb);
    let mut studio = f.studio();
    f.layer(&mut studio, &f.root.join("tracks.json")); // Same hash, different physical copy.
    let strikes = f.source("lightning.json", STRIKES);
    f.lightning(&mut studio, &strikes);
    f.retained_result(&mut studio);
    studio.data.selected = Some("another-selected-dataset".into());
    let before = review_snapshot(&studio);
    let data = data_snapshot(&studio);
    let verified = studio.run_history.verified_inputs.clone();
    let history_hash = sha256(&f.root.join("immutable-history.json")).unwrap();
    let reply = f.run(&mut studio, &entry);
    assert!(reply.outcome.is_ok());
    studio.accept_dataset_deletion(reply);
    assert_eq!(review_snapshot(&studio), before);
    assert_eq!(data_snapshot(&studio), data);
    assert_eq!(studio.run_history.verified_inputs, verified);
    assert_eq!(
        sha256(&f.root.join("immutable-history.json")).unwrap(),
        history_hash
    );
    assert!(f.root.join("tracks.json").is_file());
}

#[test]
fn deleting_loaded_copy_unloads_only_its_layer_and_retains_historical_numerical_result() {
    let f = Fixture::new();
    let entry = f.managed("tracks.json", TRACKS, DataKind::Adsb);
    let primary = f.primary(&entry);
    let mut studio = f.studio();
    f.layer(&mut studio, &primary);
    let other = f.source(
        "other.json",
        b"{\"tracks\":[],\"config\":{\"time_origin_utc_s\":1788825601,\"max_gap_s\":30}}\n",
    );
    f.layer(&mut studio, &other);
    let strikes = f.source("strikes.json", STRIKES);
    f.lightning(&mut studio, &strikes);
    f.retained_result(&mut studio);
    let before = review_snapshot(&studio);
    let other_layer = serde_json::to_value(&studio.layers[1]).unwrap();
    let lightning = serde_json::to_value(&studio.lightning).unwrap();
    let reply = f.run(&mut studio, &entry);
    studio.accept_dataset_deletion(reply);
    assert_eq!(review_snapshot(&studio), before);
    assert_eq!(studio.layers.len(), 1);
    assert_eq!(
        serde_json::to_value(&studio.layers[0]).unwrap(),
        other_layer
    );
    assert_eq!(serde_json::to_value(&studio.lightning).unwrap(), lightning);
    assert!(studio.run_history.verified_inputs.is_none());
    assert!(!studio.deleted_sources_ready(studio.document.as_ref().unwrap()));
    assert!(!studio.report_inputs_ready());
    assert!(!primary.exists());
    assert!(
        studio
            .notice
            .as_ref()
            .unwrap()
            .1
            .contains("saved results remain")
    );
}

#[test]
fn deleting_lightning_preserves_aircraft_and_requires_exact_source_reload() {
    let f = Fixture::new();
    let entry = f.managed("strikes.json", STRIKES, DataKind::Lightning);
    let primary = f.primary(&entry);
    let mut studio = f.studio();
    f.lightning(&mut studio, &primary);
    let tracks = f.source("tracks.json", TRACKS);
    f.layer(&mut studio, &tracks);
    f.retained_result(&mut studio);
    let before = review_snapshot(&studio);
    let layers = serde_json::to_value(&studio.layers).unwrap();
    let original = studio
        .document
        .as_ref()
        .unwrap()
        .datasets
        .iter()
        .find(|b| b.path == primary)
        .unwrap()
        .clone();
    let reply = f.run(&mut studio, &entry);
    studio.accept_dataset_deletion(reply);
    assert!(studio.lightning.is_none());
    assert_eq!(serde_json::to_value(&studio.layers).unwrap(), layers);
    assert_eq!(review_snapshot(&studio), before);
    studio.restore_deleted_source(&SourceBinding {
        path: primary.clone(),
        sha256: "f".repeat(64),
    });
    assert!(!studio.deleted_sources_ready(studio.document.as_ref().unwrap()));
    // The successful load callback uses this exact identity only after verifying bytes.
    studio.restore_deleted_source(&original);
    assert!(studio.deleted_sources_ready(studio.document.as_ref().unwrap()));
    assert!(
        studio.run_history.verified_inputs.is_none(),
        "Reload does not reverify a saved report automatically"
    );
}

#[test]
fn native_deletion_refusal_preserves_layers_selections_scores_and_history_verification() {
    let f = Fixture::new();
    let entry = f.managed("tracks.json", TRACKS, DataKind::Adsb);
    let primary = f.primary(&entry);
    let mut studio = f.studio();
    f.layer(&mut studio, &primary);
    f.retained_result(&mut studio);
    fs::write(&primary, b"Changed source; native deletion must refuse").unwrap();
    let before = review_snapshot(&studio);
    let data = data_snapshot(&studio);
    let verified = studio.run_history.verified_inputs.clone();
    let reply = f.run(&mut studio, &entry);
    assert!(reply.outcome.is_err());
    assert!(reply.newly_unavailable.is_empty());
    studio.accept_dataset_deletion(reply);
    assert_eq!(review_snapshot(&studio), before);
    assert_eq!(data_snapshot(&studio), data);
    assert_eq!(studio.run_history.verified_inputs, verified);
    assert!(primary.exists());
    assert!(studio.notice.as_ref().unwrap().1.contains("were preserved"));
}

#[test]
fn partial_removal_reconciles_only_missing_dependencies_and_preserves_results() {
    let f = Fixture::new();
    let entry = f.managed("tracks.json", TRACKS, DataKind::Adsb);
    let primary = f.primary(&entry);
    let mut studio = f.studio();
    f.layer(&mut studio, &primary);
    let strikes = f.source("other-strikes.json", STRIKES);
    f.lightning(&mut studio, &strikes);
    f.retained_result(&mut studio);
    let before = review_snapshot(&studio);
    let lightning = serde_json::to_value(&studio.lightning).unwrap();
    studio.data.deleting = Some((1, entry.id.clone()));
    studio.data.delete_run_id = 1;
    let reply = delete_with_reconciliation(
        f.case.manifest.clone(),
        1,
        entry.id.clone(),
        vec![primary.clone(), strikes],
        |_, _| {
            fs::write(
                primary.parent().unwrap().join("deletion.json"),
                serde_json::to_vec_pretty(&entry.files).unwrap(),
            )
            .unwrap();
            fs::remove_file(&primary).unwrap();
            Err("Injected interruption after one actual file removal".into())
        },
    );
    assert!(reply.incomplete);
    assert_eq!(reply.newly_unavailable, vec![primary.clone()]);
    studio.accept_dataset_deletion(reply);
    assert!(studio.layers.is_empty());
    assert_eq!(serde_json::to_value(&studio.lightning).unwrap(), lightning);
    assert_eq!(review_snapshot(&studio), before);
    assert!(studio.run_history.verified_inputs.is_none());
    assert!(studio.notice.as_ref().unwrap().1.contains("incomplete"));
    let resumed = f.run(&mut studio, &entry);
    assert!(resumed.outcome.is_ok());
    studio.accept_dataset_deletion(resumed);
    assert_eq!(review_snapshot(&studio), before);
}

#[test]
fn partial_indexed_block_removal_invalidates_surviving_index_and_preserves_other_evidence() {
    let f = Fixture::new();
    let entry = f.managed("owned-container.json", TRACKS, DataKind::Adsb);
    let directory = deletion_directory(&f.case.manifest, &entry.id).unwrap();
    fs::create_dir(directory.join("native")).unwrap();
    fs::create_dir(directory.join("native/blocks")).unwrap();
    let primary = directory.join("native/index.json");
    let block = directory.join("native/blocks/test.bin");
    fs::write(
        &primary,
        b"Fabricated already-verified index identity for lifecycle control",
    )
    .unwrap();
    fs::write(&block, b"Fabricated original block bytes").unwrap();
    let mut studio = f.studio();
    let before_source = studio.document.clone().unwrap();
    studio.accept_wide_reply(super::super::wide_ui::WideReply::Source(
        super::super::wide_ui::IndexedSource {
            binding: geosonnar::track_store::TrackStoreBinding {
                manifest_path: primary.clone(),
                manifest_sha256: sha256(&primary).unwrap(),
            },
            label: "Fabricated indexed source".into(),
            passes: 1,
            points: 2,
            origin: 0.,
            utc_window: [0., 86400.],
            max_gap_s: 30.,
        },
        Box::new(before_source),
    ));
    assert!(studio.wide_dependency_paths().any(|path| path == primary));
    let other = f.source("independent-tracks.json", TRACKS);
    f.layer(&mut studio, &other);
    f.retained_result(&mut studio);
    let before = review_snapshot(&studio);
    let layers = serde_json::to_value(&studio.layers).unwrap();
    studio.data.deleting = Some((1, entry.id.clone()));
    studio.data.delete_run_id = 1;
    let refused = delete_with_reconciliation_members(
        f.case.manifest.clone(),
        1,
        entry.id.clone(),
        studio.deletion_dependencies(),
        Some(IndexedDeletionMembers {
            primary: "native/index.json".into(),
            members: vec!["native/index.json".into(), "native/blocks/test.bin".into()],
        }),
        |_, _| Err("Injected refusal before any filesystem mutation".into()),
    );
    assert!(refused.newly_unavailable.is_empty());
    studio.accept_dataset_deletion(refused);
    assert!(studio.wide_dependency_paths().any(|path| path == primary));
    assert!(studio.deleted_sources_ready(studio.document.as_ref().unwrap()));
    assert_eq!(review_snapshot(&studio), before);
    studio.data.deleting = Some((2, entry.id.clone()));
    studio.data.delete_run_id = 2;
    let reply = delete_with_reconciliation_members(
        f.case.manifest.clone(),
        2,
        entry.id.clone(),
        studio.deletion_dependencies(),
        Some(IndexedDeletionMembers {
            primary: "native/index.json".into(),
            members: vec!["native/index.json".into(), "native/blocks/test.bin".into()],
        }),
        |_, _| {
            fs::write(
                directory.join("deletion.json"),
                b"Fabricated interrupted deletion marker",
            )
            .unwrap();
            fs::remove_file(&block).unwrap();
            Err("Injected interruption after actual block removal".into())
        },
    );
    assert!(reply.incomplete && reply.outcome.is_err());
    assert!(primary.exists() && !block.exists());
    assert!(reply.newly_unavailable.contains(&primary));
    studio.accept_dataset_deletion(reply);
    assert!(!studio.wide_dependency_paths().any(|path| path == primary));
    assert!(!studio.deleted_sources_ready(studio.document.as_ref().unwrap()));
    assert_eq!(review_snapshot(&studio), before);
    assert_eq!(serde_json::to_value(&studio.layers).unwrap(), layers);
    assert!(studio.score.is_some());
    assert!(studio.run_history.verified_inputs.is_none());
    assert!(studio.notice.as_ref().unwrap().1.contains("incomplete"));
}

#[test]
fn deletion_reply_cannot_clear_newer_work_or_cross_case_boundaries() {
    let f = Fixture::new();
    let entry = f.managed("tracks.json", TRACKS, DataKind::Adsb);
    let mut studio = f.studio();
    let old_generation = studio.generation;
    let reply = f.run(&mut studio, &entry);
    let old_id = reply.run_id;
    studio.data.deleting = Some((old_id + 1, "newer".into()));
    studio.busy = Some("Newer operation".into());
    let before = review_snapshot(&studio);
    studio
        .sender
        .send(Reply {
            generation: old_generation,
            lane: WorkerLane::DataDeleteTask(old_id),
            result: Ok(ResultValue::DatasetDeleted(Box::new(reply))),
        })
        .unwrap();
    studio.tick(&egui::Context::default());
    assert_eq!(studio.busy.as_deref(), Some("Newer operation"));
    assert_eq!(review_snapshot(&studio), before);
    let reply = f.run(&mut studio, &entry);
    let other = Fixture::new();
    studio.bind(&other.case);
    studio.data.refresh_requested = false;
    studio.busy = Some("Other case operation".into());
    let before = review_snapshot(&studio);
    studio
        .sender
        .send(Reply {
            generation: old_generation,
            lane: WorkerLane::DataDeleteTask(reply.run_id),
            result: Ok(ResultValue::DatasetDeleted(Box::new(reply))),
        })
        .unwrap();
    studio.tick(&egui::Context::default());
    assert_eq!(studio.busy.as_deref(), Some("Other case operation"));
    assert_eq!(review_snapshot(&studio), before);
}

#[test]
fn deleting_active_height_source_keeps_the_model_instead_of_falling_back() {
    let f = Fixture::new();
    let entry = f.managed(
        "terrain.json",
        b"Synthetic terrain bytes",
        DataKind::Terrain,
    );
    let primary = f.primary(&entry);
    let binding = SourceBinding {
        path: primary,
        sha256: entry.files[0].sha256.clone(),
    };
    let mut studio = f.studio();
    studio
        .document
        .as_mut()
        .unwrap()
        .register_dataset(binding.clone(), None, false)
        .unwrap();
    studio.document.as_mut().unwrap().receiver_terrain =
        Some(geosonnar::receiver_heights::ReceiverTerrain {
            surface_sha256s: vec![binding.sha256],
            surface_datum: geosonnar::receiver_heights::SurfaceDatum::Wgs84Ellipsoid,
            geoid: None,
            above_surface_m: 1.5,
            notes: "Fabricated explicit model".into(),
        });
    studio.heights.preview = Some(geosonnar::receiver_heights::ReceiverHeightReport {
        schema_version: 1,
        settings: studio.document.as_ref().unwrap().receiver_terrain.clone(),
        observers: vec![Some(geosonnar::evidence_score::ObserverLocation {
            latitude_deg: 0.,
            longitude_deg: 0.,
            ellipsoid_height_m: 101.5,
        })],
        samples: vec![],
        unavailable: vec![],
        sources: vec![],
        limitations: vec!["Fabricated cached height".into()],
    });
    studio.heights.preview_document = studio.document.clone();
    assert!(studio.heights.preview.is_some());
    f.retained_result(&mut studio);
    let before = review_snapshot(&studio);
    let reply = f.run(&mut studio, &entry);
    studio.accept_dataset_deletion(reply);
    assert_eq!(review_snapshot(&studio), before);
    assert!(studio.document.as_ref().unwrap().receiver_terrain.is_some());
    assert!(!studio.deleted_sources_ready(studio.document.as_ref().unwrap()));
    assert!(studio.heights.preview.is_none());
    assert!(studio.heights.preview_document.is_none());
}

#[test]
fn deleting_source_retained_only_by_historical_result_blocks_reverted_review_not_current_inputs() {
    use geosonnar::regional_search::{RegionalPlan, RegionalReport, SearchEstimate, SearchStatus};
    for partial in [false, true] {
        let f = Fixture::new();
        let entry = f.managed("tracks.json", TRACKS, DataKind::Adsb);
        let primary = f.primary(&entry);
        let mut studio = f.studio();
        f.layer(&mut studio, &primary);
        let original = studio.document.clone().unwrap();
        studio.regional.report_document = Some(original.clone());
        studio.regional.report = Some(RegionalReport {
            schema_version: 1,
            document_sha256: geosonnar::regional_search::document_sha256(&original).unwrap(),
            inputs_sha256: "a".repeat(64),
            observers_sha256: "b".repeat(64),
            plan: RegionalPlan::default(),
            status: SearchStatus::Cancelled,
            estimate: SearchEstimate {
                observer_count: 0,
                clock_count: 0,
                event_count: 0,
                source_alternatives: 0,
                evaluations: 0,
                residual_evaluations: 0,
                trajectory_point_visits: 0,
            },
            progress: Default::default(),
            hypotheses: vec![],
            location_summaries: vec![],
            unavailable: vec![],
            unavailable_count: 0,
            rankable_cells: 0,
            unavailable_cells: 0,
            supported_hypotheses: Some(0),
            retained_truncated: false,
            location_verified: false,
            limitations: vec!["Fabricated retained result".into()],
        });
        let report = serde_json::to_value(&studio.regional.report).unwrap();
        studio.forget_dataset(&entry.files[0].sha256);
        let other = f.source(
            "current-tracks.json",
            b"{\"tracks\":[],\"config\":{\"time_origin_utc_s\":1788825601,\"max_gap_s\":30}}\n",
        );
        f.layer(&mut studio, &other);
        f.retained_result(&mut studio); // Independent current score/history use only the other source.
        let current = review_snapshot(&studio);
        let layers = serde_json::to_value(&studio.layers).unwrap();
        let revision = studio.loaded_data_revision;
        let verified = studio.run_history.verified_inputs.clone();
        assert!(studio.deletion_dependencies().contains(&primary));
        let reply = if partial {
            studio.data.delete_run_id += 1;
            let run_id = studio.data.delete_run_id;
            studio.data.deleting = Some((run_id, entry.id.clone()));
            delete_with_reconciliation(
                f.case.manifest.clone(),
                run_id,
                entry.id.clone(),
                studio.deletion_dependencies(),
                |_, _| {
                    fs::remove_file(&primary).unwrap();
                    Err("Synthetic interrupted deletion after one file".into())
                },
            )
        } else {
            f.run(&mut studio, &entry)
        };
        if partial {
            assert_eq!(reply.newly_unavailable, vec![primary.clone()]);
        }
        studio.accept_dataset_deletion(reply);
        assert_eq!(review_snapshot(&studio), current);
        assert_eq!(serde_json::to_value(&studio.layers).unwrap(), layers);
        assert_eq!(studio.loaded_data_revision, revision);
        assert_eq!(studio.run_history.verified_inputs, verified);
        assert!(studio.deleted_sources_ready(studio.document.as_ref().unwrap()));
        assert!(!studio.deleted_sources_ready(&original));
        assert_eq!(
            serde_json::to_value(&studio.regional.report).unwrap(),
            report
        );
        // Revert restores the review document while preserving regional results.
        studio.document = Some(original);
        studio.run_history.opened = None;
        studio.run_history.verified_inputs = None;
        assert!(!studio.report_inputs_ready());
        assert!(studio.regional.report.is_some());
    }
}

#[test]
fn confirming_delete_does_not_unload_inputs_before_the_owned_terminal_reply() {
    let f = Fixture::new();
    let entry = f.managed("tracks.json", TRACKS, DataKind::Adsb);
    let primary = f.primary(&entry);
    let mut studio = f.studio();
    f.layer(&mut studio, &primary);
    f.retained_result(&mut studio);
    let before = review_snapshot(&studio);
    let data = data_snapshot(&studio);
    studio.delete_managed_dataset(&egui::Context::default(), entry.id);
    assert_eq!(review_snapshot(&studio), before);
    assert_eq!(data_snapshot(&studio), data);
    let reply = studio
        .receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    let Ok(ResultValue::DatasetDeleted(reply)) = reply.result else {
        panic!("Expected typed deletion completion");
    };
    assert!(reply.outcome.is_ok());
    studio.accept_dataset_deletion(*reply);
    assert!(studio.layers.is_empty());
    assert_eq!(review_snapshot(&studio), before);
}

#[test]
fn registered_but_unused_source_keeps_independent_numerical_score_as_unverified_history() {
    let f = Fixture::new();
    let entry = f.managed(
        "unused-terrain.bin",
        b"Synthetic unused surface",
        DataKind::Terrain,
    );
    let primary = f.primary(&entry);
    let mut studio = f.studio();
    studio
        .document
        .as_mut()
        .unwrap()
        .register_dataset(
            SourceBinding {
                path: primary,
                sha256: entry.files[0].sha256.clone(),
            },
            None,
            false,
        )
        .unwrap();
    let tracks = f.source("active-tracks.json", TRACKS);
    f.layer(&mut studio, &tracks);
    f.retained_result(&mut studio);
    assert!(studio.document.as_ref().unwrap().receiver_terrain.is_none());
    let before = review_snapshot(&studio);
    let layers = serde_json::to_value(&studio.layers).unwrap();
    let reply = f.run(&mut studio, &entry);
    studio.accept_dataset_deletion(reply);
    assert_eq!(review_snapshot(&studio), before);
    assert_eq!(serde_json::to_value(&studio.layers).unwrap(), layers);
    assert!(studio.score.is_some());
    assert!(!studio.deleted_sources_ready(studio.score_document.as_ref().unwrap()));
    assert!(studio.run_history.verified_inputs.is_none());
}

#[test]
fn deletion_path_identity_accepts_portable_spelling_without_matching_sibling_or_empty_paths() {
    let f = Fixture::new();
    let entry = f.managed("tracks.json", TRACKS, DataKind::Adsb);
    let primary = f.primary(&entry);
    let directory = primary.parent().unwrap();
    assert!(dependency_in_directory(
        &primary,
        directory,
        &f.case.manifest
    ));
    assert!(dependency_in_directory(
        &PathBuf::from(display_path(&primary)),
        directory,
        &f.case.manifest
    ));
    let sibling = directory
        .with_file_name(format!("{}-other", entry.id))
        .join(&entry.primary_file);
    assert!(!dependency_in_directory(
        &sibling,
        directory,
        &f.case.manifest
    ));
    assert!(!dependency_in_directory(
        Path::new(""),
        directory,
        &f.case.manifest
    ));
    assert!(!dependency_in_directory(
        &directory.join("../escape"),
        directory,
        &f.case.manifest
    ));
}
