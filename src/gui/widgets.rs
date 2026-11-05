use eframe::egui::{self, Align2, Color32, FontId, Pos2, RichText, Sense, Stroke, Vec2};
use geosonnar::event_review::ReviewGroup;
use std::path::Path;

/// Human-readable Windows paths. Keep the original PathBuf for filesystem work:
/// verbatim paths may be necessary for long names or unusual filename characters.
pub fn display_path(path: &Path) -> String {
    display_path_text(&path.to_string_lossy())
}

/// Format path-bearing messages without exposing Windows' internal verbatim
/// prefix. Only drive and UNC prefixes have an ordinary equivalent; device and
/// volume namespaces deliberately retain their original spelling.
pub fn display_path_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(start) = remaining.find(r"\\?\") {
        result.push_str(&remaining[..start]);
        let suffix = &remaining[start + 4..];
        let drive = suffix
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && suffix.as_bytes().get(1) == Some(&b':')
            && suffix.as_bytes().get(2) == Some(&b'\\');
        let unc = suffix
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"UNC\"))
            && suffix[4..]
                .split_once('\\')
                .is_some_and(|(server, rest)| !server.is_empty() && !rest.is_empty());
        if drive {
            remaining = suffix;
        } else if unc {
            result.push_str(r"\\");
            remaining = &suffix[4..];
        } else {
            result.push_str(r"\\?\");
            remaining = suffix;
        }
    }
    result.push_str(remaining);
    result
}

pub const BG: Color32 = Color32::from_rgb(237, 238, 240);
pub const PANEL: Color32 = Color32::from_rgb(250, 250, 251);
pub const BORDER: Color32 = Color32::from_rgb(171, 176, 184);
pub const TEXT: Color32 = Color32::from_rgb(28, 32, 38);
pub const MUTED: Color32 = Color32::from_rgb(82, 89, 100);
pub const ACCENT: Color32 = Color32::from_rgb(25, 91, 170);
pub const AMBER: Color32 = Color32::from_rgb(154, 89, 0);
pub const RED: Color32 = Color32::from_rgb(177, 36, 43);
pub const PALETTE: [Color32; 5] = [
    ACCENT,
    AMBER,
    Color32::from_rgb(111, 67, 170),
    Color32::from_rgb(154, 45, 124),
    Color32::from_rgb(22, 115, 82),
];

pub fn theme(ctx: &egui::Context, dark: bool) {
    ctx.set_theme(if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    ctx.global_style_mut(|style| {
        style.visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, FontId::proportional(21.0));
        style.spacing.item_spacing = Vec2::new(7.0, 5.0);
        style.spacing.button_padding = Vec2::new(9.0, 5.0);
        style.spacing.interact_size.y = 27.0;
        style.wrap_mode = Some(egui::TextWrapMode::Wrap);
        if dark {
            style.visuals.override_text_color = Some(Color32::from_rgb(229, 232, 237));
            style.visuals.panel_fill = Color32::from_rgb(33, 36, 41);
            style.visuals.window_fill = Color32::from_rgb(43, 47, 53);
            style.visuals.extreme_bg_color = Color32::from_rgb(23, 26, 30);
            style.visuals.faint_bg_color = Color32::from_rgb(48, 52, 59);
            style.visuals.selection.bg_fill = Color32::from_rgb(45, 78, 115);
            style.visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(155, 201, 255));
        } else {
            style.visuals.override_text_color = Some(TEXT);
            style.visuals.panel_fill = BG;
            style.visuals.window_fill = PANEL;
            style.visuals.extreme_bg_color = Color32::WHITE;
            style.visuals.faint_bg_color = PANEL;
            style.visuals.selection.bg_fill = Color32::from_rgb(203, 222, 245);
            style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
            style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
            style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(229, 231, 235);
            style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(212, 224, 239);
            style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(195, 215, 239);
        }
    });
}

pub fn card<R>(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(3)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            content(ui)
        })
        .inner
}

