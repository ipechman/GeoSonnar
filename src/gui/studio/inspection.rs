//! Temporary candidate inspection. It never edits measurements or associations.
use super::*;
use geosonnar::prediction_overlay::{self, OverlaySettings, PredictionOverlay};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct InputIdentity {
    revision: u64,
    layers: Vec<(PathBuf, String, Option<u64>)>,
    lightning: Option<(PathBuf, String)>,
}

impl InputIdentity {
    pub fn capture(studio: &Studio) -> Self {
        // Loaded buffers are immutable; their source hashes and explicit time
        // interpretation identify replacements without walking track points.
        Self {
            revision: studio.loaded_data_revision,
            layers: studio
                .layers
                .iter()
                .map(|layer| {
                    (
                        layer.source_path.clone(),
                        layer.source_sha256.clone(),
                        layer.time_origin_utc_s.map(f64::to_bits),
                    )
                })
                .collect(),
            lightning: studio
                .lightning
                .as_ref()
                .map(|catalog| (catalog.input_path.clone(), catalog.input_sha256.clone())),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum InspectionOrigin {
    Regional {
        key: String,
        run_id: u64,
    },
    Fixed {
        candidate_id: String,
        observer_bits: [u64; 3],
    },
}

impl InspectionOrigin {
    pub fn fixed(candidate_id: String, observer: &evidence_score::ObserverLocation) -> Self {
        Self::Fixed {
            candidate_id,
            observer_bits: [
                observer.latitude_deg.to_bits(),
                observer.longitude_deg.to_bits(),
                observer.ellipsoid_height_m.to_bits(),
            ],
        }
    }
    fn matches(&self, studio: &Studio) -> bool {
        match self {
            Self::Regional { key, run_id } => {
                studio.regional.selection_key().as_ref() == Some(key)
                    && studio.regional.run_id == *run_id
            }
            Self::Fixed {
                candidate_id,
                observer_bits,
            } => studio
                .fixed_selected_receiver()
                .is_some_and(|(id, observer)| {
                    id == *candidate_id
                        && [
                            observer.latitude_deg.to_bits(),
                            observer.longitude_deg.to_bits(),
                            observer.ellipsoid_height_m.to_bits(),
                        ] == *observer_bits
                }),
        }
    }
    fn label(&self) -> &str {
        match self {
            Self::Regional { key, .. } => key,
            Self::Fixed { candidate_id, .. } => candidate_id,
        }
    }
}

pub(super) struct PreparedInspection {
    pub original: ReviewWorkspace,
    pub inputs: InputIdentity,
    pub origin: InspectionOrigin,
    pub overlay: PredictionOverlay,
}

pub(super) struct ActiveInspection {
    original: ReviewWorkspace,
    inputs: InputIdentity,
    origin: InspectionOrigin,
    pub overlay: PredictionOverlay,
    event_ids: Vec<String>,
    caption: String,
}

pub(super) struct InspectionState {
    pub active: Option<ActiveInspection>,
    pub visible: bool,
    pub all_events: bool,
    pub invalidated: Option<String>,
}

impl Default for InspectionState {
    fn default() -> Self {
        Self {
            active: None,
            visible: true,
            all_events: false,
            invalidated: None,
        }
    }
}

impl InspectionState {
    pub fn clear(&mut self, reason: &str) {
        if self.active.take().is_some() {
            self.invalidated = Some(reason.into());
        }
    }
}

impl Studio {
    pub(super) fn invalidate_changed_inspection(&mut self) {
        let reason = self.inspection.active.as_ref().and_then(|active| {
            if self.document.as_ref() != Some(&active.original) {
                Some("Evidence or assumptions changed; inspect the candidate again.")
            } else if InputIdentity::capture(self) != active.inputs {
                Some("Loaded data changed; inspect the candidate again.")
            } else if !active.origin.matches(self) {
                Some("Candidate or evidence mode changed; inspect the new candidate.")
            } else {
                None
            }
        });
        if let Some(reason) = reason {
            self.inspection.clear(reason);
            self.pending_action = None;
        }
    }

    pub(super) fn accept_inspection(&mut self, prepared: PreparedInspection, ctx: &egui::Context) {
        if self.document.as_ref() != Some(&prepared.original)
            || InputIdentity::capture(self) != prepared.inputs
            || !prepared.origin.matches(self)
        {
            self.notice=Some((true,"Inspection finished after the evidence, data or candidate changed. Inspect the current candidate again.".into()));
            return;
        }
        if let InspectionOrigin::Regional { key, .. } = &prepared.origin {
            self.regional.detail = Some((key.clone(), prepared.overlay.score_report.clone()));
        }
        let mut ids: std::collections::BTreeSet<String> = prepared
            .overlay
            .frequency
            .iter()
            .map(|row| row.event_id.clone())
            .chain(
                prepared
                    .overlay
                    .timing
                    .iter()
                    .map(|row| row.event_id.clone()),
            )
            .chain(
                prepared
                    .overlay
                    .unavailable
                    .iter()
                    .map(|row| row.event_id.clone()),
            )
            .collect();
        let event_ids = prepared
            .original
            .events
            .iter()
            .filter(|event| ids.remove(&event.id))
            .map(|event| event.id.clone())
            .collect::<Vec<_>>();
        let caption = format!(
            "{} · {:.6}, {:.6} · clock {} · {}",
            prepared.origin.label(),
            prepared.overlay.observer.latitude_deg,
            prepared.overlay.observer.longitude_deg,
            format_args!("{:+.3}s", prepared.overlay.clock_adjustment_s),
            prepared.overlay.model_label
        );
        let selected_id = self
            .selected
            .and_then(|index| prepared.original.events.get(index))
            .map(|event| &event.id);
        let focus = selected_id
            .filter(|id| event_ids.contains(id))
            .cloned()
            .or_else(|| event_ids.first().cloned());
        self.inspection = InspectionState {
            active: Some(ActiveInspection {
                original: prepared.original,
                inputs: prepared.inputs,
                origin: prepared.origin,
                overlay: prepared.overlay,
                event_ids,
                caption,
            }),
            visible: true,
            all_events: false,
            invalidated: None,
        };
        if let Some(id) = focus {
            self.focus_inspected_event(&id, ctx);
        }
        self.pending_action = Some(Action::ReviewInspection);
        self.notice=Some((false,"Showing the inspected hypothesis over measured audio. Predictions are hypothetical; observations, source approvals and scoring selections are unchanged.".into()));
    }

    fn focus_inspected_event(&mut self, id: &str, ctx: &egui::Context) {
        let Some(index) = self
            .document
            .as_ref()
            .and_then(|doc| doc.events.iter().position(|event| event.id == id))
        else {
            return;
        };
        self.stop();
        self.selected = Some(index);
        self.tool = EditTool::Inspect;
        self.focus_selected();
        let covered = self.window.as_ref().is_some_and(|window| {
            self.window_start >= window.start_s - 1e-6 && self.window_end <= window.end_s + 1e-6
        });
        if !covered && !self.tools.ffmpeg.as_os_str().is_empty() && !self.busy() {
            self.load_window(ctx);
        }
    }

    pub(super) fn inspection_controls_ui(&mut self, ui: &mut egui::Ui) {
        self.invalidate_changed_inspection();
        let Some(active) = &self.inspection.active else {
            if let Some(reason) = &self.inspection.invalidated {
                muted(ui, reason);
            }
            return;
        };
        let mut clear = false;
        let mut focus = None;
        let selected = self
            .selected
            .and_then(|index| self.document.as_ref()?.events.get(index));
        let current_id = selected.map(|event| event.id.as_str());
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.inspection.visible, "Show inspected hypothesis");
            ui.checkbox(&mut self.inspection.all_events, "All inspected events");
            egui::ComboBox::from_id_salt("inspection_event")
                .selected_text(
                    current_id
                        .filter(|id| active.event_ids.iter().any(|v| v == id))
                        .unwrap_or("Choose inspected event"),
                )
                .show_ui(ui, |ui| {
                    for id in &active.event_ids {
                        if ui.selectable_label(current_id == Some(id), id).clicked() {
                            focus = Some(id.clone());
                        }
                    }
                });
            clear = ui.small_button("Clear inspection").clicked();
        });
        ui.label(
            egui::RichText::new("Hypothetical prediction — not confidence or a verified source")
                .strong(),
        );
        muted(ui, &active.caption);
        if self.investigation.archive.is_some()
            && matches!(active.origin, InspectionOrigin::Regional { .. })
        {
            ui.colored_label(
                tone(ui, AMBER),
                "Archived inputs; original source files not rechecked",
            );
        }
        super::acoustic_review::assumptions_ui(
            ui,
            "inspection_source_assumptions",
            active
                .original
                .events
                .iter()
                .filter(|event| active.event_ids.contains(&event.id)),
        );
        muted(
            ui,
            "Lime: predicted Doppler curve and sample markers · violet: expected arrival · observed traces keep their original colors. Gray marks unsupported predictions.",
        );
        super::fit_comparison::show(
            ui,
            &active.overlay.frequency,
            current_id,
            self.inspection.all_events,
        );
        egui::CollapsingHeader::new("Inspected sources, missing predictions and sampling").show(
            ui,
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspection_assumptions")
                    .max_height(140.0)
                    .show(ui, |ui| {
                        for event in &active.overlay.frequency {
                            ui.label(format!(
                                "{} → {} · emitted {:.3} Hz · display step {:.3}s",
                                event.event_id, event.source_id, event.emitted_frequency_hz,
                                event.sampling_step_s
                            ));
                        }
                        for event in &active.overlay.timing {
                            ui.label(format!(
                                "{} → {} · expected {:.3}s · observed {:.3}s · observed uncertainty {:.3}s · residual scale {:.3}s · normalized residual {:+.3}",
                                event.event_id,
                                event.source_id,
                                event.predicted_arrival_s,
                                event.observed_arrival_s,
                                event.observed_uncertainty_s,
                                event.effective_residual_scale_s,
                                event.normalized_residual
                            ));
                        }
                        for missing in &active.overlay.unavailable {
                            ui.label(format!("{}: {}", missing.event_id, missing.reason));
                        }
                        ui.small(format!("{} prediction samples; display {}. Exact measured timestamps remain included. Curves are not confidence bands.", active.overlay.frequency_sample_count, if active.overlay.sampling_coarsened { "coarsened to the work limit" } else { "at requested spacing" }));
                        for limit in &active.overlay.limitations {
                            ui.small(limit);
                        }
                    });
            },
        );
        if clear {
            self.inspection = InspectionState::default();
        } else if let Some(id) = focus {
            self.focus_inspected_event(&id, ui.ctx());
        }
    }

    pub(super) fn inspect_fixed_candidate(&mut self, ctx: &egui::Context) {
        if self.busy() || self.score_document.as_ref() != self.document.as_ref() {
            return;
        }
        let (Some(doc), Some((id, observer))) =
            (self.document.clone(), self.fixed_selected_receiver())
        else {
            return;
        };
        let map = match self
            .cached_bindings_match(&doc)
            .and_then(|_| self.merged_map())
        {
            Ok(value) => value,
            Err(error) => {
                self.notice = Some((true, error));
                return;
            }
        };
        let lightning = self.lightning.clone();
        let inputs = InputIdentity::capture(self);
        let manifest = self.case.as_ref().unwrap().manifest.clone();
        self.inspection
            .clear("A new candidate inspection is being prepared.");
        self.spawn(
            ctx,
            "Inspecting fixed receiver against the measured recording…",
            move || {
                workspace::verify(&manifest, &doc)?;
                let overlay = prediction_overlay::build(
                    &doc,
                    &map,
                    &lightning,
                    doc.strike_fallback_height_m,
                    &observer,
                    &OverlaySettings::default(),
                )?;
                workspace::verify(&manifest, &doc)?;
                Ok(ResultValue::PredictionInspected(Box::new(
                    PreparedInspection {
                        original: doc,
                        inputs,
                        origin: InspectionOrigin::fixed(id, &observer),
                        overlay,
                    },
                )))
            },
        );
    }
}
