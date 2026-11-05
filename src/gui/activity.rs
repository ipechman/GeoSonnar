//! Background status without an animation-driven full-window repaint loop.
use eframe::egui::{self, Response, Sense, Stroke, Ui, WidgetInfo, WidgetType};
use std::time::Duration;

pub(super) const REFRESH: Duration = Duration::from_millis(250);

/// Progress notifications and input still wake the UI immediately. The periodic
/// refresh keeps elapsed labels current while an otherwise quiet worker runs.
pub(super) fn indicator(ui: &mut Ui) -> Response {
    let size = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    response.widget_info(|| WidgetInfo::new(WidgetType::ProgressIndicator));
    if ui.is_rect_visible(rect) {
        let center = rect.center();
        let radius = (size * 0.32).max(2.0);
        let color = ui.visuals().strong_text_color();
        ui.painter()
            .circle_stroke(center, radius, Stroke::new(1.5, color));
        ui.painter().circle_filled(center, 2.0, color);
        ui.ctx().request_repaint_after(REFRESH);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &egui::Context, time: f64, notify: bool) -> egui::FullOutput {
        ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                ..Default::default()
            },
            |ui| {
                indicator(ui);
                ui.label("Background search is running");
                if notify {
                    ui.ctx().request_repaint();
                }
            },
        )
    }

    #[test]
    fn background_activity_does_not_request_continuous_frames() {
        let ctx = egui::Context::default();
        for step in 0..16 {
            let output = frame(&ctx, step as f64 * 0.3, false);
            if step >= 10 {
                assert!(
                    // egui subtracts one predicted frame from delayed requests.
                    output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
                        >= Duration::from_millis(200),
                    "A quiet background worker must not animate at the display refresh rate"
                );
            }
        }
    }

    #[test]
    fn progress_notification_can_request_an_immediate_frame() {
        let ctx = egui::Context::default();
        for step in 0..12 {
            let _ = frame(&ctx, step as f64 * 0.3, false);
        }
        let output = frame(&ctx, 4.0, true);
        assert_eq!(
            output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
            Duration::ZERO
        );
    }
}