pub fn primary(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(Color32::WHITE).strong()).fill(ACCENT))
}

/// Edit a byte-domain allowance in GiB without rounding an unchanged saved value.
/// The range remains in exact bytes, including non-power-of-two legacy limits.
pub fn data_size_input(
    ui: &mut egui::Ui,
    bytes: &mut u64,
    range: std::ops::RangeInclusive<u64>,
) -> egui::Response {
    use geosonnar::data_size::{bytes_to_gib, format_gib, gib_to_bytes};
    let original = *bytes;
    let initial = bytes_to_gib(original);
    let mut gib = initial;
    let response = ui.add(
        egui::DragValue::new(&mut gib)
            .range(bytes_to_gib(*range.start())..=bytes_to_gib(*range.end()))
            .clamp_existing_to_range(false)
            .speed(0.001)
            .min_decimals(3)
            .max_decimals(10)
            .custom_formatter(move |value, _| {
                let value = if value.to_bits() == initial.to_bits() {
                    Ok(original)
                } else {
                    gib_to_bytes(value)
                };
                value.map_or_else(
                    |_| "Invalid".into(),
                    |value| format_gib(value).trim_end_matches(" GiB").to_owned(),
                )
            })
            .suffix(" GiB"),
    );
    if response.changed() {
        match gib_to_bytes(gib) {
            Ok(value) if range.contains(&value) => *bytes = value,
            Ok(_) => {
                ui.colored_label(
                    tone(ui, RED),
                    format!(
                        "Enter {} to {}",
                        format_gib(*range.start()),
                        format_gib(*range.end())
                    ),
                );
            }
            Err(error) => {
                ui.colored_label(tone(ui, RED), error);
            }
        }
    }
    response
}

const TEXT_CHARACTER_LIMIT: usize = 32_768;

/// A normal egui text editor with the SetValue action promised to assistive
/// technology. egui 0.35 handles keyboard input and text selection itself, but
/// does not apply AccessKit SetValue to TextEdit's backing string.
pub fn accessible_text(
    ui: &mut egui::Ui,
    id_salt: &str,
    value: &mut String,
    hint: &str,
    multiline: bool,
    width: f32,
) -> egui::Response {
    use egui::{
        accesskit::{Action, ActionData},
        text::{CCursor, CCursorRange},
    };

    let id = ui.make_persistent_id(id_salt);
    let mut replacement = None;
    if ui.is_enabled() {
        // This egui API checks both the exact node ID and the root tree ID.
        // Leave focus, selection and requests for other fields untouched.
        ui.input_mut(|input| {
            input.consume_accesskit_action_requests(id, |request| {
                if request.action == Action::SetValue
                    && let Some(ActionData::Value(text)) = &request.data
                {
                    replacement = Some(
                        text.chars()
                            .take(TEXT_CHARACTER_LIMIT)
                            .map(|c| {
                                if !multiline && matches!(c, '\r' | '\n') {
                                    ' '
                                } else {
                                    c
                                }
                            })
                            .collect::<String>(),
                    );
                    true
                } else {
                    false
                }
            });
        });
    }
    let mut previous = None;
    if let Some(replacement) = replacement
        && replacement != *value
    {
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        let old_cursor = state
            .cursor
            .char_range()
            .unwrap_or_else(|| CCursorRange::one(CCursor::new(value.chars().count())));
        let mut undoer = state.undoer();
        undoer.add_undo(&(old_cursor, value.clone()));
        previous = Some(std::mem::replace(value, replacement));
        let cursor = CCursorRange::one(CCursor::new(value.chars().count()));
        state.cursor.set_char_range(Some(cursor));
        // An accessible replacement is one undoable edit, with the same redo
        // invalidation as a fresh keyboard edit. Preserve earlier undo history.
        undoer.feed_state(ui.input(|input| input.time), &(cursor, value.clone()));
        undoer.add_undo(&(cursor, value.clone()));
        state.set_undoer(undoer);
        state.store(ui.ctx(), id);
    }
    let editor = if multiline {
        egui::TextEdit::multiline(value).desired_rows(2)
    } else {
        egui::TextEdit::singleline(value)
    };
    let mut response = editor
        .id(id)
        .hint_text(hint)
        .desired_width(width.min(ui.available_width()).max(0.0))
        .char_limit(TEXT_CHARACTER_LIMIT)
        .show(ui)
        .response
        .response;
    if let Some(previous) = previous {
        response.mark_changed();
        response
            .widget_info(|| egui::WidgetInfo::text_edit(ui.is_enabled(), &previous, &*value, hint));
    }
    ui.ctx().accesskit_node_builder(id, |node| {
        // widget_info uses the generic single-line role; retain multiline
        // semantics and advertise the action only on an enabled editor.
        node.set_role(if multiline {
            egui::accesskit::Role::MultilineTextInput
        } else {
            egui::accesskit::Role::TextInput
        });
        if ui.is_enabled() {
            node.add_action(Action::SetValue);
        }
    });
    response
}

