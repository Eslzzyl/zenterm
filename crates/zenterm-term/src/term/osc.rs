//! Unified OSC scanner.
//!
//! Scans raw PTY output for all `ESC ] <number> ; <payload> (BEL|ST)`
//! sequences in a single pass and returns structured matches for the
//! caller to dispatch by OSC number.
//!
//! This replaces the previous ad-hoc scanners (`scan_osc7`, `scan_osc9_or_777`)
//! with a single unified entry point.  Adding a new OSC handler requires
//! only a new dispatch arm — no new byte-scanning logic.

use memchr::memchr2;
use zenterm_core::Progress;

/// A single OSC match found in the byte stream.
#[derive(Debug, Clone)]
pub(crate) struct OscMatch {
    /// The OSC number (e.g. 7, 9, 777).
    pub number: u16,
    /// Payload between the number and the terminator (BEL/ST).
    /// This is the raw UTF-8 content after the first `;`.
    pub payload: String,
}

/// Scan `bytes` for all well-formed `ESC ] <number> ; <payload> (BEL|ST)`
/// sequences and return them in order of appearance.
///
/// Recognised terminators:
/// - BEL (`0x07`)
/// - ST (`ESC \`, i.e. `0x1B 0x5C`)
///
/// Sequences missing a terminator are silently ignored (assumed to be
/// split across PTY reads and will be completed in a future batch).
pub(crate) fn scan_oscs(bytes: &[u8]) -> Vec<OscMatch> {
    let mut results = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        // Find the next ESC byte.
        if bytes[i] != 0x1B {
            i += 1;
            continue;
        }
        // Must be followed by `]` to be an OSC introducer.
        if i + 1 >= bytes.len() || bytes[i + 1] != b']' {
            i += 1;
            continue;
        }

        // Read the OSC number (decimal digits after `ESC ]`).
        let num_start = i + 2;
        let mut num_end = num_start;
        while num_end < bytes.len() && bytes[num_end].is_ascii_digit() {
            num_end += 1;
        }
        if num_end == num_start || num_end >= bytes.len() || bytes[num_end] != b';' {
            // Malformed — no number or no `;` after number.
            i = num_start;
            continue;
        }

        let number: u16 = match std::str::from_utf8(&bytes[num_start..num_end]) {
            Ok(s) => match s.parse() {
                Ok(n) => n,
                Err(_) => {
                    i = num_end + 1;
                    continue;
                }
            },
            Err(_) => {
                i = num_end + 1;
                continue;
            }
        };

        // Payload starts after the `;`.
        let payload_start = num_end + 1;

        // Scan for the earliest terminator (BEL or ST).
        let tail = &bytes[payload_start..];
        match find_terminator(tail) {
            Some(end) => {
                let payload_bytes = &tail[..end];
                if let Ok(payload) = std::str::from_utf8(payload_bytes) {
                    results.push(OscMatch {
                        number,
                        payload: payload.to_string(),
                    });
                }
                // Advance past the entire OSC sequence.
                i = payload_start + end + match tail[end] {
                    0x07 => 1,      // BEL
                    _ => 2,         // ST (ESC \)
                };
            }
            None => {
                // Unterminated — stop scanning; the rest may be a
                // continuation in a future batch.
                break;
            }
        }
    }

    results
}

/// Find the earliest BEL (`0x07`) or ST (`ESC \`) in `tail`.
/// Returns `Some(offset)` where `offset` points to the terminator byte,
/// or `None` if no terminator is found.
fn find_terminator(tail: &[u8]) -> Option<usize> {
    let mut j = 0;
    loop {
        match memchr2(0x07, 0x1B, &tail[j..]) {
            Some(off) => {
                let abs = j + off;
                if tail[abs] == 0x07 {
                    return Some(abs);
                }
                // `0x1B` — possible ST start.
                if abs + 1 < tail.len() && tail[abs + 1] == b'\\' {
                    return Some(abs);
                }
                // Stray ESC — skip past it.
                j = abs + 1;
            }
            None => return None,
        }
    }
}

// ─── OSC-9 / ConEmu progress helpers ─────────────────────────────────

