use egui::Key;

pub(super) fn key_to_ctrl_code(key: &Key) -> Option<u8> {
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
pub(super) fn key_to_ctrl_extended(key: &Key) -> Option<u8> {
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
pub(super) fn key_to_ascii(key: &Key) -> Option<u8> {
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
