//! Viewport, DPI, resize, and configuration methods for [`TerminalSession`].

use zenterm_config::{cursor::CursorConfig, font::FontConfig};
use zenterm_core::size::TermSize;

use super::types::TerminalSession;
use zenterm_term::CursorPrefs;

impl TerminalSession {
    /// Apply the current atlas cell metrics to both the UI and terminal
    /// layers.  This is also used for font hot-reload, where the grid size
    /// may stay unchanged and `Terminal::resize` would otherwise be skipped.
    pub(crate) fn update_cell_metrics(&mut self, cell_width: f32, cell_height: f32) {
        self.cell_width = cell_width;
        self.cell_height = cell_height;

        let cell_pixel_width = cell_width.ceil() as u32;
        let cell_pixel_height = cell_height.ceil() as u32;
        self.terminal
            .set_cell_pixel_size(cell_pixel_width, cell_pixel_height);

        if let Err(error) = self.pty.resize(self.terminal.size()) {
            log::warn!(
                "failed to propagate cell metrics for session {} to PTY: {error}",
                self.id.0
            );
        }
        self.terminal_dirty = true;
    }

    // ── Viewport (dock) helpers ─────────────────────────────────────

    /// Update the session's tracked viewport.  Called by the
    /// `TabViewer::ui` implementation before the session draws.
    pub fn set_viewport(&mut self, origin_px: [f32; 2], size_px: [f32; 2]) {
        if self.last_vp_origin_px != origin_px || self.last_vp_size_px != size_px {
            self.last_vp_origin_px = origin_px;
            self.last_vp_size_px = size_px;
            self.terminal_dirty = true;
        }
    }

    /// Set the dock-area viewport for the single-callback coordinate
    /// system.  All sessions share the same dock viewport; cell clip
    /// positions are computed relative to this rect so a single wgpu
    /// callback can render every tab.
    ///
    /// Must be called before `update_cell_instances` each frame.
    pub fn set_dock_viewport(&mut self, origin_px: [f32; 2], size_px: [f32; 2]) {
        if self.dock_vp_origin_px != origin_px || self.dock_vp_size_px != size_px {
            self.dock_vp_origin_px = origin_px;
            self.dock_vp_size_px = size_px;
            self.terminal_dirty = true;
        }
    }

    // ── DPI reinit ──────────────────────────────────────────────────

    /// Re-initialise the (shared) glyph atlas and cell metrics for a
    /// new DPI scale factor.  Called when the window moves between
    /// monitors with different DPI settings.
    pub fn reinit_for_dpi(&mut self, new_ppp: f32, font_config: &FontConfig) {
        let new_font_size = font_config.size * new_ppp;
        let font_family = std::borrow::Cow::Owned(font_config.normal.family.clone());
        let (cw, ch) = self.atlas.reinit_for_dpi(
            new_font_size,
            font_family,
            new_ppp,
            zenterm_core::SubpixelLayout::detect(),
            font_config.ligatures,
            font_config.hinting,
            font_config.render_mode,
        );
        self.atlas.seed_ascii();
        // Ensure the seeded atlas reaches the GPU before the next prepare().
        self.atlas.sync_to_gpu();
        self.update_cell_metrics(cw, ch);
        log::info!(
            "DPI reinit: session={} new_ppp={new_ppp:.2} font_size={new_font_size:.1} \
             cw={cw:.1} ch={ch:.1}",
            self.id.0
        );
    }

    /// Forward `apply_config_change`-style updates to per-session state.
    pub fn apply_config_change(&mut self, font_size: f32, cursor: &CursorConfig) {
        let mut dirty = false;
        if cursor.blink_interval != self.blink_interval {
            self.blink_interval = cursor.blink_interval;
            dirty = true;
        }
        if cursor.blink_timeout != self.blink_timeout {
            self.blink_timeout = cursor.blink_timeout;
            dirty = true;
        }
        if cursor.thickness != self.cursor_thickness {
            self.cursor_thickness = cursor.thickness;
            dirty = true;
        }
        if cursor.unfocused_hollow != self.unfocused_hollow {
            self.unfocused_hollow = cursor.unfocused_hollow;
            dirty = true;
        }
        self.terminal.set_cursor_prefs(CursorPrefs {
            shape: Self::map_cursor_shape(cursor.style.shape),
            blink: Self::map_blink_policy(cursor.style.blinking),
        });
        if dirty {
            self.blink_epoch = std::time::Instant::now();
            self.terminal_dirty = true;
        }
        // Font size changes that don't cross a DPI threshold are
        // ignored here: `reinit_for_dpi` handles the physical rebuild.
        let _ = font_size;
    }

    // ── Per-session rendering ────────────────────────────────────────

    /// Resize the terminal to fit a dock-relative pixel area.
    ///
    /// `time` is the current UI time (from `ui.input(|i| i.time)`) used to
    /// timestamp the resize so the transient size overlay can fade out.
    pub fn resize_to_viewport(&mut self, size_px: [f32; 2], ppp: f32, time: f64) {
        let vp_width_px = size_px[0];
        let vp_height_px = size_px[1];
        if vp_width_px <= 0.0 || vp_height_px <= 0.0 {
            return;
        }
        let cols = (vp_width_px / self.cell_width).max(10.0) as u16;
        let rows = (vp_height_px / self.cell_height).max(5.0) as u16;
        let current = self.terminal.size();
        if rows == current.rows && cols == current.cols {
            return;
        }
        let pixel_width = (cols as f32 * self.cell_width) as u16;
        let pixel_height = (rows as f32 * self.cell_height) as u16;
        let new_size = TermSize::new(rows, cols, pixel_width, pixel_height);
        self.terminal.resize(new_size);
        self.pty.resize(new_size).ok();
        self.terminal_dirty = true;
        self.last_resize_at = Some(time);
        let _ = ppp;
    }

    /// Render the transient resize overlay — a centered "cols × rows"
    /// label with a semi-transparent backdrop that appears for ~2 s
    /// after the most recent terminal resize, then disappears abruptly.
    ///
    /// The backdrop colour adapts to the terminal background: a dark
    /// backdrop with light text for light terminals, and a light backdrop
    /// with dark text for dark terminals, ensuring it's always readable.
    ///
    /// This method is a no-op if no resize has occurred within the
    /// display window.  Call it **after** the terminal content has been
    /// painted so the overlay appears on top.
    pub fn render_resize_overlay(&self, ui: &egui::Ui, rect: egui::Rect) {
        let last_time = match self.last_resize_at {
            Some(t) => t,
            None => return,
        };
        let now = ui.input(|i| i.time);
        let elapsed = (now - last_time) as f32;
        if !(0.0..2.0).contains(&elapsed) {
            return;
        }

        let size = self.terminal.size();
        let text = format!("{} × {}", size.cols, size.rows);

        // Use the active egui theme so the transient overlay belongs to
        // the same visual system as the rest of the application chrome.
        let backdrop_color = ui.visuals().window_fill;
        let text_color = ui.visuals().strong_text_color();

        // Semi-transparent rounded backdrop, centred in `rect`.
        let backdrop = egui::Rect::from_center_size(rect.center(), egui::vec2(160.0, 44.0));
        ui.painter().rect_filled(backdrop, 8.0, backdrop_color);

        // Text label (fully opaque during the display window).
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(20.0),
            text_color,
        );
    }
}