/// Edit a familiar-looking path without rewriting an unchanged canonical IO
/// binding. Only an actual text edit replaces the stored path string.
pub fn accessible_path(
    ui: &mut egui::Ui,
    id_salt: &str,
    value: &mut String,
    hint: &str,
    width: f32,
) -> egui::Response {
    let mut visible = display_path_text(value);
    let before = visible.clone();
    let response = accessible_text(ui, id_salt, &mut visible, hint, false, width);
    if response.changed() && visible != before {
        *value = visible;
    }
    response
}

pub fn muted(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(RichText::new(text).color(tone(ui, MUTED)));
}

pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    let color = tone(ui, color);
    egui::Frame::new()
        .fill(color.gamma_multiply(0.13))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(color).strong());
        });
}

/// Keep semantic text and small indicators readable on both panel palettes.
/// Plot canvases have their own fixed palette and do not use this mapping.
pub fn tone(ui: &egui::Ui, color: Color32) -> Color32 {
    if !ui.visuals().dark_mode {
        return color;
    }
    match color {
        TEXT => Color32::from_rgb(229, 232, 237),
        MUTED => Color32::from_rgb(175, 183, 195),
        ACCENT => Color32::from_rgb(137, 191, 255),
        AMBER => Color32::from_rgb(242, 187, 91),
        RED => Color32::from_rgb(255, 140, 145),
        _ => color,
    }
}

pub fn sun_toggle(ui: &mut egui::Ui, dark: bool) -> egui::Response {
    let label = if dark {
        "Switch to light theme"
    } else {
        "Switch to dark theme"
    };
    let response = ui.add(egui::Button::new("").min_size(Vec2::new(28.0, 26.0)));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let center = response.rect.center();
    let stroke = Stroke::new(1.5, ui.visuals().text_color());
    ui.painter().circle_stroke(center, 4.0, stroke);
    for i in 0..8 {
        let direction = Vec2::angled(i as f32 * std::f32::consts::FRAC_PI_4);
        ui.painter()
            .line_segment([center + direction * 6.0, center + direction * 9.0], stroke);
    }
    response.on_hover_text(label)
}

pub fn seconds(value: f64) -> String {
    let total = value.max(0.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        total / 60 % 60,
        total % 60
    )
}

