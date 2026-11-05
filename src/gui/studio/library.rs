use super::*;
use geosonnar::data_catalog::requests::{
    self, LightningFormat, LightningQuery, TerrainPlan, TerrainResolution,
};
use geosonnar::data_catalog::{DataKind, ImportRequest};
#[cfg(test)]
#[path = "library_deletion_tests.rs"]
mod deletion_tests;
#[cfg(test)]
#[path = "guided_presentation_tests.rs"]
mod guided_presentation_tests;
#[cfg(test)]
#[path = "library_scale_tests.rs"]
mod scale_tests;

pub(super) struct DeletionReply {
    manifest: PathBuf,
    run_id: u64,
    catalog_id: String,
    directory: Option<PathBuf>,
    newly_unavailable: Vec<PathBuf>,
    incomplete: bool,
    outcome: Result<data_catalog::DeleteReceipt, String>,
}

// Comparison only: preserve the original PathBuf for filesystem operations.
// A hash alone is insufficient: another owned/original copy can have identical bytes.
fn deletion_path_key(path: &std::path::Path, manifest: &std::path::Path) -> Option<PathBuf> {
    use std::path::Component;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        manifest.parent()?.join(path)
    };
    if absolute
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return None;
    }
    #[cfg(windows)]
    {
        Some(PathBuf::from(display_path(&absolute).to_lowercase()))
    }
    #[cfg(not(windows))]
    {
        Some(absolute)
    }
}

fn dependency_in_directory(
    path: &std::path::Path,
    directory: &std::path::Path,
    manifest: &std::path::Path,
) -> bool {
    match (
        deletion_path_key(path, manifest),
        deletion_path_key(directory, manifest),
    ) {
        (Some(path), Some(directory)) => path != directory && path.starts_with(directory),
        _ => false,
    }
}

fn ordinary_dependency(path: &std::path::Path) -> bool {
    path.ancestors().all(|part| {
        std::fs::symlink_metadata(part).is_ok_and(|metadata| {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return false;
                }
            }
            !metadata.file_type().is_symlink()
                && if part == path {
                    metadata.is_file()
                } else {
                    metadata.is_dir()
                }
        })
    })
}

fn deletion_directory(manifest: &std::path::Path, id: &str) -> Result<PathBuf, String> {
    if id.is_empty()
        || id.len() > 100
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid managed dataset identity".into());
    }
    let manifest = manifest.canonicalize().map_err(|e| e.to_string())?;
    let root = manifest
        .parent()
        .ok_or("Case has no directory")?
        .join("managed-data");
    let directory = root.join(id);
    for path in [&root, &directory] {
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("Managed directory is a reparse point".into());
            }
        }
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("Managed dataset needs an ordinary owned directory".into());
        }
    }
    Ok(directory)
}

struct IndexedDeletionMembers {
    primary: PathBuf,
    members: Vec<PathBuf>,
}

#[cfg(test)]
fn delete_with_reconciliation(
    manifest: PathBuf,
    run_id: u64,
    catalog_id: String,
    dependencies: Vec<PathBuf>,
    delete: impl FnOnce(&std::path::Path, &str) -> Result<data_catalog::DeleteReceipt, String>,
) -> DeletionReply {
    delete_with_reconciliation_members(manifest, run_id, catalog_id, dependencies, None, delete)
}

