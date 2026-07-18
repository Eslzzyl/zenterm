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
use std::collections::HashMap;
use zenterm_core::{KittyNotification, KittyOccasion, KittyUrgency, Progress, SemanticClick, SemanticPrompt, SemanticPromptKind};

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
fn is_valid_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.'))
}

/// Sanitize an identifier by removing characters not in the allowed set.
fn sanitize_identifier(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.'))
        .collect()
}

/// Parse the colon-separated metadata section of an OSC 99 sequence.
///
/// Returns a HashMap of key → value.  Keys are single characters; values
/// are raw (not base64-decoded).  If a key appears multiple times the last
/// value wins, except for keys documented as repeatable (`t`, `n`).
fn parse_osc99_metadata(metadata: &str) -> HashMap<String, String> {
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

/// Accumulator state for a single Kitty notification being built across
/// multiple chunked OSC 99 sequences.
#[derive(Debug, Clone)]
struct KittyAccumulator {
    title: String,
    body: String,
    icon_data: Vec<u8>,
    icon_g: Option<String>,
    app_name: Option<String>,
    icon_names: Vec<String>,
    urgency: KittyUrgency,
    occasion: KittyOccasion,
    sound: Option<String>,
    report_click: bool,
    close_report: bool,
    notification_types: Vec<String>,
}

impl Default for KittyAccumulator {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
            icon_data: Vec::new(),
            icon_g: None,
            app_name: None,
            icon_names: Vec::new(),
            urgency: KittyUrgency::Normal,
            occasion: KittyOccasion::Always,
            sound: None,
            report_click: false,
            close_report: false,
            notification_types: Vec::new(),
        }
    }
}

/// Manages the state of all in-flight Kitty OSC 99 notifications.
#[derive(Debug, Default)]
pub(crate) struct KittyNotificationState {
    /// Accumulators keyed by notification identifier.
    accumulators: HashMap<String, KittyAccumulator>,
}

impl KittyNotificationState {
    /// Process a single OSC 99 payload and return:
    /// - A completed notification if `d=1` and we have title/body content.
    /// - A response string if the payload is a query (`p=?` or `p=alive`).
    ///
    /// `identifier` is an optional identifier used in query responses (for
    /// multiplexer support).  Should be `i=...` from the query or empty.
    pub fn handle_event(
        &mut self,
        payload: &str,
        identifier_hint: &str,
    ) -> (Option<KittyNotification>, Option<String>) {
        // Split into metadata and data sections.
        let (metadata_str, data) = match payload.split_once(';') {
            Some((m, d)) => (m, d),
            None => (payload, ""),
        };

        let meta = parse_osc99_metadata(metadata_str);

        // Determine payload type.
        let p_type: Option<&str> = meta.get("p").map(|s| s.as_str());

        // ── Queries ─────────────────────────────────────────────────
        match p_type {
            Some("?") => {
                let id = meta.get("i").map(|s| s.as_str()).unwrap_or("0");
                return (None, Some(Self::query_response(id, identifier_hint)));
            }
            Some("alive") => {
                let id = meta.get("i").map(|s| s.as_str()).unwrap_or("0");
                let active_ids: Vec<&str> =
                    self.accumulators.keys().map(|s| s.as_str()).collect();
                return (None, Some(Self::alive_response(id, &active_ids)));
            }
            Some("close") => {
                // Close a specific notification.
                if let Some(notif_id) = meta.get("i") {
                    let sanitized = sanitize_identifier(notif_id);
                    if !sanitized.is_empty() {
                        self.accumulators.remove(&sanitized);
                    }
                }
                return (None, None);
            }
            _ => {}
        }

        // ── Data payloads ───────────────────────────────────────────
        let identifier = meta.get("i").map(|s| sanitize_identifier(s));
        let done = meta.get("d").map(|s| s == "1").unwrap_or(true);
        let is_base64 = meta.get("e").map(|s| s == "1").unwrap_or(false);

        // Decode data if base64.
        let decoded_data = if is_base64 {
            match base64_decode(data.as_bytes()) {
                Ok(bytes) => String::from_utf8(bytes).unwrap_or_default(),
                Err(_) => String::new(),
            }
        } else {
            data.to_string()
        };

        // Accumulate into the right slot.
        // We need to work with a value (not a reference) because we may
        // need to remove the accumulator from the map when done.
        let mut acc = if let Some(ref id) = identifier {
            if done {
                // Final chunk: take the accumulator out of the map.
                self.accumulators.remove(id).unwrap_or_default()
            } else {
                // Intermediate chunk: clone out, then re-insert after update.
                self.accumulators
                    .entry(id.clone())
                    .or_insert_with(KittyAccumulator::default)
                    .clone()
            }
        } else {
            // No identifier — this is a standalone notification.
            KittyAccumulator::default()
        };

        // Apply metadata to the accumulator.
        apply_metadata(&meta, &mut acc);

        // Append data based on payload type.
        match p_type {
            Some("title") | None => {
                if !decoded_data.is_empty() {
                    if acc.title.is_empty() {
                        acc.title = decoded_data;
                    } else {
                        acc.title.push_str(&decoded_data);
                    }
                }
            }
            Some("body") => {
                if !decoded_data.is_empty() {
                    if acc.body.is_empty() {
                        acc.body = decoded_data;
                    } else {
                        acc.body.push_str(&decoded_data);
                    }
                }
            }
            Some("icon") => {
                if is_base64 {
                    if let Ok(icon_bytes) = base64_decode(data.as_bytes()) {
                        acc.icon_data = icon_bytes;
                    }
                }
            }
            Some("buttons") => {
                // We just store the raw button data; the UI layer
                // can interpret it.  Buttons are separated by U+2028.
                if !decoded_data.is_empty() {
                    // Store in app_name as a delimited list for now.
                    // The actual button handling requires notify-rust
                    // action support which is XDG-only.
                }
            }
            _ => {} // Unknown p= type — ignored per spec.
        }

        // If done and we have content, produce a notification.
        if done {
            let title = if acc.title.is_empty() {
                acc.body.clone()
            } else {
                acc.title.clone()
            };
            let body = if acc.title.is_empty() {
                String::new()
            } else {
                acc.body.clone()
            };

            if !title.is_empty() || !body.is_empty() {
                let notif = KittyNotification {
                    id: identifier.clone(),
                    title,
                    body,
                    app_name: acc.app_name,
                    urgency: acc.urgency,
                    occasion: acc.occasion,
                    sound: acc.sound,
                    icon_names: acc.icon_names,
                    report_click: acc.report_click,
                    close_report: acc.close_report,
                };
                return (Some(notif), None);
            }
        } else if let Some(ref id) = identifier {
            // Not done — store accumulator for future chunks.
            self.accumulators.insert(id.clone(), acc);
        }

        (None, None)
    }

