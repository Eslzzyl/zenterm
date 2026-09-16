//! Settings panel — editing all [`Config`](zenterm_config::Config) fields
//! in a native OS window.
//!
//! # Behaviour
//!
//! - Every field change is **applied immediately** to the running program.
//! - Settings that require a restart are marked inline with a hint.
//! - "Reset All" in the nav sidebar resets to defaults, applies, and saves.
//! - No explicit Apply/Save buttons — direct manipulation.

use std::collections::HashSet;

use zenterm_config::Config;
use zenterm_config::background::{BackgroundConfig, ImageFitMode};
use zenterm_config::colors::{
    AnsiColors, ColorTheme, ColorsConfig, CursorColors, PrimaryColors, SelectionColors,
    ThemePreference,
};
use zenterm_config::cursor::{Blinking, CursorConfig, CursorShape};
use zenterm_config::font::{FontConfig, FontDescription};
use zenterm_config::selection::SelectionConfig;
use zenterm_config::ui::{SidebarPosition, UiConfig};
use zenterm_config::window::WindowConfig;
use zenterm_core::color::Rgba;
use zenterm_core::{HintingMode, RenderMode};

use crate::settings_widgets;

// ── Section selector ─────────────────────────────────────────────────────

/// Top-level config sections that the user can navigate to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Window,
    Font,
    Colors,
    Cursor,
    Selection,
    Ui,
    Background,
}

impl SettingsSection {
    /// All sections, in display order.
    pub const ALL: &'static [Self] = &[
        Self::Window,
        Self::Font,
        Self::Colors,
        Self::Cursor,
        Self::Selection,
        Self::Ui,
        Self::Background,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Window => "Window",
            Self::Font => "Font",
            Self::Colors => "Colors",
            Self::Cursor => "Cursor",
            Self::Selection => "Selection",
            Self::Ui => "UI",
            Self::Background => "Background",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Window => crate::icons::APP_WINDOW,
            Self::Font => crate::icons::TEXT_T,
            Self::Colors => crate::icons::PALETTE,
            Self::Cursor => crate::icons::CURSOR,
            Self::Selection => crate::icons::SELECTION,
            Self::Ui => crate::icons::SLIDERS_HORIZONTAL,
            Self::Background => crate::icons::IMAGE,
        }
    }
}

// ── Render result ───────────────────────────────────────────────────────

/// What happened inside the settings panel this frame.
#[derive(Debug, Default)]
pub struct SettingsOutput {
    /// `true` when the user confirmed "Reset All to defaults".
    /// The caller should reset config, apply it, and save to disk.
    pub reset_all_confirmed: bool,
}

// ── SettingsState ────────────────────────────────────────────────────────

/// Mutable state for the settings panel.
pub struct SettingsState {
    /// Whether the panel is currently visible.
    pub open: bool,
    /// The config being edited in the panel.
    pub working_config: Config,
    /// Which section is selected in the navigation sidebar.
    pub selected_section: SettingsSection,
    /// When `true`, a confirmation dialog for "Reset All" is shown.
    pub pending_reset_confirm: bool,
    /// Cached list of all monospace font families on this system.
    /// Populated once at construction by reading the font database.
    pub font_families: Vec<String>,
    /// Whether the preview fonts have been registered in egui's
    /// [`FontDefinitions`] via [`register_preview_fonts`].
    pub fonts_registered: bool,
    /// Subset of [`font_families`](Self::font_families) for which
    /// [`register_preview_fonts`] succeeded.  Every name in this set
    /// has valid font data that egui can safely use with RichText.
    pub registered_fonts: HashSet<String>,
}

impl SettingsState {
    /// Create a new settings state from the app's current config.
    ///
    /// Enumerates system monospace fonts (one-time disk scan, ≈9 ms).
    pub fn new(config: &Config) -> Self {
        let font_families = zenterm_glyph::font_list::list_monospace_families();
        Self {
            open: false,
            working_config: config.clone(),
            selected_section: SettingsSection::Window,
            pending_reset_confirm: false,
            font_families,
            fonts_registered: false,
            registered_fonts: HashSet::new(),
        }
    }

    /// Reset the working config to match a fresh config.
    pub fn reset_to(&mut self, config: &Config) {
        self.working_config = config.clone();
    }

    /// Returns `true` if the working config differs from `other`.
    pub fn is_dirty(&self, other: &Config) -> bool {
        self.working_config != *other
    }
}