fn delete_with_reconciliation_members(
    manifest: PathBuf,
    run_id: u64,
    catalog_id: String,
    mut dependencies: Vec<PathBuf>,
    indexed: Option<IndexedDeletionMembers>,
    delete: impl FnOnce(&std::path::Path, &str) -> Result<data_catalog::DeleteReceipt, String>,
) -> DeletionReply {
    let mut reply = DeletionReply {
        manifest,
        run_id,
        catalog_id,
        directory: None,
        newly_unavailable: vec![],
        incomplete: false,
        outcome: Err("Deletion did not start".into()),
    };
    let directory = match deletion_directory(&reply.manifest, &reply.catalog_id) {
        Ok(directory) => directory,
        Err(error) => {
            reply.outcome = Err(error);
            return reply;
        }
    };
    // A track index is a multi-file source. Losing a referenced block makes the
    // source unavailable even if its small primary manifest survives deletion.
    // These paths come from the selected catalog inventory; keep every join
    // lexical and owned, and inspect the filesystem only inside this worker.
    let ordinary_relative = |path: &std::path::Path| {
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
    };
    let indexed_primary = indexed
        .as_ref()
        .filter(|item| ordinary_relative(&item.primary))
        .map(|item| directory.join(&item.primary))
        .filter(|primary| {
            dependencies.iter().any(|path| {
                deletion_path_key(path, &reply.manifest)
                    == deletion_path_key(primary, &reply.manifest)
            })
        })
        .filter(|primary| ordinary_dependency(primary));
    if indexed_primary.is_some()
        && let Some(item) = indexed
    {
        dependencies.extend(
            item.members
                .into_iter()
                .filter(|path| ordinary_relative(path))
                .map(|path| directory.join(path)),
        );
    }
    let mut watched: Vec<_> = dependencies
        .into_iter()
        .filter(|path| dependency_in_directory(path, &directory, &reply.manifest))
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                reply.manifest.parent().unwrap().join(path)
            }
        })
        .filter(|path| ordinary_dependency(path))
        .collect();
    watched.sort();
    watched.dedup();
    // Even a native failure can follow partial removal. Keep this reconciliation
    // inside the worker; normal refusals do not touch loaded or reviewed state.
    reply.outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        delete(&reply.manifest, &reply.catalog_id)
    }))
    .unwrap_or_else(|_| Err("Dataset deletion stopped unexpectedly".into()));
    if reply.outcome.is_err() {
        reply.incomplete = ordinary_dependency(&directory.join("deletion.json"));
        reply.newly_unavailable = watched
            .into_iter()
            .filter(|path| !ordinary_dependency(path))
            .collect();
        if !reply.newly_unavailable.is_empty()
            && let Some(primary) = indexed_primary
            && !reply.newly_unavailable.contains(&primary)
        {
            reply.newly_unavailable.push(primary);
        }
    }
    reply.directory = Some(directory);
    reply
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceAction {
    Local,
    Adsb,
    Terrain,
    Lightning,
    Export,
    Atmosphere,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum LibraryView {
    GetData,
    Stored,
}
pub(super) struct LibraryState {
    source_action: SourceAction,
    view: LibraryView,
    pub history: super::history_ui::HistoryState,
    pub fmi: super::fmi_ui::FmiState,
    pub items: Vec<Arc<data_catalog::CatalogItem>>,
    pub refresh_requested: bool,
    pub plan: Option<data_catalog::AdsbPlan>,
    pub downloading: Option<String>,
    pub progress: Option<data_catalog::DownloadProgress>,
    kind: DataKind,
    label: String,
    date: String,
    url: String,
    max_gb: f64,
    notes: String,
    selected: Option<String>,
    delete_pending: Option<String>,
    delete_run_id: u64,
    deleting: Option<(u64, String)>,
    unavailable_sources: std::collections::BTreeSet<(PathBuf, String)>,
    lightning_ellipsoid: bool,
    import_from: String,
    import_to: String,
    allow_baro: bool,
    baro_offset: f64,
    baro_uncertainty: f64,
    baro_rationale: String,
    max_gap: f64,
    max_speed: f64,
    terrain_resolution: TerrainResolution,
    terrain_plan: Option<TerrainPlan>,
    lightning_provider: String,
    lightning_format: LightningFormat,
}
impl Default for LibraryState {
    fn default() -> Self {
        Self {
            source_action: SourceAction::Local,
            view: LibraryView::GetData,
            history: super::history_ui::HistoryState::default(),
            fmi: super::fmi_ui::FmiState::default(),
            items: vec![],
            refresh_requested: true,
            plan: None,
            downloading: None,
            progress: None,
            kind: DataKind::Adsb,
            label: String::new(),
            date: String::new(),
            url: String::new(),
            max_gb: 6.0,
            notes: String::new(),
            selected: None,
            delete_pending: None,
            delete_run_id: 0,
            deleting: None,
            unavailable_sources: Default::default(),
            lightning_ellipsoid: false,
            import_from: "00:00:00".into(),
            import_to: "23:59:59".into(),
            allow_baro: false,
            baro_offset: 0.0,
            baro_uncertainty: 150.0,
            baro_rationale: String::new(),
            max_gap: 30.0,
            max_speed: 330.0,
            terrain_resolution: TerrainResolution::Glo90,
            terrain_plan: None,
            lightning_provider: "Authorized lightning provider".into(),
            lightning_format: LightningFormat::Csv,
        }
    }
}
impl LibraryState {
    pub(super) fn selected_item(&self) -> Option<Arc<data_catalog::CatalogItem>> {
        self.selected
            .as_ref()
            .and_then(|id| self.items.iter().find(|item| &item.entry.id == id))
            .cloned()
    }
    #[cfg(test)]
    pub(super) fn set_import_gap_for_test(&mut self, value: f64) {
        self.max_gap = value;
    }
    #[cfg(test)]
    pub(super) fn set_import_speed_for_test(&mut self, value: f64) {
        self.max_speed = value;
    }
    #[cfg(test)]
    pub(super) fn set_baro_import_for_test(&mut self, offset_m: f64) {
        self.allow_baro = true;
        self.baro_offset = offset_m;
        self.baro_uncertainty = 100.;
        self.baro_rationale = "Explicit synthetic approximation".into();
    }
    pub(super) fn owns_deletion(&self, run_id: u64) -> bool {
        self.deleting
            .as_ref()
            .is_some_and(|(active, _)| *active == run_id)
    }
    pub fn show_fmi(&mut self) {
        self.view = LibraryView::GetData;
        self.source_action = SourceAction::Lightning;
        self.kind = DataKind::Lightning;
        self.fmi.enabled = true;
    }
    pub fn select_entry(&mut self, id: String) {
        self.selected = Some(id);
    }
    pub fn show_history(&mut self) {
        self.view = LibraryView::GetData;
        self.source_action = SourceAction::Adsb;
        self.history.enabled = true;
    }
}
fn kind_name(kind: DataKind) -> &'static str {
    match kind {
        DataKind::Adsb => "ADS-B / aircraft",
        DataKind::Lightning => "Lightning strikes",
        DataKind::Terrain => "Terrain",
        DataKind::Other => "Other evidence",
    }
}
fn size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1e9)
    } else {
        format!("{:.1} MB", bytes as f64 / 1e6)
    }
}

fn catalog_file_row_height(ui: &egui::Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Monospace)
        .max(ui.spacing().interact_size.y)
}

/// Paint only visible inventory rows. Every registered member remains reachable;
/// one-line clipping keeps the virtual scroll stride stable at narrow widths.
fn catalog_file_list(
    ui: &mut egui::Ui,
    entry: &data_catalog::CatalogEntry,
) -> egui::scroll_area::ScrollAreaOutput<std::ops::Range<usize>> {
    ui.label(format!(
        "{} registered files · scroll to inspect the full inventory",
        entry.files.len()
    ));
    let height = catalog_file_row_height(ui);
    egui::ScrollArea::vertical()
        .id_salt(("catalog_file_inventory", &entry.id))
        .max_height(220.)
        .show_rows(ui, height, entry.files.len(), |ui, rows| {
            for index in rows.clone() {
                let file = &entry.files[index];
                let label = format!(
                    "{}  {}",
                    display_path(&file.relative_path),
                    size(file.bytes)
                );
                let id = ui.make_persistent_id(("catalog_file", index));
                let width = ui.available_width().max(1.);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
                let mut row = ui.new_child(egui::UiBuilder::new().id(id).max_rect(rect));
                row.set_clip_rect(ui.clip_rect().intersect(rect));
                row.place(
                    rect,
                    egui::Label::new(egui::RichText::new(&label).monospace()).truncate(),
                )
                .on_hover_text(label);
            }
            rows
        })
}
impl Studio {
    fn deletion_documents(&self) -> impl Iterator<Item = &ReviewWorkspace> {
        self.document
            .iter()
            .chain(self.score_document.iter())
            .chain(self.regional.report_document.iter())
            .chain(self.joint.report_document.iter())
            .chain(self.wide_report_document())
            .chain(self.run_history.opened.iter().map(|run| &run.document))
            .chain(self.run_history.unsaved.iter().map(|run| &run.document))
            .chain(
                self.investigation
                    .archive
                    .iter()
                    .map(|archive| &archive.document),
            )
    }

    fn deletion_dependencies(&self) -> Vec<PathBuf> {
        let mut paths: Vec<_> = self
            .deletion_documents()
            .flat_map(|doc| &doc.datasets)
            .map(|binding| binding.path.clone())
            .collect();
        paths.extend(self.layers.iter().map(|layer| layer.source_path.clone()));
        paths.extend(
            self.lightning
                .iter()
                .map(|catalog| catalog.input_path.clone()),
        );
        paths.extend(
            self.wide_dependency_paths()
                .map(std::path::Path::to_path_buf),
        );
        paths
    }

    pub(super) fn deleted_sources_ready(&self, document: &ReviewWorkspace) -> bool {
        let Some(case) = &self.case else {
            return true;
        };
        !document.datasets.iter().any(|binding| {
            deletion_path_key(&binding.path, &case.manifest).is_some_and(|key| {
                self.data
                    .unavailable_sources
                    .contains(&(key, binding.sha256.clone()))
            })
        })
    }

    pub(super) fn restore_deleted_source(&mut self, binding: &SourceBinding) {
        if let Some(case) = &self.case
            && let Some(key) = deletion_path_key(&binding.path, &case.manifest)
        {
            self.data
                .unavailable_sources
                .remove(&(key, binding.sha256.clone()));
        }
    }

