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