// ── Render entry point (native viewport mode) ─────────────────────────

/// Render the settings panel inside a **native OS window**.
///
/// Returns [`SettingsOutput`] describing actions the caller should take.
pub fn render_settings_viewport(
    ctx: &egui::Context,
    state: &mut SettingsState,
    current_config: &Config,
) -> SettingsOutput {
    let mut output = SettingsOutput::default();
    #[allow(deprecated)]
    egui::CentralPanel::default().show(ctx, |ui| {
        render_settings_content(ui, state, current_config, &mut output);
    });
    output
}

// ── Shared content rendering ──────────────────────────────────────────

/// Draw the settings form inside an existing [`egui::Ui`].
fn render_settings_content(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    current_config: &Config,
    output: &mut SettingsOutput,
) {
    ui.horizontal_top(|ui| {
        // ── Left: navigation sidebar ───────────────────────────
        egui::Panel::left("settings_nav")
            .resizable(false)
            .default_size(170.0)
            .min_size(140.0)
            .show_inside(ui, |ui| {
                ui.vertical(|ui| {
                    ui.add_space(10.0);

                    for sec in SettingsSection::ALL {
                        let is_selected = *sec == state.selected_section;
                        let label = sec.label();
                        let icon = sec.icon();

                        let item_size = egui::vec2(ui.available_width(), 34.0);
                        let (rect, resp) = ui.allocate_exact_size(item_size, egui::Sense::click());
                        if resp.clicked() {
                            state.selected_section = *sec;
                        }

                        let is_hovered = resp.hovered();

                        // VS Code-style flat navigation:
                        // Clear high-contrast background and left accent bar for active item
                        if is_selected {
                            let fill = if ui.visuals().dark_mode {
                                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 20)
                            } else {
                                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 14)
                            };
                            ui.painter().rect_filled(rect, 5.0, fill);

                            // Left accent indicator bar
                            let indicator_rect = egui::Rect::from_min_size(
                                rect.left_top(),
                                egui::vec2(3.5, rect.height()),
                            );
                            let accent_color = ui.visuals().hyperlink_color;
                            ui.painter().rect_filled(indicator_rect, 1.75, accent_color);
                        } else if is_hovered {
                            let fill = if ui.visuals().dark_mode {
                                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 10)
                            } else {
                                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 8)
                            };
                            ui.painter().rect_filled(rect, 5.0, fill);
                        }

                        // Text & icon colors — ensure high contrast readability
                        let (text_color, icon_color) = if is_selected {
                            (ui.visuals().strong_text_color(), ui.visuals().hyperlink_color)
                        } else if is_hovered {
                            (ui.visuals().strong_text_color(), ui.visuals().strong_text_color())
                        } else {
                            (ui.visuals().text_color(), ui.visuals().text_color())
                        };

                        // Icon
                        let icon_font = egui::FontId::proportional(15.0);
                        let icon_pos = egui::pos2(rect.left() + 12.0, rect.center().y);
                        ui.painter().text(
                            icon_pos,
                            egui::Align2::LEFT_CENTER,
                            icon,
                            icon_font,
                            icon_color,
                        );

                        // Text label
                        let label_font = egui::FontId::proportional(13.5);
                        let label_pos = egui::pos2(rect.left() + 35.0, rect.center().y);
                        ui.painter().text(
                            label_pos,
                            egui::Align2::LEFT_CENTER,
                            label,
                            label_font,
                            text_color,
                        );

                        ui.add_space(2.0);
                    }

                    // ── Push "Reset All" to the bottom ────────
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        let btn_size = egui::vec2(ui.available_width() - 8.0, 30.0);
                        let (btn_rect, btn_resp) = ui.allocate_exact_size(btn_size, egui::Sense::click());
                        if btn_resp.clicked() {
                            state.pending_reset_confirm = true;
                        }

                        let is_hover = btn_resp.hovered();
                        let bg_fill = if is_hover {
                            ui.visuals().error_fg_color.gamma_multiply(0.15)
                        } else {
                            ui.visuals().faint_bg_color
                        };
                        let stroke = if is_hover {
                            egui::Stroke::new(1.0_f32, ui.visuals().error_fg_color.gamma_multiply(0.6))
                        } else {
                            egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color)
                        };
                        ui.painter().rect(btn_rect, 5.0, bg_fill, stroke, egui::StrokeKind::Inside);

                        let text_color = if is_hover {
                            ui.visuals().error_fg_color
                        } else {
                            ui.visuals().text_color()
                        };

                        let content = format!("{} Reset All", crate::icons::ARROW_COUNTER_CLOCKWISE);
                        let galley = ui.painter().layout_no_wrap(
                            content,
                            egui::FontId::proportional(12.5),
                            text_color,
                        );
                        let text_pos = btn_rect.center() - galley.size() * 0.5;
                        ui.painter().galley(text_pos, galley, egui::Color32::WHITE);
                    });
                });
            });

        // ── Right: content area (scrollable) ───────────────────
        egui::CentralPanel::default().show_inside(ui, |ui| {
            // Show a restart-required banner at top of content
            // if any changed field needs a restart.
            let changes = current_config.diff_to(&state.working_config);
            if changes.needs_restart {
                ui.label(
                    egui::RichText::new(
                        "⚠ Some changes require an application restart to take full effect",
                    )
                    .color(ui.visuals().warn_fg_color),
                );
                ui.add_space(6.0);
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                // Add comfortable top padding inside the scroll content.
                ui.add_space(4.0);
                render_section(
                    ui,
                    state.selected_section,
                    &mut state.working_config,
                    &state.font_families,
                    &state.registered_fonts,
                );
            });
        });
    });

    // ── Reset All confirmation dialog ──────────────────────────
    if state.pending_reset_confirm {
        egui::Window::new("Reset All Settings")
            .id(egui::Id::new("reset_all_confirm"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label("Reset all settings to their default values?");
                ui.label("This will save and apply immediately.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        state.pending_reset_confirm = false;
                    }
                    if ui.button("Reset && Save").clicked() {
                        state.pending_reset_confirm = false;
                        output.reset_all_confirmed = true;
                    }
                });
            });
    }
}

