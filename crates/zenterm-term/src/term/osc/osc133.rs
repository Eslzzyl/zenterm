use zenterm_core::{SemanticClick, SemanticPrompt, SemanticPromptKind};

use super::util::split_key_value;

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
