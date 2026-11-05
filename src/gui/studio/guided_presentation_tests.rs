//! Headless presentation controls. No files, media, workers or network are opened.
use super::*;
use geosonnar::casework::{CaseMedia, CaseRecording, SavedCase};

#[derive(Clone, Copy)]
enum Pane {
    Data,
    Map,
}

fn studio() -> Studio {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("output/never-opened-guided-fixture");
    let case = CaseOverview {
        manifest: root.join("case.json"),
        case: SavedCase {
            schema_version: 1,
            case_id: "presentation-control".into(),
            title: "Fabricated presentation control".into(),
            created_unix_s: 1,
            local_only: true,
            recording: CaseRecording {
                path: "never-opened.raw".into(),
                sha256: "a".repeat(64),
                bytes: 3,
                media: CaseMedia {
                    duration_s: 60.,
                    sample_rate_hz: 4000,
                    channels: 1,
                    audio_codec: "fabricated".into(),
                },
            },
        },
        hints: Vec::new(),
        recording_path: root.join("never-opened.raw"),
        recording_verified: Some(true),
        recording_problem: None,
    };
    let mut studio = Studio::default();
    studio.bind(&case);
    studio.data.refresh_requested = false;
    studio.detailed_controls = false;
    studio
}

fn frame(
    studio: &mut Studio,
    ctx: &egui::Context,
    pane: Pane,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400., 1500.),
            )),
            events,
            ..Default::default()
        },
        |ui| match pane {
            Pane::Data => studio.library_ui(ui),
            Pane::Map => studio.map_ui(ui),
        },
    )
}

