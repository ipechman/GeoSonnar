//! Manual review workspace. Processing runs only after an explicit action.
mod acoustic_review;
mod atmosphere;
mod fit_comparison;
mod fmi_ui;
mod history_runs;
mod history_ui;
mod inspection;
mod investigation;
mod joint;
mod library;
mod map;
mod map_display;
mod presets;
mod receiver_ui;
mod regional;
#[cfg(test)]
mod tests;
mod tiles;
mod wide_ui;
use super::{
    spectrogram::{self, EditTool, PlotView},
    widgets::*,
};
use eframe::egui::{self, Color32, RichText};
use geosonnar::{
    data_catalog,
    discovery::DiscoverySettings,
    event_review::ReviewSession,
    evidence_score,
    gui_bridge::CaseOverview,
    map_data,
    review_media::{self, AudioPlayback, SpectrogramWindow, VideoFrame},
    thunder,
    workspace::{self, EventKind, EvidenceEvent, ReviewState, ReviewWorkspace, SourceBinding},
};
use presets::{DopplerPreset, ThunderPreset};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub ffplay: PathBuf,
}

/// GUI ownership is nonblocking even when a player is discarded implicitly by
/// an obsolete reply, failed send, or receiver teardown. Backend AudioPlayback
/// keeps its ordinary synchronous stop/Drop contract for other callers.
struct PlaybackOwner(Option<AudioPlayback>);
impl PlaybackOwner {
    fn new(player: AudioPlayback) -> Self {
        Self(Some(player))
    }
    fn is_playing(&mut self) -> Result<bool, String> {
        self.0.as_mut().map_or(Ok(false), AudioPlayback::is_playing)
    }
    fn stop(&mut self) -> Result<(), String> {
        self.0
            .take()
            .map_or(Ok(()), AudioPlayback::stop_in_background)
    }
    fn stop_for_exit(mut self) -> Result<(), String> {
        self.0.take().map_or(Ok(()), AudioPlayback::stop_for_exit)
    }
}
impl Drop for PlaybackOwner {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub enum Action {
    ReviewInspection,
    ReviewEvidence,
    ReviewResults,
    OpenDiscovery,
    PrepareDiscovery(DiscoverySettings),
    LegacyReview,
}
enum ResultValue {
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
enum WorkerLane {
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
struct Reply {
    generation: u64,
    lane: WorkerLane,
    result: Result<ResultValue, String>,
}
pub struct Studio {
    /// Presentation only: never changes evidence, search mode or parameters.
    pub detailed_controls: bool,
    exiting: bool,
    case: Option<CaseOverview>,
    pub document: Option<ReviewWorkspace>,
    saved: Option<ReviewWorkspace>,
    undo: Vec<ReviewWorkspace>,
    redo: Vec<ReviewWorkspace>,
    pub tools: Tools,
    selected: Option<usize>,
    compact_inspector: bool,
    window: Option<SpectrogramWindow>,
    texture: Option<egui::TextureHandle>,
    video: Option<egui::TextureHandle>,
    video_time: Option<f64>,
    video_available: Option<bool>,
    video_overlay: bool,
    view: PlotView,
    window_start: f64,
    window_end: f64,
    fft_size: usize,
    hop_s: f64,
    spectrum_max_hz: f64,
    tool: EditTool,
    cursor_s: f64,
    playback_generation: u64,
    pending_undo: Option<ReviewWorkspace>,
    playback: Option<PlaybackOwner>,
    play_start: Option<(Instant, f64)>,
    frame_busy: bool,
    frame_requested: bool,
    next_frame: Instant,
    busy: Option<String>,
    notice: Option<(bool, String)>,
    generation: u64,
    sender: Sender<Reply>,
    receiver: Receiver<Reply>,
    discovery: DiscoverySettings,
    thunder_settings: thunder::ThunderConfig,
    show_doppler: bool,
    show_thunder: bool,
    data: library::LibraryState,
    map: map::MapState,
    layers: Vec<map_data::MapData>,
    lightning: Option<evidence_score::LightningCatalog>,
    score: Option<evidence_score::ScoreReport>,
    score_document: Option<ReviewWorkspace>,
    regional: regional::RegionalState,
    joint: joint::JointState,
    wide: wide_ui::WideState,
    inspection: inspection::InspectionState,
    investigation: investigation::InvestigationState,
    run_history: history_runs::HistoryState,
    loaded_data_revision: u64,
    pending_action: Option<Action>,
    heights: receiver_ui::ReceiverHeightState,
    download_cancel: Arc<AtomicBool>,
    progress_busy: bool,
    next_progress: Instant,
}
impl Default for Studio {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            detailed_controls: false,
            exiting: false,
            case: None,
            document: None,
            saved: None,
            undo: vec![],
            redo: vec![],
            tools: Tools::default(),
            selected: None,
            compact_inspector: false,
            window: None,
            texture: None,
            video: None,
            video_time: None,
            video_available: None,
            video_overlay: true,
            view: PlotView {
                time_min_s: 0.0,
                time_max_s: 60.0,
                frequency_min_hz: 0.0,
                frequency_max_hz: 800.0,
                db_min: -110.0,
                db_max: -35.0,
            },
            window_start: 0.0,
            window_end: 60.0,
            fft_size: 4096,
            hop_s: 0.125,
            spectrum_max_hz: 800.0,
            tool: EditTool::Inspect,
            cursor_s: 0.0,
            playback_generation: 0,
            pending_undo: None,
            playback: None,
            play_start: None,
            frame_busy: false,
            frame_requested: false,
            next_frame: Instant::now(),
            busy: None,
            notice: None,
            generation: 0,
            sender,
            receiver,
            discovery: DiscoverySettings::default(),
            thunder_settings: thunder::ThunderConfig::default(),
            show_doppler: true,
            show_thunder: true,
            data: library::LibraryState::default(),
            map: map::MapState::default(),
            layers: vec![],
            lightning: None,
            score: None,
            score_document: None,
            regional: regional::RegionalState::default(),
            joint: joint::JointState::default(),
            wide: wide_ui::WideState::default(),
            inspection: inspection::InspectionState::default(),
            investigation: investigation::InvestigationState::default(),
            run_history: history_runs::HistoryState::default(),
            loaded_data_revision: 0,
            pending_action: None,
            heights: receiver_ui::ReceiverHeightState::default(),
            download_cancel: Arc::new(AtomicBool::new(false)),
            progress_busy: false,
            next_progress: Instant::now(),
        }
    }
}
impl Studio {
    pub fn take_pending_action(&mut self) -> Option<Action> {
        self.pending_action.take()
    }
    pub fn bind(&mut self, case: &CaseOverview) {
        if self
            .case
            .as_ref()
            .is_some_and(|c| c.manifest == case.manifest)
        {
            self.case = Some(case.clone());
            return;
        }
        self.stop();
        self.regional.cancel.store(true, Ordering::Relaxed);
        self.joint.cancel.store(true, Ordering::Relaxed);
        self.wide.cancel.store(true, Ordering::Relaxed);
        self.download_cancel.store(true, Ordering::Relaxed);
        let generation = self.generation + 1;
        let tools = self.tools.clone();
        let detailed_controls = self.detailed_controls;
        *self = Self::default();
        self.generation = generation;
        self.tools = tools;
        self.detailed_controls = detailed_controls;
        self.case = Some(case.clone());
        let doc = ReviewWorkspace::new(&case.case, 0);
        self.window_end = doc.duration_s.min(60.0);
        self.view.time_max_s = self.window_end;
        self.saved = Some(doc.clone());
        self.document = Some(doc);
    }
    pub fn dirty(&self) -> bool {
        self.document != self.saved
    }
    pub fn busy(&self) -> bool {
        self.busy.is_some()
    }
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
    #[cfg(test)]
    pub(super) fn assert_unloaded_idle(&self) {
        assert!(self.case.is_none() && self.document.is_none());
        assert!(self.window.is_none() && self.texture.is_none() && self.video.is_none());
        assert!(self.layers.is_empty() && self.lightning.is_none());
        assert!(self.playback.is_none() && self.play_start.is_none());
        assert!(!self.busy() && !self.frame_busy && !self.frame_requested && !self.progress_busy);
        assert!(!self.regional.running && self.data.downloading.is_none());
        assert!(!self.joint.running);
        assert!(!self.wide.running);
        assert!(self.receiver.try_recv().is_err());
    }
    pub fn channel(&self) -> usize {
        self.document.as_ref().map(|d| d.channel).unwrap_or(0)
    }
    pub fn case_timing_ui(&mut self, ui: &mut egui::Ui) {
        let Some(before) = self.document.clone() else {
            return;
        };
        card(ui, |ui| {
            ui.strong("Recording start time · optional");
            let document = self.document.as_mut().unwrap();
            ui.horizontal_wrapped(|ui| {
                ui.label("UTC");
                accessible_text(
                    ui,
                    "case_recording_start_utc",
                    &mut document.recording_start_utc,
                    "YYYY-MM-DD HH:MM:SS",
                    false,
                    270.0,
                );
            });
            muted(
                ui,
                "Leave blank until known. This time is needed to align the recording with aircraft tracks and lightning strikes.",
            );
            if !document.recording_start_utc.trim().is_empty()
                && let Err(error) = workspace::parse_utc(&document.recording_start_utc)
            {
                ui.colored_label(tone(ui, AMBER), error);
            }
            ui.horizontal_wrapped(|ui| {
                let dirty = self.dirty();
                let time = &self.document.as_ref().unwrap().recording_start_utc;
                let valid_time = time.trim().is_empty() || workspace::parse_utc(time).is_ok();
                if ui
                    .add_enabled(
                        dirty && valid_time && !self.busy(),
                        egui::Button::new("Save timing + review"),
                    )
                    .on_hover_text(
                        "Save this UTC time and the complete current review as a new snapshot.",
                    )
                    .clicked()
                {
                    self.save(ui.ctx());
                }
                if self.busy() {
                    ui.spinner();
                    ui.label("Working…");
                } else if dirty {
                    ui.colored_label(tone(ui, AMBER), "Unsaved changes");
                } else {
                    muted(ui, "No unsaved changes");
                }
            });
            if let Some((true, error)) = &self.notice {
                ui.colored_label(tone(ui, RED), error);
            }
        });
        if self.document.as_ref() != Some(&before) {
            self.checkpoint(before);
        }
    }
    pub fn import_review(&mut self, session: &ReviewSession) -> Result<(), String> {
        let incoming = session.overview().selected_channel_index;
        let mut doc = self.document.clone().ok_or("Open a case first")?;
        let changed_channel = doc.events.is_empty() && doc.channel != incoming;
        if doc.events.is_empty() {
            doc.channel = incoming;
        }
        let count = doc.import_discovery(session)?;
        let before = self.document.replace(doc).unwrap();
        self.checkpoint(before);
        if changed_channel {
            self.stop();
            self.window = None;
            self.texture = None;
            self.video = None;
            self.video_available = None;
        }
        self.notice = Some((
            false,
            format!("Added {count} Doppler candidates. Review and select evidence explicitly."),
        ));
        if self.selected.is_none() && !self.document.as_ref().unwrap().events.is_empty() {
            self.selected = Some(0);
            self.focus_selected();
        }
        Ok(())
    }
    fn spawn<F>(&mut self, ctx: &egui::Context, label: &str, task: F)
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
    fn spawn_lane<F>(&self, ctx: &egui::Context, lane: WorkerLane, task: F)
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
    fn stop(&mut self) {
        self.playback_generation = self.playback_generation.wrapping_add(1);
        if let Some(mut playback) = self.playback.take() {
            let _ = playback.stop();
        }
        self.play_start = None;
    }
    fn checkpoint(&mut self, before: ReviewWorkspace) {
        if let Some(doc) = &mut self.document {
            acoustic_review::reset_edited_measurements(&before, doc);
        }
        self.inspection
            .clear("Evidence or assumptions changed; inspect the candidate again.");
        self.undo.push(before);
        if self.undo.len() > 60 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.score = None;
    }
    fn undo(&mut self) {
        if let Some(old) = self.undo.pop() {
            self.inspection
                .clear("Review changed by Undo; inspect the candidate again.");
            if let Some(current) = self.document.replace(old) {
                self.redo.push(current);
            }
            self.score = None;
            self.selected = self
                .selected
                .filter(|&i| i < self.document.as_ref().unwrap().events.len());
        }
    }
    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.inspection
                .clear("Review changed by Redo; inspect the candidate again.");
            if let Some(current) = self.document.replace(next) {
                self.undo.push(current);
            }
            self.score = None;
            self.selected = self
                .selected
                .filter(|&index| index < self.document.as_ref().unwrap().events.len());
        }
    }
    fn save(&mut self, ctx: &egui::Context) {
        if let (Some(case), Some(doc)) = (&self.case, &self.document) {
            let manifest = case.manifest.clone();
            let doc = doc.clone();
            self.spawn(ctx, "Saving reviewed evidence…", move || {
                workspace::save(&manifest, &doc).map(|p| ResultValue::Saved(p, doc))
            });
        }
    }
    fn load(&mut self, ctx: &egui::Context) {
        if self.dirty() || self.has_unsaved_results() {
            self.notice = Some((
                true,
                "Save or discard your current edits before opening another review.".into(),
            ));
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Review workspace", &["json"])
            .pick_file()
        {
            let manifest = self.case.as_ref().unwrap().manifest.clone();
            let previous = self.document.clone().unwrap();
            self.spawn(ctx, "Checking saved review…", move || {
                workspace::load(&manifest, &path)
                    .map(|doc| ResultValue::Loaded(doc, Box::new(previous)))
            });
        }
    }
    pub fn toolbar(&mut self, ui: &mut egui::Ui) {
        self.invalidate_changed_inspection();
        if self.document.is_none() {
            ui.heading("Open a case to start reviewing");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!self.busy(), |ui| {
                if primary(ui, "Save review + traces").clicked() {
                    self.save(ui.ctx());
                }
                if self.detailed_controls {
                    self.review_file_actions(ui);
                } else {
                    ui.menu_button("Review files", |ui| self.review_file_actions(ui));
                }
                if ui
                    .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                    .clicked()
                {
                    self.undo();
                }
                if ui
                    .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                    .clicked()
                {
                    self.redo();
                }
            });
            ui.separator();
            ui.label(if self.dirty() {
                "Unsaved edits"
            } else {
                "Review saved"
            });
            if self.has_unsaved_results() {
                ui.colored_label(tone(ui, RED), "Result not saved — see Results");
            }
            if let Some(status) = &self.busy {
                ui.spinner();
                ui.label(status);
            }
        });
        if let Some((error, text)) = self.notice.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(display_path_text(&text))
                        .color(tone(ui, if error { RED } else { ACCENT })),
                );
                if ui.small_button("Dismiss").clicked() {
                    self.notice = None;
                }
            });
        }
        self.regional_progress_ui(ui);
        self.wide_progress_ui(ui);
        self.deletion_readiness_ui(ui);
        ui.separator();
    }
    fn review_file_actions(&mut self, ui: &mut egui::Ui) {
        if ui.button("Open saved review…").clicked() {
            self.load(ui.ctx());
            if !self.detailed_controls {
                ui.close();
            }
        }
        if ui.button("Open saved investigation…").clicked() {
            self.open_investigation(ui.ctx());
            if !self.detailed_controls {
                ui.close();
            }
        }
        if ui
            .add_enabled(self.dirty(), egui::Button::new("Discard unsaved edits"))
            .clicked()
        {
            self.document = self.saved.clone();
            self.selected = None;
            self.undo.clear();
            self.redo.clear();
            self.score = None;
            if !self.detailed_controls {
                ui.close();
            }
        }
    }
    fn load_window(&mut self, ctx: &egui::Context) {
        self.stop();
        let Some(case) = &self.case else { return };
        let request = review_media::WindowRequest {
            input: case.recording_path.clone(),
            ffmpeg: self.tools.ffmpeg.clone(),
            ffprobe: self.tools.ffprobe.clone(),
            channel: self.channel(),
            start_s: self.window_start,
            end_s: self.window_end,
            fft_size: self.fft_size,
            hop_s: self.hop_s,
            max_frequency_hz: self.spectrum_max_hz,
        };
        self.spawn(
            ctx,
            "Decoding the selected interval and measuring its spectrum…",
            move || review_media::analyze_window(&request).map(ResultValue::Spectrum),
        );
    }
    fn focus_selected(&mut self) {
        if let Some(e) = self
            .selected
            .and_then(|i| self.document.as_ref()?.events.get(i))
        {
            self.cursor_s = e.start_s;
            self.window_start = (e.start_s - 5.0).max(0.0);
            self.window_end = (e.end_s + 5.0)
                .min(self.document.as_ref().unwrap().duration_s)
                .min(self.window_start + 300.0);
            self.view.time_min_s = self.window_start;
            self.view.time_max_s = self.window_end;
            self.frame_requested = true;
        }
    }
    fn add_event(&mut self, kind: EventKind) {
        if let Some(doc) = &mut self.document {
            let start = self.cursor_s.min((doc.duration_s - 0.1).max(0.0));
            let end = (start + 5.0).min(doc.duration_s);
            let id = super::unique_name(if kind == EventKind::Doppler {
                "manual-doppler"
            } else {
                "manual-thunder"
            });
            doc.events.push(EvidenceEvent::manual(id, kind, start, end));
            self.selected = Some(doc.events.len() - 1);
            self.tool = if kind == EventKind::Doppler {
                EditTool::DrawTrace
            } else {
                EditTool::ThunderInterval
            };
        }
    }
    fn propose_thunder(&mut self) {
        let Some(window) = &self.window else {
            self.notice = Some((true, "Load a spectrogram interval first.".into()));
            return;
        };
        let envelope = thunder::ThunderEnvelope {
            samples: window
                .envelope
                .iter()
                .filter(|s| s.time_s >= self.view.time_min_s && s.time_s <= self.view.time_max_s)
                .map(|s| thunder::EnvelopeSample {
                    time_s: s.time_s,
                    rms: s.rms,
                })
                .collect(),
            band_low_hz: window.envelope_band_hz[0],
            band_high_hz: window.envelope_band_hz[1],
            method: window.envelope_method.clone(),
        };
        let settings = presets::effective_thunder(
            &self.thunder_settings,
            window.hop_samples as f64 / window.sample_rate_hz as f64,
        );
        match thunder::propose(&envelope, &settings) {
            Ok(report) => {
                let doc = self.document.as_mut().unwrap();
                let mut count = 0;
                for p in &report.proposals {
                    if doc.events.iter().any(|e| {
                        e.kind == EventKind::Thunder
                            && (e.start_s - p.start_s).abs() < 0.01
                            && (e.end_s - p.end_s).abs() < 0.01
                    }) {
                        continue;
                    }
                    let mut e = EvidenceEvent::manual(
                        super::unique_name("thunder"),
                        EventKind::Thunder,
                        p.start_s,
                        p.end_s,
                    );
                    e.uncertainty_s = e
                        .uncertainty_s
                        .max(window.fft_size as f64 / (2.0 * window.sample_rate_hz as f64));
                    e.label = format!("Thunder candidate {:.1}s", p.start_s);
                    e.arrival_s = p.peak_s;
                    e.origin =
                        "low-frequency energy proposal; peak is not a verified arrival".into();
                    e.diagnostics = serde_json::json!({"threshold_rms":report.threshold_rms,"noise_rms":report.noise_rms,"peak_ratio":p.peak_to_threshold_ratio,"settings":report.config,"window_support_s":window.fft_size as f64/window.sample_rate_hz as f64,"band_hz":[report.band_low_hz,report.band_high_hz]});
                    doc.events.push(e);
                    count += 1;
                }
                self.notice = Some((
                    false,
                    format!(
                        "Added {count} low-frequency candidates. Listen, adjust the arrival, then confirm or reject."
                    ),
                ));
            }
            Err(e) => self.notice = Some((true, e)),
        }
    }
    fn remove_selected(&mut self) {
        if let Some(index) = self.selected.take()
            && let Some(document) = &mut self.document
            && index < document.events.len()
        {
            document.events.remove(index);
            self.score = None;
        }
    }
    fn keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        // A text editor (including DragValue text mode) owns editing shortcuts.
        // A focused event-list button should still allow event shortcuts.
        if ctx.text_edit_focused() || self.busy() || ctx.input(|input| input.pointer.primary_down())
        {
            return;
        }
        let (undo, redo, remove, save) = ctx.input_mut(|input| {
            let redo = input.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Z,
            ) || input.consume_key(egui::Modifiers::COMMAND, egui::Key::Y);
            let undo = !redo && input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
            let remove = input.consume_key(egui::Modifiers::NONE, egui::Key::Delete);
            let save = input.consume_key(egui::Modifiers::COMMAND, egui::Key::S);
            (undo, redo, remove, save)
        });
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
        if remove && let Some(before) = self.document.clone() {
            self.remove_selected();
            if self.document.as_ref() != Some(&before) {
                self.checkpoint(before);
            }
        }
        if save {
            self.save(ctx);
        }
    }
    fn finish_edit_frame(&mut self, before: ReviewWorkspace, pointer_down: bool) {
        if self.document.as_ref() != Some(&before) {
            acoustic_review::reset_edited_measurements(&before, self.document.as_mut().unwrap());
            self.inspection
                .clear("Measured evidence changed; inspect the candidate again.");
            if pointer_down || self.pending_undo.is_some() {
                self.pending_undo.get_or_insert(before);
            } else {
                self.checkpoint(before);
            }
        }
        if !pointer_down && let Some(before) = self.pending_undo.take() {
            self.checkpoint(before);
        }
    }
    pub fn analysis_ui(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        self.keyboard_shortcuts(ui.ctx());
        self.toolbar(ui);
        self.document.as_ref()?;
        let before = self.document.clone().unwrap();
        let mut action = None;
        let viewport_height = ui.available_height().max(1.0);
        egui::ScrollArea::vertical()
            .id_salt("audio_workspace_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let channels = self
                        .case
                        .as_ref()
                        .map(|c| c.case.recording.media.channels as usize)
                        .unwrap_or(1);
                    let previous_channel = self.channel();
                    let enabled =
                        !self.busy() && self.document.as_ref().is_some_and(|d| d.events.is_empty());
                    ui.add_enabled_ui(enabled, |ui| {
                        let doc = self.document.as_mut().unwrap();
                        egui::ComboBox::from_id_salt("review_channel")
                            .selected_text(format!("Channel {}", doc.channel + 1))
                            .show_ui(ui, |ui| {
                                for channel in 0..channels {
                                    ui.selectable_value(
                                        &mut doc.channel,
                                        channel,
                                        format!("Channel {}", channel + 1),
                                    );
                                }
                            });
                    });
                    if self.channel() != previous_channel {
                        self.stop();
                        self.window = None;
                        self.texture = None;
                        self.video = None;
                        self.video_available = None;
                        if let Some(review) = self
                            .document
                            .as_mut()
                            .and_then(|doc| doc.envelope_review.as_mut())
                        {
                            review.observation = None;
                        }
                    }
                    if ui
                        .add_enabled(
                            !self.busy(),
                            egui::Button::new("Import Doppler candidates…"),
                        )
                        .clicked()
                    {
                        action = Some(Action::OpenDiscovery);
                    }
                    if ui.button("Group review…").clicked() {
                        action = Some(Action::LegacyReview);
                    }
                    muted(ui, "Delete: remove event · Ctrl+Z / Ctrl+Y: undo / redo");
                });
                if self.detailed_controls {
                    egui::CollapsingHeader::new("Detection presets and spectrum settings").show(
                        ui,
                        |ui| {
                            if let Some(next) = self.detection_controls(ui) {
                                action = Some(next);
                            }
                        },
                    );
                } else if let Some(next) = self.detection_controls(ui) {
                    action = Some(next);
                }
                ui.separator();
                self.joint_audio_ui(ui);
                let width = ui.available_width().max(1.0);
                let content_height = (viewport_height - 65.0).max(400.0);
                if width >= 1100.0 {
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, content_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::Panel::left("event_manager")
                                .resizable(true)
                                .default_size(220.0)
                                .size_range(180.0..=(width * 0.24).max(180.0))
                                .show(ui, |ui| self.event_list(ui));
                            egui::Panel::right("event_inspector")
                                .resizable(true)
                                .default_size(260.0)
                                .size_range(210.0..=(width * 0.28).max(210.0))
                                .show(ui, |ui| {
                                    egui::ScrollArea::vertical()
                                        .id_salt("inspector_scroll")
                                        .show(ui, |ui| self.inspector(ui));
                                });
                            egui::CentralPanel::default()
                                .frame(egui::Frame::NONE)
                                .show(ui, |ui| self.editor_canvas(ui, content_height));
                        },
                    );
                } else {
                    // Narrow windows give the canvas the full width. Details
                    // remain reachable below, rather than squeezing the plot.
                    self.editor_canvas(ui, content_height);
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(&mut self.compact_inspector, false, "Events");
                        ui.selectable_value(&mut self.compact_inspector, true, "Selected event");
                    });
                    egui::ScrollArea::vertical()
                        .id_salt("compact_event_details")
                        .max_height(260.0)
                        .show(ui, |ui| {
                            if self.compact_inspector {
                                self.inspector(ui);
                            } else {
                                self.event_list(ui);
                            }
                        });
                }
            });
        self.finish_edit_frame(before, ui.input(|input| input.pointer.primary_down()));
        action
    }
    fn editor_canvas(&mut self, ui: &mut egui::Ui, height: f32) {
        self.inspection_controls_ui(ui);
        self.video_panel(ui, (height * 0.22).clamp(65.0, 160.0));
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.tool, EditTool::Inspect, "Inspect / seek");
            ui.selectable_value(&mut self.tool, EditTool::DrawTrace, "Draw trace");
            ui.selectable_value(&mut self.tool, EditTool::AddPoint, "Add point");
            ui.selectable_value(&mut self.tool, EditTool::MovePoint, "Move point");
            ui.selectable_value(&mut self.tool, EditTool::ErasePoint, "Erase point");
            ui.selectable_value(&mut self.tool, EditTool::ThunderInterval, "Thunder edges");
        });
        ui.strong("Full recording timeline · drag the cyan viewport or its edges");
        if let Some(doc) = &self.document {
            let navigation = spectrogram::timeline(
                ui,
                doc.duration_s,
                &doc.events,
                self.window
                    .as_ref()
                    .map(|window| (window.start_s, window.end_s)),
                &mut self.view,
                &mut self.cursor_s,
            );
            if navigation.changed {
                ui.ctx().request_repaint();
            }
            if navigation.seek {
                self.stop();
                self.video = None;
                self.frame_requested = true;
            }
        }
        let duration = self
            .document
            .as_ref()
            .map(|doc| doc.duration_s)
            .unwrap_or(1.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Time view");
            let start_changed = ui
                .add(
                    egui::DragValue::new(&mut self.view.time_min_s)
                        .prefix("From ")
                        .suffix(" s")
                        .range(0.0..=duration)
                        .speed(0.25),
                )
                .changed();
            let end_changed = ui
                .add(
                    egui::DragValue::new(&mut self.view.time_max_s)
                        .prefix("To ")
                        .suffix(" s")
                        .range(0.0..=duration)
                        .speed(0.25),
                )
                .changed();
            if start_changed || end_changed {
                self.view
                    .set_time_range(self.view.time_min_s, self.view.time_max_s, duration);
            }
            let midpoint = (self.view.time_min_s + self.view.time_max_s) / 2.0;
            if ui
                .button("Time −")
                .on_hover_text("Zoom out in time")
                .clicked()
            {
                self.view.zoom_time(midpoint, 2.0, duration);
            }
            if ui
                .button("Time +")
                .on_hover_text("Zoom in in time")
                .clicked()
            {
                self.view.zoom_time(midpoint, 0.5, duration);
            }
            if ui
                .add_enabled(self.window.is_some(), egui::Button::new("Fit loaded audio"))
                .clicked()
                && let Some(window) = &self.window
            {
                self.view
                    .set_time_range(window.start_s, window.end_s, duration);
            }
            if ui
                .add_enabled(!self.busy(), egui::Button::new("Load visible audio"))
                .clicked()
            {
                self.window_start = self.view.time_min_s;
                self.window_end = self.view.time_max_s;
                self.load_window(ui.ctx());
            }
        });
        let frequency_bounds = self.window.as_ref().and_then(spectrogram::frequency_bounds);
        ui.add_enabled_ui(frequency_bounds.is_some(), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Frequency view");
                let limits = frequency_bounds.unwrap_or(spectrogram::FrequencyBounds {
                    min_hz: 0.0,
                    max_hz: self.spectrum_max_hz,
                    bin_hz: 4000.0 / self.fft_size as f64,
                });
                let changed = ui
                    .add(
                        egui::DragValue::new(&mut self.view.frequency_min_hz)
                            .prefix("From ")
                            .suffix(" Hz")
                            .range(limits.min_hz..=limits.max_hz)
                            .speed(limits.bin_hz),
                    )
                    .changed()
                    | ui.add(
                        egui::DragValue::new(&mut self.view.frequency_max_hz)
                            .prefix("To ")
                            .suffix(" Hz")
                            .range(limits.min_hz..=limits.max_hz)
                            .speed(limits.bin_hz),
                    )
                    .changed();
                if changed {
                    self.view.set_frequency_range(
                        self.view.frequency_min_hz,
                        self.view.frequency_max_hz,
                        limits,
                    );
                }
                let midpoint = (self.view.frequency_min_hz + self.view.frequency_max_hz) / 2.0;
                if ui
                    .button("Frequency −")
                    .on_hover_text("Zoom out in frequency")
                    .clicked()
                {
                    self.view.zoom_frequency(midpoint, 2.0, limits);
                }
                if ui
                    .button("Frequency +")
                    .on_hover_text("Zoom in in frequency")
                    .clicked()
                {
                    self.view.zoom_frequency(midpoint, 0.5, limits);
                }
                if ui.button("Fit loaded frequencies").clicked() {
                    self.view.fit_frequency(limits);
                }
            });
        });
        if let Some(bounds) = frequency_bounds {
            muted(
                ui,
                format!(
                    "Loaded {:.1}–{:.1} Hz · FFT bins {:.3} Hz apart. Zoom changes the view, not measured frequency detail.",
                    bounds.min_hz, bounds.max_hz, bounds.bin_hz
                ),
            );
        }
        let has_coverage = self.window.as_ref().is_some_and(|window| {
            self.view.time_min_s >= window.start_s - 1e-6
                && self.view.time_max_s <= window.end_s + 1e-6
        });
        if !has_coverage {
            muted(
                ui,
                "Some visible audio is not loaded. Use Load visible audio (up to 5 minutes). Gray in the overview shows loaded coverage.",
            );
        } else {
            muted(
                ui,
                "Inspect/seek: drag up/down to pan frequency, left/right to scrub · Wheel: zoom both axes · Ctrl: time only · Alt: frequency only · Shift: pan time",
            );
        }
        let plot_height = (height - 295.0).max(180.0);
        if let (Some(texture), Some(window), Some(doc)) =
            (&self.texture, &self.window, &mut self.document)
        {
            let result = spectrogram::show(
                ui,
                texture,
                window,
                &mut self.view,
                &mut doc.events,
                self.selected,
                &mut self.cursor_s,
                self.tool,
                plot_height,
            );
            if !result.changed
                && self.inspection.visible
                && let (Some(active), Some(rect)) = (&self.inspection.active, result.plot_rect)
            {
                let filter = if self.inspection.all_events {
                    None
                } else {
                    result
                        .selected
                        .and_then(|index| doc.events.get(index))
                        .map(|event| event.id.as_str())
                        .or(Some(""))
                };
                super::prediction_plot::paint(
                    &ui.painter_at(rect),
                    rect,
                    &self.view,
                    &active.overlay,
                    filter,
                    &doc.events,
                );
            }
            if let Some(selected) = result.selected {
                self.selected = Some(selected);
            }
            if result.seek {
                self.stop();
                self.video = None;
                self.frame_requested = true;
            }
            if let Some(error) = result.error {
                self.notice = Some((true, error));
            }
        } else {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width().max(1.0), plot_height),
                egui::Sense::hover(),
            );
            ui.painter()
                .rect_filled(rect, 0.0, Color32::from_rgb(12, 13, 16));
            ui.painter_at(rect).text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Choose an interval in the timeline, then Load visible audio",
                egui::FontId::proportional(14.0),
                Color32::LIGHT_GRAY,
            );
        }
    }
    fn detection_controls(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Find candidates");
            ui.separator();
            ui.label("Doppler");
            let current = DopplerPreset::matching(&self.discovery);
            egui::ComboBox::from_id_salt("doppler_preset")
                .width(110.0)
                .selected_text(current.map(DopplerPreset::label).unwrap_or("Custom"))
                .show_ui(ui, |ui| {
                    for preset in DopplerPreset::ALL {
                        if ui.selectable_label(current == Some(preset), preset.label())
                            .on_hover_text(preset.description()).clicked()
                        { self.discovery = preset.settings(); }
                    }
                }).response.on_hover_text("Changes detection settings only. Each preset restores the standard frequency range and window settings; inspect Advanced detection settings for the actual thresholds.");
            if ui.add_enabled(!self.busy() && self.discovery.validate().is_ok(), egui::Button::new("Prepare detection"))
                .on_hover_text("Prepare a whole-recording Doppler detection job using these settings.").clicked()
            { action = Some(Action::PrepareDiscovery(self.discovery.clone())); }
            ui.separator();
            ui.label("Thunder patterns");
            let current = ThunderPreset::matching(&self.thunder_settings);
            egui::ComboBox::from_id_salt("thunder_preset")
                .width(120.0)
                .selected_text(current.map(ThunderPreset::label).unwrap_or("Custom"))
                .show_ui(ui, |ui| {
                    for preset in ThunderPreset::ALL {
                        if ui.selectable_label(current == Some(preset), preset.label())
                            .on_hover_text(preset.description()).clicked()
                        { self.thunder_settings = preset.settings(); }
                    }
                }).response.on_hover_text("Low-frequency sound-pattern proposals; wind, handling noise and engines may also trigger them. These presets are not confidence ratings.");
            if ui.add_enabled(!self.busy() && self.window.is_some(), egui::Button::new("Find in view"))
                .on_hover_text("Find low-frequency bursts only in the visible time interval. Proposals remain unreviewed.").clicked()
            { self.propose_thunder(); }
        });
        muted(
            ui,
            "Choose a sensitivity, run detection, then listen and review. Presets never accept or select evidence.",
        );
        if let Some(window) = &self.window {
            let hop = window.hop_samples as f64 / window.sample_rate_hz as f64;
            let effective = presets::effective_thunder(&self.thunder_settings, hop);
            if effective.max_gap_s != self.thunder_settings.max_gap_s
                || effective.max_sample_gap_s != self.thunder_settings.max_sample_gap_s
            {
                muted(
                    ui,
                    format!(
                        "Loaded audio has a {hop:.3}s hop: thunder uses a {:.3}s burst gap and {:.3}s sample-gap limit.",
                        effective.max_gap_s, effective.max_sample_gap_s
                    ),
                );
            }
        }
        egui::CollapsingHeader::new("Advanced detection settings")
            .default_open(false).show(ui, |ui| {
                ui.columns(2, |columns| {
                    columns[0].strong("Doppler · whole recording");
                    columns[0].add(egui::Slider::new(&mut self.discovery.min_contrast, 0.5..=8.0).text("Minimum contrast"));
                    columns[0].add(egui::Slider::new(&mut self.discovery.min_support, 0.1..=1.0).text("Minimum support"));
                    columns[0].horizontal_wrapped(|ui| {
                        ui.label("Minimum duration");
                        ui.add(egui::DragValue::new(&mut self.discovery.min_duration_s).range(2.0..=45.0).suffix(" s"));
                        ui.label("Frequency drop");
                        ui.add(egui::DragValue::new(&mut self.discovery.min_drop_hz).range(0.5..=50.0).speed(0.5).suffix(" Hz"));
                    });
                    columns[0].add(egui::Slider::new(&mut self.discovery.min_fractional_drop, 0.005..=0.2).text("Fractional drop"));
                    columns[0].label(format!("Search band: {:.0}–{:.0} Hz · {:.3}s window · {:.3}s hop", self.discovery.min_frequency_hz, self.discovery.max_frequency_hz, self.discovery.window_s, self.discovery.hop_s));
                    columns[1].strong("Thunder patterns · visible interval");
                    columns[1].add(egui::Slider::new(&mut self.thunder_settings.threshold_ratio, 1.1..=12.0).text("Energy / noise threshold"));
                    columns[1].add(egui::Slider::new(&mut self.thunder_settings.minimum_rms, 0.000001..=0.01).logarithmic(true).text("Minimum RMS"));
                    columns[1].horizontal_wrapped(|ui| {
                        ui.label("Minimum duration");
                        ui.add(egui::DragValue::new(&mut self.thunder_settings.min_duration_s).range(0.1..=10.0).speed(0.05).suffix(" s"));
                        ui.label("Gap tolerance");
                        ui.add(egui::DragValue::new(&mut self.thunder_settings.max_gap_s).range(0.05..=3.0).speed(0.05).suffix(" s"));
                    });
                    muted(&mut columns[1], "Energy bursts can also come from wind, handling noise or engines. Listen and adjust each proposed arrival.");
                });
                if let Err(error) = self.discovery.validate() { ui.colored_label(tone(ui, RED), error); }
            });
        egui::CollapsingHeader::new("Advanced spectrogram settings")
            .default_open(false).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("FFT samples");
                    egui::ComboBox::from_id_salt("fft").selected_text(self.fft_size.to_string()).show_ui(ui, |ui| {
                        for n in [1024, 2048, 4096, 8192, 16384] { ui.selectable_value(&mut self.fft_size, n, n.to_string()); }
                    });
                    ui.add(egui::DragValue::new(&mut self.hop_s).range(0.0625..=1.0).speed(0.0625).suffix(" s hop"));
                    ui.add(egui::DragValue::new(&mut self.spectrum_max_hz).range(260.0..=2000.0).suffix(" Hz load limit"))
                        .on_hover_text("Frequency band to measure on the next audio load. Independent of the visible frequency zoom.");
                    let changed = ui.add(egui::Slider::new(&mut self.view.db_min, -150.0..=-40.0).text("Dark floor dB")).changed()
                        | ui.add(egui::Slider::new(&mut self.view.db_max, -60.0..=0.0).text("Bright limit dB")).changed();
                    if changed {
                        self.view.db_max = self.view.db_max.max(self.view.db_min + 1.0);
                        if let Some(window) = &self.window { self.texture = Some(spectrogram::texture(ui.ctx(), window, self.view.db_min, self.view.db_max)); }
                    }
                });
                muted(ui, "FFT, hop and load-limit changes take effect when you load / refresh the spectrogram. Frequency zoom never reduces the next load's band. Larger FFT windows trade time detail for finer frequency bins; zoom alone adds no measured detail. Display settings do not change detection thresholds.");
            });
        action
    }
    fn event_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("Events");
        ui.horizontal_wrapped(|ui| {
            if ui.button("+ Doppler").clicked() {
                self.add_event(EventKind::Doppler);
            }
            if ui.button("+ Thunder").clicked() {
                self.add_event(EventKind::Thunder);
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_doppler, "Doppler");
            ui.checkbox(&mut self.show_thunder, "Thunder");
        });
        let Some(doc) = &self.document else { return };
        ui.label(format!(
            "{} total · {} selected",
            doc.events.len(),
            doc.events.iter().filter(|e| e.selected_for_scoring).count()
        ));
        let mut focus = false;
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - 95.0).clamp(100.0, 500.0))
            .id_salt("events")
            .show(ui, |ui| {
                for (i, e) in doc.events.iter().enumerate() {
                    if (e.kind == EventKind::Doppler && !self.show_doppler)
                        || (e.kind == EventKind::Thunder && !self.show_thunder)
                    {
                        continue;
                    }
                    let color = if e.kind == EventKind::Doppler {
                        ACCENT
                    } else {
                        AMBER
                    };
                    let status = match e.review {
                        ReviewState::Pending => "?",
                        ReviewState::Confirmed => "✓",
                        ReviewState::Rejected => "×",
                    };
                    let label = format!(
                        "{} {}  {:.1}–{:.1}s\n{}{}",
                        status,
                        if e.kind == EventKind::Doppler {
                            "DOPPLER"
                        } else {
                            "THUNDER"
                        },
                        e.start_s,
                        e.end_s,
                        e.label,
                        if e.selected_for_scoring {
                            "  [scoring]"
                        } else {
                            ""
                        }
                    );
                    let label = format!(
                        "{}\nSource: {}",
                        label,
                        acoustic_review::assessment_label(&e.acoustic_source)
                    );
                    let response = ui.selectable_label(
                        self.selected == Some(i),
                        RichText::new(label).color(tone(ui, color)),
                    );
                    if response.clicked() {
                        self.selected = Some(i);
                    }
                    if response.double_clicked() {
                        self.selected = Some(i);
                        focus = true;
                    }
                    ui.separator();
                }
            });
        if focus {
            self.focus_selected();
        }
        if ui.button("Focus selected interval").clicked() {
            self.focus_selected();
        }
        if ui.button("Load selected interval").clicked() {
            self.focus_selected();
            self.load_window(ui.ctx());
        }
        muted(
            ui,
            "Double-click to frame an event. Delete removes the selection; Ctrl+Z restores it.",
        );
    }
    fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("Event inspector");
        let Some(i) = self.selected else {
            muted(ui, "Select an event or draw a new trace.");
            return;
        };
        let Some(doc) = &mut self.document else {
            return;
        };
        let duration = doc.duration_s;
        let Some(e) = doc.events.get_mut(i) else {
            return;
        };
        accessible_text(
            ui,
            "event_label",
            &mut e.label,
            "Event label",
            false,
            ui.available_width(),
        );
        ui.label(&e.origin);
        let original = e.clone();
        egui::Grid::new("interval_fields")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Start (s)");
                let maximum = e
                    .points
                    .first()
                    .map(|p| p.time_s)
                    .unwrap_or(e.end_s - 0.001)
                    .min(e.arrival_s)
                    .min(e.end_s - 0.001)
                    .max(0.0);
                ui.add(
                    egui::DragValue::new(&mut e.start_s)
                        .range(0.0..=maximum)
                        .speed(0.1),
                );
                ui.end_row();
                ui.label("End (s)");
                let minimum = e
                    .points
                    .last()
                    .map(|p| p.time_s)
                    .unwrap_or(e.start_s + 0.001)
                    .max(e.arrival_s)
                    .max(e.start_s + 0.001)
                    .min(duration);
                ui.add(
                    egui::DragValue::new(&mut e.end_s)
                        .range(minimum..=duration)
                        .speed(0.1),
                );
                ui.end_row();
                ui.label(if e.kind == EventKind::Doppler {
                    "CPA arrival (s)"
                } else {
                    "Thunder arrival (s)"
                });
                ui.add(
                    egui::DragValue::new(&mut e.arrival_s)
                        .range(e.start_s..=e.end_s)
                        .speed(0.1),
                );
                ui.end_row();
                ui.label("Timing uncertainty (s)");
                ui.add(
                    egui::DragValue::new(&mut e.uncertainty_s)
                        .range(0.01..=300.0)
                        .speed(0.1),
                );
                ui.end_row();
            });
        if e.kind == EventKind::Doppler {
            ui.label(format!("{} trace points", e.points.len()));
            ui.horizontal(|ui| {
                ui.label("Frequency uncertainty");
                ui.add(
                    egui::DragValue::new(&mut e.frequency_sigma_hz)
                        .range(0.01..=100.0)
                        .speed(0.1)
                        .suffix(" Hz"),
                );
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
            ui.label("Associated aircraft track");
            accessible_text(
                ui,
                "associated_flight",
                &mut e.flight_id,
                "Choose on map or enter track ID",
                false,
                ui.available_width(),
            );
            egui::CollapsingHeader::new("Edit measured point values").show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .id_salt("point_values")
                    .show(ui, |ui| {
                        egui::Grid::new("points").num_columns(3).show(ui, |ui| {
                            ui.label("Time s");
                            ui.label("Hz");
                            ui.end_row();
                            let mut remove = None;
                            for j in 0..e.points.len() {
                                let lower = if j > 0 {
                                    e.points[j - 1].time_s + 0.000001
                                } else {
                                    e.start_s
                                };
                                let upper = if j + 1 < e.points.len() {
                                    e.points[j + 1].time_s - 0.000001
                                } else {
                                    e.end_s
                                };
                                let p = &mut e.points[j];
                                ui.add(
                                    egui::DragValue::new(&mut p.time_s)
                                        .range(lower..=upper)
                                        .speed(0.05),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut p.frequency_hz)
                                        .range(0.01..=2000.0)
                                        .speed(0.1),
                                );
                                if ui.small_button("×").clicked() {
                                    remove = Some(j);
                                }
                                ui.end_row();
                            }
                            if let Some(j) = remove {
                                e.points.remove(j);
                            }
                        });
                    });
            });
        } else {
            ui.label("Associated lightning strike");
            accessible_text(
                ui,
                "associated_strike",
                &mut e.strike_id,
                "Choose a strike on the map",
                false,
                ui.available_width(),
            );
        }
        if acoustic_review::measurements_changed(&original, e) {
            acoustic_review::reset_measurement_review(e);
            e.origin = "manual correction of reviewed measurements".into();
        }
        ui.separator();
        ui.strong("1 Measurement review");
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut e.review, ReviewState::Pending, "Unreviewed");
            ui.selectable_value(
                &mut e.review,
                ReviewState::Confirmed,
                "Confirm measurements",
            );
            ui.selectable_value(&mut e.review, ReviewState::Rejected, "Reject");
        });
        muted(
            ui,
            "Check that the trace, interval and timing represent what you observed.",
        );
        if e.review != ReviewState::Confirmed {
            e.selected_for_scoring = false;
        }
        ui.separator();
        acoustic_review::assessment_ui(ui, e);
        ui.separator();
        ui.strong("3 Scoring choice");
        ui.add_enabled_ui(e.review == ReviewState::Confirmed, |ui| {
            ui.checkbox(&mut e.selected_for_scoring, "Use this event in scoring");
        });
        muted(
            ui,
            "Only confirmed measurements with a recorded source assessment can be scored. Choose specific aircraft/strike alternatives separately in Search.",
        );
        egui::CollapsingHeader::new("Detection diagnostics").show(ui, |ui| {
            ui.label(serde_json::to_string_pretty(&e.diagnostics).unwrap_or_default());
        });
        ui.separator();
        if ui.button("Remove event · Delete").clicked() {
            doc.events.remove(i);
            self.selected = None;
        }
    }
    fn video_panel(&mut self, ui: &mut egui::Ui, height: f32) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(if self.playback.is_some() {
                    "Pause"
                } else {
                    "Play visible audio"
                })
                .clicked()
            {
                if self.playback.is_some() {
                    self.stop();
                } else if let Some(case) = &self.case {
                    self.window_start = self.view.time_min_s;
                    self.window_end = self.view.time_max_s;
                    let start_s = if (self.window_start..self.window_end).contains(&self.cursor_s) {
                        self.cursor_s
                    } else {
                        self.window_start
                    };
                    let request = review_media::PlaybackRequest {
                        input: case.recording_path.clone(),
                        ffplay: self.tools.ffplay.clone(),
                        ffprobe: self.tools.ffprobe.clone(),
                        expected_source: self.window.as_ref().map(|w| w.source_stamp.clone()),
                        channel: self.channel(),
                        start_s,
                        end_s: self.window_end,
                    };
                    self.playback_generation = self.playback_generation.wrapping_add(1);
                    let token = self.playback_generation;
                    self.spawn(ui.ctx(), "Starting audio playback…", move || {
                        AudioPlayback::start(&request).map(|player| {
                            ResultValue::Playing(PlaybackOwner::new(player), request.start_s, token)
                        })
                    });
                }
            }
            if ui.button("Stop").clicked() {
                self.stop();
                self.cursor_s = self.view.time_min_s;
                self.frame_requested = true;
            }
            let duration = self.document.as_ref().map(|d| d.duration_s).unwrap_or(1.0);
            if ui
                .add(egui::Slider::new(&mut self.cursor_s, 0.0..=duration).text("Recording s"))
                .changed()
            {
                self.stop();
                self.video = None;
                self.frame_requested = true;
            }
            ui.checkbox(&mut self.video_overlay, "Spectrogram overlay on video");
        });
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, Color32::from_rgb(18, 19, 23));
        if let Some(texture) = &self.video {
            let scale =
                (rect.width() / texture.size_vec2().x).min(rect.height() / texture.size_vec2().y);
            let image = egui::Rect::from_center_size(rect.center(), texture.size_vec2() * scale);
            painter.image(
                texture.id(),
                image,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                if self.video_available == Some(false) {
                    "Audio-only recording"
                } else {
                    "Video frame preview · seek or load an interval"
                },
                egui::FontId::proportional(14.0),
                Color32::LIGHT_GRAY,
            );
        }
        if self.video_overlay
            && let (Some(texture), Some(window), Some(doc)) =
                (&self.texture, &self.window, &self.document)
        {
            let overlay = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - height * 0.42),
                rect.right_bottom(),
            );
            spectrogram::paint_overlay(
                &painter,
                overlay,
                texture,
                window,
                &self.view,
                &doc.events,
                self.selected,
                self.cursor_s,
            );
            if self.inspection.visible
                && let Some(active) = &self.inspection.active
            {
                let filter = if self.inspection.all_events {
                    None
                } else {
                    self.selected
                        .and_then(|index| doc.events.get(index))
                        .map(|event| event.id.as_str())
                        .or(Some(""))
                };
                super::prediction_plot::paint(
                    &painter,
                    overlay,
                    &self.view,
                    &active.overlay,
                    filter,
                    &doc.events,
                );
            }
        }
        let text = self
            .video_time
            .map(|t| format!("Decoded video frame {t:.3}s · playback preview"))
            .unwrap_or_else(|| "Video / measured spectrum".into());
        painter.text(
            rect.left_top() + egui::vec2(8.0, 6.0),
            egui::Align2::LEFT_TOP,
            text,
            egui::FontId::proportional(12.0),
            Color32::WHITE,
        );
    }
}
