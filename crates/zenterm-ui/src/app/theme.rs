//! Theme synchronisation and colour helpers for [`ZentermApp`](super::ZentermApp).

use egui::Context;

use zenterm_core::theme::Theme;
use zenterm_term::ColorScheme;

use crate::workspace::WorkspaceManager;
use super::ZentermApp;

impl ZentermApp {
    // ── Theme sync (app-level) ─────────────────────────────────────

    /// Sync the active theme with the user's preference and the OS
    /// system theme.  Rebuilds each session's colour scheme when the
    /// theme changes.
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
        }
    }

    /// Generate a unique workspace name based on the current working
    /// directory.  Falls back to a numbered name if the cwd is
    /// unavailable or the name already exists.
    pub(crate) fn generate_workspace_name(workspaces: &WorkspaceManager) -> String {
        let cwd_name = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));

        let base = cwd_name.unwrap_or_else(|| "workspace".into());

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
    let b = theme.background;
    egui::Color32::from_rgba_premultiplied(
        (b.r() * 255.0).round().clamp(0.0, 255.0) as u8,
        (b.g() * 255.0).round().clamp(0.0, 255.0) as u8,
        (b.b() * 255.0).round().clamp(0.0, 255.0) as u8,
        (b.a() * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

