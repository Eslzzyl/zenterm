//! Workspace abstraction layer.
//!
//! A **workspace** is a named grouping of terminal tabs.  Each
//! workspace owns its own [`egui_dock::DockState<SessionId>`] so
//! that different workspaces can have independent tab layouts.
//!
//! [`WorkspaceManager`] sits between [`crate::app::ZentermApp`] and
//! the individual [`WorkspaceState`] instances.  It owns the global
//! [`SessionId`] counter (monotonically increasing across all
//! workspaces) and the list of workspaces plus the active-workspace
//! pointer.
//!
//! # ID stability
//!
//! Both [`WorkspaceId`] and [`SessionId`] are stable across restarts.
//! The manager persists `next_session_id` and `next_workspace_id`
//! through [`crate::layout_io`].

use std::time::Instant;

use egui_dock::DockState;
use serde::{Deserialize, Serialize};

use crate::session::SessionId;

// ── WorkspaceId ─────────────────────────────────────────────────────────

/// Unique identifier for a workspace.  Monotonically increasing;
/// allocated by [`WorkspaceManager`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WorkspaceId(pub u64);

impl WorkspaceId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
}

// ── WorkspaceState ──────────────────────────────────────────────────────

/// All state for a single workspace.
pub struct WorkspaceState {
    /// Unique identifier (stable across restarts).
    pub id: WorkspaceId,
    /// Human-readable name (e.g. "work", "dev").
    pub name: String,
    /// The dock tree for this workspace.  Tab data is a [`SessionId`].
    pub dock: DockState<SessionId>,
    /// `true` if the dock has been mutated since the last persist.
    pub dirty: bool,
    /// Wall-clock time of the most recent mutation (for debounce).
    pub last_change_at: Option<Instant>,
}

impl WorkspaceState {
    /// Create a fresh workspace with no tabs.
    pub fn new(id: WorkspaceId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            dock: DockState::new(vec![]),
            dirty: false,
            last_change_at: None,
        }
    }

    /// Create a workspace from a previously-persisted dock layout.
    pub fn from_dock(id: WorkspaceId, name: String, dock: DockState<SessionId>) -> Self {
        Self {
            id,
            name,
            dock,
            dirty: false,
            last_change_at: None,
        }
    }

    /// Add a tab to the currently focused leaf.  Returns the new
    /// [`SessionId`].
    pub fn new_tab(&mut self, session_id: SessionId) {
        self.dock.push_to_focused_leaf(session_id);
        self.mark_changed();
    }

    /// Drop the tab at the given path.  Returns `true` on success.
    pub fn close_tab(&mut self, tab_path: egui_dock::TabPath) -> bool {
        if self.dock.remove_tab(tab_path).is_some() {
            self.mark_changed();
            true
        } else {
            log::warn!("WorkspaceState::close_tab: remove_tab returned None");
            false
        }
    }

    /// Collect every tab id in this workspace's dock.
    pub fn all_tab_ids(&self) -> Vec<SessionId> {
        self.dock.iter_all_tabs().map(|(_, tab)| *tab).collect()
    }

    /// Mark the dock as mutated.
    pub fn mark_changed(&mut self) {
        self.dirty = true;
        self.last_change_at = Some(Instant::now());
    }
}

// ── WorkspaceManager ────────────────────────────────────────────────────

/// Top-level manager for all workspaces.
///
/// Owns the workspace list, the active-workspace pointer, and the
/// global monotonic session-ID allocator.
pub struct WorkspaceManager {
    /// All workspaces (never empty after construction).
    pub workspaces: Vec<WorkspaceState>,
    /// The currently active workspace.
    pub active_workspace_id: WorkspaceId,
    /// Next session id to allocate.  Global (shared across all
    /// workspaces) so that session ids are unique application-wide.
    pub next_session_id: u64,
    /// Next workspace id to allocate.
    pub next_workspace_id: u64,
}

impl WorkspaceManager {
    /// Create a manager with a single "default" workspace.
    pub fn new() -> Self {
        let default_ws_id = WorkspaceId(0);
        Self {
            workspaces: vec![WorkspaceState::new(default_ws_id, "default")],
            active_workspace_id: default_ws_id,
            next_session_id: 1,
            next_workspace_id: 1,
        }
    }