// ── Per-section render dispatcher ────────────────────────────────────────

fn render_section(
    ui: &mut egui::Ui,
    section: SettingsSection,
    cfg: &mut Config,
    font_families: &[String],
    registered_fonts: &HashSet<String>,
) {
    match section {
        SettingsSection::Window => render_window_section(ui, &mut cfg.window),
        SettingsSection::Font => {
            render_font_section(ui, &mut cfg.font, font_families, registered_fonts)
        }
        SettingsSection::Colors => render_colors_section(ui, &mut cfg.colors),
        SettingsSection::Cursor => render_cursor_section(ui, &mut cfg.cursor),
        SettingsSection::Selection => render_selection_section(ui, &mut cfg.selection),
        SettingsSection::Ui => render_ui_section(ui, &mut cfg.ui),
        SettingsSection::Background => render_background_section(ui, &mut cfg.background),
    }
}

// ── Window section ───────────────────────────────────────────────────────

fn render_window_section(ui: &mut egui::Ui, w: &mut WindowConfig) {
    settings_widgets::section_header(ui, "Window", "Window appearance, size, and initial state.");
    settings_widgets::drag_f32(
        ui,
        "Padding X",
        &mut w.padding.x,
        0.5,
        "Horizontal padding in logical pixels",
    );
    settings_widgets::drag_f32(
        ui,
        "Padding Y",
        &mut w.padding.y,
        0.5,
        "Vertical padding in logical pixels",
    );
    settings_widgets::text_setting(ui, "Title", &mut w.title, "Window title");
    settings_widgets::bool_setting(
        ui,
        "Decorations",
        &mut w.decorations,
        "Show window title bar and borders (requires restart)",
    );
}

// ── Font section ─────────────────────────────────────────────────────────

fn render_font_section(
    ui: &mut egui::Ui,
    f: &mut FontConfig,
    font_families: &[String],
    registered_fonts: &HashSet<String>,
) {
    settings_widgets::section_header(ui, "Font", "Terminal typeface and rendering.");
    settings_widgets::drag_f32(
        ui,
        "Size",
        &mut f.size,
        0.5,
        "Font size in logical pixels at 1× DPI",
    );
    render_font_description(ui, "Normal", &mut f.normal, font_families, registered_fonts);

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Features", "");
    settings_widgets::bool_setting(
        ui,
        "Ligatures",
        &mut f.ligatures,
        "Enable OpenType ligatures (requires font support)",
    );

    ui.add_space(4.0);
    settings_widgets::combo_setting(
        ui,
        "Hinting",
        &mut f.hinting,
        &[
            (HintingMode::None, "None"),
            (HintingMode::Auto, "Auto"),
            (HintingMode::Full, "Full"),
        ],
        "Font hinting: None (smoothest), Auto (low-DPI only), Full (sharpest)",
    );
    settings_widgets::combo_setting(
        ui,
        "Render Mode",
        &mut f.render_mode,
        &[
            (RenderMode::Subpixel, "Subpixel"),
            (RenderMode::Grayscale, "Grayscale"),
        ],
        "Subpixel LCD (sharpest on LCD) or Grayscale AA (better on OLED)",
    );
}

