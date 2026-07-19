use zenterm_core::{ITermDimension, ITermFileData, ITermProprietary, ITermUnicodeVersionOp};

use super::util::base64_decode;

// ─── OSC-1337 / iTerm2 proprietary ────────────────────────────────

/// Parse an OSC 1337 (iTerm2 proprietary) payload.
///
/// The payload is the text between `1337;` and the terminator.
///
/// Reference: wezterm/wezterm-escape-parser/src/osc.rs `ITermProprietary::parse`
/// Spec: <https://iterm2.com/documentation-escape-codes.html>
pub(crate) fn parse_iterm_proprietary(payload: &str) -> Option<ITermProprietary> {
    let parts: Vec<&str> = payload.split(';').collect();
    if parts.is_empty() {
        return None;
    }

    let param = parts[0];
    let mut iter = param.splitn(2, '=');
    let keyword = iter.next()?;
    let p1 = iter.next();

    // ── Macros matching WezTerm's single!/one_str!/const_arg! ──────
    macro_rules! single {
        ($variant:ident, $text:expr) => {
            if parts.len() == 1 && keyword == $text && p1.is_none() {
                return Some(ITermProprietary::$variant);
            }
        };
    }
    macro_rules! one_str {
        ($variant:ident, $text:expr) => {
            if parts.len() == 1 && keyword == $text {
                if let Some(v) = p1 {
                    return Some(ITermProprietary::$variant(v.to_string()));
                }
            }
        };
    }
    macro_rules! const_arg {
        ($variant:ident, $text:expr, $value:expr, $res:expr) => {
            if parts.len() == 1 && keyword == $text {
                if let Some(v) = p1 {
                    if v == $value {
                        return Some(ITermProprietary::$variant($res));
                    }
                }
            }
        };
    }

    // ── Simple keyword-only commands ───────────────────────────────
    single!(SetMark, "SetMark");
    single!(StealFocus, "StealFocus");
    single!(ClearScrollback, "ClearScrollback");
    // CopyToClipboard / EndCopy: explicitly skipped per user agreement.

    // ── Boolean toggle ─────────────────────────────────────────────
    const_arg!(HighlightCursorLine, "HighlightCursorLine", "yes", true);
    const_arg!(HighlightCursorLine, "HighlightCursorLine", "no", false);

    // ── Single-string-value commands ───────────────────────────────
    one_str!(CurrentDir, "CurrentDir");
    one_str!(SetProfile, "SetProfile");

    // ── ReportCellSize: no `=` → RequestCellSize ───────────────────
    if parts.len() == 1 && keyword == "ReportCellSize" && p1.is_none() {
        return Some(ITermProprietary::RequestCellSize);
    }

    // ── ReportCellSize=<h>;<w>[;<scale>] (response) ────────────
    if keyword == "ReportCellSize" && p1.is_some() && parts.len() >= 2 {
        let h = p1?.parse::<f32>().ok()?;
        let w = parts[1].parse::<f32>().ok()?;
        let scale = parts.get(2).and_then(|s| s.parse::<f32>().ok());
        return Some(ITermProprietary::ReportCellSize {
            height_pixels: h,
            width_pixels: w,
            scale,
        });
    }

    let p1_empty = match p1 {
        Some(v) if v.is_empty() => true,
        None => true,
        _ => false,
    };

    // ── Copy=;base64data ───────────────────────────────────────────
    if parts.len() >= 2 && keyword == "Copy" && p1_empty {
        let raw = base64_decode(parts[1].as_bytes()).ok()?;
        let text = String::from_utf8(raw).ok()?;
        return Some(ITermProprietary::Copy(text));
    }

    // ── SetBadgeFormat=;base64data ─────────────────────────────────
    if parts.len() >= 2 && keyword == "SetBadgeFormat" && p1_empty {
        let raw = base64_decode(parts[1].as_bytes()).ok()?;
        let text = String::from_utf8(raw).ok()?;
        return Some(ITermProprietary::SetBadgeFormat(text));
    }

    // ── ReportVariable=base64name ──────────────────────────────────
    if parts.len() == 1 && keyword == "ReportVariable" {
        if let Some(v) = p1 {
            let name = String::from_utf8(base64_decode(v.as_bytes()).ok()?).ok()?;
            return Some(ITermProprietary::ReportVariable(name));
        }
    }

    // ── SetUserVar=name=base64value ────────────────────────────────
    if parts.len() == 1 && keyword == "SetUserVar" {
        if let Some(v) = p1 {
            let mut inner = v.splitn(2, '=');
            let name = inner.next()?;
            let b64_value = inner.next()?;
            let value = String::from_utf8(base64_decode(b64_value.as_bytes()).ok()?).ok()?;
            return Some(ITermProprietary::SetUserVar {
                name: name.to_string(),
                value,
            });
        }
    }

    // ── UnicodeVersion=N / push [label] / pop [label] ─────────────
    if parts.len() == 1 && keyword == "UnicodeVersion" {
        if let Some(v) = p1 {
            let mut inner = v.splitn(2, ' ');
            let op = inner.next();
            let label = inner.next().map(String::from);
            match op {
                Some("push") => {
                    return Some(ITermProprietary::UnicodeVersion(
                        ITermUnicodeVersionOp::Push(label),
                    ));
                }
                Some("pop") => {
                    return Some(ITermProprietary::UnicodeVersion(
                        ITermUnicodeVersionOp::Pop(label),
                    ));
                }
                _ => {
                    if let Ok(n) = v.parse::<u8>() {
                        return Some(ITermProprietary::UnicodeVersion(
                            ITermUnicodeVersionOp::Set(n),
                        ));
                    }
                }
            }
        }
    }

    // ── File=...:base64data ────────────────────────────────────────
    if keyword == "File" {
        return parse_iterm_file(&parts);
    }

    None
}

