//! Keyboard input forwarding and shortcut handling.
//!
//! Routes egui events to the active terminal session and processes
//! application-wide keyboard shortcuts.

use egui::Context;
use alacritty_terminal::term::TermMode;

use super::ZentermApp;
impl ZentermApp {
    pub(crate) fn forward_event_to_active(&mut self, event: &egui::Event) {
        if let Some(id) = self.active_session_id {
            if let Some(session) = self.sessions.get_mut(&id) {
                // Before PTY mapping, check for IME state events that
                // update the preedit text but are not sent to the PTY.
                if let egui::Event::Ime(ime_event) = event {
                    match ime_event {
                        egui::ImeEvent::Preedit(text) => {
                            if text.is_empty() {
                                session.preedit_text = None;
                            } else {
                                session.preedit_text = Some(text.clone());
                            }
                            // Force a full re-render so the preedit text is
                            // drawn through the GPU glyph pipeline (the fast
                            // path in update_cell_instances skips preedit).
                            session.terminal_dirty = true;
                        }
                        egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled => {
                            session.preedit_text = None;
                            session.terminal_dirty = true;
                        }
                        egui::ImeEvent::Enabled => {}
                    }
                }

                // Map event to PTY bytes (handles Commit, Text, Key, Paste).
                if let Some(bytes) = zenterm_input::InputMapper::map(event) {
                    if let Err(e) = session.pty.write(&bytes) {
                        log::error!("PTY write error: {e}");
                    }
                }
            }
        }
    }

    /// Forward keyboard input to the active terminal session.
    ///
    /// If a UI widget currently has keyboard focus (e.g. a sidebar
    /// rename text-edit or a focused button), the events are left for
    /// egui to handle normally — the terminal does not receive them.
    ///
    /// When no UI widget is focused, all key events are forwarded to
    /// the active PTY session via [`InputMapper`].  We also suppress
    /// egui's built-in focus-navigation for Tab / Shift+Tab so that
    /// pressing Tab in the terminal doesn't accidentally move focus
    /// to sidebar buttons.
    pub(crate) fn feed_keyboard_to_active(&mut self, ctx: &Context) {
        // If a UI widget is focused, keyboard input belongs to it,
        // not to the terminal.
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }

        // Forward all key events to the active PTY session.
        ctx.input(|input| {
            for event in &input.events {
                self.forward_event_to_active(event);
            }
        });

