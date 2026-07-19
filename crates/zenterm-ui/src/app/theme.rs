//! Theme synchronisation and colour helpers for [`ZentermApp`](super::ZentermApp).
//!
//! In addition to the terminal colour scheme, this module also configures
//! the egui global style (widget colours, spacing, corner radii) so the
//! UI chrome (side panel, tab bar, settings) feels cohesive with the
//! terminal theme.

use egui::{Context, Stroke, CornerRadius, Color32, Visuals};

use zenterm_core::theme::Theme;
use zenterm_term::ColorScheme;

use crate::workspace::WorkspaceManager;
use super::ZentermApp;

// ── Global egui style builder ─────────────────────────────────────────

/// Build an [`egui::Style`] that matches the given terminal [`Theme`].
///
/// Called from [`ZentermApp::sync_theme`] on every theme change so the
/// whole chrome (panels, tabs, sidebar, settings) shares the terminal's
/// colour palette.
pub(crate) fn configure_egui_style(ctx: &egui::Context, theme: &Theme) {
    let ui_bg = rgba_to_color32(&theme.ui_bg);
    let ui_text = rgba_to_color32(&theme.ui_text);
    let accent = rgba_to_color32(&theme.ui_accent);
    let surface = rgba_to_color32(&theme.ui_surface);
    let dark_mode = theme.background.r() < 0.5;

    // Border colour derived from background.
    let border = if dark_mode {
        Color32::from_gray(48)
    } else {
        Color32::from_gray(210)
    };

    // ── Text colours ────────────────────────────────────────────────
    let text_color = ui_text;
    let weak_text = Color32::from_rgba_premultiplied(
        text_color.r(),
        text_color.g(),
        text_color.b(),
        (text_color.a() as f32 * 0.65) as u8,
    );

    // ── Widget state colours ────────────────────────────────────────
    let hover_bg = if dark_mode {
        Color32::from_rgb(55, 55, 55)
    } else {
        Color32::from_rgb(220, 220, 220)
    };
    let active_bg = accent.linear_multiply(0.8);
    let open_bg = accent.linear_multiply(0.15);
    let rounding = CornerRadius::same(6);
    let small_rounding = CornerRadius::same(4);

    let ext_bg = if dark_mode {
        Color32::from_rgb(12, 12, 12)
    } else {
        Color32::from_rgb(245, 245, 245)
    };

    let base = if dark_mode { Visuals::dark() } else { Visuals::light() };

    let visuals = Visuals {
        dark_mode,
        panel_fill: ext_bg,             // matches tab-bar background
        extreme_bg_color: ext_bg,       // used by the tab bar
        window_fill: surface,
        faint_bg_color: if dark_mode {
            Color32::from_rgb(22, 22, 22)
        } else {
            Color32::from_rgb(235, 235, 235)
        },
        window_corner_radius: CornerRadius::same(8),
        window_stroke: Stroke::new(1.0, border),
        window_highlight_topmost: true,
        menu_corner_radius: CornerRadius::same(6),
        popup_shadow: egui::Shadow {
            offset: [0, 8].into(),
            blur: 24,
            spread: 0,
            color: Color32::BLACK.linear_multiply(0.3),
        },
        selection: egui::style::Selection {
            bg_fill: Color32::from_rgba_premultiplied(
                accent.r(), accent.g(), accent.b(),
                if dark_mode { 55 } else { 40 },
            ),
            stroke: Stroke::new(1.0, text_color),
        },
        weak_text_alpha: 0.65,
        weak_text_color: Some(weak_text),
        hyperlink_color: accent,
        warn_fg_color: Color32::from_rgb(255, 180, 0),
        error_fg_color: Color32::from_rgb(255, 80, 80),
        // Override widget state colors.
        widgets: egui::style::Widgets {
            noninteractive: egui::style::WidgetVisuals {
                weak_bg_fill: Color32::TRANSPARENT,
                bg_fill: ui_bg,
                bg_stroke: Stroke::new(1.0, border),
                fg_stroke: Stroke::new(1.0, text_color),
                corner_radius: rounding,
                expansion: 0.0,
            },
            inactive: egui::style::WidgetVisuals {
                weak_bg_fill: Color32::TRANSPARENT,
                bg_fill: ui_bg,
                bg_stroke: Stroke::new(1.0, border),
                fg_stroke: Stroke::new(1.0, text_color),
                corner_radius: rounding,
                expansion: 0.0,
            },
            hovered: egui::style::WidgetVisuals {
                weak_bg_fill: Color32::TRANSPARENT,
                bg_fill: hover_bg,
                bg_stroke: Stroke::new(1.0, accent),
                fg_stroke: Stroke::new(1.5, text_color),
                corner_radius: small_rounding,
                expansion: 1.0,
            },
            active: egui::style::WidgetVisuals {
                weak_bg_fill: Color32::TRANSPARENT,
                bg_fill: active_bg,
                bg_stroke: Stroke::new(1.0, accent),
                fg_stroke: Stroke::new(2.0, text_color),
                corner_radius: small_rounding,
                expansion: 0.0,
            },
            open: egui::style::WidgetVisuals {
                weak_bg_fill: Color32::TRANSPARENT,
                bg_fill: open_bg,
                bg_stroke: Stroke::new(1.0, accent),
                fg_stroke: Stroke::new(1.0, text_color),
                corner_radius: rounding,
                expansion: 0.0,
            },
        },
        ..base
    };

    let mut style = egui::Style {
        visuals,
        spacing: egui::style::Spacing {
            item_spacing: egui::vec2(10.0, 6.0),
            button_padding: egui::vec2(8.0, 3.0),
            window_margin: egui::Margin::symmetric(8, 8),
            indent: 16.0,
            interact_size: egui::vec2(48.0, 24.0),
            slider_width: 120.0,
            combo_width: 160.0,
            icon_width: 18.0,
            icon_spacing: 6.0,
            tooltip_width: 320.0,
            ..egui::style::Spacing::default()
        },
        ..egui::Style::default()
    };
    style.animation_time = 0.08;

    ctx.set_global_style(style);
}

