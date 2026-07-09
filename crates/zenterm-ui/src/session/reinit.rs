//! Viewport, DPI, resize, and configuration methods for [`TerminalSession`].

use zenterm_core::size::TermSize;

use super::types::TerminalSession;

impl TerminalSession {
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
    pub fn reinit_for_dpi(&mut self, new_ppp: f32, ligatures_enabled: bool) {
        let new_font_size = self.config_font_size() * new_ppp;
        let font_family = std::borrow::Cow::Owned(self.config_font_family());
        let (cw, ch) = self.atlas.reinit_for_dpi(
            new_font_size,
            font_family,
            new_ppp,
            zenterm_core::SubpixelLayout::detect(),
            ligatures_enabled,
            zenterm_core::HintingMode::Auto,
            zenterm_core::RenderMode::Subpixel,
        );
        self.atlas.seed_ascii();
        // Ensure the seeded atlas reaches the GPU before the next prepare().
        self.atlas.sync_to_gpu();
        self.cell_width = cw;
        self.cell_height = ch;
        self.terminal.cell_pixel_width = cw.ceil() as u32;
        self.terminal.cell_pixel_height = ch.ceil() as u32;
        self.terminal_dirty = true;
        log::info!(
            "DPI reinit: session={} new_ppp={new_ppp:.2} font_size={new_font_size:.1} \
             cw={cw:.1} ch={ch:.1}",
            self.id.0
        );
    }

    /// Forward `apply_config_change`-style updates to per-session state.
    pub fn apply_config_change(&mut self, font_size: f32, blink_interval: u64) {
        if blink_interval != self.blink_interval {
            self.blink_interval = blink_interval;
        }
        // Font size changes that don't cross a DPI threshold are
        // ignored here: `reinit_for_dpi` handles the physical rebuild.
        let _ = font_size;
    }

    /// Read the configured font size (the session does not own a
    /// `Config`; the parent `ZentermApp` injects values via the
    /// `apply_config_change` method).
    fn config_font_size(&self) -> f32 {
        // Conservative fallback: a real implementation would
        // re-thread the Config through to the session.  For now,
        // the parent calls `reinit_for_dpi` directly when the config
        // changes; `apply_config_change` is the lightweight path.
        18.0
    }
    fn config_font_family(&self) -> String {
        "monospace".to_string()
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
        let new_size = TermSize::new(rows, cols);
        if new_size != self.terminal.size() {
            self.terminal.resize(new_size);
            self.pty.resize(new_size).ok();
            self.terminal_dirty = true;
            self.last_resize_at = Some(time);
        }
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
        if elapsed < 0.0 || elapsed >= 2.0 {
            return;
        }

        let size = self.terminal.size();
        let text = format!("{} × {}", size.cols, size.rows);

        // Choose backdrop and text colours based on terminal background
        // luminance so the overlay is legible on both light and dark
        // terminals.
        let bg = self.default_bg;
        let lum = 0.299 * bg.r() as f32 / 255.0
            + 0.587 * bg.g() as f32 / 255.0
            + 0.114 * bg.b() as f32 / 255.0;
        let (backdrop_color, text_color) = if lum < 0.5 {
            // Dark terminal → light backdrop with dark text.
            (
                egui::Color32::WHITE.gamma_multiply(0.55),
                egui::Color32::BLACK,
            )
        } else {
            // Light terminal → dark backdrop with light text.
            (
                egui::Color32::BLACK.gamma_multiply(0.55),
                egui::Color32::WHITE,
            )
        };

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
