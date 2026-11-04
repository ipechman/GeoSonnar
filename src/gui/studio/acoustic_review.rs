//! Separate measured-event review from an analyst's acoustic-source hypothesis.
use super::*;
use workspace::AcousticSourceAssessment;

pub(super) fn measurements_changed(before: &EvidenceEvent, after: &EvidenceEvent) -> bool {
    before.kind != after.kind
        || before.start_s != after.start_s
        || before.end_s != after.end_s
        || before.arrival_s != after.arrival_s
        || before.points != after.points
        || before.uncertainty_s != after.uncertainty_s
        || before.frequency_sigma_hz != after.frequency_sigma_hz
}

pub(super) fn reset_measurement_review(event: &mut EvidenceEvent) {
    event.review = ReviewState::Pending;
    event.selected_for_scoring = false;
    event.acoustic_source = AcousticSourceAssessment::Unassessed;
}

pub(super) fn reset_edited_measurements(before: &ReviewWorkspace, after: &mut ReviewWorkspace) {
    let previous: std::collections::BTreeMap<_, _> = before
        .events
        .iter()
        .map(|event| (&event.id, event))
        .collect();
    for event in &mut after.events {
        if let Some(previous) = previous.get(&event.id)
            && (before.channel != after.channel || measurements_changed(previous, event))
        {
            reset_measurement_review(event);
        }
    }
}

pub(super) fn assessment_label(assessment: &AcousticSourceAssessment) -> &'static str {
    match assessment {
        AcousticSourceAssessment::Unassessed => "Not assessed",
        AcousticSourceAssessment::Compatible => "Plausible source",
        AcousticSourceAssessment::UncertainHypothesis { .. } => "Uncertain hypothesis",
        AcousticSourceAssessment::Incompatible => "Does not fit this source",
    }
}

pub(super) fn block_reason(event: &EvidenceEvent) -> Option<&'static str> {
    match &event.acoustic_source {
        AcousticSourceAssessment::Unassessed => Some("Assess the sound source in Audio review."),
        AcousticSourceAssessment::Incompatible => {
            Some("This sound was marked incompatible. Review it or remove it from scoring.")
        }
        AcousticSourceAssessment::UncertainHypothesis { rationale }
            if rationale.trim().is_empty() =>
        {
            Some("Add a reason for testing this uncertain source.")
        }
        AcousticSourceAssessment::UncertainHypothesis { rationale } if rationale.len() > 4000 => {
            Some("Shorten the source-assessment reason before continuing.")
        }
        _ => None,
    }
}

pub(super) fn assessment_ui(ui: &mut egui::Ui, event: &mut EvidenceEvent) {
    ui.strong("2 Source assessment");
    ui.label(if event.kind == EventKind::Doppler {
        "Could the observed sound be an aircraft?"
    } else {
        "Could the observed sound be thunder?"
    });
    let previous = match event.acoustic_source {
        AcousticSourceAssessment::Unassessed => 0,
        AcousticSourceAssessment::Compatible => 1,
        AcousticSourceAssessment::UncertainHypothesis { .. } => 2,
        AcousticSourceAssessment::Incompatible => 3,
    };
    let mut choice = previous;
    egui::ComboBox::from_id_salt("acoustic_source_assessment")
        .selected_text(assessment_label(&event.acoustic_source))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut choice, 0, "Not assessed");
            ui.selectable_value(&mut choice, 1, "Plausible source");
            ui.selectable_value(&mut choice, 2, "Uncertain hypothesis");
            ui.selectable_value(&mut choice, 3, "Does not fit this source");
        });
    if choice != previous {
        event.acoustic_source = match choice {
            1 => AcousticSourceAssessment::Compatible,
            2 => AcousticSourceAssessment::UncertainHypothesis {
                rationale: String::new(),
            },
            3 => AcousticSourceAssessment::Incompatible,
            _ => AcousticSourceAssessment::Unassessed,
        };
    }
    if let AcousticSourceAssessment::UncertainHypothesis { rationale } = &mut event.acoustic_source
    {
        ui.label("Reason for testing this uncertain source");
        accessible_text(
            ui,
            "acoustic_source_reason",
            rationale,
            "What makes it plausible, and what else could explain it?",
            true,
            ui.available_width(),
        );
        muted(
            ui,
            "An uncertain hypothesis can be tested with a reason. It stays explicitly uncertain in the results.",
        );
    }
    if let Some(reason) = block_reason(event) {
        ui.colored_label(tone(ui, AMBER), reason);
    }
    muted(
        ui,
        "This records your interpretation of the sound. It does not identify an aircraft, lightning strike or location.",
    );
}

