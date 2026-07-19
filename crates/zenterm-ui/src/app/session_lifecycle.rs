//! Session lifecycle management — spawn, close, focus, pump PTY, handle side-effects.
//!
//! These methods are called from [`ZentermApp::update`](super::ZentermApp::update).

use egui::Context;

use zenterm_term::ColorScheme;

use crate::session::{SessionEffect, SessionId, TerminalSession};
use super::ZentermApp;

impl ZentermApp {
    /// Spawn a new session in the active workspace's currently focused
    /// dock leaf and return its id.
    pub(crate) fn spawn_session(&mut self) -> SessionId {
        let id = self.workspaces.new_session_id();
        let scheme = ColorScheme::from_theme(&self.theme);
        let size = zenterm_core::size::TermSize::new(
            self.config.window.dimensions.lines,
            self.config.window.dimensions.columns,
            0,
            0,
        );
        let session = TerminalSession::new(
            id,
            size,
            scheme,
            self.config.cursor.blink_interval,
            self.default_bg,
            self.gpu.clone(),
            self.atlas.clone(),
            self.callback.clone(),
            self.egui_ctx.clone(),
            self.config.window.opacity,
        );
        self.sessions.insert(id, session);
        self.workspaces.active_workspace_mut().new_tab(id);
        self.active_session_id = Some(id);
        self.mark_layout_dirty();
        id
    }

    /// Close a session and remove its tab from whichever workspace
    /// owns it.
    pub(crate) fn close_session(&mut self, id: SessionId) {
        // Remove the tab from the workspace that owns it.
        self.workspaces.remove_tab_from_any_workspace(id);

        // Drop the session (its `Drop` kills the PTY).
        self.sessions.remove(&id);

        // Re-focus: pick the first tab in the active workspace.
        if self.active_session_id == Some(id) {
            self.active_session_id = self
                .workspaces
                .active_workspace()
                .all_tab_ids()
                .first()
                .copied();
        }
        self.mark_layout_dirty();
    }

    /// Switch the active tab to the given `(node, tab)` pair in the
    /// active workspace.
    #[allow(dead_code)]
    pub(crate) fn focus_tab(&mut self, node: egui_dock::NodeIndex, tab: egui_dock::TabIndex) {
        let path = egui_dock::TabPath {
            surface: egui_dock::SurfaceIndex::main(),
            node,
            tab,
        };
        let ws = self.workspaces.active_workspace_mut();
        if ws.dock.set_active_tab(path).is_ok() {
            if let Some(tp) = ws.dock.iter_all_tabs().next() {
                self.active_session_id = Some(*tp.1);
            }
            ws.mark_changed();
        }
    }
}

impl ZentermApp {
    pub(crate) fn pump_pty_active_sessions(&mut self) {
        // Iterate a snapshot of ids to avoid borrow issues.
        let ids: Vec<SessionId> = self.sessions.keys().copied().collect();
        for id in ids {
            if let Some(session) = self.sessions.get_mut(&id) {
                session.pump_pty();
            }
        }
    }

    pub(crate) fn handle_side_effects(&mut self, ctx: &Context) {
        let ids: Vec<SessionId> = self.sessions.keys().copied().collect();
        let mut all_close = false;
        let mut window_title: Option<String> = None;
        let mut exit_ids: Vec<SessionId> = Vec::new();
        for id in ids {
            let effects = if let Some(s) = self.sessions.get_mut(&id) {
                s.tab_active = self.active_session_id == Some(id);
                s.handle_side_effects(ctx)
            } else {
                Vec::new()
            };
            for effect in effects {
                match effect {
                    SessionEffect::WindowTitle(t) => {
                        window_title = Some(t);
                    }
                    SessionEffect::CloseWindow => {
                        exit_ids.push(id);
                    }
                    SessionEffect::StealFocus => {
                        log::debug!("app: StealFocus requested by session {id:?}");
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                }
            }
        }
        // Only send ViewportCommand::Title when the value actually changes.
        // Calling [NSWindow setTitle:] with the same string every frame can
        // cause unnecessary title-bar redraws on some platforms (macOS).
        if let Some(t) = &window_title {
            if self.current_window_title.as_ref() != Some(t) {
                log::debug!(
                    "app: sending ViewportCommand::Title({:?}) (was {:?})",
                    t,
                    self.current_window_title,
                );
                self.current_window_title = Some(t.clone());
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(t.clone()));
                // Ensure the UI re-renders so the tab bar shows the new
                // title.  The session's `self.title` was already updated
                // in `TerminalSession::handle_side_effects`; the tab bar
                // won't reflect it until the next frame.
                ctx.request_repaint();
            } else {
                log::trace!("app: window title unchanged ({:?}), skipping ViewportCommand", t);
            }
        }
        // Handle shell-exited sessions.
        // Close the tab for any session whose shell exited.
        // If the workspace becomes empty, close it.
        // If no workspace has any tabs left, close the application.

        // 1. Remember which workspace each exiting session belongs to
        //    (must be done before close_session removes the tab).
        let mut exit_ws_ids: Vec<crate::workspace::WorkspaceId> = Vec::new();
        for id in &exit_ids {
            if let Some(ws) = self.workspaces.find_tab_workspace(*id) {
                exit_ws_ids.push(ws.id);
            }
        }

        // 2. Close the sessions.
        for id in &exit_ids {
            self.close_session(*id);
        }

        // 3. Close workspaces that are now empty (keep at least one).
        for ws_id in &exit_ws_ids {
            if let Some(ws) = self.workspaces.find_workspace(*ws_id) {
                if ws.all_tab_ids().is_empty() && self.workspaces.workspaces.len() > 1 {
                    log::info!(
                        "handle_side_effects: workspace '{}' is empty, closing it",
                        ws.name,
                    );
                    self.workspaces.close_workspace(*ws_id);
                }
            }
        }

        // 4. If no workspace has any tabs left, close the application.
        if !exit_ids.is_empty() {
            let any_tabs = self
                .workspaces
                .workspaces
                .iter()
                .any(|ws| !ws.all_tab_ids().is_empty());
            if !any_tabs {
                all_close = true;
            }
        }
        if all_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
