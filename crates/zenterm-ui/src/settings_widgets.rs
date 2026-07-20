//! Reusable egui widgets for the settings panel.
//!
//! Each function renders one "row" of the settings form: a label and
//! an interactive control.  The layout convention is:
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │  Label text                [ control ]  │
//! │  Description text (smaller, dimmer)     │
//! └─────────────────────────────────────────┘
//! ```
//!
//! The `description` is optional — pass an empty string to skip it.

use std::collections::HashSet;
use std::ops::RangeInclusive;

// ── Section header ─────────────────────────────────────────────────────

/// Draw a section title with an optional subtitle below it.
pub fn section_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.heading(title);
    });
    if !subtitle.is_empty() {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(subtitle)
                .size(ui.text_style_height(&egui::TextStyle::Body) * 0.85)
                .color(ui.visuals().weak_text_color.unwrap_or(egui::Color32::GRAY)),
        );
    }
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(6.0);
}

// ─── Label + control row helper ────────────────────────────────────────

/// Draw a label and description in the left column, then the control
/// in the right column, with consistent vertical spacing.
pub(crate) fn row<F>(ui: &mut egui::Ui, label: &str, description: &str, add_control: F)
where
    F: FnOnce(&mut egui::Ui),
{
    // Add a small gap between rows for visual breathing room.
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        // Label + description column (grow to fill space, pushing control right).
        ui.vertical(|ui| {
            ui.set_min_height(28.0);
            ui.label(egui::RichText::new(label).size(14.0));
            if !description.is_empty() {
                ui.label(
                    egui::RichText::new(description)
                        .size(ui.text_style_height(&egui::TextStyle::Body) * 0.82)
                        .color(ui.visuals().weak_text_color.unwrap_or(egui::Color32::GRAY)),
                );
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            add_control(ui);
        });
    });
}

// ── Boolean ─────────────────────────────────────────────────��──────────

/// A labelled checkbox.
pub fn bool_setting(ui: &mut egui::Ui, label: &str, value: &mut bool, description: &str) {
    row(ui, label, description, |ui| {
        ui.checkbox(value, "");
    });
}

// ── Float slider ───────────────────────────────────────────────────────

/// A labelled slider.
pub fn slider_setting(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    description: &str,
) {
    row(ui, label, description, |ui| {
        ui.add(egui::Slider::new(value, range).max_decimals(2));
    });
}

// ── Drag values ────────────────────────────────────────────────────────

/// A labelled drag-value for `f32`.
pub fn drag_f32(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    speed: f32,
    description: &str,
) {
    row(ui, label, description, |ui| {
        ui.add(egui::DragValue::new(value).speed(speed).max_decimals(2));
    });
}

/// A labelled drag-value for `u64`.
pub fn drag_u64(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u64,
    speed: f32,
    description: &str,
) {
    row(ui, label, description, |ui| {
        ui.add(egui::DragValue::new(value).speed(speed).max_decimals(0));
    });
}

// ── Text ───────────────────────────────────────────────────────────────

/// A labelled single-line text input.
pub fn text_setting(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    description: &str,
) {
    row(ui, label, description, |ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(value)
                .desired_width(180.0),
        );
        if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            resp.surrender_focus();
        }
    });
}

// ── Combo box ──────────────────────────────────────────────────────────

/// A labelled combo box (dropdown).  `variants` is a list of `(value, label)` pairs.
/// The control selects among the labels; the corresponding value is written to `current`.
pub fn combo_setting<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    label: &str,
    current: &mut T,
    variants: &[(T, &str)],
    description: &str,
) {
    row(ui, label, description, |ui| {
        let current_label = variants
            .iter()
            .find(|(v, _)| v == current)
            .map(|(_, l)| *l)
            .unwrap_or("<unknown>");
        egui::ComboBox::from_id_salt(label)
            .selected_text(current_label)
            .width(180.0)
            .show_ui(ui, |ui| {
                for (val, text) in variants {
                    if ui.selectable_label(val == current, *text).clicked() {
                        *current = *val;
                    }
                }
            });
    });
}

// ── Font family combo (with preview) ──────────────────────────────────

