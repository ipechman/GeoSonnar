//! Explicit receiver surface/geoid selection and coverage review.
use super::*;
use geosonnar::receiver_heights::{
    self, GeoidSource, ReceiverHeightReport, ReceiverTerrain, SurfaceDatum,
};

#[derive(Default)]
pub(super) struct ReceiverHeightState {
    pub draft: Option<ReceiverTerrain>,
    pub preview: Option<ReceiverHeightReport>,
    pub preview_document: Option<ReviewWorkspace>,
}

fn datum_name(datum: SurfaceDatum) -> &'static str {
    match datum {
        SurfaceDatum::Wgs84Ellipsoid => "WGS84 ellipsoid",
        SurfaceDatum::Egm96 => "EGM96",
        SurfaceDatum::Egm2008 => "EGM2008",
    }
}

impl Studio {
    pub(super) fn height_model_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Receiver heights from terrain").show(ui, |ui| {
            if let Some(active) = self.document.as_ref().and_then(|doc| doc.receiver_terrain.as_ref()) {
                ui.label(format!("Active: {} surface tiles · {} · {:.2} m above surface",active.surface_sha256s.len(),datum_name(active.surface_datum),active.above_surface_m));
            } else { ui.label("Active: declared fixed ellipsoid height"); }
            ui.add_enabled_ui(!self.busy(), |ui| {
                if ui.button("Edit terrain height settings").clicked() {
                    self.heights.draft = Some(self.document.as_ref().and_then(|doc| doc.receiver_terrain.clone()).unwrap_or(ReceiverTerrain {
                        surface_sha256s: vec![], surface_datum: SurfaceDatum::Egm2008, geoid: None,
                        above_surface_m: 1.5, notes: String::new(),
                    }));
                    self.heights.preview = None;
                    self.heights.preview_document = None;
                }
                if self.document.as_ref().is_some_and(|doc| doc.receiver_terrain.is_some())
                    && ui.button("Use fixed receiver height").clicked() {
                    self.document.as_mut().unwrap().receiver_terrain = None;
                    self.heights.preview = None;
                }
            });
            let busy = self.busy();
            let Some(draft) = &mut self.heights.draft else {
                muted(ui,"Import and register surface and, when required, geoid GeoTIFFs in Data. No missing terrain value is replaced by zero.");
                return;
            };
            let candidates: Vec<_> = self.document.as_ref().unwrap().datasets.iter().filter(|binding| binding.path.extension().and_then(|v|v.to_str()).is_some_and(|v| v.eq_ignore_ascii_case("tif") || v.eq_ignore_ascii_case("tiff"))).collect();
            let mut preview = false;
            let mut apply = false;
            ui.add_enabled_ui(!busy, |ui| {
                ui.label("Surface tiles — select the matching area");
                egui::ScrollArea::vertical().id_salt("receiver_surface_choices").max_height(170.0).show(ui, |ui| {
                    for source in &candidates {
                        let mut selected = draft.surface_sha256s.contains(&source.sha256);
                        let name = source.path.file_name().unwrap_or_default().to_string_lossy();
                        let hash_label: String = source.sha256.chars().take(12).collect();
                        if ui.add_enabled(selected || draft.surface_sha256s.len()<64,egui::Checkbox::new(&mut selected,format!("{name} · {hash_label}"))).changed() {
                            if selected { draft.surface_sha256s.push(source.sha256.clone()); draft.surface_sha256s.sort(); }
                            else { draft.surface_sha256s.retain(|hash| hash != &source.sha256); }
                        }
                    }
                });
                if candidates.is_empty() { muted(ui,"No registered GeoTIFF sources. Import the files in Data first."); }
                let missing: Vec<_> = draft.surface_sha256s.iter().filter(|hash| !candidates.iter().any(|source| &source.sha256 == *hash)).collect();
                if !missing.is_empty() { ui.colored_label(tone(ui, AMBER),format!("{} selected source(s) are no longer registered. Restore the data or edit the selection.",missing.len())); }
                egui::ComboBox::from_id_salt("receiver_surface_datum").selected_text(format!("Surface reference: {}",datum_name(draft.surface_datum))).show_ui(ui, |ui| {
                    for datum in [SurfaceDatum::Egm2008,SurfaceDatum::Egm96,SurfaceDatum::Wgs84Ellipsoid] {
                        if ui.selectable_value(&mut draft.surface_datum,datum,datum_name(datum)).changed() {
                            // A new reference needs a newly chosen compatible geoid.
                            draft.geoid = None;
                        }
                    }
                });
                if draft.surface_datum != SurfaceDatum::Wgs84Ellipsoid {
                    let selected = draft.geoid.as_ref().and_then(|geoid| candidates.iter().find(|source| source.sha256 == geoid.source_sha256)).map(|source| source.path.file_name().unwrap_or_default().to_string_lossy().to_string()).unwrap_or_else(|| "Choose compatible geoid".into());
                    egui::ComboBox::from_id_salt("receiver_geoid").selected_text(selected).show_ui(ui, |ui| {
                        if ui.selectable_label(draft.geoid.is_none(),"No geoid selected").clicked() { draft.geoid=None; }
                        for source in &candidates {
                            let disabled = draft.surface_sha256s.contains(&source.sha256);
                            let selected = draft.geoid.as_ref().is_some_and(|geoid| geoid.source_sha256 == source.sha256);
                            if ui.add_enabled(!disabled,egui::Button::new(source.path.file_name().unwrap_or_default().to_string_lossy()).selected(selected)).clicked() {
                                draft.geoid=Some(GeoidSource { source_sha256: source.sha256.clone(), reference:draft.surface_datum });
                            }
                        }
                    });
                    muted(ui,format!("Confirm the chosen geoid is actually {}. File values alone do not identify its vertical reference.",datum_name(draft.surface_datum)));
                } else { muted(ui,"Ellipsoid surfaces receive no geoid correction."); }
                ui.horizontal_wrapped(|ui| {
                    ui.label("Microphone above surface");
                    ui.add(egui::DragValue::new(&mut draft.above_surface_m).range(0.0..=1000.0).speed(0.1).suffix(" m"));
                });
                accessible_text(ui,"receiver_height_notes",&mut draft.notes,"Source / datum rationale",true,ui.available_width());
                muted(ui,"A surface model may include roofs or trees. Verify the stated reference and microphone height. Sampling checks every receiver and refuses gaps or conflicting overlaps.");
                preview = ui.button("Preview height coverage").clicked();
                let matches = self.heights.preview_document.as_ref() == self.document.as_ref()
                    && self.heights.preview.as_ref().is_some_and(|report| report.settings.as_ref() == Some(draft) && report.complete_observers().is_ok());
                apply = ui.add_enabled(matches,egui::Button::new("Apply reviewed height model")).clicked();
            });
            if let Some(report) = &self.heights.preview {
                let fresh = self.heights.preview_document.as_ref() == self.document.as_ref() && report.settings.as_ref()==Some(draft);
                ui.label(format!("Coverage: {} / {} receiver heights available",report.observers.iter().filter(|value|value.is_some()).count(),report.observers.len()));
                if !fresh { ui.colored_label(tone(ui, AMBER),"Height settings or search area changed. Preview again before applying."); }
                egui::ScrollArea::vertical().id_salt("receiver_coverage_failures").max_height(160.0).show_rows(ui,40.0,report.unavailable.len(),|ui,range| {
                    for index in range { let failure=&report.unavailable[index]; ui.label(format!("{:.5}, {:.5}: {}",failure.latitude_deg,failure.longitude_deg,failure.reason)); }
                });
                for limitation in &report.limitations { ui.small(limitation); }
            }
            if apply {
                self.document.as_mut().unwrap().receiver_terrain = self.heights.draft.clone();
                self.notice = Some((false,"Applied the reviewed height model. Save the review to preserve its source references and assumptions.".into()));
            }
            if preview { self.preview_receiver_heights(ui.ctx()); }
        });
    }

    fn preview_receiver_heights(&mut self, ctx: &egui::Context) {
        let (Some(previous), Some(draft)) = (self.document.clone(), self.heights.draft.clone())
        else {
            return;
        };
        let mut proposed = previous.clone();
        proposed.receiver_terrain = Some(draft);
        self.heights.preview = None;
        self.heights.preview_document = None;
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        self.spawn(ctx, "Checking terrain and geoid coverage…", move || {
            workspace::verify(&manifest, &proposed)?;
            let report = receiver_heights::grid(&proposed)?;
            workspace::verify(&manifest, &proposed)?;
            Ok(ResultValue::HeightPreview(
                Box::new(report),
                Box::new(previous),
            ))
        });
    }
}
