//! Cached, scoped local-helper checks. No helper process is started here.
use super::*;
use geosonnar::media_tools::{self, Tool};

#[derive(Debug)]
struct CheckedTool {
    tool: Tool,
    configured: String,
    path: Result<PathBuf, String>,
}

struct CheckRequest {
    configured: [String; 3],
    base: PathBuf,
    ctx: egui::Context,
}

#[derive(Default)]
pub(super) struct MediaSetup {
    checked: [Option<CheckedTool>; 3],
    pending: Option<Receiver<CheckedTool>>,
    queued: Option<CheckRequest>,
}

fn configured(preferences: &Preferences) -> [String; 3] {
    [
        preferences.ffmpeg.clone(),
        preferences.ffprobe.clone(),
        preferences.ffplay.clone(),
    ]
}

fn selection(preferences: &Preferences, tool: Tool) -> &str {
    match tool {
        Tool::Ffmpeg => &preferences.ffmpeg,
        Tool::Ffprobe => &preferences.ffprobe,
        Tool::Ffplay => &preferences.ffplay,
    }
}

fn index(tool: Tool) -> usize {
    match tool {
        Tool::Ffmpeg => 0,
        Tool::Ffprobe => 1,
        Tool::Ffplay => 2,
    }
}

impl MediaSetup {
    #[cfg(test)]
    pub(super) fn check_in_progress(&self) -> bool {
        self.pending.is_some()
    }

    fn result(&self, preferences: &Preferences, tool: Tool) -> Option<&Result<PathBuf, String>> {
        self.checked[index(tool)]
            .as_ref()
            .filter(|checked| checked.configured == selection(preferences, tool))
            .map(|checked| &checked.path)
    }

    pub(super) fn found(&self, preferences: &Preferences, tool: Tool) -> bool {
        self.result(preferences, tool).is_some_and(Result::is_ok)
    }

    fn start(&mut self, preferences: &Preferences, base: &Path, ctx: &egui::Context) {
        let request = CheckRequest {
            configured: configured(preferences),
            base: base.to_path_buf(),
            ctx: ctx.clone(),
        };
        // A Windows metadata lookup cannot safely be cancelled. Keep only the
        // latest requested replacement while the one active worker finishes.
        if self.pending.is_some() {
            self.queued = Some(request);
            return;
        }
        self.launch(request);
    }

    fn launch(&mut self, request: CheckRequest) {
        let CheckRequest {
            configured,
            base,
            ctx,
        } = request;
        let (sender, receiver) = mpsc::channel();
        // Keep a ready, unchanged probe usable while an optional tool is
        // refreshed. Every lookup still checks the exact current selection.
        for (i, checked) in self.checked.iter_mut().enumerate() {
            if checked
                .as_ref()
                .is_some_and(|value| value.configured != configured[i])
            {
                *checked = None;
            }
        }
        self.pending = Some(receiver);
        std::thread::spawn(move || {
            // Intake does not wait for the decoder or optional player check.
            for tool in [Tool::Ffprobe, Tool::Ffmpeg, Tool::Ffplay] {
                let i = index(tool);
                let path = std::panic::catch_unwind(|| {
                    let selected = configured[i].trim();
                    let explicit = (!selected.is_empty()).then_some(Path::new(selected));
                    media_tools::resolve(tool, explicit, &base)
                })
                .unwrap_or_else(|_| {
                    Err("Local tool check stopped. Choose Check paths to try again.".into())
                });
                if sender
                    .send(CheckedTool {
                        tool,
                        configured: configured[i].clone(),
                        path,
                    })
                    .is_err()
                {
                    return;
                }
                ctx.request_repaint();
            }
            // Poll waits for disconnection, not just the last tool's reply.
            // Wake it after the sender closes so a queued request can start.
            drop(sender);
            ctx.request_repaint();
        });
    }

    fn completed_request(&mut self) -> Option<CheckRequest> {
        loop {
            let receiver = self.pending.as_ref()?;
            match receiver.try_recv() {
                Ok(checked) => {
                    let i = index(checked.tool);
                    self.checked[i] = Some(checked);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    return self.queued.take();
                }
                Err(mpsc::TryRecvError::Empty) => return None,
            }
        }
    }

    pub(super) fn poll(&mut self) {
        if let Some(request) = self.completed_request() {
            self.launch(request);
        }
    }

    fn waiting_note(&self, ui: &mut egui::Ui) {
        if self.queued.is_some() {
            muted(
                ui,
                "Your latest selection will be checked next. If the current check does not finish, save settings and reopen GeoSonnar.",
            );
        } else if self.pending.is_some() {
            muted(
                ui,
                "Checking tool locations. You can change and save selections while this runs.",
            );
        }
    }

