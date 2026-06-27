//! Legacy single-terminal UI path.
//!
//! When `config.ui.tabs_enabled = false` (the Phase 1 default), the
//! `ZentermApp` does NOT allocate an `egui_dock::DockArea` — there
//! is no tab bar, no close button, no drag-to-reorder, no sidebar.
//! The application behaves exactly as it did in Phase 1: a single
//! `TerminalSession` (id 0) fills the central panel.
//!
//! This module is the rendering path for that mode.  It is a thin
//! wrapper around `Session::update_cell_instances` and
//! `Session::handle_mouse` that gives them a dock-relative origin
//! of `(0, 0)` and the central panel's available size as the dock
//! viewport — exactly the same coordinates the existing code used
//! before the multi-tab refactor.

use crate::session::{SessionId, TerminalSession};

/// Render the single-session UI inside the central panel.
///
/// The session is identified by [`SessionId(0)`]; if it does not
/// exist (which should never happen in `tabs_enabled = false` mode)
/// the function returns without drawing.
pub fn render_legacy_single(
    ui: &mut egui::Ui,
    sessions: &mut std::collections::HashMap<SessionId, TerminalSession>,
) {
    let session = match sessions.get_mut(&SessionId(0)) {
        Some(s) => s,
        None => return,
    };

    let available = ui.available_size();
    let ppp = ui.ctx().pixels_per_point();
    let size_px = [available.x * ppp, available.y * ppp];

    session.set_viewport([0.0, 0.0], size_px);

    // For the legacy single-session mode, the dock viewport is the
    // same as the session viewport (no multi-tab coordination needed).
    session.set_dock_viewport([0.0, 0.0], size_px);
    session.resize_to_viewport(size_px, ppp, ui.input(|i| i.time));
    session.update_cell_instances([0.0, 0.0], size_px);

    let sense = egui::Sense::click_and_drag();
    let (cell_rect, response) = ui.allocate_exact_size(available, sense);
    session.handle_mouse(ui, cell_rect, size_px, &response);

    // Legacy mode has one session → one callback → no cross-contamination.
    let callback =
        egui_wgpu::Callback::new_paint_callback(cell_rect, session.callback.clone());
    ui.painter().rect_filled(cell_rect, 0.0, session.default_bg);
    ui.painter().add(callback);

    session.render_context_menu(ui, &response);

    // Transient resize overlay (on top of terminal content).
    session.render_resize_overlay(ui, cell_rect);
}
