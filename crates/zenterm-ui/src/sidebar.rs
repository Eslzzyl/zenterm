//! Workspace sidebar — cmux-style vertical tab list.
//!
//! When `config.ui.sidebar_enabled = true` (and `tabs_enabled = true`),
//! this is rendered as a [`egui::SidePanel`] on the left edge of the
//! window.  Each entry shows the session's title, working directory
//! (parsed from OSC 7), and an active-tab indicator.  A `+ New shell`
//! button at the top spawns a new [`TerminalSession`] in the
//! currently focused dock leaf.
//!
//! # Behaviour with `tabs_enabled = false`
//!
//! The sidebar is suppressed in legacy single-terminal mode (the
//! `ZentermApp` does not allocate a side panel at all in that case).
//! This file therefore only contains the *render* helper; the parent
//! decides whether to call it.

use egui_dock::TabIndex;
use zenterm_config::ui::SidebarPosition;

use crate::session::SessionId;

/// Render the sidebar inside an existing `egui::Ui` (the caller is
/// responsible for allocating the [`egui::SidePanel`]).
///
/// `on_new_tab` and `on_select_tab` are invoked when the user clicks
/// the "New shell" button or an existing tab row.
///
/// This is a *reference* implementation kept for future extraction;
/// the actual production rendering in `app.rs` inlines the snapshot
/// pattern directly so it can pass through the borrow checker.
#[allow(dead_code)]
pub fn render_sidebar(
    ui: &mut egui::Ui,
    sessions: &std::collections::HashMap<SessionId, crate::session::TerminalSession>,
    active_session_id: Option<SessionId>,
    dock: &egui_dock::DockState<SessionId>,
    position: SidebarPosition,
    width: f32,
    on_new_tab: &mut dyn FnMut(),
    on_select_tab: &mut dyn FnMut(egui_dock::NodeIndex, TabIndex),
) {
    let _ = position;
    let _ = width;

    ui.vertical(|ui| {
        ui.add_space(6.0);

        // ── "New shell" button ──────────────────────────────────────
        if ui.button("+  New shell").clicked() {
            on_new_tab();
        }

        ui.add_space(2.0);
        ui.separator();

        // ── Tab list ────────────────────────────────────────────────
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    for (tab_path, tab_id) in dock.iter_all_tabs() {
                        let id = *tab_id;
                        let Some(session) = sessions.get(&id) else {
                            continue;
                        };
                        let is_active = Some(id) == active_session_id;

                        // Title (active gets a coloured dot).
                        let label = if is_active {
                            egui::RichText::new(&session.title)
                                .strong()
                                .color(ui.visuals().strong_text_color())
                        } else {
                            egui::RichText::new(&session.title)
                        };
                        let resp = ui.selectable_label(is_active, label);

                        if resp.clicked() {
                            on_select_tab(tab_path.node, tab_path.tab);
                        }

                        // Sub-line: working directory.
                        if let Some(cwd) = &session.cwd {
                            ui.weak(cwd.display().to_string());
                        }

                        // Future: notification dot.
                        if !matches!(session.notification, crate::session::NotificationState::None) {
                            ui.weak("● notification");
                        }
                    }
                });    });
}