    fn execution_path(&self, preferences: &Preferences, base: &Path, tool: Tool) -> PathBuf {
        if let Some(Ok(path)) = self.result(preferences, tool) {
            return path.clone();
        }
        let selected = selection(preferences, tool);
        if selected.trim().is_empty() {
            PathBuf::new()
        } else {
            // Keep an invalid explicit choice explicit. Turning it into blank
            // would let downstream automatic discovery silently replace it.
            let path = PathBuf::from(selected.trim());
            if path.is_absolute() {
                path
            } else {
                base.join(path)
            }
        }
    }

    pub(super) fn studio_tools(&self, preferences: &Preferences, base: &Path) -> studio::Tools {
        studio::Tools {
            ffmpeg: self.execution_path(preferences, base, Tool::Ffmpeg),
            ffprobe: self.execution_path(preferences, base, Tool::Ffprobe),
            ffplay: self.execution_path(preferences, base, Tool::Ffplay),
        }
    }
}

impl Workbench {
    pub(super) fn check_media_tools(&mut self, ctx: &egui::Context) {
        let base = self.preferences_path.parent().unwrap_or(Path::new("."));
        self.media_setup.start(&self.preferences, base, ctx);
    }

    fn automatic_media_tools(&mut self, ctx: &egui::Context) {
        self.preferences.ffmpeg.clear();
        self.preferences.ffprobe.clear();
        self.preferences.ffplay.clear();
        self.save_preferences();
        self.check_media_tools(ctx);
    }

