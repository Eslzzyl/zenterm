//! Sequence builders for terminal input encoding.
//!
//! Provides both legacy (xterm) and Kitty keyboard protocol helpers.

use std::fmt::Write;

use egui::Key;

use super::kitty::Terminator;

// ── Legacy (xterm) helpers ────────────────────────────────────────────

/// Build a CSI cursor-motion sequence: `\x1b[{param};{mod}{letter}`.
///
/// When `app_cursor` is true and modifiers are absent an SS3 prefix is
/// used instead (`\x1bO{letter}`).
pub(super) fn cursor_seq(letter: u8, mod_idx: u8, app_cursor: bool) -> Vec<u8> {
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
pub(super) fn tilde_seq(digit: u8, mod_idx: u8) -> Vec<u8> {
    if mod_idx == 1 {
        vec![0x1b, b'[', digit, b'~']
    } else {
        format!("\x1b[{};{}~", digit as char, mod_idx).into_bytes()
    }
}

/// Build a CSI `~` sequence for an integer parameter (e.g. F5 → 15).
pub(super) fn tilde_seq_raw(n: u16, mod_idx: u8) -> Vec<u8> {
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
pub(super) fn fkey_seq(letter: u8, mod_idx: u8) -> Vec<u8> {
    if mod_idx == 1 {
        vec![0x1b, b'O', letter]
    } else {
        format!("\x1b[1;{}{}", mod_idx, letter as char).into_bytes()
    }
}

// ── Kitty / CSI-u helpers ─────────────────────────────────────────────

/// Simple CSI-u ("fixterm") encoding: `\x1b[{code};{mods}u`.
///
/// This is the fallback when only `DISAMBIGUATE_ESCAPE_CODES` is set
/// without the other Kitty protocol flags.  It disambiguates modified
/// keys without the full protocol overhead.
///
/// When `mods` is 1 (no modifiers) the `;1` is omitted.
pub(super) fn csi_u_simple(key_code: u32, mods: u8) -> Vec<u8> {
    if mods == 1 {
        format!("\x1b[{}u", key_code).into_bytes()
    } else {
        format!("\x1b[{};{}u", key_code, mods).into_bytes()
    }
}

/// Full Kitty protocol sequence.
///
/// Format:
/// ```text
/// CSI key-code[:alt1[:alt2]] [; mods[:event-type]] [; text-as-codepoints] terminator
/// ```
///
/// * `key_code` — primary numeric key identifier.
/// * `terminator` — how the sequence ends (see [`Terminator`]).
/// * `mods` — modifier bitmask + 1 (1 = none).
/// * `event_type` — `None`=press(omitted), `Some(1)`=press, `Some(2)`=repeat,
///   `Some(3)`=release.
/// * `alternates` — `[shifted, base_or_unshifted]` alternate code points
///   (only used when `report_alternates` is active).
/// * `text_codepoints` — Unicode code points for associated text
///   (only used when `report_associated_text` is active).
pub(super) fn kitty_seq(
    key_code: u32,
    terminator: Terminator,
    mods: u8,
    event_type: Option<u8>,
    alternates: &[Option<u32>; 2],
    text_codepoints: &[u32],
) -> Vec<u8> {
    match terminator {
        // 'u' and '~' use the full format with alternates, event, and text.
        Terminator::U | Terminator::Tilde => {
            kitty_seq_full(key_code, terminator, mods, event_type, alternates, text_codepoints)
        }
        // Letters (A/B/C/D/H/F/P/Q/S) use the special (compact) format.
        Terminator::Letter(final_byte) => {
            kitty_seq_special(key_code, final_byte, mods, event_type)
        }
    }
}

/// Full-format sequence (`~` or `u` terminator).
fn kitty_seq_full(
    key_code: u32,
    terminator: Terminator,
    mods: u8,
    event_type: Option<u8>,
    alternates: &[Option<u32>; 2],
    text_codepoints: &[u32],
) -> Vec<u8> {
    let final_byte = match terminator {
        Terminator::U => b'u',
        Terminator::Tilde => b'~',
        _ => b'u', // unreachable but safe
    };

    let mut buf = String::new();

    // ── Key section ─────────────────────────────────────────────────
    write!(buf, "\x1b[{}", key_code).unwrap();

    // Alternates: :shifted or :shifted:base or ::base
    if let Some(shifted) = alternates[0] {
        write!(buf, ":{}", shifted).unwrap();
        if let Some(base) = alternates[1] {
            write!(buf, ":{}", base).unwrap();
        }
    } else if let Some(base) = alternates[1] {
        write!(buf, "::{}", base).unwrap();
    }

    // ── Modifier & event section ────────────────────────────────────
    let has_event = event_type.is_some_and(|e| e != 1); // 1 = press, omit
    if has_event {
        write!(buf, ";{}:{}", mods, event_type.unwrap()).unwrap();
    } else if mods > 1 {
        write!(buf, ";{}", mods).unwrap();
    }

    // ── Text-as-codepoints section ──────────────────────────────────
    if !text_codepoints.is_empty() {
        if mods <= 1 && !has_event {
            // Need to emit a leading ";1" (mods=1 is default) so the
            // parser doesn't mistake the text for the modifier.
            write!(buf, ";1").unwrap();
        }
        for &cp in text_codepoints {
            // Skip control characters.
            if cp < 0x20 || cp == 0x7f {
                continue;
            }
            write!(buf, ";{}", cp).unwrap();
        }
    }

    // ── Terminator ──────────────────────────────────────────────────
    buf.push(final_byte as char);
    buf.into_bytes()
}

/// Special (compact) format for cursor-navigation and F1–F4 keys.
///
/// Format: `\x1b[{key_code};{mods}{final_byte}`
///
/// Unlike the full format, alternates and associated text are not supported.
fn kitty_seq_special(
    key_code: u32,
    final_byte: u8,
    mods: u8,
    event_type: Option<u8>,
) -> Vec<u8> {
    // Special keys always have the form: \x1b[{key};{mods}{final}
    // Event type is appended as :event if present.
    let mut buf = String::new();
    write!(buf, "\x1b[{}", key_code).unwrap();

    if let Some(ev) = event_type {
        write!(buf, ";{}:{}", mods, ev).unwrap();
    } else {
        write!(buf, ";{}", mods).unwrap();
    }

    buf.push(final_byte as char);
    buf.into_bytes()
}

/// Compute the Kitty modifier bitmask + 1 from egui modifier flags.
///
/// Kitty modifier bit assignments:
///
/// | Bit | Modifier |
/// |-----|----------|
/// | 0   | Shift   |
/// | 1   | Alt     |
/// | 2   | Ctrl    |
/// | 3   | Super (Win/Cmd) — **not reliably exposed by egui, omitted** |
///
/// The returned value is `bitmask + 1` so that no modifiers = 1.
/// This matches the convention used in CSI-u and Kitty sequences.
pub(super) fn kitty_mod_idx(ctrl: bool, alt: bool, shift: bool) -> u8 {
    let mut mask = 0u8;
    if shift {
        mask |= 1;
    }
    if alt {
        mask |= 2;
    }
    if ctrl {
        mask |= 4;
    }
    // Super/Hyper/Meta/CapsLock/NumLock are not reliably exposed by
    // egui and are therefore not included.
    mask + 1
}

/// xterm-style modifier index (used by the legacy encoders).
pub(super) fn modifier_index(ctrl: bool, alt: bool, shift: bool) -> u8 {
    kitty_mod_idx(ctrl, alt, shift)
}

/// Determine the shifted and unshifted ASCII code points for a key pair.
///
/// Returns `(shifted, unshifted)` when both can be determined and they
/// differ, `None` otherwise.
pub(super) fn ascii_alternates(key: Key, physical_key: Option<Key>) -> Option<(u32, u32)> {
    use super::keys::key_to_ascii;

    // The "base" (unshifted) character always comes from the physical key
    // position if available, because it represents the keycap regardless
    // of the current keyboard layout.  When no physical key information
    // is available, the logical key value is used.
    let pk = physical_key.filter(|pk| pk != &key).unwrap_or(key);
    let unshifted = key_to_ascii(&pk)?;

    // The "shifted" character comes from the logical key (which already
    // reflects the effect of Shift), or is derived by uppercasing the
    // base character / applying the symbol shift map.
    let shifted = if pk != key {
        // Physical and logical differ: the logical key's ASCII value is
        // the shifted character.  Uppercase it because the logical key
        // may be lowercase even when Shift is pressed (egui normalises
        // letters to lowercase in the Key enum).
        key_to_ascii(&key).map(|b| b.to_ascii_uppercase()).unwrap_or(unshifted)
    } else {
        // No physical/logical split — derive shifted from unshifted.
        if unshifted.is_ascii_lowercase() {
            unshifted.to_ascii_uppercase()
        } else {
            shifted_symbol(unshifted).unwrap_or(unshifted)
        }
    };

    // Only return alternates if they actually differ.
    if shifted == unshifted {
        return None;
    }

    Some((shifted as u32, unshifted as u32))
}

/// Map an unshifted ASCII symbol byte to its shifted counterpart.
fn shifted_symbol(b: u8) -> Option<u8> {
    Some(match b {
        b'1' => b'!',
        b'2' => b'@',
        b'3' => b'#',
        b'4' => b'$',
        b'5' => b'%',
        b'6' => b'^',
        b'7' => b'&',
        b'8' => b'*',
        b'9' => b'(',
        b'0' => b')',
        b'-' => b'_',
        b'=' => b'+',
        b'[' => b'{',
        b']' => b'}',
        b'\\' => b'|',
        b';' => b':',
        b'\'' => b'"',
        b',' => b'<',
        b'.' => b'>',
        b'/' => b'?',
        b'`' => b'~',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Key;

    // ── CSI-u simple ────────────────────────────────────────────────

    #[test]
    fn test_csi_u_simple_no_mods() {
        assert_eq!(csi_u_simple(13, 1), b"\x1b[13u");
    }

    #[test]
    fn test_csi_u_simple_with_mods() {
        assert_eq!(csi_u_simple(13, 5), b"\x1b[13;5u");
        assert_eq!(csi_u_simple(97, 2), b"\x1b[97;2u");
    }

    // ── Kitty modifiers ─────────────────────────────────────────────

    #[test]
    fn test_kitty_mod_idx() {
        assert_eq!(kitty_mod_idx(false, false, false), 1);
        assert_eq!(kitty_mod_idx(false, false, true), 2);
        assert_eq!(kitty_mod_idx(false, true, false), 3);
        assert_eq!(kitty_mod_idx(true, false, false), 5);
        assert_eq!(kitty_mod_idx(false, true, true), 4);
        assert_eq!(kitty_mod_idx(true, true, false), 7);
        assert_eq!(kitty_mod_idx(true, false, true), 6);
        assert_eq!(kitty_mod_idx(true, true, true), 8);
    }

    // ── Full Kitty sequence (u terminator) ───────────────────────────

    #[test]
    fn test_kitty_seq_bare() {
        // Just the key code, no mods, no extras.
        let seq = kitty_seq(13, Terminator::U, 1, None, &[None, None], &[]);
        assert_eq!(seq, b"\x1b[13u");
    }

    #[test]
    fn test_kitty_seq_mods() {
        // With modifiers.
        let seq = kitty_seq(97, Terminator::U, 5, None, &[None, None], &[]);
        assert_eq!(seq, b"\x1b[97;5u");
    }

    #[test]
    fn test_kitty_seq_event_release() {
        // Release event.
        let seq = kitty_seq(13, Terminator::U, 1, Some(3), &[None, None], &[]);
        assert_eq!(seq, b"\x1b[13;1:3u");
    }

    #[test]
    fn test_kitty_seq_event_repeat() {
        // Repeat event.
        let seq = kitty_seq(13, Terminator::U, 1, Some(2), &[None, None], &[]);
        assert_eq!(seq, b"\x1b[13;1:2u");
    }

    #[test]
    fn test_kitty_seq_alternates_both() {
        // With shifted and base alternates.
        let seq = kitty_seq(97, Terminator::U, 2, None, &[Some(65), Some(97)], &[]);
        assert_eq!(seq, b"\x1b[97:65:97;2u"); // 'a' shifted='A'(65) base='a'(97)
    }

    #[test]
    fn test_kitty_seq_alternates_shifted_only() {
        // Only shifted alternate.
        let seq = kitty_seq(97, Terminator::U, 2, None, &[Some(65), None], &[]);
        assert_eq!(seq, b"\x1b[97:65;2u");
    }

    #[test]
    fn test_kitty_seq_alternates_base_only() {
        // Only base alternate (uses ::base syntax).
        let seq = kitty_seq(97, Terminator::U, 2, None, &[None, Some(97)], &[]);
        assert_eq!(seq, b"\x1b[97::97;2u");
    }

    #[test]
    fn test_kitty_seq_associated_text() {
        // With associated text.
        let seq = kitty_seq(97, Terminator::U, 1, None, &[None, None], &[97]);
        assert_eq!(seq, b"\x1b[97;1;97u");
    }

    #[test]
    fn test_kitty_seq_associated_text_with_mods() {
        // With mods and associated text.
        let seq = kitty_seq(106, Terminator::U, 5, None, &[None, None], &[106]);
        assert_eq!(seq, b"\x1b[106;5;106u");
    }

    // ── Full Kitty sequence (tilde terminator) ───────────────────────

    #[test]
    fn test_kitty_seq_tilde() {
        let seq = kitty_seq(15, Terminator::Tilde, 1, None, &[None, None], &[]);
        assert_eq!(seq, b"\x1b[15~");
    }

    #[test]
    fn test_kitty_seq_tilde_mods() {
        let seq = kitty_seq(15, Terminator::Tilde, 5, None, &[None, None], &[]);
        assert_eq!(seq, b"\x1b[15;5~");
    }

    // ── Special format (letter terminator) ───────────────────────────

    #[test]
    fn test_kitty_seq_special() {
        // ArrowUp with Ctrl
        let seq = kitty_seq(1, Terminator::Letter(b'A'), 5, None, &[None, None], &[]);
        assert_eq!(seq, b"\x1b[1;5A");
    }

    #[test]
    fn test_kitty_seq_special_event() {
        // ArrowUp with Ctrl + release event
        let seq = kitty_seq(1, Terminator::Letter(b'A'), 5, Some(3), &[None, None], &[]);
        assert_eq!(seq, b"\x1b[1;5:3A");
    }

    // ── ASCII alternates ─────────────────────────────────────────────

    #[test]
    fn test_ascii_alternates_letter() {
        let (shifted, unshifted) = ascii_alternates(Key::A, None).unwrap();
        assert_eq!(shifted, 65); // 'A'
        assert_eq!(unshifted, 97); // 'a'
    }

    #[test]
    fn test_ascii_alternates_digit() {
        let (shifted, unshifted) = ascii_alternates(Key::Num1, None).unwrap();
        assert_eq!(shifted, 33); // '!'
        assert_eq!(unshifted, 49); // '1'
    }

    #[test]
    fn test_ascii_alternates_symbol() {
        let (shifted, unshifted) = ascii_alternates(Key::Slash, None).unwrap();
        assert_eq!(shifted, 63); // '?'
        assert_eq!(unshifted, 47); // '/'
    }

    #[test]
    fn test_ascii_alternates_physical_key() {
        // When physical key differs from logical (e.g. Russian layout),
        // the logical key gives the shifted value and the physical key
        // gives the unshifted value.
        let (shifted, unshifted) =
            ascii_alternates(Key::A, Some(Key::F)).unwrap();
        // shifted: logical Key::A → key_to_ascii=97 → to_ascii_uppercase=65 ('A')
        // unshifted: physical Key::F → key_to_ascii=102 ('f')
        assert_eq!(shifted, 65);   // 'A'
        assert_eq!(unshifted, 102); // 'f'
    }}
