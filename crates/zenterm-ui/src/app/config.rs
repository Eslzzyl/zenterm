//! Config persistence and live reload.
//!
//! Debounced disk writes, window-size tracking, and runtime config application.

use std::time::{Duration, Instant};

use egui::Context;

use zenterm_config::Config;
use zenterm_core::SubpixelLayout;
use zenterm_term::ColorScheme;

use super::ZentermApp;
use super::theme::{configure_egui_style, system_theme_is_dark, theme_bg_to_color32};

impl ZentermApp {
    pub(crate) fn maybe_save_config(&mut self) {
        if !self.config_dirty {
            return;
        }
        const DEBOUNCE_MS: u64 = 500;
        if let Some(at) = self.last_config_save_at
            && at.elapsed() >= Duration::from_millis(DEBOUNCE_MS)
        {
            match self.config.save() {
                Ok(()) => self.config_dirty = false,
                Err(e) => log::error!("failed to save config: {e}"),
            }
            self.last_config_save_at = Some(Instant::now());
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
    pub(crate) fn apply_new_config(
        &mut self,
        new_config: Config,
        egui_ctx: &Context,
    ) -> zenterm_config::ConfigChanges {
        let old_config = std::mem::replace(&mut self.config, new_config);
        let changes = old_config.diff_to(&self.config);

        // Re-resolve theme.
        let system_dark = system_theme_is_dark(egui_ctx);
        self.theme = self.config.colors.to_theme(system_dark);
        self.last_system_dark = system_dark;
        self.default_bg = theme_bg_to_color32(&self.theme);
        // `self.theme` is replaced before the next frame's sync check, so
        // refresh egui immediately here instead of waiting for a theme
        // difference that can no longer be observed by `sync_theme`.
        configure_egui_style(egui_ctx, &self.theme);

        if changes.colors {
            let scheme = ColorScheme::from_theme(&self.theme);
            for session in self.sessions.values_mut() {
                session.terminal.set_scheme(scheme.clone());
                session.default_bg = self.default_bg;
            }
        }

        // Propagate selection config changes.
        if changes.selection {
            for session in self.sessions.values_mut() {
                session.save_to_clipboard = self.config.selection.save_to_clipboard;
            }
        }

        // Apply the scrollback limit to every existing session.  The
        // underlying terminal trims history immediately when it shrinks.
        if changes.terminal {
            for session in self.sessions.values_mut() {
                session
                    .terminal
                    .set_scrollback_lines(self.config.terminal.scrollback_lines);
                session.terminal_dirty = true;
            }
        }

        // Apply per-session config changes.
        if changes.font || changes.cursor || changes.colors {
            for session in self.sessions.values_mut() {
                session.apply_config_change(self.config.font.size, &self.config.cursor);
                session.terminal_dirty = true;
            }
        }

        // Rebuild the glyph atlas when any font property changes
        // (size, family, ligatures, hinting, etc.).
        if changes.font {
            let new_font_size = self.config.font.size * self.pixels_per_point;
            let font_family = std::borrow::Cow::Owned(self.config.font.normal.family.clone());
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
            for session in self.sessions.values_mut() {
                session.update_cell_metrics(cw, ch);
            }
        }

        // Handle background image changes.
        // Only re-decode the image when the path itself changes.
        // Image opacity and fit-mode changes are read live from config
        // in emit_background_quad() and do not require a reload.
        if changes.background {
            let _t0 = std::time::Instant::now();
            let old_path = old_config
                .background
                .image_path
                .as_deref()
                .unwrap_or("")
                .to_owned();
            let new_path = self
                .config
                .background
                .image_path
                .as_deref()
                .unwrap_or("")
                .to_owned();
            if old_path != new_path {
                // Clone the path before the mutable borrow.
                let path = self.config.background.image_path.clone();
                match path {
                    Some(p) if !p.is_empty() => {
                        self.load_background_image(&p);
                        log::debug!(
                            "bg: apply_new_config -> load_background_image took {:?}",
                            _t0.elapsed()
                        );
                    }
                    _ => {
                        // Clear the background image.
                        self.background_image_loaded = false;
                        self.loaded_bg_image_size = None;
                        self.gpu
                            .shared
                            .background_request_gen
                            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        *self
                            .gpu
                            .shared
                            .background_data
                            .lock()
                            .expect("background_data lock") = None;
                        log::debug!("bg: cleared (apply_new_config) {:?}", _t0.elapsed());
                    }
                }
            } else {
                log::debug!(
                    "bg: config change (image opacity/mode only, no reload) {:?}",
                    _t0.elapsed()
                );
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