fn labels(output: &egui::FullOutput) -> Vec<(String, egui::Pos2)> {
    fn visit(shape: &egui::epaint::Shape, clip: egui::Rect, found: &mut Vec<(String, egui::Pos2)>) {
        match shape {
            egui::epaint::Shape::Text(text) => {
                let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
                if clip.intersects(bounds) {
                    found.push((text.galley.text().to_owned(), bounds.center()));
                }
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    visit(shape, clip, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for shape in &output.shapes {
        visit(&shape.shape, shape.clip_rect, &mut found);
    }
    found
}

fn has(output: &egui::FullOutput, text: &str) -> bool {
    labels(output).iter().any(|(label, _)| label == text)
}

fn settled(studio: &mut Studio, ctx: &egui::Context, pane: Pane) -> egui::FullOutput {
    let _ = frame(studio, ctx, pane, Vec::new());
    frame(studio, ctx, pane, Vec::new())
}

fn click(studio: &mut Studio, ctx: &egui::Context, pane: Pane, text: &str) -> egui::FullOutput {
    let output = settled(studio, ctx, pane);
    let point = labels(&output)
        .into_iter()
        .find(|(label, _)| label == text)
        .unwrap_or_else(|| panic!("Missing visible control: {text}"))
        .1;
    for pressed in [true, false] {
        let _ = frame(
            studio,
            ctx,
            pane,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    let count = (ctx.global_style().animation_time / egui::RawInput::default().predicted_dt).ceil()
        as usize
        + 2;
    assert!(count <= 62);
    for _ in 0..count {
        let _ = frame(studio, ctx, pane, Vec::new());
    }
    frame(studio, ctx, pane, Vec::new())
}

#[test]
fn guided_data_views_and_all_controls_keep_the_same_drafts_and_evidence() {
    let mut studio = studio();
    let ctx = egui::Context::default();
    studio.data.date = "2025-07-19".into();
    studio.data.notes = "Preserve custom source notes".into();
    studio.document.as_mut().unwrap().sound_speed_mps = 339.;
    let before = studio.document.clone();
    let output = settled(&mut studio, &ctx, Pane::Data);
    assert!(has(&output, "Import local file…"));
    assert!(!has(&output, "Dataset"));
    let output = click(&mut studio, &ctx, Pane::Data, "Stored datasets");
    assert!(has(&output, "Dataset"));
    assert!(!has(&output, "Import local file…"));
    studio.detailed_controls = true;
    let output = settled(&mut studio, &ctx, Pane::Data);
    assert!(has(&output, "Dataset") && has(&output, "Import local file…"));
    studio.detailed_controls = false;
    let output = settled(&mut studio, &ctx, Pane::Data);
    assert!(has(&output, "Dataset") && !has(&output, "Import local file…"));
    assert_eq!(studio.data.date, "2025-07-19");
    assert_eq!(studio.data.notes, "Preserve custom source notes");
    assert!(studio.document == before && studio.undo.is_empty() && !studio.busy());
    assert!(!studio.data.refresh_requested && studio.data.downloading.is_none());
}

#[test]
fn reopening_provider_workflows_returns_to_get_data_without_resetting_requests() {
    let mut studio = studio();
    studio.data.view = LibraryView::Stored;
    studio.data.fmi.request.start_utc = "2025-07-19T16:00:00Z".into();
    studio.data.show_fmi();
    assert!(studio.data.view == LibraryView::GetData);
    assert_eq!(studio.data.fmi.request.start_utc, "2025-07-19T16:00:00Z");
    studio.data.view = LibraryView::Stored;
    studio.data.history.request.utc_start = "2024-04-08T00:00:01Z".into();
    studio.data.show_history();
    assert!(studio.data.view == LibraryView::GetData);
    assert_eq!(
        studio.data.history.request.utc_start,
        "2024-04-08T00:00:01Z"
    );
    assert!(studio.data.downloading.is_none() && !studio.busy());
}

#[test]
fn opening_map_display_controls_retains_layer_choices_and_search_mode() {
    let mut studio = studio();
    let ctx = egui::Context::default();
    studio.map.set_display_flags([false, true, false, false]);
    let search_mode = studio.regional.guided;
    let before = studio.document.clone();
    let output = settled(&mut studio, &ctx, Pane::Map);
    for label in [
        "World",
        "Fit search area",
        "Draw search rectangle",
        "Online street map",
        "Map layers and display",
    ] {
        assert!(has(&output, label), "Missing map control: {label}");
    }
    assert!(!has(&output, "Load map data…"));
    let output = click(&mut studio, &ctx, Pane::Map, "Map layers and display");
    assert!(has(&output, "Load map data…") && has(&output, "Aircraft paths"));
    studio.detailed_controls = true;
    let output = settled(&mut studio, &ctx, Pane::Map);
    assert!(has(&output, "Restore saved data layers") && has(&output, "Lightning"));
    assert_eq!(studio.map.display_flags(), [false, true, false, false]);
    assert_eq!(studio.regional.guided, search_mode);
    assert!(!studio.busy() && studio.layers.is_empty());
    assert!(studio.document == before && studio.undo.is_empty());
}

#[test]
fn data_progress_cancel_errors_and_source_assumptions_remain_visible_in_both_views() {
    let mut studio = studio();
    let ctx = egui::Context::default();
    studio.busy = Some("Receiving lightning".into());
    studio.notice = Some((true, "Failed control response".into()));
    studio.data.downloading = Some("fmi".into());
    studio.data.fmi.progress = Some(data_catalog::fmi::FmiProgress {
        chunks_total: 3,
        chunks_complete: 1,
        transferred_bytes: 1024,
        message: "Receiving fixture".into(),
    });
    studio.lightning = Some(evidence_score::LightningCatalog {
        schema_version: 1,
        input_path: PathBuf::from("never-opened.json"),
        input_bytes: 0,
        input_sha256: "b".repeat(64),
        strikes: Vec::new(),
        provenance: evidence_score::LightningProvenance {
            provider: "FMI control".into(),
            source: "fabricated presentation fixture".into(),
            license: Some("CC BY 4.0".into()),
            notes: None,
            altitude_datum: evidence_score::LightningAltitudeDatum::Unknown,
            source_metadata: Some(geosonnar::lightning_source::FmiSourceMetadata::default()),
        },
    });
    let before = studio.document.clone();
    for view in [LibraryView::GetData, LibraryView::Stored] {
        studio.data.view = view;
        let output = settled(&mut studio, &ctx, Pane::Data);
        assert!(has(&output, "Cancel data transfer"));
        assert!(has(&output, "Failed control response"));
        assert!(
            labels(&output)
                .iter()
                .any(|(text, _)| text.starts_with("Lightning data: 1 / 3 chunks"))
        );
        assert!(has(
            &output,
            "FMI source interpretation · required for thunder scoring"
        ));
    }
    let _ = click(&mut studio, &ctx, Pane::Data, "Cancel data transfer");
    assert!(studio.download_cancel.load(Ordering::Relaxed));
    assert!(studio.busy() && studio.document == before);
    assert!(
        studio
            .document
            .as_ref()
            .unwrap()
            .mix
            .lightning_source_policy
            .is_none()
    );
}

fn assert_gib_only(output: &egui::FullOutput) -> String {
    let text = labels(output)
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("GiB"), "Missing GiB presentation: {text}");
    for old in [" MiB", " MB", " GB", " bytes", " KiB", " KB"] {
        assert!(!text.contains(old), "Unexpected size unit {old}: {text}");
    }
    text
}

#[test]
fn data_request_panels_use_gib_without_changing_exact_default_allowances() {
    let mut studio = studio();
    assert_eq!(studio.data.max_bytes, 6_000_000_000);
    assert_eq!(studio.data.history.request.max_download_bytes, 100_000_000);
    assert_eq!(studio.data.fmi.request.max_total_bytes, 64 * 1_048_576);
    let before = studio.document.clone();
    // Odd-byte values make an accidental display-roundtrip visible.
    studio.data.max_bytes += 1;
    studio.data.history.request.max_download_bytes += 1;
    studio.data.fmi.request.max_total_bytes -= 1;
    for source in [
        SourceAction::Local,
        SourceAction::Adsb,
        SourceAction::Lightning,
    ] {
        let ctx = egui::Context::default();
        studio.data.source_action = source;
        let output = settled(&mut studio, &ctx, Pane::Data);
        assert_gib_only(&output);
        assert_eq!(studio.data.max_bytes, 6_000_000_001);
        assert_eq!(studio.import_request().max_bytes, 6_000_000_001);
        assert_eq!(studio.data.history.request.max_download_bytes, 100_000_001);
        assert_eq!(studio.data.fmi.request.max_total_bytes, 64 * 1_048_576 - 1);
    }
    assert_eq!(studio.document, before);
    assert!(studio.undo.is_empty() && !studio.busy() && !studio.data.refresh_requested);
}

#[test]
fn transfer_progress_uses_gib_for_small_payloads_and_distinguishes_one_byte_limit() {
    let mut studio = studio();
    studio.busy = Some("Synthetic transfer".into());
    studio.data.show_history();
    studio.data.downloading = Some("history".into());
    studio.data.history.progress = Some(geosonnar::adsblol_history::HistoryProgress {
        stage: "Receiving fixture".into(),
        completed_files: 1,
        selected_files: 3,
        http_payload_bytes: 1_073_741_824,
        max_download_bytes: 1_073_741_825,
    });
    let text = assert_gib_only(&settled(&mut studio, &egui::Context::default(), Pane::Data));
    assert!(text.contains("1.000000000 GiB / 1.000000001 GiB"));
    studio.data.show_fmi();
    studio.data.downloading = Some("fmi".into());
    studio.data.fmi.progress = Some(data_catalog::fmi::FmiProgress {
        chunks_total: 3,
        chunks_complete: 1,
        transferred_bytes: 1,
        message: "Receiving fixture".into(),
    });
    let text = assert_gib_only(&settled(&mut studio, &egui::Context::default(), Pane::Data));
    assert!(text.contains("0.000000001 GiB received"));
    assert!(!studio.download_cancel.load(Ordering::Relaxed));
}

#[test]
fn source_record_gib_display_preserves_original_json_and_non_size_counts() {
    let original = serde_json::json!({
        "bytes": 1,
        "maximum_bytes": 1_073_741_824u64,
        "points": 1048576,
        "nested": [{"source_bytes": 256, "chunks": 3, "source": "provider"}],
        "byte_order": "little-endian",
        "bytes_text": "Retained provider note"
    });
    let before = serde_json::to_vec(&original).unwrap();
    let display = source_record_gib(&original);
    assert_eq!(display["bytes"], "0.000000001 GiB");
    assert_eq!(display["maximum_bytes"], "1.000 GiB");
    assert_eq!(display["nested"][0]["source_bytes"], size(256));
    for key in ["points", "byte_order", "bytes_text"] {
        assert_eq!(display[key], original[key]);
    }
    assert_eq!(display["nested"][0]["chunks"], 3);
    assert_eq!(display["nested"][0]["source"], "provider");
    assert_eq!(serde_json::to_vec(&original).unwrap(), before);
}