/// Parse a ConEmu progress-bar payload (`4;<state>;<pct>`).
///
/// Returns `None` if the payload does not match the `4;...` pattern.
pub(crate) fn parse_conemu_progress(payload: &str) -> Option<Progress> {
    let parts: Vec<&str> = payload.splitn(3, ';').collect();
    if parts.len() < 2 || parts[0] != "4" {
        return None;
    }
    let state = parts[1];
    let pct = parts.get(2).and_then(|s| s.parse::<u8>().ok());

    match state {
        "0" => Some(Progress::None),
        "1" => Some(Progress::Percentage(pct.unwrap_or(0).min(100))),
        "2" => Some(Progress::Error(pct.unwrap_or(0).min(100))),
        "3" => Some(Progress::Indeterminate),
        "4" => Some(Progress::None), // "Paused" — treated as no progress
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── scan_oscs ───────────────────────────────────────────────────

    #[test]
    fn single_osc7_bel() {
        let bytes = b"\x1b]7;file://localhost/Users/me\x07";
        let oscs = scan_oscs(bytes);
        assert_eq!(oscs.len(), 1);
        assert_eq!(oscs[0].number, 7);
        assert_eq!(oscs[0].payload, "file://localhost/Users/me");
    }

    #[test]
    fn single_osc7_st() {
        let bytes = b"\x1b]7;file://h/p\x1b\\";
        let oscs = scan_oscs(bytes);
        assert_eq!(oscs.len(), 1);
        assert_eq!(oscs[0].number, 7);
        assert_eq!(oscs[0].payload, "file://h/p");
    }

    #[test]
    fn osc9_notification() {
        let bytes = b"\x1b]9;hello world\x07";
        let oscs = scan_oscs(bytes);
        assert_eq!(oscs.len(), 1);
        assert_eq!(oscs[0].number, 9);
        assert_eq!(oscs[0].payload, "hello world");
    }

    #[test]
    fn osc777_notification() {
        let bytes = b"\x1b]777;notify;title;body\x07";
        let oscs = scan_oscs(bytes);
        assert_eq!(oscs.len(), 1);
        assert_eq!(oscs[0].number, 777);
        assert_eq!(oscs[0].payload, "notify;title;body");
    }

    #[test]
    fn conemu_progress() {
        let bytes = b"\x1b]9;4;0\x1b\\";
        let oscs = scan_oscs(bytes);
        assert_eq!(oscs.len(), 1);
        assert_eq!(oscs[0].number, 9);
        assert_eq!(oscs[0].payload, "4;0");
    }

    #[test]
    fn multiple_oscs() {
        let bytes = b"\x1b]7;file:///home\x07\x1b]9;hi\x07\x1b]777;notify;;\x07";
        let oscs = scan_oscs(bytes);
        assert_eq!(oscs.len(), 3);
        assert_eq!(oscs[0].number, 7);
        assert_eq!(oscs[1].number, 9);
        assert_eq!(oscs[2].number, 777);
    }

    #[test]
    fn interleaved_with_other_escape_sequences() {
        let bytes = b"hello\x1b[31mred\x1b[0m\x1b]7;file://x/y\x07done";
        let oscs = scan_oscs(bytes);
        assert_eq!(oscs.len(), 1);
        assert_eq!(oscs[0].number, 7);
        assert_eq!(oscs[0].payload, "file://x/y");
    }

    #[test]
    fn unterminated_returns_none() {
        let bytes = b"\x1b]7;file://x/y";
        let oscs = scan_oscs(bytes);
        assert!(oscs.is_empty());
    }

    #[test]
    fn prefers_earliest_terminator() {
        // ST appears before BEL.
        let bytes = b"\x1b]7;file://host/path\x1b\\trailing\x07garbage";
        let oscs = scan_oscs(bytes);
        assert_eq!(oscs.len(), 1);
        assert_eq!(oscs[0].payload, "file://host/path");

        // BEL appears before ST.
        let bytes = b"\x1b]7;file://host/path\x07\x1b\\trailing";
        let oscs = scan_oscs(bytes);
        assert_eq!(oscs.len(), 1);
        assert_eq!(oscs[0].payload, "file://host/path");
    }

    #[test]
    fn no_osc_returns_empty() {
        let bytes = b"just normal bytes";
        let oscs = scan_oscs(bytes);
        assert!(oscs.is_empty());
    }

    #[test]
    fn esc_in_payload_skips_stray() {
        // Stray ESC inside payload should be skipped, not treated as ST.
        let bytes = b"\x1b]7;file\x1b/path\x07";
        let oscs = scan_oscs(bytes);
        assert_eq!(oscs.len(), 1);
        assert_eq!(oscs[0].payload, "file\x1b/path");
    }

    // ── parse_conemu_progress ───────────────────────────────────────

    #[test]
    fn conemu_none() {
        assert_eq!(parse_conemu_progress("4;0"), Some(Progress::None));
    }

    #[test]
    fn conemu_percentage() {
        assert_eq!(parse_conemu_progress("4;1;42"), Some(Progress::Percentage(42)));
    }

    #[test]
    fn conemu_error() {
        assert_eq!(parse_conemu_progress("4;2;99"), Some(Progress::Error(99)));
    }

    #[test]
    fn conemu_indeterminate() {
        assert_eq!(parse_conemu_progress("4;3"), Some(Progress::Indeterminate));
    }

    #[test]
    fn conemu_paused_is_none() {
        assert_eq!(parse_conemu_progress("4;4"), Some(Progress::None));
    }

    #[test]
    fn conemu_percentage_clamped() {
        assert_eq!(parse_conemu_progress("4;1;150"), Some(Progress::Percentage(100)));
    }

    #[test]
    fn conemu_not_a_progress() {
        assert_eq!(parse_conemu_progress("hello"), None);
    }

    #[test]
    fn conemu_unknown_state() {
        assert_eq!(parse_conemu_progress("4;9"), None);
    }
}