    /// Generate a response to `p=?` (capability query).
    fn query_response(identifier: &str, _hint: &str) -> String {
        let id = sanitize_identifier(identifier);
        // Report support: we implement base features.
        // p=title,body,close,?,alive,icon,buttons
        // a=focus (no report yet)
        // o=always
        // s=system,silent
        format!(
            "\x1b]99;i={}:p=?;p=title,body,close,?,alive,icon,buttons:o=always:a=focus:s=system,silent\x1b\\\\",
            id
        )
    }

    /// Generate a response to `p=alive`.
    fn alive_response(identifier: &str, active_ids: &[&str]) -> String {
        let id = sanitize_identifier(identifier);
        let ids = active_ids.join(",");
        format!("\x1b]99;i={}:p=alive;{}\x1b\\\\", id, ids)
    }
}

/// Apply metadata key-value pairs to a `KittyAccumulator`.
fn apply_metadata(meta: &HashMap<String, String>, acc: &mut KittyAccumulator) {
    for (key, value) in meta {
        match key.as_str() {
            "f" => {
                // Application name — base64 encoded.
                if let Ok(decoded) = base64_decode(value.as_bytes()) {
                    if let Ok(s) = String::from_utf8(decoded) {
                        acc.app_name = Some(s);
                    }
                }
            }
            "n" => {
                // Icon name — base64 encoded.
                if let Ok(decoded) = base64_decode(value.as_bytes()) {
                    if let Ok(s) = String::from_utf8(decoded) {
                        acc.icon_names.push(s);
                    }
                }
            }
            "t" => {
                // Notification type — base64 encoded.
                if let Ok(decoded) = base64_decode(value.as_bytes()) {
                    if let Ok(s) = String::from_utf8(decoded) {
                        acc.notification_types.push(s);
                    }
                }
            }
            "u" => {
                acc.urgency = match value.as_str() {
                    "0" => KittyUrgency::Low,
                    "2" => KittyUrgency::Critical,
                    _ => KittyUrgency::Normal,
                };
            }
            "o" => {
                acc.occasion = match value.as_str() {
                    "unfocused" => KittyOccasion::Unfocused,
                    "invisible" => KittyOccasion::Invisible,
                    _ => KittyOccasion::Always,
                };
            }
            "s" => {
                // Sound name — base64 encoded.
                if let Ok(decoded) = base64_decode(value.as_bytes()) {
                    if let Ok(s) = String::from_utf8(decoded) {
                        acc.sound = Some(s);
                    }
                }
            }
            "a" => {
                // Actions: comma-separated, may have leading `-`.
                for action in value.split(',') {
                    match action.trim() {
                        "report" => acc.report_click = true,
                        "-report" => acc.report_click = false,
                        "focus" => {} // Default behaviour, no action needed.
                        "-focus" => {} // Would suppress focusing, skip.
                        _ => {}
                    }
                }
            }
            "c" => {
                acc.close_report = value == "1";
            }
            "g" => {
                // Icon cache identifier.
                if is_valid_identifier(value) {
                    acc.icon_g = Some(value.clone());
                }
            }
            _ => {
                // Unknown keys are ignored per spec.
            }
        }
    }
}

