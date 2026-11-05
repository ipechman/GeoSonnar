//! Wide-area search presentation.
use super::*;

pub(super) fn physical_range_ui(
    ui: &mut egui::Ui,
    label: &str,
    range: &mut [f64; 2],
    unit: &str,
    speed: f64,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        ui.label("Min");
        ui.add(
            egui::DragValue::new(&mut range[0])
                .suffix(unit)
                .speed(speed),
        );
        ui.label("Max");
        ui.add(
            egui::DragValue::new(&mut range[1])
                .suffix(unit)
                .speed(speed),
        );
    });
}
pub(super) fn wide_progress_stage_label(stage: &str) -> &'static str {
    match stage {
        "verifying_source" => "Verifying flight data",
        "preparing_index" => "Preparing the location search",
        "local_cpa_census" => "Finding candidate flight passes",
        "counting_timing_pairs" => "Counting timing-compatible pairs",
        "shared_clock_pair_scoring" => "Comparing Doppler pairs",
        "verifying_source_final" => "Checking source files again",
        "estimated" => "Estimate ready",
        "finished" => "Run finished",
        _ => "Working",
    }
}
pub(super) fn score_bound_progress_ui(ui: &mut egui::Ui, count: u64) {
    if count > 0 {
        ui.label(format!("Pairs excluded by score bound: {count}"))
            .on_hover_text("These pairs cannot improve the retained rankings. They are counted separately from valid scores; their acoustic support was not fully evaluated.");
    }
}
pub(super) fn stop_guidance(report: &WidePairReport) -> Option<&'static str> {
    use wide_area_pair::WideStatus;
    match report.status {
        WideStatus::Completed => None,
        WideStatus::BudgetExceeded => Some(match report.error.as_deref() {
            Some("Global wide-area work budget exhausted") => {
                "The work-unit limit stopped this run. Review each event's evidence-supported tone and physical ranges and the work-unit budget (a raw integer), then re-estimate. Raising only the time limit does not remove this limit; a larger work budget is not a completion guarantee."
            }
            Some("Global wide-area wall budget exhausted") => {
                "The runtime limit stopped this run. Review the runtime budget and re-estimate before starting a new run. Extra time does not change the work or memory limits."
            }
            Some(
                "Global wide-area output budget exhausted"
                | "Wide-area output budget exhausted"
                | "Wide-area checkpoint budget exhausted"
                | "Wide-area final report exceeds its reserved output budget",
            ) => {
                "The output limit stopped this run. Review the output-byte budget and available disk space, then re-estimate before starting a new run."
            }
            Some(error)
                if geosonnar::track_store::is_memory_limit_error(error)
                    || error
                        .starts_with("Combined source/spatial-index/local-CPA memory needs ") =>
            {
                "The source/index memory limit stopped this run. Review Advanced memory limit and available system memory, then re-estimate. This allowance is separate from the track cache and total application memory."
            }
            _ => {
                "Review the reported limit and the applicable search setting, then re-estimate before starting a new run. The saved result remains incomplete; increasing an unrelated budget may not resolve the stop."
            }
        }),
        WideStatus::Cancelled => Some(
            "The saved run remains incomplete. Review the event ranges and budgets and re-estimate before starting a new run; reopening does not resume it.",
        ),
        WideStatus::Failed => Some(
            "Review the saved error and source state before re-estimating a new run. An incomplete search cannot establish that no matching source exists.",
        ),
    }
}
pub(super) fn cost_preview_ui(ui: &mut egui::Ui, estimate: &WideEstimate) {
    if let Some(ceiling) = estimate.maximum_timing_pairs_before_scan {
        ui.colored_label(tone(ui, AMBER),format!("Budget upper bound before scanning: at most {ceiling} timing-pair attempts globally (at least {} work units each). Source scanning and index work reduce this ceiling; it is not a completion estimate.", estimate.minimum_work_units_per_timing_pair));
    }
    let Some(cost) = &estimate.cost_preview else {
        muted(
            ui,
            "Detailed work breakdown is unavailable for this older estimate; re-estimate to obtain it.",
        );
        return;
    };
    ui.label(format!("Event 1: {} trace samples × {} tones · Event 2: {} trace samples × {} tones · {} shared shifts",
        cost.event_trace_samples[0], cost.event_tone_counts[0], cost.event_trace_samples[1], cost.event_tone_counts[1], cost.shared_shift_count));
    egui::CollapsingHeader::new("Detailed work breakdown")
        .id_salt("wide_cost_details")
        .show(ui, |ui| cost_details_ui(ui, cost));
    if cost.can_afford_minimum_first_receiver == Some(false) {
        ui.colored_label(tone(ui, AMBER), "The work budget cannot cover even the minimum setup for the first receiver. Review the work budget and re-estimate.");
    } else if cost.can_afford_minimum_first_pair == Some(false) {
        ui.colored_label(tone(ui, AMBER), "The work budget cannot cover even the minimum first timing-pair charge. Review the work budget and re-estimate.");
    }
}
pub(super) fn cost_details_ui(ui: &mut egui::Ui, cost: &wide_area_pair::WideCostPreview) {
    ui.label(format!("Minimum work per timing pair: {} trace-fit + {} track + {} retention units. Minimum spatial-index construction: {} units.",
        cost.trace_fit_work_units_per_pair, cost.minimum_track_work_units_per_pair, cost.retention_work_units_per_pair, cost.index_build_work_units));
    if let Some(ceiling) = cost.maximum_timing_pairs_after_minimum_grid_work {
        ui.label(format!("Full-grid work floor: {} units; at most {ceiling} timing pairs after that floor. Traversal, source census and joining cost more; this is not a completion or runtime estimate.", cost.minimum_grid_work_units));
    } else {
        ui.label(format!("Full-grid work floor: {} units. No work-unit limit is enabled; traversal, source census and joining still cost work. This is not a completion or runtime estimate.", cost.minimum_grid_work_units));
    }
}
pub(super) fn work_limit_ui(ui: &mut egui::Ui, settings: &mut Settings) -> egui::Response {
    let response = ui.checkbox(&mut settings.work_limit_enabled, "Also limit work units");
    ui.add_enabled(
        settings.work_limit_enabled,
        egui::DragValue::new(&mut settings.max_work)
            .suffix(" work-unit budget")
            .speed(100_000.),
    );
    muted(
        ui,
        "Work units are a raw integer allowance, separate from elapsed time. Turning this limit off keeps the stored value for later use.",
    );
    response
}
pub(super) fn runtime_and_work_ui(ui: &mut egui::Ui, settings: &mut Settings) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Stop after");
        ui.add(
            egui::DragValue::new(&mut settings.max_seconds)
                .range(1..=86400)
                .suffix(" s"),
        );
    });
    if !settings.work_limit_enabled {
        muted(
            ui,
            "No work-unit limit; time/memory/output limits still apply.",
        );
    }
    egui::CollapsingHeader::new("Advanced work limit")
        .id_salt("wide_optional_work_limit")
        .show(ui, |ui| {
            work_limit_ui(ui, settings);
        });
}
pub(super) fn range_ui(ui: &mut egui::Ui, label: &str, values: &mut [f64; 3], suffix: &str) {
    ui.label(label);
    ui.horizontal_wrapped(|ui| {
        for (name, value) in ["From", "To", "Step"].into_iter().zip(values) {
            ui.add(
                egui::DragValue::new(value)
                    .prefix(format!("{name} "))
                    .suffix(suffix)
                    .speed(0.5),
            );
        }
    });
}

