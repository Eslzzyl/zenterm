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
fn cursor_seq(letter: u8, mod_idx: u8, app_cursor: bool) -> Vec<u8> {
    if mod_idx == 1 {
        if app_cursor {
            vec![0x1b, b'O', letter] // SS3 prefix
        } else {
            vec![0x1b, b'[', letter] // CSI prefix
        }
    } else {
        format!("\x1b[1;{}{}", mod_idx, letter as char).into_bytes()
    }
}

/// Build a CSI `~` sequence for a single-digit parameter:
/// `\x1b[{digit};{mod}~` (no-mod case omits `;1`).
fn tilde_seq(digit: u8, mod_idx: u8) -> Vec<u8> {
    if mod_idx == 1 {
        vec![0x1b, b'[', digit, b'~']
    } else {
        format!("\x1b[{};{}~", digit as char, mod_idx).into_bytes()
    }
}

/// Build a CSI `~` sequence for an integer parameter (e.g. F5 → 15).
fn tilde_seq_raw(n: u16, mod_idx: u8) -> Vec<u8> {
    if mod_idx == 1 {
        format!("\x1b[{}~", n).into_bytes()
    } else {
        format!("\x1b[{};{}~", n, mod_idx).into_bytes()
    }
}

/// Build an F1–F4 sequence.
///
/// Without modifiers the legacy `SS3` form is used (`\x1bOP` etc.),
/// which is what most terminals send and what readline/tmux expect.
/// With modifiers the CSI form `\x1b[1;{mod}{letter}` is used.
fn fkey_seq(letter: u8, mod_idx: u8) -> Vec<u8> {
    if mod_idx == 1 {
        vec![0x1b, b'O', letter]
    } else {
        format!("\x1b[1;{}{}", mod_idx, letter as char).into_bytes()
    }
}

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

/// Map `Key::A`–`Key::Z` to C0 control codes `0x01`–`0x1a`.
fn key_to_ctrl_code(key: &Key) -> Option<u8> {
    match key {
        Key::A => Some(0x01),
        Key::B => Some(0x02),
        Key::C => Some(0x03),
        Key::D => Some(0x04),
        Key::E => Some(0x05),
        Key::F => Some(0x06),
        Key::G => Some(0x07),
        Key::H => Some(0x08), // Backspace
        Key::I => Some(0x09), // Tab
        Key::J => Some(0x0a), // Line feed
        Key::K => Some(0x0b),
        Key::L => Some(0x0c),
        Key::M => Some(0x0d), // Carriage return
        Key::N => Some(0x0e),
        Key::O => Some(0x0f),
        Key::P => Some(0x10),
        Key::Q => Some(0x11),
        Key::R => Some(0x12),
        Key::S => Some(0x13),
        Key::T => Some(0x14),
        Key::U => Some(0x15),
        Key::V => Some(0x16),
        Key::W => Some(0x17),
        Key::X => Some(0x18),
        Key::Y => Some(0x19),
        Key::Z => Some(0x1a),
        _ => None,
    }
}

