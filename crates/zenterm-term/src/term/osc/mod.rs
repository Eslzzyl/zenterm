//! Unified OSC scanner.
//!
//! Scans raw PTY output for all `ESC ] <number> ; <payload> (BEL|ST)`
//! sequences in a single pass and returns structured matches for the
//! caller to dispatch by OSC number.
//!
//! This replaces the previous ad-hoc scanners (`scan_osc7`, `scan_osc9_or_777`)
//! with a single unified entry point.  Adding a new OSC handler requires
//! only a new dispatch arm — no new byte-scanning logic.

use memchr::{memchr, memchr2};

mod conemu;
mod kitty_notify;
mod util;
mod osc133;
mod iterm;
#[cfg(test)]
mod tests;

pub(crate) use conemu::parse_conemu_progress;
pub(crate) use kitty_notify::KittyNotificationState;
pub(crate) use util::base64_encode_for_response;
pub(crate) use osc133::parse_osc133;
pub(crate) use iterm::parse_iterm_proprietary;

/// A single OSC match found in the byte stream.
#[derive(Debug, Clone)]
pub(crate) struct OscMatch {
    /// The OSC number (e.g. 7, 9, 777).
    pub number: u16,
    /// Payload between the number and the terminator (BEL/ST).
    /// This is the raw UTF-8 content after the first `;`.
    pub payload: String,
    /// Byte offset of the `ESC` (0x1b) that starts this OSC (`ESC ] …`).
    pub byte_start: usize,
    /// Byte offset after the terminator (one past the `BEL` or `ST`).
    pub byte_end: usize,
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
        // Use SIMD-accelerated memchr to find the next ESC byte.
        let esc_offset = match memchr(0x1B, &bytes[i..]) {
            Some(off) => off,
            None => break,
        };
        i += esc_offset;

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
                    let byte_start = i;
                    let terminator_len = match tail[end] {
                        0x07 => 1,      // BEL
                        _ => 2,         // ST (ESC \)
                    };
                    let byte_end = payload_start + end + terminator_len;
                    results.push(OscMatch {
                        number,
                        payload: payload.to_string(),
                        byte_start,
                        byte_end,
                    });
                    // Advance past the entire OSC sequence.
                    i = byte_end;
                } else {
                    i = payload_start + end + match tail[end] {
                        0x07 => 1,
                        _ => 2,
                    };
                }
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
