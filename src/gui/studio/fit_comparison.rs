//! Read-only comparisons against simple observed-tone alternatives.
use eframe::egui;
use geosonnar::{
    fit_diagnostics::{FitDiagnostics, ToneComparison},
    prediction_overlay::FrequencyEventOverlay,
};

pub(super) fn show(
    ui: &mut egui::Ui,
    frequency: &[FrequencyEventOverlay],
    selected_event: Option<&str>,
    all_events: bool,
) {
    if frequency.is_empty() {
        return;
    }
    egui::CollapsingHeader::new("Fit comparison")
        .id_salt("inspection_fit_comparison")
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("fit_comparison_body")
                .max_height(260.0)
                .show(ui, |ui| {
                    let mut shown = false;
                    for event in frequency.iter().filter(|event| {
                        all_events || selected_event == Some(event.event_id.as_str())
                    }) {
                        if shown {
                            ui.separator();
                        }
                        shown = true;
                        ui.push_id(&event.event_id, |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} · {}",
                                    event.event_id, event.event_label
                                ))
                                .strong(),
                            );
                            if let Some(diagnostics) = &event.fit_diagnostics {
                                event_comparison(ui, diagnostics);
                            } else {
                                ui.label("Comparison unavailable in this saved inspection. Inspect the hypothesis again to calculate it.");
                            }
                        });
                    }
                    if !shown {
                        ui.label("Select an inspected Doppler event, or enable All inspected events.");
                    }
                });
        });
}

fn event_comparison(ui: &mut egui::Ui, diagnostics: &FitDiagnostics) {
    let span = diagnostics
        .span_s
        .map(|span| format!(" · measured span {span:.3} s"))
        .unwrap_or_default();
    ui.label(format!(
        "{} of {} measured points supported{}",
        diagnostics.supported_points, diagnostics.observed_points, span
    ));
    if let Some(comparison) = &diagnostics.comparison {
        comparison_table(ui, comparison);
        ui.small("All three use the same measured timestamps and residual scales. Scaled RMSE is the root mean square of residuals after division by each point's scale. Display samples do not enter this comparison.");
        egui::CollapsingHeader::new("Fitted alternatives and interpretation").show(ui, |ui| {
            ui.label(format!(
                "Flat observed frequency: {:.5} Hz",
                comparison.flat_frequency_hz
            ));
            ui.label(format!(
                "Drift: {:.5} Hz at recording time {:.3} s, {:+.5} Hz/s",
                comparison.linear_frequency_at_reference_hz,
                comparison.linear_reference_time_s,
                comparison.linear_slope_hz_per_s
            ));
            for limitation in &diagnostics.limitations {
                ui.small(limitation);
            }
        });
        ui.small("These are in-sample descriptive errors. The drift has an extra fitted parameter; the Doppler hypothesis may already reflect a wider search. Lower error does not identify a source or verify a location.");
    } else {
        ui.label(
            diagnostics
                .unavailable_reason
                .as_deref()
                .unwrap_or("Comparison unavailable for this inspection."),
        );
    }
}

fn comparison_table(ui: &mut egui::Ui, comparison: &ToneComparison) {
    egui::ScrollArea::horizontal()
        .id_salt("fit_comparison_table_scroll")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("fit_comparison_table")
                .striped(true)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    ui.strong("Model");
                    ui.strong("RMSE (Hz)");
                    ui.strong("Scaled RMSE");
                    ui.end_row();
                    for (label, errors) in [
                        ("Inspected Doppler", &comparison.doppler),
                        ("Fitted flat tone", &comparison.flat),
                        ("Fitted linear drift", &comparison.linear),
                    ] {
                        ui.label(label);
                        ui.monospace(format!("{:.5}", errors.rmse_hz));
                        ui.monospace(format!("{:.5}", errors.normalized_rmse));
                        ui.end_row();
                    }
                });
        });
}
