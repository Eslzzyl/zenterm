//! Keyboard input encoding.
//!
//! Maps egui keyboard events into the byte sequences that the shell
//! expects.  Handles:
//!
//! - Plain ASCII text via `Event::Text`
//! - Ctrl+letter → C0 control codes (`0x01`–`0x1a`)
//! - Ctrl+digit/symbol → extended C0 codes (`0x00`, `0x1b`–`0x1f`, `0x7f`)
//! - Alt+letter/digit → `ESC` + character
//! - Modifier+arrow/Home/End → `CSI 1 ; {mod} {letter}` (xterm style)
//! - Modifier+PageUp/Down/Insert/Delete → `CSI {n} ; {mod} ~`
//! - Modifier+F1–F12 → `CSI {n} ; {mod} ~` or `SS3` prefix
//! - Application mode cursor keys → `SS3 {letter}` (when `app_cursor` is set)
//! - Clipboard paste via `Event::Paste`
//!
//! # CSI-u modifier encoding (xterm / Kitty legacy)
//!
//! | Modifiers                | Index | Example            |
//! |--------------------------|-------|--------------------|
//! | (none)                   | 1     | `\x1b[A`           |
//! | Shift                    | 2     | `\x1b[1;2A`        |
//! | Alt                      | 3     | `\x1b[1;3A`        |
//! | Alt+Shift                | 4     | `\x1b[1;4A`        |
//! | Ctrl                     | 5     | `\x1b[1;5A`        |
//! | Ctrl+Shift               | 6     | `\x1b[1;6A`        |
//! | Ctrl+Alt                 | 7     | `\x1b[1;7A`        |
//! | Ctrl+Alt+Shift           | 8     | `\x1b[1;8A`        |

use egui::Key;

mod keys;
mod sequences;
#[cfg(test)]
mod tests;

use self::keys::{key_to_ascii, key_to_ctrl_code, key_to_ctrl_extended};
use self::sequences::{cursor_seq, fkey_seq, tilde_seq, tilde_seq_raw};

/// Options that affect key encoding behaviour.
///
/// These are determined by the terminal state (DEC modes) and user
/// configuration, and are passed to [`InputMapper::map`] on each call.
#[derive(Debug, Clone, Copy, Default)]
pub struct MappingOptions {
    /// DEC mode 1 — application cursor keys.
    ///
    /// When `true`, unmodified arrow / Home / End keys send `SS3`
    /// sequences (`\x1bOA` etc.) instead of CSI sequences (`\x1b[A`).
    /// When modifiers are pressed the standard CSI form is used
    /// regardless.
    pub app_cursor: bool,

    /// macOS-only: treat the Option key as Alt.
    ///
    /// When `true`, Option+key sends `ESC` + ASCII character (like
    /// Alt on Windows/Linux).  When `false`, Option composes Unicode
    /// characters via the macOS keyboard layout.
    pub macos_option_as_alt: bool,
}

impl MappingOptions {
    /// Default options: both flags off.
    pub const fn new() -> Self {
        Self {
            app_cursor: false,
            macos_option_as_alt: false,
        }
    }
}

/// Maps egui input events to terminal byte sequences.
pub struct InputMapper;

