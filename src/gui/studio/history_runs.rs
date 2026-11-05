//! Explicit local Results history; opening never approves evidence or reruns it.
use super::*;
use geosonnar::{
    investigation_archive::ReportKind,
    regional_history::{self, LoadedRun, RunStatus, RunSummary},
};

#[cfg(test)]
#[path = "history_joint_tests.rs"]
mod joint_tests;

#[derive(Default)]
pub(super) struct HistoryState {
    pub rows: Vec<RunSummary>,
    listed: bool,
    pub status: Option<(bool, String)>,
    pub opened: Option<LoadedRun>,
    current_saved_id: Option<String>,
    pub deleted: bool,
    pub verified_inputs: Option<inspection::InputIdentity>,
    pub unsaved: Option<RunCompletion>,
    // Normally empty: the GUI refuses new computation while a result is unsaved.
    // A late already-running worker may still finish after that gate closes.
    unsaved_backlog: std::collections::VecDeque<RunCompletion>,
    delete_pending: Option<String>,
    discard_pending: bool,
}
#[derive(Clone)]
pub(super) struct RunCompletion {
    pub original_manifest: PathBuf,
    pub document: ReviewWorkspace,
    pub inputs_sha256: String,
    pub outcome: Result<ReportKind, String>,
    pub cancelled: bool,
    pub persistence: Result<RunSummary, String>,
    pub joint_request: Option<geosonnar::joint_evidence::JointRequest>,
}
impl RunCompletion {
    fn retry_history(mut self) -> Self {
        self.persistence = if let Some(request) = &self.joint_request {
            regional_history::begin_joint(
                &self.original_manifest,
                &self.document,
                request,
                &self.inputs_sha256,
            )
        } else {
            regional_history::begin(&self.original_manifest, &self.document, &self.inputs_sha256)
        }
        .and_then(|handle| regional_history::finish(handle, &self.outcome, self.cancelled));
        self
    }
}
pub(super) struct CheckedInputs {
    pub id: String,
    pub previous: ReviewWorkspace,
    pub identity: inspection::InputIdentity,
    pub result: Result<(), String>,
}

pub(super) fn status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "Running",
        RunStatus::Interrupted => "Interrupted / incomplete",
        RunStatus::Completed => "Completed",
        RunStatus::Cancelled => "Cancelled / incomplete",
        RunStatus::Failed => "Failed",
    }
}
impl Studio {
    pub fn has_unsaved_results(&self) -> bool {
        self.run_history.unsaved.is_some() || !self.run_history.unsaved_backlog.is_empty()
    }
    fn retain_unsaved_run(&mut self, completion: RunCompletion) {
        if self.run_history.unsaved.is_none() {
            self.run_history.unsaved = Some(completion);
        } else {
            self.run_history.unsaved_backlog.push_back(completion);
        }
    }
    fn advance_unsaved_run(&mut self) {
        self.run_history.unsaved = self.run_history.unsaved_backlog.pop_front();
        self.run_history.discard_pending = false;
    }
    /// A stale worker may finish after its view has been invalidated. Its durable
    /// journal or failed full outcome still belongs to the analyst. Never replace
    /// a current view or clear another job's running flag from this path.
    pub(super) fn accept_stale_run_completion(&mut self, completion: RunCompletion) {
        self.run_history.listed = false;
        if completion.persistence.is_err() {
            self.retain_unsaved_run(completion);
            self.run_history.status = Some((true, "A previous run finished after the view changed, but its result could not be saved. Its full original outcome is retained; retry saving or explicitly discard it before starting another computation.".into()));
            self.notice = self.run_history.status.clone();
        }
    }
    /// Only the explicit retry worker uses this path. Discard and case changes
    /// are disabled while it runs, so the original queue head remains owned.
    pub(super) fn accept_run_retry(&mut self, completion: RunCompletion) {
        self.run_history.listed = false;
        match &completion.persistence {
            Ok(summary) => {
                self.advance_unsaved_run();
                self.run_history.status = Some((
                    self.has_unsaved_results(),
                    format!(
                        "Original run {} saved. {}",
                        summary.id,
                        if self.has_unsaved_results() {
                            "Another unsaved original outcome remains; save or discard it before continuing."
                        } else {
                            "Open it from Run history to inspect the saved copy."
                        }
                    ),
                ));
            }
            Err(error) => {
                self.run_history.status = Some((
                    true,
                    format!(
                        "Run history NOT saved: {error}. The original outcome is still retained."
                    ),
                ));
                self.run_history.unsaved = Some(completion);
            }
        }
        self.notice = self.run_history.status.clone();
    }
    pub(super) fn invalidate_run_history_list(&mut self) {
        self.run_history.listed = false;
    }
    pub(super) fn clear_history_display_binding(&mut self) {
        self.run_history.opened = None;
        self.run_history.verified_inputs = None;
        self.run_history.current_saved_id = None;
        self.run_history.deleted = false;
    }
    pub(super) fn accept_history_deletion(&mut self, id: &str) {
        self.run_history.listed = false;
        self.run_history.rows.retain(|row| row.id != id);
        if self.run_history.current_saved_id.as_deref() == Some(id) {
            self.run_history.deleted = true;
            self.run_history.verified_inputs = None;
            self.run_history.status = Some((true, "The displayed run's history copy was explicitly deleted. This view remains in memory only; no saved result is claimed.".into()));
        }
        self.notice = Some((
            false,
            format!("Deleted saved run {id}. Source data were not changed."),
        ));
    }

