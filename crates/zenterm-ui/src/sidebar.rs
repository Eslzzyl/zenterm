//! Workspace sidebar — shows workspace cards with full-width layout.
//!
//! When `config.ui.sidebar_enabled = true`, this is rendered as a
//! [`egui::SidePanel`] on the left (or right) edge of the window.
//! Each workspace is shown as a clickable card.  The active workspace
//! is visually highlighted.  Double-click or right-click "Rename..."
//! opens a modal dialog for renaming.
//!
//! # Design
//!
//! The render function is **pure** — it takes a pre-built
//! [`SidebarData`] snapshot and returns a [`Vec<SidebarEvent>`].
//! The caller is responsible for building the snapshot and processing
//! the returned events, which avoids borrow-checker conflicts.

use crate::workspace::WorkspaceId;

// ── Data types ───────────────────────────────────────────────────────────

/// Pre-computed snapshot of all data the sidebar needs to render.
/// Built by the caller so the render function can be pure (no borrows).
pub struct SidebarData {
    pub workspaces: Vec<WorkspaceSidebarEntry>,
}

/// A single workspace card in the sidebar.
pub struct WorkspaceSidebarEntry {
    pub id: WorkspaceId,
    pub name: String,
    pub is_active: bool,
    /// Number of tabs in this workspace (shown as a subtitle).
    pub tab_count: usize,
}

// ── Events ───────────────────────────────────────────────────────────────

/// An action the user took while interacting with the sidebar.
pub enum SidebarEvent {
    NewShell,
    NewWorkspace,
    SwitchWorkspace(WorkspaceId),
    CloseWorkspace(WorkspaceId),
    RenameWorkspace(WorkspaceId, String),
    /// Open the settings panel.
    OpenSettings,
}

// ── UI memory keys ───────────────────────────────────────────────────────

const DIALOG_WS_KEY: &str = "ws_rename_dialog_ws";

fn dialog_buf_key(ws_id: WorkspaceId) -> egui::Id {
    egui::Id::new(("ws_rename_dialog_buf", ws_id.0))
}

fn open_dialog(ui: &egui::Ui, ws_id: WorkspaceId) {
    ui.ctx().data_mut(|d| {
        d.insert_temp::<u64>(DIALOG_WS_KEY.into(), ws_id.0);
    });
}

fn close_dialog(ui: &egui::Ui, ws_id: WorkspaceId) {
    ui.ctx().data_mut(|d| {
        d.remove_temp::<u64>(DIALOG_WS_KEY.into());
        d.remove_temp::<String>(dialog_buf_key(ws_id));
    });
}

// ── Render ───────────────────────────────────────────────────────────────