impl InputMapper {
    /// Convert an [`egui::Event`] into bytes to write to the PTY.
    ///
    /// `opts` controls encoding variants such as application-mode
    /// cursor keys and macOS Option behaviour.  Use
    /// [`MappingOptions::new()`] or `&Default::default()` for the
    /// default behaviour.
    ///
    /// Returns `None` if the event should be ignored (e.g. it was
    /// already consumed by egui).
    pub fn map(event: &egui::Event, opts: &MappingOptions) -> Option<Vec<u8>> {
        match event {
            egui::Event::Text(text) => {
                // Printable text — send the UTF-8 bytes directly.
                if text.is_empty() {
                    return None;
                }
                // Filter out control characters that egui sometimes
                // delivers as text.
                let has_printable =
                    text.chars().any(|c| !c.is_control() && c != '\n' && c != '\r');
                if !has_printable {
                    return None;
                }
                Some(text.as_bytes().to_vec())
            }

            egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                modifiers,
                ..
            } => {
                let ctrl = modifiers.ctrl;
                let alt = modifiers.alt;
                let shift = modifiers.shift;
                let mod_idx = modifier_index(ctrl, alt, shift);

                match key {
                    // ── Enter ──────────────────────────────────────────
                    Key::Enter => {
                        if ctrl {
                            Some(b"\x1b[13;5~".to_vec()) // Ctrl+Enter
                        } else if alt {
                            Some(b"\x1b\r".to_vec()) // Alt+Enter
                        } else {
                            Some(vec![b'\r'])
                        }
                    }

                    // ── Backspace ──────────────────────────────────────
                    Key::Backspace => {
                        if ctrl {
                            Some(vec![0x08]) // Ctrl+Backspace = Ctrl+H = BS
                        } else if alt {
                            Some(vec![0x1b, 0x7f]) // Alt+Backspace = ESC + DEL
                        } else {
                            Some(vec![0x7f]) // DEL
                        }
                    }

                    // ── Tab ────────────────────────────────────────────
                    Key::Tab => {
                        if ctrl {
                            Some(b"\x1b[9;5~".to_vec()) // Ctrl+Tab
                        } else if shift {
                            Some(vec![0x1b, b'Z']) // ESC Z = backward tab
                        } else {
                            Some(vec![b'\t'])
                        }
                    }

                    // ── Escape ─────────────────────────────────────────
                    Key::Escape => {
                        if alt {
                            // Alt+Esc — some terminals send ESC ESC
                            Some(b"\x1b\x1b".to_vec())
                        } else {
                            Some(vec![0x1b])
                        }
                    }

                    // ── Arrow keys ─────────────────────────────────────
                    Key::ArrowUp => Some(cursor_seq(b'A', mod_idx, opts.app_cursor)),
                    Key::ArrowDown => Some(cursor_seq(b'B', mod_idx, opts.app_cursor)),
                    Key::ArrowRight => Some(cursor_seq(b'C', mod_idx, opts.app_cursor)),
                    Key::ArrowLeft => Some(cursor_seq(b'D', mod_idx, opts.app_cursor)),

                    // ── Home / End ─────────────────────────────────────
                    Key::Home => Some(cursor_seq(b'H', mod_idx, opts.app_cursor)),
                    Key::End => Some(cursor_seq(b'F', mod_idx, opts.app_cursor)),

                    // ── Page Up / Down ─────────────────────────────────
                    Key::PageUp => Some(tilde_seq(b'5', mod_idx)),
                    Key::PageDown => Some(tilde_seq(b'6', mod_idx)),

                    // ── Insert / Delete ────────────────────────────────
                    Key::Insert => Some(tilde_seq(b'2', mod_idx)),
                    Key::Delete => {
                        if ctrl {
                            Some(tilde_seq(b'3', 5)) // Ctrl+Delete
                        } else if alt {
                            Some(tilde_seq(b'3', 3)) // Alt+Delete
                        } else {
                            Some(tilde_seq(b'3', 1)) // plain Delete
                        }
                    }

                    // ── Function keys ──────────────────────────────────
                    Key::F1 => Some(fkey_seq(b'P', mod_idx)),
                    Key::F2 => Some(fkey_seq(b'Q', mod_idx)),
                    Key::F3 => Some(fkey_seq(b'R', mod_idx)),
                    Key::F4 => Some(fkey_seq(b'S', mod_idx)),
                    Key::F5 => Some(tilde_seq_raw(15, mod_idx)),
                    Key::F6 => Some(tilde_seq_raw(17, mod_idx)),
                    Key::F7 => Some(tilde_seq_raw(18, mod_idx)),
                    Key::F8 => Some(tilde_seq_raw(19, mod_idx)),
                    Key::F9 => Some(tilde_seq_raw(20, mod_idx)),
                    Key::F10 => Some(tilde_seq_raw(21, mod_idx)),
                    Key::F11 => Some(tilde_seq_raw(23, mod_idx)),
                    Key::F12 => Some(tilde_seq_raw(24, mod_idx)),

                    // ── Letters (A–Z), digits, symbols ────────────────
                    _ => handle_ctrl_or_alt(key, physical_key.as_ref(), ctrl, alt, shift, opts),
                }
            }

            egui::Event::Paste(text) => {
                // Clipboard paste — send the UTF-8 bytes directly.
                if text.is_empty() {
                    None
                } else {
                    Some(text.as_bytes().to_vec())
                }
            }

            egui::Event::Ime(ime_event) => {
                // IME input (e.g. Chinese/Japanese/Korean IME composition).
                match ime_event {
                    // When the IME commits final text (user selected a candidate),
                    // send the UTF-8 bytes to the PTY — same as Text events.
                    egui::ImeEvent::Commit(text) => {
                        if text.is_empty() {
                            None
                        } else {
                            Some(text.as_bytes().to_vec())
                        }
                    }
                    // Preedit is intermediate composition state (still selecting
                    // candidates); do not send to PTY.
                    // Enabled/Disabled are IME activation state changes; ignored.
                    _ => None,
                }
            }

            // Copy / Cut events are handled by the app layer; do not forward.
            egui::Event::Copy | egui::Event::Cut => None,

            _ => None,
        }
    }
}