/// Draw measured segments only. A missing span is never joined by the painter.
pub fn ridge_plot(ui: &mut egui::Ui, group: &ReviewGroup) {
    let (response, painter) =
        ui.allocate_painter(Vec2::new(ui.available_width(), 265.0), Sense::hover());
    let rect = response.rect;
    let plot = egui::Rect::from_min_max(
        rect.min + Vec2::new(58.0, 20.0),
        rect.max - Vec2::new(18.0, 40.0),
    );
    let low = group
        .ridges
        .iter()
        .flat_map(|r| &r.points)
        .map(|p| p.frequency_hz)
        .fold(f64::INFINITY, f64::min);
    let high = group
        .ridges
        .iter()
        .flat_map(|r| &r.points)
        .map(|p| p.frequency_hz)
        .fold(f64::NEG_INFINITY, f64::max);
    if !low.is_finite() || !high.is_finite() {
        return;
    }
    let pad = ((high - low) * 0.12).max(2.0);
    let (lo, hi) = (low - pad, high + pad);
    let start = group.start_s;
    let span = (group.end_s - start).max(0.001);
    let point = |t: f64, f: f64| {
        Pos2::new(
            plot.left() + ((t - start) / span) as f32 * plot.width(),
            plot.bottom() - ((f - lo) / (hi - lo)) as f32 * plot.height(),
        )
    };
    for i in 0..=4 {
        let f = lo + (hi - lo) * f64::from(i) / 4.0;
        let y = point(start, f).y;
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(1.0, BORDER),
        );
        painter.text(
            Pos2::new(plot.left() - 8.0, y),
            Align2::RIGHT_CENTER,
            format!("{f:.0} Hz"),
            FontId::proportional(12.0),
            MUTED,
        );
        let t = start + span * f64::from(i) / 4.0;
        painter.text(
            Pos2::new(point(t, lo).x, plot.bottom() + 15.0),
            Align2::CENTER_TOP,
            format!("{t:.1}s"),
            FontId::proportional(12.0),
            MUTED,
        );
    }
    for (i, ridge) in group.ridges.iter().enumerate() {
        let color = PALETTE[i % PALETTE.len()];
        for segment in &ridge.segments {
            let values: Vec<_> = ridge.points[segment.start_index..segment.end_index_exclusive]
                .iter()
                .map(|p| point(p.time_s, p.frequency_hz))
                .collect();
            for pair in values.windows(2) {
                painter.line_segment([pair[0], pair[1]], Stroke::new(1.8, color));
            }
            for p in values {
                painter.circle_filled(p, 2.0, color);
            }
        }
    }
    if let Some(pos) = response.hover_pos().filter(|p| plot.contains(*p)) {
        let t = start + f64::from((pos.x - plot.left()) / plot.width()) * span;
        let f = lo + f64::from((plot.bottom() - pos.y) / plot.height()) * (hi - lo);
        response.on_hover_text(format!("{t:.2} s into recording · {f:.2} Hz\nDots are measurements. Breaks are missing observations."));
    }
}

pub fn icon() -> egui::IconData {
    let mut rgba = vec![0; 64 * 64 * 4];
    for y in 0..64 {
        for x in 0..64 {
            let dx = x as f32 - 31.5;
            let dy = y as f32 - 31.5;
            let distance = (dx * dx + dy * dy).sqrt();
            let ring = [12.0, 22.0].iter().any(|r| (distance - r).abs() < 1.4);
            let signal = (dy - (dx * 0.28).sin() * 8.0).abs() < 2.0 && dx.abs() < 25.0;
            let c = if ring || signal { ACCENT } else { BG };
            let i = (y * 64 + x) * 4;
            rgba[i..i + 4].copy_from_slice(&c.to_array());
        }
    }
    egui::IconData {
        rgba,
        width: 64,
        height: 64,
    }
}

#[cfg(test)]
mod accessible_text_tests {
    use super::*;
    use egui::accesskit::{Action, ActionData, ActionRequest, TreeId};

