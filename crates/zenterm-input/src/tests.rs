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