    pub(super) fn accept_run_completion(&mut self, completion: RunCompletion) {
        let is_joint = completion.joint_request.is_some()
            || matches!(&completion.outcome, Ok(ReportKind::JointEnvelope(_)));
        // Studio's worker-lane/run-id guard must reject stale replies before
        // this point. A joint completion must not reset another regional job.
        if is_joint {
            self.joint.running = false;
        } else {
            self.regional.running = false;
        }
        self.run_history.listed = false;
        self.run_history.discard_pending = false;
        let current = self.document.as_ref() == Some(&completion.document);
        let replacing_display = current && completion.outcome.is_ok();
        if replacing_display {
            self.clear_history_display_binding();
            self.run_history.current_saved_id = completion
                .persistence
                .as_ref()
                .ok()
                .map(|summary| summary.id.clone());
        }
        match &completion.persistence {
            Ok(summary) => {
                self.run_history.status = Some((
                    self.has_unsaved_results(),
                    format!(
                        "Latest run {} · {}. Saved result; replay requires original data.{}",
                        summary.id,
                        status_label(summary.status),
                        if self.has_unsaved_results() {
                            " A different original outcome remains unsaved."
                        } else {
                            ""
                        }
                    ),
                ));
            }
            Err(error) => {
                self.run_history.status = Some((
                    true,
                    format!(
                        "Run history NOT saved: {error}. Keep the app open and retry saving, or explicitly discard it. Saving another displayed report does not clear this original outcome."
                    ),
                ));
                self.retain_unsaved_run(completion.clone());
            }
        }
        match completion.outcome {
            Ok(report) if current => {
                self.investigation = investigation::InvestigationState::default();
                match report {
                    ReportKind::Regional(report) => {
                        self.clear_joint_display();
                        self.regional.comparison = None;
                        self.regional.accept_saved_report(report, completion.document);
                    }
                    ReportKind::Comparison(report) => {
                        self.clear_joint_display();
                        self.regional.accept_comparison(report, completion.document);
                    }
                    ReportKind::JointEnvelope(snapshot) => self.accept_joint_report(*snapshot, completion.document),
                }
                self.pending_action = Some(Action::ReviewResults);
                self.notice = self.run_history.status.clone();
            }
            Ok(_) => self.notice = Some((true, "Review changed during the computation. Your edits were kept. Its original result is available in Run history, or in the unsaved-result controls if storage failed.".into())),
            Err(error) => self.notice = Some((true, format!("Computation stopped: {error}. {}", self.run_history.status.as_ref().map_or("", |(_, text)| text)))),
        }
    }

    fn refresh_run_history(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let Some(case) = &self.case else {
            return;
        };
        let manifest = case.manifest.clone();
        self.run_history.listed = true;
        self.spawn(ctx, "Reading saved run history…", move || {
            Ok(ResultValue::RunHistoryListed(regional_history::list(
                &manifest,
            )?))
        });
    }

    pub(super) fn open_run_history(&mut self, ctx: &egui::Context, id: String) {
        if self.busy() {
            return;
        }
        if self.dirty() || self.has_unsaved_results() {
            self.notice = Some((true, "Save or discard your current review and any unsaved result before opening a historical run. Save copy is available without replacing your review.".into()));
            return;
        }
        let (Some(case), Some(previous)) = (&self.case, self.document.clone()) else {
            return;
        };
        let manifest = case.manifest.clone();
        self.spawn(
            ctx,
            "Checking saved run receipts and result…",
            move || {
                Ok(ResultValue::RunHistoryOpened(
                    Box::new(regional_history::load(&manifest, &id)?),
                    Box::new(previous),
                ))
            },
        );
    }

