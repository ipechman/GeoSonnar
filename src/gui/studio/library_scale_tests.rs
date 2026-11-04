//! Large metadata only: no filesystem, providers, media or worker processes.
use super::*;

const MEMBERS: usize = 50_000;

fn large_item() -> data_catalog::CatalogItem {
    data_catalog::CatalogItem {
        entry: data_catalog::CatalogEntry {
            schema_version: 1,
            id: "large-synthetic-index".into(),
            case_manifest_sha256: "a".repeat(64),
            label: "Synthetic indexed catalog scaling control".into(),
            kind: DataKind::Adsb,
            format: "indexed_tracks".into(),
            primary_file: "native/index.json".into(),
            created_unix_s: 0,
            source: serde_json::json!({"synthetic":true,"declared_members":MEMBERS}),
            files: (0..MEMBERS)
                .map(|index| data_catalog::CatalogFile {
                    relative_path: format!(
                        "native/blocks/{index:08}-{}-source.json.gz",
                        "long-original-identifier-".repeat(5)
                    )
                    .into(),
                    bytes: index as u64 + 1,
                    sha256: format!("{index:064x}"),
                })
                .collect(),
            date_utc: Some("2026-09-08".into()),
            bounds: None,
            notes: "Synthetic complete metadata; no actual source files".into(),
        },
        status: "available".into(),
        available_bytes: 1,
        problem: None,
        integrity_verified: false,
    }
}

#[test]
fn large_catalog_handoff_and_repeated_selection_share_original_file_inventory() {
    let item = large_item();
    let original_files = item.entry.files.as_ptr();
    let original_last_path = item.entry.files.last().unwrap().relative_path.clone();
    let mut studio = Studio::default();
    studio.data.refresh_requested = false;
    studio
        .sender
        .send(Reply {
            generation: studio.generation,
            lane: WorkerLane::Task,
            result: Ok(ResultValue::Catalog(vec![item])),
        })
        .unwrap();
    studio.tick(&egui::Context::default());
    studio.data.select_entry("large-synthetic-index".into());
    let retained = studio.data.items[0].clone();
    assert_eq!(
        retained.entry.files.as_ptr(),
        original_files,
        "worker handoff copied the file vector"
    );
    for _ in 0..24 {
        let selected = studio.data.selected_item().unwrap();
        assert!(Arc::ptr_eq(&retained, &selected));
        assert_eq!(selected.entry.files.as_ptr(), original_files);
        assert_eq!(selected.entry.files.len(), MEMBERS);
        assert_eq!(
            selected.entry.files.last().unwrap().relative_path,
            original_last_path
        );
        assert_eq!(selected.entry.source["declared_members"], MEMBERS);
    }
}

#[test]
fn large_catalog_file_list_virtualizes_rows_and_reaches_last_member_when_narrow() {
    let item = large_item();
    for width in [280., 720.] {
        let ctx = egui::Context::default();
        let mut row_stride = 0.;
        let mut rendered = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 420.),
                )),
                ..Default::default()
            },
            |ui| {
                row_stride = catalog_file_row_height(ui) + ui.spacing().item_spacing.y;
                rendered = Some(catalog_file_list(ui, &item.entry));
            },
        );
        let first = rendered.take().unwrap();
        assert_eq!(first.inner.start, 0);
        assert!(
            !first.inner.is_empty() && first.inner.len() < 32,
            "rendered too many of {MEMBERS} entries"
        );
        let mut state = first.state;
        state.offset.y = (row_stride * MEMBERS as f32 - 180.).max(0.);
        state.store(&ctx, first.id);
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 420.),
                )),
                ..Default::default()
            },
            |ui| {
                rendered = Some(catalog_file_list(ui, &item.entry));
            },
        );
        let last = rendered.take().unwrap();
        assert_eq!(
            last.inner.end, MEMBERS,
            "last registered member became unreachable"
        );
        assert!(!last.inner.is_empty() && last.inner.len() < 32);
        fn rows(shape: &egui::epaint::Shape, count: &mut usize) {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text.starts_with("native") => {
                    assert_eq!(
                        text.galley.rows.len(),
                        1,
                        "file path wrapped and broke virtual row height"
                    );
                    *count += 1;
                }
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        rows(shape, count);
                    }
                }
                _ => {}
            }
        }
        let mut count = 0;
        for clipped in &output.shapes {
            rows(&clipped.shape, &mut count);
        }
        assert!(
            count > 0 && count < 32,
            "expected a bounded set of actual painted file rows"
        );
        assert_eq!(item.entry.files.len(), MEMBERS);
        assert_eq!(item.entry.files.last().unwrap().bytes, MEMBERS as u64);
    }
}
