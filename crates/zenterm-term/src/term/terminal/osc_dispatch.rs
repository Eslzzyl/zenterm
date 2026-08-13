//! Side effects produced by structured OSC matches.

use alacritty_terminal::vte::ansi::{Handler, NamedColor};

use zenterm_core::{ITermProprietary, ITermUnicodeVersionOp};

use crate::term::osc::{OscMatch, parse_conemu_progress, parse_iterm_proprietary, parse_osc133};

use super::Terminal;

impl Terminal {
    /// Apply one OSC side effect and append protocol responses to `replies`.
    pub(super) fn dispatch_osc(&mut self, osc: &OscMatch, replies: &mut Vec<u8>) {
        match osc.number {
            7 => {
                self.pending_current_directory = Some(osc.payload.clone());
            }
            9 => {
                if let Some(progress) = parse_conemu_progress(&osc.payload) {
                    self.pending_progress = Some(progress);
                } else {
                    self.pending_notification = Some(("Zenterm".into(), osc.payload.clone()));
                }
            }
            777 => {
                let mut parts = osc.payload.splitn(3, ';');
                let _maybe_notify = parts.next();
                let title = parts.next().unwrap_or("").to_string();
                let body = parts.next().unwrap_or("").to_string();
                self.pending_notification = Some((title, body));
            }
            10..=12 => {
                let payload = osc.payload.trim();
                if payload != "?"
                    && let Some(rgb) = super::parse_osc_hex_rgb(payload)
                {
                    match osc.number {
                        10 => self.term.set_color(NamedColor::Foreground as usize, rgb),
                        11 => self.term.set_color(NamedColor::Background as usize, rgb),
                        12 => {
                            self.set_cursor_color(rgb);
                            self.damage.mark_all();
                        }
                        _ => unreachable!(),
                    }
                }
            }
            110..=112 => match osc.number {
                110 => self.term.reset_color(NamedColor::Foreground as usize),
                111 => self.term.reset_color(NamedColor::Background as usize),
                112 => self.reset_cursor_color(),
                _ => unreachable!(),
            },
            99 => {
                let (notification, response) = self.kitty_state.handle_event(&osc.payload, "");
                if let Some(notification) = notification {
                    let title = if notification.title.is_empty() {
                        notification.body.clone()
                    } else {
                        notification.title.clone()
                    };
                    let body = if notification.title.is_empty() {
                        String::new()
                    } else {
                        notification.body.clone()
                    };
                    self.pending_notification = Some((title, body));
                    self.pending_kitty_notification = Some(notification);
                }
                if let Some(response) = response {
                    log::debug!("Terminal::feed: OSC 99 response: {response}");
                    replies.extend_from_slice(response.as_bytes());
                }
            }
            133 => {
                if let Some(prompt) = parse_osc133(&osc.payload) {
                    // OSC 133 is semantic metadata. The shell is
                    // responsible for emitting the actual cursor motion
                    // and line breaks; injecting CR/LF here would alter the
                    // visible terminal state and breaks fish redraws.
                    self.pending_semantic_prompt = Some(prompt);
                }
            }
            1337 => self.dispatch_iterm_osc(&osc.payload, replies),
            _ => {}
        }
    }

    fn dispatch_iterm_osc(&mut self, payload: &str, replies: &mut Vec<u8>) {
        let Some(command) = parse_iterm_proprietary(payload) else {
            return;
        };

        match command {
            ITermProprietary::SetMark => {
                let cursor = self.cursor();
                self.marks.push((cursor.pos.column, cursor.pos.line));
                log::debug!(
                    "SetMark: mark recorded at ({}, {})",
                    cursor.pos.column,
                    cursor.pos.line,
                );
            }
            ITermProprietary::StealFocus => {
                self.pending_iterm_action = Some(ITermProprietary::StealFocus);
            }
            ITermProprietary::ClearScrollback => {
                self.term.grid_mut().clear_history();
                self.damage.mark_all();
            }
            ITermProprietary::CurrentDir(path) => {
                self.pending_current_directory = Some(path);
            }
            ITermProprietary::SetProfile(name) => {
                self.pending_iterm_action = Some(ITermProprietary::SetProfile(name));
            }
            ITermProprietary::HighlightCursorLine(enabled) => {
                self.pending_iterm_action = Some(ITermProprietary::HighlightCursorLine(enabled));
            }
            ITermProprietary::RequestCellSize => {
                if self.cell_pixel_width > 0 && self.cell_pixel_height > 0 {
                    let w = self.cell_pixel_width as f32;
                    let h = self.cell_pixel_height as f32;
                    let response = format!("\x1b]1337;ReportCellSize={h};{w}\x1b\\");
                    log::debug!("Terminal::feed: OSC 1337 RequestCellSize response: {response}");
                    replies.extend_from_slice(response.as_bytes());
                }
            }
            ITermProprietary::ReportCellSize { .. } => {}
            ITermProprietary::Copy(text) => {
                self.pending_clipboard_store = Some(text);
            }
            ITermProprietary::ReportVariable(name) => {
                let value = self
                    .iterm_builtin_var(&name)
                    .or_else(|| self.user_vars.get(&name).cloned());
                let b64_name = crate::term::osc::base64_encode_for_response(name.as_bytes());
                let response = if let Some(value) = value {
                    let b64_value = crate::term::osc::base64_encode_for_response(value.as_bytes());
                    format!("\x1b]1337;ReportVariable={b64_name}={b64_value}\x1b\\")
                } else {
                    format!("\x1b]1337;ReportVariable={b64_name}=\x1b\\")
                };
                log::debug!("Terminal::feed: OSC 1337 ReportVariable({name}) response: {response}");
                replies.extend_from_slice(response.as_bytes());
            }
            ITermProprietary::SetUserVar { name, value } => {
                self.user_vars.insert(name, value);
            }
            ITermProprietary::SetBadgeFormat(format) => {
                self.pending_iterm_action = Some(ITermProprietary::SetBadgeFormat(format));
            }
            ITermProprietary::File(file_data) => {
                if file_data.inline {
                    self.handle_iterm_inline_image(file_data);
                } else {
                    self.pending_iterm_action = Some(ITermProprietary::File(file_data));
                }
            }
            ITermProprietary::UnicodeVersion(op) => match op {
                ITermUnicodeVersionOp::Set(version) => {
                    self.unicode_version = version;
                }
                ITermUnicodeVersionOp::Push(label) => {
                    self.unicode_version_stack
                        .push((self.unicode_version, label));
                }
                ITermUnicodeVersionOp::Pop(label) => {
                    if let Some(label) = label {
                        while let Some((version, old_label)) = self.unicode_version_stack.pop() {
                            self.unicode_version = version;
                            if old_label == Some(label.clone()) {
                                break;
                            }
                        }
                    } else if let Some((version, _)) = self.unicode_version_stack.pop() {
                        self.unicode_version = version;
                    }
                }
            },
        }
    }
}
