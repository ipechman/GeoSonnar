//! Editing responsibilities for the review workspace.
use super::*;

impl Studio {
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
    pub(super) fn checkpoint(&mut self, before: ReviewWorkspace) {
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
    pub(super) fn undo(&mut self) {
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
    pub(super) fn redo(&mut self) {
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
    pub(super) fn save(&mut self, ctx: &egui::Context) {
        if let (Some(case), Some(doc)) = (&self.case, &self.document) {
            let manifest = case.manifest.clone();
            let doc = doc.clone();
            self.spawn(ctx, "Saving reviewed evidence…", move || {
                workspace::save(&manifest, &doc).map(|p| ResultValue::Saved(p, doc))
            });
        }
    }
    pub(super) fn load(&mut self, ctx: &egui::Context) {
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
    pub(super) fn load_window(&mut self, ctx: &egui::Context) {
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
    pub(super) fn focus_selected(&mut self) {
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
    pub(super) fn add_event(&mut self, kind: EventKind) {
        if let Some(doc) = &mut self.document {
            let start = self.cursor_s.min((doc.duration_s - 0.1).max(0.0));
            let end = (start + 5.0).min(doc.duration_s);
            let id = crate::gui::unique_name(if kind == EventKind::Doppler {
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
    pub(super) fn propose_thunder(&mut self) {
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
                        crate::gui::unique_name("thunder"),
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
    pub(super) fn remove_selected(&mut self) {
        if let Some(index) = self.selected.take()
            && let Some(document) = &mut self.document
            && index < document.events.len()
        {
            document.events.remove(index);
            self.score = None;
        }
    }
    pub(super) fn keyboard_shortcuts(&mut self, ctx: &egui::Context) {
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
    pub(super) fn finish_edit_frame(&mut self, before: ReviewWorkspace, pointer_down: bool) {
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
}
