use std::collections::HashMap;

use zenterm_core::Progress;

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

// ─── OSC-99 / Kitty desktop notification ─────────────────────────

/// The character set allowed in OSC 99 metadata values per the Kitty spec.
/// Characters: a-zA-Z0-9-_/+.,(){}[]*&^%$#@!`~
fn is_valid_osc99_value_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '?' | '-' | '_' | '/' | '+' | '.' | ',' | '(' | ')' | '{' | '}' | '[' | ']' | '*' | '&' | '^' | '%' | '$' | '#' | '@' | '!' | '`' | '~')
}

/// The character set allowed in OSC 99 metadata keys (single a-zA-Z).
fn is_valid_osc99_key_char(c: char) -> bool {
    c.is_ascii_alphabetic()
}

/// An identifier in the OSC 99 protocol: `[a-zA-Z0-9_-+.]` characters only.
pub(super) fn is_valid_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.'))
}

/// Sanitize an identifier by removing characters not in the allowed set.
pub(super) fn sanitize_identifier(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.'))
        .collect()
}

/// Parse the colon-separated metadata section of an OSC 99 sequence.
///
/// Returns a HashMap of key → value.  Keys are single characters; values
/// are raw (not base64-decoded).  If a key appears multiple times the last
/// value wins, except for keys documented as repeatable (`t`, `n`).
pub(super) fn parse_osc99_metadata(metadata: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in metadata.split(':') {
        if pair.is_empty() {
            continue;
        }
        if let Some(eq_pos) = pair.find('=') {
            let key = &pair[..eq_pos];
            let value = &pair[eq_pos + 1..];
            // Validate key: single a-zA-Z character.
            if key.len() == 1 && is_valid_osc99_key_char(key.chars().next().unwrap()) {
                // Validate value contains only allowed characters.
                if value.is_empty() || value.chars().all(|c| is_valid_osc99_value_char(c) || c == '=') {
                    map.insert(key.to_string(), value.to_string());
                }
            }
        }
    }
    map
}
