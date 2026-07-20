//! Config persistence and live reload.
//!
//! Debounced disk writes, window-size tracking, and runtime config application.

use std::time::{Duration, Instant};

use egui::Context;

use zenterm_config::Config;
use zenterm_core::SubpixelLayout;
use zenterm_term::ColorScheme;

use super::theme::theme_bg_to_color32;
use super::ZentermApp;

impl ZentermApp {
    pub(crate) fn maybe_save_config(&mut self) {
        if !self.config_dirty {
            return;
        }
        const DEBOUNCE_MS: u64 = 500;
        if let Some(at) = self.last_config_save_at {
            if at.elapsed() >= Duration::from_millis(DEBOUNCE_MS) {
                if let Err(e) = self.config.save() {
                    log::error!("failed to save config: {e}");
                }
                self.config_dirty = false;
            }
        }
    }

    /// Track the active terminal's grid dimensions and save them to
    /// the config file with a 2-second debounce.
    ///
    /// Called every frame from [`Self::update`] after all sessions
    /// have been rendered (and possibly resized).  If the terminal
    /// dimensions changed, the in-memory config is updated immediately;
    /// the disk write is deferred so that rapid drag-resizing doesn't
    /// thrash the I/O.
    pub(crate) fn track_and_persist_window_size(&mut self, ctx: &Context) {
        // Pick the active session, or any session if none is active.
        let session = match self
            .active_session_id
            .and_then(|id| self.sessions.get(&id))
            .or_else(|| self.sessions.values().next())
        {
            Some(s) => s,
            None => return,
        };

        let ts = session.terminal.size();
        let dims = &mut self.config.window.dimensions;

        if ts.cols != dims.columns || ts.rows != dims.lines {
            dims.columns = ts.cols;
            dims.lines = ts.rows;

            // Capture the window's current logical size so we can
            // restore it exactly on the next startup — bypassing the
            // inaccurate `font.size * 0.6` estimate.
            if let Some(inner_rect) = ctx.input(|i| i.viewport().inner_rect) {
                let size = inner_rect.size();
                self.config.window.last_window_size = Some([size.x, size.y]);
            }

            self.config_dirty = true;
            self.last_config_save_at = Some(Instant::now());
        }
    }

    // ── Config reload ─────────────────────────────────────────────
    /// Apply a new config in-place, updating all sessions and the
    /// glyph atlas as needed.  Returns the diff of what changed.
    pub(crate) fn apply_new_config(&mut self, new_config: Config, egui_ctx: &Context) -> zenterm_config::ConfigChanges {
        let old_config = std::mem::replace(&mut self.config, new_config);
        let changes = old_config.diff_to(&self.config);

        // Re-resolve theme.
        let system_dark = egui_ctx.input(|i| match i.raw.system_theme {
            Some(egui::Theme::Dark) => true,
            Some(egui::Theme::Light) => false,
            None => true,
        });
        self.theme = self.config.colors.to_theme(system_dark);
        self.last_system_dark = system_dark;
        self.default_bg = theme_bg_to_color32(&self.theme);

        // Propagate window opacity to all sessions so the terminal
        // background quad alpha and the egui rect_filled alpha both
        // reflect the new transparency level.
        if changes.window {
            let opacity = self.config.window.opacity;
            for (_, session) in self.sessions.iter_mut() {
                session.window_opacity = opacity;
                session.terminal_dirty = true;
            }
        }

        if changes.colors {
            let scheme = ColorScheme::from_theme(&self.theme);
            for (_, session) in self.sessions.iter_mut() {
                session.terminal.set_scheme(scheme.clone());
                session.default_bg = self.default_bg;
            }
        }

        // Apply per-session config changes.
        if changes.font || changes.cursor || changes.colors {
            for (_, session) in self.sessions.iter_mut() {
                session.apply_config_change(self.config.font.size, self.config.cursor.blink_interval);
                session.terminal_dirty = true;
            }
        }

        // Rebuild the glyph atlas when any font property changes
        // (size, family, ligatures, hinting, etc.).
        if changes.font {
            let new_font_size = self.config.font.size * self.pixels_per_point;
            let font_family =
                std::borrow::Cow::Owned(self.config.font.normal.family.clone());
            let (cw, ch) = self.atlas.reinit_for_dpi(
                new_font_size,
                font_family,
                self.pixels_per_point,
                SubpixelLayout::detect(),
                self.config.font.ligatures,
                self.config.font.hinting,
                self.config.font.render_mode,
            );
            self.atlas.seed_ascii();
            self.atlas.sync_to_gpu();
            for (_, session) in self.sessions.iter_mut() {
                session.cell_width = cw;
                session.cell_height = ch;
            }
        }

        // Handle background image changes.
        // Only re-decode the image when the path itself changes.
        // Opacity and fit-mode changes are read live from config
        // in emit_background_quad() and do not require a reload.
        if changes.background {
            let _t0 = std::time::Instant::now();
            let old_path = old_config.background.image_path.as_deref().unwrap_or("").to_owned();
            let new_path = self.config.background.image_path.as_deref().unwrap_or("").to_owned();
            if old_path != new_path {
                // Clone the path before the mutable borrow.
                let path = self.config.background.image_path.clone();
                match path {
                    Some(p) if !p.is_empty() => {
                        self.load_background_image(&p);
                        log::debug!("bg: apply_new_config -> load_background_image took {:?}", _t0.elapsed());
                    }
                    _ => {
                        // Clear the background image.
                        self.background_image_loaded = false;
                        self.loaded_bg_image_size = None;
                        *self.gpu.shared.background_data.lock().expect("background_data lock") = None;
                        log::debug!("bg: cleared (apply_new_config) {:?}", _t0.elapsed());
                    }
                }
            } else {
                log::debug!("bg: config change (opacity/mode only, no reload) {:?}", _t0.elapsed());
            }
        }

        changes
    }

    pub(crate) fn reload_config(&mut self, egui_ctx: &Context) {
        match Config::reload() {
            Ok(Some(cfg)) => {
                log::info!("config reloaded, applying changes");
                self.apply_new_config(cfg, egui_ctx);
                self.settings_state.reset_to(&self.config);
                self.error_toast = None;
            }
            Ok(None) => {
                log::info!("config file removed, keeping current settings");
                self.error_toast = None;
            }
            Err(e) => {
                log::error!("config reload failed: {e}");
                self.error_toast = Some(format!("Config error — keeping old settings:\n{}", e));
            }
        }
    }
}