/// A labelled combo box for selecting a font family.
///
/// Each option whose name appears in `registered` is rendered with
/// [`RichText`] using the font's own typeface (requires the font to be
/// registered in egui's [`FontDefinitions`] via
/// [`crate::settings::register_preview_fonts`]).
///
/// If `current` is not in `families` the combo shows a "Custom: …" entry
/// and falls back to a text input so the user can type arbitrary names
/// (e.g. `"monospace"`).
pub fn font_combo_setting(
    ui: &mut egui::Ui,
    label: &str,
    current: &mut String,
    families: &[String],
    registered: &HashSet<String>,
    description: &str,
) {
    row(ui, label, description, |ui| {
        // Decide whether the current value is in the known list.
        let known = families.iter().any(|f| f == current);

        // Helper: build a RichText with the font's own typeface when
        // the font was successfully registered, or plain text otherwise.
        let rich_or_plain = |name: &str| -> egui::WidgetText {
            if registered.contains(name) {
                egui::RichText::new(name)
                    .font(egui::FontId::new(14.0, egui::FontFamily::Name(name.into())))
                    .into()
            } else {
                egui::RichText::new(name).into()
            }
        };

        // The closed-button label.
        let selected_text = if known {
            rich_or_plain(current)
        } else {
            egui::RichText::new(format!("Custom: {current}")).into()
        };

        egui::ComboBox::from_id_salt(label)
            .selected_text(selected_text)
            .width(180.0)
            .show_ui(ui, |ui| {
                for family in families {
                    let selected = *current == *family;
                    if ui
                        .selectable_label(selected, rich_or_plain(family))
                        .clicked()
                    {
                        *current = family.clone();
                    }
                }
            });

        // When the current value is not in the list, show a text field as
        // fallback so the user can still type arbitrary family names.
        if !known {
            let resp = ui.add(
                egui::TextEdit::singleline(current)
                    .desired_width(180.0)
                    .hint_text("e.g. monospace"),
            );
            if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                resp.surrender_focus();
            }
        }
    });
}

/// A labelled hex colour input (e.g. `"#rrggbb"`).
///
/// The caller provides a mutable `Option<String>` — `None` means "use
/// the theme default".
pub fn color_hex_setting(
    ui: &mut egui::Ui,
    label: &str,
    hex: &mut Option<String>,
    description: &str,
) {
    row(ui, label, description, |ui| {
        ui.horizontal(|ui| {
            // Parse current colour.
            let mut rgba = hex_to_rgba(hex.as_deref()).unwrap_or([1.0, 1.0, 1.0, 1.0]);

            // Colour edit button (opens popup picker on click).
            let resp = ui.color_edit_button_rgba_unmultiplied(&mut rgba);
            if resp.changed() {
                let r = (rgba[0] * 255.0) as u8;
                let g = (rgba[1] * 255.0) as u8;
                let b = (rgba[2] * 255.0) as u8;
                *hex = Some(format!("#{r:02x}{g:02x}{b:02x}"));
            }

            // Hex text field as an alternative.
            let mut hex_str = hex.clone().unwrap_or_default();
            let resp = ui.add(
                egui::TextEdit::singleline(&mut hex_str)
                    .desired_width(80.0)
                    .hint_text("#rrggbb"),
            );
            if resp.changed() {
                if hex_str.is_empty() {
                    *hex = None;
                } else if hex_str.starts_with('#') && (hex_str.len() == 7 || hex_str.len() == 9) {
                    *hex = Some(hex_str);
                }
            }
        });
    });
}

/// Parse a hex colour string `"#rrggbb"` to an `[f32; 4]` Rgba array.
fn hex_to_rgba(s: Option<&str>) -> Option<[f32; 4]> {
    let s = s?;
    if !s.starts_with('#') || s.len() != 7 {
        return None;
    }
    let r = u8::from_str_radix(&s[1..3], 16).ok()?;
    let g = u8::from_str_radix(&s[3..5], 16).ok()?;
    let b = u8::from_str_radix(&s[5..7], 16).ok()?;
    Some([
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        1.0,
    ])
}
