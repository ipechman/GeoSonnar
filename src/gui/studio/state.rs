//! State responsibilities for the review workspace.
use super::*;

#[derive(Clone, Default)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub ffplay: PathBuf,
}

pub enum Action {
    ReviewInspection,
    ReviewEvidence,
    ReviewResults,
    OpenDiscovery,
    PrepareDiscovery(DiscoverySettings),
    LegacyReview,
}
pub struct Studio {
    /// Presentation only: never changes evidence, search mode or parameters.
    pub detailed_controls: bool,
    pub(super) exiting: bool,
    pub(super) case: Option<CaseOverview>,
    pub document: Option<ReviewWorkspace>,
    pub(super) saved: Option<ReviewWorkspace>,
    pub(super) undo: Vec<ReviewWorkspace>,
    pub(super) redo: Vec<ReviewWorkspace>,
    pub tools: Tools,
    pub(super) selected: Option<usize>,
    pub(super) compact_inspector: bool,
    pub(super) window: Option<SpectrogramWindow>,
    pub(super) texture: Option<egui::TextureHandle>,
    pub(super) video: Option<egui::TextureHandle>,
    pub(super) video_time: Option<f64>,
    pub(super) video_available: Option<bool>,
    pub(super) video_overlay: bool,
    pub(super) view: PlotView,
    pub(super) window_start: f64,
    pub(super) window_end: f64,
    pub(super) fft_size: usize,
    pub(super) hop_s: f64,
    pub(super) spectrum_max_hz: f64,
    pub(super) tool: EditTool,
    pub(super) cursor_s: f64,
    pub(super) playback_generation: u64,
    pub(super) pending_undo: Option<ReviewWorkspace>,
    pub(super) playback: Option<PlaybackOwner>,
    pub(super) play_start: Option<(Instant, f64)>,
    pub(super) frame_busy: bool,
    pub(super) frame_requested: bool,
    pub(super) next_frame: Instant,
    pub(super) busy: Option<String>,
    pub(super) notice: Option<(bool, String)>,
    pub(super) generation: u64,
    pub(super) sender: Sender<Reply>,
    pub(super) receiver: Receiver<Reply>,
    pub(super) discovery: DiscoverySettings,
    pub(super) thunder_settings: thunder::ThunderConfig,
    pub(super) show_doppler: bool,
    pub(super) show_thunder: bool,
    pub(super) data: library::LibraryState,
    pub(super) map: map::MapState,
    pub(super) layers: Vec<map_data::MapData>,
    pub(super) lightning: Option<evidence_score::LightningCatalog>,
    pub(super) score: Option<evidence_score::ScoreReport>,
    pub(super) score_document: Option<ReviewWorkspace>,
    pub(super) regional: regional::RegionalState,
    pub(super) joint: joint::JointState,
    pub(super) wide: wide_ui::WideState,
    pub(super) inspection: inspection::InspectionState,
    pub(super) investigation: investigation::InvestigationState,
    pub(super) run_history: history_runs::HistoryState,
    pub(super) loaded_data_revision: u64,
    pub(super) pending_action: Option<Action>,
    pub(super) heights: receiver_ui::ReceiverHeightState,
    pub(super) download_cancel: Arc<AtomicBool>,
    pub(super) progress_busy: bool,
    pub(super) next_progress: Instant,
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
    #[cfg(test)]
    pub(in crate::gui) fn assert_unloaded_idle(&self) {
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
}