/// Extended Ctrl+key mapping for digits and symbols that don't fit the
/// simple A–Z pattern.
///
/// Based on xterm / Ghostty behaviour:
///
/// | Key   | C0   | Notes                    |
/// |-------|------|--------------------------|
/// | Space | 0x00 | NUL                      |
/// | 2     | 0x00 | NUL (also @ with shift)  |
/// | 3     | 0x1b | ESC                      |
/// | 4     | 0x1c | FS                       |
/// | 5     | 0x1d | GS                       |
/// | 6     | 0x1e | RS                       |
/// | 7     | 0x1f | US                       |
/// | 8     | 0x7f | DEL                      |
/// | 0,1,9 | 0x30…| passthrough              |
/// | `/`   | 0x1f | US                       |
/// | `?`   | 0x7f | DEL                      |
/// | `\`   | 0x1c | FS                       |
/// | `|`   | 0x1c | FS (shifted `\`)         |
/// | `[`   | 0x1b | ESC                      |
/// | `{`   | 0x1b | ESC (shifted `[`)        |
/// | `]`   | 0x1d | GS                       |
/// | `}`   | 0x1d | GS (shifted `]`)         |
fn key_to_ctrl_extended(key: &Key) -> Option<u8> {
    match key {
        Key::Space => Some(0x00),
        Key::Num2 => Some(0x00),  // NUL
        Key::Num3 => Some(0x1b),  // ESC
        Key::Num4 => Some(0x1c),  // FS
        Key::Num5 => Some(0x1d),  // GS
        Key::Num6 => Some(0x1e),  // RS
        Key::Num7 => Some(0x1f),  // US
        Key::Num8 => Some(0x7f),  // DEL
        Key::Num9 => Some(0x39),  // '9' (passthrough)
        Key::Num0 => Some(0x30),  // '0' (passthrough)
        Key::Num1 => Some(0x31),  // '1' (passthrough)

        Key::Slash => Some(0x1f),          // Ctrl+/ → US
        Key::Questionmark => Some(0x7f),   // Ctrl+? → DEL
        Key::Backslash => Some(0x1c),      // Ctrl+\ → FS
        Key::Pipe => Some(0x1c),           // Ctrl+| → FS
        Key::CloseBracket => Some(0x1d),   // Ctrl+] → GS
        Key::CloseCurlyBracket => Some(0x1d), // Ctrl+} → GS
        Key::OpenBracket => Some(0x1b),    // Ctrl+[ → ESC
        Key::OpenCurlyBracket => Some(0x1b), // Ctrl+{ → ESC
        Key::Backtick => None,
        Key::Minus => None,
        Key::Equals => None,
        Key::Semicolon => None,
        Key::Quote => None,
        Key::Comma => None,
        Key::Period => None,
        Key::Colon => None,
        Key::Plus => None,
        Key::Exclamationmark => None,
        _ => None,
    }
}

