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
        let pty = zenterm_pty::PtySession::spawn(size).expect("failed to spawn PTY");
        let terminal = Terminal::new(size, scheme);

        let (cell_width, cell_height) = atlas.cell_size();

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
            cached_glyph: Vec::new(),
            cached_deco: Vec::new(),
            pending_title: None,
            preedit_text: None,
            scrollbar_dragging: false,
            scrollbar_drag_start_y: 0.0,
            scrollbar_drag_start_offset: 0,
        }
    }
}