pub(super) struct Readiness {
    pub blocked: bool,
    pub review_event: Option<usize>,
}

fn required_events(doc: &ReviewWorkspace, regional: bool) -> impl Iterator<Item = &EvidenceEvent> {
    doc.active_scoring_events().filter(move |event| {
        !regional
            || doc.regional_comparison.as_ref().is_none_or(|options| {
                options.modes.iter().any(|mode| match mode {
                    geosonnar::regional_search::SearchMode::Combined => true,
                    geosonnar::regional_search::SearchMode::DopplerOnly => {
                        event.kind == EventKind::Doppler
                    }
                    geosonnar::regional_search::SearchMode::ThunderOnly => {
                        event.kind == EventKind::Thunder
                    }
                })
            })
    })
}

pub(super) fn first_block(doc: &ReviewWorkspace, regional: bool) -> Option<String> {
    required_events(doc, regional)
        .find_map(|event| block_reason(event).map(|reason| format!("{}: {reason}", event.label)))
}

pub(super) fn assumptions_ui<'a>(
    ui: &mut egui::Ui,
    id: &str,
    events: impl Iterator<Item = &'a EvidenceEvent>,
) {
    let mut uncertain = events
        .filter_map(|event| {
            event
                .acoustic_source
                .rationale()
                .map(|reason| (event, reason))
        })
        .peekable();
    if uncertain.peek().is_none() {
        return;
    }
    ui.colored_label(
        tone(ui, AMBER),
        "Uncertain acoustic source — these results remain hypotheses",
    );
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(90.0)
        .show(ui, |ui| {
            for (event, reason) in uncertain {
                ui.label(format!("{}: {reason}", event.label));
            }
        });
}

pub(super) fn readiness_ui(ui: &mut egui::Ui, doc: &ReviewWorkspace, regional: bool) -> Readiness {
    let mut active = 0;
    let mut blocked = Vec::new();
    for event in required_events(doc, regional) {
        active += 1;
        if let Some(reason) = block_reason(event) {
            blocked.push((event, reason));
        }
    }
    let mut review_event = None;
    if !blocked.is_empty() {
        ui.colored_label(
            tone(ui, AMBER),
            format!(
                "Source review needed for {} selected event(s)",
                blocked.len()
            ),
        );
        for (event, reason) in blocked.iter().take(3) {
            ui.label(format!("{}: {reason}", event.label));
        }
        if blocked.len() > 3 {
            muted(
                ui,
                format!(
                    "{} more selected events need source review.",
                    blocked.len() - 3
                ),
            );
        }
        if ui.button("Review source in Audio").clicked() {
            review_event = doc
                .events
                .iter()
                .position(|event| event.id == blocked[0].0.id);
        }
        muted(
            ui,
            "Blocked events remain selected. Nothing is silently left out of scoring.",
        );
    } else if active > 0 {
        muted(
            ui,
            format!(
                "Source assessments recorded for {active} active event(s). This does not verify their sources."
            ),
        );
    }
    let warnings: Vec<_> = required_events(doc, regional)
        .filter_map(|event| {
            event
                .acoustic_source
                .rationale()
                .map(|reason| format!("{}: {reason}", event.label))
        })
        .collect();
    if !warnings.is_empty() {
        ui.colored_label(
            tone(ui, AMBER),
            format!(
                "{} uncertain source hypothesis(es) included",
                warnings.len()
            ),
        );
        egui::CollapsingHeader::new("Review uncertain-source reasons").show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("uncertain_source_reasons")
                .max_height(140.0)
                .show(ui, |ui| {
                    for warning in warnings {
                        ui.label(warning);
                    }
                });
        });
    }
    Readiness {
        blocked: !blocked.is_empty(),
        review_event,
    }
}

impl Studio {
    pub(super) fn open_source_review(&mut self, index: usize) {
        self.stop();
        self.selected = Some(index);
        self.tool = EditTool::Inspect;
        self.compact_inspector = true;
        self.focus_selected();
        self.pending_action = Some(Action::ReviewEvidence);
    }
}