    pub(super) fn accept_history_run(&mut self, loaded: LoadedRun, previous: ReviewWorkspace) {
        if self.document.as_ref() != Some(&previous) || self.has_unsaved_results() {
            self.notice = Some((true, "Review changed while opening history. Your current work was kept; reopen after saving or discarding it.".into()));
            return;
        }
        self.accept_saved_report_view(&loaded.document, loaded.report.as_ref());
        if loaded.report.is_none()
            && let Some(request) = &loaded.joint_request
        {
            self.restore_joint_request(request.clone());
        }
        self.investigation = investigation::InvestigationState::default();
        self.run_history.status = Some((
            false,
            format!(
                "{} · {}. Saved result; replay requires original data.",
                loaded.summary.id,
                status_label(loaded.summary.status)
            ),
        ));
        self.run_history.current_saved_id = Some(loaded.summary.id.clone());
        self.run_history.deleted = false;
        self.run_history.opened = Some(loaded);
        self.run_history.verified_inputs = None;
        self.pending_action = Some(Action::ReviewResults);
        self.notice = Some((false, "Historical result opened. No evidence was approved, data loaded, or computation rerun.".into()));
    }

    fn retry_unsaved_run(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let Some(completion) = self.run_history.unsaved.clone() else {
            return;
        };
        self.spawn(
            ctx,
            "Saving the original result to local history…",
            move || {
                Ok(ResultValue::RunHistoryRetried(Box::new(
                    completion.retry_history(),
                )))
            },
        );
    }

    pub(super) fn check_history_inputs(&mut self, ctx: &egui::Context) {
        if self.busy() || self.run_history.deleted {
            return;
        }
        let (Some(case), Some(loaded), Some(previous)) =
            (&self.case, &self.run_history.opened, self.document.clone())
        else {
            return;
        };
        if loaded.document != previous {
            return;
        }
        let manifest = case.manifest.clone();
        let id = loaded.summary.id.clone();
        let expected = loaded.summary.inputs_sha256.clone();
        let expected_result = loaded.result_sha256.clone();
        let identity = inspection::InputIdentity::capture(self);
        let map = self
            .cached_bindings_match(&previous)
            .and_then(|_| self.merged_map());
        let lightning = self.lightning.clone();
        self.spawn(ctx, "Checking original evidence against the historical result…", move || {
            let result = (|| {
                let reopened = regional_history::load(&manifest, &id)?;
                if reopened.document != previous || reopened.summary.inputs_sha256 != expected || reopened.result_sha256 != expected_result { return Err("Saved run changed; reopen it before continuing".into()); }
                workspace::verify(&manifest, &previous)?;
                let map = map?;
                if let Some(ReportKind::JointEnvelope(snapshot)) = &reopened.report {
                    geosonnar::investigation_archive::verify_joint_sources(&manifest, snapshot, &previous, &map, &lightning)?;
                } else if geosonnar::regional_search::inputs_sha256(&map, &lightning)? != expected { return Err("Loaded data do not match this result. Load the exact original datasets, time origins and height assumptions in Data.".into()); }
                Ok(())
            })();
            Ok(ResultValue::RunHistoryChecked(Box::new(CheckedInputs { id, previous, identity, result })))
        });
    }
    pub(super) fn accept_history_inputs(&mut self, checked: CheckedInputs) {
        if self.document.as_ref() != Some(&checked.previous)
            || self
                .run_history
                .opened
                .as_ref()
                .is_none_or(|run| run.summary.id != checked.id)
            || inspection::InputIdentity::capture(self) != checked.identity
        {
            self.notice = Some((
                true,
                "Review or loaded data changed during the check; check again.".into(),
            ));
            return;
        }
        match checked.result {
            Ok(()) => {
                self.run_history.verified_inputs = Some(checked.identity);
                self.notice = Some((
                    false,
                    "Original data match the saved result; hypothesis inspection is available."
                        .into(),
                ));
            }
            Err(error) => {
                self.run_history.verified_inputs = None;
                self.notice = Some((true, error));
            }
        }
    }

