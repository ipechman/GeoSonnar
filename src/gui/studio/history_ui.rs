//! Explicit small historical aircraft requests and separate native import.
use super::*;
use geosonnar::adsblol_history::{
    self, HistoryPlan, HistoryProgress, HistoryReport, HistoryRequest, HistoryStatus,
};

pub(super) struct HistoryState {
    pub enabled: bool,
    pub request: HistoryRequest,
    pub plan: Option<HistoryPlan>,
    pub catalog_id: Option<String>,
    pub report: Option<HistoryReport>,
    pub progress: Option<HistoryProgress>,
    pub run_id: u64,
    step: usize,
    padding_km: f64,
    lookback_s: f64,
    allow_partial: bool,
    allow_baro: bool,
    baro_offset: f64,
    baro_uncertainty: f64,
    baro_rationale: String,
    max_gap_s: f64,
    max_speed_mps: f64,
}

impl Default for HistoryState {
    fn default() -> Self {
        Self {
            enabled: true,
            request: HistoryRequest {
                utc_start: String::new(),
                utc_end: String::new(),
                bounds: [0.0; 4],
                max_traces: 64,
                max_download_bytes: 100_000_000,
            },
            plan: None,
            catalog_id: None,
            report: None,
            progress: None,
            run_id: 0,
            step: 0,
            padding_km: 100.0,
            lookback_s: 600.0,
            allow_partial: false,
            allow_baro: false,
            baro_offset: 0.0,
            baro_uncertainty: 150.0,
            baro_rationale: String::new(),
            max_gap_s: 30.0,
            max_speed_mps: 330.0,
        }
    }
}

impl HistoryState {
    pub(super) fn show_report(&mut self) {
        self.step = 2;
    }

    pub fn plan_current(&self) -> bool {
        self.plan
            .as_ref()
            .is_some_and(|plan| plan.request == self.request)
    }

    pub fn accept_item(&mut self, item: data_catalog::HistoryItem) {
        self.catalog_id = Some(item.entry.id);
        self.request = item.plan.request.clone();
        self.plan = Some(item.plan);
        self.report = item.report;
        self.progress = None;
        self.allow_partial = false;
        self.step = if self.report.is_some() { 2 } else { 1 };
    }
}

fn iso_utc(seconds: f64) -> Result<String, String> {
    let label = super::regional::utc_label(seconds);
    let short = label
        .get(..19)
        .ok_or("UTC time is outside the supported calendar")?;
    Ok(format!("{}Z", short.replace(' ', "T")))
}

pub(super) fn copy_search_request(
    doc: &ReviewWorkspace,
    padding_km: f64,
    lookback_s: f64,
    draft: &HistoryRequest,
) -> Result<HistoryRequest, String> {
    let start = workspace::parse_utc(&doc.recording_start_utc)?;
    let (min, max) = doc.regional_plan.as_ref().map_or(
        (doc.map.clock_adjustment_s, doc.map.clock_adjustment_s),
        |plan| (plan.clock.min_offset_s, plan.clock.max_offset_s),
    );
    let [south, west, north, east] = doc.map.bounds;
    if !padding_km.is_finite()
        || !(0.0..=500.0).contains(&padding_km)
        || !lookback_s.is_finite()
        || !(0.0..=3600.0).contains(&lookback_s)
    {
        return Err("Choose finite aircraft-area padding and travel-time look-back".into());
    }
    let dlat = padding_km * 1000.0 / 111_320.0;
    let dlon = dlat / south.abs().max(north.abs()).to_radians().cos().max(0.01);
    let mut request = draft.clone();
    request.utc_start = iso_utc((start + min - lookback_s).floor())?;
    request.utc_end = iso_utc((start + max + doc.duration_s).ceil())?;
    request.bounds = [
        (south - dlat).max(-90.0),
        west - dlon,
        (north + dlat).min(90.0),
        east + dlon,
    ];
    // Do not silently shrink a requested area/window to provider limits.
    adsblol_history::plan(&request)?;
    Ok(request)
}