/// Render the sidebar inside an existing `egui::Ui`.
/// Returns a list of events that the caller should process.
pub fn render_sidebar(ui: &mut egui::Ui, data: &SidebarData) -> Vec<SidebarEvent> {
    let mut events = Vec::new();

    ui.vertical(|ui| {
        ui.add_space(6.0);

        // ── "New shell" / "New WS" / ⚙ buttons ──────────────────
        ui.horizontal(|ui| {
            if ui.button("+  New shell").clicked() {
                events.push(SidebarEvent::NewShell);
            }
            if ui.button("+  New WS").clicked() {
                events.push(SidebarEvent::NewWorkspace);
            }
            // Push settings gear to the right.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⚙").clicked() {
                    events.push(SidebarEvent::OpenSettings);
                }
            });
        });
        ui.add_space(2.0);
        ui.separator();

        // ── Scrollable workspace list ───────────────────────────
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for ws_entry in &data.workspaces {
                    let ws_id = ws_entry.id;

                    // ── Allocate full-width clickable card ──────
                    let desired = egui::vec2(ui.available_width(), 48.0);
                    let (card_rect, card_resp) =
                        ui.allocate_exact_size(desired, egui::Sense::click());

                    // Paint background for the entire card.
                    let is_hovered = card_resp.hovered();
                    let bg = if ws_entry.is_active {
                        ui.visuals().selection.bg_fill
                    } else if is_hovered {
                        ui.visuals().widgets.hovered.bg_fill
                    } else {
                        ui.visuals().faint_bg_color
                    };
                    ui.painter()
                        .rect_filled(card_rect, 6.0, bg);

                    // ── Card content ─────────────────────────────
                    let content_rect = card_rect.shrink2(egui::vec2(10.0, 6.0));
                    let mut content_ui = ui.new_child(
                        egui::UiBuilder::default()
                            .max_rect(content_rect)
                            .layout(*ui.layout()),
                    );
                    content_ui.vertical(|ui| {
                        let label_color = if ws_entry.is_active {
                            ui.visuals().selection.stroke.color
                        } else if is_hovered {
                            ui.visuals().strong_text_color()
                        } else {
                            ui.visuals().text_color()
                        };
                        let mut label = egui::RichText::new(&ws_entry.name)
                            .size(14.0)
                            .color(label_color);
                        if ws_entry.is_active {
                            label = label.strong();
                        }
                        ui.label(label);

                        // Subtitle: tab count.
                        if ws_entry.tab_count > 0 {
                            ui.add_space(2.0);
                            ui.weak(format!("{} tabs", ws_entry.tab_count));
                        }
                    });

                    // ── Handle card interaction ─────────────────
                    if card_resp.clicked() {
                        events.push(SidebarEvent::SwitchWorkspace(ws_id));
                    }
                    if card_resp.double_clicked() {
                        open_dialog(ui, ws_id);
                    }
                    card_resp.context_menu(|ui| {
                        if ui.button("New Tab").clicked() {
                            events.push(SidebarEvent::NewShell);
                            events.push(SidebarEvent::SwitchWorkspace(ws_id));
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Rename...").clicked() {
                            open_dialog(ui, ws_id);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Close workspace").clicked() {
                            events.push(SidebarEvent::CloseWorkspace(ws_id));
                            ui.close();
                        }
                    });

                    // Gap between cards.
                    ui.add_space(4.0);
                }
            });
    });

    // ── Rename dialog (modal, rendered outside the vertical layout) ──
    let dialog_ws_id: Option<WorkspaceId> = ui.data(|d| {
        d.get_temp::<u64>(DIALOG_WS_KEY.into())
            .filter(|id| *id != 0)
            .map(WorkspaceId)
    });

    if let Some(ws_id) = dialog_ws_id {
        let buf_id = dialog_buf_key(ws_id);

        // Find the current workspace name for initial buffer.
        let initial_name = data
            .workspaces
            .iter()
            .find(|ws| ws.id == ws_id)
            .map(|ws| ws.name.clone())
            .unwrap_or_default();

        let mut buf: String = ui.data(|d| {
            d.get_temp::<String>(buf_id)
                .unwrap_or(initial_name)
        });

        let ctx = ui.ctx();
        let area_id = egui::Id::new("ws_rename_area");

        egui::Area::new(area_id)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(&*ctx.global_style())
                    .inner_margin(egui::Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        ui.set_min_width(280.0);
                        ui.strong("Rename workspace");
                        ui.add_space(10.0);

                        ui.add(
                            egui::TextEdit::singleline(&mut buf)
                                .id(egui::Id::new("rename_dialog_input"))
                                .desired_width(f32::INFINITY),
                        )
                        .request_focus();

                        ui.add_space(14.0);
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.button("OK").clicked() {
                                    if !buf.is_empty() {
                                        events.push(SidebarEvent::RenameWorkspace(
                                            ws_id,
                                            buf.clone(),
                                        ));
                                    }
                                    close_dialog(ui, ws_id);
                                }
                                ui.add_space(8.0);
                                if ui.button("Cancel").clicked() {
                                    close_dialog(ui, ws_id);
                                }
                            },
                        );
                    });
            });
    }

    events
}