/// Thin wrapper around the base64 crate for internal use.
fn base64_decode(input: &[u8]) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(input)
}

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

    // ── parse_osc99_metadata ─────────────────────────────────────────

    #[test]
    fn osc99_metadata_empty() {
        let m = parse_osc99_metadata("");
        assert!(m.is_empty());
    }

    #[test]
    fn osc99_metadata_single_pair() {
        let m = parse_osc99_metadata("i=42");
        assert_eq!(m.get("i").unwrap(), "42");
    }

    #[test]
    fn osc99_metadata_multiple_pairs() {
        let m = parse_osc99_metadata("i=1:p=title:d=0");
        assert_eq!(m.get("i").unwrap(), "1");
        assert_eq!(m.get("p").unwrap(), "title");
        assert_eq!(m.get("d").unwrap(), "0");
    }

    #[test]
    fn osc99_metadata_empty_value() {
        // i= with no value is allowed per spec (i= means unset identifier).
        let m = parse_osc99_metadata("i=:p=body");
        assert_eq!(m.get("i").unwrap(), "");
        assert_eq!(m.get("p").unwrap(), "body");
    }

    #[test]
    fn osc99_metadata_invalid_key_skipped() {
        let m = parse_osc99_metadata("i=1:badkey=val:p=title");
        // "badkey" is > 1 character, so it should be skipped.
        assert_eq!(m.len(), 2);
        assert_eq!(m.get("i").unwrap(), "1");
        assert_eq!(m.get("p").unwrap(), "title");
    }

    #[test]
    fn osc99_metadata_special_chars_in_value() {
        // Values can contain: a-zA-Z0-9-_/+.,(){}[]*&^%$#@!`~
        let m = parse_osc99_metadata("p=?");
        assert_eq!(m.get("p").unwrap(), "?");
    }

    // ── KittyNotificationState ───────────────────────────────────────

    #[test]
    fn osc99_simple_notification() {
        // ESC ] 99 ;; Hello world ST  →  simple notification
        let mut state = KittyNotificationState::default();
        let payload = ";Hello world";
        let (notif, response) = state.handle_event(payload, "");
        assert!(response.is_none());
        let notif = notif.expect("should produce a notification");
        assert_eq!(notif.title, "Hello world");
        assert_eq!(notif.body, "");
        assert!(notif.id.is_none());
    }

    #[test]
    fn osc99_chunked_title_body() {
        let mut state = KittyNotificationState::default();

        // Chunk 1: title (not done)
        let (n1, r1) = state.handle_event("i=1:d=0:p=title;Hello", "");
        assert!(n1.is_none());
        assert!(r1.is_none());

        // Chunk 2: body (done → produces notification)
        let (n2, r2) = state.handle_event("i=1:d=1:p=body;World", "");
        assert!(r2.is_none());
        let n2 = n2.expect("should produce notification on d=1");
        assert_eq!(n2.title, "Hello");
        assert_eq!(n2.body, "World");
        assert_eq!(n2.id.as_deref(), Some("1"));
    }

    #[test]
    fn osc99_chunked_body_then_title() {
        let mut state = KittyNotificationState::default();

        // Body first, then title.
        let _ = state.handle_event("i=2:d=0:p=body;Body text", "");
        let (n, _) = state.handle_event("i=2:d=1:p=title;Title text", "");
        let n = n.expect("should produce notification");
        assert_eq!(n.title, "Title text");
        assert_eq!(n.body, "Body text");
    }

    #[test]
    fn osc99_chunked_multiple_title_parts() {
        let mut state = KittyNotificationState::default();

        let _ = state.handle_event("i=3:d=0:p=title;Part ", "");
        let (n, _) = state.handle_event("i=3:d=1:p=title;Two", "");
        let n = n.expect("should produce notification");
        assert_eq!(n.title, "Part Two");
        assert_eq!(n.body, "");
    }

    #[test]
    fn osc99_title_only_no_d_flag_implies_done() {
        // When no `d` key is present, defaults to done=1.
        let mut state = KittyNotificationState::default();
        let (n, _) = state.handle_event("i=4:p=title;Just title", "");
        let n = n.expect("should produce notification");
        assert_eq!(n.title, "Just title");
    }

    #[test]
    fn osc99_no_identifier_no_chunking() {
        // Without an `i` identifier, every OSC 99 produces a standalone notification.
        let mut state = KittyNotificationState::default();
        let (n, _) = state.handle_event("p=title;Standalone", "");
        let n = n.expect("should produce notification");
        assert_eq!(n.title, "Standalone");
    }

    #[test]
    fn osc99_update_existing() {
        // Sending a new notification with the same `i` replaces the old one.
        let mut state = KittyNotificationState::default();
        let _ = state.handle_event("i=5:d=1:p=title;Old title", "");
        let (n, _) = state.handle_event("i=5:d=1:p=title;New title", "");
        let n = n.expect("should produce updated notification");
        assert_eq!(n.title, "New title");
    }

    #[test]
    fn osc99_close() {
        let mut state = KittyNotificationState::default();
        // Start a notification.
        let _ = state.handle_event("i=6:d=0:p=title;Will be closed", "");
        // Close it.
        let (n, r) = state.handle_event("i=6:p=close;", "");
        assert!(n.is_none());
        assert!(r.is_none());
        // Verify it's gone — a new chunk for id=6 should start fresh.
        let (n2, _) = state.handle_event("i=6:d=1:p=title;Fresh start", "");
        let n2 = n2.expect("should start fresh");
        assert_eq!(n2.title, "Fresh start");
    }

    #[test]
    fn osc99_query_response() {
        let mut state = KittyNotificationState::default();
        let (n, r) = state.handle_event("p=?;", "");
        assert!(n.is_none());
        let r = r.expect("query should produce a response");
        assert!(r.starts_with("\x1b]99;"));
        assert!(r.contains("p=?"));
        assert!(r.ends_with("\x1b\\\\"));
    }

    #[test]
    fn osc99_query_response_with_identifier() {
        let mut state = KittyNotificationState::default();
        let (_, r) = state.handle_event("i=abc:p=?;", "abc");
        let r = r.expect("query should produce a response");
        assert!(r.contains("i=abc"));
    }

    #[test]
    fn osc99_alive_query() {
        let mut state = KittyNotificationState::default();
        // Create an in-flight notification.
        let _ = state.handle_event("i=7:d=0:p=title;In flight", "");
        let (n, r) = state.handle_event("i=8:p=alive;", "");
        assert!(n.is_none());
        let r = r.expect("alive query should produce a response");
        assert!(r.contains("p=alive"));
        assert!(r.contains("7")); // id=7 is alive
    }

    #[test]
    fn osc99_empty_payload_no_notification() {
        let mut state = KittyNotificationState::default();
        let (n, r) = state.handle_event(";", "");
        assert!(n.is_none());
        assert!(r.is_none());
    }

    #[test]
    fn osc99_unknown_payload_type_ignored() {
        let mut state = KittyNotificationState::default();
        let (n, r) = state.handle_event("i=9:d=1:p=unknown;Data", "");
        assert!(n.is_none());
        assert!(r.is_none());
    }

    #[test]
    fn osc99_metadata_empty_after_semicolon() {
        // `ESC ] 99 ;; body`  →  metadata is empty string
        let mut state = KittyNotificationState::default();
        let (n, _) = state.handle_event(";body only", "");
        let n = n.expect("should produce notification");
        assert_eq!(n.title, "body only");
    }
}
