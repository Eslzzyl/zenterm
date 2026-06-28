//! OSC 7 (current working directory) scanner.
//!
//! Scans raw PTY output for `ESC ] 7 ; <url> (BEL|ST)` sequences that
//! report the shell's current working directory.

/// Find the first OSC 7 sequence in `bytes` and return its URL
/// payload (without the OSC introducer or terminator).
///
/// Recognised forms:
///
/// ```text
/// ESC ] 7 ; <url> BEL         (iTerm2 / most shells)
/// ESC ] 7 ; <url> ESC \       (ECMA-48 string terminator)
/// ```
///
/// Returns `None` if no well-formed OSC 7 is found.  The scan is
/// byte-oriented and intentionally cheap (no regex, no allocation
/// beyond the returned `String`).
pub(crate) fn scan_osc7(bytes: &[u8]) -> Option<String> {
    // Find `ESC ] 7 ;` introducer.
    let mut i = 0;
    while i + 3 < bytes.len() {
        if bytes[i] == 0x1B
            && bytes[i + 1] == b']'
            && bytes[i + 2] == b'7'
            && bytes[i + 3] == b';'
        {
            // Found the start.  Read until BEL or ST.
            let payload_start = i + 4;
            let mut j = payload_start;
            while j < bytes.len() {
                if bytes[j] == 0x07 {
                    // BEL terminator.
                    let payload = &bytes[payload_start..j];
                    if let Ok(s) = std::str::from_utf8(payload) {
                        return Some(s.to_string());
                    }
                    return None;
                }
                if bytes[j] == 0x1B && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                    // ST terminator.
                    let payload = &bytes[payload_start..j];
                    if let Ok(s) = std::str::from_utf8(payload) {
                        return Some(s.to_string());
                    }
                    return None;
                }
                j += 1;
            }
            // Unterminated — give up on this attempt.
            return None;
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod osc7_tests {
    use super::scan_osc7;

    #[test]
    fn parses_bel_terminated() {
        let bytes = b"\x1b]7;file://localhost/Users/me\x07";
        assert_eq!(scan_osc7(bytes).as_deref(), Some("file://localhost/Users/me"));
    }

    #[test]
    fn parses_st_terminated() {
        let bytes = b"\x1b]7;file://h/p\x1b\\";
        assert_eq!(scan_osc7(bytes).as_deref(), Some("file://h/p"));
    }

    #[test]
    fn finds_osc7_among_other_bytes() {
        let bytes = b"hello\x1b[31mred\x1b[0m\x1b]7;file://x/y\x07done";
        assert_eq!(scan_osc7(bytes).as_deref(), Some("file://x/y"));
    }

    #[test]
    fn no_osc7_returns_none() {
        let bytes = b"just normal bytes";
        assert_eq!(scan_osc7(bytes), None);
    }

    #[test]
    fn unterminated_returns_none() {
        let bytes = b"\x1b]7;file://x/y";
        assert_eq!(scan_osc7(bytes), None);
    }
}