impl Studio {
    fn start_history_fetch(&mut self, ctx: &egui::Context) {
        if self.busy() || !self.data.history.plan_current() {
            return;
        }
        let Some(plan) = self.data.history.plan.clone() else {
            return;
        };
        let Some(case) = &self.case else {
            return;
        };
        let manifest = case.manifest.clone();
        let catalog_id = self.data.history.catalog_id.clone();
        self.download_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.download_cancel.clone();
        self.data.downloading = Some("history".into());
        self.data.progress = None;
        self.data.history.progress = None;
        self.data.history.run_id = self.data.history.run_id.wrapping_add(1);
        let run_id = self.data.history.run_id;
        let generation = self.generation;
        let sender = self.sender.clone();
        let repaint = ctx.clone();
        self.spawn(ctx,"Fetching reviewed historical aircraft data…",move || {
            let id=match catalog_id {
                Some(id)=> {
                    let stored=data_catalog::read_history(&manifest,&id)?;
                    if stored.plan!=plan {return Err("Stored request differs from the reviewed plan; prepare a new request".into());}
                    id
                },
                None=>data_catalog::plan_history(&manifest,&plan.request)?.entry.id,
            };
            let mut last_update=Instant::now()-Duration::from_secs(1);
            data_catalog::fetch_history(&manifest,&id,&cancel,|progress| {
                if last_update.elapsed()>=Duration::from_millis(120) || progress.completed_files==progress.selected_files {
                    let _=sender.send(Reply {generation,lane:WorkerLane::TransferProgress,result:Ok(ResultValue::HistoryProgress(run_id,progress))});
                    last_update=Instant::now();repaint.request_repaint();
                }
            }).map(|item|ResultValue::HistoryReady(Box::new(item)))
        });
    }

    pub(super) fn reopen_history(&mut self, ctx: &egui::Context, id: String) {
        if self.busy() {
            return;
        }
        let Some(case) = &self.case else {
            return;
        };
        let manifest = case.manifest.clone();
        self.spawn(
            ctx,
            "Verifying stored historical request and response files…",
            move || {
                data_catalog::read_history(&manifest, &id)
                    .map(|item| ResultValue::HistoryReady(Box::new(item)))
            },
        );
    }

    fn import_history_traces(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (Some(case), Some(id), Some(report)) = (
            &self.case,
            &self.data.history.catalog_id,
            &self.data.history.report,
        ) else {
            return;
        };
        let config = geosonnar::adsblol::TraceImportConfig {
            source_label: format!("ADSB.lol historical request {}", report.plan.date_utc),
            time_origin_utc_s: match geosonnar::detailed_data::utc_midnight(&report.plan.date_utc) {
                Ok(value) => value,
                Err(error) => {
                    self.notice = Some((true, error));
                    return;
                }
            },
            time_window_utc_s: [report.plan.start_utc_s, report.plan.end_utc_s],
            bounds: Some(report.plan.request.bounds),
            max_gap_s: self.data.history.max_gap_s,
            max_speed_mps: self.data.history.max_speed_mps,
            accepted_sources: vec!["adsb_icao".into(), "adsr_icao".into()],
            altitude_policy: if self.data.history.allow_baro {
                geosonnar::adsblol::TraceAltitudePolicy::AllowBarometricApproximation {
                    ellipsoid_offset_m: self.data.history.baro_offset,
                    uncertainty_m: self.data.history.baro_uncertainty,
                    rationale: self.data.history.baro_rationale.clone(),
                }
            } else {
                geosonnar::adsblol::TraceAltitudePolicy::GeometricOnly
            },
        };
        if let Err(error) = geosonnar::adsblol::validate_config(&config) {
            self.notice = Some((true, error));
            return;
        }
        let manifest = case.manifest.clone();
        let id = id.clone();
        let allow_partial = self.data.history.allow_partial;
        self.download_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.download_cancel.clone();
        self.spawn(ctx,"Importing reviewed historical traces into a separate managed dataset…",move || data_catalog::import_history(&manifest,&id,&config,allow_partial,&cancel).map(|entry|ResultValue::DataReady(format!("{} imported. Select and load its track layer, then review aircraft alternatives in Search; no associations were approved.",entry.label))));
    }

