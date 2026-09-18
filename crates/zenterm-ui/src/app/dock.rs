//! Dock/tab rendering — renders the main terminal tab area with egui_dock.
//!
//! Owns the sidebar, dock area, tab viewer, and pending action queues.

use egui::{Color32, CornerRadius, Id, Margin, Stroke};
use egui_dock::{DockArea, Style, TabAddAlign};

use super::ZentermApp;
use crate::session::SessionId;
use crate::tab_viewer::TabViewerContext;

// ── Dock rendering ─────────────────────────────────────────────────────

impl ZentermApp {
    pub(crate) fn render_tabs_with_dock(&mut self, ui: &mut egui::Ui) {
        // Clear one-frame action queues collected during the previous frame.
        // `pending_rename` intentionally persists until the dialog closes.
        self.pending_close.clear();
        self.pending_adds = 0;

        // ── Optional sidebar ────────────────────────────────────────
        let show_sidebar = self.config.ui.sidebar_enabled;
        if show_sidebar {
            let pos = self.config.ui.sidebar_position;
            let width = self.config.ui.sidebar_width;
            let min_width = self.config.ui.sidebar_min_width;
            let max_w = self.config.ui.sidebar_max_width;
            let panel = match pos {
                zenterm_config::ui::SidebarPosition::Left => egui::Panel::left("zenterm_sidebar"),
                zenterm_config::ui::SidebarPosition::Right => egui::Panel::right("zenterm_sidebar"),
            };

            // Snapshot all workspaces so the closure doesn't need
            // to borrow `self`.
            let ws_snapshot: Vec<(
                crate::workspace::WorkspaceId,
                String,
                bool,
                usize, // tab count
                Option<String>,
                bool,
            )> = self
                .workspaces
                .workspaces
                .iter()
                .map(|ws| {
                    let tab_ids = ws.all_tab_ids();
                    let tab_count = tab_ids.len();
                    let tab_title = tab_ids
                        .first()
                        .and_then(|id| self.sessions.get(id))
                        .map(|session| session.title_effective());
                    let has_attention = tab_ids.iter().any(|id| {
                        self.sessions.get(id).is_some_and(|session| {
                            !matches!(
                                session.notification(),
                                crate::session::NotificationState::None
                            )
                        })
                    });
                    (
                        ws.id,
                        ws.name.clone(),
                        ws.id == self.workspaces.active_workspace_id,
                        tab_count,
                        tab_title,
                        has_attention,
                    )
                })
                .collect();

            panel
                .resizable(true)
                .default_size(width)
                .min_size(min_width)
                .max_size(max_w)
                .show_inside(ui, |ui| {
                    let mut queued_new_tab = false;
                    let mut queued_new_ws = false;
                    let mut queued_switch_ws: Option<crate::workspace::WorkspaceId> = None;
                    let mut queued_rename_ws: Option<(crate::workspace::WorkspaceId, String)> =
                        None;
                    let mut queued_close_ws: Option<crate::workspace::WorkspaceId> = None;

                    let sidebar_data = crate::sidebar::SidebarData {
                        workspaces: ws_snapshot
                            .into_iter()
                            .map(
                                |(id, name, is_active, tab_count, tab_title, has_attention)| {
                                    crate::sidebar::WorkspaceSidebarEntry {
                                        id,
                                        name,
                                        is_active,
                                        tab_count,
                                        tab_title,
                                        has_attention,
                                    }
                                },
                            )
                            .collect(),
                    };

                    let events = crate::sidebar::render_sidebar(ui, &sidebar_data);
                    for event in events {
                        match event {
                            crate::sidebar::SidebarEvent::NewShell => queued_new_tab = true,
                            crate::sidebar::SidebarEvent::NewWorkspace => queued_new_ws = true,
                            crate::sidebar::SidebarEvent::SwitchWorkspace(id) => {
                                queued_switch_ws = Some(id)
                            }
                            crate::sidebar::SidebarEvent::CloseWorkspace(id) => {
                                queued_close_ws = Some(id)
                            }
                            crate::sidebar::SidebarEvent::RenameWorkspace(id, name) => {
                                queued_rename_ws = Some((id, name))
                            }
                            crate::sidebar::SidebarEvent::OpenSettings => {
                                self.settings_state.open = true;
                                self.settings_state.reset_to(&self.config);
                            }
                        }
                    }

                    // ── Apply queued actions ──────────────────────
                    if queued_new_ws {
                        let active_session =
                            self.active_session_id.and_then(|id| self.sessions.get(&id));
                        let ws_name =
                            Self::generate_workspace_name(&self.workspaces, active_session);
                        let ws_id = self.workspaces.create_workspace(ws_name);
                        // Also spawn a first tab in the new workspace.  If
                        // PTY creation fails, remove the empty workspace so
                        // the failed action does not leave broken state.
                        if self.spawn_session().is_none() {
                            self.workspaces.close_workspace(ws_id);
                            self.focus_first_tab_in_active_workspace();
                        }
                    }
                    if let Some(ws_id) = queued_switch_ws
                        && self.workspaces.switch_to(ws_id)
                    {
                        self.focus_first_tab_in_active_workspace();
                        self.mark_layout_dirty();
                    }
                    if queued_new_tab {
                        self.spawn_session();
                    }
                    if let Some((ws_id, new_name)) = queued_rename_ws {
                        self.workspaces.rename_workspace(ws_id, new_name);
                        self.mark_layout_dirty();
                    }
                    if let Some(ws_id) = queued_close_ws {
                        let sessions_to_close = self
                            .workspaces
                            .find_workspace(ws_id)
                            .map(|ws| ws.all_tab_ids())
                            .unwrap_or_default();
                        if self.workspaces.close_workspace(ws_id) {
                            // Closing a workspace also closes every session
                            // that was owned by it.  Only remove sessions
                            // after the manager confirms the workspace was
                            // actually removed, so closing the last workspace
                            // remains a no-op.
                            for id in sessions_to_close {
                                self.sessions.remove(&id);
                            }
                            let active_workspace_id = self.workspaces.active_workspace_id;
                            let active_session_is_visible = self
                                .active_session_id
                                .and_then(|id| self.workspaces.find_tab_workspace(id))
                                .is_some_and(|ws| ws.id == active_workspace_id);
                            if !active_session_is_visible {
                                self.focus_first_tab_in_active_workspace();
                            }
                            self.mark_layout_dirty();
                        }
                    }
                });
        }

        // ── Central dock area ──────────────────────────────────────
        egui::CentralPanel::default()
            // Cover newly exposed resize pixels with the terminal canvas
            // colour instead of leaving them to the surface clear colour.
            .frame(egui::Frame::NONE.fill(self.default_bg))
            .show_inside(ui, |ui| {
                // Compute the dock-area rect (union of all session
                // viewports) for the single wgpu callback.
                let dock_rect = ui.available_rect_before_wrap();
                let ppp = ui.ctx().pixels_per_point();
                let dock_origin_px = [dock_rect.min.x * ppp, dock_rect.min.y * ppp];
                let dock_size_px = [dock_rect.size().x * ppp, dock_rect.size().y * ppp];

                // Set the shared dock viewport on every session so
                // `update_cell_instances` uses it for clip-space
                // conversion.
                for session in self.sessions.values_mut() {
                    session.set_dock_viewport(dock_origin_px, dock_size_px);
                }

                // Determine whether the active-panel border indicator
                // should be shown: hide it when the dock is a single
                // leaf (no split), even if that leaf has multiple tabs.
                let show_active_indicator = self
                    .workspaces
                    .active_workspace()
                    .dock
                    .iter_leaves()
                    .count()
                    > 1;

                // ── Style setup (reusable outside the borrow block) ──
                let mut style = Style::from_egui(ui.style().as_ref());
                let egui_visuals = ui.visuals().clone();

                // Tab bar — compact, with a clear active surface.
                style.tab_bar.bg_fill = egui_visuals.extreme_bg_color;
                style.tab_bar.height = 34.0;
                style.tab_bar.inner_margin = Margin::symmetric(6, 0);
                style.tab_bar.corner_radius = CornerRadius::ZERO;
                style.tab_bar.hline_color = egui_visuals.window_stroke.color;

                // Tab spacing — small gap between tabs, minimum width.
                style.tab.spacing = 3.0;
                style.tab.minimum_width = Some(88.0);

                // Tab body: no margin, no stroke, no corners so the
                // terminal content goes edge-to-edge.
                style.tab.tab_body.corner_radius = CornerRadius::ZERO;
                style.tab.tab_body.inner_margin = Margin::ZERO;
                style.tab.tab_body.stroke = Stroke::NONE;

                // Active tab — top corners rounded, bottom flat.
                let top_round = CornerRadius {
                    nw: 6,
                    ne: 6,
                    sw: 0,
                    se: 0,
                };
                style.tab.active.corner_radius = top_round;
                style.tab.active_with_kb_focus.corner_radius = top_round;
                style.tab.inactive.corner_radius = top_round;
                style.tab.inactive_with_kb_focus.corner_radius = top_round;
                style.tab.hovered.corner_radius = top_round;
                style.tab.focused.corner_radius = top_round;
                style.tab.focused_with_kb_focus.corner_radius = top_round;

                // Active tab seamlessly matches the terminal background canvas
                let active_tab_bg = self.default_bg;
                style.tab.active.bg_fill = active_tab_bg;
                style.tab.active_with_kb_focus.bg_fill = active_tab_bg;
                style.tab.active.outline_color = egui_visuals.hyperlink_color;
                style.tab.active_with_kb_focus.outline_color = egui_visuals.hyperlink_color;
                style.tab.inactive.bg_fill = Color32::TRANSPARENT;
                style.tab.inactive_with_kb_focus.bg_fill = Color32::TRANSPARENT;
                style.tab.hovered.bg_fill = egui_visuals.widgets.hovered.bg_fill;

                let weak_text = egui_visuals
                    .weak_text_color
                    .unwrap_or(egui_visuals.text_color().linear_multiply(0.55));
                let text_bright = egui_visuals.strong_text_color();
                let text_main = egui_visuals.text_color();

                style.tab.active.text_color = text_main;
                style.tab.active_with_kb_focus.text_color = text_main;
                style.tab.inactive.text_color = weak_text;
                style.tab.inactive_with_kb_focus.text_color = weak_text;
                style.tab.hovered.text_color = text_bright;
                style.tab.focused.text_color = text_main;
                style.tab.focused_with_kb_focus.text_color = text_main;

                // Place the "+" add-tab button right after the last tab.
                style.buttons.add_tab_align = TabAddAlign::Left;

                // Tab close button — × colour changes on hover but no
                // background highlight (avoids shape mismatch with the
                // tab's top-right corner rounding).
                style.buttons.close_tab_color = weak_text;
                style.buttons.close_tab_active_color = text_bright;
                style.buttons.close_tab_bg_fill = Color32::TRANSPARENT;

                // Paint the full-dock background before DockArea so every
                // tab bar, including bars inside split leaves, stays above
                // the background image.
                if self.background_image_loaded {
                    let background_cb = egui_wgpu::Callback::new_paint_callback(
                        dock_rect,
                        self.callback.background_only(),
                    );
                    ui.painter().add(background_cb);
                }

                // ── Render tabs (nested scope to drop viewer early) ──
                let dock_changed = {
                    let mut layout_changed = false;
                    {
                        let ws = self.workspaces.active_workspace_mut();
                        let mut area = DockArea::new(&mut ws.dock)
                            .style(style)
                            .show_close_buttons(self.config.ui.show_close_tab_button)
                            .show_add_buttons(self.config.ui.show_add_tab_button)
                            .show_leaf_collapse_buttons(false)
                            .show_leaf_close_all_buttons(false);
                        area = area.id(Id::new("zenterm_dock"));

                        let mut viewer = TabViewerContext {
                            sessions: &mut self.sessions,
                            active_session_id: &mut self.active_session_id,
                            pending_close: &mut self.pending_close,
                            pending_adds: &mut self.pending_adds,
                            pending_rename: &mut self.pending_rename,
                            show_active_indicator,
                            background_active: self.background_image_loaded,
                            terminal_padding: egui::vec2(
                                self.config.window.padding.x,
                                self.config.window.padding.y,
                            ),
                            layout_changed: &mut layout_changed,
                        };
                        area.show_inside(ui, &mut viewer);
                    }
                    layout_changed
                }; // viewer dropped → self.sessions borrow released
                if dock_changed {
                    self.workspaces.active_workspace_mut().mark_changed();
                    self.mark_layout_dirty();
                }

                // Paint terminal cells after DockArea. Cell geometry is
                // limited to tab bodies, so it does not cover the bars.
                let cb =
                    egui_wgpu::Callback::new_paint_callback(dock_rect, self.callback.cells_only());
                ui.painter().add(cb);

                // ── Transient resize overlay ─────────────────────────
                // For every session whose terminal was recently resized,
                // draw a centred "cols × rows" label that fades out over
                // 2 seconds.  Painted after the wgpu callback so it
                // appears on top of the terminal content.
                let ppp = ui.ctx().pixels_per_point();
                for session in self.sessions.values() {
                    if let Some(rect) = session.resize_overlay_rect(ppp) {
                        session.render_resize_overlay(ui, rect);
                    }
                }

                // ── Badge overlay (OSC 1337 SetBadgeFormat) ────────────
                // Renders a large text label in the top-right corner of
                // each session's viewport.
                for session in self.sessions.values() {
                    if let Some(template) = session.badge_format() {
                        let text = crate::session::render_badge(template, session);
                        if !text.is_empty() {
                            let ppp = ui.ctx().pixels_per_point();
                            let vp_rect = session.viewport_rect(ppp);
                            let font_size = (session.cell_height() * 2.0).max(14.0);
                            ui.painter().text(
                                egui::pos2(vp_rect.right() - 8.0, vp_rect.top() + 8.0),
                                egui::Align2::RIGHT_TOP,
                                &text,
                                egui::FontId::proportional(font_size),
                                ui.visuals().weak_text_color.unwrap_or_else(|| {
                                    ui.visuals().text_color().gamma_multiply(0.65)
                                }),
                            );
                        }
                    }
                }
            });

        // ── Tab rename dialog (modal, rendered outside the dock area) ──
        if let Some(rename_id) = self.pending_rename {
            let buf_id = egui::Id::new(("tab_rename_buf", rename_id.0));

            // Pre-fill with the current effective title.
            let initial = self
                .sessions
                .get(&rename_id)
                .map(|s| s.title_effective())
                .unwrap_or_default();

            let mut buf: String = ui
                .ctx()
                .data(|d| d.get_temp::<String>(buf_id).unwrap_or(initial));

            let ctx = ui.ctx();
            let area_id = egui::Id::new(("tab_rename_area", rename_id.0));
            let input_id = egui::Id::new(("tab_rename_dialog_input", rename_id.0));
            let has_saved_buffer = ctx.data(|d| d.get_temp::<String>(buf_id).is_some());
            let mut close_requested = false;

            egui::Area::new(area_id)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(&ctx.global_style())
                        .inner_margin(egui::Margin::symmetric(16, 12))
                        .show(ui, |ui| {
                            ui.set_min_width(280.0);
                            ui.strong("Rename Tab");
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
                                if !buf.is_empty()
                                    && let Some(s) = self.sessions.get_mut(&rename_id)
                                {
                                    s.set_title_override(buf.clone());
                                }
                                close_requested = true;
                            } else if cancel {
                                close_requested = true;
                            }

                            ui.add_space(14.0);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("OK").clicked() {
                                        if !buf.is_empty()
                                            && let Some(s) = self.sessions.get_mut(&rename_id)
                                        {
                                            s.set_title_override(buf.clone());
                                        }
                                        close_requested = true;
                                    }
                                    ui.add_space(8.0);
                                    if ui.button("Cancel").clicked() {
                                        close_requested = true;
                                    }
                                },
                            );
                        });
                });

            if close_requested {
                self.pending_rename = None;
                ctx.data_mut(|d| {
                    d.remove_temp::<String>(buf_id);
                });
            } else {
                ctx.data_mut(|d| {
                    d.insert_temp::<String>(buf_id, buf);
                });
            }
        }

        // ── Apply pending actions collected by the viewer ─────────
        let added = self.pending_adds;
        if added > 0 {
            for _ in 0..added {
                self.spawn_session();
            }
        }
        // Drain to a local first to avoid a borrow conflict with
        // `self.close_session` (which mutably borrows `self`).
        let to_close: Vec<SessionId> = std::mem::take(&mut self.pending_close);
        for id in to_close {
            self.close_session(id);
        }
    }
}
