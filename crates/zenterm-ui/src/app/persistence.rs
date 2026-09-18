//! Layout persistence — marks, debounce, and save/load dock layout to disk.

use std::time::{Duration, Instant};

use super::ZentermApp;
use crate::layout_io::{PersistedLayout, PersistedWorkspace, SCHEMA_VERSION, SessionMeta};

impl ZentermApp {
    // ── Layout persistence ────────────────────────────────────────

    pub(crate) fn mark_layout_dirty(&mut self) {
        self.layout_dirty = true;
    }

    /// If the layout has been mutated longer than the configured
    /// debounce window ago, write it to disk.
    pub(crate) fn maybe_persist_layout(&mut self) {
        if !self.layout_dirty {
            return;
        }
        if !self.config.ui.persist_layout {
            self.layout_dirty = false;
            return;
        }
        let debounce = Duration::from_millis(self.config.ui.layout_debounce_ms);
        if let Some(at) = self.last_persist_at
            && at.elapsed() < debounce
        {
            return;
        }
        self.persist_layout_now();
    }

    /// Force a layout write.  Called from [`Self::on_exit`] and from
    /// [`Self::maybe_persist_layout`] after the debounce window.
    pub(crate) fn persist_layout_now(&mut self) {
        let persisted = PersistedLayout {
            version: SCHEMA_VERSION,
            active_workspace_id: self.workspaces.active_workspace_id.raw(),
            next_session_id: self.workspaces.next_session_id,
            next_workspace_id: self.workspaces.next_workspace_id,
            workspaces: self
                .workspaces
                .workspaces
                .iter()
                .map(|ws| PersistedWorkspace {
                    id: ws.id.raw(),
                    name: ws.name.clone(),
                    dock: ws.dock.clone(),
                })
                .collect(),
        };
        let layout_saved = match self.layout_io.save_layout(&persisted) {
            Ok(()) => true,
            Err(e) => {
                log::error!("persist_layout_now: save_layout failed: {e}");
                false
            }
        };
        let metas: Vec<SessionMeta> = self
            .sessions
            .iter()
            .map(|(id, s)| {
                let ws_id = self
                    .workspaces
                    .find_tab_workspace(*id)
                    .map(|ws| ws.id.raw());
                SessionMeta {
                    id: id.0,
                    title: zenterm_pty::normalize_extended_path_text(s.title()),
                    title_override: s.title_override().map(str::to_owned),
                    cwd: s.working_directory().map(zenterm_pty::simplified_path),
                    shell: s.shell_path().map(zenterm_pty::simplified_path),
                    workspace_id: ws_id,
                }
            })
            .collect();
        let sessions_saved = match self.layout_io.save_sessions(&metas) {
            Ok(()) => true,
            Err(e) => {
                log::error!("persist_layout_now: save_sessions failed: {e}");
                false
            }
        };
        if layout_saved && sessions_saved {
            self.layout_dirty = false;
            self.last_persist_at = Some(Instant::now());
        }
    }
}
