mod activity;
mod media_setup;
mod prediction_plot;
pub mod renderer;
pub mod shutdown;
mod spectrogram;
pub mod startup;
mod studio;
#[cfg(test)]
mod tests;
pub mod widgets;

use eframe::egui::{self, RichText};
use geosonnar::{
    casework::{HintCertainty, HintKind, HintRequest, HintSource},
    detailed_data::sha256,
    event_review::{self, GroupChoice, ReviewActor, ReviewDecision, ReviewSession},
    gui_bridge::{self, CaseOverview, Companion, JobOverview, VerifiedDiscoveryArtifact},
    jobs::MediaTools,
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use widgets::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Intake,
    Review,
    Groups,
    Map,
    Data,
    Results,
    Jobs,
    Settings,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    dark_mode: bool,
    detailed_controls: bool,
    ffmpeg: String,
    ffprobe: String,
    ffplay: String,
    recent_cases: Vec<PathBuf>,
}

const MAX_PREFERENCES_BYTES: usize = 64 * 1024;
const MAX_RECENT_CASES: usize = 8;

fn read_preferences(path: &Path) -> Preferences {
    let read = || -> Option<Preferences> {
        let file = std::fs::File::open(path).ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.len() > MAX_PREFERENCES_BYTES as u64 {
            return None;
        }
        // The extra byte also detects growth after the metadata check.
        let mut bytes = Vec::new();
        file.take(MAX_PREFERENCES_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > MAX_PREFERENCES_BYTES {
            return None;
        }
        serde_json::from_slice(&bytes).ok()
    };
    let mut preferences = read().unwrap_or_default();
    preferences.recent_cases.truncate(MAX_RECENT_CASES);
    preferences
}

struct ReviewView {
    session: Arc<ReviewSession>,
    binding: Option<VerifiedDiscoveryArtifact>,
    choices: Vec<GroupChoice>,
    saved_choices: Vec<GroupChoice>,
    selected: usize,
    saved_path: Option<PathBuf>,
}
impl ReviewView {
    fn dirty(&self) -> bool {
        self.choices != self.saved_choices
    }
}

enum Payload {
    Case(CaseOverview),
    HintAdded(CaseOverview, String),
    Job(JobOverview),
    Jobs(Vec<JobOverview>),
    Review(Arc<ReviewSession>, Option<VerifiedDiscoveryArtifact>),
    Saved(PathBuf, Vec<GroupChoice>),
    Choices(Vec<GroupChoice>, PathBuf),
}

#[derive(Clone, Copy)]
enum Lane {
    Work,
    Run,
    Cancel,
    Poll,
}
struct Message {
    lane: Lane,
    case_manifest: Option<PathBuf>,
    result: Result<Payload, String>,
}

pub struct Workbench {
    page: Page,
    companion: Result<Companion, String>,
    preferences: Preferences,
    preferences_path: PathBuf,
    media_setup: media_setup::MediaSetup,
    case: Option<CaseOverview>,
    jobs: Vec<JobOverview>,
    selected_job: Option<PathBuf>,
    review: Option<ReviewView>,
    studio: studio::Studio,
    channel: usize,
    title: String,
    recording: String,
    case_parent: String,
    hint_kind: usize,
    hint_text: String,
    hint_reported: bool,
    busy: Option<String>,
    running: Option<PathBuf>,
    canceling: bool,
    polling: bool,
    next_poll: Instant,
    notice: Option<(bool, String)>,
    close_requested: bool,
    shutdown: shutdown::ExitState,
    allow_close: bool,
    sender: Sender<Message>,
    receiver: Receiver<Message>,
}

fn unique_name(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id()
    )
}

fn decision_name(value: ReviewDecision) -> &'static str {
    match value {
        ReviewDecision::Unreviewed => "Not reviewed",
        ReviewDecision::KeepForMatching => "Keep",
        ReviewDecision::Reject => "Reject",
        ReviewDecision::Uncertain => "Unsure",
    }
}
fn phase_name(value: &str) -> &str {
    match value {
        "pending" => "Ready",
        "running" => "Running",
        "completed" => "Completed",
        "canceled" | "cancelled" => "Canceled",
        "failed" => "Needs attention",
        "blocked" => "Needs input",
        other => other,
    }
}
fn phase_color(value: &str) -> egui::Color32 {
    match value {
        "completed" => ACCENT,
        "running" => PALETTE[4],
        "failed" => RED,
        _ => AMBER,
    }
}