pub(super) fn wide_result_line_height(ui: &egui::Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Body)
        .max(ui.text_style_height(&egui::TextStyle::Button) + 2. * ui.spacing().button_padding.y)
        .max(ui.spacing().interact_size.y)
}

pub(super) fn wide_result_row_height(ui: &egui::Ui) -> f32 {
    2. * wide_result_line_height(ui) + ui.spacing().item_spacing.y
}

/// `show_rows` requires every row to consume exactly the declared height.
/// Long source identifiers remain available in the tooltip, never extra lines
/// that change the virtual scroll offsets when the window is narrowed.
pub(super) fn wide_result_row(
    ui: &mut egui::Ui,
    id: egui::Id,
    summary: &str,
    can_refine: bool,
) -> (egui::Response, egui::Rect) {
    let line_height = wide_result_line_height(ui);
    let width = ui.available_width().max(1.);
    let (row, _) = ui.allocate_exact_size(
        egui::vec2(width, wide_result_row_height(ui)),
        egui::Sense::hover(),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().id(id).max_rect(row));
    child.set_clip_rect(ui.clip_rect().intersect(row));
    let summary_rect = egui::Rect::from_min_size(row.min, egui::vec2(width, line_height));
    child
        .place(summary_rect, egui::Label::new(summary).truncate())
        .on_hover_text(summary);
    let button_width = (ui
        .painter()
        .layout_no_wrap(
            "Prepare local area".into(),
            egui::TextStyle::Button.resolve(ui.style()),
            Color32::WHITE,
        )
        .size()
        .x
        + 2. * ui.spacing().button_padding.x)
        .min(width);
    let button_rect = egui::Rect::from_min_size(
        egui::pos2(row.left(), row.bottom() - line_height),
        egui::vec2(button_width, line_height),
    );
    if !can_refine {
        child.disable();
    }
    let response = child.place(
        button_rect,
        egui::Button::new("Prepare local area").truncate(),
    );
    (response, row)
}
