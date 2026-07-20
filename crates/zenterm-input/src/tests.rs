use super::*;
use egui::Modifiers;

// ── Helper shortcuts ───────────────────────────────────────────

/// Default mapping options for tests (app_cursor off, option_as_alt on, no kitty).
///
/// We enable `macos_option_as_alt` so that Alt+key tests work on macOS
/// (where the default configuration leaves it off).  Unit tests for the
/// mapping logic should always treat the Alt modifier as active.
fn default_opts() -> MappingOptions {
    MappingOptions {
        macos_option_as_alt: true,
        ..MappingOptions::new()
    }
}

/// Options with specific Kitty flags.
fn kitty_opts(flags: KittyKeyboardFlags) -> MappingOptions {
    MappingOptions {
        kitty_flags: Some(flags),
        ..Default::default()
    }
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

/// Key event with repeat and pressed flags.
fn key_event_full(
    key: Key,
    pressed: bool,
    repeat: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat,
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

fn assert_map_none_with(event: &egui::Event, opts: &MappingOptions) {
    let result = InputMapper::map(event, opts);
    assert_eq!(result, None, "event={event:?}");
}

// ═══════════════════════════════════════════════════════════════════
// ── Legacy: plain navigation keys ─────────────────────────────────
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_enter() {
    assert_map(&key_event(Key::Enter, false, false, false), b"\r");
}

#[test]
fn test_tab() {
    assert_map(&key_event(Key::Tab, false, false, false), b"\t");
}

#[test]
fn test_backspace() {
    assert_map(&key_event(Key::Backspace, false, false, false), b"\x7f");
}

#[test]
fn test_escape() {
    assert_map(&key_event(Key::Escape, false, false, false), b"\x1b");
}

// ── Legacy: Enter/Tab/Backspace with modifiers ───────────────────

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
fn test_shift_tab() {
    assert_map(&key_event(Key::Tab, false, false, true), b"\x1bZ");
}

#[test]
fn test_alt_escape() {
    assert_map(&key_event(Key::Escape, false, true, false), b"\x1b\x1b");
}

// ── Legacy: arrows ───────────────────────────────────────────────

#[test]
fn test_arrows() {
    assert_map(&key_event(Key::ArrowUp, false, false, false), b"\x1b[A");
    assert_map(&key_event(Key::ArrowDown, false, false, false), b"\x1b[B");
    assert_map(&key_event(Key::ArrowRight, false, false, false), b"\x1b[C");
    assert_map(&key_event(Key::ArrowLeft, false, false, false), b"\x1b[D");
}

#[test]
fn test_arrows_with_mods() {
    assert_map(&key_event(Key::ArrowUp, true, false, false), b"\x1b[1;5A");
    assert_map(&key_event(Key::ArrowDown, true, false, false), b"\x1b[1;5B");
    assert_map(&key_event(Key::ArrowRight, true, false, false), b"\x1b[1;5C");
    assert_map(&key_event(Key::ArrowLeft, true, false, false), b"\x1b[1;5D");
}

#[test]
fn test_arrows_alt_shift() {
    assert_map(&key_event(Key::ArrowLeft, false, true, true), b"\x1b[1;4D");
}

// ── Legacy: app_cursor mode ──────────────────────────────────────

#[test]
fn test_app_cursor_arrows() {
    let opts = MappingOptions {
        app_cursor: true,
        ..MappingOptions::new()
    };
    assert_map_with(&key_event(Key::ArrowUp, false, false, false), b"\x1bOA", &opts);
    assert_map_with(&key_event(Key::ArrowDown, false, false, false), b"\x1bOB", &opts);
    assert_map_with(&key_event(Key::ArrowRight, false, false, false), b"\x1bOC", &opts);
    assert_map_with(&key_event(Key::ArrowLeft, false, false, false), b"\x1bOD", &opts);
    assert_map_with(&key_event(Key::Home, false, false, false), b"\x1bOH", &opts);
    assert_map_with(&key_event(Key::End, false, false, false), b"\x1bOF", &opts);
}

#[test]
fn test_app_cursor_with_mods() {
    let opts = MappingOptions {
        app_cursor: true,
        ..MappingOptions::new()
    };
    assert_map_with(&key_event(Key::ArrowUp, true, false, false), b"\x1b[1;5A", &opts);
    assert_map_with(&key_event(Key::ArrowDown, true, false, false), b"\x1b[1;5B", &opts);
}

// ── Legacy: Home / End ───────────────────────────────────────────

#[test]
fn test_home_end() {
    assert_map(&key_event(Key::Home, false, false, false), b"\x1b[H");
    assert_map(&key_event(Key::End, false, false, false), b"\x1b[F");
}

#[test]
fn test_home_end_mods() {
    assert_map(&key_event(Key::Home, true, false, false), b"\x1b[1;5H");
    assert_map(&key_event(Key::End, true, false, false), b"\x1b[1;5F");
}

// ── Legacy: PageUp/Down, Insert, Delete ──────────────────────────

#[test]
fn test_page_up_down() {
    assert_map(&key_event(Key::PageUp, false, false, false), b"\x1b[5~");
    assert_map(&key_event(Key::PageDown, false, false, false), b"\x1b[6~");
}

#[test]
fn test_insert() {
    assert_map(&key_event(Key::Insert, false, false, false), b"\x1b[2~");
}

#[test]
fn test_delete() {
    assert_map(&key_event(Key::Delete, false, false, false), b"\x1b[3~");
}

// ── Legacy: Function keys ────────────────────────────────────────

#[test]
fn test_f1_f4() {
    assert_map(&key_event(Key::F1, false, false, false), b"\x1bOP");
    assert_map(&key_event(Key::F2, false, false, false), b"\x1bOQ");
    assert_map(&key_event(Key::F3, false, false, false), b"\x1bOR");
    assert_map(&key_event(Key::F4, false, false, false), b"\x1bOS");
}

#[test]
fn test_f5_f12() {
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
fn test_f13_f20() {
    assert_map(&key_event(Key::F13, false, false, false), b"\x1b[57376~");
    assert_map(&key_event(Key::F14, false, false, false), b"\x1b[57377~");
    assert_map(&key_event(Key::F15, false, false, false), b"\x1b[57378~");
    assert_map(&key_event(Key::F16, false, false, false), b"\x1b[57379~");
    assert_map(&key_event(Key::F17, false, false, false), b"\x1b[57380~");
    assert_map(&key_event(Key::F18, false, false, false), b"\x1b[57381~");
    assert_map(&key_event(Key::F19, false, false, false), b"\x1b[57382~");
    assert_map(&key_event(Key::F20, false, false, false), b"\x1b[57383~");
}

#[test]
fn test_fkeys_with_mods() {
    assert_map(&key_event(Key::F1, true, false, false), b"\x1b[1;5P");
    assert_map(&key_event(Key::F2, false, true, false), b"\x1b[1;3Q");
    assert_map(&key_event(Key::F5, true, false, false), b"\x1b[15;5~");
}

// ── Legacy: Ctrl+letter → C0 ─────────────────────────────────────

#[test]
fn test_ctrl_letter() {
    assert_map(&key_event(Key::A, true, false, false), b"\x01");
    assert_map(&key_event(Key::B, true, false, false), b"\x02");
    assert_map(&key_event(Key::C, true, false, false), b"\x03");
    assert_map(&key_event(Key::Z, true, false, false), b"\x1a");
}

#[test]
fn test_ctrl_letter_alt() {
    // Ctrl+Alt should NOT produce C0 — it's handled by the alt-extended path.
    let ev = key_event(Key::Q, true, true, false);
    let result = InputMapper::map(&ev, &default_opts());
    // With both ctrl and alt, legacy_ctrl_fallback skips (needs ctrl && !alt).
    assert_eq!(result, None);
}

// ── Legacy: Ctrl+digit/symbol (extended C0) ──────────────────────

#[test]
fn test_ctrl_extended() {
    assert_map(&key_event(Key::Num2, true, false, false), b"\x00");
    assert_map(&key_event(Key::Num3, true, false, false), b"\x1b");
    assert_map(&key_event(Key::Num4, true, false, false), b"\x1c");
    assert_map(&key_event(Key::Num5, true, false, false), b"\x1d");
    assert_map(&key_event(Key::Num6, true, false, false), b"\x1e");
    assert_map(&key_event(Key::Num7, true, false, false), b"\x1f");
    assert_map(&key_event(Key::Num8, true, false, false), b"\x7f");
    assert_map(&key_event(Key::Num9, true, false, false), b"9");
    assert_map(&key_event(Key::Num0, true, false, false), b"0");
    assert_map(&key_event(Key::Num1, true, false, false), b"1");
}

#[test]
fn test_ctrl_slash_question() {
    assert_map(&key_event(Key::Slash, true, false, false), b"\x1f");
    assert_map(&key_event(Key::Questionmark, true, false, false), b"\x7f");
}

#[test]
fn test_ctrl_brackets() {
    assert_map(&key_event(Key::OpenBracket, true, false, false), b"\x1b");
    assert_map(&key_event(Key::CloseBracket, true, false, false), b"\x1d");
}

#[test]
fn test_ctrl_space() {
    assert_map(&key_event(Key::Space, true, false, false), b"\x00");
}

// ── Legacy: physical key fallback for non-Latin layouts ──────────

#[test]
fn test_ctrl_physical_key_fallback() {
    // Simulate Russian layout: logical Key::A (Cyrillic) maps to physical Key::F
    // The legacy encoder uses the logical key when key_to_ascii succeeds for it.
    let event = key_event_with_physical(Key::A, Key::F, true, false, false);
    let result = InputMapper::map(&event, &default_opts());
    // key_to_ascii(Key::A) returns b'a', so Ctrl+A → 0x01
    assert_eq!(result, Some(vec![0x01]));
}

#[test]
fn test_alt_physical_key_fallback() {
    // Russian layout: logical A, physical F, Alt+key → ESC + logical char
    let event = key_event_with_physical(Key::A, Key::F, false, true, false);
    let result = InputMapper::map(&event, &default_opts());
    assert_eq!(result, Some(vec![0x1b, b'a']));
}

// ── Legacy: Alt+key → ESC prefix ─────────────────────────────────

#[test]
fn test_alt_letter() {
    assert_map(&key_event(Key::A, false, true, false), b"\x1ba");
    assert_map(&key_event(Key::Z, false, true, false), b"\x1bz");
}

#[test]
fn test_alt_shift_letter() {
    let ev = key_event(Key::A, false, true, true);
    // Alt+Shift+A should produce ESC + 'A' (uppercase)
    let result = InputMapper::map(&ev, &default_opts());
    assert_eq!(result, Some(b"\x1bA".to_vec()));
}

#[test]
fn test_alt_digit() {
    assert_map(&key_event(Key::Num0, false, true, false), b"\x1b0");
    assert_map(&key_event(Key::Num9, false, true, false), b"\x1b9");
}

#[test]
fn test_alt_space() {
    assert_map(&key_event(Key::Space, false, true, false), b"\x1b ");
}

// ── Legacy: Text events ──────────────────────────────────────────

#[test]
fn test_text_plain() {
    assert_map(&text_event("hello"), b"hello");
}

#[test]
fn test_text_unicode() {
    assert_map(&text_event("é"), b"\xc3\xa9");
}

#[test]
fn test_text_empty() {
    assert_map_none(&text_event(""));
}

#[test]
fn test_text_control_only() {
    assert_map_none(&text_event("\x00\x01"));
}

// ── Legacy: Paste ────────────────────────────────────────────────

#[test]
fn test_paste() {
    assert_map(&paste_event("hello"), b"hello");
}

#[test]
fn test_paste_empty() {
    assert_map_none(&paste_event(""));
}

// ── Legacy: unmapped keys ────────────────────────────────────────

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
    // F13 is now explicitly handled via tilde_seq_raw(57376, mod_idx).
    assert_map(&key_event(Key::F13, false, false, false), b"\x1b[57376~");
}

#[test]
fn test_f35_not_handled() {
    // F35 is not in the explicit match → falls through to None.
    assert_map_none(&key_event(Key::F35, false, false, false));
}
// ═══════════════════════════════════════════════════════════════════
// ── Kitty: DISAMBIGUATE_ESCAPE_CODES (CSI-u simple) ───────────────
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_kitty_disambiguate_ctrl_letter() {
    // Ctrl+A should send CSI u sequence instead of raw 0x01.
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_map_with(
        &key_event(Key::A, true, false, false),
        b"\x1b[97;5u",
        &opts,
    );
}

#[test]
fn test_kitty_disambiguate_ctrl_shift_a() {
    // Ctrl+Shift+A should send CSI u with mods=6 (ctrl+shift) and
    // the *unshifted* base code 'a' (97) per the Kitty spec.
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_map_with(
        &key_event(Key::A, true, false, true),
        b"\x1b[97;6u",
        &opts,
    );
}

#[test]
fn test_kitty_disambiguate_alt_a() {
    // Alt+A should send CSI u with mods=3 (alt).
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_map_with(
        &key_event(Key::A, false, true, false),
        b"\x1b[97;3u",
        &opts,
    );
}

#[test]
fn test_kitty_disambiguate_arrow_up_ctrl() {
    // Ctrl+ArrowUp → CSI u instead of legacy CSI
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_map_with(
        &key_event(Key::ArrowUp, true, false, false),
        b"\x1b[1;5A",
        &opts,
    );
}

#[test]
fn test_kitty_disambiguate_enter_with_shift() {
    // Shift+Enter should send CSI u, not bare \r.
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_map_with(
        &key_event(Key::Enter, false, false, true),
        b"\x1b[13;2u",
        &opts,
    );
}

#[test]
fn test_kitty_disambiguate_backspace_with_shift() {
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_map_with(
        &key_event(Key::Backspace, false, false, true),
        b"\x1b[127;2u",
        &opts,
    );
}

#[test]
fn test_kitty_disambiguate_tab_with_shift() {
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_map_with(
        &key_event(Key::Tab, false, false, true),
        b"\x1b[9;2u",
        &opts,
    );
}

#[test]
fn test_kitty_disambiguate_unmodified_keys_still_plain() {
    // Without REPORT_ALL, unmodified keys should still use legacy path.
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    // Enter without modifiers → \r
    assert_map_with(&key_event(Key::Enter, false, false, false), b"\r", &opts);
    // Tab without modifiers → \t
    assert_map_with(&key_event(Key::Tab, false, false, false), b"\t", &opts);
    // Backspace without modifiers → \x7f
    assert_map_with(&key_event(Key::Backspace, false, false, false), b"\x7f", &opts);
    // Printable char without modifiers → None (handled by Event::Text)
    assert_map_none_with(&key_event(Key::A, false, false, false), &opts);
}

// ═══════════════════════════════════════════════════════════════════
// ── Kitty: REPORT_ALL_KEYS_AS_ESCAPE_CODES ────────────────────────
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_kitty_report_all_enter() {
    // Even unmodified Enter should send CSI u in REPORT_ALL mode.
    let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
    let opts = kitty_opts(flags);
    assert_map_with(
        &key_event(Key::Enter, false, false, false),
        b"\x1b[13u",
        &opts,
    );
}

#[test]
fn test_kitty_report_all_backspace() {
    let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
    let opts = kitty_opts(flags);
    assert_map_with(
        &key_event(Key::Backspace, false, false, false),
        b"\x1b[127u",
        &opts,
    );
}

#[test]
fn test_kitty_report_all_tab() {
    let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
    let opts = kitty_opts(flags);
    assert_map_with(
        &key_event(Key::Tab, false, false, false),
        b"\x1b[9u",
        &opts,
    );
}

// ═══════════════════════════════════════════════════════════════════
// ── Kitty: REPORT_EVENT_TYPES ─────────────────────────────────────
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_kitty_event_type_repeat() {
    let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_EVENT_TYPES;
    let opts = kitty_opts(flags);
    // Repeat event for 'a'
    assert_map_with(
        &key_event_full(Key::A, true, true, true, false, false),
        b"\x1b[97;5:2u",
        &opts,
    );
}

#[test]
fn test_kitty_event_type_release() {
    let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_EVENT_TYPES;
    let opts = kitty_opts(flags);
    // Release event for Escape
    assert_map_with(
        &key_event_full(Key::Escape, false, false, false, false, false),
        b"\x1b[27;1:3u",
        &opts,
    );
}

#[test]
fn test_kitty_event_type_release_enter_no_report_all() {
    // Without REPORT_ALL, Enter release is suppressed.
    let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_EVENT_TYPES;
    let opts = kitty_opts(flags);
    assert_map_none_with(
        &key_event_full(Key::Enter, false, false, false, false, false),
        &opts,
    );
}

#[test]
fn test_kitty_event_type_release_enter_report_all() {
    // With REPORT_ALL, Enter release is reported.
    let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_EVENT_TYPES
        | KittyKeyboardFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
    let opts = kitty_opts(flags);
    assert_map_with(
        &key_event_full(Key::Enter, false, false, false, false, false),
        b"\x1b[13;1:3u",
        &opts,
    );
}

#[test]
fn test_kitty_without_event_types_ignores_release() {
    // Without REPORT_EVENT_TYPES, release events are ignored.
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_map_none_with(
        &key_event_full(Key::Escape, false, false, false, false, false),
        &opts,
    );
}

// ═══════════════════════════════════════════════════════════════════
// ── Kitty: REPORT_ALTERNATE_KEYS ──────────────────────────────────
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_kitty_alternates_letter() {
    let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_ALTERNATE_KEYS;
    let opts = kitty_opts(flags);
    // Shift+A: primary='a'(97), alternates=('A','a'), mods=2(shift)
    assert_map_with(
        &key_event(Key::A, false, false, true),
        b"\x1b[97:65:97;2u",
        &opts,
    );
}

#[test]
fn test_kitty_alternates_ctrl_a() {
    let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_ALTERNATE_KEYS;
    let opts = kitty_opts(flags);
    // Ctrl+A: unshifted='a'(97), shifted='A'(65)
    assert_map_with(
        &key_event(Key::A, true, false, false),
        b"\x1b[97:65:97;5u",
        &opts,
    );
}

// ═══════════════════════════════════════════════════════════════════
// ── Kitty: REPORT_ASSOCIATED_TEXT ─────────────────────────────────
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_kitty_associated_text() {
    let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_ASSOCIATED_TEXT;
    let opts = kitty_opts(flags);
    // 'j' with ctrl: key=106, mods=5, text=106
    assert_map_with(
        &key_event(Key::J, true, false, false),
        b"\x1b[106;5;106u",
        &opts,
    );
}

// ═══════════════════════════════════════════════════════════════════
// ── Kitty: functional keys with various flags ─────────────────────
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_kitty_f5_ctrl() {
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_map_with(
        &key_event(Key::F5, true, false, false),
        b"\x1b[15;5~",
        &opts,
    );
}

#[test]
fn test_kitty_f13_unmodified() {
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES);
    assert_map_with(
        &key_event(Key::F13, false, false, false),
        b"\x1b[57376u",
        &opts,
    );
}

#[test]
fn test_kitty_page_up_with_shift() {
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_map_with(
        &key_event(Key::PageUp, false, false, true),
        b"\x1b[5;2~",
        &opts,
    );
}

#[test]
fn test_kitty_arrow_left_alt() {
    let opts = kitty_opts(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_map_with(
        &key_event(Key::ArrowLeft, false, true, false),
        b"\x1b[1;3D",
        &opts,
    );
}

// ═══════════════════════════════════════════════════════════════════
// ── Kitty: composite flag combinations ────────────────────────────
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_kitty_all_flags_arrow_up() {
    // All flags: ArrowUp with Ctrl+Shift (mods=6)
    let flags = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_EVENT_TYPES
        | KittyKeyboardFlags::REPORT_ALTERNATE_KEYS
        | KittyKeyboardFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        | KittyKeyboardFlags::REPORT_ASSOCIATED_TEXT;
    let opts = kitty_opts(flags);
    assert_map_with(
        &key_event(Key::ArrowUp, true, false, true),
        b"\x1b[1;6A",
        &opts,
    );
}

#[test]
fn test_kitty_all_flags_ctrl_j_repeat() {
    let flags = KittyKeyboardFlags::all();
    let opts = kitty_opts(flags);
    // Ctrl+J repeat: key=106, mods=5, event=2, alternates (J=74, j=106), text=106
    assert_map_with(
        &key_event_full(Key::J, true, true, true, false, false),
        b"\x1b[106:74:106;5:2;106u",
        &opts,
    );
}

// ═══════════════════════════════════════════════════════════════════
// ── Kitty: Kitty explicitly disabled (no flags / None) ────────────
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_kitty_disabled_fallback_to_legacy() {
    // kitty_flags = None → legacy path
    let opts = MappingOptions::default(); // kitty_flags=None
    assert_map_with(&key_event(Key::A, true, false, false), b"\x01", &opts);
    assert_map_with(&key_event(Key::ArrowUp, true, false, false), b"\x1b[1;5A", &opts);
    assert_map_with(&key_event(Key::Enter, false, false, true), b"\r", &opts);
}

#[test]
fn test_kitty_flags_empty_bitset() {
    // kitty_flags = Some(0) → same as None, use legacy
    let opts = MappingOptions {
        kitty_flags: Some(KittyKeyboardFlags::NONE),
        ..Default::default()
    };
    assert_map_with(&key_event(Key::A, true, false, false), b"\x01", &opts);
}

// ═══════════════════════════════════════════════════════════════════
// ── KittyKeyboardFlags::from_term_mode ────────────────────────────
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_from_term_mode_none() {
    let mode: u32 = 0;
    assert_eq!(KittyKeyboardFlags::from_term_mode(mode), None);
}

#[test]
fn test_from_term_mode_disambiguate() {
    // TermMode::DISAMBIGUATE_ESC_CODES = 1 << 18 = 262144
    let mode: u32 = 1 << 18;
    let flags = KittyKeyboardFlags::from_term_mode(mode).unwrap();
    assert!(flags.contains(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES));
    assert!(!flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES));
}

#[test]
fn test_from_term_mode_all() {
    // All 5 Kitty bits: bits 18-22 set
    let mode: u32 = 0x1f << 18;
    let flags = KittyKeyboardFlags::from_term_mode(mode).unwrap();
    assert_eq!(flags, KittyKeyboardFlags::all());
}

#[test]
fn test_from_term_mode_mixed() {
    // Bits 18 and 20 set = DISAMBIGUATE | REPORT_ALTERNATE
    let mode: u32 = (1 << 18) | (1 << 20);
    let flags = KittyKeyboardFlags::from_term_mode(mode).unwrap();
    assert!(flags.contains(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES));
    assert!(!flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES));
    assert!(flags.contains(KittyKeyboardFlags::REPORT_ALTERNATE_KEYS));
}
