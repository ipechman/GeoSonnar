use super::*;
use eframe::egui::{Align2, FontId, Pos2, Rect, Sense, Stroke};
use serde::Deserialize;
use std::cmp::Ordering as SortOrdering;

const GRID_PRESETS: [(&str, f64); 5] = [
    ("10 km", 10_000.0),
    ("2 km", 2_000.0),
    ("500 m", 500.0),
    ("100 m", 100.0),
    ("25 m", 25.0),
];

fn grid_preset_label(spacing: f64) -> &'static str {
    GRID_PRESETS
        .iter()
        .find(|(_, value)| *value == spacing)
        .map_or("Custom", |(label, _)| *label)
}

/// Reimport actual bytes before restoring a historical interpretation. Legacy
/// CSV/JSON may not embed provenance; preserving the frozen fallback metadata
/// keeps input identity stable without accepting different rows or assumptions.
pub(super) fn restore_lightning_catalog(
    path: &std::path::Path,
    altitude_wgs84: bool,
    historical: Option<&evidence_score::LightningCatalog>,
) -> Result<evidence_score::LightningCatalog, String> {
    let fallback = evidence_score::LightningProvenance {
        provider: "Saved user-supplied lightning export".into(),
        source: path.display().to_string(),
        license: None,
        notes: Some("Altitude assumption restored from the saved review".into()),
        altitude_datum: if altitude_wgs84 {
            evidence_score::LightningAltitudeDatum::Wgs84Ellipsoid
        } else {
            evidence_score::LightningAltitudeDatum::Unknown
        },
        source_metadata: None,
    };
    if historical.is_some_and(|catalog| {
        catalog.input_path != path
            || altitude_wgs84
                != (catalog.provenance.altitude_datum
                    == evidence_score::LightningAltitudeDatum::Wgs84Ellipsoid)
    }) {
        return Err(
            "Historical lightning source path or altitude interpretation differs from this review"
                .into(),
        );
    }
    let catalog = evidence_score::import_lightning(
        path,
        historical.map_or(&fallback, |catalog| &catalog.provenance),
    )?;
    if let Some(expected) = historical
        && serde_json::to_vec(&catalog).map_err(|e| e.to_string())?
            != serde_json::to_vec(expected).map_err(|e| e.to_string())?
    {
        return Err("Reimported original lightning differs from the frozen joint catalog; source bytes, rows and provenance must all match".into());
    }
    Ok(catalog)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResultColumn {
    Rank,
    Score,
    Latitude,
    Longitude,
    Frequency,
    DopplerTime,
    ThunderTime,
}

const RESULT_COLUMNS: [(ResultColumn, &str, f32); 7] = [
    (ResultColumn::Rank, "Rank", 65.0),
    (ResultColumn::Score, "Combined score", 125.0),
    (ResultColumn::Latitude, "Latitude", 110.0),
    (ResultColumn::Longitude, "Longitude", 110.0),
    (ResultColumn::Frequency, "Frequency", 120.0),
    (ResultColumn::DopplerTime, "Doppler time", 120.0),
    (ResultColumn::ThunderTime, "Thunder time", 120.0),
];

impl ResultColumn {
    fn term(self) -> Option<evidence_score::EvidenceTerm> {
        use evidence_score::EvidenceTerm::*;
        match self {
            Self::Frequency => Some(DopplerFrequency),
            Self::DopplerTime => Some(DopplerTiming),
            Self::ThunderTime => Some(ThunderTiming),
            _ => None,
        }
    }
    fn value(self, candidate: &evidence_score::CandidateScore) -> Option<f64> {
        let value = match self {
            Self::Rank => candidate.rank.map(|rank| rank as f64),
            Self::Score => candidate.score,
            Self::Latitude => Some(candidate.candidate.observer.latitude_deg),
            Self::Longitude => Some(candidate.candidate.observer.longitude_deg),
            _ => candidate
                .terms
                .iter()
                .find(|term| Some(term.term) == self.term() && term.available)
                .and_then(|term| term.weighted_contribution),
        };
        value.filter(|value| value.is_finite())
    }
}

fn compare_result_values(a: Option<f64>, b: Option<f64>, descending: bool) -> SortOrdering {
    match (a.filter(|v| v.is_finite()), b.filter(|v| v.is_finite())) {
        (Some(a), Some(b)) => {
            let order = a.total_cmp(&b);
            if descending { order.reverse() } else { order }
        }
        (Some(_), None) => SortOrdering::Less,
        (None, Some(_)) => SortOrdering::Greater,
        (None, None) => SortOrdering::Equal,
    }
}

fn sorted_results(
    candidates: &[evidence_score::CandidateScore],
    column: ResultColumn,
    descending: bool,
) -> Vec<&evidence_score::CandidateScore> {
    let mut rows: Vec<_> = candidates.iter().collect();
    rows.sort_by(|a, b| {
        compare_result_values(column.value(a), column.value(b), descending)
            .then_with(|| a.candidate.id.cmp(&b.candidate.id))
    });
    rows
}

fn term_label(term: evidence_score::EvidenceTerm) -> &'static str {
    match term {
        evidence_score::EvidenceTerm::DopplerFrequency => "Doppler frequency",
        evidence_score::EvidenceTerm::DopplerTiming => "Doppler timing",
        evidence_score::EvidenceTerm::ThunderTiming => "Thunder / strike timing",
    }
}
#[derive(Deserialize)]
struct Country {
    name: String,
    label: [f64; 2],
    rings: Vec<Vec<[f64; 2]>>,
}
pub(super) struct MapState {
    center_lat: f64,
    center_lon: f64,
    span: f64,
    world: Vec<Country>,
    show_tracks: bool,
    show_strikes: bool,
    show_grid: bool,
    show_candidates: bool,
    show_terrain: bool,
    show_modalities: bool,
    show_overlap: bool,
    draw_region: bool,
    drag_start: Option<[f64; 2]>,
    selected_track: Option<String>,
    selected_strike: Option<String>,
    receiver: Option<[f64; 2]>,
    selected_candidate: Option<String>,
    result_sort: ResultColumn,
    result_descending: bool,
    pub time_origin: String,
    time_filter: bool,
    online_tiles: bool,
    tiles: super::tiles::TileLayer,
    paths: super::map_display::PathDisplayCache,
}
impl Default for MapState {
    fn default() -> Self {
        Self {
            center_lat: 25.0,
            center_lon: 0.0,
            span: 330.0,
            world: serde_json::from_str(include_str!("../world-map.json")).unwrap_or_default(),
            show_tracks: true,
            show_strikes: true,
            show_grid: true,
            show_candidates: true,
            show_terrain: true,
            show_modalities: true,
            show_overlap: true,
            draw_region: false,
            drag_start: None,
            selected_track: None,
            selected_strike: None,
            receiver: None,
            selected_candidate: None,
            result_sort: ResultColumn::Rank,
            result_descending: false,
            time_origin: String::new(),
            time_filter: false,
            online_tiles: false,
            tiles: super::tiles::TileLayer::default(),
            paths: super::map_display::PathDisplayCache::default(),
        }
    }
}
fn mercator(lat: f64) -> f64 {
    (std::f64::consts::FRAC_PI_4 + lat.clamp(-85.0, 85.0).to_radians() / 2.0)
        .tan()
        .ln()
}
#[cfg(test)]
impl MapState {
    pub(super) fn display_flags(&self) -> [bool; 4] {
        [
            self.show_tracks,
            self.show_strikes,
            self.show_grid,
            self.online_tiles,
        ]
    }

    pub(super) fn set_display_flags(&mut self, flags: [bool; 4]) {
        [
            self.show_tracks,
            self.show_strikes,
            self.show_grid,
            self.online_tiles,
        ] = flags;
    }
}