    /// Restore from persisted state (used during startup).
    pub fn from_persisted(
        workspaces: Vec<WorkspaceState>,
        active_workspace_id: WorkspaceId,
        next_session_id: u64,
        next_workspace_id: u64,
    ) -> Self {
        debug_assert!(!workspaces.is_empty());
        Self {
            workspaces,
            active_workspace_id,
            next_session_id,
            next_workspace_id,
        }
    }

    // ── Accessors ──────────────────────────────────────────────────

    /// Immutable reference to the active workspace.
    pub fn active_workspace(&self) -> &WorkspaceState {
        self.workspaces
            .iter()
            .find(|ws| ws.id == self.active_workspace_id)
            .expect("active workspace must exist")
    }

    /// Mutable reference to the active workspace.
    pub fn active_workspace_mut(&mut self) -> &mut WorkspaceState {
        self.workspaces
            .iter_mut()
            .find(|ws| ws.id == self.active_workspace_id)
            .expect("active workspace must exist")
    }

    /// Find a workspace by id (immutable).
    pub fn find_workspace(&self, id: WorkspaceId) -> Option<&WorkspaceState> {
        self.workspaces.iter().find(|ws| ws.id == id)
    }

    /// Find a workspace by id (mutable).
    pub fn find_workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut WorkspaceState> {
        self.workspaces.iter_mut().find(|ws| ws.id == id)
    }

    // ── Session ID allocation ──────────────────────────────────────

    /// Allocate a new globally-unique [`SessionId`].
    pub fn new_session_id(&mut self) -> SessionId {
        let id = SessionId(self.next_session_id);
        self.next_session_id += 1;
        id
    }

    // ── Workspace operations ───────────────────────────────────────

    /// Switch to the workspace with the given id.
    /// Returns `false` if the id was not found.
    pub fn switch_to(&mut self, id: WorkspaceId) -> bool {
        if self.workspaces.iter().any(|ws| ws.id == id) {
            self.active_workspace_id = id;
            true
        } else {
            false
        }
    }

    /// Create a new workspace and switch to it.  Returns its id.
    pub fn create_workspace(&mut self, name: impl Into<String>) -> WorkspaceId {
        let id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id += 1;
        let ws = WorkspaceState::new(id, name);
        self.workspaces.push(ws);
        self.active_workspace_id = id;
        id
    }

    /// Rename a workspace.  Returns `false` if the id was not found.
    pub fn rename_workspace(&mut self, id: WorkspaceId, new_name: impl Into<String>) -> bool {
        if let Some(ws) = self.find_workspace_mut(id) {
            ws.name = new_name.into();
            true
        } else {
            false
        }
    }

    /// Close a workspace.  Tabs in the closed workspace are migrated
    /// to the previous workspace (or the next one if it was the first).
    /// The active workspace pointer is updated if the closed workspace
    /// was active.
    ///
    /// Returns `false` if there is only one workspace left (refuse to
    /// close the last one) or the id was not found.
    pub fn close_workspace(&mut self, id: WorkspaceId) -> bool {
        if self.workspaces.len() <= 1 {
            return false;
        }
        let idx = match self.workspaces.iter().position(|ws| ws.id == id) {
            Some(i) => i,
            None => return false,
        };

        // Determine where to migrate tabs.
        let migrate_to = if idx > 0 { idx - 1 } else { idx + 1 };

        // Collect session ids from the workspace being closed.
        let tabs: Vec<SessionId> = self.workspaces[idx].all_tab_ids();

        // Migrate tabs into the target workspace.
        for tab_id in tabs {
            self.workspaces[migrate_to].dock.push_to_focused_leaf(tab_id);
            self.workspaces[migrate_to].mark_changed();
        }

        self.workspaces.remove(idx);

        // Fix the active workspace pointer.
        if self.active_workspace_id == id {
            self.active_workspace_id = self.workspaces
                .get(migrate_to.min(self.workspaces.len() - 1))
                .unwrap()
                .id;
        }

        true
    }

    // ── Tab-level helpers ──────────────────────────────────────────

    /// Find which workspace owns a given session tab.
    pub fn find_tab_workspace(&self, session_id: SessionId) -> Option<&WorkspaceState> {
        self.workspaces
            .iter()
            .find(|ws| ws.all_tab_ids().contains(&session_id))
    }