fn render_font_description(
    ui: &mut egui::Ui,
    label: &str,
    fd: &mut FontDescription,
    font_families: &[String],
    registered_fonts: &HashSet<String>,
) {
    settings_widgets::font_combo_setting(
        ui,
        &format!("{label} Family"),
        &mut fd.family,
        font_families,
        registered_fonts,
        "Font family name",
    );
}

// ── Colors section ───────────────────────────────────────────────────────

fn render_colors_section(ui: &mut egui::Ui, c: &mut ColorsConfig) {
    settings_widgets::section_header(
        ui,
        "Appearance",
        "Choose how light and dark variants are selected.",
    );
    settings_widgets::combo_setting(
        ui,
        "Appearance Mode",
        &mut c.appearance,
        &[
            (ThemePreference::System, "System"),
            (ThemePreference::Dark, "Dark"),
            (ThemePreference::Light, "Light"),
        ],
        "Follow the system preference, or force Light/Dark",
    );

    ui.add_space(8.0);
    settings_widgets::section_header(
        ui,
        "Colour Theme",
        "Each theme provides a light and dark variant.",
    );
    render_palette_gallery(ui, c);

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Primary", "Default text and background.");
    render_primary_colors(ui, &mut c.primary);

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Cursor", "");
    render_cursor_colors(ui, &mut c.cursor);

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Selection", "");
    render_selection_colors(ui, &mut c.selection);

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Normal ANSI", "The 8 dark ANSI colours.");
    render_ansi_colors(ui, &mut c.normal);

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Bright ANSI", "The 8 bright ANSI colours.");
    render_ansi_colors(ui, &mut c.bright);
}

// ── Theme preview and presets ────────────────────────────────────────────

#[derive(Clone, Copy)]
struct PalettePreset {
    palette: ColorTheme,
    name: &'static str,
    description: &'static str,
}

const PALETTE_PRESETS: [PalettePreset; 4] = [
    PalettePreset {
        palette: ColorTheme::Default,
        name: "Default",
        description: "Zenterm balanced default",
    },
    PalettePreset {
        palette: ColorTheme::Nord,
        name: "Nord",
        description: "Calm blue-grey",
    },
    PalettePreset {
        palette: ColorTheme::Forest,
        name: "Forest",
        description: "Soft green contrast",
    },
    PalettePreset {
        palette: ColorTheme::Sepia,
        name: "Sepia",
        description: "Warm paper tones",
    },
];

