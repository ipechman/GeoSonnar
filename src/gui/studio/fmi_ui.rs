//! Explicit FMI requests and independently reviewed source interpretation.
use super::*;
use geosonnar::{
    data_catalog::fmi::{self, FmiItem, FmiProgress, FmiReport, FmiStatus},
    fmi_lightning::{self, FmiPlan, FmiRequest},
    lightning_source::{self, LightningSourcePolicy},
};

pub(super) struct FmiState {
    pub enabled: bool,
    pub request: FmiRequest,
    pub plan: Option<FmiPlan>,
    pub catalog_id: Option<String>,
    pub report: Option<FmiReport>,
    pub progress: Option<FmiProgress>,
    pub imported: Option<data_catalog::CatalogEntry>,
    pub run_id: u64,
    pub step: usize,
    padding_km: f64,
    lookback_s: f64,
    interpretation_catalog: Option<String>,
    pub(super) rationale: String,
    pub(super) allowance_s: f64,
    pub(super) approximation_accepted: bool,
}

impl Default for FmiState {
    fn default() -> Self {
        Self {
            enabled: true,
            request: FmiRequest {
                start_utc: String::new(),
                end_utc: String::new(),
                bounds: [0.0; 4],
                max_total_bytes: 64 * 1024 * 1024,
            },
            plan: None,
            catalog_id: None,
            report: None,
            progress: None,
            imported: None,
            run_id: 0,
            step: 0,
            padding_km: 20.0,
            lookback_s: 120.0,
            interpretation_catalog: None,
            rationale: String::new(),
            allowance_s: 1.0,
            approximation_accepted: false,
        }
    }
}

impl FmiState {
    pub fn plan_current(&self) -> bool {
        self.plan
            .as_ref()
            .is_some_and(|p| p.request == self.request)
    }

    pub fn accept_item(&mut self, item: FmiItem) {
        self.request = item.plan.request.clone();
        self.catalog_id = Some(item.entry.id);
        self.plan = Some(item.plan);
        self.report = item.report;
        self.progress = None;
        self.imported = None;
        self.step = if self.report.is_some() { 2 } else { 1 };
    }
}

pub(super) enum FmiOutcome {
    Stored(Box<FmiItem>),
    Imported(Box<data_catalog::CatalogEntry>),
}

pub(super) struct FmiReply {
    pub request_at_start: FmiRequest,
    pub outcome: FmiOutcome,
}

fn utc(seconds: f64) -> Result<String, String> {
    let label = super::regional::utc_label(seconds);
    let date = label
        .get(..19)
        .ok_or("UTC time is outside the supported calendar")?;
    Ok(format!("{}Z", date.replace(' ', "T")))
}

pub(super) fn copy_fmi_search_window(
    doc: &ReviewWorkspace,
    draft: &FmiRequest,
    padding_km: f64,
    lookback_s: f64,
) -> Result<FmiRequest, String> {
    if !padding_km.is_finite()
        || !(0.0..=500.0).contains(&padding_km)
        || !lookback_s.is_finite()
        || !(0.0..=3600.0).contains(&lookback_s)
    {
        return Err("Choose finite lightning-area padding and acoustic look-back".into());
    }
    let start = workspace::parse_utc(&doc.recording_start_utc)?;
    let (min, max) = doc.regional_plan.as_ref().map_or(
        (doc.map.clock_adjustment_s, doc.map.clock_adjustment_s),
        |p| (p.clock.min_offset_s, p.clock.max_offset_s),
    );
    let [south, west, north, east] = doc.map.bounds;
    let dlat = padding_km * 1000.0 / 111_320.0;
    let dlon = dlat / south.abs().max(north.abs()).to_radians().cos().max(0.01);
    let request = FmiRequest {
        start_utc: utc((start + min - lookback_s).floor())?,
        end_utc: utc((start + max + doc.duration_s).ceil())?,
        bounds: [south - dlat, west - dlon, north + dlat, east + dlon],
        max_total_bytes: draft.max_total_bytes,
    };
    // Planning rejects unsupported bounds/length; never silently shrink them.
    fmi_lightning::plan(&request)?;
    Ok(request)
}

