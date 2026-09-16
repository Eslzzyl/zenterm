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

use egui::Color32;

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
    /// Title of the first tab, used as a lightweight session hint.
    pub tab_title: Option<String>,
    /// Whether any session in the workspace needs attention.
    pub has_attention: bool,
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
        ui.add_space(10.0);

        // ── "New workspace" / settings buttons ───────────────────
        let avail_w = ui.available_width();
        let accent = ui.visuals().hyperlink_color;
        let border_color = ui.visuals().window_stroke.color;
        let dark_mode = ui.visuals().dark_mode;
        let text_color = ui.visuals().text_color();
        let weak_text = ui
            .visuals()
            .weak_text_color
            .unwrap_or_else(|| text_color.linear_multiply(0.6));

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);

            // "New Workspace" button
            let settings_btn_w = 28.0;
            let spacing = 6.0;
            let new_ws_w = (avail_w - settings_btn_w - spacing).max(28.0);
            let show_label = new_ws_w >= 100.0;

            let (btn_rect, btn_resp) =
                ui.allocate_exact_size(egui::vec2(new_ws_w, 28.0), egui::Sense::click());

            let btn_hovered = btn_resp.hovered();
            let btn_active = btn_resp.is_pointer_button_down_on();

            let btn_bg = if btn_active {
                accent
            } else if btn_hovered {
                if dark_mode {
                    Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 45)
                } else {
                    Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 25)
                }
            } else {
                ui.visuals().window_fill
            };

            let btn_stroke_color = if btn_active || btn_hovered {
                accent
            } else {
                border_color
            };

            ui.painter().rect(
                btn_rect,
                6.0,
                btn_bg,
                egui::Stroke::new(1.0_f32, btn_stroke_color),
                egui::StrokeKind::Inside,
            );

            let btn_text_color = if btn_active {
                Color32::WHITE
            } else if btn_hovered {
                if dark_mode { Color32::WHITE } else { accent }
            } else {
                ui.visuals().strong_text_color()
            };

            let btn_label = if show_label {
                format!("{}  New Workspace", crate::icons::PLUS)
            } else {
                crate::icons::PLUS.to_string()
            };
            let label_shape = ui.painter().layout_no_wrap(
                btn_label,
                egui::FontId::proportional(12.5),
                btn_text_color,
            );
            let label_pos = if show_label {
                egui::pos2(
                    btn_rect.left() + 12.0,
                    btn_rect.center().y - label_shape.size().y * 0.5,
                )
            } else {
                btn_rect.center() - label_shape.size() * 0.5
            };
            ui.painter().galley(label_pos, label_shape, Color32::WHITE);

            if btn_resp.clicked() {
                events.push(SidebarEvent::NewWorkspace);
            }
            btn_resp.on_hover_text("Create new workspace");

            // Settings gear button on the right
            let settings_resp = crate::icons::icon_button(
                ui,
                crate::icons::GEAR,
                16.0,
                egui::vec2(settings_btn_w, 28.0),
                "Settings",
            );
            if settings_resp.clicked() {
                events.push(SidebarEvent::OpenSettings);
            }
        });

        ui.add_space(8.0);
        // Subtle divider hairline
        let sep_rect = egui::Rect::from_min_size(
            egui::pos2(ui.min_rect().left(), ui.cursor().top()),
            egui::vec2(ui.available_width(), 1.0),
        );
        ui.painter().rect_filled(sep_rect, 0.0, border_color);
        ui.add_space(8.0);

        // ── Scrollable workspace list ───────────────────────────
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for ws_entry in &data.workspaces {
                    let ws_id = ws_entry.id;

                    let has_subtitle = ws_entry
                        .tab_title
                        .as_ref()
                        .map(|t| !t.is_empty() && t != &ws_entry.name)
                        .unwrap_or(false);

                    let card_h = if has_subtitle { 52.0 } else { 42.0 };
                    let desired = egui::vec2(ui.available_width(), card_h);
                    let (card_rect, card_resp) =
                        ui.allocate_exact_size(desired, egui::Sense::click());

                    let is_hovered = card_resp.hovered();
                    let is_active = ws_entry.is_active;
                    let corner_radius = 7.0;

                    // Modern card background:
                    // Active card has high contrast surface and border
                    let (bg_color, stroke_color) = if is_active {
                        if dark_mode {
                            (
                                Color32::from_rgb(32, 37, 50),
                                egui::Stroke::new(1.0_f32, Color32::from_rgb(67, 76, 102)),
                            )
                        } else {
                            (
                                Color32::from_rgb(238, 242, 255),
                                egui::Stroke::new(1.0_f32, Color32::from_rgb(199, 210, 254)),
                            )
                        }
                    } else if is_hovered {
                        (
                            if dark_mode {
                                Color32::from_rgba_unmultiplied(255, 255, 255, 14)
                            } else {
                                Color32::from_rgba_unmultiplied(0, 0, 0, 10)
                            },
                            egui::Stroke::new(1.0_f32, border_color),
                        )
                    } else {
                        (Color32::TRANSPARENT, egui::Stroke::NONE)
                    };

                    ui.painter().rect(
                        card_rect,
                        corner_radius,
                        bg_color,
                        stroke_color,
                        egui::StrokeKind::Inside,
                    );

                    // Floating rounded pill indicator on active card
                    if is_active {
                        let pill_rect = egui::Rect::from_center_size(
                            egui::pos2(card_rect.left() + 4.0, card_rect.center().y),
                            egui::vec2(3.5, if has_subtitle { 24.0 } else { 20.0 }),
                        );
                        ui.painter().rect_filled(pill_rect, 1.75, accent);
                    }

                    // ── Card content ──
                    let title_y = if has_subtitle {
                        card_rect.top() + 15.0
                    } else {
                        card_rect.center().y
                    };
                    let left_x = card_rect.left() + 14.0;
                    let mut right_x = card_rect.right() - 10.0;

                    // 1. Attention icon
                    if ws_entry.has_attention {
                        let warn_shape = ui.painter().layout_no_wrap(
                            crate::icons::WARNING_CIRCLE.to_string(),
                            egui::FontId::proportional(12.5),
                            ui.visuals().warn_fg_color,
                        );
                        let warn_w = warn_shape.size().x;
                        let warn_pos =
                            egui::pos2(right_x - warn_w, title_y - warn_shape.size().y * 0.5);
                        ui.painter().galley(warn_pos, warn_shape, Color32::WHITE);
                        right_x -= warn_w + 6.0;
                    }

                    // 2. Tab count badge
                    if ws_entry.tab_count > 0 {
                        let tab_label = if ws_entry.tab_count == 1 {
                            "1 tab"
                        } else {
                            &format!("{} tabs", ws_entry.tab_count)
                        };
                        let badge_font = egui::FontId::proportional(11.0);
                        let badge_text_shape = ui.painter().layout_no_wrap(
                            tab_label.to_string(),
                            badge_font.clone(),
                            if is_active {
                                if dark_mode {
                                    Color32::from_rgb(199, 210, 254)
                                } else {
                                    Color32::from_rgb(67, 56, 202)
                                }
                            } else {
                                weak_text
                            },
                        );
                        let badge_w = badge_text_shape.size().x + 8.0;
                        let badge_h = 17.0;
                        let badge_rect = egui::Rect::from_center_size(
                            egui::pos2(right_x - badge_w * 0.5, title_y),
                            egui::vec2(badge_w, badge_h),
                        );
                        let badge_bg = if is_active {
                            if dark_mode {
                                Color32::from_rgba_unmultiplied(129, 140, 248, 40)
                            } else {
                                Color32::from_rgb(224, 231, 255)
                            }
                        } else if dark_mode {
                            Color32::from_rgba_unmultiplied(255, 255, 255, 18)
                        } else {
                            Color32::from_rgba_unmultiplied(0, 0, 0, 14)
                        };
                        ui.painter().rect_filled(badge_rect, 4.0, badge_bg);
                        let badge_top =
                            badge_rect.top() + (badge_h - badge_text_shape.size().y) * 0.5;
                        ui.painter().galley(
                            egui::pos2(badge_rect.left() + 4.0, badge_top),
                            badge_text_shape,
                            Color32::WHITE,
                        );
                        right_x = badge_rect.left() - 6.0;
                    }

                    // 3. Terminal prompt icon `>_`
                    let icon_color = if is_active {
                        accent
                    } else if is_hovered {
                        ui.visuals().strong_text_color()
                    } else {
                        weak_text
                    };
                    let icon_shape = ui.painter().layout_no_wrap(
                        crate::icons::TERMINAL.to_string(),
                        egui::FontId::proportional(14.0),
                        icon_color,
                    );
                    let icon_w = icon_shape.size().x;
                    let icon_pos = egui::pos2(left_x, title_y - icon_shape.size().y * 0.5);
                    ui.painter().galley(icon_pos, icon_shape, Color32::WHITE);

                    // 4. Title label
                    let title_start_x = left_x + icon_w + 7.0;
                    let title_color = if is_active {
                        if dark_mode {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(30, 27, 75)
                        }
                    } else if is_hovered {
                        ui.visuals().strong_text_color()
                    } else {
                        text_color
                    };

                    let title_font = egui::FontId::proportional(13.0);
                    let title_galley = ui.painter().layout(
                        ws_entry.name.clone(),
                        title_font,
                        title_color,
                        (right_x - title_start_x).max(10.0),
                    );
                    let title_top = title_y - title_galley.size().y * 0.5;
                    ui.painter().galley(
                        egui::pos2(title_start_x, title_top),
                        title_galley,
                        Color32::WHITE,
                    );

                    // 5. Subtitle (if present)
                    if has_subtitle && let Some(sub) = &ws_entry.tab_title {
                        let sub_y = card_rect.bottom() - 14.0;
                        let sub_galley = ui.painter().layout(
                            sub.clone(),
                            egui::FontId::proportional(11.0),
                            weak_text,
                            (card_rect.right() - 10.0 - title_start_x).max(10.0),
                        );
                        let sub_top = sub_y - sub_galley.size().y * 0.5;
                        ui.painter().galley(
                            egui::pos2(title_start_x, sub_top),
                            sub_galley,
                            Color32::WHITE,
                        );
                    }

                    // ── Handle card interaction ─────────────────
                    if card_resp.clicked() {
                        events.push(SidebarEvent::SwitchWorkspace(ws_id));
                    }
                    if card_resp.double_clicked() {
                        open_dialog(ui, ws_id);
                    }
                    card_resp.context_menu(|ui| {
                        if ui
                            .button(format!("{}  New Tab", crate::icons::PLUS))
                            .clicked()
                        {
                            events.push(SidebarEvent::NewShell);
                            events.push(SidebarEvent::SwitchWorkspace(ws_id));
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .button(format!("{}  Rename...", crate::icons::PENCIL_SIMPLE))
                            .clicked()
                        {
                            open_dialog(ui, ws_id);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .button(format!("{}  Close workspace", crate::icons::TRASH))
                            .clicked()
                        {
                            events.push(SidebarEvent::CloseWorkspace(ws_id));
                            ui.close();
                        }
                    });

                    // Gap between cards.
                    ui.add_space(6.0);
                }

                // Helpful empty-state keyboard hint when workspace list is minimal
                if data.workspaces.len() <= 1 {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Ctrl+Shift+T  New Tab\nCtrl+Shift+P  Commands")
                                .size(11.0)
                                .weak(),
                        );
                    });
                }
            });
    });

    // ── Rename dialog (modal, rendered outside the vertical layout) ──
    let dialog_ws_id: Option<WorkspaceId> =
        ui.data(|d| d.get_temp::<u64>(DIALOG_WS_KEY.into()).map(WorkspaceId));

    if let Some(ws_id) = dialog_ws_id {
        let buf_id = dialog_buf_key(ws_id);

        // Find the current workspace name for initial buffer.
        let initial_name = data
            .workspaces
            .iter()
            .find(|ws| ws.id == ws_id)
            .map(|ws| ws.name.clone())
            .unwrap_or_default();

        let mut buf: String = ui.data(|d| d.get_temp::<String>(buf_id).unwrap_or(initial_name));

        let ctx = ui.ctx();
        let area_id = egui::Id::new("ws_rename_area");
        let input_id = egui::Id::new(("rename_dialog_input", ws_id.0));
        let has_saved_buffer = ui.data(|d| d.get_temp::<String>(buf_id).is_some());
        let mut close_requested = false;

        egui::Area::new(area_id)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.global_style())
                    .inner_margin(egui::Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        ui.set_min_width(280.0);
                        ui.strong("Rename workspace");
                        ui.add_space(10.0);

                        ui.add(
                            egui::TextEdit::singleline(&mut buf)
                                .id(input_id)
                                .desired_width(f32::INFINITY),
                        );
                        if !has_saved_buffer {
                            ui.memory_mut(|memory| memory.request_focus(input_id));
                        }

                        let (submit, cancel) = ui.input(|input| {
                            (
                                input.key_pressed(egui::Key::Enter),
                                input.key_pressed(egui::Key::Escape),
                            )
                        });
                        if submit {
                            if !buf.is_empty() {
                                events.push(SidebarEvent::RenameWorkspace(ws_id, buf.clone()));
                            }
                            close_requested = true;
                        } else if cancel {
                            close_requested = true;
                        }

                        ui.add_space(14.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("OK").clicked() {
                                if !buf.is_empty() {
                                    events.push(SidebarEvent::RenameWorkspace(ws_id, buf.clone()));
                                }
                                close_requested = true;
                            }
                            ui.add_space(8.0);
                            if ui.button("Cancel").clicked() {
                                close_requested = true;
                            }
                        });
                    });
            });

        if close_requested {
            close_dialog(ui, ws_id);
        } else {
            ui.ctx().data_mut(|d| {
                d.insert_temp::<String>(buf_id, buf);
            });
        }
    }

    events
}