impl Workbench {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_case: Option<PathBuf>) -> Self {
        Self::with_executable(
            cc,
            std::env::current_exe().unwrap_or_default(),
            initial_case,
        )
    }

    fn with_executable(
        cc: &eframe::CreationContext<'_>,
        executable: PathBuf,
        initial_case: Option<PathBuf>,
    ) -> Self {
        let base = executable.parent().unwrap_or(Path::new("."));
        let preferences_path = base.join("geosonnar-settings.json");
        let preferences = read_preferences(&preferences_path);
        theme(&cc.egui_ctx, preferences.dark_mode);
        let (sender, receiver) = mpsc::channel();
        let mut app = Self {
            page: Page::Intake,
            companion: Companion::locate(&executable),
            preferences,
            preferences_path,
            media_setup: media_setup::MediaSetup::default(),
            case: None,
            jobs: Vec::new(),
            selected_job: None,
            review: None,
            studio: studio::Studio::default(),
            channel: 0,
            title: String::new(),
            recording: String::new(),
            case_parent: String::new(),
            hint_kind: 0,
            hint_text: String::new(),
            hint_reported: false,
            busy: None,
            running: None,
            canceling: false,
            polling: false,
            next_poll: Instant::now(),
            notice: None,
            close_requested: false,
            shutdown: shutdown::ExitState::default(),
            allow_close: false,
            sender,
            receiver,
        };
        app.check_media_tools(&cc.egui_ctx);
        if let Some(manifest) = initial_case {
            app.open_case(manifest, &cc.egui_ctx);
        }
        startup::mark(startup::Stage::WorkbenchReady);
        app
    }

    fn spawn(
        &mut self,
        ctx: &egui::Context,
        lane: Lane,
        label: &str,
        operation: impl FnOnce() -> Result<Payload, String> + Send + 'static,
    ) {
        if matches!(lane, Lane::Work) {
            self.busy = Some(label.into());
            self.notice = None;
        }
        if matches!(lane, Lane::Cancel) {
            self.canceling = true;
        }
        if matches!(lane, Lane::Poll) {
            self.polling = true;
        }
        let sender = self.sender.clone();
        let case_manifest = self.case.as_ref().map(|case| case.manifest.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
                .unwrap_or_else(|_| Err("The operation stopped unexpectedly. Existing evidence is preserved; check the job details before resuming.".into()));
            let _ = sender.send(Message {
                lane,
                case_manifest,
                result,
            });
            ctx.request_repaint();
        });
    }

    fn idle(&self) -> bool {
        self.busy.is_none() && self.running.is_none() && !self.canceling && !self.studio.busy()
    }
    fn dirty(&self) -> bool {
        self.review.as_ref().is_some_and(ReviewView::dirty)
            || self.studio.dirty()
            || self.studio.has_unsaved_results()
    }
    fn can_switch(&self) -> bool {
        self.idle() && !self.studio.active_for_exit() && !self.dirty()
    }

    fn save_preferences(&mut self) {
        let result = serde_json::to_vec_pretty(&self.preferences)
            .map_err(|e| e.to_string())
            .and_then(|data| {
                std::fs::write(&self.preferences_path, data).map_err(|e| e.to_string())
            });
        if let Err(e) = result {
            self.notice = Some((true, format!("Could not save settings beside the app: {e}")));
        }
    }

    fn open_case(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.spawn(
            ctx,
            Lane::Work,
            "Checking the recording and opening the case…",
            move || gui_bridge::case_overview(&path, true).map(Payload::Case),
        );
    }

    fn update_messages(&mut self, ctx: &egui::Context) {
        if self.shutdown.requested() {
            // Late results cannot reopen a case, restart polling, or save
            // preferences after the user has chosen to leave.
            while self.receiver.try_recv().is_ok() {}
            return;
        }
        while let Ok(message) = self.receiver.try_recv() {
            match message.lane {
                Lane::Work => self.busy = None,
                Lane::Run => self.running = None,
                Lane::Cancel => self.canceling = false,
                Lane::Poll => self.polling = false,
            }
            if matches!(message.lane, Lane::Poll)
                && message.case_manifest != self.case.as_ref().map(|case| case.manifest.clone())
            {
                continue;
            }
            match message.result {
                Err(e) => {
                    self.notice = Some((true, e));
                }
                Ok(Payload::HintAdded(mut case, submitted)) => {
                    if let Some(previous) = &self.case
                        && previous.manifest == case.manifest
                        && previous.recording_path == case.recording_path
                        && previous.case.recording.sha256 == case.case.recording.sha256
                    {
                        case.recording_verified = previous.recording_verified;
                        case.recording_problem = previous.recording_problem.clone();
                    }
                    if self.hint_text.trim() == submitted {
                        self.hint_text.clear();
                    }
                    self.case = Some(case);
                    self.notice=Some((false,"Clue saved. It remains separate from verified evidence and numerical search limits.".into()));
                }
                Ok(Payload::Case(case)) => {
                    let changed = self
                        .case
                        .as_ref()
                        .is_none_or(|old| old.manifest != case.manifest);
                    if changed
                        && (self.dirty()
                            || !self.idle()
                            || self.studio.active_for_exit()
                            || message.case_manifest
                                != self.case.as_ref().map(|old| old.manifest.clone()))
                    {
                        self.notice = Some((
                            true,
                            format!(
                                "Case check finished for {}. Your current work was kept because it changed or started work while the case was opening. Save or discard current work, finish active jobs, then open that case again.",
                                display_path(&case.manifest)
                            ),
                        ));
                        continue;
                    }
                    self.studio.bind(&case);
                    if changed {
                        self.review = None;
                        self.jobs.clear();
                        self.selected_job = None;
                        self.channel = 0;
                    }
                    self.preferences
                        .recent_cases
                        .retain(|p| p != &case.manifest);
                    self.preferences
                        .recent_cases
                        .insert(0, case.manifest.clone());
                    self.preferences.recent_cases.truncate(MAX_RECENT_CASES);
                    self.notice = case.recording_problem.as_ref().map(|problem| {
                        (
                            true,
                            format!("Case opened, but the recording needs attention: {problem}"),
                        )
                    });
                    self.case = Some(case);
                    self.page = Page::Intake;
                    self.save_preferences();
                    self.next_poll = Instant::now();
                }
                Ok(Payload::Jobs(jobs)) => {
                    self.jobs = jobs;
                }
                Ok(Payload::Job(job)) => {
                    self.selected_job = Some(job.directory.clone());
                    if let Some(existing) =
                        self.jobs.iter_mut().find(|j| j.directory == job.directory)
                    {
                        *existing = job;
                    } else {
                        self.jobs.push(job);
                    }
                    self.next_poll = Instant::now();
                    self.page = Page::Jobs;
                    if matches!(message.lane, Lane::Run) {
                        self.notice = Some((false, "Job stopped. Review its stage status below; processing does not verify a location.".into()));
                    }
                }
                Ok(Payload::Review(session, binding)) => {
                    if let Err(error) = self.studio.import_review(&session) {
                        self.notice = Some((true, error));
                    }
                    let choices = session.initial_choices();
                    self.review = Some(ReviewView {
                        session,
                        binding,
                        saved_choices: choices.clone(),
                        choices,
                        selected: 0,
                        saved_path: None,
                    });
                    self.page = Page::Review;
                    self.notice = Some((false, "Recording and measured trace files checked. Review each proposal before keeping it.".into()));
                }
                Ok(Payload::Saved(path, choices)) => {
                    if let Some(review) = &mut self.review {
                        review.saved_choices = choices;
                        review.saved_path = Some(path.clone());
                    }
                    self.notice = Some((
                        false,
                        format!("Saved a new review snapshot: {}", display_path(&path)),
                    ));
                }
                Ok(Payload::Choices(choices, path)) => {
                    if let Some(review) = &mut self.review {
                        match ordered_choices(&review.session, choices) {
                            Ok(choices) => {
                                review.saved_choices = choices.clone();
                                review.choices = choices;
                                review.saved_path = Some(path);
                                self.notice = Some((false, "Saved decisions reopened and checked against this recording and discovery result.".into()));
                            }
                            Err(error) => self.notice = Some((true, error)),
                        }
                    }
                }
            }
        }
        if self.case.is_some() && !self.polling && Instant::now() >= self.next_poll {
            let manifest = self.case.as_ref().unwrap().manifest.clone();
            self.next_poll = Instant::now() + Duration::from_secs(2);
            self.spawn(ctx, Lane::Poll, "", move || {
                gui_bridge::list_jobs(&manifest).map(Payload::Jobs)
            });
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn header(&self, ui: &mut egui::Ui, title: &str, subtitle: &str) {
        ui.heading(title);
        muted(ui, subtitle);
        ui.add_space(14.0);
    }

    fn intake(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        self.header(
            ui,
            "Case intake",
            "Start with a recording. Keep its clues and processing history together.",
        );
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.can_switch(), |ui| {
                if ui.button("Open saved case…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_parent(frame)
                        .set_title("Open a saved case.json")
                        .add_filter("Saved case", &["json"])
                        .pick_file()
                {
                    self.open_case(path, &ctx);
                }
                if self.case.is_some() && ui.button("New case").clicked() {
                    self.case = None;
                    self.studio = studio::Studio::default();
                    self.jobs.clear();
                    self.review = None;
                    self.selected_job = None;
                }
            });
            if self.dirty() {
                ui.label(
                    RichText::new("Save or discard review edits to switch cases.")
                        .color(tone(ui, AMBER)),
                );
            }
        });
        ui.add_space(12.0);
        if let Some(case) = self.case.clone() {
            self.case_details(ui, &ctx, case);
            return;
        }
        self.media_intake_card(ui);
        ui.add_space(12.0);
        card(ui, |ui| {
            ui.label(RichText::new("Create a case").size(20.0).strong());
            muted(
                ui,
                "The recording stays in its current location. Nothing is uploaded.",
            );
            ui.add_space(6.0);
            ui.label("Case title");
            accessible_text(
                ui,
                "case_title",
                &mut self.title,
                "For example: lakeside recording",
                false,
                f32::INFINITY,
            );
            ui.label("Recording");
            ui.horizontal(|ui| {
                accessible_path(
                    ui,
                    "recording_path",
                    &mut self.recording,
                    "Choose a local video or audio file",
                    (ui.available_width() - 105.0).max(100.0),
                );
                if ui.button("Browse…").clicked()
                    && let Some(p) = rfd::FileDialog::new()
                        .set_parent(frame)
                        .add_filter(
                            "Video or audio",
                            &["mp4", "mkv", "mov", "wav", "flac", "m4a", "mp3", "webm"],
                        )
                        .pick_file()
                {
                    self.recording = p.display().to_string();
                    if self.title.is_empty() {
                        self.title = p
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                    }
                }
            });
            ui.label("Save cases in");
            ui.horizontal(|ui| {
                accessible_path(
                    ui,
                    "case_parent",
                    &mut self.case_parent,
                    "Choose a workspace folder",
                    (ui.available_width() - 105.0).max(100.0),
                );
                if ui.button("Folder…").clicked()
                    && let Some(p) = rfd::FileDialog::new()
                        .set_parent(frame)
                        .set_title("Choose where to save new cases")
                        .pick_folder()
                {
                    self.case_parent = p.display().to_string();
                }
            });
            muted(
                ui,
                "A new case folder is created here. Existing cases are kept.",
            );
            ui.add_space(8.0);
            let ready = self.idle()
                && !self.title.trim().is_empty()
                && !self.recording.trim().is_empty()
                && !self.case_parent.trim().is_empty()
                && self.checked_probe_path().is_some()
                && self.companion.is_ok();
            ui.add_enabled_ui(ready, |ui| {
                if primary(ui, "Create case").clicked() {
                    let cli = self.companion.clone().unwrap();
                    let recording = PathBuf::from(self.recording.trim());
                    let output = PathBuf::from(self.case_parent.trim()).join(unique_name("case"));
                    let title = self.title.trim().to_owned();
                    let probe = self.checked_probe_path();
                    self.spawn(
                        &ctx,
                        Lane::Work,
                        "Checking and saving your recording…",
                        move || {
                            cli.create_case_with_probe(
                                &recording,
                                &output,
                                &title,
                                probe.as_deref(),
                            )
                            .map(Payload::Case)
                        },
                    );
                }
            });
        });
        ui.add_space(14.0);
        if !self.preferences.recent_cases.is_empty() {
            card(ui, |ui| {
                ui.label(RichText::new("Recent cases").strong());
                for path in self.preferences.recent_cases.clone() {
                    if ui
                        .add_enabled(
                            self.can_switch(),
                            egui::Button::new(display_path(path.parent().unwrap_or(&path))),
                        )
                        .clicked()
                    {
                        self.open_case(path, &ctx);
                    }
                }
            });
        }
        ui.add_space(14.0);
        muted(
            ui,
            "Next: discover possible flyovers, inspect their measured traces, then prepare a bounded search with flight tracks and terrain.",
        );
    }

    fn case_details(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, case: CaseOverview) {
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&case.case.title).size(22.0).strong());
                if case.recording_verified == Some(true) {
                    badge(ui, "RECORDING CHECKED", ACCENT);
                } else {
                    badge(ui, "RECORDING NEEDS CHECKING", AMBER);
                }
            });
            ui.add_space(8.0);
            ui.columns(3, |cols| {
                for (col, (label, value)) in cols.iter_mut().zip([
                    ("DURATION", seconds(case.case.recording.media.duration_s)),
                    (
                        "AUDIO CHANNELS",
                        case.case.recording.media.channels.to_string(),
                    ),
                    (
                        "SAMPLE RATE",
                        format!("{} Hz", case.case.recording.media.sample_rate_hz),
                    ),
                ]) {
                    muted(col, label);
                    col.label(RichText::new(value).size(23.0));
                }
            });
            ui.add_space(8.0);
            ui.collapsing("Recording and case details", |ui| {
                ui.label(format!("Recording: {}", display_path(&case.recording_path)));
                ui.label(format!("Case: {}", display_path(&case.manifest)));
                ui.label(format!("Recording SHA-256: {}", case.case.recording.sha256));
                muted(ui, "The check confirms the file matches this case. It does not confirm the recording's date or location.");
            });
            ui.horizontal(|ui| {
                ui.add_enabled_ui(self.idle(), |ui| {
                    if ui.button("Check recording again").clicked() {
                        self.open_case(case.manifest.clone(), ctx);
                    }
                });
                if ui.button("Open case folder").clicked() {
                    self.open_folder(case.manifest.parent().unwrap());
                }
            });
        });
        ui.add_space(14.0);
        self.studio.case_timing_ui(ui);
        ui.add_space(10.0);
        if primary(ui, "Continue to Audio review →").clicked() {
            self.page = Page::Review;
        }
        ui.add_space(14.0);
        card(ui, |ui| {
            ui.label(RichText::new("Clues").size(20.0).strong());
            muted(
                ui,
                "Record what you know. Clues do not automatically narrow the search.",
            );
            for clue in &case.hints {
                let kind = match clue.hint.kind {
                    HintKind::Region => "Region",
                    HintKind::DateTime => "Date / time",
                    HintKind::Other => "Other",
                };
                let certainty = match clue.hint.certainty {
                    HintCertainty::Uncertain => "uncertain",
                    HintCertainty::Reported => "reported",
                };
                ui.label(format!("{kind} · {certainty}: {}", clue.hint.text));
            }
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("hint_kind")
                    .selected_text(["Region", "Date / time (UTC)", "Other"][self.hint_kind])
                    .show_ui(ui, |ui| {
                        for (i, t) in ["Region", "Date / time (UTC)", "Other"].iter().enumerate() {
                            ui.selectable_value(&mut self.hint_kind, i, *t);
                        }
                    });
                ui.checkbox(&mut self.hint_reported, "Reported by the source");
            });
            accessible_text(
                ui,
                "clue",
                &mut self.hint_text,
                "For example: possibly a coastal region",
                true,
                f32::INFINITY,
            );
            ui.add_enabled_ui(
                self.idle() && self.companion.is_ok() && !self.hint_text.trim().is_empty(),
                |ui| {
                    if ui.button("Add clue").clicked() {
                        let cli = self.companion.clone().unwrap();
                        let manifest = case.manifest.clone();
                        let request = HintRequest {
                            kind: match self.hint_kind {
                                0 => HintKind::Region,
                                1 => HintKind::DateTime,
                                _ => HintKind::Other,
                            },
                            text: self.hint_text.trim().into(),
                            source: HintSource::User,
                            certainty: if self.hint_reported {
                                HintCertainty::Reported
                            } else {
                                HintCertainty::Uncertain
                            },
                        };
                        self.spawn(ctx, Lane::Work, "Saving the clue…", move || {
                            cli.add_hint(&manifest, &request)
                                .map(|case| Payload::HintAdded(case, request.text))
                        });
                    }
                },
            );
        });
    }

    fn open_folder(&mut self, path: &Path) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            if let Err(e) = std::process::Command::new("explorer.exe")
                .arg(path)
                .creation_flags(0x0800_0000)
                .spawn()
            {
                self.notice = Some((true, format!("Could not open folder: {e}")));
            }
        }
        #[cfg(not(windows))]
        {
            let _ = path;
            self.notice = Some((true, "Folder opening is supported on Windows.".into()));
        }
    }

    fn load_discovery(&mut self, path: PathBuf, ctx: &egui::Context) {
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        let channel = self.channel;
        self.spawn(
            ctx,
            Lane::Work,
            "Checking the recording and measured traces…",
            move || {
                let checksum = sha256(&path)?;
                event_review::load(&manifest, &path, &checksum, channel)
                    .map(|session| Payload::Review(Arc::new(session), None))
            },
        );
    }

    fn review_page(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        self.header(
            ui,
            "Event review",
            "Inspect possible flyovers before using them for matching.",
        );
        if self.case.is_none() {
            card(ui, |ui| {
                muted(
                    ui,
                    "Open or create a case first. Its recording will be checked against every discovery result.",
                );
            });
            return;
        }
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.can_switch(), |ui| {
                if ui.button("Open discovery result…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_parent(frame)
                        .set_title("Open discovery.json for this recording")
                        .add_filter("Discovery result", &["json"])
                        .pick_file()
                {
                    self.load_discovery(path, &ctx);
                }
            });
            muted(ui, format!("Expected audio channel: {}", self.channel + 1));
        });
        ui.add_space(12.0);
        if self.review.is_none() {
            card(ui, |ui| {
                ui.label(RichText::new("No event review open").size(22.0));
                muted(
                    ui,
                    "Run a discovery job, then choose Review events in Jobs. You can also open an existing discovery.json for this recording and channel.",
                );
                if primary(ui, "Go to jobs").clicked() {
                    self.page = Page::Jobs;
                }
            });
            return;
        }
        let idle = self.idle();
        let review = self.review.as_mut().unwrap();
        let overview = review.session.overview().clone();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                badge(ui, "MEASURED AUDIO", ACCENT);
                ui.label(format!(
                    "{} groups · {} ridges · channel {}",
                    overview.group_count,
                    overview.ridge_count,
                    overview.selected_channel_index + 1
                ));
                if review.dirty() {
                    badge(ui, "UNSAVED CHANGES", AMBER);
                }
            });
            muted(
                ui,
                "Related harmonics are grouped as one possible flyover. A kept event does not confirm a flight or location.",
            );
        });
        ui.add_space(12.0);
        if review.session.groups().is_empty() {
            card(ui, |ui| {
                ui.label("No proposals met the discovery settings.");
                muted(
                    ui,
                    "Faint, masked or nontonal aircraft may still be present. No events have been invented to fill the result.",
                );
            });
        } else {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(250.0, 440.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        card(ui, |ui| {
                            ui.label(RichText::new("Possible flyovers").strong());
                            egui::ScrollArea::vertical()
                                .id_salt("group_list")
                                .max_height(400.0)
                                .show(ui, |ui| {
                                    for (i, group) in review.session.groups().iter().enumerate() {
                                        let text = format!(
                                            "{}   {}\n{} – {}",
                                            group.harmonic_group_id,
                                            decision_name(review.choices[i].decision),
                                            seconds(group.start_s),
                                            seconds(group.end_s)
                                        );
                                        if ui
                                            .add_sized(
                                                [ui.available_width(), 56.0],
                                                egui::Button::new(text)
                                                    .selected(review.selected == i),
                                            )
                                            .clicked()
                                        {
                                            review.selected = i;
                                        }
                                    }
                                });
                        })
                    },
                );
                let group = &review.session.groups()[review.selected];
                ui.vertical(|ui| {
                    card(ui, |ui| {
                        ui.label(RichText::new(&group.harmonic_group_id).size(20.0).strong());
                        muted(
                            ui,
                            format!(
                                "{:.1}–{:.1} seconds into the recording",
                                group.start_s, group.end_s
                            ),
                        );
                        ridge_plot(ui, group);
                        let choice = &mut review.choices[review.selected];
                        ui.add_enabled_ui(idle, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                for decision in [
                                    ReviewDecision::Unreviewed,
                                    ReviewDecision::KeepForMatching,
                                    ReviewDecision::Reject,
                                    ReviewDecision::Uncertain,
                                ] {
                                    if ui
                                        .selectable_value(
                                            &mut choice.decision,
                                            decision,
                                            decision_name(decision),
                                        )
                                        .changed()
                                        && decision != ReviewDecision::KeepForMatching
                                    {
                                        choice.representative_event_id = None;
                                    }
                                }
                            });
                            if choice.decision == ReviewDecision::KeepForMatching {
                                egui::ComboBox::from_id_salt("representative")
                                    .selected_text(
                                        choice
                                            .representative_event_id
                                            .as_deref()
                                            .unwrap_or("Representative ridge: not chosen"),
                                    )
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut choice.representative_event_id,
                                            None,
                                            "Not chosen yet",
                                        );
                                        for ridge in &group.ridges {
                                            ui.selectable_value(
                                                &mut choice.representative_event_id,
                                                Some(ridge.event_id.clone()),
                                                &ridge.event_id,
                                            );
                                        }
                                    });
                            }
                        });
                        for (i, ridge) in group.ridges.iter().enumerate() {
                            ui.label(
                                RichText::new(format!(
                                    "{} · {} samples · {:.1}–{:.1} Hz",
                                    ridge.event_id,
                                    ridge.points.len(),
                                    ridge.band_hz[0],
                                    ridge.band_hz[1]
                                ))
                                .size(12.0)
                                .color(PALETTE[i % PALETTE.len()]),
                            );
                        }
                        muted(ui, "Dots are measured samples. Breaks remain unmeasured.");
                    })
                });
            });
        }
        ui.add_space(14.0);
        let mut save = false;
        let mut reopen = None;
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(idle, |ui| {
                save = primary(ui, "Save review snapshot").clicked();
            });
            ui.add_enabled_ui(idle && !review.dirty(), |ui| {
                if ui.button("Reopen saved decisions…").clicked() {
                    reopen = rfd::FileDialog::new()
                        .set_parent(frame)
                        .set_title("Choose a saved review snapshot folder")
                        .pick_folder();
                }
            });
            if review.dirty()
                && ui
                    .add_enabled(idle, egui::Button::new("Discard unsaved changes"))
                    .clicked()
            {
                review.choices = review.saved_choices.clone();
            }
        });
        ui.collapsing("Evidence details", |ui| {
            ui.label(format!(
                "Discovery: {}",
                display_path(&overview.discovery_path)
            ));
            ui.label(format!("Discovery SHA-256: {}", overview.discovery_sha256));
            if let Some(path) = &review.saved_path {
                ui.label(format!("Last saved: {}", display_path(path)));
            }
            for limit in &overview.limitations {
                muted(ui, limit);
            }
        });
        if save {
            let session = review.session.clone();
            let choices = review.choices.clone();
            let binding = review.binding.clone();
            let manifest = self.case.as_ref().unwrap().manifest.clone();
            let output = manifest
                .parent()
                .unwrap()
                .join("reviews")
                .join(unique_name("review"));
            self.spawn(
                &ctx,
                Lane::Work,
                "Checking evidence and saving your review…",
                move || {
                    verify_binding(&manifest, &binding)?;
                    event_review::save(&session, &choices, ReviewActor::Human, &output)?;
                    Ok(Payload::Saved(output, choices))
                },
            );
        } else if let Some(path) = reopen {
            let session = review.session.clone();
            let binding = review.binding.clone();
            let manifest = self.case.as_ref().unwrap().manifest.clone();
            self.spawn(&ctx, Lane::Work, "Checking saved decisions…", move || {
                verify_binding(&manifest, &binding)?;
                event_review::load_choices(&session, &path)
                    .map(|choices| Payload::Choices(choices, path))
            });
        }
    }

    fn jobs_page(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        self.header(
            ui,
            "Jobs",
            "Follow each stage. Interrupted work can resume without replacing earlier attempts.",
        );
        let Some(case) = self.case.as_ref() else {
            card(ui, |ui| {
                muted(ui, "Open a case to see its jobs.");
            });
            return;
        };
        let manifest = case.manifest.clone();
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.idle() && self.companion.is_ok(), |ui| {
                if ui.button("Import prepared job plan…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_parent(frame)
                        .set_title("Import a job plan for this case")
                        .add_filter("Job plan", &["json"])
                        .pick_file()
                {
                    let cli = self.companion.clone().unwrap();
                    let manifest = manifest.clone();
                    self.spawn(
                        &ctx,
                        Lane::Work,
                        "Preparing the imported job…",
                        move || cli.import_job_plan(&manifest, &path).map(Payload::Job),
                    );
                }
            });
            if ui.button("Refresh").clicked() {
                self.next_poll = Instant::now();
            }
        });
        ui.add_space(12.0);
        if self.jobs.is_empty() {
            card(ui, |ui| {
                ui.label(RichText::new("No jobs yet").size(22.0));
                muted(
                    ui,
                    "Prepare a discovery job from Case intake, or import a plan for terrain, ranking or overlays.",
                );
                if primary(ui, "Go to case intake").clicked() {
                    self.page = Page::Intake;
                }
            });
            return;
        }
        for job in self.jobs.clone() {
            let selected = self.selected_job.as_ref() == Some(&job.directory);
            let phase = job
                .status
                .as_ref()
                .map(|s| s.phase.as_str())
                .unwrap_or("unavailable");
            let name = job
                .directory
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            ui.horizontal(|ui| {
                if ui.selectable_label(selected, name).clicked() {
                    self.selected_job = Some(job.directory.clone());
                }
                badge(ui, phase_name(phase), phase_color(phase));
            });
        }
        ui.add_space(12.0);
        let Some(job) = self
            .jobs
            .iter()
            .find(|j| Some(&j.directory) == self.selected_job.as_ref())
            .cloned()
        else {
            muted(ui, "Select a job to inspect its stages.");
            return;
        };
        card(ui, |ui| {
            if let Some(problem) = &job.problem {
                ui.label(RichText::new(problem).color(tone(ui, RED)));
            }
            let Some(status) = &job.status else {
                muted(
                    ui,
                    "This job could not be read. Open its folder for the saved evidence.",
                );
                return;
            };
            let complete = status
                .steps
                .iter()
                .filter(|s| s.state == "completed")
                .count();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Stage progress").size(20.0).strong());
                badge(ui, phase_name(&status.phase), phase_color(&status.phase));
            });
            ui.add(
                egui::ProgressBar::new(complete as f32 / status.steps.len().max(1) as f32)
                    .fill(ACCENT)
                    .text(format!(
                        "{complete} of {} stages complete",
                        status.steps.len()
                    )),
            );
            if status.phase == "running" {
                ui.horizontal(|ui| { activity::indicator(ui); muted(ui,"Working. A stage may take several minutes; this bar counts completed stages."); });
            }
            for step in &status.steps {
                ui.horizontal(|ui| {
                    badge(ui, phase_name(&step.state), phase_color(&step.state));
                    ui.label(RichText::new(&step.id).strong());
                    muted(
                        ui,
                        format!(
                            "Attempt {}{}",
                            step.attempt,
                            if step.reused {
                                " · reused after checking"
                            } else {
                                ""
                            }
                        ),
                    );
                });
            }
            if let Some(message) = &status.message {
                ui.label(message);
            }
            if status.cancel_requested
                && (status.phase == "running" || self.running.as_ref() == Some(&job.directory))
            {
                ui.label(
                    RichText::new("Cancellation requested. Waiting for the current stage to stop.")
                        .color(tone(ui, AMBER)),
                );
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(
                    self.idle() && self.companion.is_ok() && status.phase != "running",
                    |ui| {
                        let label = if status.phase == "pending" {
                            "Start job"
                        } else {
                            "Resume / check completed stages"
                        };
                        if primary(ui, label).clicked() {
                            let cli = self.companion.clone().unwrap();
                            let case = manifest.clone();
                            let path = job.directory.clone();
                            let resume = status.phase != "pending";
                            self.running = Some(path.clone());
                            self.notice = None;
                            self.spawn(&ctx, Lane::Run, "", move || {
                                cli.run_job(&case, &path, resume).map(Payload::Job)
                            });
                        }
                    },
                );
                ui.add_enabled_ui(
                    !self.canceling
                        && self.companion.is_ok()
                        && (status.phase == "running"
                            || self.running.as_ref() == Some(&job.directory)),
                    |ui| {
                        if ui.button("Cancel job").clicked() {
                            self.cancel_job(&ctx, &manifest, &job.directory);
                        }
                    },
                );
                if ui.button("Open job folder").clicked() {
                    self.open_folder(&job.directory);
                }
            });
            for artifact in &job.discovery_artifacts {
                ui.add_enabled_ui(self.can_switch(), |ui| {
                    if primary(ui, "Review events").clicked() {
                        let case = manifest.clone();
                        let path = job.directory.clone();
                        let step = artifact.step_id.clone();
                        self.spawn(
                            &ctx,
                            Lane::Work,
                            "Checking completed discovery evidence…",
                            move || {
                                let binding =
                                    gui_bridge::verified_discovery_artifact(&case, &path, &step)?;
                                event_review::load(
                                    &case,
                                    &binding.path,
                                    &binding.sha256,
                                    binding.channel,
                                )
                                .map(|session| Payload::Review(Arc::new(session), Some(binding)))
                            },
                        );
                    }
                });
            }
            ui.collapsing("Recovery and details", |ui| {
                ui.label(format!("Job: {}",display_path(&job.directory)));
                muted(ui,"Recover only after an interrupted runner has stopped. A live or uncertain process is refused. Then resume the job.");
                ui.add_enabled_ui(self.idle() && self.companion.is_ok(),|ui| {
                    if ui.button("Recover interrupted job").clicked() {
                        let cli=self.companion.clone().unwrap();let case=manifest.clone();let path=job.directory.clone();
                        self.spawn(&ctx,Lane::Work,"Checking whether the interrupted job can be recovered…",move || cli.recover_job(&case,&path).map(Payload::Job));
                    }
                });
                muted(ui,"Completed means the stages finished. No location is marked verified by this interface.");
            });
        });
    }

    fn cancel_job(&mut self, ctx: &egui::Context, manifest: &Path, directory: &Path) {
        let cli = self.companion.clone().unwrap();
        let manifest = manifest.to_path_buf();
        let path = directory.to_path_buf();
        self.spawn(ctx, Lane::Cancel, "", move || {
            cli.cancel_job(&manifest, &path).map(Payload::Job)
        });
    }

    fn settings(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        self.header(
            ui,
            "Settings",
            "Local tools for decoding audio and inspecting recordings.",
        );
        self.media_settings_card(ui, frame);
        ui.add_space(14.0);
        card(ui, |ui| {
            ui.label(RichText::new("Processing program").size(20.0).strong());
            match &self.companion {
                Ok(cli) => {
                    ui.label(display_path(cli.path()));
                }
                Err(error) => {
                    ui.label(RichText::new(display_path_text(error)).color(tone(ui, RED)));
                }
            }
            muted(
                ui,
                "Keep geosonnar-gui.exe and geosonnar.exe together. Cases keep a reference to the original recording; moving the media can require a new case or job.",
            );
            ui.add_space(8.0);
            muted(
                ui,
                "Review measured spectra and video, edit traces, choose evidence, inspect map layers, and manage case data. Thunder proposals and timing comparisons are experimental and require manual review.",
            );
        });
    }

    fn closing(&mut self, ctx: &egui::Context) {
        if self.allow_close {
            return;
        }
        if ctx.input(|input| input.viewport().close_requested())
            && (!self.idle()
                || self.studio.active_for_exit()
                || self.dirty()
                || self.shutdown.requested())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_requested = true;
        }
        if self.shutdown.requested() {
            if self.shutdown.ready(Instant::now()) {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                egui::Modal::new(egui::Id::new("close_guard")).show(ctx, |ui| {
                    ui.set_max_width(450.0);
                    ui.heading("Stopping work and closing");
                    activity::indicator(ui);
                    muted(ui, "Downloads, searches and playback have been asked to stop. GeoSonnar will close within two seconds and stop its remaining helper processes.");
                    muted(ui, "Unsaved review edits and results will be discarded. Existing files and partial downloads stay on disk.");
                });
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            return;
        }
        if self.close_requested {
            egui::Modal::new(egui::Id::new("close_guard")).show(ctx, |ui| {
                ui.set_max_width(470.0);
                ui.heading("Before closing");
                let active = !self.idle() || self.studio.active_for_exit();
                if active {
                    muted(ui, "Work is still active. Cancel all and exit stops downloads, searches, playback and this window's processing jobs, then closes GeoSonnar.");
                    muted(ui, "Completed files and partial downloads are retained. An interrupted processing job may need recovery before resuming.");
                }
                if self.dirty() {
                    ui.label(RichText::new("Unsaved review edits and results will be discarded.").strong());
                    if self.studio.has_unsaved_results() {
                        muted(ui, "A completed result has not been saved. Return to Results to retry saving or explicitly discard it.");
                    }
                    muted(ui, "Choose Keep working to save them first. Closing does not automatically save or approve evidence.");
                }
                if active {
                    if ui.button("Cancel all and exit").clicked() {
                        self.begin_cancel_and_exit(ctx);
                    }
                } else if self.dirty() {
                    if ui.button("Discard unsaved work and exit").clicked() {
                        self.begin_cancel_and_exit(ctx);
                    }
                } else if ui.button("Exit GeoSonnar").clicked() {
                    self.begin_cancel_and_exit(ctx);
                }
                if !self.shutdown.requested() && ui.button("Keep working").clicked() {
                    self.close_requested = false;
                }
            });
        }
    }
}