impl Studio {
    fn spawn_fmi<F>(&mut self, ctx: &egui::Context, label: &str, task: F)
    where
        F: FnOnce(u64, u64, Sender<Reply>, egui::Context) -> Result<ResultValue, String>
            + Send
            + 'static,
    {
        if self.busy() {
            return;
        }
        self.data.fmi.run_id = self.data.fmi.run_id.wrapping_add(1);
        let run_id = self.data.fmi.run_id;
        let generation = self.generation;
        let sender = self.sender.clone();
        let repaint = ctx.clone();
        self.data.progress = None;
        self.busy = Some(label.into());
        self.notice = None;
        self.spawn_lane(ctx, WorkerLane::FmiTask(run_id), move || {
            task(run_id, generation, sender, repaint)
        });
    }

    pub(super) fn accept_fmi_reply(&mut self, reply: FmiReply) {
        self.data.downloading = None;
        self.data.refresh_requested = true;
        if self.data.fmi.request != reply.request_at_start {
            self.notice = Some((true, "The lightning request changed while the operation ran. Its stored files remain in the library; your newer draft was kept.".into()));
            return;
        }
        match reply.outcome {
            FmiOutcome::Stored(item) => {
                self.data.fmi.accept_item(*item);
                self.data.show_fmi();
                self.notice = Some((false, "FMI request ready for review. Download completion does not establish complete lightning coverage; import and load are separate actions.".into()));
            }
            FmiOutcome::Imported(entry) => {
                self.data.select_entry(entry.id.clone());
                self.data.fmi.imported = Some(*entry);
                self.data.fmi.step = 3;
                self.notice = Some((false, "FMI strikes imported into a separate managed dataset. Load the dataset explicitly, then review source assumptions and strike associations.".into()));
            }
        }
    }

    pub(super) fn reopen_fmi(&mut self, ctx: &egui::Context, id: String) {
        if self.busy() {
            return;
        }
        let Some(case) = &self.case else {
            return;
        };
        let manifest = case.manifest.clone();
        let request_at_start = self.data.fmi.request.clone();
        self.spawn_fmi(
            ctx,
            "Verifying the stored FMI request and its files…",
            move |_, _, _, _| {
                fmi::load(&manifest, &id).map(|item| {
                    ResultValue::FmiReady(Box::new(FmiReply {
                        request_at_start,
                        outcome: FmiOutcome::Stored(Box::new(item)),
                    }))
                })
            },
        );
    }

    fn fetch_fmi(&mut self, ctx: &egui::Context) {
        if self.busy() || !self.data.fmi.plan_current() {
            return;
        }
        let (Some(case), Some(plan)) = (&self.case, self.data.fmi.plan.clone()) else {
            return;
        };
        let manifest = case.manifest.clone();
        let catalog_id = self.data.fmi.catalog_id.clone();
        let request_at_start = self.data.fmi.request.clone();
        self.download_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.download_cancel.clone();
        self.data.downloading = Some("fmi".into());
        self.data.fmi.progress = None;
        self.spawn_fmi(ctx, "Downloading the reviewed FMI lightning request…", move |run_id, generation, sender, repaint| {
            let id = match catalog_id {
                Some(id) => {
                    let stored = fmi::load(&manifest, &id)?;
                    if stored.plan != plan { return Err("Stored FMI plan differs from the reviewed request; prepare a new plan".into()); }
                    id
                }
                None => fmi::plan(&manifest, &plan.request)?.entry.id,
            };
            let mut last_update = Instant::now() - Duration::from_secs(1);
            fmi::fetch(&manifest, &id, &cancel, |progress| {
                if last_update.elapsed() >= Duration::from_millis(120) || progress.chunks_complete == progress.chunks_total {
                    let _ = sender.send(Reply { generation, lane: WorkerLane::TransferProgress,
                        result: Ok(ResultValue::FmiProgress(run_id, progress)) });
                    last_update = Instant::now();
                    repaint.request_repaint();
                }
            }).map(|item| ResultValue::FmiReady(Box::new(FmiReply { request_at_start, outcome: FmiOutcome::Stored(Box::new(item)) })))
        });
    }