    /// Remove a session tab from whichever workspace owns it.
    /// Returns `true` if the tab was found and removed.
    pub fn remove_tab_from_any_workspace(&mut self, session_id: SessionId) -> bool {
        for ws in &mut self.workspaces {
            let path = ws.dock.find_tab(&session_id);
            if let Some(path) = path {
                if ws.dock.remove_tab(path).is_some() {
                    ws.mark_changed();
                    return true;
                }
            }
        }
        false
    }

    /// Collect every session id across all workspaces.
    pub fn all_tab_ids(&self) -> Vec<SessionId> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.all_tab_ids())
            .collect()
    }

    /// Mark all workspaces' layout as dirty.
    pub fn mark_all_changed(&mut self) {
        for ws in &mut self.workspaces {
            ws.mark_changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_manager_has_default_workspace() {
        let mgr = WorkspaceManager::new();
        assert_eq!(mgr.workspaces.len(), 1);
        assert_eq!(mgr.active_workspace().name, "default");
    }

    #[test]
    fn create_and_switch_workspace() {
        let mut mgr = WorkspaceManager::new();
        let dev_id = mgr.create_workspace("dev");
        assert_eq!(mgr.workspaces.len(), 2);
        assert_eq!(mgr.active_workspace().name, "dev");

        let ws0_id = mgr.workspaces[0].id;
        assert!(mgr.switch_to(ws0_id));
        assert_eq!(mgr.active_workspace().name, "default");
        assert!(mgr.switch_to(dev_id));
        assert_eq!(mgr.active_workspace().name, "dev");
    }

    #[test]
    fn close_workspace_migrates_tabs() {
        let mut mgr = WorkspaceManager::new();
        let ws0_id = mgr.workspaces[0].id;
        let dev_id = mgr.create_workspace("dev");

        // Allocate two sessions and add them to "dev".
        let s1 = mgr.new_session_id();
        let s2 = mgr.new_session_id();
        mgr.active_workspace_mut().new_tab(s1);
        mgr.active_workspace_mut().new_tab(s2);

        assert!(mgr.close_workspace(dev_id));
        assert_eq!(mgr.workspaces.len(), 1);
        assert_eq!(mgr.active_workspace_id, ws0_id);

        // Tabs should have migrated to "default".
        let tabs = mgr.active_workspace().all_tab_ids();
        assert!(tabs.contains(&s1));
        assert!(tabs.contains(&s2));
    }

    #[test]
    fn cannot_close_last_workspace() {
        let mut mgr = WorkspaceManager::new();
        let last_id = mgr.workspaces[0].id;
        assert!(!mgr.close_workspace(last_id));
        assert_eq!(mgr.workspaces.len(), 1);
    }

    #[test]
    fn session_ids_are_global() {
        let mut mgr = WorkspaceManager::new();
        let _ = mgr.create_workspace("dev");

        let s1 = mgr.new_session_id();
        let s2 = mgr.new_session_id();
        assert_ne!(s1, s2);
        assert_eq!(s1.raw(), 1);
        assert_eq!(s2.raw(), 2);
    }

    #[test]
    fn rename_workspace() {
        let mut mgr = WorkspaceManager::new();
        let id = mgr.workspaces[0].id;
        assert!(mgr.rename_workspace(id, "my-project"));
        assert_eq!(mgr.active_workspace().name, "my-project");
    }

    #[test]
    fn find_tab_workspace() {
        let mut mgr = WorkspaceManager::new();
        let ws0_id = mgr.workspaces[0].id;
        let dev_id = mgr.create_workspace("dev");

        let s1 = mgr.new_session_id();
        mgr.active_workspace_mut().new_tab(s1);

        let found = mgr.find_tab_workspace(s1);
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, dev_id);

        // Switch back, add another tab.
        mgr.switch_to(ws0_id);
        let s2 = mgr.new_session_id();
        mgr.active_workspace_mut().new_tab(s2);

        let found = mgr.find_tab_workspace(s2);
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, ws0_id);
    }

    #[test]
    fn remove_tab_from_any_workspace() {
        let mut mgr = WorkspaceManager::new();
        let s1 = mgr.new_session_id();
        mgr.active_workspace_mut().new_tab(s1);
        assert!(mgr.remove_tab_from_any_workspace(s1));
        assert!(mgr.active_workspace().all_tab_ids().is_empty());
    }
}
