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
//! When the Kitty keyboard protocol is active, keys are encoded using the
//! progressive CSI-u format (see [`KittyKeyboardFlags`]).
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
mod kitty;
mod sequences;
#[cfg(test)]
mod tests;

use self::keys::{key_to_ascii, key_to_ctrl_code, key_to_ctrl_extended};
use self::kitty::{lookup as kitty_lookup, Terminator};
use self::sequences::{
    ascii_alternates, csi_u_simple, cursor_seq, fkey_seq, kitty_mod_idx, kitty_seq,
    modifier_index, tilde_seq, tilde_seq_raw,
};

bitflags::bitflags! {
    /// Flags for the Kitty keyboard protocol.
    ///
    /// These correspond to the bit positions used in the `CSI ? {flags} u`
    /// query and the `KeyboardModes` type in `vte::ansi`.
    ///
    /// | Bit | Constant                   | Meaning                                    |
    /// |-----|----------------------------|--------------------------------------------|
    /// | 0   | `DISAMBIGUATE_ESCAPE_CODES`| Send CSI-u sequences for modified keys     |
    /// | 1   | `REPORT_EVENT_TYPES`       | Include press/repeat/release event type    |
    /// | 2   | `REPORT_ALTERNATE_KEYS`    | Include shifted/unshifted alternate codes  |
    /// | 3   | `REPORT_ALL_KEYS_AS_ESCAPE_CODES`| Every key produces a CSI sequence    |
    /// | 4   | `REPORT_ASSOCIATED_TEXT`   | Include the generated text code points     |
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct KittyKeyboardFlags: u8 {
        const NONE                          = 0;
        const DISAMBIGUATE_ESCAPE_CODES     = 0b00001;
        const REPORT_EVENT_TYPES            = 0b00010;
        const REPORT_ALTERNATE_KEYS         = 0b00100;
        const REPORT_ALL_KEYS_AS_ESCAPE_CODES = 0b01000;
        const REPORT_ASSOCIATED_TEXT        = 0b10000;
    }
}

impl KittyKeyboardFlags {
    /// Convert from the `TermMode` bits used by `alacritty_terminal`.
    ///
    /// `alacritty_terminal` stores the five Kitty flags in `TermMode`
    /// bits 18–22.  This function extracts them and returns the
    /// corresponding [`KittyKeyboardFlags`].
    ///
    /// Returns `None` when **no** Kitty bits are set (equivalent to
    /// the legacy / xterm encoding path).
    pub fn from_term_mode(mode: u32) -> Option<Self> {
        let raw = ((mode >> 18) & 0x1f) as u8;
        if raw == 0 {
            None
        } else {
            Some(Self::from_bits_truncate(raw))
        }
    }
}

/// Options that affect key encoding behaviour.
///
/// These are determined by the terminal state (DEC modes) and user
/// configuration, and are passed to [`InputMapper::map`] on each call.
#[derive(Debug, Clone, Copy)]
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

    /// Kitty keyboard protocol flags.
    ///
    /// When `Some(flags)` and at least one flag is set, keys are encoded
    /// using the progressive CSI-u format.  When `None` or all flags
    /// are zero, the classic xterm encoding is used.
    pub kitty_flags: Option<KittyKeyboardFlags>,
}

impl Default for MappingOptions {
    fn default() -> Self {
        Self {
            app_cursor: false,
            macos_option_as_alt: false,
            kitty_flags: None,
        }
    }
}

impl MappingOptions {
    /// Default options: all flags off, Kitty disabled.
    pub const fn new() -> Self {
        Self {
            app_cursor: false,
            macos_option_as_alt: false,
            kitty_flags: None,
        }
    }

    /// Convenience: enable Kitty keyboard with the given flags.
    pub fn with_kitty(mut self, flags: KittyKeyboardFlags) -> Self {
        self.kitty_flags = Some(flags);
        self
    }
}