    pub(super) fn deletion_readiness_ui(&self, ui: &mut egui::Ui) {
        if self
            .document
            .as_ref()
            .is_some_and(|doc| !self.deleted_sources_ready(doc))
        {
            ui.colored_label(tone(ui, AMBER), "Deleted source references are unavailable. Retained results remain historical; original inputs are not currently verified. Restore the exact source, or explicitly remove its unavailable reference before a new search.");
        } else if self
            .deletion_documents()
            .any(|doc| !self.deleted_sources_ready(doc))
        {
            ui.colored_label(tone(ui, AMBER), "A retained historical result references deleted original data. Current review inputs were not changed.");
        }
    }

    fn delete_managed_dataset(&mut self, ctx: &egui::Context, id: String) {
        if self.busy() || self.exiting {
            return;
        }
        let Some(case) = &self.case else { return };
        let manifest = case.manifest.clone();
        let dependencies = self.deletion_dependencies();
        let indexed = self
            .data
            .items
            .iter()
            .find(|item| item.entry.id == id && item.entry.format == "indexed_tracks")
            .map(|item| IndexedDeletionMembers {
                primary: item.entry.primary_file.clone(),
                members: item
                    .entry
                    .files
                    .iter()
                    .map(|file| file.relative_path.clone())
                    .collect(),
            });
        self.data.delete_run_id = self.data.delete_run_id.wrapping_add(1);
        let run_id = self.data.delete_run_id;
        self.data.deleting = Some((run_id, id.clone()));
        self.data.delete_pending = None;
        self.busy = Some("Deleting the selected managed copy…".into());
        self.notice = None;
        self.spawn_lane(ctx, WorkerLane::DataDeleteTask(run_id), move || {
            Ok(ResultValue::DatasetDeleted(Box::new(
                delete_with_reconciliation_members(
                    manifest,
                    run_id,
                    id,
                    dependencies,
                    indexed,
                    data_catalog::delete_owned,
                ),
            )))
        });
    }

    pub(super) fn accept_dataset_deletion(&mut self, reply: DeletionReply) {
        if !self.owns_dataset_deletion_reply(&reply) {
            return;
        }
        self.data.deleting = None;
        self.data.refresh_requested = true;
        let affected = |path: &std::path::Path| {
            if reply.outcome.is_ok() {
                reply.directory.as_ref().is_some_and(|directory| {
                    dependency_in_directory(path, directory, &reply.manifest)
                })
            } else {
                let key = deletion_path_key(path, &reply.manifest);
                key.is_some()
                    && reply
                        .newly_unavailable
                        .iter()
                        .any(|missing| deletion_path_key(missing, &reply.manifest) == key)
            }
        };
        let affected_bindings: Vec<_> = self
            .deletion_documents()
            .flat_map(|doc| &doc.datasets)
            .filter(|binding| affected(&binding.path))
            .cloned()
            .collect();
        let affected_hashes: Vec<_> = self
            .document
            .iter()
            .flat_map(|doc| &doc.datasets)
            .filter(|binding| affected(&binding.path))
            .map(|binding| binding.sha256.clone())
            .collect();
        let retained_affected = !affected_bindings.is_empty();
        let before_layers = self.layers.len();
        self.layers.retain(|layer| !affected(&layer.source_path));
        let removed_lightning = self
            .lightning
            .as_ref()
            .is_some_and(|catalog| affected(&catalog.input_path));
        if removed_lightning {
            self.lightning = None;
        }
        let unloaded = before_layers - self.layers.len() + usize::from(removed_lightning);
        for binding in affected_bindings {
            if let Some(key) = deletion_path_key(&binding.path, &reply.manifest) {
                self.data.unavailable_sources.insert((key, binding.sha256));
            }
        }
        if self.run_history.opened.as_ref().is_some_and(|run| {
            run.document
                .datasets
                .iter()
                .any(|binding| affected(&binding.path))
        }) {
            self.run_history.verified_inputs = None;
        }
        self.invalidate_wide_source(affected);
        self.invalidate_deleted_archive_sources(affected);
        if unloaded > 0 || !affected_hashes.is_empty() {
            self.report_data_changed();
            self.inspection
                .clear("A source used by this inspection was deleted or became unavailable.");
            self.regional.estimate = None;
            self.regional.comparison_estimate = None;
            // Keep the chosen terrain model and source bindings. Missing terrain
            // must remain unavailable, never silently become fixed receiver height.
            let height_affected = self.heights.preview.as_ref().is_some_and(|preview| {
                preview
                    .sources
                    .iter()
                    .any(|source| affected_hashes.contains(&source.sha256))
            }) || self
                .document
                .as_ref()
                .and_then(|doc| doc.receiver_terrain.as_ref())
                .is_some_and(|terrain| {
                    terrain
                        .surface_sha256s
                        .iter()
                        .any(|sha| affected_hashes.contains(sha))
                        || terrain
                            .geoid
                            .as_ref()
                            .is_some_and(|geoid| affected_hashes.contains(&geoid.source_sha256))
                });
            if height_affected {
                self.heights.preview = None;
                self.heights.preview_document = None;
            }
        }
        let suffix = if unloaded > 0 || !affected_hashes.is_empty() {
            " Only affected loaded inputs were invalidated. Review choices and saved results remain; their original source references may now be unavailable."
        } else if retained_affected {
            " Current review inputs were unchanged. Retained historical results now reference unavailable original sources."
        } else {
            " Loaded evidence, review choices and saved results were unchanged."
        };
        self.notice = Some(match reply.outcome {
            Ok(receipt) => (
                false,
                format!(
                    "Deleted {} files ({}) from the selected managed copy.{suffix}",
                    receipt.deleted_files,
                    size(receipt.deleted_bytes)
                ),
            ),
            Err(error) if !reply.newly_unavailable.is_empty() => (
                true,
                format!(
                    "Dataset deletion is incomplete: {error}. {} reviewed source file(s) became unavailable; only their dependent loaded inputs were invalidated. Resume deletion from Stored datasets. Review choices and saved results remain.",
                    reply.newly_unavailable.len()
                ),
            ),
            Err(error) if reply.incomplete => (
                true,
                format!(
                    "Dataset deletion is incomplete: {error}. No loaded or registered source became unavailable during this attempt; current evidence and results were preserved. Resume deletion from Stored datasets."
                ),
            ),
            Err(error) => (
                true,
                format!(
                    "Dataset deletion failed: {error}. Loaded evidence, selections, scores and saved results were preserved."
                ),
            ),
        });
    }

    pub(super) fn owns_dataset_deletion_reply(&self, reply: &DeletionReply) -> bool {
        self.data.owns_deletion(reply.run_id)
            && self
                .data
                .deleting
                .as_ref()
                .is_some_and(|(_, id)| id == &reply.catalog_id)
            && self.case.as_ref().is_some_and(|case| {
                deletion_path_key(&case.manifest, &case.manifest)
                    == deletion_path_key(&reply.manifest, &reply.manifest)
            })
    }

