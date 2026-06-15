//! Multi-tab dock state.
//!
//! Wraps [`egui_dock::DockState<SessionId>`] with a monotonic id
//! allocator and a "layout changed" flag that the parent
//! [`crate::app::ZentermApp`] uses to trigger a debounced
//! `LayoutIo::save_dock`.
//!
//! # ID stability
//!
//! `SessionId`s are persistent across restarts when the dock layout
//! is restored.  Newly-allocated ids start at
//! `next_session_id` and increment.  When the user loads a saved
//! layout, the dock file's `next_session_id` value is used as the
//! starting point so the app never reuses an id that is still
//! referenced by a previous layout.

use std::time::Instant;

use egui_dock::DockState;

use crate::session::SessionId;

/// Mutable dock state plus change tracking.
pub struct TabsState {
    /// The dock tree; tab data is a [`SessionId`].
    pub dock: DockState<SessionId>,
    /// Next id to allocate via [`Self::new_session_id`].  Persisted
    /// across restarts so that newly-spawned sessions never collide
    /// with previously-allocated ones.
    pub next_session_id: u64,
    /// `true` if the dock has been mutated since the last persist.
    pub dirty: bool,
    /// Wall-clock time of the most recent mutation.  Used by the
    /// debounce timer in [`crate::app::ZentermApp`].
    pub last_change_at: Option<Instant>,
}

impl TabsState {
    /// Build a new state with no tabs.  Use [`Self::with_dock`] to
    /// start with a restored layout.
    pub fn empty() -> Self {
        Self {
            dock: DockState::new(vec![]),
            next_session_id: 0,
            dirty: false,
            last_change_at: None,
        }
    }

    /// Build a state from a previously-persisted dock and id
    /// counter.
    pub fn with_dock(dock: DockState<SessionId>, next_session_id: u64) -> Self {
        Self {
            dock,
            next_session_id,
            dirty: false,
            last_change_at: None,
        }
    }

    /// Allocate a new [`SessionId`] and mark the layout as dirty.
    pub fn new_session_id(&mut self) -> SessionId {
        let id = SessionId(self.next_session_id);
        self.next_session_id += 1;
        self.mark_changed();
        id
    }

    /// Allocate a new id and add a tab for it in the currently
    /// focused leaf.
    pub fn new_tab(&mut self) -> SessionId {
        let id = self.new_session_id();
        self.dock.push_to_focused_leaf(id);
        self.mark_changed();
        id
    }

    /// Drop the tab at the given path.  Returns `true` on success.
    pub fn close_tab(&mut self, tab_path: egui_dock::TabPath) -> bool {
        match self.dock.remove_tab(tab_path) {
            Some(_) => {
                self.mark_changed();
                true
            }
            None => {
                log::warn!("TabsState::close_tab: remove_tab returned None");
                false
            }
        }
    }

    /// Mark the dock as mutated.
    pub fn mark_changed(&mut self) {
        self.dirty = true;
        self.last_change_at = Some(Instant::now());
    }

    /// Collect every tab id currently in the dock.
    pub fn all_tab_ids(&self) -> Vec<SessionId> {
        self.dock
            .iter_all_tabs()
            .map(|(_, tab)| *tab)
            .collect()
    }
}
