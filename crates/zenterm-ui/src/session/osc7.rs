//! OSC 7 working-directory URL helpers.
//!
//! Parses `file://host/path` URLs emitted by shells (zsh, fish, etc.)
//! and converts them to filesystem [`PathBuf`] values.

use std::path::PathBuf;

/// Convert an OSC 7 URL (`file://host/path` or `/abs/path`) to a
/// filesystem [`PathBuf`].  Returns `None` on parse failure.
pub(crate) fn osc7_url_to_path(url: &str) -> Option<PathBuf> {
    if let Some(stripped) = url.strip_prefix("file://") {
        // Strip the host component (e.g. `file://localhost/...`).
        if let Some(slash_pos) = stripped.find('/') {
            let after_host = &stripped[slash_pos..];
            return Some(PathBuf::from(percent_decode(after_host)));
        }
        return None;
    }
    if url.starts_with('/') {
        return Some(PathBuf::from(url));
    }
    None
}

/// Minimal percent-decode for OSC 7 paths.
///
/// Only decodes `%2F` → `/` and `%20` → ` `, which is all that shells
/// typically emit.  A full implementation would handle every `%XX`
/// sequence but this keeps the common case fast.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = bytes.next().and_then(|c| hex_val(c));
            let lo = bytes.next().and_then(|c| hex_val(c));
            match (hi, lo) {
                (Some(h), Some(l)) => out.push((h << 4 | l) as char),
                _ => out.push('%'),
            }
        } else {
            out.push(b as char);
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