fn render_palette_gallery(ui: &mut egui::Ui, c: &mut ColorsConfig) {
    ui.label(
        egui::RichText::new("Choose a theme family or edit individual colours below.")
            .color(
                ui.visuals()
                    .weak_text_color
                    .unwrap_or(ui.visuals().text_color()),
            )
            .size(12.0),
    );
    ui.add_space(6.0);

    ui.horizontal_wrapped(|ui| {
        for preset in PALETTE_PRESETS.iter().copied() {
            let desired = egui::vec2(154.0, 72.0);
            let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
            let dark_config = ColorsConfig {
                appearance: ThemePreference::Dark,
                palette: preset.palette,
                ..ColorsConfig::default()
            };
            let light_config = ColorsConfig {
                appearance: ThemePreference::Light,
                palette: preset.palette,
                ..ColorsConfig::default()
            };
            let current_theme = if ui.visuals().dark_mode {
                dark_config.to_theme(true)
            } else {
                light_config.to_theme(false)
            };
            let dark_theme = dark_config.to_theme(true);
            let light_theme = light_config.to_theme(false);
            let background = rgba_to_color32(&current_theme.background);
            let foreground = rgba_to_color32(&current_theme.foreground);
            let accent = rgba_to_color32(&current_theme.ui_accent);
            let selected = c.palette == preset.palette;

            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(8), background);
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(8),
                egui::Stroke::new(
                    if selected { 2.0_f32 } else { 1.0_f32 },
                    if selected {
                        accent
                    } else {
                        foreground.gamma_multiply(0.28)
                    },
                ),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.left_top() + egui::vec2(12.0, 10.0),
                egui::Align2::LEFT_TOP,
                preset.name,
                egui::FontId::proportional(14.0),
                foreground,
            );
            ui.painter().text(
                rect.left_top() + egui::vec2(12.0, 31.0),
                egui::Align2::LEFT_TOP,
                preset.description,
                egui::FontId::proportional(11.0),
                foreground.gamma_multiply(0.68),
            );
            for (index, color) in current_theme.ansi_normal.iter().skip(1).take(4).enumerate() {
                let swatch = egui::Rect::from_min_size(
                    rect.left_bottom() + egui::vec2(12.0 + index as f32 * 23.0, -18.0),
                    egui::vec2(16.0, 8.0),
                );
                ui.painter().rect_filled(
                    swatch,
                    egui::CornerRadius::same(3),
                    rgba_to_color32(color),
                );
            }

            let mode_swatch = egui::Rect::from_min_size(
                rect.right_bottom() - egui::vec2(42.0, 18.0),
                egui::vec2(30.0, 8.0),
            );
            ui.painter().rect_filled(
                egui::Rect::from_min_size(mode_swatch.left_top(), egui::vec2(15.0, 8.0)),
                egui::CornerRadius::same(3),
                rgba_to_color32(&dark_theme.background),
            );
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    mode_swatch.left_top() + egui::vec2(15.0, 0.0),
                    egui::vec2(15.0, 8.0),
                ),
                egui::CornerRadius::same(3),
                rgba_to_color32(&light_theme.background),
            );

            let clicked = response.clicked();
            response.on_hover_text(format!("Apply {} palette", preset.name));
            if clicked {
                apply_palette(c, preset);
            }
        }
    });

    if ui
        .small_button("Use built-in default colours")
        .on_hover_text("Clear custom colour overrides for the selected theme")
        .clicked()
    {
        let preference = c.appearance;
        *c = ColorsConfig::default();
        c.appearance = preference;
    }

    ui.add_space(8.0);
    render_theme_preview(ui, c);
}