/// Map a `Key` to its corresponding ASCII byte (lowercase for letters).
///
/// Used to produce the character emitted by `Alt+key`.
fn key_to_ascii(key: &Key) -> Option<u8> {
    match key {
        // Letters
        Key::A => Some(b'a'),
        Key::B => Some(b'b'),
        Key::C => Some(b'c'),
        Key::D => Some(b'd'),
        Key::E => Some(b'e'),
        Key::F => Some(b'f'),
        Key::G => Some(b'g'),
        Key::H => Some(b'h'),
        Key::I => Some(b'i'),
        Key::J => Some(b'j'),
        Key::K => Some(b'k'),
        Key::L => Some(b'l'),
        Key::M => Some(b'm'),
        Key::N => Some(b'n'),
        Key::O => Some(b'o'),
        Key::P => Some(b'p'),
        Key::Q => Some(b'q'),
        Key::R => Some(b'r'),
        Key::S => Some(b's'),
        Key::T => Some(b't'),
        Key::U => Some(b'u'),
        Key::V => Some(b'v'),
        Key::W => Some(b'w'),
        Key::X => Some(b'x'),
        Key::Y => Some(b'y'),
        Key::Z => Some(b'z'),

        // Digits
        Key::Num0 => Some(b'0'),
        Key::Num1 => Some(b'1'),
        Key::Num2 => Some(b'2'),
        Key::Num3 => Some(b'3'),
        Key::Num4 => Some(b'4'),
        Key::Num5 => Some(b'5'),
        Key::Num6 => Some(b'6'),
        Key::Num7 => Some(b'7'),
        Key::Num8 => Some(b'8'),
        Key::Num9 => Some(b'9'),

        // Punctuation / symbols (unshifted)
        Key::Space => Some(b' '),
        Key::Minus => Some(b'-'),
        Key::Equals => Some(b'='),
        Key::Comma => Some(b','),
        Key::Period => Some(b'.'),
        Key::Slash => Some(b'/'),
        Key::Backslash => Some(b'\\'),
        Key::Semicolon => Some(b';'),
        Key::Quote => Some(b'\''),
        Key::Backtick => Some(b'`'),
        Key::OpenBracket => Some(b'['),
        Key::CloseBracket => Some(b']'),
        Key::Colon => Some(b':'),
        Key::Plus => Some(b'+'),
        Key::Pipe => Some(b'|'),
        Key::Questionmark => Some(b'?'),
        Key::Exclamationmark => Some(b'!'),
        Key::OpenCurlyBracket => Some(b'{'),
        Key::CloseCurlyBracket => Some(b'}'),

        _ => None,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

    // ── Helper shortcuts ───────────────────────────────────────────

    /// Default mapping options for tests (app_cursor off, option_as_alt off).
    fn default_opts() -> MappingOptions {
        MappingOptions::new()
    }

    /// Helper to create a Key event.
    fn key_event(key: Key, ctrl: bool, alt: bool, shift: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                ctrl,
                alt,
                shift,
                mac_cmd: false,
                command: false,
            },
        }
    }

    /// Like [`key_event`] but with a specific `physical_key`.
    ///
    /// Used to test non-Latin layout fallback where the logical key
    /// differs from the physical key position.
    fn key_event_with_physical(
        key: Key,
        physical_key: Key,
        ctrl: bool,
        alt: bool,
        shift: bool,
    ) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(physical_key),
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                ctrl,
                alt,
                shift,
                mac_cmd: false,
                command: false,
            },
        }
    }

    fn text_event(text: &str) -> egui::Event {
        egui::Event::Text(text.to_owned())
    }

    fn paste_event(text: &str) -> egui::Event {
        egui::Event::Paste(text.to_owned())
    }

    fn assert_map(event: &egui::Event, expected: &[u8]) {
        let result = InputMapper::map(event, &default_opts());
        assert_eq!(result.as_deref(), Some(expected), "event={event:?}");
    }

    fn assert_map_none(event: &egui::Event) {
        let result = InputMapper::map(event, &default_opts());
        assert_eq!(result, None, "event={event:?}");
    }

    fn assert_map_with(
        event: &egui::Event,
        expected: &[u8],
        opts: &MappingOptions,
    ) {
        let result = InputMapper::map(event, opts);
        assert_eq!(result.as_deref(), Some(expected), "event={event:?}");
    }

    // ── Plain navigation keys ──────────────────────────────────────

    #[test]
    fn test_enter() {
        assert_map(&key_event(Key::Enter, false, false, false), b"\r");
    }

    #[test]
    fn test_tab() {
        assert_map(&key_event(Key::Tab, false, false, false), b"\t");
    }

    #[test]
    fn test_shift_tab() {
        assert_map(&key_event(Key::Tab, false, false, true), b"\x1bZ");
    }

    #[test]
    fn test_backspace() {
        assert_map(&key_event(Key::Backspace, false, false, false), b"\x7f");
    }

    #[test]
    fn test_escape() {
        assert_map(&key_event(Key::Escape, false, false, false), b"\x1b");
    }

    // ── Modifier + Enter / Backspace / Tab ──────────────────────────

    #[test]
    fn test_ctrl_enter() {
        assert_map(&key_event(Key::Enter, true, false, false), b"\x1b[13;5~");
    }

    #[test]
    fn test_alt_enter() {
        assert_map(&key_event(Key::Enter, false, true, false), b"\x1b\r");
    }

    #[test]
    fn test_ctrl_backspace() {
        assert_map(&key_event(Key::Backspace, true, false, false), b"\x08");
    }

    #[test]
    fn test_alt_backspace() {
        assert_map(&key_event(Key::Backspace, false, true, false), b"\x1b\x7f");
    }

    #[test]
    fn test_ctrl_tab() {
        assert_map(&key_event(Key::Tab, true, false, false), b"\x1b[9;5~");
    }

    #[test]
    fn test_alt_escape() {
        assert_map(&key_event(Key::Escape, false, true, false), b"\x1b\x1b");
    }

    // ── Arrow keys (plain) ──────────────────────────────────────────

    #[test]
    fn test_arrow_plain() {
        assert_map(&key_event(Key::ArrowUp, false, false, false), b"\x1b[A");
        assert_map(&key_event(Key::ArrowDown, false, false, false), b"\x1b[B");
        assert_map(&key_event(Key::ArrowRight, false, false, false), b"\x1b[C");
        assert_map(&key_event(Key::ArrowLeft, false, false, false), b"\x1b[D");
    }

    // ── Application mode cursor keys (DEC mode 1) ───────────────────

    #[test]
    fn test_arrow_app_cursor() {
        let opts = MappingOptions {
            app_cursor: true,
            ..MappingOptions::new()
        };
        // Without modifiers: SS3 form
        assert_map_with(
            &key_event(Key::ArrowUp, false, false, false),
            b"\x1bOA",
            &opts,
        );
        assert_map_with(
            &key_event(Key::ArrowDown, false, false, false),
            b"\x1bOB",
            &opts,
        );
        assert_map_with(
            &key_event(Key::ArrowRight, false, false, false),
            b"\x1bOC",
            &opts,
        );
        assert_map_with(
            &key_event(Key::ArrowLeft, false, false, false),
            b"\x1bOD",
            &opts,
        );
        // Home/End: SS3 form
        assert_map_with(&key_event(Key::Home, false, false, false), b"\x1bOH", &opts);
        assert_map_with(&key_event(Key::End, false, false, false), b"\x1bOF", &opts);
    }

    #[test]
    fn test_arrow_app_cursor_with_modifiers() {
        // With modifiers: CSI form regardless of app_cursor
        let opts = MappingOptions {
            app_cursor: true,
            ..MappingOptions::new()
        };
        assert_map_with(
            &key_event(Key::ArrowUp, true, false, false),
            b"\x1b[1;5A",
            &opts,
        );
        assert_map_with(
            &key_event(Key::ArrowRight, false, true, false),
            b"\x1b[1;3C",
            &opts,
        );
        assert_map_with(
            &key_event(Key::Home, false, false, true),
            b"\x1b[1;2H",
            &opts,
        );
    }

    // ── Arrow keys + modifiers (CSI 1 ; {mod} {letter}) ────────────

    #[test]
    fn test_arrow_ctrl() {
        // Ctrl+ArrowUp = \x1b[1;5A (word jumping in shell)
        assert_map(&key_event(Key::ArrowUp, true, false, false), b"\x1b[1;5A");
        assert_map(&key_event(Key::ArrowDown, true, false, false), b"\x1b[1;5B");
        assert_map(&key_event(Key::ArrowRight, true, false, false), b"\x1b[1;5C");
        assert_map(&key_event(Key::ArrowLeft, true, false, false), b"\x1b[1;5D");
    }

    #[test]
    fn test_arrow_shift() {
        assert_map(&key_event(Key::ArrowUp, false, false, true), b"\x1b[1;2A");
        assert_map(&key_event(Key::ArrowDown, false, false, true), b"\x1b[1;2B");
        assert_map(&key_event(Key::ArrowRight, false, false, true), b"\x1b[1;2C");
        assert_map(&key_event(Key::ArrowLeft, false, false, true), b"\x1b[1;2D");
    }

    #[test]
    fn test_arrow_alt() {
        assert_map(&key_event(Key::ArrowUp, false, true, false), b"\x1b[1;3A");
        assert_map(&key_event(Key::ArrowDown, false, true, false), b"\x1b[1;3B");
        assert_map(&key_event(Key::ArrowRight, false, true, false), b"\x1b[1;3C");
        assert_map(&key_event(Key::ArrowLeft, false, true, false), b"\x1b[1;3D");
    }

    #[test]
    fn test_arrow_ctrl_shift() {
        assert_map(
            &key_event(Key::ArrowUp, true, false, true),
            b"\x1b[1;6A",
        );
        assert_map(
            &key_event(Key::ArrowRight, true, false, true),
            b"\x1b[1;6C",
        );
    }

    #[test]
    fn test_arrow_ctrl_alt_shift() {
        assert_map(
            &key_event(Key::ArrowUp, true, true, true),
            b"\x1b[1;8A",
        );
    }

    // ── Home / End ──────────────────────────────────────────────────

    #[test]
    fn test_home_end_plain() {
        assert_map(&key_event(Key::Home, false, false, false), b"\x1b[H");
        assert_map(&key_event(Key::End, false, false, false), b"\x1b[F");
    }

    #[test]
    fn test_home_end_ctrl() {
        assert_map(&key_event(Key::Home, true, false, false), b"\x1b[1;5H");
        assert_map(&key_event(Key::End, true, false, false), b"\x1b[1;5F");
    }

    #[test]
    fn test_home_end_shift() {
        assert_map(&key_event(Key::Home, false, false, true), b"\x1b[1;2H");
        assert_map(&key_event(Key::End, false, false, true), b"\x1b[1;2F");
    }

    #[test]
    fn test_home_end_alt() {
        assert_map(&key_event(Key::Home, false, true, false), b"\x1b[1;3H");
        assert_map(&key_event(Key::End, false, true, false), b"\x1b[1;3F");
    }

    // ── Page Up / Down ──────────────────────────────────────────────

    #[test]
    fn test_page_plain() {
        assert_map(&key_event(Key::PageUp, false, false, false), b"\x1b[5~");
        assert_map(&key_event(Key::PageDown, false, false, false), b"\x1b[6~");
    }

    #[test]
    fn test_page_ctrl() {
        assert_map(&key_event(Key::PageUp, true, false, false), b"\x1b[5;5~");
        assert_map(&key_event(Key::PageDown, true, false, false), b"\x1b[6;5~");
    }

    // ── Insert / Delete ─────────────────────────────────────────────

    #[test]
    fn test_insert_plain() {
        assert_map(&key_event(Key::Insert, false, false, false), b"\x1b[2~");
    }

    #[test]
    fn test_delete_plain() {
        assert_map(&key_event(Key::Delete, false, false, false), b"\x1b[3~");
    }

    #[test]
    fn test_delete_ctrl() {
        assert_map(&key_event(Key::Delete, true, false, false), b"\x1b[3;5~");
    }

    #[test]
    fn test_delete_alt() {
        assert_map(&key_event(Key::Delete, false, true, false), b"\x1b[3;3~");
    }

    // ── Ctrl+letter → C0 codes ──────────────────────────────────────

    #[test]
    fn test_ctrl_letter() {
        assert_map(&key_event(Key::A, true, false, false), b"\x01");
        assert_map(&key_event(Key::B, true, false, false), b"\x02");
        assert_map(&key_event(Key::C, true, false, false), b"\x03");
        assert_map(&key_event(Key::D, true, false, false), b"\x04");
        assert_map(&key_event(Key::E, true, false, false), b"\x05");
        assert_map(&key_event(Key::F, true, false, false), b"\x06");
        assert_map(&key_event(Key::G, true, false, false), b"\x07");
        assert_map(&key_event(Key::H, true, false, false), b"\x08");
        assert_map(&key_event(Key::I, true, false, false), b"\x09");
        assert_map(&key_event(Key::J, true, false, false), b"\x0a");
        assert_map(&key_event(Key::K, true, false, false), b"\x0b");
        assert_map(&key_event(Key::L, true, false, false), b"\x0c");
        assert_map(&key_event(Key::M, true, false, false), b"\x0d");
        assert_map(&key_event(Key::N, true, false, false), b"\x0e");
        assert_map(&key_event(Key::O, true, false, false), b"\x0f");
        assert_map(&key_event(Key::P, true, false, false), b"\x10");
        assert_map(&key_event(Key::Q, true, false, false), b"\x11");
        assert_map(&key_event(Key::R, true, false, false), b"\x12");
        assert_map(&key_event(Key::S, true, false, false), b"\x13");
        assert_map(&key_event(Key::T, true, false, false), b"\x14");
        assert_map(&key_event(Key::U, true, false, false), b"\x15");
        assert_map(&key_event(Key::V, true, false, false), b"\x16");
        assert_map(&key_event(Key::W, true, false, false), b"\x17");
        assert_map(&key_event(Key::X, true, false, false), b"\x18");
        assert_map(&key_event(Key::Y, true, false, false), b"\x19");
        assert_map(&key_event(Key::Z, true, false, false), b"\x1a");
    }

    #[test]
    fn test_ctrl_physical_key_fallback() {
        // Simulate a Cyrillic keyboard: logical key is "Unidentified"
        // or a non-Latin key, but physical key is Key::V.
        // Ctrl+physical V should send 0x16 even if the logical key
        // doesn't match our A–Z table.
        let event = key_event_with_physical(Key::Backslash, Key::V, true, false, false);
        assert_map(&event, b"\x16");

        // Physical key that ALSO falls through extended table.
        // Use a logical key that maps to nothing (Key::Minus is in
        // the "None" group in key_to_ctrl_extended).
        let event = key_event_with_physical(Key::Minus, Key::Num2, true, false, false);
        assert_map(&event, b"\x00"); // physical Num2 → NUL
    }

    #[test]
    fn test_ctrl_physical_key_ignored_when_logical_works() {
        // When the logical key already matches, physical_key should
        // NOT override it (no infinite regress — key V sends 0x16
        // regardless of what physical_key says).
        let event = key_event_with_physical(Key::V, Key::Backslash, true, false, false);
        assert_map(&event, b"\x16");
    }

    // ── Ctrl+digit / Ctrl+symbol → extended C0 codes ───────────────

    #[test]
    fn test_ctrl_space() {
        assert_map(&key_event(Key::Space, true, false, false), b"\x00");
    }

    #[test]
    fn test_ctrl_digits() {
        assert_map(&key_event(Key::Num0, true, false, false), b"0");
        assert_map(&key_event(Key::Num1, true, false, false), b"1");
        // Num2 = NUL (same as Ctrl+@)
        assert_map(&key_event(Key::Num2, true, false, false), b"\x00");
        // Num3 = ESC
        assert_map(&key_event(Key::Num3, true, false, false), b"\x1b");
        assert_map(&key_event(Key::Num4, true, false, false), b"\x1c");
        assert_map(&key_event(Key::Num5, true, false, false), b"\x1d");
        assert_map(&key_event(Key::Num6, true, false, false), b"\x1e");
        assert_map(&key_event(Key::Num7, true, false, false), b"\x1f");
        assert_map(&key_event(Key::Num8, true, false, false), b"\x7f");
        assert_map(&key_event(Key::Num9, true, false, false), b"9");
    }

    #[test]
    fn test_ctrl_slash() {
        // Ctrl+/ = US (0x1F)
        assert_map(&key_event(Key::Slash, true, false, false), b"\x1f");
    }

    #[test]
    fn test_ctrl_question() {
        // Ctrl+? = DEL (0x7F)
        assert_map(&key_event(Key::Questionmark, true, false, false), b"\x7f");
    }

    #[test]
    fn test_ctrl_backslash() {
        // Ctrl+\ = FS (0x1C)
        assert_map(&key_event(Key::Backslash, true, false, false), b"\x1c");
    }

    #[test]
    fn test_ctrl_pipe() {
        // Ctrl+| = FS (0x1C)
        assert_map(&key_event(Key::Pipe, true, false, false), b"\x1c");
    }

    #[test]
    fn test_ctrl_brackets() {
        // Ctrl+[ = ESC
        assert_map(&key_event(Key::OpenBracket, true, false, false), b"\x1b");
        assert_map(&key_event(Key::OpenCurlyBracket, true, false, false), b"\x1b");
        // Ctrl+] = GS
        assert_map(&key_event(Key::CloseBracket, true, false, false), b"\x1d");
        assert_map(&key_event(Key::CloseCurlyBracket, true, false, false), b"\x1d");
    }

    // ── Alt+letter → ESC + letter ───────────────────────────────────

    #[test]
    fn test_alt_letter_lowercase() {
        // Alt+A → \x1ba
        assert_map(&key_event(Key::A, false, true, false), b"\x1ba");
        assert_map(&key_event(Key::Z, false, true, false), b"\x1bz");
    }

    #[test]
    fn test_alt_letter_with_macos_option_as_alt_false() {
        // On macOS with macos_option_as_alt=false, Alt+letter should NOT
        // be encoded (the Text event will carry the composed Unicode).
        let opts = MappingOptions {
            macos_option_as_alt: false,
            ..MappingOptions::new()
        };
        // cfg!(target_os = "macos") is false on non-macOS → handle_alt=true
        // regardless of opts.macos_option_as_alt.  So on non-macOS the
        // mapping still fires.  This test verifies the flag is accepted
        // (on macOS it would suppress Alt; on other platforms it's a no-op).
        let expected_on_this_platform = if cfg!(target_os = "macos") {
            None
        } else {
            Some(vec![0x1b, b'a'])
        };
        let result = InputMapper::map(&key_event(Key::A, false, true, false), &opts);
        assert_eq!(result, expected_on_this_platform, "event platform mismatch");
    }

    #[test]
    fn test_alt_letter_with_macos_option_as_alt_true() {
        // On macOS with macos_option_as_alt=true, Alt+letter IS encoded.
        let opts = MappingOptions {
            macos_option_as_alt: true,
            ..MappingOptions::new()
        };
        assert_map_with(&key_event(Key::A, false, true, false), b"\x1ba", &opts);
    }

    #[test]
    fn test_alt_letter_uppercase() {
        // Alt+Shift+A → \x1bA
        assert_map(&key_event(Key::A, false, true, true), b"\x1bA");
        assert_map(&key_event(Key::Z, false, true, true), b"\x1bZ");
    }

    #[test]
    fn test_alt_digit() {
        // Alt+1 → \x1b1
        assert_map(&key_event(Key::Num0, false, true, false), b"\x1b0");
        assert_map(&key_event(Key::Num9, false, true, false), b"\x1b9");
    }

    #[test]
    fn test_alt_symbol() {
        // Alt+/ → \x1b/
        assert_map(&key_event(Key::Slash, false, true, false), b"\x1b/");
        // Alt+Space → \x1b (space)
        assert_map(&key_event(Key::Space, false, true, false), b"\x1b ");
    }

    #[test]
    fn test_alt_shift_symbol() {
        // Alt+Shift+/ = Alt+? → \x1b?
        assert_map(
            &key_event(Key::Questionmark, false, true, true),
            b"\x1b?",
        );
    }

    // ── Ctrl + letter (with other modifiers) ────────────────────────

    #[test]
    fn test_ctrl_shift_letter() {
        // Ctrl+Shift+A should still send the standard C0 code 0x01
        assert_map(&key_event(Key::A, true, false, true), b"\x01");
    }

    #[test]
    fn test_ctrl_alt_letter() {
        // Ctrl+Alt+A should not match Ctrl-only branch; falls through
        // to Alt+letter → \x1ba.  But Ctrl is pressed, so the
        // `ctrl && !alt` guard prevents the Ctrl path.
        assert_map_none(&key_event(Key::A, true, true, false));
    }

    // ── Function keys ───────────────────────────────────────────────

    #[test]
    fn test_f_keys_plain() {
        assert_map(&key_event(Key::F1, false, false, false), b"\x1bOP");
        assert_map(&key_event(Key::F2, false, false, false), b"\x1bOQ");
        assert_map(&key_event(Key::F3, false, false, false), b"\x1bOR");
        assert_map(&key_event(Key::F4, false, false, false), b"\x1bOS");
        assert_map(&key_event(Key::F5, false, false, false), b"\x1b[15~");
        assert_map(&key_event(Key::F6, false, false, false), b"\x1b[17~");
        assert_map(&key_event(Key::F7, false, false, false), b"\x1b[18~");
        assert_map(&key_event(Key::F8, false, false, false), b"\x1b[19~");
        assert_map(&key_event(Key::F9, false, false, false), b"\x1b[20~");
        assert_map(&key_event(Key::F10, false, false, false), b"\x1b[21~");
        assert_map(&key_event(Key::F11, false, false, false), b"\x1b[23~");
        assert_map(&key_event(Key::F12, false, false, false), b"\x1b[24~");
    }

    #[test]
    fn test_f_keys_ctrl() {
        assert_map(&key_event(Key::F1, true, false, false), b"\x1b[1;5P");
        assert_map(&key_event(Key::F5, true, false, false), b"\x1b[15;5~");
        assert_map(&key_event(Key::F12, true, false, false), b"\x1b[24;5~");
    }

    #[test]
    fn test_f_keys_shift() {
        assert_map(&key_event(Key::F1, false, false, true), b"\x1b[1;2P");
        assert_map(&key_event(Key::F12, false, false, true), b"\x1b[24;2~");
    }

    // ── Text events ─────────────────────────────────────────────────

    #[test]
    fn test_text_printable() {
        assert_map(&text_event("hello"), b"hello");
    }

    #[test]
    fn test_text_empty() {
        assert_map_none(&text_event(""));
    }

    #[test]
    fn test_text_control_only() {
        // Control characters only → ignore
        assert_map_none(&text_event("\x00\x01\x02"));
    }

    // ── Paste events ────────────────────────────────────────────────

    #[test]
    fn test_paste() {
        assert_map(&paste_event("pasted content"), b"pasted content");
    }

    #[test]
    fn test_paste_empty() {
        assert_map_none(&paste_event(""));
    }

    // ── Copy / Cut events ──────────────────────────────────────────

    #[test]
    fn test_copy_cut_not_forwarded() {
        assert_map_none(&egui::Event::Copy);
        assert_map_none(&egui::Event::Cut);
    }

    // ── IME events (unchanged) ──────────────────────────────────────

    #[test]
    fn test_ime_commit() {
        let event = egui::Event::Ime(egui::ImeEvent::Commit("hello".to_string()));
        assert_map(&event, b"hello");
    }

    #[test]
    fn test_ime_commit_empty() {
        let event = egui::Event::Ime(egui::ImeEvent::Commit(String::new()));
        assert_map_none(&event);
    }

    #[test]
    fn test_ime_preedit_ignored() {
        let event = egui::Event::Ime(egui::ImeEvent::Preedit("ni".to_string()));
        assert_map_none(&event);
    }

    #[test]
    fn test_ime_enabled_ignored() {
        let event = egui::Event::Ime(egui::ImeEvent::Enabled);
        assert_map_none(&event);
    }

    #[test]
    fn test_ime_disabled_ignored() {
        let event = egui::Event::Ime(egui::ImeEvent::Disabled);
        assert_map_none(&event);
    }

    // ── Miscellaneous: events that should NOT produce output ────────

    #[test]
    fn test_key_release_ignored() {
        let event = egui::Event::Key {
            key: Key::A,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        assert_map_none(&event);
    }

    #[test]
    fn test_unmapped_keys() {
        // Keys without Ctrl/Alt should return None in the fallback branch.
        // They will arrive as Text events instead.
        assert_map_none(&key_event(Key::A, false, false, false));
        assert_map_none(&key_event(Key::Num0, false, false, false));
        assert_map_none(&key_event(Key::Space, false, false, false));
        assert_map_none(&key_event(Key::Slash, false, false, false));
        assert_map_none(&key_event(Key::Backslash, false, false, false));
    }

    #[test]
    fn test_f13_not_handled() {
        // F13+ are not in our match — they should fall through to None.
        assert_map_none(&key_event(Key::F13, false, false, false));
        assert_map_none(&key_event(Key::F35, false, false, false));
    }
}
