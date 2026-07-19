use std::collections::HashMap;

use zenterm_core::{KittyNotification, KittyOccasion, KittyUrgency};

use super::conemu::{parse_osc99_metadata, sanitize_identifier};
use super::util::base64_decode;

#[derive(Debug, Clone)]
pub(super) struct KittyAccumulator {
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
    buttons: Vec<String>,
    timeout_ms: i32,
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
            buttons: Vec::new(),
            timeout_ms: -1,
        }
    }
}

use super::conemu::is_valid_identifier;

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
            "w" => {
                // Auto-close timeout: -1 = default, 0 = never, >0 = ms.
                acc.timeout_ms = value.parse::<i32>().unwrap_or(-1);
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

/// Manages the state of all in-flight Kitty OSC 99 notifications
/// and the icon data cache.
#[derive(Debug, Default)]
pub(crate) struct KittyNotificationState {
    /// Accumulators keyed by notification identifier.
    accumulators: HashMap<String, KittyAccumulator>,
    /// Cached icon data keyed by `g` identifier.
    icon_cache: HashMap<String, Vec<u8>>,
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
                        // Cache under `g` key if provided.
                        if let Some(ref g) = acc.icon_g {
                            if !g.is_empty() {
                                self.icon_cache.insert(g.clone(), acc.icon_data.clone());
                            }
                        }
                    }
                }
            }
            Some("buttons") => {
                // Buttons are separated by U+2028 (LINE SEPARATOR).
                if !decoded_data.is_empty() {
                    acc.buttons = decoded_data
                        .split('\u{2028}')
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
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
                // Look up cached icon data if the notification references
                // a cache key but has no direct icon_data.
                let icon_data = if acc.icon_data.is_empty() {
                    acc.icon_g
                        .as_ref()
                        .and_then(|g| self.icon_cache.get(g).cloned())
                        .unwrap_or_default()
                } else {
                    acc.icon_data.clone()
                };

                let notif = KittyNotification {
                    id: identifier.clone(),
                    title,
                    body,
                    app_name: acc.app_name,
                    urgency: acc.urgency,
                    occasion: acc.occasion,
                    sound: acc.sound,
                    icon_names: acc.icon_names,
                    icon_data,
                    icon_cache_key: acc.icon_g,
                    notification_types: acc.notification_types,
                    buttons: acc.buttons,
                    timeout_ms: acc.timeout_ms,
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
        // Report the features we actually implement.
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