fn render_theme_preview(ui: &mut egui::Ui, c: &ColorsConfig) {
    let theme = c.to_theme(ui.visuals().dark_mode);
    let bg = rgba_to_color32(&theme.background);
    let fg = rgba_to_color32(&theme.foreground);
    let dim = rgba_to_color32(&theme.dim_foreground);
    let accent = rgba_to_color32(&theme.ui_accent);
    let selection = rgba_to_color32(&theme.selection_bg);
    let surface = rgba_to_color32(&theme.ui_surface);

    ui.label(egui::RichText::new("Live preview").strong().size(13.0));
    ui.add_space(4.0);

    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 184.0),
        egui::Sense::hover(),
    );
    {
        let painter = ui.painter();
        painter.rect_filled(rect, egui::CornerRadius::same(10), bg);
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(10),
            egui::Stroke::new(1.0_f32, accent.gamma_multiply(0.35)),
            egui::StrokeKind::Inside,
        );

        let origin = rect.left_top() + egui::vec2(16.0, 14.0);
        let mono = egui::FontId::monospace(13.0);
        painter.text(
            origin,
            egui::Align2::LEFT_TOP,
            "user@zenterm  ~/project",
            mono.clone(),
            dim,
        );

        let line_y = [38.0, 61.0, 84.0, 107.0];
        painter.text(
            origin + egui::vec2(0.0, line_y[0]),
            egui::Align2::LEFT_TOP,
            "$ cargo check",
            mono.clone(),
            fg,
        );
        painter.text(
            origin + egui::vec2(0.0, line_y[1]),
            egui::Align2::LEFT_TOP,
            "✓ finished successfully",
            mono.clone(),
            rgba_to_color32(&theme.ansi_normal[2]),
        );
        painter.text(
            origin + egui::vec2(0.0, line_y[2]),
            egui::Align2::LEFT_TOP,
            "warning: unused variable",
            mono.clone(),
            rgba_to_color32(&theme.ansi_normal[3]),
        );
        painter.text(
            origin + egui::vec2(0.0, line_y[3]),
            egui::Align2::LEFT_TOP,
            "error: could not compile",
            mono.clone(),
            rgba_to_color32(&theme.ansi_normal[1]),
        );

        let selected_rect = egui::Rect::from_min_size(
            origin + egui::vec2(205.0, line_y[0] - 2.0),
            egui::vec2(91.0, 18.0),
        );
        painter.rect_filled(selected_rect, egui::CornerRadius::same(3), selection);
        painter.text(
            selected_rect.left_top() + egui::vec2(6.0, 1.0),
            egui::Align2::LEFT_TOP,
            "selected",
            mono.clone(),
            rgba_to_color32(&theme.selection_fg),
        );
        painter.rect_filled(
            egui::Rect::from_min_size(origin + egui::vec2(310.0, line_y[0]), egui::vec2(2.0, 16.0)),
            1.0,
            accent,
        );

        let swatch_y = rect.bottom() - 30.0;
        painter.text(
            egui::pos2(rect.left() + 16.0, swatch_y - 1.0),
            egui::Align2::LEFT_TOP,
            "ANSI",
            egui::FontId::proportional(10.0),
            dim,
        );
        for (index, color) in theme
            .ansi_normal
            .iter()
            .chain(theme.ansi_bright.iter())
            .enumerate()
        {
            let swatch = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 51.0 + index as f32 * 15.0, swatch_y),
                egui::vec2(11.0, 11.0),
            );
            painter.rect_filled(swatch, egui::CornerRadius::same(3), rgba_to_color32(color));
        }
    }

    ui.add_space(6.0);
    let (surface_rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        surface_rect,
        egui::CornerRadius::same(8),
        rgba_to_color32(&theme.ui_bg),
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            surface_rect.left_top() + egui::vec2(10.0, 9.0),
            egui::vec2(82.0, 28.0),
        ),
        egui::CornerRadius::same(5),
        surface,
    );
    painter.text(
        surface_rect.left_top() + egui::vec2(23.0, 15.0),
        egui::Align2::LEFT_TOP,
        "workspace",
        egui::FontId::proportional(12.0),
        fg,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            surface_rect.left_top() + egui::vec2(105.0, 9.0),
            egui::vec2(78.0, 28.0),
        ),
        egui::CornerRadius::same(5),
        accent,
    );
    painter.text(
        surface_rect.left_top() + egui::vec2(121.0, 15.0),
        egui::Align2::LEFT_TOP,
        "active tab",
        egui::FontId::proportional(12.0),
        bg,
    );
}

fn apply_palette(c: &mut ColorsConfig, preset: PalettePreset) {
    c.palette = preset.palette;
    c.primary = PrimaryColors::default();
    c.cursor = CursorColors::default();
    c.selection = SelectionColors::default();
    c.normal = AnsiColors::default();
    c.bright = AnsiColors::default();
}

