//! Measured PSD raster with explicit, user-owned annotation editing.
use super::widgets::{BORDER, MUTED, RED};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use geosonnar::{
    review_media::SpectrogramWindow,
    workspace::{EventKind, EvidenceEvent, ReviewState, TracePoint},
};

const DOPPLER_COLORS: [Color32; 2] = [
    Color32::from_rgb(82, 242, 255),
    Color32::from_rgb(255, 117, 231),
];
const THUNDER_COLOR: Color32 = Color32::from_rgb(255, 182, 70);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditTool {
    Inspect,
    AddPoint,
    DrawTrace,
    MovePoint,
    ErasePoint,
    ThunderInterval,
}

#[derive(Clone, Debug)]
pub struct PlotView {
    pub time_min_s: f64,
    pub time_max_s: f64,
    pub frequency_min_hz: f64,
    pub frequency_max_hz: f64,
    pub db_min: f64,
    pub db_max: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct FrequencyBounds {
    pub min_hz: f64,
    pub max_hz: f64,
    pub bin_hz: f64,
}

/// Use only the measured PSD band, never an imagined extension above its last
/// bin or the decoded sample rate's Nyquist frequency.
pub fn frequency_bounds(window: &SpectrogramWindow) -> Option<FrequencyBounds> {
    if window.sample_rate_hz == 0 || window.fft_size == 0 {
        return None;
    }
    let first = *window.frequencies_hz.first()?;
    let last = *window.frequencies_hz.last()?;
    if !first.is_finite() || !last.is_finite() {
        return None;
    }
    let min_hz = first.max(0.0);
    let max_hz = last.min(window.sample_rate_hz as f64 / 2.0);
    let bin_hz = window.sample_rate_hz as f64 / window.fft_size as f64;
    (min_hz.is_finite() && max_hz.is_finite() && min_hz < max_hz).then_some(FrequencyBounds {
        min_hz,
        max_hz,
        bin_hz,
    })
}

impl PlotView {
    pub fn set_time_range(&mut self, start: f64, end: f64, duration: f64) {
        let duration = duration.max(0.001);
        let span = (end - start).clamp(0.05_f64.min(duration), 300.0_f64.min(duration));
        self.time_min_s = start.clamp(0.0, (duration - span).max(0.0));
        self.time_max_s = self.time_min_s + span;
    }
    pub fn zoom_time(&mut self, anchor: f64, factor: f64, duration: f64) {
        let span = self.time_max_s - self.time_min_s;
        let fraction = ((anchor - self.time_min_s) / span).clamp(0.0, 1.0);
        let next = (span * factor).clamp(0.05_f64.min(duration), 300.0_f64.min(duration));
        let start = anchor - fraction * next;
        self.set_time_range(start, start + next, duration);
    }
    pub fn pan_time(&mut self, delta: f64, duration: f64) {
        self.set_time_range(self.time_min_s + delta, self.time_max_s + delta, duration);
    }
    pub fn set_frequency_range(&mut self, start: f64, end: f64, bounds: FrequencyBounds) {
        let full_span = bounds.max_hz - bounds.min_hz;
        let minimum_span = bounds.bin_hz.min(full_span);
        let span = if start.is_finite() && end.is_finite() {
            (end - start).clamp(minimum_span, full_span)
        } else {
            full_span
        };
        let start = if start.is_finite() {
            start
        } else {
            bounds.min_hz
        };
        self.frequency_min_hz =
            start.clamp(bounds.min_hz, (bounds.max_hz - span).max(bounds.min_hz));
        self.frequency_max_hz = self.frequency_min_hz + span;
    }
    pub fn fit_frequency(&mut self, bounds: FrequencyBounds) {
        self.frequency_min_hz = bounds.min_hz;
        self.frequency_max_hz = bounds.max_hz;
    }
    pub fn pan_frequency(&mut self, delta_hz: f64, bounds: FrequencyBounds) {
        let start = self.frequency_min_hz + delta_hz;
        let end = self.frequency_max_hz + delta_hz;
        if start.is_finite() && end.is_finite() {
            self.set_frequency_range(start, end, bounds);
        }
    }
    pub fn zoom_frequency(&mut self, anchor: f64, factor: f64, bounds: FrequencyBounds) {
        if !anchor.is_finite() || !factor.is_finite() || factor <= 0.0 {
            return;
        }
        self.set_frequency_range(self.frequency_min_hz, self.frequency_max_hz, bounds);
        let span = self.frequency_max_hz - self.frequency_min_hz;
        let anchor = anchor.clamp(self.frequency_min_hz, self.frequency_max_hz);
        let fraction = (anchor - self.frequency_min_hz) / span;
        let full_span = bounds.max_hz - bounds.min_hz;
        let next = (span * factor).clamp(bounds.bin_hz.min(full_span), full_span);
        let start = anchor - fraction * next;
        self.set_frequency_range(start, start + next, bounds);
    }
    pub(super) fn valid(&self) -> bool {
        [
            self.time_min_s,
            self.time_max_s,
            self.frequency_min_hz,
            self.frequency_max_hz,
            self.db_min,
            self.db_max,
        ]
        .iter()
        .all(|value| value.is_finite())
            && self.time_min_s < self.time_max_s
            && self.frequency_min_hz < self.frequency_max_hz
            && self.db_min < self.db_max
    }
    pub(super) fn point(&self, rect: Rect, time: f64, frequency: f64) -> Pos2 {
        Pos2::new(
            rect.left()
                + ((time - self.time_min_s) / (self.time_max_s - self.time_min_s)) as f32
                    * rect.width(),
            rect.bottom()
                - ((frequency - self.frequency_min_hz)
                    / (self.frequency_max_hz - self.frequency_min_hz)) as f32
                    * rect.height(),
        )
    }
    fn coordinates(&self, rect: Rect, point: Pos2) -> (f64, f64) {
        (
            self.time_min_s
                + f64::from((point.x - rect.left()) / rect.width())
                    * (self.time_max_s - self.time_min_s),
            self.frequency_min_hz
                + f64::from((rect.bottom() - point.y) / rect.height())
                    * (self.frequency_max_hz - self.frequency_min_hz),
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct PlotInteraction {
    pub plot_rect: Option<Rect>,
    pub selected: Option<usize>,
    pub changed: bool,
    pub seek: bool,
    pub error: Option<String>,
}

fn heat(value: f32, low: f64, high: f64) -> Color32 {
    let t = ((f64::from(value) - low) / (high - low)).clamp(0., 1.) as f32;
    let stops = [
        [8., 15., 30.],
        [41., 32., 92.],
        [27., 113., 145.],
        [84., 199., 146.],
        [249., 232., 128.],
    ];
    let index = ((t * 4.).floor() as usize).min(3);
    let fraction = t * 4. - index as f32;
    let rgb = std::array::from_fn::<_, 3, _>(|c| {
        (stops[index][c] * (1. - fraction) + stops[index + 1][c] * fraction).round() as u8
    });
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

fn raster(window: &SpectrogramWindow, low: f64, high: f64, maximum: usize) -> egui::ColorImage {
    let times = window.times_s.len();
    let frequencies = window.frequencies_hz.len();
    if times == 0
        || frequencies == 0
        || times.saturating_mul(frequencies) != window.power_db.len()
        || !low.is_finite()
        || !high.is_finite()
        || low >= high
    {
        return egui::ColorImage::filled([1, 1], Color32::BLACK);
    }
    let width = times.min(maximum.max(1));
    let height = frequencies.min(maximum.max(1));
    let mut pixels = vec![Color32::BLACK; width * height];
    for y in 0..height {
        let frequency_first = (height - 1 - y) * frequencies / height;
        let frequency_last = (height - y) * frequencies / height;
        for x in 0..width {
            let first = x * times / width;
            let last = (x + 1) * times / width;
            let mut strongest = f32::NEG_INFINITY;
            for time in first..last {
                for frequency in frequency_first..frequency_last {
                    strongest = strongest.max(window.power_db[time * frequencies + frequency]);
                }
            }
            pixels[y * width + x] = heat(strongest, low, high);
        }
    }
    egui::ColorImage::new([width, height], pixels)
}

/// Frequency increases upwards; time increases rightwards. If the hardware
/// texture limit is smaller than the data grid, the display uses maximum power
/// per block. The full measured raster and its time/frequency arrays are intact.
pub fn texture(
    ctx: &egui::Context,
    window: &SpectrogramWindow,
    db_min: f64,
    db_max: f64,
) -> egui::TextureHandle {
    let maximum = ctx.input(|input| input.max_texture_side).min(4096);
    ctx.load_texture(
        "review spectrogram",
        raster(window, db_min, db_max, maximum),
        egui::TextureOptions::NEAREST,
    )
}

/// Raster cells are centered on STFT center times and FFT bin frequencies.
/// Their displayed edges extend half a hop/bin, not half an FFT window.
pub fn texture_bounds(window: &SpectrogramWindow) -> Option<[f64; 4]> {
    let hop = window.hop_samples as f64 / window.sample_rate_hz as f64;
    let bin = window.sample_rate_hz as f64 / window.fft_size as f64;
    Some([
        window.times_s.first()? - hop / 2.,
        window.times_s.last()? + hop / 2.,
        window.frequencies_hz.first()? - bin / 2.,
        window.frequencies_hz.last()? + bin / 2.,
    ])
}

fn invalidate(event: &mut EvidenceEvent) {
    event.review = ReviewState::Pending;
    event.selected_for_scoring = false;
    event.acoustic_source = geosonnar::workspace::AcousticSourceAssessment::Unassessed;
    event.origin = "manual correction".into();
    if !event.diagnostics.is_object() {
        event.diagnostics = serde_json::json!({});
    }
    event.diagnostics["manually_edited"] = true.into();
}

fn add_point(
    event: &mut EvidenceEvent,
    time: f64,
    frequency: f64,
    duration: f64,
    half_hop_s: f64,
) -> Result<bool, String> {
    if event.kind != EventKind::Doppler {
        return Err("Choose a Doppler trace to add a frequency point".into());
    }
    if !time.is_finite()
        || !frequency.is_finite()
        || time < 0.
        || time > duration
        || frequency <= 0.
        || frequency > 2000.
    {
        return Err("Point must be within the recording and between 0 and 2000 Hz".into());
    }
    if event.points.len() >= 100_000 {
        return Err("This trace reached its point limit".into());
    }
    let index = event.points.partition_point(|p| p.time_s < time);
    if event
        .points
        .get(index)
        .is_some_and(|point| (point.time_s - time).abs() < 1e-7)
        || index
            .checked_sub(1)
            .and_then(|i| event.points.get(i))
            .is_some_and(|point| (point.time_s - time).abs() < 1e-7)
    {
        return Err("A trace already has a point at that recording time; use Move point".into());
    }
    if event.source.is_none() && event.points.is_empty() {
        let padding = half_hop_s.max(0.000125);
        event.start_s = (time - padding).max(0.);
        event.end_s = (time + padding).min(duration);
        event.arrival_s = time;
    }
    event.points.insert(
        index,
        TracePoint {
            time_s: time,
            frequency_hz: frequency,
        },
    );
    event.start_s = event.start_s.min(time);
    event.end_s = event.end_s.max(time);
    invalidate(event);
    Ok(true)
}

/// A dragged stroke replaces only the time span the operator actually draws.
/// Samples are limited by the loaded hop and display resolution, not invented
/// observations between separate mouse gestures.
fn draw_segment(
    event: &mut EvidenceEvent,
    from: (f64, f64),
    to: (f64, f64),
    duration: f64,
    step: f64,
) -> Result<bool, String> {
    if event.kind != EventKind::Doppler {
        return Err("Select a Doppler event before drawing a trace".into());
    }
    let valid = |(time, frequency): (f64, f64)| {
        time.is_finite()
            && frequency.is_finite()
            && (0.0..=duration).contains(&time)
            && frequency > 0.0
            && frequency <= 2000.0
    };
    if !valid(from) || !valid(to) || !step.is_finite() || step <= 0.0 {
        return Err("Draw inside the recording and between 0 and 2000 Hz".into());
    }
    let low = from.0.min(to.0);
    let high = from.0.max(to.0);
    let samples = (((high - low) / step).ceil() as usize).clamp(1, 20_000);
    let retained = event
        .points
        .iter()
        .filter(|p| p.time_s < low - 1e-7 || p.time_s > high + 1e-7)
        .count();
    if retained + samples + 1 > 100_000 {
        return Err("This trace reached its point limit".into());
    }
    let before = event.clone();
    event
        .points
        .retain(|p| p.time_s < low - 1e-7 || p.time_s > high + 1e-7);
    for i in 0..=samples {
        let fraction = i as f64 / samples as f64;
        let point = (
            from.0 + (to.0 - from.0) * fraction,
            from.1 + (to.1 - from.1) * fraction,
        );
        // A vertical mouse movement changes the sample at this time, rather than
        // introducing duplicate timestamps which cannot be scored.
        let index = event.points.partition_point(|p| p.time_s < point.0);
        if let Some(existing) = event
            .points
            .get_mut(index)
            .filter(|p| (p.time_s - point.0).abs() < 1e-7)
        {
            existing.frequency_hz = point.1;
        } else {
            add_point(event, point.0, point.1, duration, step / 2.0)?;
        }
    }
    invalidate(event);
    Ok(*event != before)
}

fn move_point(event: &mut EvidenceEvent, index: usize, time: f64, frequency: f64) -> bool {
    if event.kind != EventKind::Doppler
        || index >= event.points.len()
        || !time.is_finite()
        || !frequency.is_finite()
    {
        return false;
    }
    let lower = if index > 0 {
        event.points[index - 1].time_s + 1e-7
    } else {
        event.start_s
    };
    let upper = if index + 1 < event.points.len() {
        event.points[index + 1].time_s - 1e-7
    } else {
        event.end_s
    };
    if !lower.is_finite() || !upper.is_finite() || lower > upper {
        return false;
    }
    let point = TracePoint {
        time_s: time.clamp(lower, upper),
        frequency_hz: frequency.clamp(0.001, 2000.),
    };
    if event.points[index] == point {
        return false;
    }
    event.points[index] = point;
    invalidate(event);
    true
}

fn erase_point(event: &mut EvidenceEvent, index: usize) -> bool {
    if event.kind != EventKind::Doppler || index >= event.points.len() {
        return false;
    }
    event.points.remove(index);
    invalidate(event);
    true
}

fn thunder_edge(event: &mut EvidenceEvent, start: bool, time: f64, duration: f64) -> bool {
    if event.kind != EventKind::Thunder
        || !time.is_finite()
        || ![event.start_s, event.end_s, event.arrival_s, duration]
            .iter()
            .all(|v| v.is_finite())
        || event.start_s < 0.
        || event.start_s >= event.end_s
        || event.end_s > duration
    {
        return false;
    }
    let old = (event.start_s, event.end_s, event.arrival_s);
    if start {
        event.start_s = time.clamp(0., (event.end_s - 0.001).max(0.));
    } else {
        event.end_s = time.clamp((event.start_s + 0.001).min(duration), duration);
    }
    event.arrival_s = event.arrival_s.clamp(event.start_s, event.end_s);
    if old == (event.start_s, event.end_s, event.arrival_s) {
        return false;
    }
    invalidate(event);
    true
}

fn nearest_point(
    event: &EvidenceEvent,
    view: &PlotView,
    rect: Rect,
    pointer: Pos2,
) -> Option<(usize, f32)> {
    event
        .points
        .iter()
        .enumerate()
        .map(|(index, p)| {
            (
                index,
                view.point(rect, p.time_s, p.frequency_hz).distance(pointer),
            )
        })
        .filter(|(_, distance)| *distance <= 12.)
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

fn hit_event(
    events: &[EvidenceEvent],
    view: &PlotView,
    rect: Rect,
    pointer: Pos2,
) -> Option<usize> {
    let point = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            nearest_point(event, view, rect, pointer).map(|(_, distance)| (index, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((index, _)) = point {
        return Some(index);
    }
    let (time, _) = view.coordinates(rect, pointer);
    events
        .iter()
        .enumerate()
        .rev()
        .find(|(_, e)| e.kind == EventKind::Thunder && e.start_s <= time && time <= e.end_s)
        .map(|(index, _)| index)
}

/// Read-only trace/band/playhead painter, also usable on top of a video frame.
/// Coordinates retain their audio time/frequency meaning over either surface.
pub fn readonly_overlay(
    ui: &egui::Ui,
    rect: Rect,
    view: &PlotView,
    events: &[EvidenceEvent],
    selected: Option<usize>,
    cursor_s: f64,
) {
    paint_events(&ui.painter_at(rect), rect, view, events, selected, cursor_s);
}

fn paint_events(
    painter: &egui::Painter,
    rect: Rect,
    view: &PlotView,
    events: &[EvidenceEvent],
    selected: Option<usize>,
    cursor_s: f64,
) {
    if rect.width() <= 1.0 || rect.height() <= 1.0 {
        return;
    }
    if !view.valid() {
        return;
    }
    for (index, event) in events.iter().enumerate() {
        if !event.start_s.is_finite()
            || !event.end_s.is_finite()
            || !event.arrival_s.is_finite()
            || event.start_s >= event.end_s
            || event.end_s < view.time_min_s
            || event.start_s > view.time_max_s
        {
            continue;
        }
        let chosen = selected == Some(index);
        let color = if chosen {
            Color32::WHITE
        } else if event.review == ReviewState::Rejected {
            Color32::from_gray(145)
        } else {
            DOPPLER_COLORS[index % DOPPLER_COLORS.len()]
        };
        if event.kind == EventKind::Thunder {
            let left = view.point(rect, event.start_s, view.frequency_max_hz).x;
            let right = view.point(rect, event.end_s, view.frequency_min_hz).x;
            let band =
                Rect::from_min_max(Pos2::new(left, rect.top()), Pos2::new(right, rect.bottom()));
            painter.rect_filled(
                band,
                0.,
                THUNDER_COLOR.gamma_multiply(if chosen { 0.22 } else { 0.12 }),
            );
            for x in [left, right] {
                painter.line_segment(
                    [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                    Stroke::new(if chosen { 5. } else { 3. }, Color32::BLACK),
                );
                painter.line_segment(
                    [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                    Stroke::new(if chosen { 2.5 } else { 1.5 }, THUNDER_COLOR),
                );
            }
            let arrival = view.point(rect, event.arrival_s, 0.).x;
            painter.line_segment(
                [
                    Pos2::new(arrival, rect.top()),
                    Pos2::new(arrival, rect.bottom()),
                ],
                Stroke::new(1., THUNDER_COLOR),
            );
        } else {
            let gap = event.diagnostics["max_gap_s"]
                .as_f64()
                .filter(|v| v.is_finite() && *v > 0.)
                .unwrap_or(2.);
            let first = event
                .points
                .partition_point(|point| point.time_s < view.time_min_s)
                .saturating_sub(1);
            let last = event
                .points
                .partition_point(|point| point.time_s <= view.time_max_s)
                .saturating_add(1)
                .min(event.points.len());
            let visible = &event.points[first.min(last)..last];
            for pair in visible.windows(2).filter(|p| {
                p[0].time_s.is_finite()
                    && p[1].time_s.is_finite()
                    && p[0].frequency_hz.is_finite()
                    && p[1].frequency_hz.is_finite()
                    && p[0].time_s < p[1].time_s
                    && (event.source.is_none() || p[1].time_s - p[0].time_s <= gap)
            }) {
                let line = [
                    view.point(rect, pair[0].time_s, pair[0].frequency_hz),
                    view.point(rect, pair[1].time_s, pair[1].frequency_hz),
                ];
                if event.source.is_none() {
                    if let Some(line) = clip_line(rect, line) {
                        painter.extend(egui::Shape::dashed_line(
                            &line,
                            Stroke::new(if chosen { 5. } else { 4. }, Color32::BLACK),
                            7.,
                            5.,
                        ));
                        painter.extend(egui::Shape::dashed_line(
                            &line,
                            Stroke::new(if chosen { 2.5 } else { 1.5 }, color),
                            7.,
                            5.,
                        ));
                    }
                } else {
                    painter.line_segment(
                        line,
                        Stroke::new(if chosen { 5. } else { 3.5 }, Color32::BLACK),
                    );
                    painter.line_segment(line, Stroke::new(if chosen { 2.5 } else { 1.5 }, color));
                }
            }
            for point in visible
                .iter()
                .filter(|point| point.time_s.is_finite() && point.frequency_hz.is_finite())
            {
                let pos = view.point(rect, point.time_s, point.frequency_hz);
                painter.circle_filled(pos, if chosen { 5. } else { 2.8 }, Color32::BLACK);
                painter.circle_filled(
                    pos,
                    if chosen { 3.5 } else { 1.7 },
                    if chosen { DOPPLER_COLORS[0] } else { color },
                );
                if chosen {
                    painter.circle_stroke(pos, 4., Stroke::new(0.8, Color32::BLACK));
                }
            }
        }
    }
    if cursor_s.is_finite() {
        let x = view.point(rect, cursor_s, 0.).x;
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.5, Color32::WHITE),
        );
    }
}

pub(super) fn clip_line(rect: Rect, line: [Pos2; 2]) -> Option<[Pos2; 2]> {
    let delta = line[1] - line[0];
    let mut first = 0_f32;
    let mut last = 1_f32;
    for (p, q) in [
        (-delta.x, line[0].x - rect.left()),
        (delta.x, rect.right() - line[0].x),
        (-delta.y, line[0].y - rect.top()),
        (delta.y, rect.bottom() - line[0].y),
    ] {
        if p == 0. {
            if q < 0. {
                return None;
            }
        } else {
            let ratio = q / p;
            if p < 0. {
                first = first.max(ratio);
            } else {
                last = last.min(ratio);
            }
            if first > last {
                return None;
            }
        }
    }
    Some([line[0] + delta * first, line[0] + delta * last])
}

/// Optional translucent spectrogram over video. The axes still describe audio
/// time/frequency; annotations never imply a position of a source in the image.
#[allow(clippy::too_many_arguments)]
pub fn paint_overlay(
    painter: &egui::Painter,
    rect: Rect,
    image: &egui::TextureHandle,
    window: &SpectrogramWindow,
    view: &PlotView,
    events: &[EvidenceEvent],
    selected: Option<usize>,
    cursor_s: f64,
) {
    if !view.valid() {
        return;
    }
    let painter = painter.with_clip_rect(rect);
    if let Some([time_min, time_max, frequency_min, frequency_max]) = texture_bounds(window) {
        let image_rect = Rect::from_min_max(
            view.point(rect, time_min, frequency_max),
            view.point(rect, time_max, frequency_min),
        );
        painter.image(
            image.id(),
            image_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
            Color32::WHITE.gamma_multiply(0.35),
        );
    }
    paint_events(&painter, rect, view, events, selected, cursor_s);
}

#[derive(Default)]
pub struct TimelineInteraction {
    pub seek: bool,
    pub changed: bool,
}
#[derive(Clone, Copy)]
struct TimelineDrag {
    edge: i8,
    origin_s: f64,
    start_s: f64,
    end_s: f64,
}

/// Full-recording overview. Gray is measured audio in memory, cyan is the
/// visible interval. Moving the view never silently invents unloaded pixels.
pub fn timeline(
    ui: &mut egui::Ui,
    duration: f64,
    events: &[EvidenceEvent],
    loaded: Option<(f64, f64)>,
    view: &mut PlotView,
    cursor_s: &mut f64,
) -> TimelineInteraction {
    let before = (view.time_min_s, view.time_max_s);
    let mut result = TimelineInteraction::default();
    let (response, painter) = ui.allocate_painter(
        Vec2::new(ui.available_width().max(1.0), 66.0),
        Sense::click_and_drag(),
    );
    let rect = Rect::from_min_max(
        response.rect.min + Vec2::new(5.0, 4.0),
        response.rect.max - Vec2::new(5.0, 20.0),
    );
    if duration <= 0.0 || rect.width() <= 0.0 {
        return result;
    }
    let x = |time: f64| rect.left() + (time / duration).clamp(0.0, 1.0) as f32 * rect.width();
    let time =
        |pos: Pos2| f64::from(((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0)) * duration;
    let drag_id = response.id.with("timeline-window");
    if response.drag_started() {
        ui.data_mut(|data| data.remove::<TimelineDrag>(drag_id));
        if let Some(origin) = ui
            .input(|input| input.pointer.press_origin())
            .filter(|p| rect.contains(*p))
        {
            let left = (origin.x - x(view.time_min_s)).abs();
            let right = (origin.x - x(view.time_max_s)).abs();
            let edge = if left.min(right) <= 8.0 {
                if left <= right { -1 } else { 1 }
            } else {
                0
            };
            ui.data_mut(|data| {
                data.insert_temp(
                    drag_id,
                    TimelineDrag {
                        edge,
                        origin_s: time(origin),
                        start_s: view.time_min_s,
                        end_s: view.time_max_s,
                    },
                )
            });
        }
    }
    if (response.dragged() || response.drag_stopped())
        && let Some(pos) = response.interact_pointer_pos()
        && let Some(drag) = ui.data(|data| data.get_temp::<TimelineDrag>(drag_id))
    {
        let minimum = 0.05_f64.min(duration);
        match drag.edge {
            -1 => {
                view.time_min_s = time(pos).clamp(
                    (drag.end_s - 300.0).max(0.0),
                    (drag.end_s - minimum).max(0.0),
                )
            }
            1 => {
                view.time_max_s = time(pos).clamp(
                    (drag.start_s + minimum).min(duration),
                    (drag.start_s + 300.0).min(duration),
                )
            }
            _ => {
                let delta = time(pos) - drag.origin_s;
                view.set_time_range(drag.start_s + delta, drag.end_s + delta, duration);
            }
        }
    }
    if response.clicked()
        && let Some(pos) = response
            .interact_pointer_pos()
            .filter(|p| rect.contains(*p))
    {
        *cursor_s = time(pos);
        let span = view.time_max_s - view.time_min_s;
        view.set_time_range(*cursor_s - span / 2.0, *cursor_s + span / 2.0, duration);
        result.seek = true;
    }
    if response.drag_stopped() || !ui.input(|input| input.pointer.primary_down()) {
        ui.data_mut(|data| data.remove::<TimelineDrag>(drag_id));
    }
    painter.rect_filled(rect, 3.0, Color32::from_rgb(27, 30, 35));
    if let Some((start, end)) = loaded {
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x(start), rect.top()),
                Pos2::new(x(end), rect.bottom()),
            ),
            0.0,
            Color32::from_rgb(73, 82, 94),
        );
    }
    for event in events {
        let (y, color) = if event.kind == EventKind::Doppler {
            (rect.top() + 9.0, DOPPLER_COLORS[0])
        } else {
            (rect.bottom() - 9.0, THUNDER_COLOR)
        };
        painter.line_segment(
            [
                Pos2::new(x(event.start_s), y),
                Pos2::new(x(event.end_s).max(x(event.start_s) + 2.0), y),
            ],
            Stroke::new(4.0, color),
        );
    }
    let visible = Rect::from_min_max(
        Pos2::new(x(view.time_min_s), rect.top()),
        Pos2::new(x(view.time_max_s), rect.bottom()),
    );
    painter.rect_filled(visible, 0.0, DOPPLER_COLORS[0].gamma_multiply(0.12));
    painter.rect_stroke(
        visible,
        0.0,
        Stroke::new(1.5, DOPPLER_COLORS[0]),
        egui::StrokeKind::Inside,
    );
    for edge in [visible.left(), visible.right()] {
        painter.rect_filled(
            Rect::from_center_size(
                Pos2::new(edge, rect.center().y),
                Vec2::new(5.0, rect.height()),
            ),
            1.0,
            DOPPLER_COLORS[0],
        );
    }
    painter.line_segment(
        [
            Pos2::new(x(*cursor_s), rect.top()),
            Pos2::new(x(*cursor_s), rect.bottom()),
        ],
        Stroke::new(1.5, Color32::WHITE),
    );
    for (fraction, align) in [
        (0.0, Align2::LEFT_TOP),
        (0.5, Align2::CENTER_TOP),
        (1.0, Align2::RIGHT_TOP),
    ] {
        painter.text(
            Pos2::new(
                rect.left() + fraction as f32 * rect.width(),
                rect.bottom() + 4.0,
            ),
            align,
            super::widgets::seconds(duration * fraction),
            FontId::proportional(11.0),
            ui.visuals().text_color(),
        );
    }
    response.on_hover_text("Full recording · gray: loaded audio · cyan: visible interval\nDrag the cyan edges to zoom; drag inside to pan; click elsewhere to move the view.");
    result.changed = before != (view.time_min_s, view.time_max_s);
    result
}

#[derive(Clone)]
struct Drag {
    event_id: String,
    event_index: usize,
    target: DragTarget,
}
#[derive(Clone, Copy)]
enum DragTarget {
    Point(usize),
    ThunderStart,
    ThunderEnd,
    Stroke { last: (f64, f64), step: f64 },
}

#[derive(Clone, Copy)]
enum InspectAxis {
    Frequency,
    Seek,
}
#[derive(Clone, Copy)]
struct InspectDrag {
    axis: InspectAxis,
    last_pointer: Pos2,
}

#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    image: &egui::TextureHandle,
    window: &SpectrogramWindow,
    view: &mut PlotView,
    events: &mut [EvidenceEvent],
    selected: Option<usize>,
    cursor_s: &mut f64,
    tool: EditTool,
    height: f32,
) -> PlotInteraction {
    let mut result = PlotInteraction {
        selected: selected.filter(|i| *i < events.len()),
        ..Default::default()
    };
    let (response, painter) = ui.allocate_painter(
        Vec2::new(ui.available_width().max(1.), height.max(80.)),
        Sense::click_and_drag(),
    );
    let rect = Rect::from_min_max(
        response.rect.min + Vec2::new(58., 16.),
        response.rect.max - Vec2::new(14., 32.),
    );
    if rect.width() <= 1.0 || rect.height() <= 1.0 {
        return result;
    }
    let Some(frequencies) = frequency_bounds(window) else {
        result.error = Some("The loaded spectrum has no usable frequency band".into());
        return result;
    };
    view.set_frequency_range(view.frequency_min_hz, view.frequency_max_hz, frequencies);
    if !view.valid() {
        result.error = Some("Plot axes must have finite ordered bounds".into());
        return result;
    }
    result.plot_rect = Some(rect);
    if ui.is_enabled() && response.hover_pos().is_some_and(|pos| rect.contains(pos)) {
        let (scroll, modifiers, pointer_down, zoom) = ui.input(|input| {
            (
                input.smooth_scroll_delta,
                input.modifiers,
                input.pointer.primary_down(),
                input.zoom_delta(),
            )
        });
        let time_only = modifiers.ctrl || modifiers.command;
        if scroll != Vec2::ZERO || time_only && zoom != 1.0 {
            // Consume the wheel over this canvas; the containing workspace must
            // not scroll at the same time. Keep axes fixed during a trace stroke.
            ui.input_mut(|input| input.smooth_scroll_delta = Vec2::ZERO);
            if !pointer_down {
                if !time_only
                    && !modifiers.alt
                    && (modifiers.shift || scroll.x.abs() > scroll.y.abs())
                {
                    let delta = if scroll.x != 0.0 { scroll.x } else { scroll.y };
                    view.pan_time(
                        -f64::from(delta) * (view.time_max_s - view.time_min_s) / 500.0,
                        window.source_duration_s,
                    );
                } else if let Some(pos) = response.hover_pos() {
                    let (time, frequency) = view.coordinates(rect, pos);
                    // egui turns Ctrl-wheel into zoom_delta before widgets see
                    // smooth_scroll_delta. Consume that scale locally; changing
                    // a plot never changes the application's text/UI scale.
                    let factor = if time_only && zoom != 1.0 {
                        f64::from(zoom).recip()
                    } else {
                        (-f64::from(scroll.y) * 0.008).exp()
                    };
                    if time_only || !modifiers.alt {
                        view.zoom_time(time, factor, window.source_duration_s);
                    }
                    if !time_only {
                        view.zoom_frequency(frequency, factor, frequencies);
                    }
                }
            }
        }
    }
    // Inspection gestures change only the viewport/cursor. Apply them before
    // painting so the measured raster, axes and event overlays use one view.
    let inspect_id = response.id.with("inspect-pan");
    if !ui.is_enabled() || tool != EditTool::Inspect {
        ui.data_mut(|data| data.remove::<InspectDrag>(inspect_id));
    } else {
        if response.drag_started_by(egui::PointerButton::Primary) {
            ui.data_mut(|data| data.remove::<InspectDrag>(inspect_id));
            if let Some(origin) = ui
                .input(|input| input.pointer.press_origin())
                .filter(|point| rect.contains(*point))
                && let Some(pointer) = response.interact_pointer_pos()
            {
                let delta = pointer - origin;
                let axis = if delta.y.abs() >= delta.x.abs() {
                    InspectAxis::Frequency
                } else {
                    InspectAxis::Seek
                };
                ui.data_mut(|data| {
                    data.insert_temp(
                        inspect_id,
                        InspectDrag {
                            axis,
                            last_pointer: origin,
                        },
                    )
                });
            }
        }
        if (response.dragged_by(egui::PointerButton::Primary)
            || response.drag_stopped_by(egui::PointerButton::Primary))
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some(mut drag) = ui.data(|data| data.get_temp::<InspectDrag>(inspect_id))
        {
            match drag.axis {
                InspectAxis::Frequency => {
                    let delta_hz = f64::from((pointer.y - drag.last_pointer.y) / rect.height())
                        * (view.frequency_max_hz - view.frequency_min_hz);
                    view.pan_frequency(delta_hz, frequencies);
                }
                InspectAxis::Seek => {
                    let (time, _) = view.coordinates(rect, rect.clamp(pointer));
                    let next = time.clamp(0.0, window.source_duration_s);
                    if next != *cursor_s {
                        *cursor_s = next;
                        result.seek = true;
                    }
                }
            }
            // Rebase even at a band edge: reversing the pointer moves the view
            // immediately rather than retracing an accumulated overshoot.
            drag.last_pointer = pointer;
            ui.data_mut(|data| data.insert_temp(inspect_id, drag));
        }
    }
    if response.drag_stopped_by(egui::PointerButton::Primary)
        || !ui.input(|input| input.pointer.primary_down())
    {
        ui.data_mut(|data| data.remove::<InspectDrag>(inspect_id));
    }
    painter.rect_filled(rect, 0., Color32::from_rgb(8, 15, 30));
    if let Some([time_min, time_max, frequency_min, frequency_max]) = texture_bounds(window) {
        let image_rect = Rect::from_min_max(
            view.point(rect, time_min, frequency_max),
            view.point(rect, time_max, frequency_min),
        );
        painter.with_clip_rect(rect).image(
            image.id(),
            image_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
            Color32::WHITE,
        );
    }
    for index in 0..=4 {
        let fraction = f64::from(index) / 4.;
        let time = view.time_min_s + (view.time_max_s - view.time_min_s) * fraction;
        let frequency =
            view.frequency_min_hz + (view.frequency_max_hz - view.frequency_min_hz) * fraction;
        let point = view.point(rect, time, frequency);
        painter.line_segment(
            [
                Pos2::new(rect.left(), point.y),
                Pos2::new(rect.right(), point.y),
            ],
            Stroke::new(0.5, BORDER),
        );
        painter.text(
            Pos2::new(rect.left() - 7., point.y),
            Align2::RIGHT_CENTER,
            if view.frequency_max_hz - view.frequency_min_hz < 10.0 {
                format!("{frequency:.2} Hz")
            } else {
                format!("{frequency:.0} Hz")
            },
            FontId::proportional(11.),
            MUTED,
        );
        painter.text(
            Pos2::new(point.x, rect.bottom() + 7.),
            Align2::CENTER_TOP,
            format!("{time:.2}s"),
            FontId::proportional(11.),
            MUTED,
        );
    }
    let drag_id = response.id.with("annotation-drag");
    let pointer = response
        .interact_pointer_pos()
        .or_else(|| response.hover_pos());
    if response.drag_started() || !ui.is_enabled() {
        ui.data_mut(|data| data.remove::<Drag>(drag_id));
    }
    if ui.is_enabled() {
        if response.drag_started()
            && let Some(pos) = ui
                .input(|input| input.pointer.press_origin())
                .filter(|p| rect.contains(*p))
            && let Some(index) = result.selected
        {
            let event = &events[index];
            let target = match tool {
                EditTool::DrawTrace if event.kind == EventKind::Doppler => {
                    Some(DragTarget::Stroke {
                        last: view.coordinates(rect, pos),
                        step: (window.hop_samples as f64 / window.sample_rate_hz as f64 / 2.0).max(
                            (view.time_max_s - view.time_min_s) / f64::from(rect.width()) * 2.0,
                        ),
                    })
                }
                EditTool::MovePoint => {
                    nearest_point(event, view, rect, pos).map(|(point, _)| DragTarget::Point(point))
                }
                EditTool::ThunderInterval if event.kind == EventKind::Thunder => {
                    let left = (view.point(rect, event.start_s, 0.).x - pos.x).abs();
                    let right = (view.point(rect, event.end_s, 0.).x - pos.x).abs();
                    if left.min(right) <= 14. {
                        Some(if left <= right {
                            DragTarget::ThunderStart
                        } else {
                            DragTarget::ThunderEnd
                        })
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(target) = target {
                ui.data_mut(|data| {
                    data.insert_temp(
                        drag_id,
                        Drag {
                            event_id: event.id.clone(),
                            event_index: index,
                            target,
                        },
                    )
                });
            }
        }
        if (response.dragged() || response.drag_stopped())
            && let Some(pos) = pointer
            && let Some(drag) = ui.data(|data| data.get_temp::<Drag>(drag_id))
            && let Some(event) = events
                .get_mut(drag.event_index)
                .filter(|e| e.id == drag.event_id)
        {
            let (time, frequency) = view.coordinates(rect, rect.clamp(pos));
            result.changed |= match drag.target {
                DragTarget::Point(index) => move_point(event, index, time, frequency),
                DragTarget::ThunderStart => {
                    thunder_edge(event, true, time, window.source_duration_s)
                }
                DragTarget::ThunderEnd => {
                    thunder_edge(event, false, time, window.source_duration_s)
                }
                DragTarget::Stroke { last, step } => {
                    let next = (
                        time.clamp(0.0, window.source_duration_s),
                        frequency.clamp(0.01, 2000.0),
                    );
                    let changed =
                        match draw_segment(event, last, next, window.source_duration_s, step) {
                            Ok(changed) => changed,
                            Err(error) => {
                                result.error = Some(error);
                                false
                            }
                        };
                    ui.data_mut(|data| {
                        data.insert_temp(
                            drag_id,
                            Drag {
                                target: DragTarget::Stroke { last: next, step },
                                ..drag
                            },
                        )
                    });
                    changed
                }
            };
        }
        if response.clicked()
            && let Some(pos) = pointer.filter(|p| rect.contains(*p))
        {
            let (time, frequency) = view.coordinates(rect, pos);
            if tool == EditTool::Inspect {
                if let Some(index) = hit_event(events, view, rect, pos) {
                    result.selected = Some(index)
                } else {
                    *cursor_s = time.clamp(0., window.source_duration_s);
                    result.seek = true;
                }
            } else if let Some(index) = result.selected {
                let event = &mut events[index];
                match tool {
                    EditTool::AddPoint | EditTool::DrawTrace => {
                        match add_point(
                            event,
                            time,
                            frequency,
                            window.source_duration_s,
                            window.hop_samples as f64 / window.sample_rate_hz as f64 / 2.,
                        ) {
                            Ok(changed) => result.changed |= changed,
                            Err(error) => result.error = Some(error),
                        }
                    }
                    EditTool::ErasePoint => {
                        if let Some((point, _)) = nearest_point(event, view, rect, pos) {
                            result.changed |= erase_point(event, point)
                        }
                    }
                    EditTool::MovePoint | EditTool::ThunderInterval => {}
                    EditTool::Inspect => {}
                }
            } else {
                result.error = Some("Select an event before editing its points or interval".into());
            }
        }
    }
    if response.drag_stopped() || !ui.input(|input| input.pointer.primary_down()) {
        ui.data_mut(|data| data.remove::<Drag>(drag_id));
    }
    readonly_overlay(ui, rect, view, events, result.selected, *cursor_s);
    let legend = Rect::from_min_size(
        rect.min + Vec2::splat(5.),
        Vec2::new(rect.width().min(270.) - 10., 34.),
    );
    painter
        .with_clip_rect(rect)
        .rect_filled(legend, 3., Color32::from_black_alpha(180));
    for (fraction, label, color) in [
        (0., "Doppler", DOPPLER_COLORS[0]),
        (0.34, "Thunder", THUNDER_COLOR),
        (0.68, "Selected", Color32::WHITE),
    ] {
        painter.with_clip_rect(rect).text(
            legend.min + Vec2::new(5. + fraction * legend.width(), 3.),
            Align2::LEFT_TOP,
            label,
            FontId::proportional(10.),
            color,
        );
    }
    painter.with_clip_rect(rect).text(
        legend.min + Vec2::new(5., 18.),
        Align2::LEFT_TOP,
        "Dashed connections = manual guides",
        FontId::proportional(10.),
        Color32::LIGHT_GRAY,
    );
    if let Some(pos) = response.hover_pos().filter(|p| rect.contains(*p)) {
        let (time, frequency) = view.coordinates(rect, pos);
        response.on_hover_text(format!(
            "{time:.3} s · {frequency:.2} Hz\n{}",
            match tool {
                EditTool::Inspect => "Click a trace to select it; click empty space to seek. Drag vertically to pan frequency; drag horizontally to scrub time.",
                EditTool::AddPoint =>
                    "Click to add an explicit anchor. Dashed connections are visual guides; only anchors are scored.",
                EditTool::DrawTrace => "Hold the left mouse button and trace the ridge. Your stroke replaces points in the drawn time span.",
                EditTool::MovePoint =>
                    "Drag a selected trace point; neighboring times remain ordered.",
                EditTool::ErasePoint => "Click a selected trace point to remove it.",
                EditTool::ThunderInterval => "Drag either edge of the selected thunder interval.",
            }
        ));
    }
    if let Some(error) = &result.error {
        ui.colored_label(RED, error);
    }
    ui.colored_label(
        MUTED,
        format!(
            "PSD {:.0} to {:.0} dBFS/Hz · {} time frames × {} frequency bins{}",
            view.db_min,
            view.db_max,
            window.times_s.len(),
            window.frequencies_hz.len(),
            if image.size() != [window.times_s.len(), window.frequencies_hz.len()] {
                " · display pools peak power; measurements stay unchanged"
            } else {
                ""
            }
        ),
    );
    if events.iter().any(|event| {
        event.kind == EventKind::Doppler && event.source.is_none() && event.points.len() > 1
    }) {
        ui.label("Manual connections are visual guides; only anchors are scored.");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(kind: EventKind) -> EvidenceEvent {
        let mut event = EvidenceEvent::manual("manual-1".into(), kind, 1., 9.);
        event.review = ReviewState::Confirmed;
        event.acoustic_source = geosonnar::workspace::AcousticSourceAssessment::Compatible;
        event.selected_for_scoring = true;
        event
    }
    fn view() -> PlotView {
        PlotView {
            time_min_s: 10.,
            time_max_s: 30.,
            frequency_min_hz: 50.,
            frequency_max_hz: 250.,
            db_min: -100.,
            db_max: -10.,
        }
    }
    fn window() -> SpectrogramWindow {
        SpectrogramWindow {
            input: std::path::PathBuf::from("fabricated.wav"),
            source_sha256: "0".repeat(64),
            source_stamp: geosonnar::review_media::SourceStamp {
                path: "fabricated.wav".into(),
                bytes: 1,
                modified_unix_ns: 0,
            },
            channel: 0,
            source_duration_s: 20.,
            requested_start_s: 10.,
            requested_end_s: 11.,
            start_s: 10.,
            end_s: 11.,
            decoded_samples: 4000,
            sample_rate_hz: 4000,
            fft_size: 1024,
            hop_samples: 512,
            times_s: vec![10.128, 10.256],
            frequencies_hz: vec![0., 3.90625, 7.8125],
            power_db: vec![-100., -60., -20., -90., -50., -10.],
            envelope: vec![],
            envelope_band_hz: [20., 250.],
            envelope_method: "Fabricated raster orientation control".into(),
            time_reference: "Synthetic recording seconds".into(),
        }
    }
    fn editing_window() -> SpectrogramWindow {
        let mut window = window();
        window.sample_rate_hz = 4096;
        window.decoded_samples = 4096;
        window.times_s = vec![10.125, 10.25];
        window.frequencies_hz = (0..=50).map(|bin| bin as f64 * 4.0).collect();
        window.power_db = vec![-60.0; window.times_s.len() * window.frequencies_hz.len()];
        window
    }
    #[test]
    fn axes_map_recording_seconds_and_frequency_without_vertical_flip() {
        let rect = Rect::from_min_max(Pos2::new(20., 40.), Pos2::new(420., 240.));
        let view = view();
        assert_eq!(view.point(rect, 10., 50.), rect.left_bottom());
        assert_eq!(view.point(rect, 30., 250.), rect.right_top());
        assert_eq!(view.coordinates(rect, Pos2::new(120., 90.)), (15., 200.));
    }
    #[test]
    fn raster_rows_are_high_to_low_frequency_and_cells_use_half_hop_edges() {
        let window = window();
        let image = raster(&window, -100., 0., 2048);
        assert_eq!(image.size, [2, 3]);
        assert_eq!(
            image.pixels,
            [
                heat(-20., -100., 0.),
                heat(-10., -100., 0.),
                heat(-60., -100., 0.),
                heat(-50., -100., 0.),
                heat(-100., -100., 0.),
                heat(-90., -100., 0.)
            ]
        );
        let bounds = texture_bounds(&window).unwrap();
        assert!((bounds[0] - 10.064).abs() < 1e-12);
        assert!((bounds[1] - 10.32).abs() < 1e-12);
        assert_eq!([bounds[2], bounds[3]], [-1.953125, 9.765625]);
        let pooled = raster(&window, -100., 0., 1);
        assert_eq!(pooled.pixels, [heat(-10., -100., 0.)]);
        assert_eq!(window.power_db, [-100., -60., -20., -90., -50., -10.]);
    }
    #[test]
    fn manual_points_remain_ordered_exact_and_require_review_again() {
        let mut event = event(EventKind::Doppler);
        add_point(&mut event, 5.25, 123.456, 10., 0.125).unwrap();
        assert_eq!(
            (event.start_s, event.end_s, event.arrival_s),
            (5.125, 5.375, 5.25)
        );
        add_point(&mut event, 2.5, 130., 10., 0.125).unwrap();
        assert_eq!(
            event.points[1],
            TracePoint {
                time_s: 5.25,
                frequency_hz: 123.456
            }
        );
        assert_eq!(event.review, ReviewState::Pending);
        assert!(event.acoustic_source.is_unassessed());
        assert!(!event.selected_for_scoring);
        assert!(add_point(&mut event, 5.25, 120., 10., 0.125).is_err());
        assert!(move_point(&mut event, 0, 8., 140.));
        assert!(event.points[0].time_s < event.points[1].time_s);
        assert!(erase_point(&mut event, 1));
        assert_eq!(event.points.len(), 1);
    }
    #[test]
    fn thunder_interval_edits_preserve_arrival_and_never_create_frequency_points() {
        let mut event = event(EventKind::Thunder);
        event.arrival_s = 2.;
        assert!(thunder_edge(&mut event, true, 4., 10.));
        assert_eq!(event.arrival_s, 4.);
        assert!(thunder_edge(&mut event, false, 3., 10.));
        assert!(event.end_s > event.start_s);
        assert!(event.points.is_empty());
        assert!(event.acoustic_source.is_unassessed());
        assert!(!event.selected_for_scoring);
        assert!(add_point(&mut event, 5., 100., 10., 0.125).is_err());
    }

    #[test]
    fn rendered_editor_click_drag_and_erase_change_only_the_selected_trace() {
        let ctx = egui::Context::default();
        let window = editing_window();
        let image = texture(&ctx, &window, -100., 0.);
        let view = PlotView {
            time_min_s: 0.,
            time_max_s: 10.,
            frequency_min_hz: 0.,
            frequency_max_hz: 200.,
            db_min: -100.,
            db_max: 0.,
        };
        let mut first = event(EventKind::Doppler);
        first.points.push(TracePoint {
            time_s: 7.,
            frequency_hz: 120.,
        });
        let mut second = event(EventKind::Doppler);
        second.id = "manual-2".into();
        let untouched = second.clone();
        let mut events = vec![first, second];
        let mut cursor = 0.;
        let mut render =
            |inputs: Vec<egui::Event>, tool: EditTool, events: &mut Vec<EvidenceEvent>| {
                let mut result = None;
                let mut plot = None;
                let _ = ctx.run_ui(
                    egui::RawInput {
                        events: inputs,
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.))),
                        ..Default::default()
                    },
                    |ui| {
                        let start = ui.cursor().min;
                        let width = ui.available_width().max(160.);
                        plot = Some(Rect::from_min_max(
                            start + Vec2::new(58., 16.),
                            start + Vec2::new(width - 14., 268.),
                        ));
                        result = Some(show(
                            ui,
                            &image,
                            &window,
                            &mut view.clone(),
                            events,
                            Some(0),
                            &mut cursor,
                            tool,
                            300.,
                        ));
                    },
                );
                (result.unwrap(), plot.unwrap())
            };
        let (_, rect) = render(vec![], EditTool::AddPoint, &mut events);
        let click = |point: Pos2, pressed: bool| {
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let added = view.point(rect, 3., 140.);
        render(click(added, true), EditTool::AddPoint, &mut events);
        assert!(
            render(click(added, false), EditTool::AddPoint, &mut events)
                .0
                .changed
        );
        assert_eq!(events[0].points.len(), 2);
        assert!((events[0].points[0].time_s - 3.).abs() < 1e-6);
        assert!((events[0].points[0].frequency_hz - 140.).abs() < 1e-5);
        render(click(added, true), EditTool::MovePoint, &mut events);
        let destination = view.point(rect, 9., 160.);
        assert!(
            render(
                vec![egui::Event::PointerMoved(destination)],
                EditTool::MovePoint,
                &mut events
            )
            .0
            .changed
        );
        render(click(destination, false), EditTool::MovePoint, &mut events);
        assert!(events[0].points[0].time_s < 7.);
        assert!((events[0].points[0].time_s - 7.).abs() < 1e-5);
        let moved = view.point(
            rect,
            events[0].points[0].time_s,
            events[0].points[0].frequency_hz,
        );
        render(click(moved, true), EditTool::ErasePoint, &mut events);
        assert!(
            render(click(moved, false), EditTool::ErasePoint, &mut events)
                .0
                .changed
        );
        assert_eq!(
            events[0].points,
            [TracePoint {
                time_s: 7.,
                frequency_hz: 120.
            }]
        );
        assert_eq!(events[0].review, ReviewState::Pending);
        assert!(!events[0].selected_for_scoring);
        assert_eq!(events[1], untouched);
    }
    struct EditorHarness {
        ctx: egui::Context,
        window: SpectrogramWindow,
        image: egui::TextureHandle,
        view: PlotView,
        events: Vec<EvidenceEvent>,
        cursor: f64,
    }
    impl EditorHarness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let window = editing_window();
            let image = texture(&ctx, &window, -100.0, 0.0);
            Self {
                ctx,
                window,
                image,
                view: PlotView {
                    time_min_s: 0.0,
                    time_max_s: 10.0,
                    frequency_min_hz: 0.0,
                    frequency_max_hz: 200.0,
                    db_min: -100.0,
                    db_max: 0.0,
                },
                events: vec![event(EventKind::Doppler)],
                cursor: 0.0,
            }
        }
        fn render(&mut self, inputs: Vec<egui::Event>, tool: EditTool) -> (PlotInteraction, Rect) {
            let mut result = PlotInteraction::default();
            let mut rect = Rect::NOTHING;
            let modifiers = inputs
                .iter()
                .rev()
                .find_map(|event| match event {
                    egui::Event::MouseWheel { modifiers, .. }
                    | egui::Event::PointerButton { modifiers, .. } => Some(*modifiers),
                    _ => None,
                })
                .unwrap_or_default();
            let _ = self.ctx.run_ui(
                egui::RawInput {
                    events: inputs,
                    modifiers,
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                    ..Default::default()
                },
                |ui| {
                    let start = ui.cursor().min;
                    rect = Rect::from_min_max(
                        start + Vec2::new(58.0, 16.0),
                        start + Vec2::new(ui.available_width() - 14.0, 268.0),
                    );
                    result = show(
                        ui,
                        &self.image,
                        &self.window,
                        &mut self.view,
                        &mut self.events,
                        Some(0),
                        &mut self.cursor,
                        tool,
                        300.0,
                    );
                },
            );
            (result, rect)
        }
    }
    fn pointer_button(position: Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }
    #[test]
    fn wheel_zoom_is_pointer_anchored_consumed_and_does_not_edit_evidence() {
        let mut harness = EditorHarness::new();
        let original = harness.events.clone();
        let original_psd = harness.window.power_db.clone();
        let (_, rect) = harness.render(vec![], EditTool::Inspect);
        let anchor = harness.view.point(rect, 3.0, 100.0);
        harness.render(vec![egui::Event::PointerMoved(anchor)], EditTool::Inspect);
        let (interaction, _) = harness.render(
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: Vec2::new(0.0, 60.0),
                modifiers: egui::Modifiers::NONE,
            }],
            EditTool::Inspect,
        );
        assert!(harness.view.time_max_s - harness.view.time_min_s < 10.0);
        assert!(harness.view.frequency_max_hz - harness.view.frequency_min_hz < 200.0);
        assert!((harness.view.coordinates(rect, anchor).0 - 3.0).abs() < 1e-6);
        assert!((harness.view.coordinates(rect, anchor).1 - 100.0).abs() < 1e-5);
        assert_eq!(
            harness.ctx.input(|input| input.smooth_scroll_delta),
            Vec2::ZERO
        );
        assert_eq!(harness.events, original);
        assert_eq!(harness.window.power_db, original_psd);
        assert!(!interaction.changed && !interaction.seek);
        harness.view.zoom_time(3.0, 1e12, 1000.0);
        assert_eq!(harness.view.time_max_s - harness.view.time_min_s, 300.0);
        harness.view.pan_time(1e6, 1000.0);
        assert_eq!(
            (harness.view.time_min_s, harness.view.time_max_s),
            (700.0, 1000.0)
        );
    }
    #[test]
    fn frequency_zoom_preserves_anchor_and_recovers_outward_within_the_loaded_band() {
        let window = editing_window();
        let bounds = frequency_bounds(&window).unwrap();
        let mut view = view();
        view.set_frequency_range(40.0, 160.0, bounds);
        view.zoom_frequency(130.0, 0.5, bounds);
        assert_eq!(
            (view.frequency_min_hz, view.frequency_max_hz),
            (85.0, 145.0)
        );
        assert_eq!(
            (130.0 - view.frequency_min_hz) / (view.frequency_max_hz - view.frequency_min_hz),
            0.75
        );
        view.zoom_frequency(130.0, 1e-12, bounds);
        assert_eq!(view.frequency_max_hz - view.frequency_min_hz, bounds.bin_hz);
        view.zoom_frequency(130.0, 1e12, bounds);
        assert_eq!((view.frequency_min_hz, view.frequency_max_hz), (0.0, 200.0));
        view.set_frequency_range(199.0, 5000.0, bounds);
        assert!(view.frequency_min_hz >= 0.0 && view.frequency_max_hz <= 200.0);
        view.fit_frequency(bounds);
        assert_eq!((view.frequency_min_hz, view.frequency_max_hz), (0.0, 200.0));
        let mut low_rate = window;
        low_rate.sample_rate_hz = 320;
        let bounds = frequency_bounds(&low_rate).unwrap();
        assert_eq!(bounds.max_hz, 160.0);
        view.set_frequency_range(-100.0, 300.0, bounds);
        assert_eq!((view.frequency_min_hz, view.frequency_max_hz), (0.0, 160.0));
    }
    #[test]
    fn wheel_modifiers_change_only_the_requested_axis_and_keep_ui_scale() {
        for (modifiers, time_changes, frequency_changes) in [
            (
                egui::Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
                true,
                false,
            ),
            (egui::Modifiers::ALT, false, true),
        ] {
            let mut harness = EditorHarness::new();
            let original = harness.events.clone();
            let (_, rect) = harness.render(vec![], EditTool::Inspect);
            let anchor = harness.view.point(rect, 3.0, 120.0);
            let original_scale = harness.ctx.zoom_factor();
            harness.render(vec![egui::Event::PointerMoved(anchor)], EditTool::Inspect);
            let (result, _) = harness.render(
                vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    phase: egui::TouchPhase::Move,
                    delta: Vec2::new(0.0, 60.0),
                    modifiers,
                }],
                EditTool::Inspect,
            );
            assert_eq!(
                harness.view.time_max_s - harness.view.time_min_s < 10.0,
                time_changes
            );
            assert_eq!(
                harness.view.frequency_max_hz - harness.view.frequency_min_hz < 200.0,
                frequency_changes
            );
            assert!((harness.view.coordinates(rect, anchor).0 - 3.0).abs() < 1e-6);
            assert!((harness.view.coordinates(rect, anchor).1 - 120.0).abs() < 1e-5);
            assert_eq!(harness.ctx.zoom_factor(), original_scale);
            assert_eq!(harness.events, original);
            assert!(!result.changed && !result.seek);
        }
    }
    #[test]
    fn shift_wheel_pans_time_without_zoom_or_evidence_edits() {
        let mut harness = EditorHarness::new();
        harness.view.set_time_range(5.0, 15.0, 20.0);
        let original = harness.events.clone();
        let (_, rect) = harness.render(vec![], EditTool::Inspect);
        harness.render(
            vec![egui::Event::PointerMoved(rect.center())],
            EditTool::Inspect,
        );
        let (result, _) = harness.render(
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: Vec2::new(0.0, 60.0),
                modifiers: egui::Modifiers::SHIFT,
            }],
            EditTool::Inspect,
        );
        assert!(harness.view.time_min_s < 5.0);
        assert!((harness.view.time_max_s - harness.view.time_min_s - 10.0).abs() < 1e-9);
        assert_eq!(
            (harness.view.frequency_min_hz, harness.view.frequency_max_hz),
            (0.0, 200.0)
        );
        assert_eq!(harness.events, original);
        assert!(!result.changed && !result.seek);
    }
    #[test]
    fn inspect_vertical_drag_pans_frequency_without_seeking_or_editing_and_stops_on_release() {
        let mut harness = EditorHarness::new();
        harness
            .view
            .set_frequency_range(50.0, 130.0, frequency_bounds(&harness.window).unwrap());
        let original = harness.events.clone();
        let original_psd = harness.window.power_db.clone();
        let (_, rect) = harness.render(vec![], EditTool::Inspect);
        let start = rect.center();
        let destination = start + Vec2::new(8.0, 40.0);
        harness.render(pointer_button(start, true), EditTool::Inspect);
        let (result, _) = harness.render(
            vec![egui::Event::PointerMoved(destination)],
            EditTool::Inspect,
        );
        let lower = harness.view.frequency_min_hz;
        assert!(lower > 50.0);
        assert!((harness.view.frequency_max_hz - lower - 80.0).abs() < 1e-8);
        assert_eq!(
            (
                harness.view.time_min_s,
                harness.view.time_max_s,
                harness.cursor
            ),
            (0.0, 10.0, 0.0)
        );
        assert_eq!(result.selected, Some(0));
        assert!(!result.changed && !result.seek);
        // A held but stationary pointer must not reapply the drag each frame.
        for _ in 0..3 {
            harness.render(vec![], EditTool::Inspect);
        }
        assert_eq!(harness.view.frequency_min_hz, lower);
        harness.render(pointer_button(destination, false), EditTool::Inspect);
        harness.render(vec![egui::Event::PointerMoved(start)], EditTool::Inspect);
        assert_eq!(harness.view.frequency_min_hz, lower);
        assert_eq!(harness.events, original);
        assert_eq!(harness.window.power_db, original_psd);
        // The annotation and measured-raster coordinate transform share the new view.
        let pinned = harness.view.point(rect, 5.0, 100.0);
        let expected = start.y - (100.0 - 90.0) / 80.0 * rect.height() + 40.0;
        assert!((pinned.y - expected).abs() < 1e-4);
    }
    #[test]
    fn inspect_frequency_pan_clamps_both_loaded_edges_and_reverses_immediately() {
        for direction in [-1.0_f32, 1.0] {
            let mut harness = EditorHarness::new();
            harness.view.set_frequency_range(
                60.0,
                140.0,
                frequency_bounds(&harness.window).unwrap(),
            );
            let (_, rect) = harness.render(vec![], EditTool::Inspect);
            let start = rect.center();
            harness.render(pointer_button(start, true), EditTool::Inspect);
            let overshoot = start + Vec2::new(0.0, direction * 2000.0);
            let (result, _) = harness.render(
                vec![egui::Event::PointerMoved(overshoot)],
                EditTool::Inspect,
            );
            let expected = if direction > 0.0 {
                (120.0, 200.0)
            } else {
                (0.0, 80.0)
            };
            assert_eq!(
                (harness.view.frequency_min_hz, harness.view.frequency_max_hz),
                expected
            );
            assert!(!result.changed && !result.seek);
            let reversal = overshoot - Vec2::new(0.0, direction * 20.0);
            harness.render(vec![egui::Event::PointerMoved(reversal)], EditTool::Inspect);
            assert!(harness.view.frequency_min_hz > 0.0 && harness.view.frequency_max_hz < 200.0);
            assert!(
                (harness.view.frequency_max_hz - harness.view.frequency_min_hz - 80.0).abs() < 1e-8
            );
            harness.render(pointer_button(reversal, false), EditTool::Inspect);
        }
    }
    #[test]
    fn inspect_click_selection_seek_and_horizontal_scrub_preserve_frequency_view() {
        let mut harness = EditorHarness::new();
        harness.events[0].points = vec![TracePoint {
            time_s: 2.0,
            frequency_hz: 80.0,
        }];
        harness
            .view
            .set_frequency_range(40.0, 140.0, frequency_bounds(&harness.window).unwrap());
        let original = harness.events.clone();
        let (_, rect) = harness.render(vec![], EditTool::Inspect);
        let trace = harness.view.point(rect, 2.0, 80.0);
        harness.render(pointer_button(trace, true), EditTool::Inspect);
        let (selected, _) = harness.render(pointer_button(trace, false), EditTool::Inspect);
        assert_eq!(selected.selected, Some(0));
        assert!(!selected.seek);
        let empty = harness.view.point(rect, 6.0, 120.0);
        harness.render(pointer_button(empty, true), EditTool::Inspect);
        let (seek, _) = harness.render(pointer_button(empty, false), EditTool::Inspect);
        assert!(seek.seek && !seek.changed);
        assert!((harness.cursor - 6.0).abs() < 1e-6);
        let start = harness.view.point(rect, 4.0, 100.0);
        let end = harness.view.point(rect, 8.0, 95.0);
        harness.render(pointer_button(start, true), EditTool::Inspect);
        let (scrub, _) = harness.render(vec![egui::Event::PointerMoved(end)], EditTool::Inspect);
        assert!(scrub.seek && !scrub.changed);
        assert!((harness.cursor - 8.0).abs() < 1e-6);
        // Initial horizontal intent stays locked even after a large vertical move.
        harness.render(
            vec![egui::Event::PointerMoved(end + Vec2::new(0.0, 100.0))],
            EditTool::Inspect,
        );
        assert_eq!(
            (harness.view.frequency_min_hz, harness.view.frequency_max_hz),
            (40.0, 140.0)
        );
        harness.render(pointer_button(end, false), EditTool::Inspect);
        assert_eq!(harness.events, original);
    }
    #[test]
    fn inspect_pan_state_does_not_leak_into_trace_tools_or_start_outside_the_plot() {
        let mut harness = EditorHarness::new();
        harness
            .view
            .set_frequency_range(40.0, 140.0, frequency_bounds(&harness.window).unwrap());
        let (_, rect) = harness.render(vec![], EditTool::Inspect);
        let outside = Pos2::new(rect.left() - 20.0, rect.center().y);
        harness.render(pointer_button(outside, true), EditTool::Inspect);
        harness.render(
            vec![egui::Event::PointerMoved(
                rect.center() + Vec2::new(0.0, 40.0),
            )],
            EditTool::Inspect,
        );
        harness.render(pointer_button(rect.center(), false), EditTool::Inspect);
        assert_eq!(
            (harness.view.frequency_min_hz, harness.view.frequency_max_hz),
            (40.0, 140.0)
        );
        let start = rect.center();
        harness.render(pointer_button(start, true), EditTool::Inspect);
        harness.render(
            vec![egui::Event::PointerMoved(start + Vec2::new(0.0, 30.0))],
            EditTool::Inspect,
        );
        let band = (harness.view.frequency_min_hz, harness.view.frequency_max_hz);
        harness.render(
            vec![egui::Event::PointerMoved(start + Vec2::new(0.0, 60.0))],
            EditTool::DrawTrace,
        );
        harness.render(
            vec![egui::Event::PointerMoved(start + Vec2::new(0.0, 90.0))],
            EditTool::Inspect,
        );
        harness.render(pointer_button(start, false), EditTool::Inspect);
        assert_eq!(
            (harness.view.frequency_min_hz, harness.view.frequency_max_hz),
            band
        );
        for tool in [
            EditTool::DrawTrace,
            EditTool::AddPoint,
            EditTool::MovePoint,
            EditTool::ErasePoint,
            EditTool::ThunderInterval,
        ] {
            harness.render(pointer_button(start, true), tool);
            harness.render(
                vec![egui::Event::PointerMoved(start + Vec2::new(0.0, 30.0))],
                tool,
            );
            harness.render(pointer_button(start + Vec2::new(0.0, 30.0), false), tool);
            assert_eq!(
                (harness.view.frequency_min_hz, harness.view.frequency_max_hz),
                band
            );
        }
    }
    #[test]
    fn mouse_stroke_replaces_only_drawn_span_keeps_order_and_invalidates_review() {
        let mut harness = EditorHarness::new();
        harness.events[0].points = [1.0, 3.0, 5.0, 7.0, 9.0]
            .into_iter()
            .map(|time_s| TracePoint {
                time_s,
                frequency_hz: 100.0,
            })
            .collect();
        let outside = [
            harness.events[0].points[0].clone(),
            harness.events[0].points[4].clone(),
        ];
        let (_, rect) = harness.render(vec![], EditTool::DrawTrace);
        let start = harness.view.point(rect, 2.0, 150.0);
        let middle = harness.view.point(rect, 5.0, 120.0);
        let end = harness.view.point(rect, 8.0, 90.0);
        harness.render(pointer_button(start, true), EditTool::DrawTrace);
        assert!(
            harness
                .render(vec![egui::Event::PointerMoved(middle)], EditTool::DrawTrace)
                .0
                .changed
        );
        assert!(
            harness
                .render(vec![egui::Event::PointerMoved(end)], EditTool::DrawTrace)
                .0
                .changed
        );
        harness.render(pointer_button(end, false), EditTool::DrawTrace);
        let event = &harness.events[0];
        assert!(event.points.len() > 20);
        assert_eq!(event.points.first(), Some(&outside[0]));
        assert_eq!(event.points.last(), Some(&outside[1]));
        assert!(
            event
                .points
                .windows(2)
                .all(|pair| pair[0].time_s < pair[1].time_s)
        );
        for point in event
            .points
            .iter()
            .filter(|point| point.time_s > 2.0 && point.time_s < 8.0)
        {
            assert!((point.frequency_hz - (170.0 - point.time_s * 10.0)).abs() < 1e-4);
        }
        assert_eq!(event.review, ReviewState::Pending);
        assert!(event.acoustic_source.is_unassessed());
        assert!(!event.selected_for_scoring);
        let before = event.points.len();
        let mut vertical = event.clone();
        draw_segment(&mut vertical, (5.0, 120.0), (5.0, 121.0), 20.0, 0.1).unwrap();
        assert!(vertical.points.len() <= before + 1);
        assert!(
            vertical
                .points
                .windows(2)
                .all(|pair| pair[0].time_s < pair[1].time_s)
        );
    }
    #[test]
    fn timeline_click_and_edge_drag_select_unloaded_recording_regions() {
        let ctx = egui::Context::default();
        let mut view = PlotView {
            time_min_s: 0.0,
            time_max_s: 10.0,
            ..view()
        };
        let mut cursor = 0.0;
        let mut render = |inputs: Vec<egui::Event>, view: &mut PlotView| {
            let mut result = TimelineInteraction::default();
            let mut rect = Rect::NOTHING;
            let _ = ctx.run_ui(
                egui::RawInput {
                    events: inputs,
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                    ..Default::default()
                },
                |ui| {
                    let start = ui.cursor().min;
                    rect = Rect::from_min_max(
                        start + Vec2::new(5.0, 4.0),
                        start + Vec2::new(ui.available_width() - 5.0, 46.0),
                    );
                    result = timeline(ui, 60.0, &[], Some((0.0, 10.0)), view, &mut cursor);
                },
            );
            (result, rect)
        };
        let (_, rect) = render(vec![], &mut view);
        let point = |time: f64| {
            Pos2::new(
                rect.left() + (time / 60.0) as f32 * rect.width(),
                rect.center().y,
            )
        };
        render(pointer_button(point(45.0), true), &mut view);
        assert!(render(pointer_button(point(45.0), false), &mut view).0.seek);
        assert!((view.time_min_s - 40.0).abs() < 1e-5);
        assert!((view.time_max_s - 50.0).abs() < 1e-5);
        render(pointer_button(point(40.0), true), &mut view);
        assert!(
            render(vec![egui::Event::PointerMoved(point(35.0))], &mut view)
                .0
                .changed
        );
        render(pointer_button(point(35.0), false), &mut view);
        assert!((view.time_min_s - 35.0).abs() < 1e-5);
        assert!((view.time_max_s - 50.0).abs() < 1e-5);
    }
}
