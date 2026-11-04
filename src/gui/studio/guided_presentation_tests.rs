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
