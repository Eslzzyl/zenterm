//! Keyboard input forwarding and shortcut handling.
//!
//! Routes egui events to the active terminal session and processes
//! application-wide keyboard shortcuts.

use alacritty_terminal::term::TermMode;
use egui::Context;

use zenterm_input::MappingOptions;

use super::ZentermApp;
impl ZentermApp {
    pub(crate) fn forward_event_to_active(&mut self, event: &egui::Event) {
        if let Some(id) = self.active_session_id
            && let Some(session) = self.sessions.get_mut(&id)
        {
            // User input counts as activity — restart the cursor blink
            // timeout window.
            if matches!(
                event,
                egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_)
            ) {
                session.note_input_activity();
            }

            // Before PTY mapping, check for IME state events that
            // update the preedit text but are not sent to the PTY.
            if let egui::Event::Ime(ime_event) = event {
                match ime_event {
                    egui::ImeEvent::Preedit(text) => {
                        // Force a full re-render so the preedit text is
                        // drawn through the GPU glyph pipeline (the fast
                        // path in update_cell_instances skips preedit).
                        session.set_preedit_text(Some(text.clone()));
                    }
                    egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled => {
                        session.set_preedit_text(None);
                    }
                    egui::ImeEvent::Enabled => {}
                }
            }

            // Build mapping options from terminal state + config.
            let mode = session.terminal().mode();
            let opts = MappingOptions {
                app_cursor: mode.contains(TermMode::APP_CURSOR),
                macos_option_as_alt: self.config.window.macos_option_as_alt,
                kitty_flags: session.terminal().kitty_keyboard_flags(),
            };

            // Map event to PTY bytes (handles Commit, Text, Key, Paste).
            if let Some(bytes) = zenterm_input::InputMapper::map(event, &opts)
                && let Err(e) = session.pty_mut().write(&bytes)
            {
                log::error!("PTY write error: {e}");
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
        // Once the command palette owns the interaction, keep all other
        // application shortcuts and PTY input out of the search field.
        if self.command_palette.open {
            return true;
        }

        // Allow Escape to close the settings viewport when the main window
        // owns keyboard focus.  The settings viewport handles its own Escape
        // event when it is focused.
        if self.settings_state.open && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.settings_state.open = false;
            ctx.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            });
            return true;
        }

        let (copy, paste, reload, settings, command_palette, ws_switch, ws_cycle) =
            ctx.input(|input| {
                let mut c = false;
                let mut p = false;
                let mut r = false;
                let mut s = false;
                let mut palette = false;
                let mut ws_switch: Option<usize> = None;
                let mut ws_cycle: Option<isize> = None;
                for event in &input.events {
                    // Catch Event::Copy from egui-winit (Windows/Linux: Ctrl+C/Ctrl+Shift+C,
                    // macOS: Cmd+C).  When there's a selection, copy it; without selection,
                    // fall through to InputMapper which sends SIGINT on non-macOS.
                    if matches!(event, egui::Event::Copy) {
                        c = true;
                    }
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        repeat,
                        modifiers,
                        ..
                    } = event
                    {
                        // Ctrl+Shift+C / V / R
                        let shift_ctrl = modifiers.ctrl && modifiers.shift && !modifiers.alt;
                        if shift_ctrl {
                            match key {
                                egui::Key::C => {
                                    log::info!(
                                        "[clipboard] Ctrl+Shift+C detected, setting copy=true"
                                    );
                                    c = true;
                                }
                                egui::Key::V => p = true,
                                egui::Key::R => r = true,
                                _ => {}
                            }
                        }
                        // Ctrl+Shift+P (Cmd+Shift+P on macOS) → command palette.
                        // Ignore repeats so holding the key cannot reset the query.
                        if !*repeat
                            && (modifiers.ctrl || modifiers.mac_cmd)
                            && modifiers.shift
                            && !modifiers.alt
                            && *key == egui::Key::P
                        {
                            palette = true;
                        }
                        // Cmd/Ctrl+, → toggle settings panel
                        if (modifiers.ctrl || modifiers.mac_cmd)
                            && !modifiers.shift
                            && !modifiers.alt
                            && *key == egui::Key::Comma
                        {
                            s = true;
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
                        // Ctrl+Insert → copy, Shift+Insert → paste
                        if *key == egui::Key::Insert {
                            if modifiers.ctrl && !modifiers.shift && !modifiers.alt {
                                c = true;
                            } else if modifiers.shift && !modifiers.ctrl && !modifiers.alt {
                                p = true;
                            }
                        }
                    }
                }
                (c, p, r, s, palette, ws_switch, ws_cycle)
            });
        if command_palette {
            self.open_command_palette();
            return true;
        }
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
        if let Some(idx) = ws_switch
            && let Some(ws) = self.workspaces.workspaces.get(idx)
        {
            let ws_id = ws.id;
            self.workspaces.switch_to(ws_id);
            self.focus_first_tab_in_active_workspace();
            self.mark_layout_dirty();
            return true;
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
                self.focus_first_tab_in_active_workspace();
                self.mark_layout_dirty();
                return true;
            }
        }
        if copy {
            log::warn!(
                "[clipboard] entering copy handler, active_session_id={:?}",
                self.active_session_id
            );
            if let Some(id) = self.active_session_id {
                if let Some(session) = self.sessions.get_mut(&id) {
                    let has_sel = session.terminal().has_selection();
                    log::warn!("[clipboard] session found, has_selection={has_sel}");
                    if has_sel {
                        if let Some(text) = session.terminal().selected_text() {
                            log::warn!("[clipboard] selected_text len={}", text.len());
                            if let Some(cb) = session.clipboard_mut() {
                                log::warn!("[clipboard] clipboard Some, calling set_text");
                                match cb.set_text(text) {
                                    Ok(_) => log::warn!("[clipboard] *** set_text SUCCESS ***"),
                                    Err(e) => log::error!("[clipboard] set_text FAILED: {e}"),
                                }
                            } else {
                                log::error!(
                                    "[clipboard] clipboard is None (arboard init failed at session creation)"
                                );
                            }
                            return true;
                        } else {
                            log::warn!(
                                "[clipboard] has_selection=true but selected_text() returned None"
                            );
                        }
                    }
                } else {
                    log::error!("[clipboard] get_mut returned None for session");
                }
            } else {
                log::error!("[clipboard] active_session_id is None");
            }
        }
        if paste
            && let Some(id) = self.active_session_id
            && let Some(session) = self.sessions.get_mut(&id)
        {
            let text = session
                .clipboard_mut()
                .and_then(|clipboard| clipboard.get_text().ok())
                .filter(|text| !text.is_empty());
            if let Some(text) = text {
                if let Err(e) = session.pty_mut().write(text.as_bytes()) {
                    log::error!("PTY paste error: {e}");
                }
                return true;
            }
        }

        // ── Terminal scroll shortcuts (PageUp/Down/Home/End) ─────
        let no_ui_focus = !ctx.memory(|m| m.focused().is_some());
        if no_ui_focus
            && let Some(id) = self.active_session_id
            && let Some(session) = self.sessions.get_mut(&id)
        {
            if !session.terminal().mode().contains(TermMode::ALT_SCREEN) {
                log::info!("[dbg] keyboard: NOT alt_screen → consuming PageUp/Down for scrollback");
                let rows = session.terminal().size().rows as i32;
                let mut scrolled = false;
                ctx.input(|input| {
                    for event in &input.events {
                        if let egui::Event::Key {
                            key, pressed: true, ..
                        } = event
                        {
                            match key {
                                egui::Key::PageUp => {
                                    session.terminal_mut().scroll_display(rows);
                                    scrolled = true;
                                }
                                egui::Key::PageDown => {
                                    session.terminal_mut().scroll_display(-rows);
                                    scrolled = true;
                                }
                                egui::Key::Home => {
                                    session.terminal_mut().scroll_to_top();
                                    scrolled = true;
                                }
                                egui::Key::End => {
                                    session.terminal_mut().scroll_to_bottom();
                                    scrolled = true;
                                }
                                _ => {}
                            }
                        }
                    }
                });
                if scrolled {
                    session.mark_terminal_dirty();
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
                log::info!(
                    "[dbg] keyboard: ALT_SCREEN active → PageUp/Down/Home/End will be forwarded to PTY"
                );
            }
        }

        false
    }
}