/// Parse the `File` sub-protocol payload parts into `ITermProprietary::File`.
///
/// `parts` is the payload split by `;`.  The first part begins with `File=…`.
/// Subsequent parts contain key=value metadata.  The final part includes a
/// `:` that separates the trailing argument from the base64-encoded data.
///
/// Reference: wezterm `ITermFileData::parse`
fn parse_iterm_file(parts: &[&str]) -> Option<ITermProprietary> {
    use std::collections::HashMap;
    let mut params = HashMap::new();
    let mut data = None;
    let last = parts.len() - 1;

    for (idx, s) in parts.iter().enumerate() {
        let param = if idx == 0 {
            // First element: strip "File=" prefix.
            if s.len() >= 5 { &s[5..] } else { return None; }
        } else {
            s
        };

        if idx == last {
            // Last element: split on `:` to extract base64 data.
            let colon = param.find(':')?;
            data = Some(base64_decode(param[colon + 1..].as_bytes()).ok()?);
            let args = &param[..colon];
            if !args.is_empty() {
                insert_file_param(args, &mut params);
            }
        } else {
            insert_file_param(param, &mut params);
        }
    }

    let name = params
        .get("name")
        .and_then(|s| base64_decode(s.as_bytes()).ok())
        .and_then(|b| String::from_utf8(b).ok());
    let size = params.get("size").and_then(|s| s.parse().ok());
    let width = params
        .get("width")
        .and_then(|s| parse_iterm_dimension(s))
        .unwrap_or(ITermDimension::Automatic);
    let height = params
        .get("height")
        .and_then(|s| parse_iterm_dimension(s))
        .unwrap_or(ITermDimension::Automatic);
    let preserve_aspect_ratio = params
        .get("preserveAspectRatio")
        .map(|s| s != "0")
        .unwrap_or(true);
    let inline_val = params.get("inline").map(|s| s != "0").unwrap_or(false);
    let do_not_move_cursor = params
        .get("doNotMoveCursor")
        .map(|s| s != "0")
        .unwrap_or(false);

    Some(ITermProprietary::File(ITermFileData {
        name,
        size,
        width,
        height,
        preserve_aspect_ratio,
        inline: inline_val,
        do_not_move_cursor,
        data: data?,
    }))
}

/// Insert a `key=value` pair from a file parameter string into `params`.
fn insert_file_param(s: &str, params: &mut std::collections::HashMap<String, String>) {
    if let Some(eq) = s.find('=') {
        params.insert(s[..eq].to_string(), s[eq + 1..].to_string());
    }
}

/// Parse an iTerm2 dimension string (e.g. `"auto"`, `"10"`, `"100px"`, `"50%"`, `"8c"`).
pub(super) fn parse_iterm_dimension(s: &str) -> Option<ITermDimension> {
    if s == "auto" {
        return Some(ITermDimension::Automatic);
    }
    if let Some(rest) = s.strip_suffix("px") {
        return rest.parse::<isize>().ok().map(ITermDimension::Pixels);
    }
    if let Some(rest) = s.strip_suffix('%') {
        return rest.parse::<isize>().ok().map(ITermDimension::Percent);
    }
    if let Some(rest) = s.strip_suffix('c') {
        return rest.parse::<isize>().ok().map(ITermDimension::Cells);
    }
    // Plain number → cells (WezTerm behaviour).
    s.parse::<isize>().ok().map(ITermDimension::Cells)
}
