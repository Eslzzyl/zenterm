//! `egui_dock::TabViewer` implementation for the Zenterm app.
//!
//! This is the **bridge** between egui_dock's per-tab rendering API
//! and our [`TerminalSession`]-based architecture.  When egui_dock
//! asks to render a tab whose data is a [`SessionId`], we look up
//! the corresponding session in the app's session map and ask it to
//! draw itself with the appropriate dock-relative viewport.
//!
//! # What this file does NOT do
//!
//! - It does **not** spawn or close sessions.  The actual session
//!   lifecycle is handled by the parent `ZentermApp`, which receives
//!   `pending_new` and `pending_close` queues from this viewer and
//!   applies them after `DockArea::show_inside` returns.  This keeps
//!   the borrow checker happy: `TabViewer::ui` cannot mutate the
//!   session map while the `DockArea` is iterating it.

use egui::{Stroke, StrokeKind, WidgetText};
use egui_dock::tab_viewer::OnCloseResponse;
use egui_dock::{NodePath, TabViewer};

use crate::session::{SessionId, TerminalSession};

/// Borrowed handles needed to render a tab.
pub struct TabViewerContext<'a> {
    pub sessions: &'a mut std::collections::HashMap<SessionId, TerminalSession>,
    pub active_session_id: &'a mut Option<SessionId>,
    /// IDs of sessions to close after `DockArea::show_inside` returns.
    pub pending_close: &'a mut Vec<SessionId>,
    /// Counts `on_add` invocations that should result in a new tab.
    pub pending_adds: &'a mut u32,
    /// Whether to draw the active-panel border indicator.
    /// Should be `false` when there is only a single tab (no split).
    pub show_active_indicator: bool,
}

impl<'a> TabViewer for TabViewerContext<'a> {
    type Tab = SessionId;

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        let base = self
            .sessions
            .get(tab)
            .map(|s| s.title.clone())
            .unwrap_or_else(|| format!("(missing #{})", tab.0));

        // Append ConEmu progress indicator if non-default.
        match self.sessions.get(tab).map(|s| s.progress) {
            Some(zenterm_core::Progress::Percentage(p)) => format!("{base} {p}%"),
            Some(zenterm_core::Progress::Error(p)) => format!("{base} !{p}%"),
            Some(zenterm_core::Progress::Indeterminate) => format!("{base} ~"),
            _ => base,
        }
        .into()
    }

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(("zenterm_session", tab.0))
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        // Look up the session.  If the dock references an id we no
        // longer have (e.g. layout was restored after a session was
        // dropped), render a placeholder.
        let session = match self.sessions.get_mut(tab) {
            Some(s) => s,
            None => {
                ui.centered_and_justified(|ui| {
                    ui.label(format!("(session #{} no longer exists)", tab.0));
                });
                return;
            }
        };

        // Compute the session's dock-relative pixel rectangle and
        // build GPU instance data appended to the shared buffer.
        // Cell clip coordinates use the dock-area viewport (set by
        // the app before `DockArea::show_inside`) so a single wgpu
        // callback covering the entire dock area renders every tab.
        let rect = ui.max_rect();
        let ppp = ui.ctx().pixels_per_point();
        let origin_px = [rect.min.x * ppp, rect.min.y * ppp];
        let size_px = [rect.size().x * ppp, rect.size().y * ppp];
        session.set_viewport(origin_px, size_px);

        // Resize the terminal to match the new pixel area.
        session.resize_to_viewport(size_px, ppp, ui.input(|i| i.time));

        // Compute hover BEFORE cell-instance building so URL underline
        // is rendered on the correct frame (not one frame behind).
        session.compute_hover(ui, rect);

        // Build GPU instance data and append to the shared instance
        // buffer.  Instances are positioned in the dock-area clip
        // space set via `set_dock_viewport`.
        session.update_cell_instances(origin_px, size_px);

        // Allocate the terminal area and run mouse / SGR / context-menu.
        let sense = egui::Sense::click_and_drag();
        let (cell_rect, response) = ui.allocate_exact_size(ui.available_size(), sense);
        log::info!(
            "[dbg] tab_viewer: calling handle_mouse for tab={:?}, active={:?}, rect={:?}, event_count={}",
            tab, self.active_session_id, cell_rect,
            ui.ctx().input(|i| i.events.len()),
        );
        session.handle_mouse(ui, cell_rect, size_px, &response);

        // Paint the terminal background (egui shape, not wgpu callback).
        // The wgpu callback for cell instances is registered once at the
        // dock level (see `render_tabs_with_dock` in `app.rs`).
        ui.painter().rect_filled(cell_rect, 0.0, session.default_bg);

        // Scrollbar overlay (on top of the background).
        session.render_scrollbar(ui, cell_rect);

        // Right-click context menu: copy / paste.
        session.render_context_menu(ui, &response);

        // Switch keyboard focus to this tab only when the user actively
        // interacts with the terminal body (click, drag, right-click).
        // Without this guard, the last tab rendered in a split layout
        // unconditionally becomes active, stealing keyboard input from
        // the tab the user actually clicked on.
        if response.clicked() || response.drag_started() || response.secondary_clicked() {
            *self.active_session_id = Some(*tab);
        }

        // Visual indicator: draw a thin border around the active
        // (keyboard-focus) panel when the layout has more than one
        // tab (split or tabbed).  The colour comes from the terminal's
        // selection background, which is designed to be visible on any
        // foreground/background combination.
        if self.show_active_indicator && *self.active_session_id == Some(*tab) {
            let sel = session.terminal.scheme().selection_bg;
            let accent = egui::Color32::from_rgba_premultiplied(
                (sel.r() * 255.0).round().clamp(0.0, 255.0) as u8,
                (sel.g() * 255.0).round().clamp(0.0, 255.0) as u8,
                (sel.b() * 255.0).round().clamp(0.0, 255.0) as u8,
                (sel.a() * 255.0).round().clamp(0.0, 255.0) as u8,
            );
            ui.painter().rect_stroke(cell_rect, 0.0, Stroke::new(1.0, accent), StrokeKind::Inside);
        }
    }

    fn on_close(&mut self, tab: &mut Self::Tab) -> OnCloseResponse {
        self.pending_close.push(*tab);
        OnCloseResponse::Close
    }

    fn on_tab_button(&mut self, tab: &mut Self::Tab, response: &egui::Response) {
        if response.clicked() {
            *self.active_session_id = Some(*tab);
        }
    }

    fn on_add(&mut self, _path: NodePath) {
        *self.pending_adds += 1;
    }

    fn allowed_in_windows(&self, _tab: &mut Self::Tab) -> bool {
        // PTY sessions are tied to the master process; never allow
        // them to be dragged into standalone `egui::Window`s.
        false
    }

    fn scroll_bars(&self, _tab: &Self::Tab) -> [bool; 2] {
        // The terminal renders its own scrollback; egui_dock should
        // not add a scroll bar on top of it.
        [false, false]
    }

    fn clear_background(&self, _tab: &Self::Tab) -> bool {
        // We paint the terminal background ourselves via
        // `ui.painter().rect_filled`, so don't let egui_dock
        // overwrite it with the tab bar's fill colour.
        false
    }
}
