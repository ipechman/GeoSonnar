//! Bounded presentation of immutable, unaccepted candidate predictions.
use super::spectrogram::{PlotView, clip_line};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke};
use geosonnar::{
    prediction_overlay::PredictionOverlay,
    workspace::{EventKind, EvidenceEvent},
};

pub(super) const MAX_DRAW_ITEMS: usize = 12_000;
pub(super) const PREDICTED_FREQUENCY: Color32 = Color32::from_rgb(178, 255, 94);
pub(super) const PREDICTED_TIMING: Color32 = Color32::from_rgb(204, 167, 255);

#[derive(Default, Debug)]
pub(super) struct DrawStats {
    pub examined: usize,
    pub frequency_segments: usize,
    pub frequency_markers: usize,
    pub timing_markers: usize,
    pub unavailable_spans: usize,
    pub budget_exhausted: bool,
}

impl DrawStats {
    fn take(&mut self) -> bool {
        if self.examined >= MAX_DRAW_ITEMS {
            self.budget_exhausted = true;
            false
        } else {
            self.examined += 1;
            true
        }
    }
}

pub(super) fn paint(
    painter: &egui::Painter,
    rect: Rect,
    view: &PlotView,
    overlay: &PredictionOverlay,
    event_filter: Option<&str>,
    events: &[EvidenceEvent],
) -> DrawStats {
    let mut stats = DrawStats::default();
    if !view.valid() || rect.width() <= 1.0 || rect.height() <= 1.0 {
        return stats;
    }
    let painter = painter.with_clip_rect(rect);
    let chosen = |id: &str| event_filter.is_none_or(|selected| id == selected);
    for marker in &overlay.timing {
        if !chosen(&marker.event_id)
            || !marker.predicted_arrival_s.is_finite()
            || marker.predicted_arrival_s < view.time_min_s
            || marker.predicted_arrival_s > view.time_max_s
        {
            continue;
        }
        if !stats.take() {
            break;
        }
        let x = view
            .point(rect, marker.predicted_arrival_s, view.frequency_min_hz)
            .x;
        let line = [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())];
        painter.line_segment(line, Stroke::new(4.0, Color32::BLACK));
        painter.extend(egui::Shape::dashed_line(
            &line,
            Stroke::new(2.0, PREDICTED_TIMING),
            5.0,
            4.0,
        ));
        painter.text(
            Pos2::new(x + 4.0, rect.bottom() - 16.0),
            Align2::LEFT_BOTTOM,
            if marker.kind == EventKind::Thunder {
                "Expected thunder"
            } else {
                "Expected CPA"
            },
            FontId::proportional(11.0),
            PREDICTED_TIMING,
        );
        stats.timing_markers += 1;
    }
    for missing in &overlay.unavailable {
        if !chosen(&missing.event_id) {
            continue;
        }
        let (Some(start), Some(end)) = (missing.start_s, missing.end_s) else {
            continue;
        };
        if !start.is_finite()
            || !end.is_finite()
            || end < view.time_min_s
            || start > view.time_max_s
        {
            continue;
        }
        if !stats.take() {
            break;
        }
        let a = view
            .point(rect, start, view.frequency_min_hz)
            .x
            .clamp(rect.left(), rect.right());
        let b = view
            .point(rect, end, view.frequency_min_hz)
            .x
            .clamp(rect.left(), rect.right());
        let band = Rect::from_min_max(
            Pos2::new(a.min(b), rect.bottom() - 9.0),
            Pos2::new((a.max(b)).max(a.min(b) + 2.0), rect.bottom()),
        );
        painter.rect_filled(band, 0.0, Color32::from_gray(145));
        if let Some(index) = missing.measured_point_index
            && let Some(point) = events
                .iter()
                .find(|event| event.id == missing.event_id)
                .and_then(|event| event.points.get(index))
        {
            let p = view.point(rect, point.time_s, point.frequency_hz);
            for direction in [-1.0, 1.0] {
                painter.line_segment(
                    [
                        p + egui::vec2(-4.0, -4.0 * direction),
                        p + egui::vec2(4.0, 4.0 * direction),
                    ],
                    Stroke::new(2.0, Color32::GRAY),
                );
            }
        }
        stats.unavailable_spans += 1;
    }
    'frequency: for event in &overlay.frequency {
        if !chosen(&event.event_id) {
            continue;
        }
        for segment in &event.segments {
            // Each native piece is independent. Never join across support gaps,
            // velocity knots, unavailable predictions, or an omitted piece.
            let first = segment
                .samples
                .partition_point(|sample| sample.time_s < view.time_min_s)
                .saturating_sub(1);
            let last = segment
                .samples
                .partition_point(|sample| sample.time_s <= view.time_max_s)
                .saturating_add(1)
                .min(segment.samples.len());
            let samples = &segment.samples[first.min(last)..last];
            for pair in samples.windows(2) {
                if !stats.take() {
                    break 'frequency;
                }
                if ![
                    pair[0].time_s,
                    pair[1].time_s,
                    pair[0].frequency_hz,
                    pair[1].frequency_hz,
                ]
                .iter()
                .all(|v| v.is_finite())
                    || pair[0].time_s >= pair[1].time_s
                {
                    continue;
                }
                let line = [
                    view.point(rect, pair[0].time_s, pair[0].frequency_hz),
                    view.point(rect, pair[1].time_s, pair[1].frequency_hz),
                ];
                if let Some(line) = clip_line(rect, line) {
                    painter.line_segment(line, Stroke::new(4.5, Color32::BLACK));
                    painter.line_segment(line, Stroke::new(2.0, PREDICTED_FREQUENCY));
                    stats.frequency_segments += 1;
                }
            }
            for sample in samples
                .iter()
                .filter(|sample| sample.measured_point_index.is_some())
            {
                if !stats.take() {
                    break 'frequency;
                }
                if !sample.time_s.is_finite() || !sample.frequency_hz.is_finite() {
                    continue;
                }
                let point = view.point(rect, sample.time_s, sample.frequency_hz);
                if !rect.contains(point) {
                    continue;
                }
                if let Some(observed) = sample
                    .observed_frequency_hz
                    .filter(|value| value.is_finite())
                {
                    let measured = view.point(rect, sample.time_s, observed);
                    if let Some(line) = clip_line(rect, [measured, point]) {
                        painter.line_segment(
                            line,
                            Stroke::new(1.0, PREDICTED_FREQUENCY.gamma_multiply(0.65)),
                        );
                    }
                }
                let delta = 4.0;
                let diamond = vec![
                    point + egui::vec2(0.0, -delta),
                    point + egui::vec2(delta, 0.0),
                    point + egui::vec2(0.0, delta),
                    point + egui::vec2(-delta, 0.0),
                ];
                painter.add(egui::Shape::convex_polygon(
                    diamond,
                    Color32::BLACK,
                    Stroke::new(1.6, PREDICTED_FREQUENCY),
                ));
                stats.frequency_markers += 1;
            }
        }
    }
    let source = overlay
        .frequency
        .iter()
        .filter(|event| chosen(&event.event_id))
        .map(|event| event.source_id.as_str())
        .chain(
            overlay
                .timing
                .iter()
                .filter(|event| chosen(&event.event_id))
                .map(|event| event.source_id.as_str()),
        )
        .chain(
            overlay
                .unavailable
                .iter()
                .filter(|event| chosen(&event.event_id))
                .map(|event| event.source_id.as_str()),
        )
        .take(3)
        .collect::<Vec<_>>()
        .join(" / ");
    let uncertain_source = events.iter().any(|event| {
        chosen(&event.id)
            && event.acoustic_source.rationale().is_some()
            && (overlay.frequency.iter().any(|row| row.event_id == event.id)
                || overlay.timing.iter().any(|row| row.event_id == event.id)
                || overlay
                    .unavailable
                    .iter()
                    .any(|row| row.event_id == event.id))
    });
    let caption = format!(
        "{}HYPOTHESIS · not confidence · {} · clock {:+.3}s · {}{}",
        if uncertain_source {
            "UNCERTAIN SOURCE · "
        } else {
            ""
        },
        source,
        overlay.clock_adjustment_s,
        overlay.model_label,
        if stats.budget_exhausted {
            " · display limit reached"
        } else if stats.unavailable_spans > 0 {
            " · gray: unavailable"
        } else {
            ""
        }
    );
    let galley = painter.layout(
        caption,
        FontId::proportional(11.0),
        PREDICTED_FREQUENCY,
        (rect.width() - 12.0).max(1.0),
    );
    let caption_rect = Rect::from_min_size(rect.left_top() + egui::vec2(4.0, 3.0), galley.size());
    painter.rect_filled(
        caption_rect.expand(2.0),
        2.0,
        Color32::from_black_alpha(200),
    );
    painter.galley(caption_rect.min, galley, PREDICTED_FREQUENCY);
    stats
}
