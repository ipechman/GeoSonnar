//! Reopen immutable investigations without implicitly loading evidence or scoring.
use super::*;
use geosonnar::investigation_archive::{self, LoadedInvestigation, ReplayInputs, ReportKind};

#[derive(Default)]
pub(super) struct InvestigationState {
    pub archive: Option<Arc<LoadedInvestigation>>,
    pub token: u64,
    verified_inputs: Option<inspection::InputIdentity>,
    pub problem: Option<String>,
}

pub(super) struct InputsReply {
    pub token: u64,
    pub receipt_sha256: String,
    pub document: ReviewWorkspace,
    pub previous_inputs: inspection::InputIdentity,
    /// Some restores archived buffers; None only verifies the current buffers.
    pub result: Result<Option<ReplayInputs>, String>,
}

impl Studio {
    pub(super) fn open_investigation(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        if self.dirty() || self.has_unsaved_results() {
            self.notice = Some((
                true,
                "Save or discard your unsaved review before opening a saved investigation.".into(),
            ));
            return;
        }
        if let Some(directory) = rfd::FileDialog::new()
            .set_title("Open saved investigation folder")
            .pick_folder()
        {
            self.open_investigation_path(ctx, directory);
        }
    }

    pub(super) fn open_investigation_path(&mut self, ctx: &egui::Context, directory: PathBuf) {
        if self.busy() {
            return;
        }
        if self.dirty() || self.has_unsaved_results() {
            self.notice = Some((
                true,
                "Save or discard your unsaved review before opening a saved investigation.".into(),
            ));
            return;
        }
        let (Some(case), Some(previous)) = (&self.case, self.document.clone()) else {
            return;
        };
        let manifest = case.manifest.clone();
        self.spawn(
            ctx,
            "Checking the saved investigation and its receipt…",
            move || {
                let loaded = investigation_archive::load(&directory)?;
                let case = geosonnar::casework::read_case(&manifest, false)?;
                loaded.matches_case(&case, loaded.document.channel)?;
                Ok(ResultValue::InvestigationLoaded(
                    Box::new(loaded),
                    Box::new(previous),
                ))
            },
        );
    }

    pub(super) fn accept_investigation(
        &mut self,
        loaded: LoadedInvestigation,
        previous: ReviewWorkspace,
    ) {
        if self.document.as_ref() != Some(&previous) || self.has_unsaved_results() {
            self.notice = Some((true, "Your review changed while opening the investigation. Your edits were kept; open it again after saving or discarding them.".into()));
            return;
        }
        let Some(case) = &self.case else {
            return;
        };
        if let Err(error) = loaded.matches_case(&case.case, loaded.document.channel) {
            self.notice = Some((true, error));
            return;
        }
        self.accept_saved_report_view(&loaded.document, Some(&loaded.report));
        self.run_history.opened = None;
        self.run_history.verified_inputs = None;
        let token = self.investigation.token.wrapping_add(1);
        self.investigation = InvestigationState {
            archive: Some(Arc::new(loaded)),
            token,
            verified_inputs: None,
            problem: None,
        };
        self.pending_action = Some(Action::ReviewResults);
        self.notice = Some((false, "Saved Results opened. Nothing was applied or rerun. Restore report data explicitly to inspect a hypothesis; recording and original source availability are checked separately.".into()));
    }