        // Suppress egui's focus-navigation for Tab/Shift+Tab.
        // egui sets the internal `focus_direction` during
        // `begin_pass()` (before `update`), so we must both consume
        // the event from the input state AND reset the direction.
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab));
        ctx.memory_mut(|mem| mem.move_focus(egui::FocusDirection::None));
    }

    /// Handle app-level keyboard shortcuts.
    ///
    /// Returns `true` if a shortcut was consumed (skip forwarding to
    /// the active session).
    pub(crate) fn handle_shortcuts(&mut self, ctx: &Context) -> bool {
        let (copy, paste, reload, settings, ws_switch, ws_cycle) = ctx.input(|input| {
            let mut c = false;
            let mut p = false;
            let mut r = false;
            let mut s = false;
            let mut ws_switch: Option<usize> = None;
            let mut ws_cycle: Option<isize> = None;
            for event in &input.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                {
                    // Ctrl+Shift+C / V / R
                    let shift_ctrl = modifiers.ctrl && modifiers.shift && !modifiers.alt;
                    if shift_ctrl {
                        match key {
                            egui::Key::C => c = true,
                            egui::Key::V => p = true,
                            egui::Key::R => r = true,
                            _ => {}
                        }
                    }
                    // Cmd/Ctrl+, → toggle settings panel
                    if (modifiers.ctrl || modifiers.mac_cmd) && !modifiers.shift && !modifiers.alt {
                        if *key == egui::Key::Comma {
                            s = true;
                        }
                    }
                    // Ctrl+1..9 → switch to workspace by index
                    if modifiers.ctrl && !modifiers.shift && !modifiers.alt {
                        match key {
                            egui::Key::Num1 => ws_switch = Some(0),
                            egui::Key::Num2 => ws_switch = Some(1),
                            egui::Key::Num3 => ws_switch = Some(2),
                            egui::Key::Num4 => ws_switch = Some(3),
                            egui::Key::Num5 => ws_switch = Some(4),
                            egui::Key::Num6 => ws_switch = Some(5),
                            egui::Key::Num7 => ws_switch = Some(6),
                            egui::Key::Num8 => ws_switch = Some(7),
                            egui::Key::Num9 => ws_switch = Some(8),
                            _ => {}
                        }
                    }
                    // Ctrl+Tab → next workspace, Ctrl+Shift+Tab → prev
                    if modifiers.ctrl && !modifiers.alt {
                        match key {
                            egui::Key::Tab if !modifiers.shift => ws_cycle = Some(1),
                            egui::Key::Tab if modifiers.shift => ws_cycle = Some(-1),
                            _ => {}
                        }
                    }
                }
            }
            (c, p, r, s, ws_switch, ws_cycle)
        });
        if reload {
            self.reload_config(ctx);
            return true;
        }
        if settings {
            self.settings_state.open = !self.settings_state.open;
            if self.settings_state.open {
                // Reset working config to current when opening.
                self.settings_state.reset_to(&self.config);
            }
            return true;
        }
        // Workspace switching shortcuts.
        if let Some(idx) = ws_switch {
            if let Some(ws) = self.workspaces.workspaces.get(idx) {
                let ws_id = ws.id;
                self.workspaces.switch_to(ws_id);
                self.active_session_id = self
                    .workspaces
                    .active_workspace()
                    .all_tab_ids()
                    .first()
                    .copied();
                self.mark_layout_dirty();
                return true;
            }
        }
        if let Some(dir) = ws_cycle {
            let len = self.workspaces.workspaces.len();
            if len > 0 {
                let current_idx = self
                    .workspaces
                    .workspaces
                    .iter()
                    .position(|ws| ws.id == self.workspaces.active_workspace_id)
                    .unwrap_or(0);
                let new_idx = ((current_idx as isize + dir).rem_euclid(len as isize)) as usize;
                let ws_id = self.workspaces.workspaces[new_idx].id;
                self.workspaces.switch_to(ws_id);
                self.active_session_id = self
                    .workspaces
                    .active_workspace()
                    .all_tab_ids()
                    .first()
                    .copied();
                self.mark_layout_dirty();
                return true;
            }
        }
        if copy {
            if let Some(id) = self.active_session_id {
                if let Some(session) = self.sessions.get_mut(&id) {
                    if session.terminal.has_selection() {
                        if let Some(text) = session.terminal.selected_text() {
                            ctx.copy_text(text);
                            return true;
                        }
                    }
                }
            }
        }
        if paste {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                if let Ok(text) = cb.get_text() {
                    if !text.is_empty() {
                        if let Some(id) = self.active_session_id {
                            if let Some(session) = self.sessions.get_mut(&id) {
                                if let Err(e) = session.pty.write(text.as_bytes()) {
                                    log::error!("PTY paste error: {e}");
                                }
                                return true;
                            }
                        }
                    }
                }
            }
        }

        // ── Terminal scroll shortcuts (PageUp/Down/Home/End) ─────
        let no_ui_focus = !ctx.memory(|m| m.focused().is_some());
        if no_ui_focus {
            if let Some(id) = self.active_session_id {
                if let Some(session) = self.sessions.get_mut(&id) {
                    if !session.terminal.mode().contains(TermMode::ALT_SCREEN) {
                        log::info!("[dbg] keyboard: NOT alt_screen → consuming PageUp/Down for scrollback");
                        let rows = session.terminal.size().rows as i32;
                        let mut scrolled = false;
                        ctx.input(|input| {
                            for event in &input.events {
                                if let egui::Event::Key {
                                    key,
                                    pressed: true,
                                    ..
                                } = event
                                {
                                    match key {
                                        egui::Key::PageUp => {
                                            session.terminal.scroll_display(rows);
                                            scrolled = true;
                                        }
                                        egui::Key::PageDown => {
                                            session.terminal.scroll_display(-rows);
                                            scrolled = true;
                                        }
                                        egui::Key::Home => {
                                            session.terminal.scroll_to_top();
                                            scrolled = true;
                                        }
                                        egui::Key::End => {
                                            session.terminal.scroll_to_bottom();
                                            scrolled = true;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        });
                        if scrolled {
                            session.terminal_dirty = true;
                            // Consume the scroll keys so they aren't forwarded to the PTY.
                            ctx.input_mut(|i| {
                                i.events.retain(|e| {
                                    !matches!(
                                        e,
                                        egui::Event::Key {
                                            key: egui::Key::PageUp
                                            | egui::Key::PageDown
                                            | egui::Key::Home
                                            | egui::Key::End,
                                            pressed: true,
                                            ..
                                        }
                                    )
                                })
                            });
                            return true;
                        }
                    } else {
                        log::info!("[dbg] keyboard: ALT_SCREEN active → PageUp/Down/Home/End will be forwarded to PTY");
                    }
                }
            }
        }

        log::info!("[dbg] keyboard: handle_shortcuts returning false (no shortcut consumed)");

        false
    }

}
