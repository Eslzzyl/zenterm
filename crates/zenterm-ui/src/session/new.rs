//! Terminal session construction.

use std::sync::Arc;

use zenterm_core::size::TermSize;
use zenterm_render::callback::CallbackHandle;
use zenterm_term::{ColorScheme, Terminal};

use super::types::{NotificationState, SessionId, TerminalSession};
use crate::glyph_cache::SharedGlyphAtlas;
use crate::gpu::SharedGpuContext;

impl TerminalSession {
    /// Construct a new session: spawn a PTY, initialise the terminal,
    /// measure cell geometry, and wire the wgpu callback.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: SessionId,
        size: TermSize,
        scheme: ColorScheme,
        blink_interval: u64,
        default_bg: egui::Color32,
        gpu: SharedGpuContext,
        atlas: Arc<SharedGlyphAtlas>,
        callback: CallbackHandle,
    ) -> Self {
        let mut pty = zenterm_pty::PtySession::spawn(size).expect("failed to spawn PTY");
        let mut terminal = Terminal::new(size, scheme);

        let (cell_width, cell_height) = atlas.cell_size();
        let cell_w = cell_width.ceil() as u32;
        let cell_h = cell_height.ceil() as u32;
        terminal.cell_pixel_width = cell_w;
        terminal.cell_pixel_height = cell_h;

        // Compute total pixel dimensions from initial cell size * rows/cols.
        let px_w = (size.cols as f32 * cell_width).ceil() as u16;
        let px_h = (size.rows as f32 * cell_height).ceil() as u16;
        terminal.pixel_width = px_w as u32;
        terminal.pixel_height = px_h as u32;
        // Propagate pixel dimensions to the PTY so TIOCGWINSZ reports them.
        if let Err(e) = pty.resize(TermSize::new(size.rows, size.cols, px_w, px_h)) {
            log::error!("failed to resize PTY with pixel dims: {e}");
        }

        // Initialise `last_vp_size_px` so the first render picks up the
        // resize correctly.  Starting at [0, 0] is fine; the first
        // `update_cell_instances` call will overwrite it.
        Self {
            id,
            title: format!("shell-{}", id.0),
            cwd: None,
            git_branch: None,
            notification: NotificationState::None,
            terminal,
            pty,
            gpu,
            atlas,
            callback,
            cell_width,
            cell_height,
            last_vp_size_px: [0.0, 0.0],
            last_vp_origin_px: [0.0, 0.0],
            dock_vp_origin_px: [0.0, 0.0],
            dock_vp_size_px: [0.0, 0.0],
            selecting: false,
            terminal_dirty: true,
            last_resize_at: None,
            frame_count: 0,
            blink_interval,
            pty_exited: false,
            exit_effect_sent: false,
            default_bg,
            cached_bg: Vec::new(),
            cached_glyph_per_atlas: Vec::new(),
            cached_deco: Vec::new(),
            cached_image_below: Vec::new(),
            cached_image_above: Vec::new(),
            pending_title: None,
            preedit_text: None,
            url_open: true,
            url_hover_underline: true,
            hover_cell: None,
            url_spans: Vec::new(),
            url_click_handled: false,
            scrollbar_dragging: false,
            scrollbar_drag_start_y: 0.0,
            scrollbar_drag_start_offset: 0,
            scroll_accumulator_y: 0.0,
        }
    }
}