    pub(super) fn poll_library(&mut self, ctx: &egui::Context) {
        let Some(case) = &self.case else { return };
        let manifest = case.manifest.clone();
        if self.data.refresh_requested && !self.busy() {
            self.data.refresh_requested = false;
            // This refresh follows an operation; retain its outcome or failure notice.
            self.busy = Some("Reading the case data library…".into());
            self.spawn_lane(ctx, WorkerLane::Task, move || {
                data_catalog::list(&manifest).map(ResultValue::Catalog)
            });
        } else if let Some(id) = self.data.downloading.clone()
            && id != "https"
            && id != "history"
            && id != "fmi"
            && !self.progress_busy
            && Instant::now() >= self.next_progress
        {
            self.progress_busy = true;
            self.next_progress = Instant::now() + Duration::from_secs(2);
            self.spawn_lane(ctx, WorkerLane::Progress, move || {
                data_catalog::adsblol_progress(&manifest, &id).map(ResultValue::Progress)
            });
        }
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
    fn import_request(&self) -> ImportRequest {
        ImportRequest {
            label: if self.data.label.trim().is_empty() {
                kind_name(self.data.kind).into()
            } else {
                self.data.label.trim().into()
            },
            kind: self.data.kind,
            date_utc: (!self.data.date.trim().is_empty()).then(|| self.data.date.trim().to_owned()),
            bounds: self.document.as_ref().map(|d| d.map.bounds),
            notes: self.data.notes.clone(),
            max_bytes: (self.data.max_gb * 1e9) as u64,
        }
    }
    pub fn library_ui(&mut self, ui: &mut egui::Ui) {
        self.toolbar(ui);
        let before = self.document.clone();
        let Some(case) = &self.case else { return };
        let manifest = case.manifest.clone();
        egui::ScrollArea::vertical().id_salt("library_scroll").show(ui,|ui|{
            ui.heading("Data library & download manager");
            muted(ui,"Add a dataset, then select it below to load it for the search.");
            if !self.detailed_controls {
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut self.data.view, LibraryView::GetData, "Get data");
                    ui.selectable_value(&mut self.data.view, LibraryView::Stored, "Stored datasets");
                });
                muted(ui, match self.data.view {
                    LibraryView::GetData => "Choose a source below. Your downloaded and imported copies are in Stored datasets.",
                    LibraryView::Stored => "Select a managed copy to load, resume, import or delete it. Get data opens the request forms.",
                });
            }
            self.library_transfer_ui(ui);
            if self.detailed_controls || self.data.view == LibraryView::GetData {
            ui.horizontal_wrapped(|ui|{
                ui.selectable_value(&mut self.data.source_action,SourceAction::Local,"Import a file");
                ui.selectable_value(&mut self.data.source_action,SourceAction::Adsb,"Request ADS-B");
                if ui.selectable_value(&mut self.data.source_action,SourceAction::Terrain,"Request terrain").clicked(){self.data.kind=DataKind::Terrain;}
                if ui.selectable_value(&mut self.data.source_action,SourceAction::Lightning,"Request lightning").clicked(){self.data.kind=DataKind::Lightning;}
                if ui.selectable_value(&mut self.data.source_action,SourceAction::Export,"Download an export").clicked()&&self.data.kind==DataKind::Adsb{self.data.kind=DataKind::Lightning;}
                ui.selectable_value(&mut self.data.source_action,SourceAction::Atmosphere,"Atmospheric profile");
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui|{
                if matches!(self.data.source_action,SourceAction::Local|SourceAction::Export){
                    egui::ComboBox::from_id_salt("data_kind").selected_text(kind_name(self.data.kind)).show_ui(ui,|ui|{for k in[DataKind::Adsb,DataKind::Lightning,DataKind::Terrain,DataKind::Other]{ui.selectable_value(&mut self.data.kind,k,kind_name(k));}});
                    ui.label("Label");accessible_text(ui,"data_label",&mut self.data.label,"Dataset name",false,220.0);
                }
                if !(matches!(self.data.source_action,SourceAction::Terrain|SourceAction::Atmosphere) || self.data.source_action==SourceAction::Adsb && self.data.history.enabled || self.data.source_action==SourceAction::Lightning && self.data.fmi.enabled){
                    ui.label("UTC date");accessible_text(ui,"data_date",&mut self.data.date,"YYYY-MM-DD",false,110.0);
                    if ui.button("Use recording date").clicked()&&let Some(doc)=&self.document&&workspace::parse_utc(&doc.recording_start_utc).is_ok(){self.data.date=doc.recording_start_utc.trim()[..10].to_owned();}
                }
                if self.data.source_action!=SourceAction::Atmosphere && !(self.data.source_action==SourceAction::Adsb && self.data.history.enabled || self.data.source_action==SourceAction::Lightning && self.data.fmi.enabled) { ui.add(egui::DragValue::new(&mut self.data.max_gb).range(0.001..=100.0).speed(0.1).suffix(" GB limit")); }
            });
            ui.horizontal_wrapped(|ui|{
                if self.data.source_action==SourceAction::Local&&ui.add_enabled(!self.busy(),egui::Button::new("Import local file…")).clicked()&&let Some(path)=rfd::FileDialog::new().pick_file(){
                    let request=self.import_request();let manifest=manifest.clone();self.download_cancel=Arc::new(AtomicBool::new(false));let cancel=self.download_cancel.clone();
                    self.spawn(ui.ctx(),"Copying and checking the selected dataset…",move||data_catalog::import_file_with_cancel(&manifest,&path,&request,&cancel).map(|e|ResultValue::DataReady(format!("Stored {} as a managed copy; original preserved.",e.label))));
                }
                if matches!(self.data.source_action,SourceAction::Local|SourceAction::Export) || self.data.source_action==SourceAction::Lightning && !self.data.fmi.enabled {ui.label("Source / license notes");accessible_text(ui,"dataset_notes",&mut self.data.notes,"Provider, access terms, limitations",false,350.0);}
            });
            ui.separator();
            if self.data.source_action==SourceAction::Terrain || self.data.source_action==SourceAction::Lightning && !self.data.fmi.enabled {
                ui.strong("Requested region · shared with the Search map");
                if let Some(doc)=&mut self.document{ui.horizontal_wrapped(|ui|{for(label,value)in ["South","West","North","East"].into_iter().zip(&mut doc.map.bounds){ui.label(label);ui.add(egui::DragValue::new(value).speed(0.01).max_decimals(5));}});}
            }
            if self.data.source_action==SourceAction::Terrain{self.terrain_request_ui(ui);}
            if self.data.source_action==SourceAction::Lightning{self.lightning_request_ui(ui);}
            if self.data.source_action==SourceAction::Atmosphere{self.atmosphere_ui(ui);}
            if self.data.source_action==SourceAction::Adsb {
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut self.data.history.enabled,true,"Small time/area request");
                    ui.selectable_value(&mut self.data.history.enabled,false,"Global day archive");
                });
                if self.data.history.enabled { self.history_request_ui(ui); } else { card(ui,|ui|{
                    ui.strong("ADSB.lol historical archive");
                    muted(ui,"Check a date first. The provider supplies global day archives; geographic and time filtering happens during import.");
                    if ui.add_enabled(!self.busy(),egui::Button::new("Check availability & download size")).clicked(){
                        let request=geosonnar::adsblol_acquisition::AcquisitionRequest{date_utc:self.data.date.trim().into(),instance:None,max_download_bytes:(self.data.max_gb*1e9)as u64};
                        let bounds=self.document.as_ref().map(|d|d.map.bounds);let manifest=manifest.clone();
                        self.spawn(ui.ctx(),"Requesting archive availability…",move||data_catalog::plan_adsblol(&manifest,&request,bounds).map(ResultValue::Planned));
                    }
                    if let Some(plan)=&self.data.plan{
                        ui.label(format!("{} · {} · {} parts",plan.plan.date_utc,size(plan.plan.planned_bytes),plan.plan.assets.len()));
                        let id=plan.catalog_id.clone();
                        if ui.add_enabled(!self.busy(),egui::Button::new("Download planned archive")).clicked(){self.start_archive_download(ui.ctx(),id);}
                    }
                    ui.hyperlink_to("Provider and archive details","https://www.adsb.lol/docs/open-data/");
                });}}
            if self.data.source_action==SourceAction::Export{card(ui,|ui|{
                    ui.strong("Authorized dataset export");
                    accessible_text(ui,"dataset_url",&mut self.data.url,"HTTPS URL for a CSV, JSON or terrain file",false,ui.available_width());
                    if ui.add_enabled(!self.busy()&&self.data.kind!=DataKind::Adsb,egui::Button::new("Download export into library")).clicked(){
                        let manifest=manifest.clone();let request=self.import_request();let url=self.data.url.trim().to_owned();
                        self.download_cancel=Arc::new(AtomicBool::new(false));let cancel=self.download_cancel.clone();self.data.downloading=Some("https".into());self.data.progress=None;
                        self.spawn(ui.ctx(),"Downloading the supplied export…",move||data_catalog::download_url_with_cancel(&manifest,&url,&request,&cancel).map(|e|ResultValue::DataReady(format!("Downloaded and stored {}",e.label))));
                    }
                    muted(ui,"Blitzortung historical raw data require provider access. Import an authorized export or supply its download link. A download does not establish coverage.");
                    ui.hyperlink_to("Blitzortung archive access","https://www.blitzortung.org/en/archive_data.php");
                });}
            }
            ui.separator();
            self.lightning_source_policy_ui(ui);
            if self.detailed_controls || self.data.view == LibraryView::Stored {
            ui.horizontal(|ui|{ui.strong("Stored datasets");if ui.add_enabled(!self.busy(),egui::Button::new("Refresh")).clicked(){self.data.refresh_requested=true;}});
            if self.data.items.is_empty(){muted(ui,"No datasets stored yet. Import a file or request a download above.");}
            egui::ScrollArea::horizontal().id_salt("catalog_columns").show(ui,|ui|{egui::Grid::new("catalog_table").striped(true).num_columns(5).min_col_width(85.0).show(ui,|ui|{
                ui.strong("Dataset");ui.strong("Type");ui.strong("Stored");ui.strong("State");ui.strong("Date / coverage");ui.end_row();
                for item in &self.data.items{
                    if ui.add(egui::Button::selectable(self.data.selected.as_ref()==Some(&item.entry.id),&item.entry.label).truncate()).on_hover_text(&item.entry.label).clicked(){self.data.selected=Some(item.entry.id.clone());
                        let sha=item.entry.files.iter().find(|f|f.relative_path==item.entry.primary_file).map(|f|&f.sha256);
                        self.data.lightning_ellipsoid=self.document.as_ref().and_then(|d|d.dataset_options.iter().find(|o|Some(&o.source_sha256)==sha)).is_some_and(|o|o.lightning_altitude_wgs84);}
                    ui.label(kind_name(item.entry.kind));ui.label(size(item.available_bytes));ui.label(&item.status);ui.label(item.entry.date_utc.as_deref().unwrap_or("Date not declared"));ui.end_row();
                }
            });});
            let selected=self.data.selected_item();
            if let Some(item)=selected{
                ui.separator();ui.strong(&item.entry.label);ui.label(&item.entry.notes);
                if let Some(problem)=&item.problem{ui.colored_label(tone(ui,RED),problem);}
                if item.entry.kind==DataKind::Lightning && !item.entry.format.starts_with("fmi_lightning") {ui.checkbox(&mut self.data.lightning_ellipsoid,"Legacy file: imported strike altitudes are WGS84 ellipsoid heights");muted(ui,"Embedded source metadata, when present, is preserved instead of using this legacy declaration.");}
                if item.entry.format=="indexed_tracks" {self.wide_index_memory_ui(ui);}
                ui.horizontal_wrapped(|ui|{
                    if !matches!(item.entry.format.as_str(),"adsblol_history"|"adsblol_history_plan"|"adsblol_archive"|"adsblol_plan"|"indexed_tracks"|"fmi_lightning_plan"|"fmi_lightning_download") && ui.add_enabled(!self.busy()&&item.status!="deleted",egui::Button::new("Verify & load map / scoring layer")).clicked(){self.load_catalog_layer(ui.ctx(),&item.entry);}
                    if item.entry.format=="indexed_tracks" && ui.add_enabled(!self.busy()&&item.status!="deleted",egui::Button::new("Use indexed flight data")).clicked(){self.load_indexed_source(ui.ctx(),&item.entry);}
                    if matches!(item.entry.format.as_str(),"adsblol_history"|"adsblol_history_plan") && ui.add_enabled(!self.busy()&&item.status!="deleted",egui::Button::new("Review stored history / resume / import")).clicked(){self.data.show_history();self.reopen_history(ui.ctx(),item.entry.id.clone());}
                    if matches!(item.entry.format.as_str(),"fmi_lightning_plan"|"fmi_lightning_download") && ui.add_enabled(!self.busy()&&item.status!="deleted",egui::Button::new("Review stored FMI request / resume / import")).clicked(){self.data.show_fmi();self.reopen_fmi(ui.ctx(),item.entry.id.clone());}
                    if matches!(item.entry.format.as_str(),"adsblol_plan"|"adsblol_archive")&&ui.add_enabled(!self.busy(),egui::Button::new("Download / resume archive")).clicked(){self.data.view=LibraryView::GetData;self.data.source_action=SourceAction::Adsb;self.data.history.enabled=false;self.start_archive_download(ui.ctx(),item.entry.id.clone());}
                    if ui.add_enabled(!self.busy()&&item.status!="deleted",egui::Button::new("Delete managed copy…")).clicked(){self.data.delete_pending=Some(item.entry.id.clone());}
                });
                if item.entry.format=="adsblol_archive"{self.archive_import_controls(ui,&item.entry);}
                egui::CollapsingHeader::new("Files and source record").show(ui,|ui|{let _=catalog_file_list(ui,&item.entry);ui.label(display_path_text(&serde_json::to_string_pretty(&item.entry.source).unwrap_or_default()));});
            }
            egui::CollapsingHeader::new("Lightning interchange format").show(ui,|ui|{ui.monospace("id,time_utc_s,latitude_deg,longitude_deg,altitude_m,intensity");ui.label("UTF-8 CSV; first four columns required. Time is Unix UTC seconds. Altitude and intensity are optional. JSON accepts {schema_version:1,strikes:[…]} with the same field names. Keep the provider name and altitude assumptions with the import.");});
            }
        });
        if let Some(id) = self.data.delete_pending.clone() {
            egui::Modal::new(egui::Id::new("delete_dataset")).show(ui.ctx(),|ui|{
            ui.heading("Delete this managed dataset?");
            if let Some(item)=self.data.items.iter().find(|i|i.entry.id==id){ui.strong(format!("{} · {}",item.entry.label,size(item.available_bytes)));}
            ui.label("Only this case's managed copy will be removed. Original imports remain. Saved reviews or jobs that reference this copy may need it restored before they can run.");
            ui.horizontal(|ui|{
                if ui.button("Delete managed copy").clicked(){self.delete_managed_dataset(ui.ctx(),id.clone());}
                if ui.button("Keep dataset").clicked(){self.data.delete_pending=None;}
            });
        });
        }
        if self.document != before
            && let Some(before) = before
        {
            self.checkpoint(before);
        }
    }
    fn library_transfer_ui(&mut self, ui: &mut egui::Ui) {
        // Keep progress and cancellation above either subview. Switching views
        // changes only presentation, never the request or the worker lifecycle.
        if self.busy() {
            ui.horizontal_wrapped(|ui| {
            if let Some(progress) = &self.data.progress {
                let fraction = progress.downloaded_bytes as f32 / progress.planned_bytes.max(1) as f32;
                ui.add(egui::ProgressBar::new(fraction.clamp(0.0,1.0))
                    .text(format!("{} / {}",size(progress.downloaded_bytes),size(progress.planned_bytes))));
            } else { ui.spinner(); }
            if ui.button("Cancel data transfer").clicked() {
                self.download_cancel.store(true,Ordering::Relaxed);
                self.notice=Some((false,"Cancellation requested. Historical requests stop the active transfer and preserve an incomplete receipt; day archives stop after the current chunk/retry and retain reusable parts.".into()));
            }
        });
        }
        if self.data.downloading.as_deref() == Some("history")
            && let Some(progress) = &self.data.history.progress
        {
            ui.label(format!(
                "Aircraft data: {} / {} files · {:.1} MB received",
                progress.completed_files,
                progress.selected_files,
                progress.http_payload_bytes as f64 / 1e6
            ));
        }
        if self.data.downloading.as_deref() == Some("fmi")
            && let Some(progress) = &self.data.fmi.progress
        {
            ui.label(format!(
                "Lightning data: {} / {} chunks · {:.1} MiB received",
                progress.chunks_complete,
                progress.chunks_total,
                progress.transferred_bytes as f64 / 1_048_576.0
            ));
        }
        if !self.detailed_controls && self.data.view == LibraryView::Stored {
            if let Some(report) = &self.data.fmi.report
                && let Some(error) = report.errors.first()
            {
                ui.colored_label(tone(ui, AMBER), format!("Lightning request: {error}"));
                if ui.button("Review lightning request issues").clicked() {
                    self.data.show_fmi();
                    self.data.fmi.step = 2;
                }
            }
            if self.data.history.report.as_ref().is_some_and(|report| {
                report.status != geosonnar::adsblol_history::HistoryStatus::Complete
            }) {
                ui.colored_label(tone(ui, AMBER), "Aircraft request is incomplete. Review failures, omissions and retained files.");
                if ui.button("Review aircraft request issues").clicked() {
                    self.data.show_history();
                    self.data.history.show_report();
                }
            }
        }
    }
    fn terrain_request_ui(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.strong("Copernicus terrain · public download");
            muted(
                ui,
                "Static surface elevation, including vegetation and buildings. No date is needed. Heights use EGM2008; downloading does not apply receiver heights to scoring.",
            );
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(
                    &mut self.data.terrain_resolution,
                    TerrainResolution::Glo90,
                    TerrainResolution::Glo90.label(),
                );
                ui.selectable_value(
                    &mut self.data.terrain_resolution,
                    TerrainResolution::Glo30,
                    TerrainResolution::Glo30.label(),
                );
            });
            let current = self
                .document
                .as_ref()
                .ok_or_else(|| "Open a case first".to_owned())
                .and_then(|d| {
                    requests::plan_terrain(
                        d.map.bounds,
                        self.data.terrain_resolution,
                        (self.data.max_gb * 1e9) as u64,
                    )
                });
            if self
                .data
                .terrain_plan
                .as_ref()
                .is_some_and(|plan| current.as_ref().ok() != Some(plan))
            {
                self.data.terrain_plan = None;
            }
            if let Err(error) = &current {
                ui.colored_label(tone(ui, AMBER), error);
            }
            if ui
                .add_enabled(
                    !self.busy() && current.is_ok(),
                    egui::Button::new("Plan terrain tiles for this region"),
                )
                .clicked()
            {
                self.data.terrain_plan = current.ok();
            }
            if let Some(plan) = self.data.terrain_plan.clone() {
                ui.label(format!(
                    "{} one-degree tiles · {} total download limit",
                    plan.tiles.len(),
                    size(plan.max_bytes)
                ));
                muted(
                    ui,
                    "Tile sizes vary. Up to 64 tiles per request; ocean or unavailable tiles remain explicit gaps. Previously verified matching tiles are reused.",
                );
                egui::CollapsingHeader::new("Planned tiles").show(ui, |ui| {
                    for tile in &plan.tiles {
                        ui.label(&tile.name);
                    }
                });
                if ui
                    .add_enabled(!self.busy(), egui::Button::new("Download terrain tiles"))
                    .clicked()
                {
                    let manifest = self.case.as_ref().unwrap().manifest.clone();
                    self.download_cancel = Arc::new(AtomicBool::new(false));
                    let cancel = self.download_cancel.clone();
                    self.data.downloading = Some("https".into());
                    self.data.progress = None;
                    self.spawn(ui.ctx(), "Downloading and checking terrain tiles…", move || {
                        requests::download_terrain_with_cancel(&manifest, &plan, &cancel)
                            .map(|entries| ResultValue::DataReady(format!("{} terrain tiles are ready. Select each tile below to load its coverage.", entries.len())))
                    });
                }
            }
            ui.hyperlink_to(
                "Copernicus provider, coverage and license",
                requests::TERRAIN_PROVIDER_URL,
            );
        });
    }
    fn lightning_query(&self) -> LightningQuery {
        LightningQuery {
            provider: self.data.lightning_provider.trim().into(),
            start_utc: format!("{} {}", self.data.date.trim(), self.data.import_from.trim()),
            end_utc: format!("{} {}", self.data.date.trim(), self.data.import_to.trim()),
            bounds: self
                .document
                .as_ref()
                .map(|d| d.map.bounds)
                .unwrap_or([0.; 4]),
        }
    }
    fn lightning_request_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.data.fmi.enabled,
                true,
                "FMI / NORDLIS public history",
            );
            ui.selectable_value(
                &mut self.data.fmi.enabled,
                false,
                "Authorized provider export",
            );
        });
        if self.data.fmi.enabled {
            self.fmi_request_ui(ui);
        } else {
            self.authorized_lightning_request_ui(ui);
        }
    }
    fn authorized_lightning_request_ui(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.strong("Lightning strikes · authorized export request");
            muted(
                ui,
                "Blitzortung raw historical data require provider access. Prepare a date/region request, obtain an authorized export link, then download and select matching strikes here. No anonymous worldwide history API is assumed.",
            );
            ui.horizontal_wrapped(|ui| {
                ui.label("Provider");
                accessible_text(
                    ui,
                    "lightning_provider",
                    &mut self.data.lightning_provider,
                    "Export provider",
                    false,
                    230.0,
                );
                ui.label("From UTC");
                accessible_text(
                    ui,
                    "lightning_from",
                    &mut self.data.import_from,
                    "HH:MM:SS",
                    false,
                    90.0,
                );
                ui.label("To UTC");
                accessible_text(
                    ui,
                    "lightning_to",
                    &mut self.data.import_to,
                    "HH:MM:SS",
                    false,
                    90.0,
                );
            });
            muted(
                ui,
                "Both times use the selected UTC date. Include time before the recording for thunder travel delay.",
            );
            let query = self.lightning_query();
            let valid = query.validate();
            if let Err(error) = &valid {
                ui.colored_label(tone(ui, AMBER), error);
            }
            if ui
                .add_enabled(
                    valid.is_ok(),
                    egui::Button::new("Copy date / region export request"),
                )
                .clicked()
                && let Ok(text) = query.request_text()
            {
                ui.ctx().copy_text(text);
                self.notice = Some((
                    false,
                    "Export request copied. Submit it through your provider's authorized service."
                        .into(),
                ));
            }
            ui.hyperlink_to(
                "Blitzortung archive access",
                "https://www.blitzortung.org/en/archive_data.php",
            );
            ui.separator();
            ui.label("Authorized export download link");
            accessible_text(
                ui,
                "lightning_export_url",
                &mut self.data.url,
                "https://… signed CSV or JSON download",
                false,
                ui.available_width(),
            );
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.data.lightning_format, LightningFormat::Csv, "CSV");
                ui.selectable_value(
                    &mut self.data.lightning_format,
                    LightningFormat::Json,
                    "JSON",
                );
                ui.label("64 MiB maximum export; download uses the smaller selected size limit.");
            });
            if ui
                .add_enabled(
                    !self.busy() && valid.is_ok() && !self.data.url.trim().is_empty(),
                    egui::Button::new("Download lightning & select this time / region"),
                )
                .clicked()
            {
                let manifest = self.case.as_ref().unwrap().manifest.clone();
                let url = self.data.url.trim().to_owned();
                let format = self.data.lightning_format;
                let notes = self.data.notes.clone();
                let bytes = ((self.data.max_gb * 1e9) as u64).min(requests::MAX_LIGHTNING_BYTES);
                self.download_cancel = Arc::new(AtomicBool::new(false));
                let cancel = self.download_cancel.clone();
                self.data.downloading = Some("https".into());
                self.data.progress = None;
                self.spawn(ui.ctx(), "Downloading and selecting lightning strikes…", move || {
                    requests::download_lightning_with_cancel(&manifest, &url, format, &query, &notes, bytes, &cancel)
                        .map(|entry| ResultValue::DataReady(format!("{} is ready. Source export is retained separately; coverage is not certified.", entry.label)))
                });
            }
        });
    }
    fn start_archive_download(&mut self, ctx: &egui::Context, id: String) {
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        self.download_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.download_cancel.clone();
        self.data.downloading = Some(id.clone());
        self.data.progress = None;
        self.spawn(ctx,"Downloading / resuming the archive…",move||data_catalog::fetch_adsblol_with_cancel(&manifest,&id,&cancel).map(|e|ResultValue::DataReady(format!("Archive {} downloaded and verified. Import a bounded time/region before loading trajectories.",e.label))));
    }
    fn archive_import_controls(&mut self, ui: &mut egui::Ui, entry: &data_catalog::CatalogEntry) {
        egui::CollapsingHeader::new("Extract detailed tracks for the selected region").default_open(true).show(ui,|ui|{
            ui.label(format!("Archive day: {}",entry.date_utc.as_deref().unwrap_or("unknown")));
            ui.label("Extraction region (degrees; shared with the Search map)");
            if let Some(doc)=&mut self.document{ui.horizontal_wrapped(|ui|{for(label,value)in ["South","West","North","East"].into_iter().zip(&mut doc.map.bounds){ui.label(label);ui.add(egui::DragValue::new(value).speed(0.01).max_decimals(5));}});}
            ui.horizontal_wrapped(|ui|{
                ui.label("From UTC");accessible_text(ui,"import_from",&mut self.data.import_from,"HH:MM:SS",false,90.0);
                ui.label("To UTC");accessible_text(ui,"import_to",&mut self.data.import_to,"HH:MM:SS",false,90.0);
                if ui.button("Use recording interval").clicked()&&let Some(doc)=&self.document&&let Ok(start)=workspace::parse_utc(&doc.recording_start_utc){
                    let clock=(start%86400.0).floor()as u32;self.data.import_from=format!("{:02}:{:02}:{:02}",clock/3600,(clock/60)%60,clock%60);let end=((start%86400.0)+doc.duration_s).min(86399.0).floor()as u32;
                    self.data.import_to=format!("{:02}:{:02}:{:02}",end/3600,(end/60)%60,end%60);
                }
                ui.add(egui::DragValue::new(&mut self.data.max_gap).range(1.0..=120.0).suffix(" s maximum gap"));
                ui.add(egui::DragValue::new(&mut self.data.max_speed).range(100.0..=400.0).suffix(" m/s maximum speed"));
            });
            ui.label("Geometric altitude and ADS-B / ADS-R positions are selected by default.");
            ui.checkbox(&mut self.data.allow_baro,"Also allow an explicit barometric-height approximation");
            if self.data.allow_baro{ui.horizontal_wrapped(|ui|{ui.add(egui::DragValue::new(&mut self.data.baro_offset).suffix(" m ellipsoid offset"));ui.add(egui::DragValue::new(&mut self.data.baro_uncertainty).range(1.0..=10000.0).suffix(" m uncertainty"));});accessible_text(ui,"baro_rationale",&mut self.data.baro_rationale,"Explain this altitude assumption",false,ui.available_width());}
            self.indexed_archive_ui(ui,entry);
            ui.separator(); muted(ui,"Optional bounded map / regional layer: this separate import retains the two-million-point in-memory limit.");
            if ui.add_enabled(!self.busy(),egui::Button::new("Import bounded tracks into library")).clicked(){
                let prepared=(||->Result<_,String>{let date=entry.date_utc.as_ref().ok_or("Archive date is missing")?;
                    let config=geosonnar::adsblol::TraceImportConfig{source_label:entry.label.clone(),time_origin_utc_s:geosonnar::detailed_data::utc_midnight(date)?,time_window_utc_s:[workspace::parse_utc(&format!("{} {}",date,self.data.import_from))?,workspace::parse_utc(&format!("{} {}",date,self.data.import_to))?],bounds:self.document.as_ref().map(|d|d.map.bounds),max_gap_s:self.data.max_gap,max_speed_mps:self.data.max_speed,accepted_sources:vec!["adsb_icao".into(),"adsr_icao".into()],altitude_policy:if self.data.allow_baro{geosonnar::adsblol::TraceAltitudePolicy::AllowBarometricApproximation{ellipsoid_offset_m:self.data.baro_offset,uncertainty_m:self.data.baro_uncertainty,rationale:self.data.baro_rationale.clone()}}else{geosonnar::adsblol::TraceAltitudePolicy::GeometricOnly}};Ok(config)})();
                match prepared{Err(e)=>self.notice=Some((true,e)),Ok(config)=>{
                    let manifest=self.case.as_ref().unwrap().manifest.clone();let id=entry.id.clone();self.download_cancel=Arc::new(AtomicBool::new(false));let cancel=self.download_cancel.clone();
                    self.spawn(ui.ctx(),"Importing the selected time and region from the archive…",move||data_catalog::import_adsblol_archive_with_cancel(&manifest,&id,&config,&cancel).map(|e|ResultValue::DataReady(format!("{} is ready. Select it and load the map layer.",e.label))));
                }}
            }
        });
    }
    pub(super) fn indexed_import_config(
        &self,
        entry: &data_catalog::CatalogEntry,
        start: &str,
        end: &str,
        padding_km: f64,
    ) -> Result<geosonnar::adsblol::TraceImportConfig, String> {
        let origin = geosonnar::detailed_data::utc_midnight(
            entry.date_utc.as_deref().ok_or("Archive date is missing")?,
        )?;
        let start = if start.trim().is_empty() {
            origin
        } else {
            workspace::parse_utc(start)?
        };
        let end = if end.trim().is_empty() {
            origin + 86400.
        } else {
            workspace::parse_utc(end)?
        };
        if start < origin || end > origin + 86400. || start >= end {
            return Err("Choose an ordered source interval within this archive UTC day; adjacent days need separate archives".into());
        }
        if !padding_km.is_finite() || !(0.0..=500.0).contains(&padding_km) {
            return Err("Aircraft-area padding must be 0–500 km".into());
        }
        let mut bounds = self
            .document
            .as_ref()
            .ok_or("Open a case first")?
            .map
            .bounds;
        let latitude_padding = padding_km * 1000. / 111_320.;
        let latitude = bounds[0].abs().max(bounds[2].abs()).min(89.9);
        let longitude_padding = latitude_padding / latitude.to_radians().cos().max(0.001);
        bounds = [
            (bounds[0] - latitude_padding).max(-90.),
            (bounds[1] - longitude_padding).max(-180.),
            (bounds[2] + latitude_padding).min(90.),
            (bounds[3] + longitude_padding).min(180.),
        ];
        let config = geosonnar::adsblol::TraceImportConfig {
            source_label: entry.label.clone(),
            time_origin_utc_s: origin,
            time_window_utc_s: [start, end],
            bounds: Some(bounds),
            max_gap_s: self.data.max_gap,
            max_speed_mps: self.data.max_speed,
            accepted_sources: vec!["adsb_icao".into(), "adsr_icao".into()],
            altitude_policy: if self.data.allow_baro {
                geosonnar::adsblol::TraceAltitudePolicy::AllowBarometricApproximation {
                    ellipsoid_offset_m: self.data.baro_offset,
                    uncertainty_m: self.data.baro_uncertainty,
                    rationale: self.data.baro_rationale.clone(),
                }
            } else {
                geosonnar::adsblol::TraceAltitudePolicy::GeometricOnly
            },
        };
        geosonnar::adsblol::validate_config(&config)?;
        Ok(config)
    }
    pub(super) fn load_catalog_layer(
        &mut self,
        ctx: &egui::Context,
        entry: &data_catalog::CatalogEntry,
    ) {
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        let id = entry.id.clone();
        let primary = entry.primary_file.clone();
        let kind = entry.kind;
        let provider = entry.label.clone();
        let notes = entry.notes.clone();
        let ellipsoid = self.data.lightning_ellipsoid;
        let saved_origin = entry
            .files
            .iter()
            .find(|f| f.relative_path == entry.primary_file)
            .and_then(|f| {
                self.document
                    .as_ref()?
                    .dataset_options
                    .iter()
                    .find(|o| o.source_sha256 == f.sha256)
            })
            .and_then(|o| o.time_origin_utc_s);
        let origin = if self.map.time_origin.trim().is_empty() {
            saved_origin
        } else {
            match workspace::parse_utc(&self.map.time_origin) {
                Ok(n) => Some(n),
                Err(e) => {
                    self.notice = Some((true, e));
                    return;
                }
            }
        };
        self.spawn(
            ctx,
            "Checking dataset bytes and loading the selected layer…",
            move || {
                let files = data_catalog::verified_files(&manifest, &id)?;
                let path = files
                    .into_iter()
                    .find(|p| p.ends_with(&primary))
                    .ok_or("Primary dataset file is unavailable")?;
                if kind == DataKind::Lightning {
                    evidence_score::import_lightning(
                        &path,
                        &evidence_score::LightningProvenance {
                            provider,
                            source: path.display().to_string(),
                            license: None,
                            notes: Some(notes),
                            altitude_datum: if ellipsoid {
                                evidence_score::LightningAltitudeDatum::Wgs84Ellipsoid
                            } else {
                                evidence_score::LightningAltitudeDatum::Unknown
                            },
                            source_metadata: None,
                        },
                    )
                    .map(ResultValue::Lightning)
                } else {
                    let layer = map_data::load_with_time_origin(&path, origin)?;
                    let binding = SourceBinding {
                        path: layer.source_path.clone(),
                        sha256: layer.source_sha256.clone(),
                    };
                    Ok(ResultValue::Layer(layer, binding))
                }
            },
        );
    }
}