// ── Helper functions ────────────────────────────────────────────────────

/// Compute the CSI-u modifier index (1–8).
///
/// The index is a 3-bit bitfield: 1 + shift*1 + alt*2 + ctrl*4.
/// Value 1 (no modifiers) can be omitted in most sequences.
#[inline]
fn modifier_index(ctrl: bool, alt: bool, shift: bool) -> u8 {
    1 + (shift as u8) + (alt as u8) * 2 + (ctrl as u8) * 4
}

/// Build a cursor/Home/End sequence.
///
/// When `mod_idx == 1` (no modifiers):
/// - Normal mode: `\x1b[{letter}` (e.g. `\x1b[A`)
/// - Application mode (`app_cursor = true`): `\x1bO{letter}` (e.g. `\x1bOA`)
///
/// When modifiers are present the CSI form `\x1b[1;{mod}{letter}` is
/// used regardless of application mode.

/// Handle Ctrl+key and Alt+key for letters, digits, and symbols.
///
/// `physical_key` is the key at its physical keyboard position (ignoring
/// layout), used as a fallback on non-Latin keyboard layouts where the
/// logical key may not map to an ASCII letter (e.g. Cyrillic layouts
/// where the Ctrl+C physical position produces a non-Latin logical key).
fn handle_ctrl_or_alt(
    key: &Key,
    physical_key: Option<&Key>,
    ctrl: bool,
    alt: bool,
    shift: bool,
    opts: &MappingOptions,
) -> Option<Vec<u8>> {
    // Ctrl+key
    if ctrl && !alt {
        // Ctrl+letter (A–Z) → C0 control codes 0x01–0x1a
        if let Some(code) = key_to_ctrl_code(key) {
            return Some(vec![code]);
        }
        // Try physical key fallback for non-Latin keyboard layouts.
        // On Russian etc. the logical key may not be A–Z even though
        // the physical key position is correct.
        if let Some(pk) = physical_key {
            if pk != key {
                if let Some(code) = key_to_ctrl_code(pk) {
                    return Some(vec![code]);
                }
            }
        }
        // Ctrl+digit / Ctrl+symbol → extended C0 codes
        if let Some(code) = key_to_ctrl_extended(key) {
            return Some(vec![code]);
        }
        // Try physical key fallback for extended codes too.
        if let Some(pk) = physical_key {
            if pk != key {
                if let Some(code) = key_to_ctrl_extended(pk) {
                    return Some(vec![code]);
                }
            }
        }
    }

    // Alt+key → ESC + character
    //
    // On macOS, Option by default composes Unicode characters rather
    // than acting as Alt.  The `macos_option_as_alt` option (from the
    // user config) overrides this so that Option can be used as Alt
    // just like on Windows/Linux.
    if alt && !ctrl {
        let handle_alt = if cfg!(target_os = "macos") {
            opts.macos_option_as_alt
        } else {
            true
        };
        if handle_alt {
            if let Some(mut byte) = key_to_ascii(key) {
                if shift {
                    byte = byte.to_ascii_uppercase();
                }
                return Some(vec![0x1b, byte]);
            }
            // Try physical key fallback for Alt too.
            if let Some(pk) = physical_key {
                if pk != key {
                    if let Some(mut byte) = key_to_ascii(pk) {
                        if shift {
                            byte = byte.to_ascii_uppercase();
                        }
                        return Some(vec![0x1b, byte]);
                    }
                }
            }
        }
    }

    None
}
