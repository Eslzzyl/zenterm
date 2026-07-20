//! Kitty keyboard protocol — functional key definitions.
//!
//! Maps [`egui::Key`] values to their numeric key-code and terminator
//! character as specified by the Kitty keyboard protocol:
//! <https://sw.kovidgoyal.net/kitty/keyboard-protocol/#functional-key-definitions>
//!
//! The table is derived from the one used by foot, which in turn follows
//! the Kitty specification.  Entries are ordered to match the spec so it
//! is easy to find the entry for a given key.

use egui::Key;

/// The final byte (terminator) of a Kitty CSI sequence for a functional key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Terminator {
    /// Terminate with `u` — full protocol format.
    U,
    /// Terminate with `~` — full protocol format.
    Tilde,
    /// Terminate with a letter (A, B, C, D, H, F, P, Q, S) — special
    /// (compact) format used for cursor keys, Home/End, F1–F4.
    Letter(u8),
}

/// A single entry in the Kitty functional-key mapping table.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Entry {
    /// The egui key.
    pub key: Key,
    /// Numeric code used in the CSI sequence (e.g. 13 for Enter).
    pub code: u32,
    /// How the sequence is terminated.
    pub terminator: Terminator,
}

// ── Lookup ────────────────────────────────────────────────────────────

/// Look up `key` in the Kitty functional-key table.
///
/// Returns `None` for keys that are not in the table (they should be
/// encoded as Unicode code-points instead).
pub(crate) fn lookup(key: &Key) -> Option<&'static Entry> {
    TABLE.iter().find(|e| &e.key == key)
}

// ── Table ─────────────────────────────────────────────────────────────

/// The full Kitty functional-key mapping table.
///
/// Based on <https://sw.kovidgoyal.net/kitty/keyboard-protocol/#functional-key-definitions>
/// ported from foot's `kitty-keymap.h`.
const TABLE: &[Entry] = &[
    // ── Editing / basic ─────────────────────────────────────────────
    Entry { key: Key::Escape, code: 27, terminator: Terminator::U },
    Entry { key: Key::Enter, code: 13, terminator: Terminator::U },
    Entry { key: Key::Tab, code: 9, terminator: Terminator::U },
    Entry { key: Key::Backspace, code: 127, terminator: Terminator::U },
    Entry { key: Key::Insert, code: 2, terminator: Terminator::Tilde },
    Entry { key: Key::Delete, code: 3, terminator: Terminator::Tilde },
    // ── Cursor motion ───────────────────────────────────────────────
    Entry { key: Key::ArrowLeft, code: 1, terminator: Terminator::Letter(b'D') },
    Entry { key: Key::ArrowRight, code: 1, terminator: Terminator::Letter(b'C') },
    Entry { key: Key::ArrowUp, code: 1, terminator: Terminator::Letter(b'A') },
    Entry { key: Key::ArrowDown, code: 1, terminator: Terminator::Letter(b'B') },
    // ── Navigation ──────────────────────────────────────────────────
    Entry { key: Key::PageUp, code: 5, terminator: Terminator::Tilde },
    Entry { key: Key::PageDown, code: 6, terminator: Terminator::Tilde },
    Entry { key: Key::Home, code: 1, terminator: Terminator::Letter(b'H') },
    Entry { key: Key::End, code: 1, terminator: Terminator::Letter(b'F') },
    // ── Function keys ───────────────────────────────────────────────
    Entry { key: Key::F1, code: 1, terminator: Terminator::Letter(b'P') },
    Entry { key: Key::F2, code: 1, terminator: Terminator::Letter(b'Q') },
    Entry { key: Key::F3, code: 13, terminator: Terminator::Tilde },
    Entry { key: Key::F4, code: 1, terminator: Terminator::Letter(b'S') },
    Entry { key: Key::F5, code: 15, terminator: Terminator::Tilde },
    Entry { key: Key::F6, code: 17, terminator: Terminator::Tilde },
    Entry { key: Key::F7, code: 18, terminator: Terminator::Tilde },
    Entry { key: Key::F8, code: 19, terminator: Terminator::Tilde },
    Entry { key: Key::F9, code: 20, terminator: Terminator::Tilde },
    Entry { key: Key::F10, code: 21, terminator: Terminator::Tilde },
    Entry { key: Key::F11, code: 23, terminator: Terminator::Tilde },
    Entry { key: Key::F12, code: 24, terminator: Terminator::Tilde },
    // F13–F35 all use the 'u' terminator with private-use-area codes.
    Entry { key: Key::F13, code: 57376, terminator: Terminator::U },
    Entry { key: Key::F14, code: 57377, terminator: Terminator::U },
    Entry { key: Key::F15, code: 57378, terminator: Terminator::U },
    Entry { key: Key::F16, code: 57379, terminator: Terminator::U },
    Entry { key: Key::F17, code: 57380, terminator: Terminator::U },
    Entry { key: Key::F18, code: 57381, terminator: Terminator::U },
    Entry { key: Key::F19, code: 57382, terminator: Terminator::U },
    Entry { key: Key::F20, code: 57383, terminator: Terminator::U },
    Entry { key: Key::F21, code: 57384, terminator: Terminator::U },
    Entry { key: Key::F22, code: 57385, terminator: Terminator::U },
    Entry { key: Key::F23, code: 57386, terminator: Terminator::U },
    Entry { key: Key::F24, code: 57387, terminator: Terminator::U },
    Entry { key: Key::F25, code: 57388, terminator: Terminator::U },
    Entry { key: Key::F26, code: 57389, terminator: Terminator::U },
    Entry { key: Key::F27, code: 57390, terminator: Terminator::U },
    Entry { key: Key::F28, code: 57391, terminator: Terminator::U },
    Entry { key: Key::F29, code: 57392, terminator: Terminator::U },
    Entry { key: Key::F30, code: 57393, terminator: Terminator::U },
    Entry { key: Key::F31, code: 57394, terminator: Terminator::U },
    Entry { key: Key::F32, code: 57395, terminator: Terminator::U },
    Entry { key: Key::F33, code: 57396, terminator: Terminator::U },
    Entry { key: Key::F34, code: 57397, terminator: Terminator::U },
    Entry { key: Key::F35, code: 57398, terminator: Terminator::U },
];

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_escape() {
        let e = lookup(&Key::Escape).expect("Escape should be in the table");
        assert_eq!(e.code, 27);
        assert_eq!(e.terminator, Terminator::U);
    }

    #[test]
    fn test_lookup_arrow_up() {
        let e = lookup(&Key::ArrowUp).expect("ArrowUp should be in the table");
        assert_eq!(e.code, 1);
        assert_eq!(e.terminator, Terminator::Letter(b'A'));
    }

    #[test]
    fn test_lookup_f3() {
        let e = lookup(&Key::F3).expect("F3 should be in the table");
        assert_eq!(e.code, 13);
        assert_eq!(e.terminator, Terminator::Tilde);
    }

    #[test]
    fn test_lookup_f13() {
        let e = lookup(&Key::F13).expect("F13 should be in the table");
        assert_eq!(e.code, 57376);
        assert_eq!(e.terminator, Terminator::U);
    }

    #[test]
    fn test_lookup_missing_returns_none() {
        assert!(lookup(&Key::A).is_none());
        assert!(lookup(&Key::Space).is_none());
    }
}
