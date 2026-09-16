//! Theme synchronisation and colour helpers for [`ZentermApp`](super::ZentermApp).
//!
//! In addition to the terminal colour scheme, this module also configures
//! the egui global style (widget colours, spacing, corner radii) so the
//! UI chrome (side panel, tab bar, settings) feels cohesive with the
//! terminal theme.

use egui::{Color32, Context, CornerRadius, Stroke, Visuals};

use zenterm_core::theme::Theme;
use zenterm_term::ColorScheme;

use super::ZentermApp;
use crate::workspace::WorkspaceManager;

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

    // Keep UI chrome derived from resolved theme tokens instead of a
    // separate hard-coded light/dark palette.
    let border = if dark_mode {
        Color32::from_rgba_unmultiplied(255, 255, 255, 28)
    } else {
        Color32::from_rgba_unmultiplied(0, 0, 0, 24)
    };

    // ── Text colours ────────────────────────────────────────────────
    let text_color = ui_text;
    let weak_text = Color32::from_rgba_unmultiplied(
        text_color.r(),
        text_color.g(),
        text_color.b(),
        (text_color.a() as f32 * 0.65) as u8,
    );

    // ── Widget state colours ────────────────────────────────────────
    let hover_bg = if dark_mode {
        with_alpha(accent, 35)
    } else {
        with_alpha(accent, 22)
    };
    let active_bg = accent.linear_multiply(0.8);
    let open_bg = with_alpha(accent, if dark_mode { 45 } else { 30 });
    let rounding = CornerRadius::same(6);
    let small_rounding = CornerRadius::same(5);

    let faint_bg = if dark_mode {
        blend_colors(ui_bg, surface, 0.35)
    } else {
        blend_colors(ui_bg, surface, 0.50)
    };

    let base = if dark_mode {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    let visuals = Visuals {
        dark_mode,
        override_text_color: Some(text_color),
        panel_fill: ui_bg,
        extreme_bg_color: ui_bg,
        window_fill: surface,
        faint_bg_color: faint_bg,
        window_corner_radius: CornerRadius::same(8),
        window_stroke: Stroke::new(1.0_f32, border),
        window_highlight_topmost: true,
        menu_corner_radius: CornerRadius::same(6),
        popup_shadow: egui::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::BLACK.linear_multiply(if dark_mode { 0.4 } else { 0.12 }),
        },
        selection: egui::style::Selection {
            bg_fill: with_alpha(accent, if dark_mode { 55 } else { 40 }),
            stroke: Stroke::new(1.0_f32, accent),
        },
        weak_text_alpha: 0.65,
        weak_text_color: Some(weak_text),
        hyperlink_color: accent,
        warn_fg_color: rgba_to_color32(&theme.ansi_bright[3]),
        error_fg_color: rgba_to_color32(&theme.ansi_bright[1]),
        // Override widget state colors.
        widgets: egui::style::Widgets {
            noninteractive: egui::style::WidgetVisuals {
                weak_bg_fill: Color32::TRANSPARENT,
                bg_fill: surface,
                bg_stroke: Stroke::new(1.0_f32, border),
                fg_stroke: Stroke::new(1.0_f32, text_color),
                corner_radius: rounding,
                expansion: 0.0,
            },
            inactive: egui::style::WidgetVisuals {
                weak_bg_fill: Color32::TRANSPARENT,
                bg_fill: surface,
                bg_stroke: Stroke::new(1.0_f32, border),
                fg_stroke: Stroke::new(1.0_f32, text_color),
                corner_radius: rounding,
                expansion: 0.0,
            },
            hovered: egui::style::WidgetVisuals {
                weak_bg_fill: hover_bg,
                bg_fill: hover_bg,
                bg_stroke: Stroke::new(1.0_f32, accent),
                fg_stroke: Stroke::new(1.5_f32, text_color),
                corner_radius: small_rounding,
                expansion: 0.0,
            },
            active: egui::style::WidgetVisuals {
                weak_bg_fill: active_bg,
                bg_fill: active_bg,
                bg_stroke: Stroke::new(1.0_f32, accent),
                fg_stroke: Stroke::new(1.5_f32, if dark_mode { Color32::WHITE } else { text_color }),
                corner_radius: small_rounding,
                expansion: 0.0,
            },
            open: egui::style::WidgetVisuals {
                weak_bg_fill: open_bg,
                bg_fill: open_bg,
                bg_stroke: Stroke::new(1.0_f32, accent),
                fg_stroke: Stroke::new(1.0_f32, text_color),
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

/// Resolve the OS theme exposed by egui.  During very early startup the
/// platform may not have populated `raw.system_theme` yet, so preserve the
/// current egui visual mode instead of unconditionally forcing dark mode.
pub(crate) fn system_theme_is_dark(ctx: &Context) -> bool {
    let current_visuals_dark = ctx.global_style().visuals.dark_mode;
    ctx.input(|i| match i.raw.system_theme {
        Some(egui::Theme::Dark) => true,
        Some(egui::Theme::Light) => false,
        None => current_visuals_dark,
    })
}

impl ZentermApp {
    // ── Theme sync (app-level) ─────────────────────────────────────

    /// Sync the active theme with the user's preference and the OS
    /// system theme.  Rebuilds each session's colour scheme when the
    /// theme changes, and re-configures the egui global style so the
    /// UI chrome matches.
    pub(crate) fn sync_theme(&mut self, egui_ctx: &Context) {
        let system_dark = system_theme_is_dark(egui_ctx);
        let new_theme = self.config.colors.to_theme(system_dark);
        let theme_changed = new_theme != self.theme;
        if theme_changed || self.last_system_dark != system_dark {
            self.theme = new_theme.clone();
            self.last_system_dark = system_dark;
            self.default_bg = theme_bg_to_color32(&new_theme);
            let scheme = ColorScheme::from_theme(&new_theme);
            for session in self.sessions.values_mut() {
                session.terminal.set_scheme(scheme.clone());
                session.default_bg = self.default_bg;
                session.terminal_dirty = true;
            }

            // Sync egui chrome style to match the terminal theme.
            configure_egui_style(egui_ctx, &new_theme);
        }
    }

    /// Generate a unique workspace name.
    ///
    /// Uses the active session's effective title to produce a concise,
    /// readable name suitable for the sidebar.  Falls back to
    /// `"workspace"` if no session is available or the title is empty.
    /// Appends a numeric suffix (`-2`, `-3`, …) if the name is taken.
    pub(crate) fn generate_workspace_name(
        workspaces: &WorkspaceManager,
        active_session: Option<&crate::session::TerminalSession>,
    ) -> String {
        let base = match active_session {
            Some(session) => {
                let title = session.title_effective();
                extract_workspace_essence(&title)
            }
            None => "workspace".into(),
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

/// Extract a concise, human-readable workspace name from a session title.
fn extract_workspace_essence(title: &str) -> String {
    let t = title.trim();
    if t.is_empty() {
        return "workspace".into();
    }

    // ① If the title looks like a path, take the last (non-empty) component.
    if (t.contains('/') || t.contains('\\'))
        && let Some(last) = t.split(&['/', '\\'][..]).rfind(|s| !s.is_empty())
    {
        // Also strip common file extensions for cleanliness.
        let cleaned = if let Some((stem, _ext)) = last.rsplit_once('.') {
            if stem.len() > 1 { stem } else { last }
        } else {
            last
        };
        return truncate_str(cleaned, 30);
    }

    // ② If it looks like a command line (contains spaces), take the first word.
    if let Some((cmd, _)) = t.split_once(' ') {
        return truncate_str(cmd, 30);
    }

    // ③ Use the title as-is (truncated to 30 chars).
    truncate_str(t, 30)
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        s[..max].to_string()
    }
}
// ── Colour helpers ─────────────────────────────────────────────────────

/// Convert a [`Theme`] background colour to `egui::Color32`.
pub(crate) fn theme_bg_to_color32(theme: &Theme) -> egui::Color32 {
    rgba_to_color32(&theme.background)
}

/// Convert a [`zenterm_core::color::Rgba`] to `egui::Color32`.
fn rgba_to_color32(c: &zenterm_core::color::Rgba) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (c.r() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.g() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.b() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.a() * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn with_alpha(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn blend_colors(a: egui::Color32, b: egui::Color32, amount: f32) -> egui::Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    egui::Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}