    pub(super) fn media_intake_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.label(RichText::new("Media setup").strong());
            ui.horizontal_wrapped(|ui| {
                for tool in [Tool::Ffprobe, Tool::Ffmpeg] {
                    let state = match self.media_setup.result(&self.preferences, tool) {
                        Some(Ok(_)) => "Found",
                        Some(Err(_)) => "Missing",
                        None if self.media_setup.queued.is_some() => "Waiting for earlier check…",
                        None if self.media_setup.pending.is_some() => "Checking…",
                        None => "Check paths",
                    };
                    ui.label(format!("{}: {state}", tool.label()));
                }
            });
            if !self.media_setup.found(&self.preferences, Tool::Ffprobe) {
                muted(
                    ui,
                    "FFprobe is needed to create a case. Use Auto-detect or choose its executable in Settings.",
                );
            } else if !self.media_setup.found(&self.preferences, Tool::Ffmpeg) {
                muted(
                    ui,
                    "You can create a case now. Add FFmpeg before decoding audio or video in Audio review.",
                );
            } else {
                muted(
                    ui,
                    "Ready to register and inspect recordings. Tools are run only when you request work.",
                );
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button("Auto-detect")
                    .on_hover_text("Use automatic discovery for all media tools instead of saved manual paths.").clicked()
                {
                    self.automatic_media_tools(ui.ctx());
                }
                if ui.button("Browse tools / Settings").clicked() {
                    self.page = Page::Settings;
                }
                if ui.button("Check again").clicked() {
                    self.check_media_tools(ui.ctx());
                }
            });
            self.media_setup.waiting_note(ui);
        });
    }

    pub(super) fn media_settings_card(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        card(ui, |ui| {
            ui.label(RichText::new("Media tools").size(20.0).strong());
            muted(
                ui,
                "FFprobe reads recording details. FFmpeg decodes audio and video. FFplay is optional and only provides audio playback.",
            );
            muted(
                ui,
                "Blank paths search GeoSonnar's tools folder, beside the program, the actual Chocolatey ffmpeg-full install, then local absolute PATH entries. Put the actual executables in tools, or Browse to installed files. Package-manager launch shortcuts are not supported.",
            );
            let previous = configured(&self.preferences);
            let mut picked = false;
            for tool in [Tool::Ffprobe, Tool::Ffmpeg, Tool::Ffplay] {
                let label = match tool {
                    Tool::Ffprobe => "FFprobe — required for case intake",
                    Tool::Ffmpeg => "FFmpeg — required for audio and video review",
                    Tool::Ffplay => "FFplay — optional audio playback",
                };
                ui.add_space(6.0);
                ui.label(label);
                let value = match tool {
                    Tool::Ffmpeg => &mut self.preferences.ffmpeg,
                    Tool::Ffprobe => &mut self.preferences.ffprobe,
                    Tool::Ffplay => &mut self.preferences.ffplay,
                };
                ui.horizontal(|ui| {
                    accessible_path(
                        ui,
                        tool.label(),
                        value,
                        "Automatic when blank",
                        (ui.available_width() - 105.0).max(100.0),
                    );
                    if ui.button("Browse…").clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .set_parent(frame)
                            .set_title(format!("Choose {}", tool.filename()))
                            .add_filter("Executable", &["exe"])
                            .pick_file()
                    {
                        *value = path.display().to_string();
                        picked = true;
                    }
                });
                match self.media_setup.result(&self.preferences, tool) {
                    Some(Ok(path)) => muted(ui, format!("Found: {}", display_path(path))),
                    Some(Err(reason)) => {
                        ui.label(RichText::new("Missing").color(tone(ui, AMBER)));
                        muted(ui, display_path_text(reason));
                    }
                    None if self.media_setup.queued.is_some() => {
                        muted(ui, "Waiting for the earlier file check…")
                    }
                    None if self.media_setup.pending.is_some() => {
                        muted(ui, "Checking local files…")
                    }
                    None => muted(ui, "Path changed or not checked. Choose Check paths."),
                }
            }
            let cleared = previous
                .iter()
                .zip(configured(&self.preferences))
                .any(|(before, after)| !before.trim().is_empty() && after.trim().is_empty());
            if picked || cleared {
                // Persist recovery selections even if an old metadata lookup
                // never returns and the user must close and reopen the app.
                self.save_preferences();
                self.check_media_tools(ui.ctx());
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Check paths").clicked() {
                    self.check_media_tools(ui.ctx());
                }
                if ui
                    .button("Auto-detect")
                    .on_hover_text(
                        "Clear saved manual selections and use portable automatic discovery.",
                    )
                    .clicked()
                {
                    self.automatic_media_tools(ui.ctx());
                }
                if primary(ui, "Save settings").clicked() {
                    self.notice = Some((
                        false,
                        "Media tool selections saved for this portable copy.".into(),
                    ));
                    self.save_preferences();
                    self.check_media_tools(ui.ctx());
                }
                if ui.button("Back to Case").clicked() {
                    self.page = Page::Intake;
                }
            });
            self.media_setup.waiting_note(ui);
            muted(
                ui,
                "Found checks the local file location. Compatibility is checked when a tool is used. A missing saved manual path is kept until you change it or choose Auto-detect.",
            );
            muted(
                ui,
                format!("Settings file: {}", display_path(&self.preferences_path)),
            );
        });
    }

    pub(super) fn checked_analysis_tools(&self) -> Result<MediaTools, String> {
        let path = |tool| match self.media_setup.result(&self.preferences, tool) {
            Some(Ok(path)) => Ok(path.clone()),
            _ => Err(format!(
                "{} is needed for audio analysis. Open Settings, choose its file or Auto-detect, then Check paths.",
                tool.label()
            )),
        };
        Ok(MediaTools {
            ffmpeg: Some(path(Tool::Ffmpeg)?),
            ffprobe: Some(path(Tool::Ffprobe)?),
        })
    }

    pub(super) fn checked_probe_path(&self) -> Option<PathBuf> {
        self.media_setup
            .result(&self.preferences, Tool::Ffprobe)
            .and_then(|result| result.as_ref().ok())
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_only(preferences: &Preferences) -> MediaSetup {
        MediaSetup {
            checked: std::array::from_fn(|i| {
                let tool = [Tool::Ffmpeg, Tool::Ffprobe, Tool::Ffplay][i];
                Some(CheckedTool {
                    tool,
                    configured: selection(preferences, tool).into(),
                    path: if tool == Tool::Ffprobe {
                        Ok(PathBuf::from("checked-ffprobe.exe"))
                    } else {
                        Err(format!("{} missing", tool.label()))
                    },
                })
            }),
            pending: None,
            queued: None,
        }
    }

    #[test]
    fn intake_probe_can_be_ready_without_decode_or_playback_tools() {
        let preferences = Preferences::default();
        let setup = probe_only(&preferences);
        assert!(setup.found(&preferences, Tool::Ffprobe));
        assert!(!setup.found(&preferences, Tool::Ffmpeg));
        assert!(!setup.found(&preferences, Tool::Ffplay));
        assert!(
            preferences.ffprobe.is_empty(),
            "automatic paths stay portable"
        );
    }

    #[test]
    fn old_path_reply_cannot_enable_a_changed_selection() {
        let mut preferences = Preferences::default();
        let (sender, receiver) = mpsc::channel();
        let mut setup = MediaSetup {
            pending: Some(receiver),
            ..MediaSetup::default()
        };
        sender
            .send(probe_only(&preferences).checked[1].take().unwrap())
            .unwrap();
        drop(sender);
        preferences.ffprobe = "different-explicit-probe.exe".into();
        setup.poll();
        assert!(setup.pending.is_none());
        assert!(!setup.found(&preferences, Tool::Ffprobe));
        assert!(setup.result(&preferences, Tool::Ffprobe).is_none());
    }

    #[test]
    fn invalid_explicit_tool_stays_explicit_for_execution() {
        let preferences = Preferences {
            ffmpeg: "missing-manual-ffmpeg.exe".into(),
            ..Preferences::default()
        };
        let setup = probe_only(&preferences);
        let base = Path::new("portable-folder");
        assert_eq!(
            setup.studio_tools(&preferences, base).ffmpeg,
            base.join("missing-manual-ffmpeg.exe")
        );
        assert_eq!(preferences.ffmpeg, "missing-manual-ffmpeg.exe");
    }

    #[test]
    fn default_status_does_not_start_a_worker_or_resolve_tools() {
        let setup = MediaSetup::default();
        let preferences = Preferences::default();
        assert!(
            setup.pending.is_none()
                && setup.queued.is_none()
                && setup.checked.iter().all(Option::is_none)
        );
        for tool in [Tool::Ffprobe, Tool::Ffmpeg, Tool::Ffplay] {
            assert!(!setup.found(&preferences, tool));
            assert!(
                setup
                    .execution_path(&preferences, Path::new("portable"), tool)
                    .as_os_str()
                    .is_empty()
            );
        }
    }

    #[test]
    fn closing_drops_check_receiver_without_waiting_for_the_worker() {
        let (sender, receiver) = mpsc::channel();
        let mut setup = MediaSetup {
            pending: Some(receiver),
            ..MediaSetup::default()
        };
        setup.start(
            &Preferences::default(),
            Path::new("portable"),
            &egui::Context::default(),
        );
        assert!(
            setup.queued.is_some(),
            "replacement is retained without another worker"
        );
        drop(setup);
        assert!(
            sender
                .send(
                    probe_only(&Preferences::default()).checked[1]
                        .take()
                        .unwrap()
                )
                .is_err()
        );
    }

    #[test]
    fn latest_refresh_waits_for_worker_disconnect_even_after_last_tool_reply() {
        let (sender, receiver) = mpsc::channel();
        let mut setup = MediaSetup {
            pending: Some(receiver),
            ..MediaSetup::default()
        };
        let ctx = egui::Context::default();
        let mut preferences = Preferences::default();
        for i in 0..32 {
            preferences.ffprobe = format!("chosen-probe-{i}.exe");
            setup.start(&preferences, Path::new("new-portable-folder"), &ctx);
            assert!(setup.completed_request().is_none());
        }
        let mut old_reply = probe_only(&Preferences::default());
        sender
            .send(old_reply.checked[index(Tool::Ffplay)].take().unwrap())
            .unwrap();
        assert!(
            setup.completed_request().is_none(),
            "a final reply is not worker completion"
        );
        assert!(setup.pending.is_some());
        assert_eq!(
            setup.queued.as_ref().unwrap().configured[1],
            "chosen-probe-31.exe"
        );
        drop(sender);
        let next = setup
            .completed_request()
            .expect("only latest replacement becomes runnable");
        assert_eq!(next.configured, configured(&preferences));
        assert_eq!(next.base, PathBuf::from("new-portable-folder"));
        assert!(setup.pending.is_none() && setup.queued.is_none());
        assert!(
            setup.completed_request().is_none(),
            "replacement can be consumed only once"
        );
    }

    #[test]
    fn queued_optional_change_keeps_ready_probe_and_rejects_stale_player_reply() {
        let mut preferences = Preferences::default();
        let (sender, receiver) = mpsc::channel();
        let mut setup = probe_only(&preferences);
        setup.pending = Some(receiver);
        preferences.ffplay = "replacement-player.exe".into();
        setup.start(
            &preferences,
            Path::new("portable"),
            &egui::Context::default(),
        );
        sender
            .send(
                probe_only(&Preferences::default()).checked[index(Tool::Ffplay)]
                    .take()
                    .unwrap(),
            )
            .unwrap();
        setup.poll();
        assert!(setup.found(&preferences, Tool::Ffprobe));
        assert!(setup.result(&preferences, Tool::Ffplay).is_none());
        assert!(setup.pending.is_some() && setup.queued.is_some());
        drop(sender);
        let request = setup.completed_request().unwrap();
        assert_eq!(request.configured[index(Tool::Ffplay)], preferences.ffplay);
        assert!(setup.found(&preferences, Tool::Ffprobe));
    }

    #[test]
    fn required_probe_arrives_before_optional_check_and_survives_its_path_edits() {
        let mut preferences = Preferences::default();
        let (sender, receiver) = mpsc::channel();
        let mut setup = MediaSetup {
            pending: Some(receiver),
            ..MediaSetup::default()
        };
        sender
            .send(probe_only(&preferences).checked[1].take().unwrap())
            .unwrap();
        setup.poll();
        assert!(
            setup.pending.is_some(),
            "remaining metadata checks may still be pending"
        );
        assert!(setup.found(&preferences, Tool::Ffprobe));
        preferences.ffplay = "optional-new-player.exe".into();
        assert!(setup.found(&preferences, Tool::Ffprobe));
        assert!(setup.result(&preferences, Tool::Ffplay).is_none());
    }
}