// ── InputMapper ───────────────────────────────────────────────────────

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
        // ── Kitty encoding path ─────────────────────────────────────
        // When the Kitty keyboard protocol is active, all Key events
        // (including presses, repeats, and releases) are routed through
        // the progressive CSI-u encoder.  Text events are still passed
        // through as raw UTF-8 unless REPORT_ALL_KEYS_AS_ESCAPE_CODES
        // is set (in which case they are also intercepted).
        if let Some(kitty_flags) = opts.kitty_flags {
            if kitty_flags != KittyKeyboardFlags::NONE {
                if let egui::Event::Key {
                    key,
                    physical_key: pk,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } = event
                {
                    return encode_kitty(
                        *key,
                        *pk,
                        *pressed,
                        *repeat,
                        *modifiers,
                        kitty_flags,
                        opts,
                    );
                }
                // In REPORT_ALL mode we also encode Text events as CSI u.
                if kitty_flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES) {
                    if let egui::Event::Text(text) = event {
                        return encode_kitty_text(text, kitty_flags, opts);
                    }
                }
            }
        }

        // ── Legacy (xterm) encoding path ────────────────────────────
        match event {
            egui::Event::Text(text) => {
                // Printable text — send the UTF-8 bytes directly.
                if text.is_empty() {
                    return None;
                }
                let has_printable =
                    text.chars().any(|c| !c.is_control() && c != '\n' && c != '\r');
                if !has_printable {
                    return None;
                }
                Some(text.as_bytes().to_vec())
            }

            egui::Event::Key {
                key,
                physical_key: _,
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
                    Key::F13 => Some(tilde_seq_raw(57376, mod_idx)),
                    Key::F14 => Some(tilde_seq_raw(57377, mod_idx)),
                    Key::F15 => Some(tilde_seq_raw(57378, mod_idx)),
                    Key::F16 => Some(tilde_seq_raw(57379, mod_idx)),
                    Key::F17 => Some(tilde_seq_raw(57380, mod_idx)),
                    Key::F18 => Some(tilde_seq_raw(57381, mod_idx)),
                    Key::F19 => Some(tilde_seq_raw(57382, mod_idx)),
                    Key::F20 => Some(tilde_seq_raw(57383, mod_idx)),

                    // ── Space ──────────────────────────────────────────
                    Key::Space => {
                        if ctrl {
                            Some(vec![0x00]) // Ctrl+Space → NUL
                        } else {
                            None // handled by Event::Text
                        }
                    }

                    // ── All other keys ────────────────────────────────
                    _ => None,
                }
            }

            egui::Event::Paste(text) => {
                // Bracketed paste mode is handled by the PTY layer; we
                // just forward the raw paste data here.
                if text.is_empty() {
                    return None;
                }
                Some(text.as_bytes().to_vec())
            }

            _ => None,
        }
        .or_else(|| {
            // ── Fallback: Ctrl+letter → C0 ───────────────────────────
            // This must be after the explicit match so that keys handled
            // above (Enter, Backspace, Tab, Escape, arrows, …) are NOT
            // also sent as C0 codes.
            legacy_ctrl_fallback(event, opts)
        })
    }
}

// ── Legacy fallback ───────────────────────────────────────────────────

/// Handle Ctrl+letter/digit/symbol that wasn't caught by the main match.
///
/// This is intentionally a separate step so that keys like Enter and Tab
/// produce their expected sequences rather than C0 codes.
fn legacy_ctrl_fallback(event: &egui::Event, opts: &MappingOptions) -> Option<Vec<u8>> {
    if let egui::Event::Key {
        key,
        physical_key,
        pressed: true,
        modifiers,
        ..
    } = event
    {
        let ctrl = modifiers.ctrl;
        let alt = modifiers.alt;
        let shift = modifiers.shift;

        if ctrl && !alt {
            // Ctrl+letter → C0 control code (0x01–0x1a)
            if let Some(code) = key_to_ctrl_code(key) {
                return Some(vec![code]);
            }
            // Try physical key fallback for non-Latin layouts.
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
    }

    None
}

// ── Kitty encoding ────────────────────────────────────────────────────

/// Encode a Key event using the Kitty keyboard protocol.
///
/// This is the main entry point for the progressive CSI-u encoding.
/// The behaviour is controlled by the [`KittyKeyboardFlags`] bitmask.
fn encode_kitty(
    key: Key,
    physical_key: Option<Key>,
    pressed: bool,
    repeat: bool,
    modifiers: egui::Modifiers,
    flags: KittyKeyboardFlags,
    _opts: &MappingOptions,
) -> Option<Vec<u8>> {
    let ctrl = modifiers.ctrl;
    let alt = modifiers.alt;
    let shift = modifiers.shift;
    let mods = kitty_mod_idx(ctrl, alt, shift);

    // ── Release events ────────────────────────────────────────────────
    // Without REPORT_EVENT_TYPES we ignore all non-press events.
    if !pressed {
        if !flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES) {
            return None;
        }
        // Enter, backspace, and tab do NOT report release events unless
        // REPORT_ALL_KEYS_AS_ESCAPE_CODES is also set.
        if !flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES) {
            match key {
                Key::Enter | Key::Backspace | Key::Tab => return None,
                _ => {}
            }
        }
    }

    // ── Legacy fallback for unmodified Enter/Tab/Backspace ────────────
    // Without REPORT_ALL, these keys send their simple bytes when
    // no modifiers are held.  This lets the user interact with the shell
    // normally even if the protocol was left active by a crashed program.
    if pressed && !flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES) {
        if mods == 1 {
            match key {
                Key::Enter => return Some(vec![b'\r']),
                Key::Tab => return Some(vec![b'\t']),
                Key::Backspace => return Some(vec![0x7f]),
                _ => {}
            }
        }

        // Unmodified printable characters: let Event::Text handle them.
        if mods == 1 && is_printable_key(&key) {
            return None;
        }
    }

    // ── Determine event type ──────────────────────────────────────────
    let event_type = if flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES) {
        if !pressed {
            Some(3u8) // release
        } else if repeat {
            Some(2u8) // repeat
        } else {
            None // press — default, omitted from output
        }
    } else {
        None
    };

    // ── Look up in functional key table ───────────────────────────────
    let (key_code, terminator) = if let Some(entry) = kitty_lookup(&key) {
        (entry.code, entry.terminator)
    } else {
        // Not a functional key.  Use the Unicode code point.
        let cp = logical_codepoint(&key, physical_key)?;
        (cp, Terminator::U)
    };

    // ── Alternates ────────────────────────────────────────────────────
    let alternates = if flags.contains(KittyKeyboardFlags::REPORT_ALTERNATE_KEYS) {
        ascii_alternates(key, physical_key)
            .map(|(s, u)| [Some(s), Some(u)])
            .unwrap_or([None, None])
    } else {
        [None, None]
    };

    // ── Associated text ───────────────────────────────────────────────
    let text_codepoints = if flags.contains(KittyKeyboardFlags::REPORT_ASSOCIATED_TEXT) && pressed
    {
        associated_text_codepoints(&key)
    } else {
        vec![]
    };

    // ── Build sequence ────────────────────────────────────────────────
    Some(kitty_seq(
        key_code,
        terminator,
        mods,
        event_type,
        &alternates,
        &text_codepoints,
    ))
}