    pub(super) fn run_history_ui(&mut self, ui: &mut egui::Ui) {
        if let Some((problem, text)) = &self.run_history.status {
            if *problem {
                ui.colored_label(tone(ui, RED), text);
            } else {
                muted(ui, text);
            }
        }
        if self.has_unsaved_results() {
            if !self.run_history.unsaved_backlog.is_empty() {
                ui.colored_label(tone(ui, AMBER), format!("{} original outcomes remain unsaved. The controls below save or discard the oldest one.", 1 + self.run_history.unsaved_backlog.len()));
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !self.busy(),
                        egui::Button::new("Retry saving original result"),
                    )
                    .clicked()
                {
                    self.retry_unsaved_run(ui.ctx());
                }
                if ui
                    .add_enabled(!self.busy(), egui::Button::new("Discard unsaved result…"))
                    .clicked()
                {
                    self.run_history.discard_pending = true;
                }
            });
            if self.run_history.discard_pending {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Discard the only unsaved copy of this run?");
                    if ui
                        .add_enabled(!self.busy(), egui::Button::new("Discard result"))
                        .clicked()
                    {
                        self.advance_unsaved_run();
                        self.run_history.status = Some((
                            true,
                            "Unsaved run explicitly discarded. It is not preserved in history."
                                .into(),
                        ));
                    }
                    if ui.button("Keep result").clicked() {
                        self.run_history.discard_pending = false;
                    }
                });
            }
        }
        if let Some(loaded) = &self.run_history.opened {
            ui.label(format!(
                "Historical {} · {}",
                loaded.summary.id,
                status_label(loaded.summary.status)
            ));
            if let Some(error) = &loaded.summary.error {
                ui.colored_label(tone(ui, AMBER), error);
            }
            if loaded.report.is_some() {
                let current = self.document.as_ref() == Some(&loaded.document);
                muted(
                    ui,
                    if self.run_history.deleted {
                        "History copy deleted; this displayed result is held in memory only."
                    } else if loaded.joint_request.is_some() {
                        "Saved joint snapshot includes its observation, plans, grid and normalized source inputs. Original recording and dataset files must be explicitly verified before replay; opening history never restores or runs them."
                    } else {
                        "Saved result; replay requires original data. The exact reviewed inputs are identified by their saved hash. No source buffers or recording are copied into history."
                    },
                );
                if !current {
                    ui.colored_label(tone(ui, AMBER), "Your review changed; this displayed result remains historical. Save copy still uses its original evidence and settings.");
                }
                if ui
                    .add_enabled(
                        current && !self.busy() && !self.run_history.deleted,
                        egui::Button::new("Check loaded original data for inspection"),
                    )
                    .clicked()
                {
                    self.check_history_inputs(ui.ctx());
                }
            }
        }
        let mut refresh = false;
        let mut open = None;
        let mut copy = None;
        let mut delete = None;
        egui::CollapsingHeader::new("Run history — saved, cancelled and failed computations")
            .id_salt("regional_run_history").show(ui, |ui| {
                refresh = !self.run_history.listed && !self.busy();
                if ui.add_enabled(!self.busy(), egui::Button::new("Refresh history")).clicked() { refresh = true; }
                muted(ui, "Local limit: 256 runs / 1.000 GiB. Saving copies and deleting history never changes source datasets. History is not a complete portable case.");
                if self.run_history.rows.is_empty() { ui.label("No saved runs listed."); }
                egui::ScrollArea::vertical().id_salt("run_history_rows").max_height(200.0).show(ui, |ui| {
                    for row in &self.run_history.rows {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("{} · {}", row.id, status_label(row.status)));
                            let inactive = row.status != RunStatus::Running && !self.busy();
                            if ui.add_enabled(inactive, egui::Button::new("Open")).clicked() { open = Some(row.id.clone()); }
                            if ui.add_enabled(inactive && row.has_result, egui::Button::new("Save copy…")).clicked() { copy = Some(row.id.clone()); }
                            if ui.add_enabled(inactive, egui::Button::new("Delete…")).clicked() { delete = Some(row.id.clone()); }
                        });
                        if let Some(error) = &row.error { muted(ui, error); }
                    }
                });
            });
        if let Some(id) = delete {
            self.run_history.delete_pending = Some(id);
        }
        let mut confirm_delete = None;
        if let Some(id) = self.run_history.delete_pending.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "Delete saved run {id}? Source data remain available."
                ));
                if ui
                    .add_enabled(!self.busy(), egui::Button::new("Delete saved run"))
                    .clicked()
                {
                    confirm_delete = Some(id);
                    self.run_history.delete_pending = None;
                }
                if ui.button("Keep saved run").clicked() {
                    self.run_history.delete_pending = None;
                }
            });
        }
        if let Some(id) = confirm_delete {
            if let Some(case) = &self.case {
                let manifest = case.manifest.clone();
                self.spawn(ui.ctx(), "Deleting the selected owned run…", move || {
                    regional_history::delete(&manifest, &id)?;
                    Ok(ResultValue::RunHistoryDeleted(id))
                });
            }
        } else if let Some(id) = open {
            self.open_run_history(ui.ctx(), id);
        } else if let Some(id) = copy {
            if let Some(parent) = rfd::FileDialog::new()
                .set_title("Choose a folder for a new result copy")
                .pick_folder()
                && let Some(case) = &self.case
            {
                let destination = parent.join(format!("{id}-copy"));
                let manifest = case.manifest.clone();
                self.spawn(
                    ui.ctx(),
                    "Exporting the original historical result…",
                    move || {
                        regional_history::export_copy(&manifest, &id, &destination)?;
                        Ok(ResultValue::RunHistoryChanged(format!(
                            "Saved historical result copy: {}. Replay requires original data.",
                            display_path(&destination)
                        )))
                    },
                );
            }
        } else if refresh {
            self.refresh_run_history(ui.ctx());
        }
    }
}