impl ZentermApp {
    // ── Theme sync (app-level) ─────────────────────────────────────

    /// Sync the active theme with the user's preference and the OS
    /// system theme.  Rebuilds each session's colour scheme when the
    /// theme changes, and re-configures the egui global style so the
    /// UI chrome matches.
    pub(crate) fn sync_theme(&mut self, egui_ctx: &Context) {
        let system_dark = egui_ctx.input(|i| match i.raw.system_theme {
            Some(egui::Theme::Dark) => true,
            Some(egui::Theme::Light) => false,
            None => true,
        });
        let new_theme = self.config.colors.to_theme(system_dark);
        let theme_changed = new_theme.background.r() != self.theme.background.r()
            || new_theme.background.g() != self.theme.background.g()
            || new_theme.background.b() != self.theme.background.b();
        if theme_changed || self.last_system_dark != system_dark {
            self.theme = new_theme.clone();
            self.last_system_dark = system_dark;
            self.default_bg = theme_bg_to_color32(&new_theme);
            let scheme = ColorScheme::from_theme(&new_theme);
            for (_, session) in self.sessions.iter_mut() {
                session.terminal.set_scheme(scheme.clone());
                session.default_bg = self.default_bg;
                session.terminal_dirty = true;
            }

            // Sync egui chrome style to match the terminal theme.
            configure_egui_style(egui_ctx, &new_theme);
        }
    }

    /// Generate a unique workspace name based on the active tab's
    /// title.  Falls back to a numbered name if the title is empty
    /// or the name already exists.
    pub(crate) fn generate_workspace_name(
        workspaces: &WorkspaceManager,
        active_title: &str,
    ) -> String {
        let base = if active_title.is_empty() {
            "workspace".into()
        } else {
            // Truncate very long titles to keep the sidebar tidy.
            let mut t = active_title.to_string();
            t.truncate(40);
            t
        };

        // Ensure uniqueness by appending a suffix if needed.
        let existing: std::collections::HashSet<String> = workspaces
            .workspaces
            .iter()
            .map(|ws| ws.name.clone())
            .collect();
        if !existing.contains(&base) {
            return base;
        }
        for i in 2.. {
            let candidate = format!("{base}-{i}");
            if !existing.contains(&candidate) {
                return candidate;
            }
        }
        unreachable!()
    }

}
// ── Colour helpers ─────────────────────────────────────────────────────

/// Convert a [`Theme`] background colour to `egui::Color32`.
pub(crate) fn theme_bg_to_color32(theme: &Theme) -> egui::Color32 {
    rgba_to_color32(&theme.background)
}

/// Convert a [`zenterm_core::color::Rgba`] to `egui::Color32`.
fn rgba_to_color32(c: &zenterm_core::color::Rgba) -> egui::Color32 {
    egui::Color32::from_rgba_premultiplied(
        (c.r() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.g() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.b() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.a() * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