    pub(super) fn accept_saved_report_view(
        &mut self,
        document: &ReviewWorkspace,
        report: Option<&ReportKind>,
    ) {
        self.clear_history_display_binding();
        self.stop();
        self.regional.cancel.store(true, Ordering::Relaxed);
        self.clear_joint_display();
        self.download_cancel.store(true, Ordering::Relaxed);
        self.generation = self.generation.wrapping_add(1);
        self.frame_busy = false;
        self.frame_requested = false;
        self.progress_busy = false;
        self.busy = None;
        self.window = None;
        self.texture = None;
        self.video = None;
        self.video_time = None;
        self.video_available = None;
        self.layers.clear();
        self.lightning = None;
        self.loaded_data_revision = self.loaded_data_revision.wrapping_add(1);
        self.score = None;
        self.score_document = None;
        self.selected = None;
        self.pending_undo = None;
        self.undo.clear();
        self.redo.clear();
        self.cursor_s = 0.0;
        self.window_start = 0.0;
        self.window_end = document.duration_s.min(60.0);
        self.view.time_min_s = 0.0;
        self.view.time_max_s = self.window_end;
        self.tool = EditTool::Inspect;
        self.heights = receiver_ui::ReceiverHeightState::default();
        self.inspection = inspection::InspectionState::default();
        self.data.downloading = None;
        self.data.progress = None;
        self.data.refresh_requested = false;
        let run_id = self.regional.run_id.wrapping_add(1);
        self.regional = regional::RegionalState::default();
        self.regional.run_id = run_id;
        match report {
            Some(ReportKind::Regional(report)) => self
                .regional
                .accept_saved_report(report.clone(), document.clone()),
            Some(ReportKind::Comparison(report)) => self
                .regional
                .accept_comparison(report.clone(), document.clone()),
            Some(ReportKind::JointEnvelope(snapshot)) => {
                self.accept_joint_report((**snapshot).clone(), document.clone())
            }
            None => {}
        }
        self.saved = Some(document.clone());
        self.document = Some(document.clone());
        self.map = map::MapState::default();
    }

    pub(super) fn report_inputs_ready(&self) -> bool {
        if self.run_history.opened.is_none()
            && self.investigation.archive.is_none()
            && self
                .document
                .as_ref()
                .is_some_and(|doc| !self.deleted_sources_ready(doc))
        {
            return false;
        }
        if let Some(run) = &self.run_history.opened {
            return !self.run_history.deleted
                && self.document.as_ref() == Some(&run.document)
                && self.run_history.verified_inputs.as_ref()
                    == Some(&inspection::InputIdentity::capture(self));
        }
        self.investigation.archive.as_ref().is_none_or(|archive| {
            self.document.as_ref() == Some(&archive.document)
                && self.investigation.verified_inputs.as_ref()
                    == Some(&inspection::InputIdentity::capture(self))
        })
    }

    pub(super) fn restored_report_context(&self) -> Option<Arc<LoadedInvestigation>> {
        self.investigation.archive.clone()
    }

    pub(super) fn report_data_changed(&mut self) {
        self.loaded_data_revision = self.loaded_data_revision.wrapping_add(1);
        self.investigation.verified_inputs = None;
        self.investigation.problem = None;
        self.run_history.verified_inputs = None;
    }

    pub(super) fn invalidate_deleted_archive_sources(
        &mut self,
        affected: impl Fn(&std::path::Path) -> bool,
    ) {
        if self.investigation.archive.as_ref().is_some_and(|archive| {
            archive
                .document
                .datasets
                .iter()
                .any(|binding| affected(&binding.path))
        }) {
            self.investigation.verified_inputs = None;
            self.investigation.problem = Some("An original source referenced by this saved investigation was deleted. Its numerical result and archived snapshot remain unchanged.".into());
        }
    }

    pub(super) fn prepare_report_inputs(&mut self, ctx: &egui::Context, restore: bool) {
        if self.busy() {
            return;
        }
        let Some(archive) = self.investigation.archive.clone() else {
            return;
        };
        if self.document.as_ref() != Some(&archive.document) {
            self.notice = Some((true, "This review differs from the saved investigation. Undo those edits or reopen the saved investigation before restoring its inputs.".into()));
            return;
        }
        if restore && !archive.has_replay_inputs {
            self.notice = Some((true, "This older export has no input snapshot. Load its original datasets in Data, then check that they match this saved report.".into()));
            return;
        }
        let token = self.investigation.token;
        let previous_inputs = inspection::InputIdentity::capture(self);
        let receipt_sha256 = archive.receipt_sha256.clone();
        let document = archive.document.clone();
        let current = if restore {
            None
        } else {
            Some((self.merged_map(), self.lightning.clone()))
        };
        self.investigation.problem = None;
        self.spawn(ctx, if restore { "Restoring the report's archived input snapshot…" } else { "Checking loaded data against the saved report…" }, move || {
            let result = if restore {
                investigation_archive::load_replay_inputs(&archive).map(Some)
            } else {
                (|| {
                    let reopened = investigation_archive::load(&archive.directory)?;
                    if reopened.receipt_sha256 != archive.receipt_sha256 { return Err("The investigation changed on disk; reopen it before continuing.".into()); }
                    let (map, lightning) = current.unwrap();
                    if geosonnar::regional_search::inputs_sha256(&map?, &lightning)? != archive.inputs_sha256 {
                        return Err("Loaded data do not match the saved report. Restore its input snapshot, or load the exact original datasets and their time/height assumptions.".into());
                    }
                    Ok(None)
                })()
            };
            Ok(ResultValue::InvestigationInputs(Box::new(InputsReply { token, receipt_sha256, document, previous_inputs, result })))
        });
    }