fn verify_binding(
    manifest: &Path,
    binding: &Option<VerifiedDiscoveryArtifact>,
) -> Result<(), String> {
    if let Some(old) = binding {
        let current =
            gui_bridge::verified_discovery_artifact(manifest, &old.job_directory, &old.step_id)?;
        if current.path != old.path
            || current.sha256 != old.sha256
            || current.channel != old.channel
        {
            return Err(
                "The job's discovery evidence changed. Reopen and review its current result."
                    .into(),
            );
        }
    }
    Ok(())
}

fn ordered_choices(
    session: &ReviewSession,
    choices: Vec<GroupChoice>,
) -> Result<Vec<GroupChoice>, String> {
    let count = choices.len();
    let mut by_id: std::collections::BTreeMap<_, _> = choices
        .into_iter()
        .map(|choice| (choice.harmonic_group_id.clone(), choice))
        .collect();
    if count != session.groups().len() || by_id.len() != count {
        return Err("Saved review has missing or duplicate group choices.".into());
    }
    session
        .groups()
        .iter()
        .map(|group| {
            by_id
                .remove(&group.harmonic_group_id)
                .ok_or_else(|| "Saved review does not match the displayed groups.".into())
        })
        .collect()
}

impl eframe::App for Workbench {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.shutdown.requested() {
            self.studio.tick(ctx);
            self.update_messages(ctx);
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }
        self.studio.detailed_controls = self.preferences.detailed_controls;
        self.media_setup.poll();
        let base = self.preferences_path.parent().unwrap_or(Path::new("."));
        self.studio.tools = self.media_setup.studio_tools(&self.preferences, base);
        self.studio.tick(ctx);
        self.update_messages(ctx);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.shutdown.requested() {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.heading("Closing GeoSonnar");
            });
            self.closing(&ctx);
            return;
        }
        egui::Panel::top("navigation")
            .frame(egui::Frame::new().fill(ui.visuals().window_fill).inner_margin(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("GeoSonnar").size(22.0).strong());
                    ui.separator();
                    ui.add(egui::Label::new(RichText::new(
                        self.case
                            .as_ref()
                            .map(|c| c.case.title.as_str())
                            .unwrap_or("No case open"),
                    ).strong()).truncate());
                });
                ui.horizontal_wrapped(|ui| {
                    for (index, (page, label)) in [
                        (Page::Intake, "1  Case"),
                        (Page::Review, "2  Audio review"),
                        (Page::Data, "3  Data"),
                        (Page::Map, "4  Search"),
                        (Page::Results, "5  Results"),
                    ].into_iter().enumerate() {
                        if index > 0 { muted(ui, "→"); }
                        ui.selectable_value(&mut self.page, page, label);
                    }
                    ui.separator();
                    ui.selectable_value(&mut self.page, Page::Jobs, "Jobs");
                    ui.selectable_value(&mut self.page, Page::Settings, "Settings");
                    ui.separator();
                    let changed = ui.selectable_value(&mut self.preferences.detailed_controls, false, "Guided view").on_hover_text("Focus on the current step; all settings and evidence stay the same").changed()
                        | ui.selectable_value(&mut self.preferences.detailed_controls, true, "All controls").on_hover_text("Show secondary controls without changing your settings").changed();
                    if changed {
                        self.studio.detailed_controls = self.preferences.detailed_controls;
                        self.save_preferences();
                    }
                });
                if self.preferences.detailed_controls { muted(ui, match self.page {
                    Page::Intake => "Choose a recording and save the date, time and location clues you know.",
                    Page::Review => "Load an interval, find candidate sounds, then listen, trace and confirm your evidence.",
                    Page::Data => "Import or request aircraft, lightning and terrain data for this case.",
                    Page::Map => "Draw a search area, set the grid, associate events and choose the evidence to compare.",
                    Page::Results => "Sort candidate locations by score and inspect the evidence behind each result.",
                    Page::Jobs => "Start prepared jobs and follow their progress. Return to Audio review to inspect detections.",
                    Page::Settings => "Choose local media tools. Use the sun button below to change the appearance.",
                    Page::Groups => "Advanced review of imported Doppler groups. Return to Audio review for the recording.",
                }); }
                if !self.preferences.detailed_controls {
                    let (tip, next) = match self.page {
                        Page::Intake => ("Start here: open a recording, then save the clues you know.", None),
                        Page::Review => ("Load audio → find candidates → listen and confirm measurements. Review source identity separately.", Some((Page::Data, "Continue to Data →"))),
                        Page::Data => ("Get data → import → load. Stored datasets contains resume, load and delete actions.", Some((Page::Map, "Continue to Search →"))),
                        Page::Map => ("Set area and time → choose reviewed evidence → estimate the work before running.", Some((Page::Results, "Open Results →"))),
                        Page::Results => ("Compare candidate locations, inspect their fit, then save the investigation.", Some((Page::Review, "Return to Audio review"))),
                        _ => ("Use the numbered tabs to return to the investigation.", None),
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tip);
                        if let Some((page, label)) = next
                            && ui.button(label).on_hover_text("Navigate only; does not approve evidence or run work").clicked() {
                                self.page = page;
                            }
                    });
                }
            });
        egui::Panel::bottom("status_bar")
            .frame(
                egui::Frame::new()
                    .fill(ui.visuals().window_fill)
                    .inner_margin(10),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(label) = &self.busy {
                        activity::indicator(ui);
                        ui.label(label);
                    } else if self.running.is_some() {
                        activity::indicator(ui);
                        ui.label("Job running · progress in Jobs");
                    } else {
                        muted(ui, "Ready");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if sun_toggle(ui, self.preferences.dark_mode).clicked() {
                            self.preferences.dark_mode = !self.preferences.dark_mode;
                            theme(ui.ctx(), self.preferences.dark_mode);
                            self.save_preferences();
                        }
                        muted(ui, concat!("v", env!("CARGO_PKG_VERSION")));
                    });
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(8),
            )
            .show(ui, |ui| {
                if let Err(error) = &self.companion {
                    ui.label(RichText::new(display_path_text(error)).color(tone(ui, RED)));
                }
                if let Some((error, message)) = self.notice.clone() {
                    egui::Frame::new()
                        .fill(ui.visuals().window_fill)
                        .stroke(egui::Stroke::new(1.0, if error { RED } else { BORDER }))
                        .corner_radius(8)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(display_path_text(&message))
                                        .color(tone(ui, if error { RED } else { ACCENT })),
                                );
                                if ui.small_button("Dismiss").clicked() {
                                    self.notice = None;
                                }
                            });
                        });
                    ui.add_space(12.0);
                }
                let action = match self.page {
                    Page::Review => self.studio.analysis_ui(ui),
                    Page::Map => {
                        self.studio.map_ui(ui);
                        None
                    }
                    Page::Data => {
                        self.studio.library_ui(ui);
                        None
                    }
                    Page::Results => {
                        self.studio.results_ui(ui);
                        None
                    }
                    _ => {
                        egui::ScrollArea::vertical()
                            .id_salt("main_content")
                            .auto_shrink([false, false])
                            .show(ui, |ui| match self.page {
                                Page::Intake => self.intake(ui, frame),
                                Page::Groups => self.review_page(ui, frame),
                                Page::Jobs => self.jobs_page(ui, frame),
                                Page::Settings => self.settings(ui, frame),
                                _ => {}
                            });
                        None
                    }
                };
                if let Some(action) = action.or_else(|| self.studio.take_pending_action()) {
                    match action {
                        studio::Action::ReviewResults => self.page = Page::Results,
                        studio::Action::ReviewInspection | studio::Action::ReviewEvidence => {
                            self.page = Page::Review
                        }
                        studio::Action::OpenDiscovery => {
                            if self.review.as_ref().is_some_and(ReviewView::dirty) {
                                self.notice = Some((
                                    true,
                                    "Save or discard the old group-review choices first.".into(),
                                ));
                            } else if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Discovery report", &["json"])
                                .pick_file()
                            {
                                self.channel = self.studio.channel();
                                self.load_discovery(path, &ctx);
                            }
                        }
                        studio::Action::LegacyReview => self.page = Page::Groups,
                        studio::Action::PrepareDiscovery(settings) => {
                            let checked_tools = self.checked_analysis_tools();
                            if let Err(error) = &checked_tools {
                                self.notice = Some((true, error.clone()));
                                self.page = Page::Settings;
                            } else if let (Some(case), Ok(cli), Ok(tools)) =
                                (&self.case, &self.companion, checked_tools)
                            {
                                let cli = cli.clone();
                                let manifest = case.manifest.clone();
                                let channel = self.studio.channel();
                                self.spawn(
                                    &ctx,
                                    Lane::Work,
                                    "Preparing detection with your thresholds…",
                                    move || {
                                        cli.prepare_discovery_job_with_settings(
                                            &manifest,
                                            channel,
                                            &tools,
                                            Some(&settings),
                                        )
                                        .map(Payload::Job)
                                    },
                                );
                            }
                        }
                    }
                }
            });
        self.closing(&ctx);
        startup::mark(startup::Stage::FirstUiFrame);
    }
}
