//! Explicit planning and review for uncertain regional hypotheses.
use super::*;
use geosonnar::regional_search::{
    self, EventAlternatives, ModalityEstimate, ModalityPlan, ModalityProgress, ModalityReport,
    RegionalPlan, RegionalReport, SearchEstimate, SearchMode, SearchProgress, SearchStatus,
};

#[derive(PartialEq)]
struct SourceCacheKey {
    kind: EventKind,
    layers: Vec<(String, Option<u64>, usize)>,
    lightning: Option<(String, usize)>,
}

struct SourceChoice {
    id: String,
    folded_id: String,
    point: Option<[f64; 2]>,
    time: Option<[f64; 2]>,
}

#[derive(Default)]
struct SourceCache {
    key: Option<SourceCacheKey>,
    rows: Vec<SourceChoice>,
    ids: std::collections::HashSet<String>,
    filter: Option<(String, Option<[f64; 2]>)>,
    visible: Vec<usize>,
}

impl SourceCache {
    fn refresh(
        &mut self,
        layers: &[map_data::MapData],
        lightning: &Option<evidence_score::LightningCatalog>,
        kind: EventKind,
    ) {
        let key = SourceCacheKey {
            kind,
            layers: layers
                .iter()
                .map(|layer| {
                    (
                        layer.source_sha256.clone(),
                        layer.time_origin_utc_s.map(f64::to_bits),
                        layer.tracks.len(),
                    )
                })
                .collect(),
            lightning: lightning
                .as_ref()
                .map(|catalog| (catalog.input_sha256.clone(), catalog.strikes.len())),
        };
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.rows = match kind {
            EventKind::Doppler => layers
                .iter()
                .flat_map(|layer| &layer.tracks)
                .map(|track| SourceChoice {
                    id: track.id.clone(),
                    folded_id: track.id.to_lowercase(),
                    point: None,
                    time: track
                        .points
                        .first()
                        .zip(track.points.last())
                        .and_then(|(a, b)| a.time_utc_s.zip(b.time_utc_s))
                        .map(|(a, b)| [a, b]),
                })
                .collect(),
            EventKind::Thunder => lightning
                .iter()
                .flat_map(|catalog| &catalog.strikes)
                .map(|strike| SourceChoice {
                    id: strike.id.clone(),
                    folded_id: strike.id.to_lowercase(),
                    point: Some([strike.latitude_deg, strike.longitude_deg]),
                    time: Some([strike.time_utc_s; 2]),
                })
                .collect(),
        };
        self.ids = self.rows.iter().map(|row| row.id.clone()).collect();
        self.key = Some(key);
        self.filter = None;
    }

    fn filter(&mut self, text: String, time: Option<[f64; 2]>) {
        let key = (text, time);
        if self.filter.as_ref() == Some(&key) {
            return;
        }
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                (key.0.is_empty() || row.folded_id.contains(&key.0))
                    && match (row.time, key.1) {
                        (Some([first, last]), Some([earliest, latest])) => {
                            last >= earliest && first <= latest
                        }
                        _ => true,
                    }
            })
            .map(|(index, _)| index)
            .collect();
        self.filter = Some(key);
    }
}

pub(super) struct RegionalState {
    pub guided: bool,
    pub show_results: bool,
    step: usize,
    event_id: Option<String>,
    filter: String,
    time_filter: bool,
    travel_filter_s: f64,
    sources: SourceCache,
    pub estimate: Option<SearchEstimate>,
    pub comparison_estimate: Option<ModalityEstimate>,
    estimate_document: Option<ReviewWorkspace>,
    pub report: Option<RegionalReport>,
    pub comparison: Option<ModalityReport>,
    pub mode: SearchMode,
    pub report_document: Option<ReviewWorkspace>,
    pub detail: Option<(String, evidence_score::ScoreReport)>,
    selected_hypothesis: Option<String>,
    selected_location: Option<usize>,
    locations_view: bool,
    modality_delta: f64,
    sort: usize,
    descending: bool,
    refinement_km: f64,
    refinement_seconds: f64,
    refinement_spacing_m: f64,
    refinement_time_step_s: f64,
    pub running: bool,
    pub run_id: u64,
    pub progress: Option<SearchProgress>,
    pub comparison_progress: Option<ModalityProgress>,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Clone)]
enum ReviewedChoice {
    Hypothesis(String),
    Location(usize),
}

fn apply_choice(
    document: &ReviewWorkspace,
    report: &RegionalReport,
    choice: &ReviewedChoice,
) -> Result<ReviewWorkspace, String> {
    match choice {
        ReviewedChoice::Hypothesis(id) => regional_search::apply_hypothesis(document, report, id),
        ReviewedChoice::Location(index) => {
            regional_search::apply_location_hypothesis(document, report, *index)
        }
    }
}