    fn request(id: egui::Id, value: &str) -> egui::Event {
        egui::Event::AccessKitActionRequest(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: id.accesskit_id(),
            data: Some(ActionData::Value(value.into())),
        })
    }

    fn frame(
        ctx: &egui::Context,
        values: &mut [String; 2],
        events: Vec<egui::Event>,
        enabled: bool,
        multiline: bool,
    ) -> (egui::FullOutput, [egui::Response; 2]) {
        let mut responses = None;
        let output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    let first = accessible_text(
                        ui,
                        "first",
                        &mut values[0],
                        "First field",
                        multiline,
                        300.,
                    );
                    let second =
                        accessible_text(ui, "second", &mut values[1], "Second field", false, 300.);
                    responses = Some([first, second]);
                });
            },
        );
        (output, responses.unwrap())
    }

    #[test]
    fn exact_target_updates_backing_text_rendered_text_and_accessibility_value() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut values = ["original".into(), "other".into()];
        let (_, first_frame) = frame(&ctx, &mut values, vec![], true, false);
        let id = first_frame[0].id;
        let (output, responses) = frame(
            &ctx,
            &mut values,
            vec![request(id, "Québec 🦉 東京")],
            true,
            false,
        );
        assert_eq!(values, ["Québec 🦉 東京", "other"]);
        assert_eq!(responses[0].id, id);
        assert!(responses[0].changed());
        assert!(!responses[1].changed());
        let update = output.platform_output.accesskit_update.unwrap();
        let node = &update
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id.accesskit_id())
            .unwrap()
            .1;
        assert_eq!(node.value(), Some(values[0].as_str()));
        assert!(node.supports_action(Action::SetValue));
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::epaint::Shape::Text(text) if text.galley.text() == values[0])));
        let cursor = egui::TextEdit::load_state(&ctx, id)
            .unwrap()
            .cursor
            .char_range()
            .unwrap();
        assert_eq!(cursor.primary.index.0, values[0].chars().count());
        let (_, unchanged) = frame(&ctx, &mut values, vec![], true, false);
        assert!(!unchanged[0].changed());
    }

    #[test]
    fn disabled_wrong_tree_wrong_target_and_wrong_payload_cannot_edit() {
        let ctx = egui::Context::default();
        let mut values = ["first".into(), "second".into()];
        let (_, initial) = frame(&ctx, &mut values, vec![], true, false);
        let id = initial[0].id;
        let (_, disabled) = frame(
            &ctx,
            &mut values,
            vec![request(id, "forbidden")],
            false,
            false,
        );
        assert_eq!(values[0], "first");
        assert!(!disabled[0].changed());
        let mut foreign = match request(id, "foreign") {
            egui::Event::AccessKitActionRequest(request) => request,
            _ => unreachable!(),
        };
        foreign.target_tree = TreeId(egui::accesskit::Uuid::from_u128(1));
        let mut numeric = foreign.clone();
        numeric.target_tree = TreeId::ROOT;
        numeric.data = Some(ActionData::NumericValue(42.));
        let (_, responses) = frame(
            &ctx,
            &mut values,
            vec![
                egui::Event::AccessKitActionRequest(foreign),
                egui::Event::AccessKitActionRequest(numeric),
                request(egui::Id::new("not-a-field"), "wrong"),
                request(initial[1].id, "second updated"),
            ],
            true,
            false,
        );
        assert_eq!(values, ["first", "second updated"]);
        assert!(!responses[0].changed());
        assert!(responses[1].changed());
    }

    #[test]
    fn newline_policy_unicode_character_limit_and_multiline_role_match_editor() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut values = [String::new(), String::new()];
        let (_, initial) = frame(&ctx, &mut values, vec![], true, false);
        frame(
            &ctx,
            &mut values,
            vec![request(initial[0].id, "a\r\nb\nc")],
            true,
            false,
        );
        assert_eq!(values[0], "a  b c");
        let (output, _) = frame(
            &ctx,
            &mut values,
            vec![request(initial[0].id, "a\r\nb\nc")],
            true,
            true,
        );
        assert_eq!(values[0], "a\r\nb\nc");
        let update = output.platform_output.accesskit_update.unwrap();
        let node = &update
            .nodes
            .iter()
            .find(|(id, _)| *id == initial[0].id.accesskit_id())
            .unwrap()
            .1;
        assert_eq!(node.role(), egui::accesskit::Role::MultilineTextInput);
        frame(
            &ctx,
            &mut values,
            vec![request(
                initial[0].id,
                &"🦉".repeat(TEXT_CHARACTER_LIMIT + 1),
            )],
            true,
            false,
        );
        assert_eq!(values[0].chars().count(), TEXT_CHARACTER_LIMIT);
        assert!(values[0].chars().all(|c| c == '🦉'));
    }

    #[test]
    fn native_typing_and_undo_redo_work_after_accessible_replacement() {
        let ctx = egui::Context::default();
        let mut values = ["original".into(), "other".into()];
        let (_, initial) = frame(&ctx, &mut values, vec![], true, false);
        let id = initial[0].id;
        ctx.memory_mut(|memory| memory.request_focus(id));
        frame(&ctx, &mut values, vec![request(id, "é🦉")], true, false);
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        frame(&ctx, &mut values, vec![key(egui::Key::Z)], true, false);
        assert_eq!(values[0], "original");
        frame(&ctx, &mut values, vec![key(egui::Key::Y)], true, false);
        assert_eq!(values[0], "é🦉");
        frame(
            &ctx,
            &mut values,
            vec![egui::Event::Text("g".into())],
            true,
            false,
        );
        assert_eq!(values[0], "é🦉g");
        assert_eq!(values[1], "other");
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::*;
    fn luminance(color: Color32) -> f64 {
        let channel = |n: u8| {
            let c = f64::from(n) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }
    fn contrast(a: Color32, b: Color32) -> f64 {
        let a = luminance(a);
        let b = luminance(b);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    #[test]
    fn theme_round_trip_keeps_panel_body_and_semantic_text_readable() {
        let ctx = egui::Context::default();
        for dark in [false, true, false] {
            theme(&ctx, dark);
            assert_eq!(ctx.global_style().visuals.dark_mode, dark);
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    for background in [ui.visuals().panel_fill, ui.visuals().window_fill] {
                        for color in [TEXT, MUTED, ACCENT, AMBER, RED] {
                            assert!(
                                contrast(tone(ui, color), background) >= 4.5,
                                "dark={dark}, color={color:?}"
                            );
                        }
                    }
                });
            });
        }
    }
}

