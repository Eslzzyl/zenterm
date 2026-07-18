//! Badge template renderer for iTerm2 `OSC 1337 ; SetBadgeFormat`.
//!
//! The badge format string is a template with `\(variable)` placeholders
//! that are replaced with session state values at render time.
//!
//! Supported variables:
//! - `session.name`          — session title
//! - `session.path`          — current working directory (basename)
//! - `session.hostname`      — machine hostname
//! - `session.terminalName`  — "Zenterm"
//! - `user.<name>`           — user-defined variable (from SetUserVar)

use super::types::TerminalSession;

/// Render a badge template string by resolving `\(variable)` placeholders
/// against the session's current state.
///
/// Returns the resolved text, or an empty string if the template is empty.
pub fn render_badge(template: &str, session: &TerminalSession) -> String {
    if template.is_empty() {
        return String::new();
    }

    let mut result = String::with_capacity(template.len());
    let mut chars = template.char_indices().peekable();

    while let Some((_, ch)) = chars.next() {
        if ch == '\\' {
            // Check for \( — escaped parenthesis.
            match chars.peek() {
                Some(&(_, '(')) => {
                    chars.next(); // consume '('
                    // Collect the variable name up to ')'.
                    let mut var_name = String::new();
                    loop {
                        match chars.next() {
                            Some((_, ')')) => break,
                            Some((_, c)) => var_name.push(c),
                            None => {
                                // Unterminated — emit as-is up to what we consumed.
                                result.push('\\');
                                result.push('(');
                                result.push_str(&var_name);
                                return result;
                            }
                        }
                    }
                    // Resolve the variable.
                    let resolved = resolve_var(&var_name, session);
                    result.push_str(&resolved);
                }
                Some(&(_, c)) if c == '\\' => {
                    // Escaped backslash — emit single backslash.
                    chars.next();
                    result.push('\\');
                }
                _ => {
                    // Lone backslash — emit as-is.
                    result.push('\\');
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Resolve a single `\(variable)` name against session state.
fn resolve_var(name: &str, session: &TerminalSession) -> String {
    match name {
        "session.name" => session.title.clone(),
        "session.terminalName" => "Zenterm".into(),
        "session.path" => session
            .cwd
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_default(),
        "session.hostname" => {
            // Try env vars first, fall back to command.
            let from_env = std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("HOST"))
                .ok();
            if let Some(host) = from_env {
                host
            } else {
                std::process::Command::new("hostname")
                    .output()
                    .ok()
                    .and_then(|o| {
                        if o.status.success() {
                            String::from_utf8(o.stdout)
                                .ok()
                                .map(|s| s.trim().to_string())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default()
            }
        }
        _ => {
            // Check for user.<varname>
            if let Some(rest) = name.strip_prefix("user.") {
                session
                    .terminal
                    .user_vars()
                    .get(rest)
                    .cloned()
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }
    }
}