fn rgba_to_color32(c: &Rgba) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (c.r() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.g() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.b() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.a() * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn render_primary_colors(ui: &mut egui::Ui, p: &mut PrimaryColors) {
    settings_widgets::color_hex_setting(ui, "Foreground", &mut p.foreground, "Default text colour");
    settings_widgets::color_hex_setting(
        ui,
        "Background",
        &mut p.background,
        "Default background colour",
    );
    settings_widgets::color_hex_setting(
        ui,
        "Dim Foreground",
        &mut p.dim_foreground,
        "Half-intensity text colour",
    );
    settings_widgets::color_hex_setting(
        ui,
        "Bright Foreground",
        &mut p.bright_foreground,
        "Text colour when bold + bright-colours is enabled",
    );
}

fn render_cursor_colors(ui: &mut egui::Ui, c: &mut CursorColors) {
    settings_widgets::color_hex_setting(
        ui,
        "Text",
        &mut c.text,
        "Colour for text under the cursor",
    );
    settings_widgets::color_hex_setting(ui, "Cursor", &mut c.cursor, "Colour for the cursor cell");
}

fn render_selection_colors(ui: &mut egui::Ui, s: &mut SelectionColors) {
    settings_widgets::color_hex_setting(
        ui,
        "Foreground",
        &mut s.foreground,
        "Selected text colour",
    );
    settings_widgets::color_hex_setting(
        ui,
        "Background",
        &mut s.background,
        "Selection background colour",
    );
}

fn render_ansi_colors(ui: &mut egui::Ui, a: &mut AnsiColors) {
    settings_widgets::color_hex_setting(ui, "Black", &mut a.black, "");
    settings_widgets::color_hex_setting(ui, "Red", &mut a.red, "");
    settings_widgets::color_hex_setting(ui, "Green", &mut a.green, "");
    settings_widgets::color_hex_setting(ui, "Yellow", &mut a.yellow, "");
    settings_widgets::color_hex_setting(ui, "Blue", &mut a.blue, "");
    settings_widgets::color_hex_setting(ui, "Magenta", &mut a.magenta, "");
    settings_widgets::color_hex_setting(ui, "Cyan", &mut a.cyan, "");
    settings_widgets::color_hex_setting(ui, "White", &mut a.white, "");
}

// ── Cursor section ───────────────────────────────────────────────────────

fn render_cursor_section(ui: &mut egui::Ui, c: &mut CursorConfig) {
    settings_widgets::section_header(ui, "Cursor", "Cursor appearance and behaviour.");
    settings_widgets::combo_setting(
        ui,
        "Shape",
        &mut c.style.shape,
        &[
            (CursorShape::Block, "Block"),
            (CursorShape::Beam, "Beam"),
            (CursorShape::Underline, "Underline"),
        ],
        "Visual cursor shape",
    );
    settings_widgets::combo_setting(
        ui,
        "Blinking",
        &mut c.style.blinking,
        &[
            (Blinking::Off, "Off"),
            (Blinking::On, "On"),
            (Blinking::Terminal, "Terminal"),
        ],
        "Cursor blinking mode",
    );

    settings_widgets::bool_setting(
        ui,
        "Unfocused Hollow",
        &mut c.unfocused_hollow,
        "Show a hollow cursor when the window loses focus",
    );
    settings_widgets::slider_setting(
        ui,
        "Thickness",
        &mut c.thickness,
        0.0..=1.0,
        "Thickness of Beam/Underline cursor (fraction of cell)",
    );
    settings_widgets::drag_u64(
        ui,
        "Blink Interval",
        &mut c.blink_interval,
        1.0,
        "Frames between blinks at 60 FPS (30 ≈ 500 ms)",
    );
    settings_widgets::drag_u64(
        ui,
        "Blink Timeout",
        &mut c.blink_timeout,
        1.0,
        "Seconds before blinking stops (0 = blink forever)",
    );
}

// ── Selection section ────────────────────────────────────────────────────

fn render_selection_section(ui: &mut egui::Ui, s: &mut SelectionConfig) {
    settings_widgets::section_header(ui, "Selection", "Text selection behaviour.");
    settings_widgets::bool_setting(
        ui,
        "Save to Clipboard",
        &mut s.save_to_clipboard,
        "Automatically copy selected text to the system clipboard",
    );
}

// ── UI section ───────────────────────────────────────────────────────────

fn render_ui_section(ui: &mut egui::Ui, u: &mut UiConfig) {
    settings_widgets::section_header(ui, "Tabs", "Multi-terminal tab bar.");
    settings_widgets::bool_setting(
        ui,
        "Enable Tabs",
        &mut u.tabs_enabled,
        "Show tab bar for multiple terminal sessions",
    );
    settings_widgets::bool_setting(ui, "Show Add Tab Button", &mut u.show_add_tab_button, "");
    settings_widgets::bool_setting(
        ui,
        "Show Close Tab Button",
        &mut u.show_close_tab_button,
        "",
    );

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Sidebar", "cmux-style workspace sidebar.");
    settings_widgets::bool_setting(
        ui,
        "Enable Sidebar",
        &mut u.sidebar_enabled,
        "Show the workspace sidebar (requires tabs)",
    );
    settings_widgets::combo_setting(
        ui,
        "Sidebar Position",
        &mut u.sidebar_position,
        &[
            (SidebarPosition::Left, "Left"),
            (SidebarPosition::Right, "Right"),
        ],
        "",
    );
    settings_widgets::slider_setting(
        ui,
        "Width",
        &mut u.sidebar_width,
        100.0..=800.0,
        "Default sidebar width in logical pixels",
    );
    settings_widgets::slider_setting(
        ui,
        "Min Width",
        &mut u.sidebar_min_width,
        80.0..=600.0,
        "Minimum sidebar width",
    );
    settings_widgets::slider_setting(
        ui,
        "Max Width",
        &mut u.sidebar_max_width,
        200.0..=1200.0,
        "Maximum sidebar width",
    );

    ui.add_space(8.0);
    settings_widgets::section_header(ui, "Layout Persistence", "");
    settings_widgets::bool_setting(
        ui,
        "Restore Layout on Startup",
        &mut u.restore_layout_on_startup,
        "Re-open previous tabs and workspaces on start",
    );
    settings_widgets::bool_setting(
        ui,
        "Persist Layout",
        &mut u.persist_layout,
        "Save tab layout changes to disk automatically",
    );
    settings_widgets::drag_u64(
        ui,
        "Debounce (ms)",
        &mut u.layout_debounce_ms,
        10.0,
        "Milliseconds to wait before writing layout changes",
    );
}

// ── Background section ────────────────────────────────────────────────────

fn render_background_section(ui: &mut egui::Ui, bg: &mut BackgroundConfig) {
    settings_widgets::section_header(ui, "Background", "Terminal background image.");

    // Image path with browse button.
    let mut path = bg.image_path.clone().unwrap_or_default();
    settings_widgets::row(ui, "Image Path", "Absolute path to an image file.", |ui| {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut path)
                    .hint_text("Select or type a path…")
                    .desired_width(220.0),
            );
            if ui.button("Browse…").clicked()
                && let Some(picked) = rfd::FileDialog::new()
                    .set_title("Select Background Image")
                    .add_filter(
                        "Images",
                        &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "ico"],
                    )
                    .pick_file()
            {
                path = picked.display().to_string();
            }
        });
    });
    bg.image_path = if path.is_empty() { None } else { Some(path) };

    settings_widgets::slider_setting(
        ui,
        "Image Opacity",
        &mut bg.image_opacity,
        0.0..=1.0,
        "Blends image with terminal background.",
    );

    settings_widgets::combo_setting(
        ui,
        "Fit Mode",
        &mut bg.image_mode,
        &[
            (ImageFitMode::Cover, "Cover"),
            (ImageFitMode::Contain, "Contain"),
            (ImageFitMode::Stretch, "Stretch"),
            (ImageFitMode::Center, "Center"),
        ],
        "How the image fits the terminal area.",
    );
}