#[cfg(test)]
mod data_size_tests {
    use super::*;
    use geosonnar::data_size::{BYTES_PER_GIB, bytes_to_gib};

    fn frame(
        ctx: &egui::Context,
        bytes: &mut u64,
        range: std::ops::RangeInclusive<u64>,
        events: Vec<egui::Event>,
    ) -> egui::Response {
        let mut response = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                response = Some(data_size_input(ui, bytes, range.clone()));
            },
        );
        response.unwrap()
    }

    fn value_event(id: egui::Id, value: f64) -> egui::Event {
        use egui::accesskit::{Action, ActionData, ActionRequest, TreeId};
        egui::Event::AccessKitActionRequest(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: id.accesskit_id(),
            data: Some(ActionData::NumericValue(value)),
        })
    }

    #[test]
    fn gib_input_idle_preserves_exact_bytes_and_legacy_out_of_range_values() {
        for original in [0, 1, 1_000_000, 6_000_000_001, (1u64 << 53) + 1, u64::MAX] {
            let ctx = egui::Context::default();
            let mut bytes = original;
            for _ in 0..3 {
                let response = frame(&ctx, &mut bytes, 1_000_000..=100_000_000_000, vec![]);
                assert!(!response.changed());
                assert_eq!(bytes, original);
            }
        }
    }

    #[test]
    fn gib_input_decimal_edits_commit_bytes_and_keep_original_domain_boundaries() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut bytes = 6_000_000_000;
        let range = 1_000_000..=100_000_000_000;
        let id = frame(&ctx, &mut bytes, range.clone(), vec![]).id;
        for expected in [
            BYTES_PER_GIB + BYTES_PER_GIB / 2,
            1_000_000,
            100_000_000_000,
        ] {
            let response = frame(
                &ctx,
                &mut bytes,
                range.clone(),
                vec![value_event(id, bytes_to_gib(expected))],
            );
            assert!(response.changed());
            assert_eq!(bytes, expected);
            assert!(!frame(&ctx, &mut bytes, range.clone(), vec![]).changed());
            assert_eq!(bytes, expected);
        }
    }

    #[test]
    fn gib_input_rejects_invalid_accesskit_values_without_saturating_bytes() {
        for invalid in [
            f64::NAN,
            f64::INFINITY,
            -1.0,
            bytes_to_gib(999_999),
            bytes_to_gib(100_000_000_001),
        ] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let mut bytes = 6_000_000_000;
            let range = 1_000_000..=100_000_000_000;
            let id = frame(&ctx, &mut bytes, range.clone(), vec![]).id;
            frame(&ctx, &mut bytes, range, vec![value_event(id, invalid)]);
            assert_eq!(bytes, 6_000_000_000, "invalid input {invalid}");
        }
    }
}

