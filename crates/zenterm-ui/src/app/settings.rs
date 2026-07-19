//! Settings viewport — renders the settings panel in a separate OS window.

use std::time::Instant;

use zenterm_config::Config;

use super::ZentermApp;

// ── Settings viewport (native OS window) ─────────────────────────────

impl ZentermApp {
    /// Show the settings panel in a separate native OS window (no
    /// minimize/maximize buttons, resizable).
    pub(crate) fn render_settings_viewport(&mut self, ctx: &egui::Context) {
        use egui::{ViewportBuilder, ViewportId};

        // ── One-time font registration ─────────────────────────────
        if self.settings_state.open && !self.settings_state.fonts_registered {
            let ok = crate::settings::register_preview_fonts(ctx, &self.settings_state.font_families);
            self.settings_state.registered_fonts = ok;
            self.settings_state.fonts_registered = true;
        }

        let viewport_id = ViewportId::from_hash_of("zenterm_settings_viewport");
        let builder = ViewportBuilder::default()
            .with_title("Settings")
            .with_inner_size(egui::vec2(720.0, 520.0))
            .with_resizable(true)
            .with_minimize_button(false)
            .with_maximize_button(false);

        // Extract borrows *before* the closure to satisfy the borrow
        // checker — show_viewport_immediate borrows the context, and
        // the closure must not borrow self (which is already mutably
        // borrowed by update()).
        let settings_state = &mut self.settings_state;
        let config = &self.config;

        let output = ctx.show_viewport_immediate(
            viewport_id,
            builder,
            |ctx, _class| {
                // User clicked the native close button → hide the viewport.
                if ctx.input(|i| i.viewport().close_requested()) {
                    settings_state.open = false;
                    return crate::settings::SettingsOutput::default();
                }

                // Set the window title (dirty indicator).
                let dirty = settings_state.is_dirty(config);
                let title = if dirty {
                    "Settings ●"
                } else {
                    "Settings"
                };
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.to_owned()));

                // Render the settings form (no egui::Window wrapper).
                crate::settings::render_settings_viewport(ctx, settings_state, config)
            },
        );

        // ── Immediate apply: if working_config changed, apply now ─
        if settings_state.is_dirty(&self.config) {
            let new_config = settings_state.working_config.clone();
            self.apply_new_config(new_config, ctx);
            // Persist settings changes to disk (debounced).
            self.config_dirty = true;
            self.last_config_save_at = Some(Instant::now());
            // Wake the main viewport so terminal re-renders with new config.
            ctx.request_repaint();
        }

        // ── Handle Reset All ──────────────────────────────────────
        if output.reset_all_confirmed {
            // Reset to defaults, apply, and save.
            self.settings_state.working_config = Config::default();
            self.apply_new_config(Config::default(), ctx);
            if let Err(e) = self.config.save() {
                log::error!("settings reset + save failed: {e}");
                self.error_toast = Some(format!("Failed to save reset config: {e}"));
            }
            self.settings_state.open = false;
            ctx.request_repaint();
        }

        // Keep the main viewport alive while the settings window is
        // open.  egui's `show_viewport_immediate` requires the parent
        // to keep painting for the child native window to remain
        // responsive.  We use a timer (not immediate) so idle CPU stays
        // near zero while the settings panel is visible but inactive.
        // User interactions with the settings panel trigger their own
        // repaints at full framerate.
        if self.settings_state.open {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}