    pub(super) fn history_request_ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.busy();
        card(ui, |ui| {
            ui.strong("ADSB.lol small historical aircraft request");
            ui.horizontal_wrapped(|ui| {
                for (step, label) in [
                    "1 Time & aircraft area",
                    "2 Review & fetch",
                    "3 Coverage & import",
                ]
                .into_iter()
                .enumerate()
                {
                    ui.selectable_value(&mut self.data.history.step, step, label);
                }
            });
            match self.data.history.step {
                0 => self.history_draft_ui(ui),
                1 => self.history_plan_ui(ui),
                _ => self.history_report_ui(ui),
            }
            if self.data.downloading.as_deref() == Some("history") {
                if let Some(progress) = &self.data.history.progress {
                    let (received, allowed) = geosonnar::data_size::format_gib_pair(
                        progress.http_payload_bytes,
                        progress.max_download_bytes,
                    );
                    ui.label(format!(
                        "{}: {} / {} files · {} / {} received",
                        progress.stage,
                        progress.completed_files,
                        progress.selected_files,
                        received,
                        allowed
                    ));
                    ui.add(
                        egui::ProgressBar::new(
                            progress.completed_files as f32 / progress.selected_files.max(1) as f32,
                        )
                        .desired_width(ui.available_width().min(400.0)),
                    );
                } else if busy {
                    ui.label("Preparing the reviewed historical request…");
                }
            }
        });
    }

    fn history_draft_ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.busy();
        ui.add_enabled_ui(!busy, |ui| {
            ui.label("Explicit UTC interval · at most one hour within one UTC day");
            ui.horizontal_wrapped(|ui| {
                ui.label("From"); accessible_text(ui,"history_start",&mut self.data.history.request.utc_start,"YYYY-MM-DDTHH:MM:SSZ",false,205.0);
                ui.label("To"); accessible_text(ui,"history_end",&mut self.data.history.request.utc_end,"YYYY-MM-DDTHH:MM:SSZ",false,205.0);
            });
            ui.label("Aircraft coverage rectangle · separate from the receiver search area");
            ui.horizontal_wrapped(|ui| {
                for (label,value) in ["South","West","North","East"].into_iter().zip(&mut self.data.history.request.bounds) {
                    ui.label(label); ui.add(egui::DragValue::new(value).speed(0.01).max_decimals(5));
                }
            });
            muted(ui,"Aircraft can be far outside the receiver region. Each area span is limited to 20 degrees. Replay sampling can miss aircraft between observations.");
            egui::CollapsingHeader::new("Copy a padded window from Search").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Area padding");ui.add(egui::DragValue::new(&mut self.data.history.padding_km).range(0.0..=500.0).suffix(" km"));
                    ui.label("Acoustic look-back");ui.add(egui::DragValue::new(&mut self.data.history.lookback_s).range(0.0..=3600.0).suffix(" s"));
                    if ui.button("Copy reviewed Search window").clicked() && let Some(doc)=&self.document {
                        match copy_search_request(doc,self.data.history.padding_km,self.data.history.lookback_s,&self.data.history.request) {
                            Ok(request)=>self.data.history.request=request,
                            Err(error)=>self.notice=Some((true,error)),
                        }
                    }
                });
                muted(ui,"Copies the shared clock range and recording duration with your padding. It does not change Search, request data, or silently truncate an oversized window.");
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Full-trace cap"); ui.add(egui::DragValue::new(&mut self.data.history.request.max_traces).range(1..=adsblol_history::MAX_TRACES));
                ui.label("Cumulative payload allowance (GiB)");
                data_size_input(ui, &mut self.data.history.request.max_download_bytes, 1_000_000..=adsblol_history::MAX_DOWNLOAD_BYTES);
            });
            if ui.button("Prepare bounded history request").clicked() {
                match adsblol_history::plan(&self.data.history.request) {
                    Ok(plan)=> {self.data.history.plan=Some(plan);self.data.history.catalog_id=None;self.data.history.report=None;self.data.history.allow_partial=false;self.data.history.step=1;},
                    Err(error)=>self.notice=Some((true,error)),
                }
            }
        });
    }

    fn history_plan_ui(&mut self, ui: &mut egui::Ui) {
        let current = self.data.history.plan_current();
        let busy = self.busy();
        let Some(plan) = &self.data.history.plan else {
            muted(
                ui,
                "Prepare a bounded request in step 1. Planning does not download data.",
            );
            return;
        };
        ui.label(format!(
            "{} → {}",
            plan.request.utc_start, plan.request.utc_end
        ));
        ui.label(format!(
            "Aircraft area: {:.5}, {:.5} to {:.5}, {:.5}",
            plan.request.bounds[0],
            plan.request.bounds[1],
            plan.request.bounds[2],
            plan.request.bounds[3]
        ));
        ui.label(format!(
            "{} replay files · up to {} full traces · {} cumulative payload allowance",
            plan.replay_chunks.len(),
            plan.request.max_traces,
            geosonnar::data_size::format_gib(plan.request.max_download_bytes)
        ));
        muted(
            ui,
            "This is a bounded request plan, not an availability check or exact download-size quote. Replay files cover broader areas; full traces are requested for sampled candidates in your time/area. The provider receives these requests.",
        );
        muted(
            ui,
            "If the trace cap is reached, candidates are chosen in aircraft-ID order, not acoustic relevance. Omitted candidates remain visible in the report. Budget includes failed response bytes and previous attempts in this request cache.",
        );
        ui.hyperlink_to(
            "ADSB.lol historical data and provider terms",
            &plan.source_docs,
        );
        ui.label(format!("Provider license: {}", plan.license));
        if !current {
            ui.colored_label(
                tone(ui, AMBER),
                "Request changed. Prepare and review the current plan before fetching.",
            );
        }
        if ui
            .add_enabled(
                current && !busy,
                egui::Button::new("Fetch reviewed history request"),
            )
            .clicked()
        {
            self.start_history_fetch(ui.ctx());
        }
        muted(
            ui,
            "A completed fetch is never complete sky coverage and never approves an aircraft for scoring.",
        );
    }

    fn history_report_ui(&mut self, ui: &mut egui::Ui) {
        let Some(report) = self.data.history.report.as_ref() else {
            muted(
                ui,
                "Fetch the reviewed request, or reopen a stored historical request from the library below.",
            );
            return;
        };
        ui.label(format!(
            "Stored request: {} → {}",
            report.plan.request.utc_start, report.plan.request.utc_end
        ));
        ui.strong(match report.status {
            HistoryStatus::Complete => "Requested files finished — sky coverage remains unknown",
            HistoryStatus::Partial => "Partial request — inspect failed or omitted data",
            HistoryStatus::Cancelled => "Cancelled request — incomplete data",
            HistoryStatus::BudgetExhausted => "Payload budget exhausted — incomplete data",
        });
        ui.label(format!(
            "{} candidates · {} trace attempts · {} omitted by cap · {} non-ICAO omissions",
            report.candidates.len(),
            report.traces.len(),
            report.trace_cap_omissions,
            report.non_icao_omissions
        ));
        ui.label(format!("Replay files complete: {} · replay timing gaps: {} · selected trace files complete: {}",report.replay_files_complete,report.replay_timing_gaps,report.selected_traces_complete));
        ui.label(format!(
            "{} cumulative payload · {} reused files",
            geosonnar::data_size::format_gib(report.http_payload_bytes),
            report.reused_files
        ));
        muted(
            ui,
            "Complete describes this bounded request's files only. Missing aircraft are not proof of absence. Downloaded candidates are not confirmed acoustic associations.",
        );
        egui::CollapsingHeader::new("Candidate origins, omissions and download failures").show(
            ui,
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("history_candidates")
                    .max_height(160.0)
                    .show_rows(ui, 40.0, report.candidates.len(), |ui, range| {
                        for index in range {
                            let row = &report.candidates[index];
                            ui.label(format!(
                                "{} · {} · {}{}",
                                row.icao24,
                                row.source_types.join(", "),
                                if row.selected { "requested" } else { "omitted" },
                                row.omission
                                    .as_ref()
                                    .map_or(String::new(), |reason| format!(": {reason}"))
                            ));
                        }
                    });
                egui::ScrollArea::vertical()
                    .id_salt("history_failures")
                    .max_height(130.0)
                    .show(ui, |ui| {
                        for row in &report.replay {
                            if let Some(error) = &row.error {
                                ui.label(format!("Replay {}: {error}", row.chunk.index));
                            }
                        }
                        for row in &report.traces {
                            if let Some(error) = &row.error {
                                ui.label(format!("Aircraft {}: {error}", row.icao24));
                            }
                        }
                        for limitation in &report.limitations {
                            ui.small(limitation);
                        }
                    });
            },
        );
        self.history_import_ui(ui);
    }

    fn history_import_ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.busy();
        let Some(report) = &self.data.history.report else {
            return;
        };
        let partial = report.status != HistoryStatus::Complete;
        let has_traces = report.traces.iter().any(|row| row.decoded.is_some());
        ui.separator();
        ui.strong("Explicit native import");
        ui.label("Import uses this request's time/aircraft area. Geometric altitude and ADS-B / ADS-R positions are the default.");
        ui.add_enabled_ui(!busy, |ui| {
            if partial {
                ui.checkbox(
                    &mut self.data.history.allow_partial,
                    "I reviewed the incomplete coverage; allow importing available traces",
                );
            }
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::DragValue::new(&mut self.data.history.max_gap_s)
                        .range(1.0..=120.0)
                        .suffix(" s maximum gap"),
                );
                ui.add(
                    egui::DragValue::new(&mut self.data.history.max_speed_mps)
                        .range(100.0..=400.0)
                        .suffix(" m/s maximum speed"),
                );
            });
            ui.checkbox(
                &mut self.data.history.allow_baro,
                "Allow an explicit barometric-height approximation",
            );
            if self.data.history.allow_baro {
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.data.history.baro_offset)
                            .suffix(" m ellipsoid offset"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.data.history.baro_uncertainty)
                            .range(1.0..=10000.0)
                            .suffix(" m uncertainty"),
                    );
                });
                accessible_text(
                    ui,
                    "history_baro_rationale",
                    &mut self.data.history.baro_rationale,
                    "Explain the altitude assumption",
                    false,
                    ui.available_width(),
                );
            }
        });
        if ui
            .add_enabled(
                !busy && has_traces && (!partial || self.data.history.allow_partial),
                egui::Button::new("Import reviewed historical traces into library"),
            )
            .clicked()
        {
            self.import_history_traces(ui.ctx());
        }
        if !has_traces {
            muted(ui, "No verified full trace is available to import.");
        }
        muted(
            ui,
            "Import creates a separate managed track dataset. Select and load it afterward; choose acoustic associations separately in Search.",
        );
    }
}