#[cfg(test)]
mod path_and_layout_tests {
    use super::*;

    #[test]
    fn readable_paths_preserve_unc_devices_and_underlying_bindings() {
        let original = std::path::PathBuf::from(r"\\?\C:\cases\Québec\clip.mp4");
        let original_binding = original.clone();
        assert_eq!(display_path(&original), r"C:\cases\Québec\clip.mp4");
        assert_eq!(original, original_binding);
        assert_eq!(
            display_path_text(r"\\?\UNC\server\share\clip.mp4"),
            r"\\server\share\clip.mp4"
        );
        assert_eq!(
            display_path_text(r"\\?\unc\server\share\clip.mp4"),
            r"\\server\share\clip.mp4"
        );
        for unchanged in [
            r"\\server\share\clip.mp4",
            r"C:\cases\clip.mp4",
            r"\\?\Volume{123}\clip.mp4",
            r"\\.\PhysicalDrive0",
            r"\\?\UNC\server",
            "https://example.org/a?b=1",
            "Quel fichier ?",
        ] {
            assert_eq!(display_path_text(unchanged), unchanged);
        }
        assert_eq!(
            display_path_text(r"Cannot copy \\?\C:\one.wav to \\?\UNC\server\share\two.wav (é)."),
            r"Cannot copy C:\one.wav to \\server\share\two.wav (é)."
        );
    }

    #[test]
    fn path_field_preserves_canonical_binding_until_user_edits() {
        use egui::accesskit::{Action, ActionData, ActionRequest, TreeId};
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let canonical = r"\\?\C:\cases\clip.mp4";
        let mut value = canonical.to_owned();
        let mut field_id = None;
        for _ in 0..2 {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                field_id = Some(accessible_path(ui, "path", &mut value, "Recording", 300.).id);
            });
            assert_eq!(value, canonical);
        }
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::AccessKitActionRequest(ActionRequest {
                    action: Action::SetValue,
                    target_tree: TreeId::ROOT,
                    target_node: field_id.unwrap().accesskit_id(),
                    data: Some(ActionData::Value(r"D:\recordings\new.mp4".into())),
                })],
                ..Default::default()
            },
            |ui| {
                let response = accessible_path(ui, "path", &mut value, "Recording", 300.);
                assert!(response.changed());
            },
        );
        assert_eq!(value, r"D:\recordings\new.mp4");
    }

    #[test]
    fn cards_and_path_fields_shrink_with_the_window() {
        let ctx = egui::Context::default();
        theme(&ctx, false);
        let path = format!(r"C:\{}\clip.mp4", "long-folder-name-".repeat(60));
        let mut value = path.clone();
        for width in [1200., 760., 940., 760.] {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 640.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let right = ui.max_rect().right();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            card(ui, |ui| {
                                let label = ui.label(&path);
                                let input =
                                    accessible_path(ui, "resized_path", &mut value, "Path", 1200.);
                                assert!(
                                    label.rect.right() <= right + 1.0,
                                    "label overflow at {width}"
                                );
                                assert!(
                                    input.rect.right() <= right + 1.0,
                                    "input overflow at {width}"
                                );
                            });
                        });
                    });
                },
            );
        }
    }
}
