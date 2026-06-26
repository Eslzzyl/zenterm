//! Workspace sidebar — cmux-style vertical tab list.
//!
//! When `config.ui.sidebar_enabled = true` (and `tabs_enabled = true`),
//! this is rendered as a [`egui::SidePanel`] on the left edge of the
//! window.  Each entry shows the session's title, working directory
//! (parsed from OSC 7), and an active-tab indicator.  A `+ New shell`
//! button at the top spawns a new [`TerminalSession`] in the
//! currently focused dock leaf.
//!
//! # Design
//!
//! The render function is **pure** — it takes a pre-built
//! [`SidebarData`] snapshot and returns a [`Vec<SidebarEvent>`].
//! The caller is responsible for building the snapshot and processing
//! the returned events, which avoids borrow-checker conflicts.

use crate::session::SessionId;
use crate::workspace::WorkspaceId;

// ── Data types ───────────────────────────────────────────────────────────

/// Pre-computed snapshot of all data the sidebar needs to render.
/// Built by the caller so the render function can be pure (no borrows).
pub struct SidebarData {
    pub workspaces: Vec<WorkspaceSidebarEntry>,
    pub active_session_id: Option<SessionId>,
}

/// A single workspace row in the sidebar.
pub struct WorkspaceSidebarEntry {
    pub id: WorkspaceId,
    pub name: String,
    pub is_active: bool,
    pub tabs: Vec<TabSidebarEntry>,
}

/// A single tab under a workspace.
pub struct TabSidebarEntry {
    pub node: egui_dock::NodeIndex,
    pub tab: egui_dock::TabIndex,
    pub id: SessionId,
    pub title: String,
    pub cwd: Option<std::path::PathBuf>,
}

// ── Events ───────────────────────────────────────────────────────────────

/// An action the user took while interacting with the sidebar.
pub enum SidebarEvent {
    NewShell,
    NewWorkspace,
    SwitchWorkspace(WorkspaceId),
    CloseWorkspace(WorkspaceId),
    RenameWorkspace(WorkspaceId, String),
    FocusTab(egui_dock::NodeIndex, egui_dock::TabIndex),
}

// ── Render ───────────────────────────────────────────────────────────────

/// Render the sidebar inside an existing `egui::Ui`.
/// Returns a list of events that the caller should process.
pub fn render_sidebar(ui: &mut egui::Ui, data: &SidebarData) -> Vec<SidebarEvent> {
    let mut events = Vec::new();

    ui.vertical(|ui| {
        ui.add_space(6.0);

        // ── "New shell" / "New WS" buttons ──────────────────────
        ui.horizontal(|ui| {
            if ui.button("+  New shell").clicked() {
                events.push(SidebarEvent::NewShell);
            }
            if ui.button("+  New WS").clicked() {
                events.push(SidebarEvent::NewWorkspace);
            }
        });
        ui.add_space(2.0);
        ui.separator();

        // ── Scrollable workspace list ───────────────────────────
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for ws_entry in &data.workspaces {
                    let ws_id = ws_entry.id;
                    let rename_id = egui::Id::new(("ws_rename", ws_id.0));
                    let is_renaming = ui.memory(|m| {
                        m.data
                            .get_temp::<bool>(rename_id)
                            .unwrap_or(false)
                    });

                    if is_renaming {
                        // ── Inline rename mode ──
                        let mut buf = ws_entry.name.clone();
                        let resp = ui.text_edit_singleline(&mut buf);
                        if resp.lost_focus()
                            || ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            ui.memory_mut(|m| {
                                m.data.remove_temp::<bool>(rename_id);
                            });
                            if !buf.is_empty() && buf != ws_entry.name {
                                events.push(SidebarEvent::RenameWorkspace(ws_id, buf));
                            }
                        }
                        // Cancel on Escape.
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            ui.memory_mut(|m| {
                                m.data.remove_temp::<bool>(rename_id);
                            });
                        }
                        // Keep focus on the text edit.
                        resp.request_focus();
                    } else {
                        // ── Normal display mode ──
                        let header_label = if ws_entry.is_active {
                            egui::RichText::new(&ws_entry.name)
                                .strong()
                                .color(ui.visuals().strong_text_color())
                        } else {
                            egui::RichText::new(&ws_entry.name)
                        };
                        let header_resp =
                            ui.selectable_label(ws_entry.is_active, header_label);

                        if header_resp.clicked() {
                            events.push(SidebarEvent::SwitchWorkspace(ws_id));
                        }
                        // Double-click to rename.
                        if header_resp.double_clicked() {
                            ui.memory_mut(|m| {
                                m.data.insert_temp::<bool>(rename_id, true);
                            });
                        }
                        // Right-click context menu.
                        header_resp.context_menu(|ui| {
                            if ui.button("New Tab").clicked() {
                                events.push(SidebarEvent::NewShell);
                                events.push(SidebarEvent::SwitchWorkspace(ws_id));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Rename...").clicked() {
                                ui.memory_mut(|m| {
                                    m.data.insert_temp::<bool>(rename_id, true);
                                });
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Close workspace").clicked() {
                                events.push(SidebarEvent::CloseWorkspace(ws_id));
                                ui.close();
                            }
                        });
                    }

                    // ── Tabs under this workspace ──
                    ui.indent(egui::Id::new(("ws_tabs", ws_id.0)), |ui| {
                        if ws_entry.tabs.is_empty() {
                            ui.weak("(no tabs)");
                        }
                        for tab_entry in &ws_entry.tabs {
                            let is_active_tab =
                                Some(tab_entry.id) == data.active_session_id;
                            let label = if is_active_tab {
                                egui::RichText::new(&tab_entry.title)
                                    .strong()
                                    .color(ui.visuals().strong_text_color())
                            } else {
                                egui::RichText::new(&tab_entry.title)
                            };
                            let resp =
                                ui.selectable_label(is_active_tab, label);
                            if resp.clicked() {
                                // Switch to the tab's workspace first,
                                // then focus the tab.
                                events.push(SidebarEvent::SwitchWorkspace(ws_id));
                                events.push(SidebarEvent::FocusTab(
                                    tab_entry.node,
                                    tab_entry.tab,
                                ));
                            }
                            if let Some(cwd) = &tab_entry.cwd {
                                ui.weak(cwd.display().to_string());
                            }
                        }
                    });

                    // Small gap between workspace sections.
                    ui.add_space(4.0);
                }
            });
    });

    events
}