/// Encode a Text event as a Kitty CSI-u sequence.
///
/// Used when `REPORT_ALL_KEYS_AS_ESCAPE_CODES` is set and the
/// application receives a text/IME commit event.
fn encode_kitty_text(
    text: &str,
    _flags: KittyKeyboardFlags,
    _opts: &MappingOptions,
) -> Option<Vec<u8>> {
    if text.is_empty() {
        return None;
    }

    // Extract the first printable codepoint.
    let cp = text.chars().find(|c| !c.is_control())?;
    let cp_u32 = cp as u32;

    // Simple encoding: \x1b[{cp};1u (press, no alternates)
    Some(csi_u_simple(cp_u32, 1))
}

/// Returns `true` if `key` is a printable character (letter, digit,
/// punctuation) that egui delivers through `Event::Text`.
fn is_printable_key(key: &Key) -> bool {
    matches!(
        key,
        Key::A | Key::B | Key::C | Key::D | Key::E | Key::F | Key::G
            | Key::H | Key::I | Key::J | Key::K | Key::L | Key::M
            | Key::N | Key::O | Key::P | Key::Q | Key::R | Key::S
            | Key::T | Key::U | Key::V | Key::W | Key::X | Key::Y | Key::Z
            | Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4
            | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9
            | Key::Space
            | Key::Minus | Key::Equals | Key::Comma | Key::Period
            | Key::Slash | Key::Backslash | Key::Semicolon | Key::Quote
            | Key::Backtick | Key::OpenBracket | Key::CloseBracket
            | Key::Colon | Key::Plus | Key::Pipe | Key::Questionmark
            | Key::Exclamationmark | Key::OpenCurlyBracket
            | Key::CloseCurlyBracket
    )
}

/// Get the logical (unshifted) Unicode code point for a key.
///
/// Used as the primary key code in Kitty CSI-u sequences.  Per the
/// protocol the primary code is **always** the unshifted/base value;
/// the Shift modifier is conveyed only via the modifier field.
fn logical_codepoint(
    key: &Key,
    physical_key: Option<Key>,
) -> Option<u32> {
    // Prefer the physical key position over the logical key when they
    // differ, because the physical key represents the unshifted
    // character on the keyboard.
    let base_key = physical_key.filter(|pk| pk != key).unwrap_or(*key);

    if let Some(byte) = key_to_ascii(&base_key) {
        // Always use the raw (lowercase for letters) value — the
        // Shift state is reflected only in the modifier field.
        return Some(byte as u32);
    }

    None
}

/// Compute the associated text code points for a key.
///
/// For simple ASCII keys this is just the character itself.
/// For composed/modified keys it would be the resulting Unicode
/// text, but egui does not provide that through `Event::Key`.
fn associated_text_codepoints(key: &Key) -> Vec<u32> {
    key_to_ascii(key)
        .map(|b| vec![b as u32])
        .unwrap_or_default()
}