    fn import_fmi(&mut self, ctx: &egui::Context) {
        if self.busy() || !self.data.fmi.plan_current() {
            return;
        }
        let (Some(case), Some(id), Some(report)) =
            (&self.case, &self.data.fmi.catalog_id, &self.data.fmi.report)
        else {
            return;
        };
        if !matches!(report.status, FmiStatus::Complete) || report.owned_rows == 0 {
            return;
        }
        let manifest = case.manifest.clone();
        let id = id.clone();
        let request_at_start = self.data.fmi.request.clone();
        self.download_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.download_cancel.clone();
        self.spawn_fmi(
            ctx,
            "Importing FMI strikes with their original source metadata…",
            move |_, _, _, _| {
                fmi::import(&manifest, &id, &cancel).map(|entry| {
                    ResultValue::FmiReady(Box::new(FmiReply {
                        request_at_start,
                        outcome: FmiOutcome::Imported(Box::new(entry)),
                    }))
                })
            },
        );
    }

    pub(super) fn fmi_request_ui(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.strong("FMI / NORDLIS · public Nordic lightning history");
            muted(
                ui,
                "Northern Europe coverage; availability and completeness vary. This source does not supply worldwide or Quebec lightning history. No account is needed.",
            );
            ui.horizontal_wrapped(|ui| {
                for (step, label) in ["1 Plan", "2 Download", "3 Import", "4 Load"]
                    .into_iter()
                    .enumerate()
                {
                    ui.selectable_value(&mut self.data.fmi.step, step, label);
                }
            });
            match self.data.fmi.step {
                0 => self.fmi_draft_ui(ui),
                1 => self.fmi_download_ui(ui),
                2 => self.fmi_import_ui(ui),
                _ => self.fmi_load_ui(ui),
            }
            if self.data.downloading.as_deref() == Some("fmi") {
                if let Some(progress) = &self.data.fmi.progress {
                    ui.label(format!(
                        "{} · {} / {} chunks · {} received",
                        progress.message,
                        progress.chunks_complete,
                        progress.chunks_total,
                        geosonnar::data_size::format_gib(progress.transferred_bytes)
                    ));
                    ui.add(
                        egui::ProgressBar::new(
                            progress.chunks_complete as f32 / progress.chunks_total.max(1) as f32,
                        )
                        .desired_width(ui.available_width().min(400.0)),
                    );
                } else {
                    ui.label("Preparing the reviewed request…");
                }
            }
            ui.hyperlink_to(
                "FMI open-data terms · CC BY 4.0",
                "https://en.ilmatieteenlaitos.fi/open-data-licence",
            );
        });
    }

    fn fmi_draft_ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.busy();
        ui.add_enabled_ui(!busy, |ui| {
            ui.label("Explicit UTC interval · at most one hour per request");
            ui.horizontal_wrapped(|ui| {
                ui.label("From"); accessible_text(ui, "fmi_start", &mut self.data.fmi.request.start_utc, "YYYY-MM-DDTHH:MM:SSZ", false, 205.0);
                ui.label("To"); accessible_text(ui, "fmi_end", &mut self.data.fmi.request.end_utc, "YYYY-MM-DDTHH:MM:SSZ", false, 205.0);
            });
            ui.label("Requested lightning area · separate from the receiver search area");
            ui.horizontal_wrapped(|ui| {
                for (label, value) in ["South", "West", "North", "East"].into_iter().zip(&mut self.data.fmi.request.bounds) {
                    ui.label(label); ui.add(egui::DragValue::new(value).speed(0.01).max_decimals(5));
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Cumulative response allowance (GiB)");
                data_size_input(ui, &mut self.data.fmi.request.max_total_bytes, 1_048_576..=64 * 1_048_576);
            });
            egui::CollapsingHeader::new("Copy a padded window from Search").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Area padding"); ui.add(egui::DragValue::new(&mut self.data.fmi.padding_km).range(0.0..=500.0).suffix(" km"));
                    ui.label("Thunder travel look-back"); ui.add(egui::DragValue::new(&mut self.data.fmi.lookback_s).range(0.0..=3600.0).suffix(" s"));
                    if ui.button("Copy Search window for lightning").clicked() && let Some(doc) = &self.document {
                        match copy_fmi_search_window(doc, &self.data.fmi.request, self.data.fmi.padding_km, self.data.fmi.lookback_s) {
                            Ok(request) => self.data.fmi.request = request,
                            Err(error) => self.notice = Some((true, error)),
                        }
                    }
                });
                muted(ui, "Copies clock uncertainty and recording duration with your padding. Oversized requests are rejected, not shortened; Search itself is unchanged.");
            });
            if ui.button("Prepare FMI request").clicked() {
                match fmi_lightning::plan(&self.data.fmi.request) {
                    Ok(plan) => {
                        self.data.fmi.plan = Some(plan);
                        self.data.fmi.catalog_id = None;
                        self.data.fmi.report = None;
                        self.data.fmi.imported = None;
                        self.data.fmi.step = 1;
                    }
                    Err(error) => self.notice = Some((true, error)),
                }
            }
        });
    }

    fn fmi_download_ui(&mut self, ui: &mut egui::Ui) {
        let Some(plan) = &self.data.fmi.plan else {
            ui.label("Prepare a request in step 1. Planning uses no network.");
            return;
        };
        ui.label(format!(
            "{} → {}",
            plan.request.start_utc, plan.request.end_utc
        ));
        ui.label(format!(
            "South / west / north / east: {:.5}, {:.5}, {:.5}, {:.5}",
            plan.request.bounds[0],
            plan.request.bounds[1],
            plan.request.bounds[2],
            plan.request.bounds[3]
        ));
        ui.label(format!(
            "{} chunks · {} cumulative allowance · {} per response",
            plan.chunks.len(),
            geosonnar::data_size::format_gib(plan.request.max_total_bytes),
            geosonnar::data_size::format_gib(plan.max_response_bytes)
        ));
        muted(
            ui,
            "This local plan does not certify availability. Download sends the displayed time and area to FMI; verified cached chunks may be reused. Failed bytes also count toward the request budget.",
        );
        if !self.data.fmi.plan_current() {
            ui.colored_label(
                tone(ui, AMBER),
                "Request changed. Prepare a new plan before downloading or importing.",
            );
        }
        if ui
            .add_enabled(
                !self.busy() && self.data.fmi.plan_current(),
                egui::Button::new("Download / resume reviewed FMI request"),
            )
            .clicked()
        {
            self.fetch_fmi(ui.ctx());
        }
    }

    fn fmi_import_ui(&mut self, ui: &mut egui::Ui) {
        let Some(report) = &self.data.fmi.report else {
            ui.label(
                "Download the reviewed request, or reopen a stored FMI request from the library.",
            );
            return;
        };
        ui.strong(match report.status {
            FmiStatus::Complete => {
                "Requested chunks completed; lightning coverage is not certified"
            }
            FmiStatus::Partial => "Partial request; import is unavailable",
            FmiStatus::Cancelled => "Cancelled request; import is unavailable",
        });
        ui.label(format!(
            "{} / {} chunks complete · {} source rows · {} retained rows · {} transferred",
            report.chunks_complete,
            report.chunks_total,
            report.raw_rows,
            report.owned_rows,
            geosonnar::data_size::format_gib(report.transferred_bytes)
        ));
        muted(
            ui,
            "Whole-second timestamps, ETRS89 coordinates and provider row identities remain attached. Strike altitude is unknown. Conflicting location-error units are retained as raw metadata and are not scored.",
        );
        if report.owned_rows == 0 {
            ui.label("No rows to import. An empty response does not establish that lightning was absent.");
        }
        egui::CollapsingHeader::new("Request errors and omissions").show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("fmi_errors")
                .max_height(140.0)
                .show(ui, |ui| {
                    for error in &report.errors {
                        ui.label(error);
                    }
                });
        });
        let complete = matches!(report.status, FmiStatus::Complete) && report.owned_rows > 0;
        if !self.data.fmi.plan_current() {
            ui.colored_label(
                tone(ui, AMBER),
                "Request changed. Reopen the stored request or prepare a new plan.",
            );
        }
        if ui
            .add_enabled(
                !self.busy() && complete && self.data.fmi.plan_current(),
                egui::Button::new("Import verified FMI strikes into library"),
            )
            .clicked()
        {
            self.import_fmi(ui.ctx());
        }
        if ui
            .add_enabled(
                !self.busy(),
                egui::Button::new("Review download / resume step"),
            )
            .clicked()
        {
            self.data.fmi.step = 1;
        }
    }

    fn fmi_load_ui(&mut self, ui: &mut egui::Ui) {
        let Some(entry) = self.data.fmi.imported.clone() else {
            ui.label("Import a completed nonempty request first, or select a previously imported FMI dataset in Stored datasets.");
            return;
        };
        ui.strong(&entry.label);
        muted(
            ui,
            "Loading adds a map/scoring dataset. It does not select strikes, approve sound sources, set strike heights or run a search.",
        );
        if ui
            .add_enabled(!self.busy(), egui::Button::new("Load imported FMI strikes"))
            .clicked()
        {
            self.load_catalog_layer(ui.ctx(), &entry);
        }
    }

    pub(super) fn lightning_source_policy_ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.busy();
        let (Some(catalog), Some(doc)) = (&self.lightning, &mut self.document) else {
            return;
        };
        let Some(metadata) = catalog.provenance.source_metadata.as_ref() else {
            return;
        };
        let state = &mut self.data.fmi;
        if state.interpretation_catalog.as_deref() != Some(catalog.input_sha256.as_str()) {
            state.interpretation_catalog = Some(catalog.input_sha256.clone());
            if let Some(policy) = doc
                .mix
                .lightning_source_policy
                .as_ref()
                .filter(|p| p.catalog_sha256 == catalog.input_sha256)
            {
                state.rationale = policy.etrs89_as_wgs84_rationale.clone();
                state.allowance_s = policy.time_representation_allowance_s;
                state.approximation_accepted = true;
            } else {
                state.rationale.clear();
                state.allowance_s = 1.0;
                state.approximation_accepted = false;
            }
        }
        card(ui, |ui| {
            ui.strong("FMI source interpretation · required for thunder scoring");
            muted(
                ui,
                "Original source: ETRS89 coordinates; whole-second timestamps with unknown rounding and clock accuracy. No strike height is provided. The location-error field has conflicting units and is not a calibrated radius.",
            );
            if let Some(notes) = &catalog.provenance.notes {
                muted(ui, notes);
            }
            if let Some(license) = &catalog.provenance.license {
                ui.small(format!("Source license: {license}"));
            }
            ui.add_enabled_ui(!busy, |ui| {
                ui.checkbox(&mut state.approximation_accepted, "I accept an ETRS89-as-WGS84 approximation for this analysis");
                accessible_text(ui, "fmi_crs_rationale", &mut state.rationale, "Explain the approximation and its limits", true, ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label("Declared timestamp-representation allowance");
                    ui.add(egui::DragValue::new(&mut state.allowance_s).range(1.0..=3600.0).suffix(" s"));
                });
                muted(ui, "This allowance is a declared model limit, not a measured Gaussian error or clock correction. Original timestamps remain unchanged. The Search thunder model scale must be at least this allowance; it is never adjusted automatically.");
                let policy = LightningSourcePolicy {
                    catalog_sha256: catalog.input_sha256.clone(),
                    etrs89_as_wgs84_rationale: state.rationale.trim().into(),
                    time_representation_allowance_s: state.allowance_s,
                };
                // Import validates immutable records once. Live form validation
                // touches only the bounded policy and small source descriptor.
                let valid = lightning_source::validate_policy_parameters(&catalog.input_sha256, metadata, Some(&policy), doc.mix.thunder_scale_s);
                if let Err(error) = &valid { ui.colored_label(tone(ui, AMBER), error); }
                if ui.add_enabled(state.approximation_accepted && valid.is_ok(), egui::Button::new("Apply declared lightning source assumptions")).clicked() {
                    doc.mix.lightning_source_policy = Some(policy);
                }
                if doc.mix.lightning_source_policy.is_some() && ui.small_button("Clear lightning source assumptions").clicked() {
                    doc.mix.lightning_source_policy = None;
                    state.approximation_accepted = false;
                }
            });
            match lightning_source::validate_policy_parameters(&catalog.input_sha256, metadata, doc.mix.lightning_source_policy.as_ref(), doc.mix.thunder_scale_s) {
                Ok(()) => ui.label("Saved assumptions match this dataset. Thunder picks, source review, associations and strike-height assumptions remain separate."),
                Err(error) => ui.colored_label(tone(ui, AMBER), format!("Thunder scoring needs source review: {error}")),
            };
        });
    }
}
