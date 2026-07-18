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
use zenterm_core::{Progress, SemanticClick, SemanticPrompt, SemanticPromptKind};

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

// ─── OSC-133 / FinalTerm semantic prompt ────────────────────────

/// Parse a single `key=value` parameter from `s`.
///
/// Returns `(key, value)` if `s` contains `=`, otherwise `None`.
fn split_key_value(s: &str) -> Option<(&str, &str)> {
    let mut parts = s.splitn(2, '=');
    let key = parts.next()?;
    let val = parts.next()?;
    Some((key, val))
}

/// Parse an OSC 133 payload into a [`SemanticPrompt`].
///
/// The payload is the text between the OSC number (`133;`) and the
/// terminator.  For example, `ESC ] 133 ; A ; aid=123 ST` yields
/// payload `"A;aid=123"`.
pub(crate) fn parse_osc133(payload: &str) -> Option<SemanticPrompt> {
    let parts: Vec<&str> = payload.split(';').collect();
    if parts.is_empty() {
        return None;
    }

    let command = parts[0];

    match command {
        "L" => {
            // FreshLine — no params allowed.
            if parts.len() == 1 {
                Some(SemanticPrompt::FreshLine)
            } else {
                None
            }
        }
        "B" => {
            // Mark end of prompt, start of input until next marker.
            if parts.len() == 1 {
                Some(SemanticPrompt::MarkEndOfPromptAndStartOfInputUntilNextMarker)
            } else {
                None
            }
        }
        "I" => {
            // Mark end of prompt, start of input until end of line.
            if parts.len() == 1 {
                Some(SemanticPrompt::MarkEndOfPromptAndStartOfInputUntilEndOfLine)
            } else {
                None
            }
        }
        "A" => {
            // Fresh line and start prompt.
            let mut aid = None;
            let mut cl = None;
            for p in &parts[1..] {
                if let Some((k, v)) = split_key_value(p) {
                    match k {
                        "aid" => aid = Some(v.to_string()),
                        "cl" => {
                            cl = Some(match v {
                                "line" => SemanticClick::Line,
                                "m" => SemanticClick::MultipleLine,
                                "v" => SemanticClick::ConservativeVertical,
                                "w" => SemanticClick::SmartVertical,
                                _ => return None,
                            });
                        }
                        _ => {} // Unknown keys are ignored per spec.
                    }
                } else {
                    return None; // Malformed param (not key=value).
                }
            }
            Some(SemanticPrompt::FreshLineAndStartPrompt { aid, cl })
        }
        "C" => {
            // Mark end of input, start of output.
            let mut aid = None;
            for p in &parts[1..] {
                if let Some((k, v)) = split_key_value(p) {
                    match k {
                        "aid" => aid = Some(v.to_string()),
                        _ => {} // Unknown keys are ignored per spec.
                    }
                } else {
                    return None;
                }
            }
            Some(SemanticPrompt::MarkEndOfInputAndStartOfOutput { aid })
        }
        "D" => {
            // Command finished with exit status.
            let status: i32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let mut aid = None;
            if parts.len() >= 2 {
                for p in &parts[2..] {
                    if let Some((k, v)) = split_key_value(p) {
                        match k {
                            "aid" => aid = Some(v.to_string()),
                            _ => {} // Unknown keys are ignored per spec.
                        }
                    }
                    // Non-key=value segments (e.g. bare "err=0") are ignored.
                }
            }
            Some(SemanticPrompt::CommandStatus { status, aid })
        }
        "N" => {
            // End of command output + fresh line + start prompt.
            let mut aid = None;
            let mut cl = None;
            for p in &parts[1..] {
                if let Some((k, v)) = split_key_value(p) {
                    match k {
                        "aid" => aid = Some(v.to_string()),
                        "cl" => {
                            cl = Some(match v {
                                "line" => SemanticClick::Line,
                                "m" => SemanticClick::MultipleLine,
                                "v" => SemanticClick::ConservativeVertical,
                                "w" => SemanticClick::SmartVertical,
                                _ => return None,
                            });
                        }
                        _ => {} // Unknown keys are ignored per spec.
                    }
                } else {
                    return None;
                }
            }
            Some(SemanticPrompt::MarkEndOfCommandWithFreshLine { aid, cl })
        }
        "P" => {
            // Start prompt with kind.
            let kind = parts
                .get(1)
                .and_then(|p| {
                    if let Some((_k, v)) = split_key_value(p) {
                        Some(v)
                    } else {
                        None
                    }
                })
                .and_then(|k| match k {
                    "i" => Some(SemanticPromptKind::Initial),
                    "r" => Some(SemanticPromptKind::RightSide),
                    "c" => Some(SemanticPromptKind::Continuation),
                    "s" => Some(SemanticPromptKind::Secondary),
                    _ => None,
                })
                .unwrap_or_default();
            // Extra params (if any) are ignored per spec.
            Some(SemanticPrompt::StartPrompt(kind))
        }
        _ => {
            // Unknown command — ignore silently.
            None
        }
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

    // ── parse_osc133 ────────────────────────────────────────────────

    #[test]
    fn osc133_fresh_line() {
        assert_eq!(parse_osc133("L"), Some(SemanticPrompt::FreshLine));
    }

    #[test]
    fn osc133_fresh_line_with_params_is_error() {
        assert_eq!(parse_osc133("L;extra"), None);
    }

    #[test]
    fn osc133_prompt_start_a() {
        assert_eq!(
            parse_osc133("A"),
            Some(SemanticPrompt::FreshLineAndStartPrompt {
                aid: None,
                cl: None,
            })
        );
    }

    #[test]
    fn osc133_prompt_start_a_with_aid() {
        assert_eq!(
            parse_osc133("A;aid=42"),
            Some(SemanticPrompt::FreshLineAndStartPrompt {
                aid: Some("42".into()),
                cl: None,
            })
        );
    }

    #[test]
    fn osc133_prompt_start_a_with_cl() {
        assert_eq!(
            parse_osc133("A;cl=w"),
            Some(SemanticPrompt::FreshLineAndStartPrompt {
                aid: None,
                cl: Some(SemanticClick::SmartVertical),
            })
        );
    }

    #[test]
    fn osc133_prompt_start_a_with_aid_and_cl() {
        assert_eq!(
            parse_osc133("A;aid=99;cl=line"),
            Some(SemanticPrompt::FreshLineAndStartPrompt {
                aid: Some("99".into()),
                cl: Some(SemanticClick::Line),
            })
        );
    }

    #[test]
    fn osc133_prompt_start_a_unknown_key_ignored() {
        assert_eq!(
            parse_osc133("A;aid=1;foo=bar"),
            Some(SemanticPrompt::FreshLineAndStartPrompt {
                aid: Some("1".into()),
                cl: None,
            })
        );
    }

    #[test]
    fn osc133_prompt_start_b() {
        assert_eq!(
            parse_osc133("B"),
            Some(SemanticPrompt::MarkEndOfPromptAndStartOfInputUntilNextMarker)
        );
    }

    #[test]
    fn osc133_prompt_start_b_with_params_is_error() {
        assert_eq!(parse_osc133("B;aid=1"), None);
    }

    #[test]
    fn osc133_input_start_c() {
        assert_eq!(
            parse_osc133("C"),
            Some(SemanticPrompt::MarkEndOfInputAndStartOfOutput { aid: None })
        );
    }

    #[test]
    fn osc133_input_start_c_with_aid() {
        assert_eq!(
            parse_osc133("C;aid=7"),
            Some(SemanticPrompt::MarkEndOfInputAndStartOfOutput {
                aid: Some("7".into()),
            })
        );
    }

    #[test]
    fn osc133_command_status_d() {
        assert_eq!(
            parse_osc133("D;0"),
            Some(SemanticPrompt::CommandStatus {
                status: 0,
                aid: None,
            })
        );
    }

    #[test]
    fn osc133_command_status_d_nonzero() {
        assert_eq!(
            parse_osc133("D;1"),
            Some(SemanticPrompt::CommandStatus {
                status: 1,
                aid: None,
            })
        );
    }

    #[test]
    fn osc133_command_status_d_with_aid() {
        assert_eq!(
            parse_osc133("D;0;aid=23"),
            Some(SemanticPrompt::CommandStatus {
                status: 0,
                aid: Some("23".into()),
            })
        );
    }

    #[test]
    fn osc133_command_status_d_with_aid_and_extra() {
        // The spec allows extra key=value pairs (like err=...) which
        // should be silently ignored.
        assert_eq!(
            parse_osc133("D;1;err=1;aid=23"),
            Some(SemanticPrompt::CommandStatus {
                status: 1,
                aid: Some("23".into()),
            })
        );
    }

    #[test]
    fn osc133_command_status_d_no_status_defaults_zero() {
        assert_eq!(
            parse_osc133("D"),
            Some(SemanticPrompt::CommandStatus {
                status: 0,
                aid: None,
            })
        );
    }

    #[test]
    fn osc133_continue_prompt_i() {
        assert_eq!(
            parse_osc133("I"),
            Some(SemanticPrompt::MarkEndOfPromptAndStartOfInputUntilEndOfLine)
        );
    }

    #[test]
    fn osc133_continue_prompt_i_with_params_is_error() {
        assert_eq!(parse_osc133("I;extra"), None);
    }

    #[test]
    fn osc133_end_of_command_n() {
        assert_eq!(
            parse_osc133("N"),
            Some(SemanticPrompt::MarkEndOfCommandWithFreshLine {
                aid: None,
                cl: None,
            })
        );
    }

    #[test]
    fn osc133_end_of_command_n_with_aid() {
        assert_eq!(
            parse_osc133("N;aid=5"),
            Some(SemanticPrompt::MarkEndOfCommandWithFreshLine {
                aid: Some("5".into()),
                cl: None,
            })
        );
    }

    #[test]
    fn osc133_end_of_command_n_with_cl() {
        assert_eq!(
            parse_osc133("N;cl=m"),
            Some(SemanticPrompt::MarkEndOfCommandWithFreshLine {
                aid: None,
                cl: Some(SemanticClick::MultipleLine),
            })
        );
    }

    #[test]
    fn osc133_start_prompt_p_initial() {
        assert_eq!(
            parse_osc133("P;k=i"),
            Some(SemanticPrompt::StartPrompt(SemanticPromptKind::Initial))
        );
    }

    #[test]
    fn osc133_start_prompt_p_right_side() {
        assert_eq!(
            parse_osc133("P;k=r"),
            Some(SemanticPrompt::StartPrompt(SemanticPromptKind::RightSide))
        );
    }

    #[test]
    fn osc133_start_prompt_p_continuation() {
        assert_eq!(
            parse_osc133("P;k=c"),
            Some(SemanticPrompt::StartPrompt(SemanticPromptKind::Continuation))
        );
    }

    #[test]
    fn osc133_start_prompt_p_secondary() {
        assert_eq!(
            parse_osc133("P;k=s"),
            Some(SemanticPrompt::StartPrompt(SemanticPromptKind::Secondary))
        );
    }

    #[test]
    fn osc133_start_prompt_p_default_initial() {
        // P without k= defaults to Initial.
        assert_eq!(
            parse_osc133("P"),
            Some(SemanticPrompt::StartPrompt(SemanticPromptKind::Initial))
        );
    }

    #[test]
    fn osc133_start_prompt_p_unknown_kind_defaults_initial() {
        assert_eq!(
            parse_osc133("P;k=z"),
            Some(SemanticPrompt::StartPrompt(SemanticPromptKind::Initial))
        );
    }

    #[test]
    fn osc133_unknown_command_is_none() {
        assert_eq!(parse_osc133("Z"), None);
        assert_eq!(parse_osc133("Q"), None);
        assert_eq!(parse_osc133(""), None);
    }

    #[test]
    fn osc133_all_cl_variants() {
        assert_eq!(
            parse_osc133("A;cl=line").unwrap(),
            SemanticPrompt::FreshLineAndStartPrompt {
                aid: None,
                cl: Some(SemanticClick::Line),
            }
        );
        assert_eq!(
            parse_osc133("A;cl=m").unwrap(),
            SemanticPrompt::FreshLineAndStartPrompt {
                aid: None,
                cl: Some(SemanticClick::MultipleLine),
            }
        );
        assert_eq!(
            parse_osc133("A;cl=v").unwrap(),
            SemanticPrompt::FreshLineAndStartPrompt {
                aid: None,
                cl: Some(SemanticClick::ConservativeVertical),
            }
        );
        assert_eq!(
            parse_osc133("A;cl=w").unwrap(),
            SemanticPrompt::FreshLineAndStartPrompt {
                aid: None,
                cl: Some(SemanticClick::SmartVertical),
            }
        );
    }

    #[test]
    fn osc133_invalid_cl_is_none() {
        assert_eq!(parse_osc133("A;cl=bad"), None);
    }
}
