//! Views responsibilities for the review workspace.
use super::*;

impl Studio {
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
    pub(super) fn review_file_actions(&mut self, ui: &mut egui::Ui) {
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
    pub(super) fn editor_canvas(&mut self, ui: &mut egui::Ui, height: f32) {
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
                crate::gui::prediction_plot::paint(
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
    pub(super) fn detection_controls(&mut self, ui: &mut egui::Ui) -> Option<Action> {
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
    pub(super) fn event_list(&mut self, ui: &mut egui::Ui) {
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
    pub(super) fn inspector(&mut self, ui: &mut egui::Ui) {
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
    pub(super) fn video_panel(&mut self, ui: &mut egui::Ui, height: f32) {
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
                crate::gui::prediction_plot::paint(
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