impl MapState {
    pub(super) fn cancel_for_exit(&mut self) {
        self.tiles.pause();
        self.online_tiles = false;
    }
    fn project(&self, rect: Rect, lat: f64, lon: f64) -> Pos2 {
        let dx = (lon - self.center_lon + 180.0).rem_euclid(360.0) - 180.0;
        let scale = rect.width() as f64 / self.span.to_radians();
        egui::pos2(
            rect.center().x + (dx / self.span) as f32 * rect.width(),
            rect.center().y - ((mercator(lat) - mercator(self.center_lat)) * scale) as f32,
        )
    }
    fn unproject(&self, rect: Rect, p: Pos2) -> [f64; 2] {
        let scale = rect.width() as f64 / self.span.to_radians();
        let lat = (mercator(self.center_lat) - (p.y - rect.center().y) as f64 / scale)
            .sinh()
            .atan()
            .to_degrees();
        let lon =
            self.center_lon + (p.x - rect.center().x) as f64 / rect.width() as f64 * self.span;
        [
            lat.clamp(-85.0, 85.0),
            (lon + 180.0).rem_euclid(360.0) - 180.0,
        ]
    }
    fn fit(&mut self, b: [f64; 4]) {
        self.center_lat = (b[0] + b[2]) / 2.0;
        let east = if b[3] < b[1] { b[3] + 360.0 } else { b[3] };
        self.center_lon = ((b[1] + east) / 2.0 + 180.0).rem_euclid(360.0) - 180.0;
        self.span = ((east - b[1]).max((b[2] - b[0]) * 1.8) * 1.2).clamp(0.002, 360.0);
    }
}
impl Studio {
    pub(super) fn fixed_selected_receiver(
        &self,
    ) -> Option<(String, evidence_score::ObserverLocation)> {
        let id = self.map.selected_candidate.as_ref()?;
        let candidate = self
            .score
            .as_ref()?
            .candidates
            .iter()
            .find(|candidate| &candidate.candidate.id == id)?;
        Some((id.clone(), candidate.candidate.observer.clone()))
    }
    pub(super) fn forget_dataset(&mut self, sha: &str) {
        self.report_data_changed();
        if let Some(doc) = &mut self.document {
            doc.datasets.retain(|d| d.sha256 != sha);
            doc.dataset_options.retain(|o| o.source_sha256 != sha);
        }
        self.layers.retain(|l| l.source_sha256 != sha);
        if self
            .lightning
            .as_ref()
            .is_some_and(|c| c.input_sha256 == sha)
        {
            self.lightning = None;
        }
        self.score = None;
    }
    pub(super) fn add_layer(&mut self, layer: map_data::MapData, binding: SourceBinding) {
        let restored_binding = binding.clone();
        if let Some(doc) = &mut self.document
            && let Err(error) = doc.register_dataset(binding, layer.time_origin_utc_s, false)
        {
            self.notice = Some((true, error));
            return;
        }
        self.restore_deleted_source(&restored_binding);
        let points: Vec<_> = layer
            .tracks
            .iter()
            .flat_map(|t| t.points.iter().cloned())
            .chain(layer.observers.iter().cloned())
            .collect();
        if let Some(bounds) = map_data::geographic_bounds(&points) {
            self.map.fit(bounds);
        }
        self.layers.retain(|l| {
            l.source_path != layer.source_path && l.source_sha256 != layer.source_sha256
        });
        self.layers.push(layer);
        self.report_data_changed();
        self.score = None;
    }
    pub(super) fn cached_bindings_match(&self, doc: &ReviewWorkspace) -> Result<(), String> {
        for layer in &self.layers {
            if !doc
                .datasets
                .iter()
                .any(|d| d.path == layer.source_path && d.sha256 == layer.source_sha256)
            {
                return Err("A loaded map layer is not part of this review. Restore saved layers or reload the intended dataset.".into());
            }
            let options = doc
                .dataset_options
                .iter()
                .find(|o| o.source_sha256 == layer.source_sha256)
                .ok_or("Loaded map layer has no saved assumptions")?;
            if options.time_origin_utc_s != layer.time_origin_utc_s {
                return Err("Loaded track UTC origin differs from the saved review. Restore saved data layers before scoring.".into());
            }
        }
        if let Some(catalog) = &self.lightning {
            if !doc
                .datasets
                .iter()
                .any(|d| d.path == catalog.input_path && d.sha256 == catalog.input_sha256)
            {
                return Err("Loaded lightning data do not belong to this review".into());
            }
            let options = doc
                .dataset_options
                .iter()
                .find(|o| o.source_sha256 == catalog.input_sha256)
                .ok_or("Lightning data have no saved altitude assumption")?;
            if options.lightning_altitude_wgs84
                != (catalog.provenance.altitude_datum
                    == evidence_score::LightningAltitudeDatum::Wgs84Ellipsoid)
            {
                return Err("Loaded lightning altitude meaning differs from this review. Restore saved data layers before scoring.".into());
            }
        }
        Ok(())
    }
    fn restore_layers(&mut self, ctx: &egui::Context) {
        let doc = self.document.clone().unwrap();
        let historical_catalog = self.joint_catalog_for_restore(&doc);
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        self.spawn(ctx,"Restoring saved datasets and their assumptions…",move||{
            workspace::verify(&manifest,&doc)?;let mut layers=Vec::new();let mut lightning=None;
            for binding in&doc.datasets{
                if binding.path.file_name().is_some_and(|name|name=="index.json") && geosonnar::track_store::TrackStore::open(&binding.path).is_ok() { continue; }
                let options=doc.dataset_options.iter().find(|o|o.source_sha256==binding.sha256).ok_or("Saved dataset has no assumptions record")?;
                let layer=map_data::load_with_time_origin(&binding.path,options.time_origin_utc_s)?;
                if !layer.lightning.is_empty(){
                    if lightning.is_some(){return Err("Multiple lightning catalogs require an explicit combined source file".into())}
                    let historical=historical_catalog.as_ref().filter(|catalog|catalog.input_path==binding.path&&catalog.input_sha256==binding.sha256);
                    lightning=Some(restore_lightning_catalog(&binding.path,options.lightning_altitude_wgs84,historical)?);
                }else{layers.push(layer);}
            }
            workspace::verify(&manifest,&doc)?;Ok(ResultValue::RestoredLayers(layers,lightning,doc))
        });
    }
    pub(super) fn merged_map(&self) -> Result<map_data::MapData, String> {
        geosonnar::investigation_archive::merge_layers(&self.layers)
    }
    fn load_map_file(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Map data", &["json", "csv", "tif", "tiff"])
            .pick_file()
        {
            let origin = if self.map.time_origin.trim().is_empty() {
                None
            } else {
                match workspace::parse_utc(&self.map.time_origin) {
                    Ok(n) => Some(n),
                    Err(e) => {
                        self.notice = Some((true, e));
                        return;
                    }
                }
            };
            self.spawn(ctx,"Loading map geometry and source details…",move||{let layer=map_data::load_with_time_origin(&path,origin)?;
            if !layer.lightning.is_empty(){return evidence_score::import_lightning(&path,&evidence_score::LightningProvenance{provider:"User-supplied lightning export".into(),source:path.display().to_string(),license:None,notes:Some("Height datum is unspecified; use the library to declare known WGS84 heights".into()),altitude_datum:evidence_score::LightningAltitudeDatum::Unknown,source_metadata:None}).map(ResultValue::Lightning)}
            let source=SourceBinding{path:layer.source_path.clone(),sha256:layer.source_sha256.clone()};Ok(ResultValue::Layer(layer,source))});
        }
    }
    fn score_locations(&mut self, ctx: &egui::Context, one: bool) {
        let Some(doc) = self.document.clone() else {
            return;
        };
        if let Some(reason) = super::acoustic_review::first_block(&doc, false) {
            self.notice = Some((true, reason));
            return;
        }
        if let Err(e) = self.cached_bindings_match(&doc) {
            self.notice = Some((true, e));
            return;
        }
        let map = match self.merged_map() {
            Ok(v) => v,
            Err(e) => {
                self.notice = Some((true, e));
                return;
            }
        };
        let lightning = self.lightning.clone();
        let fallback = doc.strike_fallback_height_m;
        let receiver = self.map.receiver;
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        self.spawn(
            ctx,
            "Comparing the explicitly selected evidence…",
            move || {
                workspace::verify(&manifest, &doc)?;
                let report = if one {
                    let p = receiver.ok_or("Click a receiver position on the map first")?;
                    let observers = geosonnar::receiver_heights::resolve(
                        doc.receiver_terrain.as_ref(),
                        &doc.datasets,
                        &[evidence_score::ObserverLocation {
                            latitude_deg: p[0],
                            longitude_deg: p[1],
                            ellipsoid_height_m: doc.map.receiver_height_m,
                        }],
                    )?
                    .complete_observers()?;
                    geosonnar::workspace_scoring::score_observers(
                        &doc, &map, &lightning, fallback, &observers,
                    )?
                } else {
                    geosonnar::workspace_scoring::score_grid(&doc, &map, &lightning, fallback)?
                };
                workspace::verify(&manifest, &doc)?;
                Ok(ResultValue::Scored(Box::new(report), doc))
            },
        );
    }
    pub fn map_ui(&mut self, ui: &mut egui::Ui) {
        self.toolbar(ui);
        let Some(before) = self.document.clone() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.wide.enabled, true, "Wide-area two-pass Doppler");
            if ui
                .selectable_label(!self.wide.enabled, "Regional / evidence comparison")
                .clicked()
            {
                self.wide.enabled = false;
            }
        });
        if !self.wide.enabled
            && self
                .document
                .as_ref()
                .and_then(|d| geosonnar::receiver_grid::GridLayout::new(&d.map).ok())
                .is_some_and(|g| g.count > 10_000)
        {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    tone(ui, AMBER),
                    "This area exceeds the regional engine's 10,000 receivers.",
                );
                if ui.button("Use wide-area two-pass Doppler").clicked() {
                    self.wide.enabled = true;
                }
            });
        }
        ui.add_enabled_ui(!self.wide.enabled, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.regional.guided, true, "Guided regional search");
                ui.selectable_value(
                    &mut self.regional.guided,
                    false,
                    "Fixed assumptions comparison",
                );
            });
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("World").clicked() {
                self.map.center_lat = 25.0;
                self.map.center_lon = 0.0;
                self.map.span = 330.0;
            }
            if ui.button("Fit search area").clicked() {
                self.map.fit(self.document.as_ref().unwrap().map.bounds);
            }
            if ui
                .checkbox(&mut self.map.online_tiles, "Online street map")
                .changed()
                && !self.map.online_tiles
            {
                self.map.tiles.pause();
            }
            ui.checkbox(&mut self.map.draw_region, "Draw search rectangle");
        });
        if self.detailed_controls {
            self.map_layers_ui(ui);
        } else {
            egui::CollapsingHeader::new("Map layers and display")
                .id_salt("map_layers_disclosure")
                .show(ui, |ui| self.map_layers_ui(ui));
        }
        // Keep a usable map when a previously widened inspector meets a smaller window.
        let inspector_max = (ui.available_width() * 0.48).clamp(220.0, 550.0);
        egui::Panel::right("map_evidence_panel")
            .resizable(true)
            .default_size(330.0_f32.min(inspector_max))
            .size_range(220.0..=inspector_max)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("evidence_scoring")
                    .show(ui, |ui| {
                        self.lightning_source_policy_ui(ui);
                        if self.wide.enabled {
                            self.wide_controls_ui(ui);
                        } else if self.regional.guided {
                            self.regional_controls(ui);
                        } else {
                            self.scoring_controls(ui);
                        }
                    });
            });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui,|ui|{
            let available = ui.available_height().max(0.0);
            let map_height = (available - 125.0).max(60.0).min(available);
            self.draw_map(ui,map_height);
            egui::ScrollArea::vertical().id_salt("map_context_notes").show(ui, |ui| {
            ui.horizontal_wrapped(|ui|{ui.label("Drag to pan · wheel to zoom · click paths/strikes to inspect · click empty map to place a receiver");ui.hyperlink_to("Made with Natural Earth","https://www.naturalearthdata.com/about/terms-of-use/");});
            if self.map.online_tiles{ui.horizontal_wrapped(|ui|{ui.label("Visible map tiles are requested online and cached in this case.");ui.hyperlink_to("© OpenStreetMap contributors","https://www.openstreetmap.org/copyright");});}else{ui.label("Offline country outlines. Enable Online street map for detailed, cached tiles.");}
            if let Some(report) = &self.score {
                muted(ui, format!("Comparison ready: {} of {} receivers can be ranked. Open Results to sort the full table, inspect residuals and save a snapshot.", report.ranked_candidate_ids.len(), report.candidates.len()));
            } else {
                muted(ui, "Choose the area, confirmed events and terms, then score the grid. Detailed comparisons appear in Results.");
            }
            });
        });
        if self.document.as_ref() != Some(&before) {
            self.checkpoint(before);
        }
    }
    fn map_layers_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.busy(), egui::Button::new("Load map data…"))
                .clicked()
            {
                self.load_map_file(ui.ctx());
            }
            if ui
                .add_enabled(
                    !self.busy()
                        && self
                            .document
                            .as_ref()
                            .is_some_and(|doc| !doc.datasets.is_empty()),
                    egui::Button::new("Restore saved data layers"),
                )
                .clicked()
            {
                self.restore_layers(ui.ctx());
            }
            ui.checkbox(&mut self.map.show_tracks, "Aircraft paths");
            ui.checkbox(&mut self.map.show_strikes, "Lightning");
            ui.checkbox(&mut self.map.show_grid, "Grid");
            ui.checkbox(&mut self.map.show_candidates, "Scores");
            ui.checkbox(&mut self.map.show_terrain, "Terrain bounds");
            ui.checkbox(&mut self.map.show_modalities, "Doppler / thunder estimates");
            ui.checkbox(&mut self.map.show_overlap, "Low-score overlap");
            ui.checkbox(&mut self.map.time_filter, "Strikes near playhead");
        });
    }
    fn scoring_controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("Search & evidence");
        let doc = self.document.as_mut().unwrap();
        egui::CollapsingHeader::new("Search area & grid")
            .default_open(true)
            .show(ui, |ui| {
                muted(ui, "Draw a rectangle on the map or enter its bounds. Smaller spacing needs more receivers; the limit is 10,000.");
        egui::Grid::new("search_bounds")
            .num_columns(2)
            .show(ui, |ui| {
                for (label, v) in ["South", "West", "North", "East"]
                    .iter()
                    .zip(&mut doc.map.bounds)
                {
                    ui.label(*label);
                    ui.add(egui::DragValue::new(v).speed(0.001).max_decimals(6));
                    ui.end_row();
                }
                ui.label("Grid spacing preset");
                egui::ComboBox::from_id_salt("grid_spacing_preset")
                    .selected_text(grid_preset_label(doc.map.grid_spacing_m))
                    .show_ui(ui, |ui| {
                        for (label, value) in GRID_PRESETS {
                            ui.selectable_value(&mut doc.map.grid_spacing_m, value, label);
                        }
                        ui.add_enabled(false, egui::Label::new("Custom — edit spacing below"));
                    });
                ui.end_row();
                ui.label("Maximum grid spacing");
                ui.add(
                    egui::DragValue::new(&mut doc.map.grid_spacing_m)
                        .range(10.0..=100_000.0)
                        .speed(25.0)
                        .suffix(" m"),
                );
                ui.end_row();
            });
        match geosonnar::workspace_scoring::grid_observers(&doc.map) {
            Ok(points) => {
                ui.label(format!(
                    "{} receiver locations · includes boundary points",
                    points.len()
                ));
            }
            Err(e) => {
                ui.colored_label(tone(ui, AMBER), e);
            }
        }
            });
        egui::CollapsingHeader::new("Terms to compare")
            .default_open(true)
            .show(ui, |ui| {
        egui::Grid::new("evidence_terms")
            .striped(true)
            .num_columns(3)
            .show(ui, |ui| {
                ui.label("Include");
                ui.label("Weight");
                ui.label("Model floor");
                ui.end_row();
                ui.checkbox(&mut doc.mix.doppler_frequency, "Doppler frequency");
                ui.add(
                    egui::DragValue::new(&mut doc.mix.frequency_weight)
                        .range(0.01..=100.0)
                        .speed(0.1),
                );
                ui.add(
                    egui::DragValue::new(&mut doc.mix.frequency_scale_hz)
                        .range(0.01..=100.0)
                        .speed(0.1)
                        .suffix(" Hz"),
                );
                ui.end_row();
                ui.checkbox(&mut doc.mix.doppler_timing, "Doppler timing");
                ui.add(
                    egui::DragValue::new(&mut doc.mix.doppler_timing_weight)
                        .range(0.01..=100.0)
                        .speed(0.1),
                );
                ui.add(
                    egui::DragValue::new(&mut doc.mix.doppler_timing_scale_s)
                        .range(0.01..=300.0)
                        .speed(0.1)
                        .suffix(" s"),
                );
                ui.end_row();
                ui.checkbox(&mut doc.mix.thunder_timing, "Thunder / strike timing");
                ui.add(
                    egui::DragValue::new(&mut doc.mix.thunder_weight)
                        .range(0.01..=100.0)
                        .speed(0.1),
                );
                ui.add(
                    egui::DragValue::new(&mut doc.mix.thunder_scale_s)
                        .range(0.01..=300.0)
                        .speed(0.1)
                        .suffix(" s"),
                );
                ui.end_row();
            });
        muted(
            ui,
            "Lower scores mean smaller residuals in the chosen uncertainty units. Weights are user choices; scores are not probabilities.",
        );
            });
        egui::CollapsingHeader::new("Reviewed events & associations")
            .default_open(true)
            .show(ui, |ui| {
                muted(ui, "Confirm measurements and assess the sound source in Audio review. Choose aircraft or strike associations explicitly.");
        for (i, e) in doc.events.iter_mut().enumerate() {
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(e.review == ReviewState::Confirmed, |ui| {
                    ui.checkbox(
                        &mut e.selected_for_scoring,
                        format!("{} · {:.1}s", e.label, e.arrival_s),
                    );
                });
                if ui.small_button("Inspect").clicked() {
                    self.selected = Some(i);
                }
            });
        }
        if let Some(e) = self.selected.and_then(|i| doc.events.get_mut(i)) {
            ui.strong(format!("Selected: {}", e.label));
            match e.kind {
                EventKind::Doppler => {
                    egui::ComboBox::from_id_salt("score_flight_choice")
                        .selected_text(if e.flight_id.is_empty() {
                            "Select associated aircraft"
                        } else {
                            &e.flight_id
                        })
                        .show_ui(ui, |ui| {
                            for t in self.layers.iter().flat_map(|l| &l.tracks) {
                                ui.selectable_value(&mut e.flight_id, t.id.clone(), &t.id);
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.label("Emitted tone");
                        ui.add(
                            egui::DragValue::new(&mut e.emitted_frequency_hz)
                                .range(0.0..=4000.0)
                                .speed(0.5)
                                .suffix(" Hz"),
                        );
                    });
                    if let Some(track) = &self.map.selected_track
                        && ui.button(format!("Associate map track {track}")).clicked()
                    {
                        e.flight_id = track.clone();
                    }
                }
                EventKind::Thunder => {
                    egui::ComboBox::from_id_salt("score_strike_choice")
                        .selected_text(if e.strike_id.is_empty() {
                            "Select associated lightning strike"
                        } else {
                            &e.strike_id
                        })
                        .show_ui(ui, |ui| {
                            if let Some(c) = &self.lightning {
                                for s in &c.strikes {
                                    ui.selectable_value(
                                        &mut e.strike_id,
                                        s.id.clone(),
                                        format!("{} · UTC {:.3}", s.id, s.time_utc_s),
                                    );
                                }
                            }
                        });
                    if let Some(strike) = &self.map.selected_strike
                        && ui
                            .button(format!("Associate map strike {strike}"))
                            .clicked()
                    {
                        e.strike_id = strike.clone();
                    }
                }
            }
        }
            });
        egui::CollapsingHeader::new("Advanced timing & physical assumptions")
            .show(ui, |ui| {
        ui.label("Recording start — UTC");
        accessible_text(
            ui,
            "recording_utc",
            &mut doc.recording_start_utc,
            "YYYY-MM-DD HH:MM:SS",
            false,
            ui.available_width(),
        );
        egui::Grid::new("physical_parameters")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Clock adjustment");
                ui.add(
                    egui::DragValue::new(&mut doc.map.clock_adjustment_s)
                        .speed(0.1)
                        .suffix(" s"),
                );
                ui.end_row();
                ui.label("Fixed sound speed");
                ui.add_enabled(doc.atmosphere.is_none(),
                    egui::DragValue::new(&mut doc.sound_speed_mps)
                        .range(250.0..=400.0)
                        .speed(0.1)
                        .suffix(" m/s"),
                );
                ui.end_row();
                ui.label("Receiver ellipsoid height");
                ui.add(
                    egui::DragValue::new(&mut doc.map.receiver_height_m)
                        .range(-500.0..=9000.0)
                        .suffix(" m"),
                );
                ui.end_row();
            });
        let mut fallback = doc.strike_fallback_height_m.is_some();
        if ui
            .checkbox(&mut fallback, "Declare fallback strike ellipsoid height")
            .changed()
        {
            doc.strike_fallback_height_m = fallback.then_some(0.0);
        }
        if let Some(height) = &mut doc.strike_fallback_height_m {
            ui.add(
                egui::DragValue::new(height)
                    .range(-1000.0..=30000.0)
                    .suffix(" m"),
            );
        }
                muted(ui, "Heights are WGS84 ellipsoid heights. Sound speed and clock shift are user assumptions, not fitted values.");
                if let Some(profile) = &doc.atmosphere {
                    ui.label(format!("Atmosphere: {} · {} levels", profile.name,profile.levels.len()));
                    muted(ui, "Profile propagation active for aircraft and thunder. Limit: 400 receivers. Manage the profile in Data → Atmospheric profile.");
                } else {
                    muted(ui, "Optional altitude-dependent weather: Data → Atmospheric profile.");
                }
            });
        let readiness = super::acoustic_review::readiness_ui(ui, doc, false);
        if let Some(index) = readiness.review_event {
            self.open_source_review(index);
            return;
        }
        self.height_model_ui(ui);
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.busy() && !readiness.blocked,
                    egui::Button::new("Score search grid"),
                )
                .clicked()
            {
                self.score_locations(ui.ctx(), false);
            }
            if ui
                .add_enabled(
                    !self.busy() && !readiness.blocked && self.map.receiver.is_some(),
                    egui::Button::new("Score clicked receiver"),
                )
                .clicked()
            {
                self.score_locations(ui.ctx(), true);
            }
        });
        if let Some(p) = self.map.receiver {
            ui.label(format!("Receiver pin: {:.6}, {:.6}", p[0], p[1]));
        }
        egui::CollapsingHeader::new("Loaded data & timeline origin").show(ui, |ui| {
            ui.label("UTC origin for files without one (explicit)");
            accessible_text(
                ui,
                "map_origin",
                &mut self.map.time_origin,
                "YYYY-MM-DD HH:MM:SS",
                false,
                ui.available_width(),
            );
            let mut remove = None;
            for (i, layer) in self.layers.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!(
                        "{} · {} tracks",
                        layer
                            .source_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy(),
                        layer.tracks.len()
                    ));
                    if ui.small_button("Remove layer").clicked() {
                        remove = Some(i);
                    }
                });
                for note in &layer.notes {
                    muted(ui, note);
                }
            }
            if let Some(i) = remove {
                let sha = self.layers[i].source_sha256.clone();
                self.forget_dataset(&sha);
            }
            if let Some(lightning) = &self.lightning {
                let sha = lightning.input_sha256.clone();
                ui.label(format!(
                    "{} lightning strikes · {}",
                    lightning.strikes.len(),
                    lightning.provenance.provider
                ));
                if ui.small_button("Remove lightning layer").clicked() {
                    self.forget_dataset(&sha);
                }
            }
            let bindings = self.document.as_ref().unwrap().datasets.clone();
            for binding in bindings {
                if !binding.path.exists() {
                    ui.colored_label(
                        tone(ui, RED),
                        format!("Unavailable: {}", display_path(&binding.path)),
                    );
                    if ui
                        .small_button(format!(
                            "Remove unavailable reference {}",
                            &binding.sha256[..8]
                        ))
                        .clicked()
                    {
                        self.forget_dataset(&binding.sha256);
                    }
                }
            }
        });
    }
    fn draw_map(&mut self, ui: &mut egui::Ui, height: f32) {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.map.span =
                    (self.map.span * (-scroll as f64 / 300.0).exp()).clamp(0.0005, 360.0);
            }
        }
        if response.dragged() && !self.map.draw_region {
            let delta = ui.input(|i| i.pointer.delta());
            let center = self.map.unproject(rect, rect.center() - delta);
            self.map.center_lat = center[0];
            self.map.center_lon = center[1];
        }
        if response.drag_started()
            && self.map.draw_region
            && let Some(p) = response.interact_pointer_pos()
        {
            self.map.drag_start = Some(self.map.unproject(rect, p));
        }
        if response.drag_stopped()
            && self.map.draw_region
            && let (Some(start), Some(p)) =
                (self.map.drag_start.take(), response.interact_pointer_pos())
        {
            let end = self.map.unproject(rect, p);
            let bounds = [
                start[0].min(end[0]),
                start[1].min(end[1]),
                start[0].max(end[0]),
                start[1].max(end[1]),
            ];
            if bounds[2] - bounds[0] > 1e-7
                && bounds[3] - bounds[1] > 1e-7
                && bounds[3] - bounds[1] < 180.0
            {
                self.document.as_mut().unwrap().map.bounds = bounds;
                self.map.draw_region = false;
            }
        }
        painter.rect_filled(rect, 0, Color32::from_rgb(224, 233, 239));
        for country in &self.map.world {
            for ring in &country.rings {
                for pair in ring.windows(2) {
                    let a = self.map.project(rect, pair[0][1], pair[0][0]);
                    let b = self.map.project(rect, pair[1][1], pair[1][0]);
                    if (a.x - b.x).abs() < rect.width() * 0.6
                        && (rect.contains(a)
                            || rect.contains(b)
                            || Rect::from_two_pos(a, b).intersects(rect))
                    {
                        painter.line_segment(
                            [a, b],
                            Stroke::new(1.0, Color32::from_rgb(143, 154, 157)),
                        );
                    }
                }
            }
            let p = self.map.project(rect, country.label[1], country.label[0]);
            if rect.contains(p) && self.map.span < 170.0 {
                painter.text(
                    p,
                    Align2::CENTER_CENTER,
                    &country.name,
                    FontId::proportional(11.0),
                    Color32::from_rgb(88, 99, 105),
                );
            }
        }
        if self.map.online_tiles {
            let cache = self
                .case
                .as_ref()
                .unwrap()
                .manifest
                .parent()
                .unwrap()
                .join("map-tile-cache");
            if let Some(status) = self.map.tiles.paint(
                ui.ctx(),
                &painter,
                rect,
                self.map.center_lat,
                self.map.center_lon,
                self.map.span,
                &cache,
            ) {
                painter.text(
                    rect.left_top() + egui::vec2(8.0, 8.0),
                    Align2::LEFT_TOP,
                    status,
                    FontId::proportional(11.0),
                    TEXT,
                );
            }
        }
        // Context graticule; the actual scoring grid is drawn separately below.
        let step = if self.map.span > 90.0 {
            30.0
        } else if self.map.span > 15.0 {
            5.0
        } else if self.map.span > 2.0 {
            1.0
        } else if self.map.span > 0.2 {
            0.1
        } else {
            0.01
        };
        let corners = [
            self.map.unproject(rect, rect.left_bottom()),
            self.map.unproject(rect, rect.right_top()),
        ];
        let south = corners[0][0];
        let north = corners[1][0];
        let mut lat = (south / step).ceil() * step;
        while lat < north {
            let y = self.map.project(rect, lat, self.map.center_lon).y;
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                Stroke::new(0.5, Color32::from_gray(185)),
            );
            painter.text(
                egui::pos2(rect.left() + 3.0, y),
                Align2::LEFT_BOTTOM,
                format!("{lat:.3}°"),
                FontId::monospace(10.0),
                MUTED,
            );
            lat += step;
        }
        let doc = self.document.as_ref().unwrap();
        let bounds = doc.map.bounds;
        let region = Rect::from_two_pos(
            self.map.project(rect, bounds[0], bounds[1]),
            self.map.project(rect, bounds[2], bounds[3]),
        );
        if bounds[3] - bounds[1] < 180.0 && region.intersects(rect) {
            painter.rect_stroke(
                region,
                0,
                Stroke::new(2.0, Color32::from_rgb(80, 66, 155)),
                egui::StrokeKind::Inside,
            );
            painter.text(
                region.left_top() + egui::vec2(4.0, 4.0),
                Align2::LEFT_TOP,
                format!("Search area · ≤{:.0} m grid", doc.map.grid_spacing_m),
                FontId::proportional(12.0),
                Color32::from_rgb(80, 66, 155),
            );
        }
        if self.map.show_grid && self.wide.enabled {
            if let Ok(grid) = geosonnar::receiver_grid::GridLayout::new(&doc.map) {
                let stride = grid.count.div_ceil(1000).max(1);
                for index in (0..grid.count).step_by(stride as usize) {
                    if let Ok(point) = grid.observer(index) {
                        let position =
                            self.map
                                .project(rect, point.latitude_deg, point.longitude_deg);
                        if rect.contains(position) {
                            painter.circle_filled(
                                position,
                                1.5,
                                Color32::from_rgba_unmultiplied(93, 77, 150, 100),
                            );
                        }
                    }
                }
                painter.text(
                    rect.left_bottom() + egui::vec2(5., -5.),
                    Align2::LEFT_BOTTOM,
                    format!(
                        "Wide grid: {} receivers; display sample up to 1,000; physics uses all",
                        grid.count
                    ),
                    FontId::proportional(11.),
                    tone(ui, AMBER),
                );
            }
        } else if self.map.show_grid
            && let Ok(points) = geosonnar::workspace_scoring::grid_observers(&doc.map)
        {
            for point in points {
                let position = self
                    .map
                    .project(rect, point.latitude_deg, point.longitude_deg);
                if rect.contains(position) {
                    painter.circle_filled(
                        position,
                        1.5,
                        Color32::from_rgba_unmultiplied(93, 77, 150, 100),
                    );
                }
            }
        }
        let timeline = workspace::parse_utc(&doc.recording_start_utc)
            .ok()
            .map(|t| t + doc.map.clock_adjustment_s + self.cursor_s);
        let pointer = response.hover_pos();
        let mut nearest: Option<(f32, String, String)> = None;
        let mut consider = |p: Pos2, kind: &str, id: &str| {
            if let Some(cursor) = pointer {
                let distance = p.distance(cursor);
                if distance < 12.0 && nearest.as_ref().is_none_or(|a| distance < a.0) {
                    nearest = Some((distance, kind.into(), id.into()));
                }
            }
        };
        for layer in &self.layers {
            if self.map.show_terrain
                && let Some(terrain) = &layer.terrain
            {
                let b = terrain.bounds;
                let area = Rect::from_two_pos(
                    self.map.project(rect, b[0], b[1]),
                    self.map.project(rect, b[2], b[3]),
                );
                painter.rect_stroke(
                    area,
                    0,
                    Stroke::new(2.0, Color32::from_rgb(102, 118, 43)),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    area.left_bottom(),
                    Align2::LEFT_BOTTOM,
                    format!("Terrain {}×{}", terrain.width, terrain.height),
                    FontId::proportional(11.0),
                    Color32::from_rgb(102, 118, 43),
                );
            }
            if self.map.show_candidates {
                for c in layer.candidates.iter().take(10000) {
                    let p = self.map.project(rect, c.latitude_deg, c.longitude_deg);
                    if rect.contains(p) {
                        painter.circle_stroke(
                            p,
                            3.0,
                            Stroke::new(1.5, Color32::from_rgb(40, 129, 85)),
                        );
                        consider(p, "candidate", &c.id);
                    }
                }
            }
        }
        if self.map.show_tracks {
            self.map
                .paths
                .update(&self.layers, self.map.selected_track.as_deref());
            for segment in self.map.paths.base.iter().chain(&self.map.paths.selected) {
                let track = &self.layers[segment.layer].tracks[segment.track];
                let selected = self.map.selected_track.as_ref() == Some(&track.id);
                let color = if selected {
                    Color32::from_rgb(4, 72, 151)
                } else {
                    Color32::from_rgb(51, 126, 188)
                };
                for pair in segment.indices.windows(2) {
                    let a = self.map.project(
                        rect,
                        track.points[pair[0]].latitude_deg,
                        track.points[pair[0]].longitude_deg,
                    );
                    let b = self.map.project(
                        rect,
                        track.points[pair[1]].latitude_deg,
                        track.points[pair[1]].longitude_deg,
                    );
                    if (a.x - b.x).abs() >= rect.width() * 0.6
                        || !Rect::from_two_pos(a, b).expand(2.0).intersects(rect)
                    {
                        continue;
                    }
                    painter
                        .line_segment([a, b], Stroke::new(if selected { 2.6 } else { 1.3 }, color));
                    if let Some(cursor) = pointer {
                        let delta = b - a;
                        let fraction = ((cursor - a).dot(delta) / delta.length_sq().max(1e-12))
                            .clamp(0.0, 1.0);
                        consider(a + delta * fraction, "track", &track.id);
                    }
                }
                if let Some(now) = timeline
                    && let Some([latitude, longitude]) = segment.current_point(track, now)
                {
                    let point = self.map.project(rect, latitude, longitude);
                    if rect.contains(point) {
                        painter.circle_filled(point, 5.0, color);
                        if selected {
                            painter.text(
                                point + egui::vec2(7.0, -7.0),
                                Align2::LEFT_BOTTOM,
                                &track.id,
                                FontId::proportional(11.0),
                                color,
                            );
                        }
                        consider(point, "track", &track.id);
                    }
                }
            }
            if self.map.paths.original_edges > self.map.paths.displayed_edges()
                || self.map.paths.original_segments > self.map.paths.base.len()
            {
                painter.text(rect.left_top()+egui::vec2(8.0,32.0),Align2::LEFT_TOP,
                    format!("Display paths simplified: {} / {} source segments · full tracks retained for scoring",self.map.paths.base.len(),self.map.paths.original_segments),
                    FontId::proportional(11.0),Color32::from_rgb(70,75,90));
            }
        }
        if self.map.show_strikes {
            let strikes: Vec<_> = self
                .lightning
                .as_ref()
                .map(|l| l.strikes.iter().collect())
                .unwrap_or_else(|| {
                    self.layers
                        .iter()
                        .flat_map(|l| l.lightning.iter())
                        .collect()
                });
            for strike in strikes {
                if self.map.time_filter
                    && timeline.is_some_and(|t| (strike.time_utc_s - t).abs() > 300.0)
                {
                    continue;
                }
                let p = self
                    .map
                    .project(rect, strike.latitude_deg, strike.longitude_deg);
                if !rect.contains(p) {
                    continue;
                }
                let selected = self.map.selected_strike.as_ref() == Some(&strike.id);
                let color = Color32::from_rgb(180, 100, 0);
                let r = if selected { 7.0 } else { 4.0 };
                painter.line_segment(
                    [p + egui::vec2(-r, -r), p + egui::vec2(r, r)],
                    Stroke::new(2.0, color),
                );
                painter.line_segment(
                    [p + egui::vec2(-r, r), p + egui::vec2(r, -r)],
                    Stroke::new(2.0, color),
                );
                consider(p, "strike", &strike.id);
                if selected {
                    painter.text(
                        p + egui::vec2(9.0, -8.0),
                        Align2::LEFT_BOTTOM,
                        format!("{} · UTC {:.3}", strike.id, strike.time_utc_s),
                        FontId::proportional(12.0),
                        color,
                    );
                }
            }
        }
        if self.map.show_candidates
            && let Some(report) = &self.score
        {
            let best = report
                .candidates
                .iter()
                .filter_map(|c| c.score)
                .fold(f64::INFINITY, f64::min);
            let worst = report
                .candidates
                .iter()
                .filter_map(|c| c.score)
                .fold(0.0_f64, f64::max);
            for c in &report.candidates {
                let p = self.map.project(
                    rect,
                    c.candidate.observer.latitude_deg,
                    c.candidate.observer.longitude_deg,
                );
                if !rect.contains(p) {
                    continue;
                }
                let ratio = c.score.map(|v| {
                    ((v.ln_1p() - best.ln_1p()) / (worst.ln_1p() - best.ln_1p()).max(1e-12))
                        .clamp(0.0, 1.0)
                });
                let color = ratio
                    .map(|r| {
                        Color32::from_rgb((35.0 + 190.0 * r) as u8, (150.0 - 85.0 * r) as u8, 70)
                    })
                    .unwrap_or(Color32::GRAY);
                painter.circle_filled(p, if c.rank == Some(1) { 6.0 } else { 3.0 }, color);
                if c.rank == Some(1) {
                    painter.circle_stroke(p, 8.0, Stroke::new(2.0, Color32::BLACK));
                }
                consider(p, "score", &c.candidate.id);
            }
        }
        let current_regional = self.regional.report_document.as_ref() == self.document.as_ref();
        if self.map.show_candidates
            && current_regional
            && let Some(report) = &self.regional.report
        {
            if !report.location_summaries.is_empty() {
                let best = report
                    .location_summaries
                    .iter()
                    .filter_map(|row| row.best_score)
                    .fold(f64::INFINITY, f64::min);
                let worst = report
                    .location_summaries
                    .iter()
                    .filter_map(|row| row.best_score)
                    .fold(0.0_f64, f64::max);
                for row in &report.location_summaries {
                    let p = self.map.project(
                        rect,
                        row.observer.latitude_deg,
                        row.observer.longitude_deg,
                    );
                    if !rect.contains(p) {
                        continue;
                    }
                    let color = row
                        .best_score
                        .map(|score| {
                            let r = ((score.ln_1p() - best.ln_1p())
                                / (worst.ln_1p() - best.ln_1p()).max(1e-12))
                            .clamp(0.0, 1.0);
                            Color32::from_rgb(
                                (35.0 + 190.0 * r) as u8,
                                (150.0 - 85.0 * r) as u8,
                                95,
                            )
                        })
                        .unwrap_or(Color32::GRAY);
                    painter.circle_filled(p, 4.0, color);
                    if row.best_score == Some(best) {
                        painter.circle_stroke(p, 8.0, Stroke::new(2.0, color));
                    }
                    if report.status == geosonnar::regional_search::SearchStatus::Cancelled {
                        painter.circle_stroke(
                            p,
                            6.0,
                            Stroke::new(1.0, Color32::from_rgb(190, 120, 20)),
                        );
                    }
                    consider(p, "regional-location", &row.observer_index.to_string());
                }
            } else {
                let mut positions = std::collections::BTreeSet::new();
                for hypothesis in &report.hypotheses {
                    if !positions.insert((
                        hypothesis.observer.latitude_deg.to_bits(),
                        hypothesis.observer.longitude_deg.to_bits(),
                    )) {
                        continue;
                    }
                    let p = self.map.project(
                        rect,
                        hypothesis.observer.latitude_deg,
                        hypothesis.observer.longitude_deg,
                    );
                    if !rect.contains(p) {
                        continue;
                    }
                    let color =
                        if report.status == geosonnar::regional_search::SearchStatus::Completed {
                            Color32::from_rgb(120, 65, 180)
                        } else {
                            Color32::from_rgb(190, 120, 20)
                        };
                    painter.circle_stroke(
                        p,
                        if hypothesis.rank == 1 { 9.0 } else { 5.0 },
                        Stroke::new(2.0, color),
                    );
                    consider(p, "regional", &hypothesis.id);
                }
            }
        }
        let independent = if current_regional && self.regional.guided {
            self.regional.comparison.as_ref()
        } else {
            None
        };
        if let Some(comparison) = independent
            && self.map.show_modalities
        {
            use geosonnar::regional_search::SearchMode;
            let best_for = |mode| {
                comparison
                    .modes
                    .iter()
                    .find(|entry| entry.mode == mode)
                    .and_then(|entry| entry.report.as_ref())
                    .and_then(|report| report.hypotheses.first())
            };
            if let (Some(d), Some(t)) = (
                best_for(SearchMode::DopplerOnly),
                best_for(SearchMode::ThunderOnly),
            ) {
                let a = self
                    .map
                    .project(rect, d.observer.latitude_deg, d.observer.longitude_deg);
                let b = self
                    .map
                    .project(rect, t.observer.latitude_deg, t.observer.longitude_deg);
                painter.line_segment([a, b], Stroke::new(1.5, Color32::from_rgb(130, 95, 155)));
            }
            for (mode, label, color, offset) in [
                (
                    SearchMode::DopplerOnly,
                    "Independent Doppler",
                    Color32::from_rgb(35, 100, 200),
                    egui::vec2(12.0, -25.0),
                ),
                (
                    SearchMode::ThunderOnly,
                    "Independent thunder",
                    Color32::from_rgb(185, 95, 10),
                    egui::vec2(12.0, -5.0),
                ),
                (
                    SearchMode::Combined,
                    "Shared-clock combined",
                    Color32::from_rgb(130, 60, 165),
                    egui::vec2(12.0, 15.0),
                ),
            ] {
                if let Some(best) = best_for(mode) {
                    let p = self.map.project(
                        rect,
                        best.observer.latitude_deg,
                        best.observer.longitude_deg,
                    );
                    if rect.contains(p) {
                        painter.circle_stroke(p, 10.0, Stroke::new(2.5, color));
                        painter.text(
                            p + offset,
                            Align2::LEFT_CENTER,
                            format!("{label} ({:+.2}s)", best.clock_offset_s),
                            FontId::proportional(13.0),
                            color,
                        );
                    }
                }
            }
        }
        if independent.is_none()
            && let Some(diagnostics) = self.score.as_ref().and_then(|r| r.modalities.as_ref())
        {
            if self.map.show_overlap {
                let shared: std::collections::BTreeSet<_> = diagnostics
                    .comparison
                    .overlap_candidate_ids
                    .iter()
                    .collect();
                for cell in &diagnostics.doppler.ranked {
                    if shared.contains(&cell.candidate_id) {
                        let p = self.map.project(
                            rect,
                            cell.observer.latitude_deg,
                            cell.observer.longitude_deg,
                        );
                        if rect.contains(p) {
                            painter.circle_stroke(
                                p,
                                5.0,
                                Stroke::new(2.0, Color32::from_rgb(20, 155, 125)),
                            );
                        }
                    }
                }
            }
            if self.map.show_modalities {
                if let (Some(d), Some(t)) = (&diagnostics.doppler.best, &diagnostics.thunder.best) {
                    let a =
                        self.map
                            .project(rect, d.observer.latitude_deg, d.observer.longitude_deg);
                    let b =
                        self.map
                            .project(rect, t.observer.latitude_deg, t.observer.longitude_deg);
                    painter.line_segment([a, b], Stroke::new(1.5, Color32::from_rgb(130, 95, 155)));
                }
                for (label, summary, color, offset) in [
                    (
                        "Doppler",
                        &diagnostics.doppler,
                        Color32::from_rgb(35, 100, 200),
                        egui::vec2(12.0, -25.0),
                    ),
                    (
                        "Thunder",
                        &diagnostics.thunder,
                        Color32::from_rgb(185, 95, 10),
                        egui::vec2(12.0, -5.0),
                    ),
                    (
                        "Combined",
                        &diagnostics.combined,
                        Color32::from_rgb(130, 60, 165),
                        egui::vec2(12.0, 15.0),
                    ),
                ] {
                    if let Some(best) = &summary.best {
                        let p = self.map.project(
                            rect,
                            best.observer.latitude_deg,
                            best.observer.longitude_deg,
                        );
                        if rect.contains(p) {
                            painter.circle_stroke(p, 10.0, Stroke::new(2.5, color));
                            painter.text(
                                p + offset,
                                Align2::LEFT_CENTER,
                                label,
                                FontId::proportional(13.0),
                                color,
                            );
                            consider(p, "score", &best.candidate_id);
                        }
                    }
                }
            }
        }
        if let Some(p) = self.map.receiver {
            let p = self.map.project(rect, p[0], p[1]);
            painter.circle_stroke(p, 8.0, Stroke::new(2.0, Color32::BLACK));
            painter.line_segment(
                [p - egui::vec2(12.0, 0.0), p + egui::vec2(12.0, 0.0)],
                Stroke::new(1.5, Color32::BLACK),
            );
            painter.line_segment(
                [p - egui::vec2(0.0, 12.0), p + egui::vec2(0.0, 12.0)],
                Stroke::new(1.5, Color32::BLACK),
            );
        }
        if let (Some(start), Some(p)) = (self.map.drag_start, pointer) {
            painter.rect_stroke(
                Rect::from_two_pos(self.map.project(rect, start[0], start[1]), p),
                0,
                Stroke::new(2.0, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        if response.clicked() && !self.map.draw_region {
            if let Some((_, kind, id)) = &nearest {
                match kind.as_str() {
                    "track" => self.map.selected_track = Some(id.clone()),
                    "strike" => self.map.selected_strike = Some(id.clone()),
                    "score" => self.map.selected_candidate = Some(id.clone()),
                    "regional-location" => {
                        if let Ok(index) = id.parse::<usize>() {
                            self.regional.select_location(index);
                            if let Some(row) = self.regional.report.as_ref().and_then(|report| {
                                report
                                    .location_summaries
                                    .iter()
                                    .find(|row| row.observer_index == index)
                            }) {
                                self.map.receiver =
                                    Some([row.observer.latitude_deg, row.observer.longitude_deg]);
                            }
                        }
                    }
                    "regional" => {
                        self.regional.select_hypothesis(id);
                        if let Some(hypothesis) =
                            self.regional.report.as_ref().and_then(|report| {
                                report.hypotheses.iter().find(|row| &row.id == id)
                            })
                        {
                            self.map.receiver = Some([
                                hypothesis.observer.latitude_deg,
                                hypothesis.observer.longitude_deg,
                            ]);
                        }
                    }
                    _ => {}
                }
            } else if let Some(p) = pointer {
                self.map.receiver = Some(self.map.unproject(rect, p));
            }
        }
        if let Some((_, kind, id)) = nearest {
            response.clone().on_hover_text(format!("{kind}: {id}"));
        }
        if let Some(p) = pointer {
            let geo = self.map.unproject(rect, p);
            painter.text(
                rect.right_bottom() - egui::vec2(8.0, 8.0),
                Align2::RIGHT_BOTTOM,
                format!("{:.6}, {:.6}", geo[0], geo[1]),
                FontId::monospace(12.0),
                TEXT,
            );
        }
        if self.map.online_tiles {
            super::tiles::paint_attribution(&painter, rect);
        }
        let metres = self.map.span.to_radians()
            * 6_378_137.0
            * self.map.center_lat.to_radians().cos()
            * 100.0
            / rect.width() as f64;
        painter.line_segment(
            [
                rect.left_bottom() + egui::vec2(12.0, -20.0),
                rect.left_bottom() + egui::vec2(112.0, -20.0),
            ],
            Stroke::new(3.0, TEXT),
        );
        painter.text(
            rect.left_bottom() + egui::vec2(12.0, -25.0),
            Align2::LEFT_BOTTOM,
            if metres > 1000.0 {
                format!("≈ {:.1} km", metres / 1000.0)
            } else {
                format!("≈ {metres:.0} m")
            },
            FontId::proportional(11.0),
            TEXT,
        );
    }
    fn save_comparison(&mut self, ctx: &egui::Context) {
        let (Some(report), Some(doc), Some(case)) = (&self.score, &self.score_document, &self.case)
        else {
            return;
        };
        if self.document.as_ref() != Some(doc) {
            self.notice = Some((
                true,
                "Evidence changed after this comparison. Score the locations again before saving."
                    .into(),
            ));
            return;
        }
        let report = report.clone();
        let doc = doc.clone();
        let manifest = case.manifest.clone();
        self.spawn(ctx, "Saving comparison evidence…", move || {
            let path = workspace::save(&manifest, &doc)?;
            let destination = path.parent().unwrap().join("comparison.json");
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(&mut file, &report).map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            Ok(ResultValue::Saved(path, doc))
        });
    }

    fn modality_summary_ui(&mut self, ui: &mut egui::Ui) {
        use geosonnar::modality_diagnostics::{ComparisonStatus, DiagnosticOptions};
        let Some(report) = &mut self.score else {
            return;
        };
        let Some(previous) = &report.modalities else {
            return;
        };
        let mut delta = previous.options.near_best_score_delta;
        let bounds = previous.bounds;
        egui::CollapsingHeader::new("Doppler, thunder and their combined search")
            .default_open(true).show(ui, |ui| {
            let height = (ui.available_height() * 0.42).clamp(0.0, 240.0);
            egui::ScrollArea::vertical().id_salt("modality_summary_scroll").max_height(height).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Show cells within best score +");
                if ui.add(egui::DragValue::new(&mut delta).range(0.0..=100.0).speed(0.05)).changed() {
                    match geosonnar::modality_diagnostics::summarize(report,bounds,&DiagnosticOptions{near_best_score_delta:delta}) {
                        Ok(value) => report.modalities = Some(value),
                        Err(error) => { ui.colored_label(tone(ui,AMBER), error); }
                    }
                }
                muted(ui,"Display tolerance only; scores and selected evidence stay fixed.");
            });
            let diagnostics = report.modalities.as_ref().unwrap();
            let message = match diagnostics.comparison.status {
                ComparisonStatus::NotComparable => "Both modalities need selected, available evidence for a comparison.",
                ComparisonStatus::SharedBest => "Both modalities prefer the same grid cell. Check the width of their low-score areas before interpreting this.",
                ComparisonStatus::DifferentBestWithOverlap => "The best locations differ, but their low-score areas overlap. This can help narrow otherwise ambiguous evidence.",
                ComparisonStatus::DisjointNearBest => "The low-score areas do not overlap at this tolerance. Review event associations, clock, search coverage and model assumptions; combined scoring is still available.",
            };
            ui.label(message);
            if let Some(distance) = diagnostics.comparison.best_separation_m {
                ui.strong(format!("Doppler ↔ thunder: {:.0} m apart · {} shared low-score cells", distance, diagnostics.comparison.overlap_count));
            }
            for (label,summary,color) in [
                ("Doppler", &diagnostics.doppler, Color32::from_rgb(35,100,200)),
                ("Thunder", &diagnostics.thunder, Color32::from_rgb(185,95,10)),
                ("Combined", &diagnostics.combined, Color32::from_rgb(130,60,165)),
            ] {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(color,RichText::new(label).strong());
                    if let Some(best) = &summary.best {
                        ui.label(format!("{:.6}°, {:.6}° · score {:.4} · {} low-score cells · {} rankable / {} unavailable",best.observer.latitude_deg,best.observer.longitude_deg,best.score,summary.near_best_candidate_ids.len(),summary.rankable_count,summary.unavailable_count));
                        if ui.small_button(format!("Inspect {label}")).clicked() {
                            self.map.selected_candidate = Some(best.candidate_id.clone());
                            self.map.receiver = Some([best.observer.latitude_deg,best.observer.longitude_deg]);
                            self.map.center_lat = best.observer.latitude_deg;
                            self.map.center_lon = best.observer.longitude_deg;
                        }
                        if best.on_search_boundary == Some(true) { ui.colored_label(tone(ui,AMBER),"Best cell is on the search edge"); }
                        if best.outside_search_bounds == Some(true) { ui.colored_label(tone(ui,AMBER),"Outside declared search area"); }
                    } else {
                        ui.label(summary.unavailable_reason.as_deref().unwrap_or("No rankable receiver"));
                    }
                });
                if summary.near_best_touches_boundary == Some(true) {
                    muted(ui,format!("{label} low-score cells reach the search edge; broader alternatives may remain."));
                }
            }
            if let (Some(d),Some(t)) = (diagnostics.comparison.combined_to_doppler_m,diagnostics.comparison.combined_to_thunder_m) {
                ui.label(format!("Combined estimate: {:.0} m from Doppler, {:.0} m from thunder",d,t));
            }
            muted(ui,"This breakdown holds the chosen source associations and shared clock fixed. Use guided independent-mode searches to compare alternative sources and clocks.");
            muted(ui,"Low-score cells are a descriptive residual tolerance, not confidence regions. Separate scores use their own selected-term weights and are not probabilities. Map: blue Doppler, orange thunder, purple combined; green rings mark overlap.");
            });
        });
    }

    /// Full comparison browser; selecting a row also pins its receiver on the map.
    pub fn results_ui(&mut self, ui: &mut egui::Ui) {
        self.toolbar(ui);
        if self.document.is_none() {
            return;
        }
        if self.wide_results_ui(ui) {
            return;
        }
        if self.regional_results_ui(ui) {
            return;
        }
        ui.heading("Location comparison");
        if self.score.is_none() {
            muted(
                ui,
                "No comparison yet. In Map & search, choose a search area, confirmed events and scoring terms, then score the grid or a receiver pin.",
            );
            return;
        }
        let matches_evidence = self
            .score_document
            .as_ref()
            .is_some_and(|doc| self.deleted_sources_ready(doc))
            && self.score_document.as_ref() == self.document.as_ref();
        let mut inspect_audio = false;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.busy() && matches_evidence,
                    egui::Button::new("Save comparison + evidence snapshot"),
                )
                .clicked()
            {
                self.save_comparison(ui.ctx());
            }
            muted(
                ui,
                "Uncalibrated comparison · a low score does not verify a location.",
            );
            inspect_audio = ui
                .add_enabled(
                    !self.busy() && matches_evidence && self.fixed_selected_receiver().is_some(),
                    egui::Button::new("Inspect selected receiver in audio"),
                )
                .clicked();
        });
        if inspect_audio {
            self.inspect_fixed_candidate(ui.ctx());
        }
        if !matches_evidence {
            ui.colored_label(
                tone(ui, AMBER),
                if self.score_document.as_ref().is_some_and(|doc| !self.deleted_sources_ready(doc)) {
                    "Original source data were removed. The numerical result is retained as historical; restore verified sources before inspecting or saving a new evidence snapshot."
                } else { "Evidence changed. Run the comparison again before saving." },
            );
        }
        if let Some(doc) = &self.score_document {
            super::acoustic_review::assumptions_ui(
                ui,
                "fixed_source_assumptions",
                doc.active_scoring_events(),
            );
        }
        self.modality_summary_ui(ui);
        let report = self.score.as_ref().unwrap();
        ui.label(format!(
            "{} rankable / {} receivers · lower score is better · click a column to sort",
            report.ranked_candidate_ids.len(),
            report.candidates.len()
        ));
        muted(
            ui,
            "Term columns are weighted contributions to the combined score. Off = term not selected; unavailable values remain last in either sort direction.",
        );
        let details_max = (ui.available_width() * 0.45).clamp(220.0, 600.0);
        egui::Panel::right("receiver_result_details")
            .resizable(true)
            .default_size(320.0_f32.min(details_max))
            .size_range(220.0..=details_max)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("receiver_residuals").show(ui, |ui| {
                    ui.heading("Selected receiver");
                    let Some(candidate) = self.map.selected_candidate.as_ref()
                        .and_then(|id| report.candidates.iter().find(|row| &row.candidate.id == id)) else {
                        muted(ui, "Select any row to inspect every observation, contribution and missing-data reason. Its receiver is also pinned on the map.");
                        return;
                    };
                    let observer = &candidate.candidate.observer;
                    ui.strong(&candidate.candidate.id);
                    ui.label(format!("{:.6}°, {:.6}° · {:.1} m ellipsoid height", observer.latitude_deg, observer.longitude_deg, observer.ellipsoid_height_m));
                    ui.label(candidate.score.map(|value| format!("Combined score: {value:.6}")).unwrap_or_else(|| "Combined score unavailable".into()));
                    for reason in &candidate.unavailable_reasons {
                        ui.colored_label(tone(ui, RED), reason);
                    }
                    for term in &candidate.terms {
                        egui::CollapsingHeader::new(term_label(term.term)).default_open(true).show(ui, |ui| {
                            ui.label(format!("Weight: {} · model floor: {}", term.weight, term.residual_scale));
                            ui.label(term.weighted_contribution.map(|value| format!("Weighted contribution: {value:.6}")).unwrap_or_else(|| "Contribution unavailable".into()));
                            if let Some(reason) = &term.unavailable_reason {
                                ui.colored_label(tone(ui, RED), reason);
                            }
                            for residual in &term.residuals {
                                ui.separator();
                                ui.strong(format!("{} / {}", residual.event_id, residual.observation_id));
                                ui.label(format!("Observed {:.6} {} · predicted {:.6} {}", residual.observed, residual.unit, residual.predicted, residual.unit));
                                ui.label(format!("Residual {:+.6} {} · effective scale {:.6} {}", residual.residual, residual.unit, residual.effective_scale, residual.unit));
                                ui.label(format!("Observation uncertainty: {:.6} {} · normalized residual {:+.6} · squared {:.6}", residual.observation_uncertainty, residual.unit, residual.normalized_residual, residual.squared_normalized_residual));
                                if let Some(thunder) = &residual.thunder_prediction {
                                    ui.label(format!("Strike {} · {:.1} m slant range · {:.3} s propagation", thunder.strike_id, thunder.slant_distance_m, thunder.propagation_s));
                                    if let Some(source) = &thunder.source_interpretation {
                                        ui.label(format!("Source timestamps: {:.3} s representation · declared allowance {:.3} s", source.source_time_resolution_s, source.declared_time_representation_allowance_s));
                                        ui.label(format!("ETRS89-as-WGS84 approximation: {}", source.etrs89_as_wgs84_rationale));
                                        muted(ui, "No datum transformation or calibrated provider-clock error is claimed. The declared allowance is already bounded by the chosen model scale.");
                                    }
                                    if thunder.strike_height_assumed {
                                        ui.colored_label(tone(ui, AMBER), format!("Assumed strike ellipsoid height: {:.1} m", thunder.strike_ellipsoid_height_m));
                                    }
                                }
                            }
                        });
                    }
                    egui::CollapsingHeader::new("Comparison assumptions & limits").show(ui, |ui| {
                        ui.label(&report.calibration);
                        for limitation in &report.limitations { ui.label(limitation); }
                        muted(ui, "Residuals are averaged within each event, then events are averaged equally. Contributions use the sum of all selected term weights.");
                    });
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let table_width = RESULT_COLUMNS
                    .iter()
                    .map(|(_, _, width)| *width)
                    .sum::<f32>()
                    + ui.spacing().item_spacing.x * (RESULT_COLUMNS.len() - 1) as f32;
                egui::ScrollArea::horizontal()
                    .id_salt("results_columns")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_min_width(table_width);
                        ui.horizontal(|ui| {
                            for (column, title, width) in RESULT_COLUMNS {
                                let indicator = if self.map.result_sort == column {
                                    if self.map.result_descending {
                                        " ↓"
                                    } else {
                                        " ↑"
                                    }
                                } else {
                                    ""
                                };
                                if ui
                                    .add_sized(
                                        [width, 30.0],
                                        egui::Button::new(format!("{title}{indicator}")),
                                    )
                                    .clicked()
                                {
                                    if self.map.result_sort == column {
                                        self.map.result_descending = !self.map.result_descending;
                                    } else {
                                        self.map.result_sort = column;
                                        self.map.result_descending = false;
                                    }
                                }
                            }
                        });
                        let rows = sorted_results(
                            &report.candidates,
                            self.map.result_sort,
                            self.map.result_descending,
                        );
                        // Virtualized rows keep every receiver accessible without building 70,000 widgets per frame.
                        egui::ScrollArea::vertical()
                            .id_salt("all_receiver_scores")
                            .auto_shrink([false, false])
                            .show_rows(ui, 28.0, rows.len(), |ui, range| {
                                for index in range {
                                    let candidate = rows[index];
                                    let selected = self.map.selected_candidate.as_ref()
                                        == Some(&candidate.candidate.id);
                                    let mut clicked = false;
                                    ui.push_id(&candidate.candidate.id, |ui| {
                                        ui.horizontal(|ui| {
                                            for (column, _, width) in RESULT_COLUMNS {
                                                let text = match column.value(candidate) {
                                                    Some(value) if column == ResultColumn::Rank => {
                                                        format!("{value:.0}")
                                                    }
                                                    Some(value)
                                                        if matches!(
                                                            column,
                                                            ResultColumn::Latitude
                                                                | ResultColumn::Longitude
                                                        ) =>
                                                    {
                                                        format!("{value:.6}")
                                                    }
                                                    Some(value) => format!("{value:.5}"),
                                                    None if column == ResultColumn::Rank => {
                                                        "—".into()
                                                    }
                                                    None if column.term().is_some_and(|term| {
                                                        !report.selected_terms.contains(&term)
                                                    }) =>
                                                    {
                                                        "Off".into()
                                                    }
                                                    None => "Unavailable".into(),
                                                };
                                                clicked |= ui
                                                    .add_sized(
                                                        [width, 28.0],
                                                        egui::Button::new(text).selected(selected),
                                                    )
                                                    .on_hover_text(&candidate.candidate.id)
                                                    .clicked();
                                            }
                                        });
                                    });
                                    if clicked {
                                        self.map.selected_candidate =
                                            Some(candidate.candidate.id.clone());
                                        self.map.receiver = Some([
                                            candidate.candidate.observer.latitude_deg,
                                            candidate.candidate.observer.longitude_deg,
                                        ]);
                                    }
                                }
                            });
                    });
            });
    }

    pub(super) fn regional_pin(&mut self, latitude: f64, longitude: f64) {
        self.map.receiver = Some([latitude, longitude]);
        self.map.center_lat = latitude;
        self.map.center_lon = longitude;
    }

    pub(super) fn regional_inspect_source(&mut self, kind: EventKind, id: &str) {
        match kind {
            EventKind::Doppler => self.map.selected_track = Some(id.into()),
            EventKind::Thunder => self.map.selected_strike = Some(id.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evidence_score::{
        CandidateEvidence, CandidateScore, EvidenceTerm, ObserverLocation, TermContribution,
    };

    fn candidate(id: &str, rank: Option<usize>, score: Option<f64>) -> CandidateScore {
        CandidateScore {
            candidate: CandidateEvidence {
                id: id.into(),
                observer: ObserverLocation {
                    latitude_deg: 0.0,
                    longitude_deg: 0.0,
                    ellipsoid_height_m: 100.0,
                },
                recording_start_utc_s: 0.0,
                frequency: vec![],
                doppler_timing: vec![],
            },
            rank,
            score,
            terms: vec![],
            unavailable_reasons: vec![],
            location_verified: false,
        }
    }

    fn ids(rows: &[CandidateScore], column: ResultColumn, descending: bool) -> Vec<&str> {
        sorted_results(rows, column, descending)
            .into_iter()
            .map(|row| row.candidate.id.as_str())
            .collect()
    }

    #[test]
    fn rank_and_score_sorts_keep_missing_last_in_both_directions() {
        let rows = vec![
            candidate("missing", None, None),
            candidate("third", Some(3), Some(4.0)),
            candidate("first", Some(1), Some(0.0)),
            candidate("second", Some(2), Some(0.75)),
        ];
        for column in [ResultColumn::Rank, ResultColumn::Score] {
            assert_eq!(
                ids(&rows, column, false),
                ["first", "second", "third", "missing"]
            );
            assert_eq!(
                ids(&rows, column, true),
                ["third", "second", "first", "missing"]
            );
        }
        // Sorting is a view operation; the evidence and rank assignments stay intact.
        assert_eq!(rows[0].candidate.id, "missing");
        assert_eq!(rows[1].rank, Some(3));
    }

    #[test]
    fn geographic_sorts_use_numeric_coordinates_and_leave_nonfinite_last() {
        let mut rows = vec![
            candidate("east", None, None),
            candidate("west", None, None),
            candidate("unknown", None, None),
        ];
        rows[0].candidate.observer.latitude_deg = -1.0;
        rows[0].candidate.observer.longitude_deg = 100.0;
        rows[1].candidate.observer.latitude_deg = -10.0;
        rows[1].candidate.observer.longitude_deg = -120.0;
        rows[2].candidate.observer.latitude_deg = f64::NAN;
        rows[2].candidate.observer.longitude_deg = f64::INFINITY;
        for column in [ResultColumn::Latitude, ResultColumn::Longitude] {
            assert_eq!(ids(&rows, column, false), ["west", "east", "unknown"]);
            assert_eq!(ids(&rows, column, true), ["east", "west", "unknown"]);
        }
    }

    #[test]
    fn every_contribution_sort_excludes_unavailable_terms_and_orders_ties_by_id() {
        for (column, term) in [
            (ResultColumn::Frequency, EvidenceTerm::DopplerFrequency),
            (ResultColumn::DopplerTime, EvidenceTerm::DopplerTiming),
            (ResultColumn::ThunderTime, EvidenceTerm::ThunderTiming),
        ] {
            let mut rows = vec![
                candidate("missing", None, None),
                candidate("small-b", None, None),
                candidate("large", None, None),
                candidate("small-a", None, None),
                candidate("invalid", None, None),
            ];
            for (i, row) in rows[1..].iter_mut().enumerate() {
                row.terms.push(TermContribution {
                    term,
                    weight: 1.0,
                    residual_scale: 1.0,
                    available: i != 3,
                    unavailable_reason: None,
                    residuals: vec![],
                    event_mean_squared_residuals: Default::default(),
                    mean_normalized_squared_residual: None,
                    weighted_contribution: Some(if i == 1 { 10.0 } else { 0.0 }),
                });
            }
            assert_eq!(
                ids(&rows, column, false),
                ["small-a", "small-b", "large", "invalid", "missing"]
            );
            assert_eq!(
                ids(&rows, column, true),
                ["large", "small-a", "small-b", "invalid", "missing"]
            );
        }
    }

    #[test]
    fn nonfinite_score_is_missing_and_numeric_ties_remain_deterministic() {
        let rows = vec![
            candidate("z", None, Some(2.0)),
            candidate("a", None, Some(2.0)),
            candidate("nan", None, Some(f64::NAN)),
            candidate("inf", None, Some(f64::INFINITY)),
            candidate("absent", None, None),
        ];
        for descending in [false, true] {
            assert_eq!(
                ids(&rows, ResultColumn::Score, descending),
                ["a", "z", "absent", "inf", "nan"]
            );
        }
    }

    #[test]
    fn full_grid_remains_available_in_sorted_view() {
        let rows: Vec<_> = (1..=10_000)
            .map(|rank| {
                candidate(
                    &format!("receiver-{rank:05}"),
                    Some(rank),
                    Some(rank as f64),
                )
            })
            .collect();
        let sorted = sorted_results(&rows, ResultColumn::Score, true);
        assert_eq!(sorted.len(), 10_000);
        assert_eq!(sorted[0].rank, Some(10_000));
        assert_eq!(sorted[9_999].rank, Some(1));
    }

    #[test]
    fn preset_label_follows_exact_spacing_and_keeps_custom_values() {
        for (label, spacing) in GRID_PRESETS {
            assert_eq!(grid_preset_label(spacing), label);
        }
        for spacing in [1000.0, 250.0, 100.01, 24.99] {
            assert_eq!(grid_preset_label(spacing), "Custom");
        }
        let selection = workspace::MapSelection {
            grid_spacing_m: 25.0,
            ..Default::default()
        };
        let error = geosonnar::workspace_scoring::grid_observers(&selection).unwrap_err();
        assert!(error.contains("10000"));
        assert_eq!(
            selection.grid_spacing_m, 25.0,
            "An oversized grid must not silently change the selected spacing"
        );
    }
}