// ── Egui font registration for preview ─────────────────────────────────

/// Register all known monospace fonts into egui's font system so that
/// [`font_combo_setting`](settings_widgets::font_combo_setting) can render
/// each font-family name in its own typeface.
///
/// This is a **one-time** operation (gated by
/// [`SettingsState::fonts_registered`]).  It reads font-file data from disk
/// and calls [`egui::Context::set_fonts`], which rebuilds the font atlas.
/// On a typical system with 20–30 monospace families the total cost is
/// ≈30–60 ms and happens only when the settings panel is first opened.
///
/// Returns the set of family names that were successfully registered.
/// Only these names should be passed to [`font_combo_setting`] via
/// [`SettingsState::registered_fonts`] so that the ComboBox only uses
/// `RichText` previews for fonts that egui can actually render.
pub fn register_preview_fonts(ctx: &egui::Context, families: &[String]) -> HashSet<String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    let mut fonts = egui::FontDefinitions::default();
    crate::icons::init_fonts(&mut fonts);
    let mut ok: HashSet<String> = HashSet::with_capacity(families.len());

    for family in families {
        // Find the file path + face index for a regular-weight face.
        let Some(src) = zenterm_glyph::font_list::find_font_source(&db, family) else {
            continue;
        };

        let Ok(data) = std::fs::read(&src.path) else {
            continue;
        };

        // Validate font data with ttf-parser (same engine fontdb uses).
        // egui uses skrifa internally; some fonts that pass fontdb's
        // ttf-parser still confuse skrifa and produce a zero units_per_em,
        // which would cause a division-by-zero panic inside epaint.
        if !zenterm_glyph::font_list::validate_font_data(&data, src.index) {
            log::warn!(
                "register_preview_fonts: skipping {family:?} ({}:{}) – ttf-parser rejects face",
                src.path.display(),
                src.index,
            );
            continue;
        }

        fonts.font_data.insert(
            family.clone(),
            std::sync::Arc::new(egui::FontData {
                font: std::borrow::Cow::Owned(data),
                index: src.index,
                tweak: Default::default(),
            }),
        );
        fonts
            .families
            .entry(egui::FontFamily::Name(family.clone().into()))
            .or_default()
            .push(family.clone());
        ok.insert(family.clone());
    }

    log::info!(
        "register_preview_fonts: registered {} / {} families",
        ok.len(),
        families.len()
    );
    ctx.set_fonts(fonts);
    ok
}
