//! Workers responsibilities for the review workspace.
use super::*;

/// GUI ownership is nonblocking even when a player is discarded implicitly by
/// an obsolete reply, failed send, or receiver teardown. Backend AudioPlayback
/// keeps its ordinary synchronous stop/Drop contract for other callers.
pub(super) struct PlaybackOwner(Option<AudioPlayback>);
impl PlaybackOwner {
    pub(super) fn new(player: AudioPlayback) -> Self {
        Self(Some(player))
    }
    pub(super) fn is_playing(&mut self) -> Result<bool, String> {
        self.0.as_mut().map_or(Ok(false), AudioPlayback::is_playing)
    }
    pub(super) fn stop(&mut self) -> Result<(), String> {
        self.0
            .take()
            .map_or(Ok(()), AudioPlayback::stop_in_background)
    }
    pub(super) fn stop_for_exit(mut self) -> Result<(), String> {
        self.0.take().map_or(Ok(()), AudioPlayback::stop_for_exit)
    }
}
impl Drop for PlaybackOwner {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub(super) enum ResultValue {
    WideReady(Box<wide_ui::WideReply>),
    WideProgress(u64, geosonnar::wide_area_pair::WideProgress),
    Spectrum(SpectrogramWindow),
    Frame(Option<VideoFrame>),
    Saved(PathBuf, ReviewWorkspace),
    Loaded(ReviewWorkspace, Box<ReviewWorkspace>),
    InvestigationLoaded(
        Box<geosonnar::investigation_archive::LoadedInvestigation>,
        Box<ReviewWorkspace>,
    ),
    InvestigationInputs(Box<investigation::InputsReply>),
    RunRecorded(Box<history_runs::RunCompletion>),
    RunHistoryRetried(Box<history_runs::RunCompletion>),
    RunJournalNotice(u64, String),
    RunHistoryListed(Vec<geosonnar::regional_history::RunSummary>),
    RunHistoryOpened(
        Box<geosonnar::regional_history::LoadedRun>,
        Box<ReviewWorkspace>,
    ),
    RunHistoryChanged(String),
    RunHistoryDeleted(String),
    RunHistoryChecked(Box<history_runs::CheckedInputs>),
    JointMeasured(Box<joint::MeasurementReply>),
    JointEstimated(Box<joint::EstimateReply>),
    JointProgress(u64, geosonnar::joint_evidence::JointProgress),
    JointStopped(u64, String),
    JointExported(PathBuf),
    Playing(PlaybackOwner, f64, u64),
    Layer(map_data::MapData, SourceBinding),
    Lightning(evidence_score::LightningCatalog),
    Catalog(Vec<data_catalog::CatalogItem>),
    DataReady(String),
    DatasetDeleted(Box<library::DeletionReply>),
    Planned(data_catalog::AdsbPlan),
    Progress(data_catalog::DownloadProgress),
    HistoryReady(Box<data_catalog::HistoryItem>),
    HistoryProgress(u64, geosonnar::adsblol_history::HistoryProgress),
    FmiReady(Box<fmi_ui::FmiReply>),
    FmiProgress(u64, data_catalog::fmi::FmiProgress),
    Scored(Box<evidence_score::ScoreReport>, ReviewWorkspace),
    RegionalEstimated(
        geosonnar::regional_search::SearchEstimate,
        Box<ReviewWorkspace>,
    ),
    RegionalProgress(u64, geosonnar::regional_search::SearchProgress),
    #[cfg(test)]
    RegionalSearched(
        Box<geosonnar::regional_search::RegionalReport>,
        Box<ReviewWorkspace>,
    ),
    RegionalApplied(Box<ReviewWorkspace>, Box<ReviewWorkspace>),
    PredictionInspected(Box<inspection::PreparedInspection>),
    RegionalSaved(PathBuf, Box<ReviewWorkspace>),
    RegionalComparisonEstimated(
        geosonnar::regional_search::ModalityEstimate,
        Box<ReviewWorkspace>,
    ),
    RegionalComparisonProgress(u64, geosonnar::regional_search::ModalityProgress),
    HeightPreview(
        Box<geosonnar::receiver_heights::ReceiverHeightReport>,
        Box<ReviewWorkspace>,
    ),
    Atmosphere(geosonnar::atmosphere::AtmosphereProfile, ReviewWorkspace),
    RestoredLayers(
        Vec<map_data::MapData>,
        Option<evidence_score::LightningCatalog>,
        ReviewWorkspace,
    ),
}
#[derive(Clone, Copy)]
pub(super) enum WorkerLane {
    Task,
    FmiTask(u64),
    JointTask(u64),
    WideTask(u64),
    DataDeleteTask(u64),
    Frame,
    Progress,
    SearchProgress,
    TransferProgress,
}
pub(super) struct Reply {
    pub(super) generation: u64,
    pub(super) lane: WorkerLane,
    pub(super) result: Result<ResultValue, String>,
}
impl Studio {
    pub fn active_for_exit(&self) -> bool {
        self.busy()
            || self.frame_busy
            || self.playback.is_some()
            || self.play_start.is_some()
            || self.regional.running
            || self.joint.running
            || self.wide.running
            || self.data.downloading.is_some()
            || self.progress_busy
    }
    /// Stop accepting new work while the process owner performs bounded exit.
    pub fn cancel_for_exit(&mut self) {
        self.exiting = true;
        self.regional.cancel.store(true, Ordering::Relaxed);
        self.joint.cancel.store(true, Ordering::Relaxed);
        self.wide.cancel.store(true, Ordering::Relaxed);
        self.download_cancel.store(true, Ordering::Relaxed);
        self.map.cancel_for_exit();
        self.frame_requested = false;
        self.data.refresh_requested = false;
        self.pending_action = None;
        self.playback_generation = self.playback_generation.wrapping_add(1);
        if let Some(playback) = self.playback.take() {
            let _ = playback.stop_for_exit();
        }
        self.play_start = None;
    }
    pub(super) fn spawn<F>(&mut self, ctx: &egui::Context, label: &str, task: F)
    where
        F: FnOnce() -> Result<ResultValue, String> + Send + 'static,
    {
        if self.busy() || self.exiting {
            return;
        }
        self.busy = Some(label.into());
        self.notice = None;
        self.spawn_lane(ctx, WorkerLane::Task, task);
    }
    pub(super) fn spawn_lane<F>(&self, ctx: &egui::Context, lane: WorkerLane, task: F)
    where
        F: FnOnce() -> Result<ResultValue, String> + Send + 'static,
    {
        if self.exiting {
            return;
        }
        let sender = self.sender.clone();
        let generation = self.generation;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task))
                .unwrap_or_else(|_| Err("Review worker stopped unexpectedly".into()));
            let _ = sender.send(Reply {
                generation,
                lane,
                result,
            });
            ctx.request_repaint();
        });
    }
    pub fn tick(&mut self, ctx: &egui::Context) {
        if self.exiting {
            // A late player needs exit-only disposal too: its normal Drop can
            // wait for diagnostics and must not run on this shutdown UI path.
            while let Ok(reply) = self.receiver.try_recv() {
                if let Ok(ResultValue::Playing(playback, _, _)) = reply.result {
                    let _ = playback.stop_for_exit();
                }
            }
            return;
        }
        self.invalidate_changed_inspection();
        while let Ok(reply) = self.receiver.try_recv() {
            if reply.generation != self.generation {
                if let Ok(ResultValue::RunRecorded(completion)) = reply.result {
                    self.accept_stale_run_completion(*completion);
                }
                continue;
            }
            if let WorkerLane::FmiTask(run_id) = reply.lane
                && run_id != self.data.fmi.run_id
            {
                // An old request cannot clear the activity owned by a newer run.
                continue;
            }
            if let WorkerLane::JointTask(run_id) = reply.lane
                && run_id != self.joint.run_id
            {
                if let Ok(ResultValue::RunRecorded(completion)) = reply.result {
                    self.accept_stale_run_completion(*completion);
                }
                continue;
            }
            if let WorkerLane::DataDeleteTask(run_id) = reply.lane
                && !self.data.owns_deletion(run_id)
            {
                continue;
            }
            if let WorkerLane::WideTask(run_id) = reply.lane
                && run_id != self.wide.run_id
            {
                continue;
            }
            if let Ok(ResultValue::DatasetDeleted(deletion)) = &reply.result
                && !self.owns_dataset_deletion_reply(deletion)
            {
                continue;
            }
            match reply.lane {
                WorkerLane::Task
                | WorkerLane::FmiTask(_)
                | WorkerLane::JointTask(_)
                | WorkerLane::WideTask(_)
                | WorkerLane::DataDeleteTask(_) => self.busy = None,
                WorkerLane::Frame => self.frame_busy = false,
                WorkerLane::Progress => self.progress_busy = false,
                WorkerLane::SearchProgress | WorkerLane::TransferProgress => {}
            }
            match reply.result {
                Err(e) => {
                    if matches!(
                        reply.lane,
                        WorkerLane::Task
                            | WorkerLane::FmiTask(_)
                            | WorkerLane::JointTask(_)
                            | WorkerLane::WideTask(_)
                    ) {
                        self.regional.running = false;
                        self.joint.running = false;
                        self.wide.running = false;
                        self.data.downloading = None;
                        self.data.refresh_requested = true;
                    }
                    self.notice = Some((true, e));
                    if matches!(reply.lane, WorkerLane::Frame) {
                        self.frame_requested = false;
                    }
                }
                Ok(ResultValue::WideReady(reply)) => self.accept_wide_reply(*reply),
                Ok(ResultValue::WideProgress(id, progress)) => {
                    if id == self.wide.run_id && self.wide.running {
                        self.wide.progress = Some(progress);
                    }
                }
                Ok(ResultValue::Spectrum(window)) => {
                    if self.document.as_ref().is_none_or(|d| {
                        d.recording_sha256 != window.source_sha256 || d.channel != window.channel
                    }) {
                        self.notice = Some((
                            true,
                            "Recording or channel changed; reload the case before review".into(),
                        ));
                        continue;
                    }
                    self.texture = Some(spectrogram::texture(
                        ctx,
                        &window,
                        self.view.db_min,
                        self.view.db_max,
                    ));
                    self.view.time_min_s = window.start_s;
                    self.view.time_max_s = window.end_s;
                    if let Some(bounds) = spectrogram::frequency_bounds(&window) {
                        self.view.set_frequency_range(
                            self.view.frequency_min_hz,
                            self.view.frequency_max_hz,
                            bounds,
                        );
                    }
                    self.window = Some(window);
                    self.frame_requested = true;
                }
                Ok(ResultValue::Frame(frame)) => {
                    if self.frame_requested && self.playback.is_none() {
                        continue;
                    }
                    self.video_available = Some(frame.is_some());
                    if let Some(frame) = frame {
                        self.video_time = Some(frame.frame_time_s);
                        self.video = Some(ctx.load_texture(
                            "video-frame",
                            egui::ColorImage::from_rgb([frame.width, frame.height], &frame.rgb),
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
                Ok(ResultValue::Saved(path, doc)) => {
                    self.saved = Some(doc);
                    self.notice = Some((
                        false,
                        format!("Saved review and trace exports: {}", display_path(&path)),
                    ));
                }
                Ok(ResultValue::InvestigationLoaded(loaded, previous)) => {
                    self.accept_investigation(*loaded, *previous)
                }
                Ok(ResultValue::InvestigationInputs(reply)) => self.accept_report_inputs(*reply),
                Ok(ResultValue::RunRecorded(completion)) => self.accept_run_completion(*completion),
                Ok(ResultValue::RunHistoryRetried(completion)) => {
                    self.accept_run_retry(*completion)
                }
                Ok(ResultValue::JointMeasured(reply)) => self.accept_joint_measurement(*reply),
                Ok(ResultValue::JointEstimated(reply)) => self.accept_joint_estimate(*reply),
                Ok(ResultValue::JointProgress(id, progress)) => {
                    self.accept_joint_progress(id, progress)
                }
                Ok(ResultValue::JointStopped(id, error)) => self.accept_joint_stopped(id, error),
                Ok(ResultValue::JointExported(path)) => {
                    self.notice = Some((
                        false,
                        format!(
                            "Saved immutable shared-cell result: {}",
                            display_path(&path)
                        ),
                    ));
                }
                Ok(ResultValue::RunJournalNotice(run_id, message)) => {
                    if run_id == self.regional.run_id {
                        self.run_history.status = Some((true, message.clone()));
                        self.notice = Some((true, message));
                    }
                }
                Ok(ResultValue::RunHistoryListed(rows)) => self.run_history.rows = rows,
                Ok(ResultValue::RunHistoryOpened(loaded, previous)) => {
                    self.accept_history_run(*loaded, *previous)
                }
                Ok(ResultValue::RunHistoryChanged(message)) => {
                    self.run_history.rows.clear();
                    self.invalidate_run_history_list();
                    self.notice = Some((false, message));
                }
                Ok(ResultValue::RunHistoryChecked(checked)) => self.accept_history_inputs(*checked),
                Ok(ResultValue::RunHistoryDeleted(id)) => self.accept_history_deletion(&id),
                Ok(ResultValue::Loaded(doc, previous)) => {
                    if self.document.as_ref() != Some(previous.as_ref()) {
                        self.notice=Some((true,"Review changed while loading. Your edits were kept; open the saved review again after saving or discarding them.".into()));
                        continue;
                    }
                    self.stop();
                    self.saved = Some(doc.clone());
                    self.document = Some(doc);
                    self.undo.clear();
                    self.redo.clear();
                    self.selected = None;
                    self.window = None;
                    self.texture = None;
                    self.video = None;
                    self.video_available = None;
                    self.layers.clear();
                    self.lightning = None;
                    self.report_data_changed();
                    self.investigation = investigation::InvestigationState::default();
                    self.score = None;
                    self.clear_history_display_binding();
                    self.regional.reset_results();
                    self.inspection = inspection::InspectionState::default();
                    self.pending_action = None;
                    self.heights = receiver_ui::ReceiverHeightState::default();
                    self.notice=Some((false,"Review restored. Reload its data layers from the library to compare locations.".into()));
                }
                Ok(ResultValue::Playing(player, start, token)) => {
                    if token != self.playback_generation {
                        drop(player);
                        continue;
                    }
                    self.cursor_s = start;
                    self.play_start = Some((Instant::now(), start));
                    self.playback = Some(player);
                }
                Ok(ResultValue::Layer(layer, binding)) => {
                    self.add_layer(layer, binding);
                }
                Ok(ResultValue::Lightning(catalog)) => {
                    self.restore_deleted_source(&SourceBinding {
                        path: catalog.input_path.clone(),
                        sha256: catalog.input_sha256.clone(),
                    });
                    if let Some(old) = &self.lightning {
                        let sha = old.input_sha256.clone();
                        self.forget_dataset(&sha);
                    }
                    if let Some(doc) = &mut self.document {
                        let binding = SourceBinding {
                            path: catalog.input_path.clone(),
                            sha256: catalog.input_sha256.clone(),
                        };
                        if let Err(error) = doc.register_dataset(
                            binding,
                            None,
                            catalog.provenance.altitude_datum
                                == evidence_score::LightningAltitudeDatum::Wgs84Ellipsoid,
                        ) {
                            self.notice = Some((true, error));
                            continue;
                        }
                    }
                    self.lightning = Some(catalog);
                    self.report_data_changed();
                    self.score = None;
                }
                Ok(ResultValue::Catalog(rows)) => {
                    self.data.items = rows.into_iter().map(Arc::new).collect();
                }
                Ok(ResultValue::DataReady(message)) => {
                    self.notice = Some((false, message));
                    self.data.refresh_requested = true;
                    self.data.downloading = None;
                }
                Ok(ResultValue::DatasetDeleted(reply)) => self.accept_dataset_deletion(*reply),
                Ok(ResultValue::Planned(plan)) => {
                    self.data.plan = Some(plan);
                    self.data.refresh_requested = true;
                }
                Ok(ResultValue::Progress(progress)) => self.data.progress = Some(progress),
                Ok(ResultValue::HistoryProgress(run_id, progress)) => {
                    if run_id == self.data.history.run_id {
                        self.data.history.progress = Some(progress);
                    }
                }
                Ok(ResultValue::HistoryReady(item)) => {
                    self.data.history.accept_item(*item);
                    self.data.show_history();
                    self.data.downloading = None;
                    self.data.refresh_requested = true;
                    self.notice=Some((false,"Historical request is ready to review in Data. Check coverage, omissions and source policy before explicitly importing traces. Aircraft associations remain unchanged.".into()));
                }
                Ok(ResultValue::FmiReady(reply)) => self.accept_fmi_reply(*reply),
                Ok(ResultValue::FmiProgress(run_id, progress)) => {
                    if run_id == self.data.fmi.run_id
                        && self.data.downloading.as_deref() == Some("fmi")
                    {
                        self.data.fmi.progress = Some(progress);
                    }
                }
                Ok(ResultValue::Scored(report, doc)) => {
                    if self.document.as_ref() == Some(&doc) {
                        self.inspection.clear(
                            "The fixed comparison changed; inspect its selected receiver again.",
                        );
                        self.pending_action = None;
                        self.score = Some(*report);
                        self.score_document = Some(doc);
                        self.regional.show_results = false;
                        self.notice=Some((false,"Comparison completed using the selected observations and weights. Inspect individual residuals below.".into()));
                    } else {
                        self.notice = Some((
                            true,
                            "Evidence changed during scoring. Run the comparison again.".into(),
                        ));
                    }
                }
                Ok(ResultValue::RegionalEstimated(estimate, doc)) => {
                    let current = self.document.as_ref() == Some(doc.as_ref());
                    self.regional
                        .accept_estimate(estimate, *doc, self.document.as_ref());
                    self.notice = Some(if current {
                        (
                            false,
                            "Regional plan checked. Review the work estimate, then run when ready."
                                .into(),
                        )
                    } else {
                        (
                            true,
                            "Plan changed while estimating. Estimate the current plan again."
                                .into(),
                        )
                    });
                }
                Ok(ResultValue::RegionalProgress(run_id, progress)) => {
                    if run_id == self.regional.run_id {
                        self.regional.progress = Some(progress);
                    }
                }
                Ok(ResultValue::RegionalComparisonEstimated(estimate, doc)) => {
                    let current = self.document.as_ref() == Some(doc.as_ref());
                    self.regional.accept_comparison_estimate(
                        estimate,
                        *doc,
                        self.document.as_ref(),
                    );
                    self.notice = Some(if current {
                        (false,"Independent-mode work estimate ready. Review all requested modes before running.".into())
                    } else {
                        (
                            true,
                            "Plan changed while estimating. Estimate the current plan again."
                                .into(),
                        )
                    });
                }
                Ok(ResultValue::RegionalComparisonProgress(run_id, progress)) => {
                    if run_id == self.regional.run_id {
                        self.regional.comparison_progress = Some(progress);
                    }
                }
                #[cfg(test)]
                Ok(ResultValue::RegionalSearched(report, doc)) => {
                    self.regional.running = false;
                    if self.document.as_ref() == Some(doc.as_ref()) {
                        self.investigation = investigation::InvestigationState::default();
                        self.clear_history_display_binding();
                        self.regional.comparison = None;
                        self.regional.progress = Some(report.progress.clone());
                        self.regional.report = Some(*report);
                        self.regional.report_document = Some(*doc);
                        self.regional.clear_selection();
                        self.regional.show_results = true;
                        self.notice = Some((false, "Regional search returned. Open Results to review location, time and association alternatives; nothing has been applied.".into()));
                    } else {
                        self.notice = Some((true, "Evidence changed during regional search. The result was not applied; estimate and run again.".into()));
                    }
                }
                Ok(ResultValue::RegionalApplied(next, previous)) => {
                    if self.document.as_ref() == Some(previous.as_ref()) {
                        self.document = Some(*next);
                        self.checkpoint(*previous);
                        self.notice = Some((false, "Applied the selected clock and source associations as one undoable edit. Review them or run the fixed comparison for full residuals.".into()));
                    } else {
                        self.notice = Some((true, "Evidence changed while applying the hypothesis. Your newer edits were kept.".into()));
                    }
                }
                Ok(ResultValue::PredictionInspected(prepared)) => {
                    self.accept_inspection(*prepared, ctx)
                }
                Ok(ResultValue::RegionalSaved(path, doc)) => {
                    self.saved = Some(*doc);
                    self.notice = Some((
                        false,
                        format!(
                            "Saved regional investigation, evidence and source identities. Open the report: {}",
                            display_path(&path)
                        ),
                    ));
                }
                Ok(ResultValue::HeightPreview(report, previous)) => {
                    if self.document.as_ref() == Some(previous.as_ref())
                        && self.heights.draft == report.settings
                    {
                        self.heights.preview = Some(*report);
                        self.heights.preview_document = Some(*previous);
                    } else {
                        self.notice = Some((true, "Height settings changed during coverage checking. Preview the current settings again.".into()));
                    }
                }
                Ok(ResultValue::Atmosphere(profile, previous)) => {
                    if self.document.as_ref() != Some(&previous) {
                        self.notice = Some((true, "Review changed while the atmospheric profile was loading. Your edits were kept; import the profile again.".into()));
                        continue;
                    }
                    self.checkpoint(previous);
                    self.document.as_mut().unwrap().atmosphere = Some(profile);
                    self.score = None;
                    self.notice = Some((false, "Atmospheric profile loaded for local refinement. Save the review to keep this exact profile with its source record.".into()));
                }
                Ok(ResultValue::RestoredLayers(layers, lightning, previous)) => {
                    if self.document.as_ref() != Some(&previous) {
                        self.notice=Some((true,"Review changed while data layers were loading. Your edits were kept; restore the layers again.".into()));
                        continue;
                    }
                    for binding in &previous.datasets {
                        self.restore_deleted_source(binding);
                    }
                    self.layers = layers;
                    self.lightning = lightning;
                    self.report_data_changed();
                    self.score = None;
                    self.notice = Some((
                        false,
                        "Restored the saved data layers and their UTC/altitude assumptions.".into(),
                    ));
                }
            }
        }
        if let Some((started, origin)) = self.play_start {
            self.cursor_s = origin + started.elapsed().as_secs_f64();
            let active = self
                .playback
                .as_mut()
                .is_some_and(|p| p.is_playing().unwrap_or(false));
            if !active || self.cursor_s >= self.window_end {
                self.cursor_s = self.cursor_s.min(self.window_end);
                self.stop();
            } else if Instant::now() >= self.next_frame {
                self.frame_requested = true;
                self.next_frame = Instant::now() + Duration::from_millis(600);
            }
            ctx.request_repaint_after(Duration::from_millis(40));
        }
        if self.frame_requested
            && !self.frame_busy
            && self.video_available != Some(false)
            && !self.tools.ffmpeg.as_os_str().is_empty()
            && let Some(case) = &self.case
        {
            self.frame_requested = false;
            self.frame_busy = true;
            let request = review_media::FrameRequest {
                input: case.recording_path.clone(),
                ffmpeg: self.tools.ffmpeg.clone(),
                ffprobe: self.tools.ffprobe.clone(),
                time_s: self.cursor_s,
                max_width: 960,
                max_height: 540,
                expected_source: self.window.as_ref().map(|w| w.source_stamp.clone()),
            };
            self.spawn_lane(ctx, WorkerLane::Frame, move || {
                review_media::extract_frame(&request).map(ResultValue::Frame)
            });
        }
        self.poll_library(ctx);
    }
    pub(super) fn stop(&mut self) {
        self.playback_generation = self.playback_generation.wrapping_add(1);
        if let Some(mut playback) = self.playback.take() {
            let _ = playback.stop();
        }
        self.play_start = None;
    }
}
