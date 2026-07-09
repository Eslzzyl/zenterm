//! OSC 7 (current working directory) scanner.
//!
//! Scans raw PTY output for `ESC ] 7 ; <url> (BEL|ST)` sequences that
//! report the shell's current working directory.

use memchr::{memchr, memchr2};

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
    // Find `ESC ] 7 ;` introducer via SIMD ESC hunt.
    let mut start = 0;
    while let Some(esc_pos) = memchr(0x1B, &bytes[start..]) {
        let i = start + esc_pos;
        if i + 3 < bytes.len()
            && bytes[i + 1] == b']'
            && bytes[i + 2] == b'7'
            && bytes[i + 3] == b';'
        {
            let payload_start = i + 4;
            // Scan for the EARLIEST terminator (BEL or ST) in the payload tail.
            // Left-to-right precedence: whichever byte (0x07 or 0x1B) appears
            // first wins.
            let tail = &bytes[payload_start..];
            let mut j = 0;
            loop {
                // Find whichever of BEL (0x07) or ESC (0x1B, possible ST start)
                // comes first from the current search position.
                let (off, byte) = match memchr2(0x07, 0x1B, &tail[j..]) {
                    Some(o) => {
                        let b = tail[j + o];
                        (o, b)
                    }
                    None => return None,
                };
                let abs = j + off;
                if byte == 0x07 {
                    // BEL terminator.
                    let payload = &tail[..abs];
                    return std::str::from_utf8(payload).ok().map(|s| s.to_string());
                }
                // byte == 0x1B — possible ST start.
                if abs + 1 < tail.len() && tail[abs + 1] == b'\\' {
                    let payload = &tail[..abs];
                    return std::str::from_utf8(payload).ok().map(|s| s.to_string());
                }
                // False alarm — skip past this bare ESC and keep looking.
                j = abs + 1;
            }
        }
        start = i + 1;
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

    #[test]
    fn prefers_earliest_terminator() {
        // ST appears before BEL — must return on ST.
        let bytes = b"\x1b]7;file://host/path\x1b\\trailing\x07garbage";
        assert_eq!(scan_osc7(bytes).as_deref(), Some("file://host/path"));

        // BEL appears before ST — must return on BEL.
        let bytes = b"\x1b]7;file://host/path\x07\x1b\\trailing";
        assert_eq!(scan_osc7(bytes).as_deref(), Some("file://host/path"));
    }
}