impl Default for RegionalState {
    fn default() -> Self {
        Self {
            guided: true,
            show_results: false,
            step: 0,
            event_id: None,
            filter: String::new(),
            time_filter: false,
            travel_filter_s: 600.0,
            sources: SourceCache::default(),
            estimate: None,
            comparison_estimate: None,
            estimate_document: None,
            report: None,
            comparison: None,
            mode: SearchMode::Combined,
            report_document: None,
            detail: None,
            selected_hypothesis: None,
            selected_location: None,
            locations_view: true,
            modality_delta: 1.0,
            sort: 0,
            descending: false,
            refinement_km: 1.0,
            refinement_seconds: 10.0,
            refinement_spacing_m: 100.0,
            refinement_time_step_s: 0.5,
            running: false,
            run_id: 0,
            progress: None,
            comparison_progress: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl RegionalState {
    pub(super) fn accept_saved_report(
        &mut self,
        report: RegionalReport,
        document: ReviewWorkspace,
    ) {
        self.progress = Some(report.progress.clone());
        self.report = Some(report);
        self.report_document = Some(document);
        self.clear_selection();
        self.show_results = true;
        self.running = false;
    }

    pub fn select_hypothesis(&mut self, id: &str) {
        self.selected_hypothesis = Some(id.into());
        self.selected_location = None;
        self.show_results = true;
    }

    pub fn select_location(&mut self, index: usize) {
        self.selected_location = Some(index);
        self.selected_hypothesis = None;
        self.show_results = true;
    }

    pub(super) fn selection_key(&self) -> Option<String> {
        let key = self.selected_hypothesis.clone().or_else(|| {
            self.selected_location
                .map(|index| format!("location:{index}"))
        })?;
        Some(if self.comparison.is_some() {
            format!("{:?}:{key}", self.mode)
        } else {
            key
        })
    }

    fn choice(&self) -> Option<ReviewedChoice> {
        self.selected_hypothesis
            .clone()
            .map(ReviewedChoice::Hypothesis)
            .or_else(|| self.selected_location.map(ReviewedChoice::Location))
    }

    fn selected_receiver(&self) -> Option<(evidence_score::ObserverLocation, f64)> {
        let report = self.report.as_ref()?;
        if let Some(id) = &self.selected_hypothesis {
            let row = report.hypotheses.iter().find(|row| &row.id == id)?;
            Some((row.observer.clone(), row.clock_offset_s))
        } else {
            let row = report
                .location_summaries
                .iter()
                .find(|row| Some(row.observer_index) == self.selected_location)?;
            Some((row.observer.clone(), row.best_clock_offset_s?))
        }
    }

    fn choose_mode(&mut self, mode: SearchMode) {
        self.mode = mode;
        self.report = self
            .comparison
            .as_ref()
            .and_then(|comparison| comparison.modes.iter().find(|entry| entry.mode == mode))
            .and_then(|entry| entry.report.clone());
        self.clear_selection();
    }

    pub fn accept_comparison(&mut self, report: ModalityReport, document: ReviewWorkspace) {
        self.comparison_progress = Some(report.progress.clone());
        let mode = report
            .modes
            .iter()
            .find(|entry| entry.mode == SearchMode::Combined && entry.report.is_some())
            .or_else(|| report.modes.iter().find(|entry| entry.report.is_some()))
            .map_or(SearchMode::Combined, |entry| entry.mode);
        self.comparison = Some(report);
        self.report_document = Some(document);
        self.choose_mode(mode);
        self.show_results = true;
        self.running = false;
    }

    pub fn clear_selection(&mut self) {
        self.selected_hypothesis = None;
        self.selected_location = None;
        self.detail = None;
    }

    pub fn reset_results(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.estimate = None;
        self.comparison_estimate = None;
        self.estimate_document = None;
        self.report = None;
        self.comparison = None;
        self.report_document = None;
        self.detail = None;
        self.progress = None;
        self.comparison_progress = None;
        self.running = false;
        self.selected_hypothesis = None;
        self.selected_location = None;
    }

    pub fn accept_estimate(
        &mut self,
        estimate: SearchEstimate,
        document: ReviewWorkspace,
        current: Option<&ReviewWorkspace>,
    ) {
        if current == Some(&document) {
            self.estimate = Some(estimate);
            self.comparison_estimate = None;
            self.estimate_document = Some(document);
        }
    }

    pub fn accept_comparison_estimate(
        &mut self,
        estimate: ModalityEstimate,
        document: ReviewWorkspace,
        current: Option<&ReviewWorkspace>,
    ) {
        if current == Some(&document) {
            self.comparison_estimate = Some(estimate);
            self.estimate = None;
            self.estimate_document = Some(document);
        }
    }
}

pub(super) fn mode_label(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::DopplerOnly => "Doppler only",
        SearchMode::ThunderOnly => "Thunder only",
        SearchMode::Combined => "Combined",
    }
}

fn active_event(event: &EvidenceEvent, mix: &workspace::EvidenceMix) -> bool {
    event.review == ReviewState::Confirmed
        && event.selected_for_scoring
        && match event.kind {
            EventKind::Doppler => mix.doppler_frequency || mix.doppler_timing,
            EventKind::Thunder => mix.thunder_timing,
        }
}

pub(super) fn utc_label(seconds: f64) -> String {
    if !seconds.is_finite() || !(0.0..253_402_300_800.0).contains(&seconds) {
        return "UTC unavailable".into();
    }
    let whole = seconds.floor() as u64;
    let mut days = whole / 86_400;
    let mut year = 1970u64;
    let leap = |year: u64| {
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
    };
    loop {
        let count = if leap(year) { 366 } else { 365 };
        if days < count {
            break;
        }
        days -= count;
        year += 1;
    }
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0;
    while days >= months[month] {
        days -= months[month];
        month += 1;
    }
    let time = whole % 86_400;
    let milliseconds = ((seconds - seconds.floor()) * 1000.0)
        .round()
        .clamp(0.0, 999.0) as u64;
    format!(
        "{year:04}-{:02}-{:02} {:02}:{:02}:{:02}.{milliseconds:03} UTC",
        month + 1,
        days + 1,
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

/// Refreshing the inventory never includes a source or confirms an observation.
fn refresh_event_rows(document: &mut ReviewWorkspace) {
    let plan = document
        .regional_plan
        .get_or_insert_with(RegionalPlan::default);
    plan.events.retain(|row| {
        document
            .events
            .iter()
            .any(|event| event.id == row.event_id && active_event(event, &document.mix))
    });
    for event in document
        .events
        .iter()
        .filter(|event| active_event(event, &document.mix))
    {
        if plan.events.iter().any(|row| row.event_id == event.id) {
            continue;
        }
        let tone = if event.kind == EventKind::Doppler && document.mix.doppler_frequency {
            if event.emitted_frequency_hz > 0.0 {
                Some([event.emitted_frequency_hz, event.emitted_frequency_hz])
            } else {
                let mut values = event
                    .points
                    .iter()
                    .map(|point| point.frequency_hz)
                    .filter(|n| n.is_finite() && *n > 0.0);
                values.next().map(|first| {
                    let (minimum, maximum) =
                        values.fold((first, first), |(lo, hi), n| (lo.min(n), hi.max(n)));
                    [minimum * 0.9, maximum * 1.1]
                })
            }
        } else {
            None
        };
        plan.events.push(EventAlternatives {
            event_id: event.id.clone(),
            source_ids: vec![],
            emitted_frequency_hz: tone,
        });
    }
}

fn refinement_document(
    document: &ReviewWorkspace,
    observer: &evidence_score::ObserverLocation,
    clock_offset_s: f64,
    radius_km: f64,
    half_time_s: f64,
    spacing_m: f64,
    time_step_s: f64,
) -> Result<ReviewWorkspace, String> {
    if ![radius_km, half_time_s, spacing_m, time_step_s]
        .iter()
        .all(|value| value.is_finite())
        || !(0.05..=100.0).contains(&radius_km)
        || !(0.0..=86400.0).contains(&half_time_s)
        || !(10.0..=100000.0).contains(&spacing_m)
        || !(0.001..=86400.0).contains(&time_step_s)
    {
        return Err("Refinement needs a 0.05..100 km half-width, 0..86400 s time half-width, at least 10 m spacing and a positive time step".into());
    }
    let latitude = observer.latitude_deg;
    let longitude = observer.longitude_deg;
    let dlat = radius_km * 1000.0 / 111_320.0;
    let dlon = dlat / latitude.to_radians().cos();
    let bounds = [
        latitude - dlat,
        longitude - dlon,
        latitude + dlat,
        longitude + dlon,
    ];
    if bounds[0] < -85.0 || bounds[2] > 85.0 || bounds[1] < -180.0 || bounds[3] > 180.0 {
        return Err("Refinement crosses supported latitude or date-line limits; enter a supported rectangle manually".into());
    }
    let mut next = document.clone();
    let plan = next
        .regional_plan
        .as_mut()
        .ok_or("No regional plan to refine")?;
    next.map.bounds = bounds;
    next.map.grid_spacing_m = spacing_m;
    plan.clock.min_offset_s = clock_offset_s - half_time_s;
    plan.clock.max_offset_s = clock_offset_s + half_time_s;
    plan.clock.step_s = time_step_s;
    next.validate()?;
    Ok(next)
}

impl Studio {
    pub(super) fn regional_controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("Regional investigation");
        ui.add_enabled_ui(!self.busy(), |ui| self.joint_method_ui(ui));
        ui.horizontal_wrapped(|ui| {
            for (step, label) in ["1 Area & time", "2 Evidence", "3 Run"]
                .into_iter()
                .enumerate()
            {
                ui.selectable_value(&mut self.regional.step, step, label);
            }
        });
        ui.separator();
        if self
            .document
            .as_ref()
            .is_some_and(|doc| doc.regional_plan.is_none())
        {
            muted(
                ui,
                "Build a plan for uncertain time and alternative sources. Your confirmed observations stay unchanged.",
            );
            if ui
                .add_enabled(!self.busy(), egui::Button::new("Set up regional search"))
                .clicked()
            {
                let doc = self.document.as_mut().unwrap();
                let mut plan = RegionalPlan::default();
                plan.clock.min_offset_s = doc.map.clock_adjustment_s - 30.0;
                plan.clock.max_offset_s = doc.map.clock_adjustment_s + 30.0;
                doc.regional_plan = Some(plan);
                refresh_event_rows(doc);
            }
            return;
        }
        ui.add_enabled_ui(!self.busy(), |ui| match self.regional.step {
            0 => self.regional_area_time_ui(ui),
            1 => self.regional_evidence_ui(ui),
            _ if self.joint.enabled => self.joint_run_ui(ui),
            _ => self.regional_run_ui(ui),
        });
    }

    fn regional_area_time_ui(&mut self, ui: &mut egui::Ui) {
        let doc = self.document.as_mut().unwrap();
        ui.strong("Search rectangle");
        muted(ui, "Draw the area on the map or enter its edges.");
        egui::Grid::new("regional_bounds")
            .num_columns(2)
            .show(ui, |ui| {
                for (label, value) in ["South", "West", "North", "East"]
                    .iter()
                    .zip(&mut doc.map.bounds)
                {
                    ui.label(*label);
                    ui.add(egui::DragValue::new(value).speed(0.001).max_decimals(6));
                    ui.end_row();
                }
                ui.label("Grid spacing");
                egui::ComboBox::from_id_salt("regional_spacing")
                    .selected_text(format!("{} m", doc.map.grid_spacing_m))
                    .show_ui(ui, |ui| {
                        for spacing in [10_000.0, 2_000.0, 500.0, 100.0, 25.0] {
                            ui.selectable_value(
                                &mut doc.map.grid_spacing_m,
                                spacing,
                                format!("{spacing} m"),
                            );
                        }
                    });
                ui.end_row();
                ui.label("Custom spacing");
                ui.add(
                    egui::DragValue::new(&mut doc.map.grid_spacing_m)
                        .range(10.0..=100_000.0)
                        .suffix(" m"),
                );
                ui.end_row();
            });
        ui.separator();
        ui.strong("Approximate recording start — UTC");
        accessible_text(
            ui,
            "regional_recording_start",
            &mut doc.recording_start_utc,
            "YYYY-MM-DD HH:MM:SS",
            false,
            ui.available_width(),
        );
        let plan = doc.regional_plan.as_mut().unwrap();
        muted(
            ui,
            "Search these offsets around the entered start. One shared clock applies to every event. These values replace the fixed comparison's clock adjustment.",
        );
        egui::Grid::new("regional_clock")
            .num_columns(2)
            .show(ui, |ui| {
                for (label, value) in [
                    ("Earliest offset", &mut plan.clock.min_offset_s),
                    ("Latest offset", &mut plan.clock.max_offset_s),
                    ("Time step", &mut plan.clock.step_s),
                ] {
                    ui.label(label);
                    ui.add(egui::DragValue::new(value).speed(0.5).suffix(" s"));
                    ui.end_row();
                }
            });
        ui.horizontal_wrapped(|ui| {
            for (label, half, step) in [
                ("±30 s", 30.0, 1.0),
                ("±5 min", 300.0, 5.0),
                ("±1 hour", 3600.0, 30.0),
            ] {
                if ui.button(label).clicked() {
                    plan.clock.min_offset_s = -half;
                    plan.clock.max_offset_s = half;
                    plan.clock.step_s = step;
                }
            }
        });
        muted(
            ui,
            "A larger time range needs more work. A coarse time step can miss a match; refine a retained interval explicitly.",
        );
        egui::CollapsingHeader::new("Height, sound speed and strike assumptions").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Receiver ellipsoid height");
                ui.add(egui::DragValue::new(&mut doc.map.receiver_height_m).range(-500.0..=9_000.0).suffix(" m"));
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Fixed sound speed");
                ui.add_enabled(doc.atmosphere.is_none(), egui::DragValue::new(&mut doc.sound_speed_mps).range(250.0..=400.0).suffix(" m/s"));
            });
            let mut fallback = doc.strike_fallback_height_m.is_some();
            if ui.checkbox(&mut fallback, "Declare fallback strike ellipsoid height").changed() { doc.strike_fallback_height_m = fallback.then_some(0.0); }
            if let Some(height) = &mut doc.strike_fallback_height_m { ui.add(egui::DragValue::new(height).range(-1000.0..=30000.0).suffix(" m")); }
            if let Some(profile) = &doc.atmosphere { ui.label(format!("Atmosphere: {}", profile.name)); }
            muted(ui, "Manage atmospheric profiles in Data. Receiver and source height assumptions remain part of the saved evidence.");
        });
        self.height_model_ui(ui);
        if ui.button("Next: review evidence →").clicked() {
            self.regional.step = 1;
        }
    }

    fn regional_evidence_ui(&mut self, ui: &mut egui::Ui) {
        let mut inspect = None;
        let doc = self.document.as_mut().unwrap();
        ui.strong("Choose scoring terms");
        ui.checkbox(&mut doc.mix.doppler_frequency, "Doppler frequency");
        ui.checkbox(&mut doc.mix.doppler_timing, "Doppler timing");
        ui.checkbox(&mut doc.mix.thunder_timing, "Thunder arrival timing");
        egui::CollapsingHeader::new("Weights and residual scales").show(ui, |ui| {
            egui::Grid::new("regional_term_weights")
                .num_columns(3)
                .show(ui, |ui| {
                    ui.label("Term");
                    ui.label("Weight");
                    ui.label("Scale");
                    ui.end_row();
                    for (name, weight, scale, unit) in [
                        (
                            "Frequency",
                            &mut doc.mix.frequency_weight,
                            &mut doc.mix.frequency_scale_hz,
                            " Hz",
                        ),
                        (
                            "Doppler time",
                            &mut doc.mix.doppler_timing_weight,
                            &mut doc.mix.doppler_timing_scale_s,
                            " s",
                        ),
                        (
                            "Thunder",
                            &mut doc.mix.thunder_weight,
                            &mut doc.mix.thunder_scale_s,
                            " s",
                        ),
                    ] {
                        ui.label(name);
                        ui.add(
                            egui::DragValue::new(weight)
                                .range(0.001..=1000.0)
                                .speed(0.1),
                        );
                        ui.add(
                            egui::DragValue::new(scale)
                                .range(0.001..=1000.0)
                                .speed(0.1)
                                .suffix(unit),
                        );
                        ui.end_row();
                    }
                });
        });
        muted(
            ui,
            "Confirm measurements and assess the sound source in Audio review, then explicitly include events here. Changing terms or selected events requires refreshing the plan list.",
        );
        for event in &mut doc.events {
            ui.add_enabled(
                event.review == ReviewState::Confirmed,
                egui::Checkbox::new(
                    &mut event.selected_for_scoring,
                    format!("{} · {:.1}s", event.label, event.arrival_s),
                ),
            );
        }
        let readiness = super::acoustic_review::readiness_ui(ui, doc, true);
        if let Some(index) = readiness.review_event {
            self.open_source_review(index);
            return;
        }
        if ui.button("Refresh event list").clicked() {
            refresh_event_rows(doc);
        }
        let plan = doc.regional_plan.as_mut().unwrap();
        if plan.events.is_empty() {
            muted(
                ui,
                "No planned events. Select confirmed events and terms, then refresh.",
            );
            return;
        }
        if self
            .regional
            .event_id
            .as_ref()
            .is_none_or(|id| !plan.events.iter().any(|row| &row.event_id == id))
        {
            self.regional.event_id = plan.events.first().map(|row| row.event_id.clone());
        }
        egui::ComboBox::from_id_salt("regional_event")
            .selected_text(self.regional.event_id.as_deref().unwrap_or("Event"))
            .show_ui(ui, |ui| {
                for row in &plan.events {
                    let label = doc
                        .events
                        .iter()
                        .find(|event| event.id == row.event_id)
                        .map_or(row.event_id.as_str(), |event| event.label.as_str());
                    ui.selectable_value(
                        &mut self.regional.event_id,
                        Some(row.event_id.clone()),
                        label,
                    );
                }
            });
        let Some(row) = plan
            .events
            .iter_mut()
            .find(|row| Some(&row.event_id) == self.regional.event_id.as_ref())
        else {
            return;
        };
        let Some(event) = doc.events.iter().find(|event| event.id == row.event_id) else {
            return;
        };
        if event.kind == EventKind::Doppler && doc.mix.doppler_frequency {
            let bounds = row.emitted_frequency_hz.get_or_insert([100.0, 200.0]);
            ui.label("Allowed emitted tone range");
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::DragValue::new(&mut bounds[0])
                        .range(0.001..=4000.0)
                        .suffix(" Hz"),
                );
                ui.label("to");
                ui.add(
                    egui::DragValue::new(&mut bounds[1])
                        .range(0.001..=4000.0)
                        .suffix(" Hz"),
                );
            });
            muted(
                ui,
                "Initial bounds use the fixed tone, or the observed ridge with 10% padding. Review this assumption. The search fits one constant tone per event within these bounds.",
            );
        } else {
            row.emitted_frequency_hz = None;
        }
        self.regional
            .sources
            .refresh(&self.layers, &self.lightning, event.kind);
        ui.separator();
        ui.strong("Allow source alternatives");
        muted(
            ui,
            "Only checked sources enter the search. Inspect highlights a source on the map. Inclusion means a hypothesis to test, not an identified source.",
        );
        accessible_text(
            ui,
            "regional_source_filter",
            &mut self.regional.filter,
            "Filter source ID",
            false,
            ui.available_width(),
        );
        let origin = workspace::parse_utc(&doc.recording_start_utc).ok();
        ui.add_enabled(
            origin.is_some(),
            egui::Checkbox::new(
                &mut self.regional.time_filter,
                "List only possible event times",
            ),
        );
        if self.regional.time_filter {
            ui.horizontal_wrapped(|ui| {
                ui.label("Travel look-back");
                ui.add(
                    egui::DragValue::new(&mut self.regional.travel_filter_s)
                        .range(0.0..=86400.0)
                        .suffix(" s"),
                );
            });
            muted(
                ui,
                "Listing filter only. Expand the look-back or show all if a source is missing. Existing approved choices remain included.",
            );
        }
        let filter = self.regional.filter.to_lowercase();
        let possible = origin.map(|origin| {
            [
                origin + plan.clock.min_offset_s + event.start_s - self.regional.travel_filter_s,
                origin + plan.clock.max_offset_s + event.end_s,
            ]
        });
        self.regional.sources.filter(
            filter,
            if self.regional.time_filter {
                possible
            } else {
                None
            },
        );
        let sources = &self.regional.sources;
        let visible = &sources.visible;
        ui.label(format!(
            "{} approved · {} shown / {} loaded",
            row.source_ids.len(),
            visible.len(),
            sources.rows.len()
        ));
        ui.horizontal_wrapped(|ui| {
            let union_count = if visible.len() > regional_search::MAX_SOURCES_PER_EVENT {
                usize::MAX
            } else {
                row.source_ids.len()
                    + visible
                        .iter()
                        .filter(|index| !row.source_ids.contains(&sources.rows[**index].id))
                        .count()
            };
            if ui
                .add_enabled(
                    union_count <= regional_search::MAX_SOURCES_PER_EVENT,
                    egui::Button::new("Include shown sources"),
                )
                .clicked()
            {
                for index in visible {
                    let id = &sources.rows[*index].id;
                    if !row.source_ids.contains(id) {
                        row.source_ids.push(id.clone());
                    }
                }
                row.source_ids.sort();
            }
            if ui.button("Clear sources").clicked() {
                row.source_ids.clear();
            }
        });
        if visible.len() > regional_search::MAX_SOURCES_PER_EVENT {
            muted(
                ui,
                format!(
                    "At most {} alternatives per event. Narrow the list or select individual sources.",
                    regional_search::MAX_SOURCES_PER_EVENT
                ),
            );
        }
        let missing: Vec<_> = row
            .source_ids
            .iter()
            .filter(|id| !sources.ids.contains(*id))
            .cloned()
            .collect();
        if !missing.is_empty() {
            ui.colored_label(
                tone(ui, AMBER),
                format!(
                    "Not loaded: {}. Restore data or remove these choices.",
                    missing.join(", ")
                ),
            );
        }
        egui::ScrollArea::vertical()
            .id_salt("regional_source_rows")
            .max_height(260.0)
            .show_rows(ui, 50.0, visible.len(), |ui, range| {
                for index in range {
                    let source = &sources.rows[visible[index]];
                    let (id, point, time) = (&source.id, &source.point, &source.time);
                    ui.push_id(id, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            let mut included = row.source_ids.contains(id);
                            if ui
                                .add_enabled(
                                    included
                                        || row.source_ids.len()
                                            < regional_search::MAX_SOURCES_PER_EVENT,
                                    egui::Checkbox::new(&mut included, id),
                                )
                                .changed()
                            {
                                if included {
                                    row.source_ids.push(id.clone());
                                    row.source_ids.sort();
                                } else {
                                    row.source_ids.retain(|source| source != id);
                                }
                            }
                            if ui.small_button("Inspect").clicked() {
                                inspect = Some((event.kind, id.clone()));
                            }
                        });
                        match (point, time) {
                            (Some([latitude, longitude]), Some([start, _])) => {
                                ui.small(format!(
                                    "{} · {latitude:.4}, {longitude:.4}",
                                    utc_label(*start)
                                ));
                            }
                            (_, Some([start, end])) => {
                                ui.small(format!("{} → {}", utc_label(*start), utc_label(*end)));
                            }
                            _ => {
                                ui.small("UTC coverage needs interpretation");
                            }
                        }
                    });
                }
            });
        if sources.rows.is_empty() {
            muted(
                ui,
                "Load aircraft or lightning data in Data, then return here.",
            );
        }
        if ui.button("Next: estimate & run →").clicked() {
            self.regional.step = 2;
        }
        if let Some((kind, id)) = inspect {
            self.regional_inspect_source(kind, &id);
        }
    }

    fn regional_run_ui(&mut self, ui: &mut egui::Ui) {
        ui.strong("Review the work limit");
        let doc = self.document.as_mut().unwrap();
        let plan = doc.regional_plan.as_mut().unwrap();
        ui.horizontal_wrapped(|ui| {
            ui.label("Maximum evaluations");
            ui.add(
                egui::DragValue::new(&mut plan.max_evaluations)
                    .range(1..=regional_search::MAX_EVALUATIONS)
                    .speed(1000),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Retain best hypotheses");
            ui.add(
                egui::DragValue::new(&mut plan.retain_hypotheses)
                    .range(1..=regional_search::MAX_RETAINED),
            );
        });
        let mut compare = doc.regional_comparison.is_some();
        if ui
            .checkbox(&mut compare, "Compare independent evidence modes")
            .changed()
        {
            doc.regional_comparison =
                compare.then(geosonnar::regional_search::ComparisonOptions::default);
        }
        if let Some(options) = &mut doc.regional_comparison {
            muted(
                ui,
                "Each selected mode searches its own best clock and source associations on this same grid and approved source pool. Scores across modes are not probabilities.",
            );
            for mode in [
                SearchMode::DopplerOnly,
                SearchMode::ThunderOnly,
                SearchMode::Combined,
            ] {
                let mut selected = options.modes.contains(&mode);
                if ui.checkbox(&mut selected, mode_label(mode)).changed() {
                    if selected {
                        options.modes.push(mode);
                    } else {
                        options.modes.retain(|value| *value != mode);
                    }
                }
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Total evaluations across modes");
                ui.add(
                    egui::DragValue::new(&mut options.max_total_evaluations)
                        .range(1..=regional_search::MAX_EVALUATIONS * 3)
                        .speed(1000),
                );
            });
        }
        muted(
            ui,
            "The estimate checks the complete declared grid, shared time range, source alternatives and data support. Retaining fewer rows does not reduce the search work.",
        );
        let readiness = super::acoustic_review::readiness_ui(ui, doc, true);
        if let Some(index) = readiness.review_event {
            self.open_source_review(index);
            return;
        }
        if ui
            .add_enabled(
                !readiness.blocked,
                egui::Button::new("Estimate regional search"),
            )
            .clicked()
        {
            self.prepare_regional(ui.ctx(), false);
        }
        let current = self.regional.estimate_document.as_ref() == self.document.as_ref();
        if let Some(estimate) = &self.regional.estimate {
            ui.label(format!(
                "{} receivers × {} clock values",
                estimate.observer_count, estimate.clock_count
            ));
            ui.label(format!(
                "{} events · {} source alternatives",
                estimate.event_count, estimate.source_alternatives
            ));
            ui.label(format!(
                "{} evaluations · {} residual evaluations",
                estimate.evaluations, estimate.residual_evaluations
            ));
            if !current {
                ui.colored_label(
                    tone(ui, AMBER),
                    "Plan changed. Estimate again before running.",
                );
            }
        }
        if let Some(estimate) = &self.regional.comparison_estimate {
            ui.label(format!(
                "All requested modes: {} evaluations · {} residual evaluations",
                estimate.evaluations, estimate.residual_evaluations
            ));
            for mode in &estimate.modes {
                ui.label(format!(
                    "{}: {} receivers × {} clocks · {} alternatives · {} evaluations",
                    mode_label(mode.mode),
                    mode.estimate.observer_count,
                    mode.estimate.clock_count,
                    mode.estimate.source_alternatives,
                    mode.estimate.evaluations
                ));
            }
            if !current {
                ui.colored_label(
                    tone(ui, AMBER),
                    "Plan changed. Estimate again before running.",
                );
            }
        }
        if ui
            .add_enabled(
                current
                    && !readiness.blocked
                    && (self.regional.estimate.is_some()
                        || self.regional.comparison_estimate.is_some()),
                egui::Button::new("Run reviewed regional plan"),
            )
            .clicked()
        {
            self.prepare_regional(ui.ctx(), true);
        }
        muted(
            ui,
            "Nothing is applied automatically. Open Results after the run to inspect alternatives. A cancelled run remains incomplete.",
        );
    }

    /// One seam for the registered terrain-height provider.
    fn regional_observers(
        doc: &ReviewWorkspace,
    ) -> Result<Vec<evidence_score::ObserverLocation>, String> {
        geosonnar::receiver_heights::grid(doc)?.complete_observers()
    }

    pub(super) fn prepare_regional(&mut self, ctx: &egui::Context, run: bool) {
        if self.busy() {
            return;
        }
        if run && self.has_unsaved_results() {
            self.notice = Some((true, "The previous result has not been saved. Retry saving or explicitly discard it in Results before starting another run.".into()));
            self.pending_action = Some(Action::ReviewResults);
            return;
        }
        let Some(doc) = self.document.clone() else {
            return;
        };
        if let Some(reason) = super::acoustic_review::first_block(&doc, true) {
            self.notice = Some((true, reason));
            return;
        }
        let Some(plan) = doc.regional_plan.clone() else {
            return;
        };
        let prepared = self
            .cached_bindings_match(&doc)
            .and_then(|_| self.merged_map());
        let map = match prepared {
            Ok(map) => map,
            Err(error) => {
                self.notice = Some((true, error));
                return;
            }
        };
        let lightning = self.lightning.clone();
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        let comparison = doc
            .regional_comparison
            .as_ref()
            .map(|options| ModalityPlan {
                regional: plan.clone(),
                modes: options.modes.clone(),
                max_total_evaluations: options.max_total_evaluations,
            });
        if !run {
            self.regional.estimate = None;
            self.regional.comparison_estimate = None;
            self.regional.estimate_document = None;
            self.spawn(
                ctx,
                "Checking regional plan and work estimate…",
                move || {
                    workspace::verify(&manifest, &doc)?;
                    let observers = Self::regional_observers(&doc)?;
                    if let Some(request) = comparison {
                        let estimate = regional_search::estimate_modalities(
                            &doc, &map, &lightning, &observers, &request,
                        )?;
                        workspace::verify(&manifest, &doc)?;
                        return Ok(ResultValue::RegionalComparisonEstimated(
                            estimate,
                            Box::new(doc),
                        ));
                    }
                    let estimate =
                        regional_search::estimate(&doc, &map, &lightning, &observers, &plan)?;
                    workspace::verify(&manifest, &doc)?;
                    Ok(ResultValue::RegionalEstimated(estimate, Box::new(doc)))
                },
            );
            return;
        }
        if self.regional.estimate_document.as_ref() != Some(&doc) {
            self.notice = Some((true, "Estimate the current plan before running.".into()));
            return;
        }
        self.regional.cancel = Arc::new(AtomicBool::new(false));
        self.regional.run_id = self.regional.run_id.wrapping_add(1);
        self.regional.running = true;
        self.regional.progress = None;
        self.regional.comparison_progress = None;
        let cancel = self.regional.cancel.clone();
        let run_id = self.regional.run_id;
        let generation = self.generation;
        let sender = self.sender.clone();
        let repaint = ctx.clone();
        self.spawn(
            ctx,
            "Searching reviewed regional alternatives…",
            move || {
                use geosonnar::{investigation_archive::ReportKind, regional_history};
                let inputs_sha256 = regional_search::inputs_sha256(&map, &lightning)?;
                let journal = regional_history::begin(&manifest, &doc, &inputs_sha256);
                if let Err(error) = &journal {
                    let _ = sender.send(Reply { generation, lane: WorkerLane::SearchProgress,
                        result: Ok(ResultValue::RunJournalNotice(run_id, format!("Run history could not be started: {error}. This computation may finish unsaved; Results will provide retry/discard controls."))) });
                    repaint.request_repaint();
                }
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    workspace::verify(&manifest, &doc)?;
                    let observers = Self::regional_observers(&doc)?;
                    let mut last_update = Instant::now() - Duration::from_secs(1);
                    if let Some(request) = comparison {
                        let report = regional_search::run_modalities(
                            &doc, &map, &lightning, &observers, &request, &cancel,
                            |progress| {
                                if last_update.elapsed() >= Duration::from_millis(120)
                                    || progress.completed_evaluations == progress.total_evaluations {
                                    let _ = sender.send(Reply { generation, lane: WorkerLane::SearchProgress,
                                        result: Ok(ResultValue::RegionalComparisonProgress(run_id, progress)) });
                                    last_update = Instant::now(); repaint.request_repaint();
                                }
                            },
                        )?;
                        workspace::verify(&manifest, &doc)?;
                        return Ok(ReportKind::Comparison(report));
                    }
                    let report = regional_search::run(
                        &doc, &map, &lightning, &observers, &plan, &cancel,
                        |progress| {
                            if last_update.elapsed() >= Duration::from_millis(120)
                                || progress.completed_evaluations == progress.total_evaluations {
                                let _ = sender.send(Reply { generation, lane: WorkerLane::SearchProgress,
                                    result: Ok(ResultValue::RegionalProgress(run_id, progress)) });
                                last_update = Instant::now(); repaint.request_repaint();
                            }
                        },
                    )?;
                    workspace::verify(&manifest, &doc)?;
                    Ok(ReportKind::Regional(report))
                })).unwrap_or_else(|_| Err("Regional computation stopped unexpectedly".into()));
                let cancelled = cancel.load(Ordering::Relaxed);
                let persistence = journal.and_then(|handle| regional_history::finish(handle, &outcome, cancelled));
                Ok(ResultValue::RunRecorded(Box::new(history_runs::RunCompletion {
                    original_manifest: manifest.clone(), document: doc, inputs_sha256, outcome, cancelled, persistence, joint_request: None,
                })))
            },
        );
    }

    pub(super) fn regional_progress_ui(&mut self, ui: &mut egui::Ui) {
        self.joint_progress_ui(ui);
        if !self.regional.running {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(progress) = &self.regional.comparison_progress {
                let fraction = progress.completed_evaluations as f32
                    / progress.total_evaluations.max(1) as f32;
                ui.add(
                    egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                        .desired_width(180.0)
                        .text(format!(
                            "{} / {}",
                            progress.completed_evaluations, progress.total_evaluations
                        )),
                );
                ui.label(format!(
                    "{} · {} / {} modes finished",
                    mode_label(progress.mode),
                    progress.completed_modes,
                    progress.total_modes
                ));
            } else if let Some(progress) = &self.regional.progress {
                let fraction = progress.completed_evaluations as f32
                    / progress.total_evaluations.max(1) as f32;
                ui.add(
                    egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                        .desired_width(180.0)
                        .text(format!(
                            "{} / {}",
                            progress.completed_evaluations, progress.total_evaluations
                        )),
                );
                ui.label(format!(
                    "{} / {} location/time cells",
                    progress.completed_cells, progress.total_cells
                ));
            } else {
                ui.label("Preparing regional search…");
            }
            if ui.button("Cancel regional search").clicked() {
                self.regional.cancel.store(true, Ordering::Relaxed);
            }
            if self.regional.cancel.load(Ordering::Relaxed) {
                ui.label("Cancellation requested; retaining completed work as incomplete.");
            }
        });
    }

    fn regional_comparison_ui(&mut self, ui: &mut egui::Ui) {
        let Some(comparison) = &self.regional.comparison else {
            return;
        };
        let mut choose = None;
        let height = (ui.available_height() * 0.28).clamp(80.0, 190.0);
        egui::CollapsingHeader::new("Independent Doppler / thunder searches").default_open(true).show(ui,|ui| {
            egui::ScrollArea::vertical().id_salt("independent_modes").max_height(height).show(ui,|ui| {
                ui.strong(if comparison.status==SearchStatus::Completed {"All requested modes completed"} else {"Comparison incomplete — some modes are cancelled or not started"});
                for entry in &comparison.modes {
                    ui.horizontal_wrapped(|ui| {
                        if ui.selectable_label(self.regional.mode==entry.mode,mode_label(entry.mode)).clicked() {choose=Some(entry.mode);}
                        ui.label(match entry.status {regional_search::ModeStatus::Completed=>"Complete",regional_search::ModeStatus::Cancelled=>"Cancelled / partial",regional_search::ModeStatus::NotStarted=>"Not started"});
                        if let Some(best)=entry.report.as_ref().and_then(|report|report.hypotheses.first()) {
                            ui.label(format!("{:.6}, {:.6} · clock {:+.3}s",best.observer.latitude_deg,best.observer.longitude_deg,best.clock_offset_s));
                        } else if entry.report.is_some() {ui.label("No rankable hypothesis");}
                    });
                }
                let doppler=comparison.modes.iter().find(|entry|entry.mode==SearchMode::DopplerOnly).and_then(|entry|entry.report.as_ref());
                let thunder=comparison.modes.iter().find(|entry|entry.mode==SearchMode::ThunderOnly).and_then(|entry|entry.report.as_ref());
                if let (Some(doppler),Some(thunder))=(doppler,thunder)
                    && let (Some(d),Some(t))=(doppler.hypotheses.first(),thunder.hypotheses.first()) {
                    let lat1=d.observer.latitude_deg.to_radians();let lat2=t.observer.latitude_deg.to_radians();
                    let half_lon=((d.observer.longitude_deg-t.observer.longitude_deg).to_radians()/2.0).sin();
                    let a=((lat1-lat2)/2.0).sin().powi(2)+lat1.cos()*lat2.cos()*half_lon.powi(2);
                    let metres=2.0*6_371_000.0*a.clamp(0.0,1.0).sqrt().asin();
                    ui.label(format!("Doppler / thunder peaks: {:.0} m apart · clock difference {:.3}s",metres,(d.clock_offset_s-t.clock_offset_s).abs()));
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Spatial low-score tolerance");
                        ui.add(egui::DragValue::new(&mut self.regional.modality_delta).range(0.0..=100.0).speed(0.1));
                    });
                    let d_cells:std::collections::BTreeSet<_>=doppler.location_summaries.iter().filter(|row|row.best_score.is_some_and(|score|score<=d.score+self.regional.modality_delta)).map(|row|row.observer_index).collect();
                    let t_cells:std::collections::BTreeSet<_>=thunder.location_summaries.iter().filter(|row|row.best_score.is_some_and(|score|score<=t.score+self.regional.modality_delta)).map(|row|row.observer_index).collect();
                    ui.label(format!("{} Doppler cells · {} thunder cells · {} shared cells",d_cells.len(),t_cells.len(),d_cells.intersection(&t_cells).count()));
                    muted(ui,"Tolerance is best score + this amount, not confidence. Each mode fits its own clock and sources; spatial overlap does not establish a shared time. Combined search tests one shared clock.");
                }
                muted(ui,"Changing this display mode does not change your selected evidence. Applying a mode explicitly changes its scoring-term switches along with the selected associations and clock.");
            });
        });
        if let Some(mode) = choose {
            self.regional.choose_mode(mode);
        }
    }

    pub(super) fn regional_results_ui(&mut self, ui: &mut egui::Ui) -> bool {
        self.run_history_ui(ui);
        if self.has_joint_results() {
            if self.regional.report.is_some() || self.regional.comparison.is_some() {
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(
                        &mut self.joint.show_results,
                        true,
                        "Shared-cell comparison",
                    );
                    ui.selectable_value(
                        &mut self.joint.show_results,
                        false,
                        "Reviewed-event results",
                    );
                });
            }
            if self.joint.show_results {
                return self.joint_results_ui(ui);
            }
        }
        if self.regional.report.is_none() && self.regional.comparison.is_none() {
            return false;
        }
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.regional.show_results, true, "Regional hypotheses");
            ui.selectable_value(&mut self.regional.show_results, false, "Fixed comparison");
        });
        if !self.regional.show_results {
            return false;
        }
        let table_height = (ui.available_height() * 0.6).clamp(140.0, 650.0);
        egui::ScrollArea::vertical()
            .id_salt("regional_results_page")
            .auto_shrink([false, false])
            .show(ui, |ui| self.regional_results_contents(ui, table_height));
        true
    }

    fn regional_results_contents(&mut self, ui: &mut egui::Ui, table_height: f32) -> bool {
        let busy = self.busy();
        ui.heading("Regional location, time and source hypotheses");
        let current = self.regional.report_document.as_ref() == self.document.as_ref();
        self.investigation_readiness_ui(ui);
        let inputs_ready = self.report_inputs_ready();
        self.regional_comparison_ui(ui);
        let Some(report) = self.regional.report.as_ref() else {
            muted(
                ui,
                "This mode was not started or has no report. Its absence is not negative evidence.",
            );
            if ui
                .add_enabled(
                    current && !busy && inputs_ready,
                    egui::Button::new("Save investigation report + evidence"),
                )
                .clicked()
            {
                self.save_regional(ui.ctx());
            }
            return true;
        };
        let inspected_complete = report.status == SearchStatus::Completed;
        let complete = inspected_complete
            && self
                .regional
                .comparison
                .as_ref()
                .is_none_or(|comparison| comparison.status == SearchStatus::Completed);
        let has_selection = self.regional.selected_receiver().is_some();
        if let Some(doc) = &self.regional.report_document {
            super::acoustic_review::assumptions_ui(
                ui,
                "regional_source_assumptions",
                doc.events.iter().filter(|event| {
                    report
                        .plan
                        .events
                        .iter()
                        .any(|row| row.event_id == event.id)
                }),
            );
        }
        let detail_key = self.regional.selection_key();
        ui.label(if inspected_complete {
            "Completed declared search"
        } else {
            "Cancelled — incomplete search; omitted work may contain better alternatives"
        });
        ui.label(format!(
            "{} retained hypotheses · {} unavailable alternatives · {} / {} evaluations",
            report.hypotheses.len(),
            report.unavailable_count,
            report.progress.completed_evaluations,
            report.progress.total_evaluations
        ));
        ui.label(format!(
            "{} rankable location/time cells · {} unavailable cells",
            report.rankable_cells, report.unavailable_cells
        ));
        if report.retained_truncated {
            muted(
                ui,
                format!(
                    "Showing the best retained rows from {} supported hypotheses. Increase the retention limit and rerun to inspect more alternatives.",
                    report.supported_hypotheses.map_or_else(
                        || "more than the counter can represent".into(),
                        |count| count.to_string()
                    )
                ),
            );
        }
        muted(
            ui,
            "Scores are uncalibrated. Inspect competing sources and boundary flags; a ranked row does not verify a location. Click columns to sort.",
        );
        if !current {
            ui.colored_label(
                tone(ui, AMBER),
                "Evidence or plan changed since this run. Rerun before applying or saving.",
            );
        }
        let mut apply = false;
        let mut save = false;
        let mut inspect = false;
        let mut refine = false;
        ui.horizontal_wrapped(|ui| {
            save = ui
                .add_enabled(
                    current && !busy && inputs_ready,
                    egui::Button::new("Save investigation report + evidence"),
                )
                .clicked();
            apply = ui
                .add_enabled(
                    current && complete && !busy && has_selection && inputs_ready,
                    egui::Button::new(if self.regional.comparison.is_some() {
                        "Apply selected mode, associations & clock"
                    } else {
                        "Apply selected associations & clock"
                    }),
                )
                .clicked();
            inspect = ui
                .add_enabled(
                    current && inspected_complete && !busy && has_selection && inputs_ready,
                    egui::Button::new("Inspect fit in audio"),
                )
                .clicked();
        });
        egui::CollapsingHeader::new("Prepare a finer search around the selected hypothesis").show(ui,|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Area half-width"); ui.add(egui::DragValue::new(&mut self.regional.refinement_km).range(0.05..=100.0).speed(0.1).suffix(" km"));
                ui.label("Grid spacing"); ui.add(egui::DragValue::new(&mut self.regional.refinement_spacing_m).range(10.0..=100000.0).suffix(" m"));
                ui.label("Time half-width"); ui.add(egui::DragValue::new(&mut self.regional.refinement_seconds).range(0.0..=86400.0).suffix(" s"));
                ui.label("Time step"); ui.add(egui::DragValue::new(&mut self.regional.refinement_time_step_s).range(0.001..=86400.0).speed(0.1).suffix(" s"));
            });
            muted(ui,"Keeps every approved source alternative and tone range. This prepares an undoable plan only; inspect the new area/time range and estimate before running.");
            refine=ui.add_enabled(current && complete && !busy && has_selection,egui::Button::new("Prepare refinement plan")).clicked();
        });
        let mut pin = None;
        egui::CollapsingHeader::new("Selected hypothesis and assumptions").default_open(true).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("regional_hypothesis_details").max_height(220.0).show(ui, |ui| {
                if let Some(row) = self.regional.selected_hypothesis.as_ref().and_then(|id| report.hypotheses.iter().find(|row| &row.id == id)) {
                    ui.strong(format!("{} · score {:.6}", row.id, row.score));
                    ui.label(format!("{:.6}, {:.6} · clock offset {:+.3}s · {}", row.observer.latitude_deg, row.observer.longitude_deg, row.clock_offset_s, utc_label(row.recording_start_utc_s)));
                    if row.spatial_at_boundary || row.clock_at_boundary || row.frequency_at_boundary {
                        ui.colored_label(tone(ui, AMBER), format!("At search limit: area {} · time {} · tone {}. Review whether to widen the declared search.", row.spatial_at_boundary, row.clock_at_boundary, row.frequency_at_boundary));
                    }
                    for assignment in &row.assignments {
                        ui.label(format!("{} → {}{} · contribution {:.5}", assignment.event_id, assignment.source_id, assignment.emitted_frequency_hz.map_or(String::new(), |f| format!(" · {f:.4} Hz")), assignment.weighted_contribution));
                        for term in &assignment.terms {
                            ui.small(format!("{:?}: {}", term.term, term.mean_normalized_squared_residual.map_or_else(|| "unavailable".into(), |v| format!("{v:.5}"))));
                        }
                    }
                    muted(ui, "Inspect fit in audio shows predicted curves, arrivals and exact residuals without changing your review. Applying is a separate undoable edit.");
                } else if let Some(row)=report.location_summaries.iter().find(|row|Some(row.observer_index)==self.regional.selected_location) {
                    ui.strong(format!("Receiver {} · {:.6}, {:.6}",row.observer_index,row.observer.latitude_deg,row.observer.longitude_deg));
                    ui.label(format!("{} rankable clocks / {} completed · {} unavailable",row.rankable_clocks,row.completed_clocks,row.unavailable_clocks));
                    if let (Some(score),Some(clock))=(row.best_score,row.best_clock_offset_s) {ui.label(format!("Best score {score:.6} · clock {clock:+.3}s"));}
                    for assignment in &row.best_assignments {ui.label(format!("{} → {}{}",assignment.event_id,assignment.source_id,assignment.emitted_frequency_hz.map_or(String::new(),|f|format!(" · {f:.4} Hz"))));}
                    if row.spatial_at_boundary {ui.colored_label(tone(ui,AMBER),"Receiver is on the declared search boundary.");}
                    muted(ui,"This location's best source/time combination is retained even when absent from the global combination shortlist. Inspect its fit in audio before applying.");
                } else { muted(ui, "Select a row below to inspect its time and source choices."); }
                if let Some((id,detail)) = &self.regional.detail && Some(id)==detail_key.as_ref() {
                    for candidate in &detail.candidates {
                        for reason in &candidate.unavailable_reasons { ui.colored_label(tone(ui, AMBER),reason); }
                        for term in &candidate.terms {
                            ui.strong(match term.term { evidence_score::EvidenceTerm::DopplerFrequency=>"Doppler frequency",evidence_score::EvidenceTerm::DopplerTiming=>"Doppler timing",evidence_score::EvidenceTerm::ThunderTiming=>"Thunder timing" });
                            if let Some(reason)=&term.unavailable_reason { ui.label(reason); }
                            for residual in &term.residuals {ui.label(format!("{} · measured {:.5} / predicted {:.5} {} · difference {:+.5} {}",residual.observation_id,residual.observed,residual.predicted,residual.unit,residual.residual,residual.unit));}
                        }
                    }
                }
            });
        });
        egui::CollapsingHeader::new("Coverage failures and limitations").show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("regional_limits")
                .max_height(200.0)
                .show(ui, |ui| {
                    for limitation in &report.limitations {
                        ui.label(limitation);
                    }
                    for missing in &report.unavailable {
                        ui.label(format!(
                            "{} / {} at clock {:+.2}s, receiver {}: {}",
                            missing.event_id,
                            missing.source_id,
                            missing.clock_offset_s,
                            missing.observer_index,
                            missing.reasons.join("; ")
                        ));
                    }
                });
        });
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.regional.locations_view,
                true,
                "Locations — best at each receiver",
            );
            ui.selectable_value(
                &mut self.regional.locations_view,
                false,
                "Retained source/time combinations",
            );
        });
        let headers = [
            "Rank",
            "Score",
            "Latitude",
            "Longitude",
            "Clock offset",
            "Sources",
        ];
        ui.horizontal_wrapped(|ui| {
            for (index, label) in headers.iter().enumerate() {
                if ui
                    .selectable_label(self.regional.sort == index, *label)
                    .clicked()
                {
                    if self.regional.sort == index {
                        self.regional.descending = !self.regional.descending;
                    } else {
                        self.regional.sort = index;
                        self.regional.descending = false;
                    }
                }
            }
        });
        if self.regional.locations_view && !report.location_summaries.is_empty() {
            let mut rows: Vec<_> = report.location_summaries.iter().collect();
            rows.sort_by(|a, b| {
                if self.regional.sort == 5 {
                    let order = a
                        .best_assignments
                        .iter()
                        .map(|row| &row.source_id)
                        .cmp(b.best_assignments.iter().map(|row| &row.source_id));
                    return (if self.regional.descending {
                        order.reverse()
                    } else {
                        order
                    })
                    .then_with(|| a.observer_index.cmp(&b.observer_index));
                }
                let values = match self.regional.sort {
                    2 => (Some(a.observer.latitude_deg), Some(b.observer.latitude_deg)),
                    3 => (
                        Some(a.observer.longitude_deg),
                        Some(b.observer.longitude_deg),
                    ),
                    4 => (a.best_clock_offset_s, b.best_clock_offset_s),
                    _ => (a.best_score, b.best_score),
                };
                match values {
                    (Some(a), Some(b)) => {
                        if self.regional.descending {
                            b.total_cmp(&a)
                        } else {
                            a.total_cmp(&b)
                        }
                    }
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.observer_index.cmp(&b.observer_index),
                }
                .then_with(|| a.observer_index.cmp(&b.observer_index))
            });
            egui::ScrollArea::both()
                .id_salt("regional_location_rows")
                .auto_shrink([false, false])
                .max_height(table_height)
                .show_rows(ui, 44.0, rows.len(), |ui, range| {
                    for index in range {
                        let row = rows[index];
                        let label = format!(
                            "Cell {:05}   {}   {:.6}, {:.6}   {}   {}/{} clocks{}",
                            row.observer_index,
                            row.best_score.map_or_else(
                                || "Unavailable".into(),
                                |score| format!("{score:.4}")
                            ),
                            row.observer.latitude_deg,
                            row.observer.longitude_deg,
                            row.best_clock_offset_s
                                .map_or_else(|| "—".into(), |clock| format!("{clock:+.2}s")),
                            row.rankable_clocks,
                            row.completed_clocks,
                            if row.spatial_at_boundary {
                                " [limit]"
                            } else {
                                ""
                            }
                        );
                        if ui
                            .selectable_label(
                                self.regional.selected_location == Some(row.observer_index),
                                label,
                            )
                            .clicked()
                        {
                            self.regional.selected_location = Some(row.observer_index);
                            self.regional.selected_hypothesis = None;
                            pin = Some((row.observer.latitude_deg, row.observer.longitude_deg));
                        }
                    }
                });
        } else {
            let mut rows: Vec<_> = report.hypotheses.iter().collect();
            rows.sort_by(|a, b| {
                let order = match self.regional.sort {
                    0 => a.rank.cmp(&b.rank),
                    1 => a.score.total_cmp(&b.score),
                    2 => a.observer.latitude_deg.total_cmp(&b.observer.latitude_deg),
                    3 => a
                        .observer
                        .longitude_deg
                        .total_cmp(&b.observer.longitude_deg),
                    4 => a.clock_offset_s.total_cmp(&b.clock_offset_s),
                    _ => a
                        .assignments
                        .iter()
                        .map(|v| &v.source_id)
                        .cmp(b.assignments.iter().map(|v| &v.source_id)),
                };
                (if self.regional.descending {
                    order.reverse()
                } else {
                    order
                })
                .then_with(|| a.id.cmp(&b.id))
            });
            egui::ScrollArea::both()
                .id_salt("regional_result_rows")
                .auto_shrink([false, false])
                .max_height(table_height)
                .show_rows(ui, 44.0, rows.len(), |ui, range| {
                    for index in range {
                        let row = rows[index];
                        let sources = row
                            .assignments
                            .iter()
                            .map(|a| a.source_id.as_str())
                            .collect::<Vec<_>>()
                            .join(" / ");
                        let label = format!(
                            "#{:>3}   {:>9.4}   {:.6}, {:.6}   {:+.2}s   {}{}",
                            row.rank,
                            row.score,
                            row.observer.latitude_deg,
                            row.observer.longitude_deg,
                            row.clock_offset_s,
                            sources,
                            if row.spatial_at_boundary
                                || row.clock_at_boundary
                                || row.frequency_at_boundary
                            {
                                "  [limit]"
                            } else {
                                ""
                            }
                        );
                        if ui
                            .selectable_label(
                                self.regional.selected_hypothesis.as_ref() == Some(&row.id),
                                label,
                            )
                            .clicked()
                        {
                            self.regional.selected_hypothesis = Some(row.id.clone());
                            self.regional.selected_location = None;
                            pin = Some((row.observer.latitude_deg, row.observer.longitude_deg));
                        }
                    }
                });
        }
        if let Some((latitude, longitude)) = pin {
            self.regional_pin(latitude, longitude);
        }
        if save {
            self.save_regional(ui.ctx());
        }
        if inspect {
            self.inspect_regional(ui.ctx());
        }
        if refine {
            self.prepare_refinement();
        }
        if apply {
            self.apply_regional(ui.ctx());
        }
        true
    }

    fn save_regional(&mut self, ctx: &egui::Context) {
        let Some(doc) = self.regional.report_document.clone() else {
            return;
        };
        if self.document.as_ref() != Some(&doc) {
            return;
        }
        let comparison = self.regional.comparison.clone();
        let report = self.regional.report.clone();
        let expected = if let Some(comparison) = &comparison {
            comparison.inputs_sha256.clone()
        } else if let Some(report) = &report {
            report.inputs_sha256.clone()
        } else {
            return;
        };
        let layers = self.layers.clone();
        let lightning = self.lightning.clone();
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        self.spawn(
            ctx,
            "Saving investigation report and evidence...",
            move || {
                let replay = geosonnar::investigation_archive::capture_replay_inputs(
                    &doc, &layers, &lightning,
                )?;
                let map = geosonnar::investigation_archive::merge_layers(&layers)?;
                if regional_search::inputs_sha256(&map, &lightning)? != expected {
                    return Err("Loaded data changed after this search".into());
                }
                let path = workspace::save(&manifest, &doc)?;
                let directory = path.parent().unwrap().join("regional-investigation");
                if let Some(comparison) = comparison {
                    geosonnar::investigation_report::export_modalities_with_replay_inputs(
                        &directory,
                        &doc,
                        &comparison,
                        &replay,
                    )?;
                } else {
                    geosonnar::investigation_report::export_with_replay_inputs(
                        &directory,
                        &doc,
                        report.as_ref().unwrap(),
                        &replay,
                    )?;
                }
                Ok(ResultValue::RegionalSaved(
                    directory.join("index.html"),
                    Box::new(doc),
                ))
            },
        );
    }

    fn prepare_refinement(&mut self) {
        let (Some(doc), Some(report), Some((observer, clock))) = (
            self.document.as_ref(),
            self.regional.report.as_ref(),
            self.regional.selected_receiver(),
        ) else {
            return;
        };
        if self.regional.report_document.as_ref() != Some(doc)
            || report.status != SearchStatus::Completed
            || self
                .regional
                .comparison
                .as_ref()
                .is_some_and(|comparison| comparison.status != SearchStatus::Completed)
        {
            return;
        }
        match refinement_document(
            doc,
            &observer,
            clock,
            self.regional.refinement_km,
            self.regional.refinement_seconds,
            self.regional.refinement_spacing_m,
            self.regional.refinement_time_step_s,
        ) {
            Ok(next) => {
                let before = self.document.replace(next).unwrap();
                self.checkpoint(before);
                self.regional.step = 0;
                self.regional.guided = true;
                self.notice=Some((false,"Prepared a finer area/time plan with all approved alternatives retained. Review it in Search, estimate, and run explicitly.".into()));
            }
            Err(error) => self.notice = Some((true, error)),
        }
    }

    pub(super) fn inspect_regional(&mut self, ctx: &egui::Context) {
        if !self.report_inputs_ready() {
            self.notice = Some((
                true,
                "Restore or check report data before inspecting a saved hypothesis.".into(),
            ));
            return;
        }
        let (Some(doc), Some(report), Some(choice), Some(key), Some((observer, _))) = (
            self.document.clone(),
            self.regional.report.clone(),
            self.regional.choice(),
            self.regional.selection_key(),
            self.regional.selected_receiver(),
        ) else {
            return;
        };
        if self.regional.report_document.as_ref() != Some(&doc) {
            return;
        }
        let mode_request = self
            .regional
            .comparison
            .as_ref()
            .map(|comparison| comparison.request.regional.clone());
        let mode = self.regional.mode;
        let map = match self.merged_map() {
            Ok(map) => map,
            Err(error) => {
                self.notice = Some((true, error));
                return;
            }
        };
        let lightning = self.lightning.clone();
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        let archived = self.restored_report_context();
        let inputs = super::inspection::InputIdentity::capture(self);
        let origin = super::inspection::InspectionOrigin::Regional {
            key,
            run_id: self.regional.run_id,
        };
        self.inspection
            .clear("A new hypothesis inspection is being prepared.");
        self.spawn(
            ctx,
            "Building the selected hypothesis overlay and exact residuals…",
            move || {
                if let Some(archive) = &archived {
                    geosonnar::investigation_archive::verify_archived_inspection(
                        &manifest, archive, &doc, &map, &lightning,
                    )?;
                } else {
                    workspace::verify(&manifest, &doc)?;
                }
                if regional_search::inputs_sha256(&map, &lightning)? != report.inputs_sha256 {
                    return Err("Loaded data changed after this search".into());
                }
                let mode_doc = mode_request
                    .as_ref()
                    .map(|request| regional_search::mode_document(&doc, request, mode))
                    .transpose()?;
                let temporary = apply_choice(mode_doc.as_ref().unwrap_or(&doc), &report, &choice)?;
                let overlay = geosonnar::prediction_overlay::build(
                    &temporary,
                    &map,
                    &lightning,
                    temporary.strike_fallback_height_m,
                    &observer,
                    &geosonnar::prediction_overlay::OverlaySettings::default(),
                )?;
                if let Some(archive) = &archived {
                    geosonnar::investigation_archive::verify_archived_inspection(
                        &manifest, archive, &doc, &map, &lightning,
                    )?;
                } else {
                    workspace::verify(&manifest, &doc)?;
                }
                Ok(ResultValue::PredictionInspected(Box::new(
                    super::inspection::PreparedInspection {
                        original: doc,
                        inputs,
                        origin,
                        overlay,
                    },
                )))
            },
        );
    }

    pub(super) fn apply_regional(&mut self, ctx: &egui::Context) {
        if !self.report_inputs_ready() {
            self.notice = Some((
                true,
                "Restore or check report data before applying a saved hypothesis.".into(),
            ));
            return;
        }
        let (Some(doc), Some(report), Some(choice)) = (
            self.document.clone(),
            self.regional.report.clone(),
            self.regional.choice(),
        ) else {
            return;
        };
        if self.regional.report_document.as_ref() != Some(&doc) {
            return;
        }
        let comparison = self.regional.comparison.clone();
        let mode = self.regional.mode;
        let map = match self.merged_map() {
            Ok(map) => map,
            Err(error) => {
                self.notice = Some((true, error));
                return;
            }
        };
        let lightning = self.lightning.clone();
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        self.spawn(
            ctx,
            "Verifying selected regional hypothesis...",
            move || {
                workspace::verify(&manifest, &doc).map_err(|error| format!("Applying requires the original source files to be available and unchanged. Archived inspection remains available. {error}"))?;
                if regional_search::inputs_sha256(&map, &lightning)? != report.inputs_sha256 {
                    return Err("Loaded data changed after this search".into());
                }
                let next = if let Some(comparison) = comparison {
                    match choice {
                        ReviewedChoice::Hypothesis(id) => {
                            regional_search::apply_modality_hypothesis(
                                &doc,
                                &comparison,
                                mode,
                                &id,
                            )?
                        }
                        ReviewedChoice::Location(index) => {
                            regional_search::apply_modality_location(
                                &doc,
                                &comparison,
                                mode,
                                index,
                            )?
                        }
                    }
                } else {
                    apply_choice(&doc, &report, &choice)?
                };
                workspace::verify(&manifest, &doc).map_err(|error| format!("Applying requires the original source files to be available and unchanged. Archived inspection remains available. {error}"))?;
                Ok(ResultValue::RegionalApplied(Box::new(next), Box::new(doc)))
            },
        );
    }
}