    pub(super) fn accept_report_inputs(&mut self, reply: InputsReply) {
        if self.document.as_ref() != Some(&reply.document)
            || self.investigation.token != reply.token
            || self
                .investigation
                .archive
                .as_ref()
                .is_none_or(|archive| archive.receipt_sha256 != reply.receipt_sha256)
            || inspection::InputIdentity::capture(self) != reply.previous_inputs
        {
            self.notice = Some((true, "Review or loaded data changed during restoration. Your current state was kept; check the report data again.".into()));
            return;
        }
        match reply.result {
            Ok(replay) => {
                if let Some(replay) = replay {
                    self.layers = replay.layers;
                    self.lightning = replay.lightning;
                    self.report_data_changed();
                    self.score = None;
                    self.inspection
                        .clear("Report inputs were restored; inspect a hypothesis explicitly.");
                }
                self.investigation.verified_inputs = Some(inspection::InputIdentity::capture(self));
                self.investigation.problem = None;
                self.notice = Some((false, "Inputs match the saved report. Archived inputs; original source files not rechecked. Inspect checks the recording; applying or new scoring still requires original source files.".into()));
            }
            Err(error) => {
                self.investigation.verified_inputs = None;
                self.investigation.problem = Some(error.clone());
                self.notice = Some((true, error));
            }
        }
    }

    pub(super) fn investigation_readiness_ui(&mut self, ui: &mut egui::Ui) {
        let Some(archive) = self.investigation.archive.as_ref() else {
            return;
        };
        let current = self.document.as_ref() == Some(&archive.document);
        let ready = self.report_inputs_ready();
        let can_restore = archive.has_replay_inputs;
        let mut restore = false;
        let mut verify = false;
        ui.label(format!(
            "Saved investigation: {}",
            display_path(&archive.directory)
        ));
        if !current {
            ui.colored_label(tone(ui, AMBER), "Review changed since this saved investigation. Its results remain historical; Undo or reopen before inspecting them.");
        } else if ready {
            ui.colored_label(
                tone(ui, AMBER),
                "Archived inputs; original source files not rechecked",
            );
            muted(
                ui,
                "Input identity matches. Inspect checks the matching recording. Apply, new scoring and normal review saving also require original source files.",
            );
        } else {
            muted(
                ui,
                "Saved Results are readable. Report data are not loaded or have not been checked against this report.",
            );
            if !can_restore {
                muted(
                    ui,
                    "This older export has no input snapshot. Load its original datasets in Data and check them here; a mismatch never changes the saved result.",
                );
            }
        }
        if let Some(problem) = &self.investigation.problem {
            ui.colored_label(tone(ui, RED), problem);
        }
        ui.horizontal_wrapped(|ui| {
            restore = ui
                .add_enabled(
                    current && !self.busy() && can_restore,
                    egui::Button::new("Restore report data"),
                )
                .clicked();
            verify = ui
                .add_enabled(
                    current && !self.busy(),
                    egui::Button::new("Check loaded data against report"),
                )
                .clicked();
        });
        if restore {
            self.prepare_report_inputs(ui.ctx(), true);
        } else if verify {
            self.prepare_report_inputs(ui.ctx(), false);
        }
    }
}
