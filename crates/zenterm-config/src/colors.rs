//! Terminal colour configuration parsed from the TOML config file.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field, default value, or enum variant in this module,
//! update [`docs/usages/config.md`] to match.
//!
//! Each section mirrors the `[colors]` table in `zenterm.toml`.
//! All fields are optional — `None` means "use the built-in theme default".

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

use zenterm_core::color::Rgba;
use zenterm_core::theme::Theme;

// ── Top-level colors table ─────────────────────────────────────────────

/// The `[colors]` section of the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorsConfig {
    /// Built-in theme preference: `"Dark"`, `"Light"`, or `"System"`.
    /// Falls back to `System` when absent.
    #[serde(default)]
    pub theme: ThemePreference,

    /// Core foreground / background colours.
    #[serde(default)]
    pub primary: PrimaryColors,

    /// Cursor colours.
    #[serde(default)]
    pub cursor: CursorColors,

    /// Selection colours.
    #[serde(default)]
    pub selection: SelectionColors,

    /// Normal (dark) ANSI colours: black, red, green, yellow, blue,
    /// magenta, cyan, white.
    #[serde(default)]
    pub normal: AnsiColors,

    /// Bright ANSI colours.
    #[serde(default)]
    pub bright: AnsiColors,

    /// Dim ANSI colours (optional — auto-calculated when absent).
    #[serde(default)]
    pub dim: Option<AnsiColors>,
}

impl Default for ColorsConfig {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            primary: PrimaryColors::default(),
            cursor: CursorColors::default(),
            selection: SelectionColors::default(),
            normal: AnsiColors::default(),
            bright: AnsiColors::default(),
            dim: None,
        }
    }
}

/// Serialisable mirror of [`ThemePreference`] that uses lowercase tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemePreference {
    #[default]
    #[serde(rename = "System")]
    System,
    #[serde(rename = "Dark")]
    Dark,
    #[serde(rename = "Light")]
    Light,
}

// ── Primary colours ────────────────────────────────────────────────────

/// `[colors.primary]` — the two core terminal colours.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimaryColors {
    /// Default text colour (hex `"#rrggbb"`).
    pub foreground: Option<String>,
    /// Default background colour (hex `"#rrggbb"`).
    pub background: Option<String>,
    /// Text colour for the "dim" (half-intensity) SGR attribute.
    pub dim_foreground: Option<String>,
    /// Text colour used when `draw_bold_text_with_bright_colors` is
    /// enabled and bold is active.
    pub bright_foreground: Option<String>,
}

impl Default for PrimaryColors {
    fn default() -> Self {
        Self {
            foreground: None,
            background: None,
            dim_foreground: None,
            bright_foreground: None,
        }
    }
}

// ── Cursor colours ─────────────────────────────────────────────────────

/// `[colors.cursor]` — colours for the terminal cursor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorColors {
    /// Colour for the text under the cursor.  `"CellBackground"` means
    /// "use the cell's background colour" (inverse video).
    pub text: Option<String>,
    /// Colour for the cursor cell itself.  `"CellForeground"` means
    /// "use the cell's foreground colour".
    pub cursor: Option<String>,
}

impl Default for CursorColors {
    fn default() -> Self {
        Self {
            text: None,
            cursor: None,
        }
    }
}

// ── Selection colours ──────────────────────────────────────────────────

/// `[colors.selection]` — colours for selected text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionColors {
    /// Foreground colour of selected text.
    pub foreground: Option<String>,
    /// Background colour of selected text.
    pub background: Option<String>,
}

impl Default for SelectionColors {
    fn default() -> Self {
        Self {
            foreground: None,
            background: None,
        }
    }
}

// ── ANSI 16-colour palette ─────────────────────────────────────────────

/// The 8-colour ANSI palette (used for both `[colors.normal]` and
/// `[colors.bright]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnsiColors {
    pub black: Option<String>,
    pub red: Option<String>,
    pub green: Option<String>,
    pub yellow: Option<String>,
    pub blue: Option<String>,
    pub magenta: Option<String>,
    pub cyan: Option<String>,
    pub white: Option<String>,
}

impl Default for AnsiColors {
    fn default() -> Self {
        Self {
            black: None,
            red: None,
            green: None,
            yellow: None,
            blue: None,
            magenta: None,
            cyan: None,
            white: None,
        }
    }
}

// ── Conversion helpers ─────────────────────────────────────────────────

impl ColorsConfig {
    /// Build a [`Theme`] from this config section, using the built-in
    /// theme as a base and overlaying any custom colours that were set.
    pub fn to_theme(&self, system_dark: bool) -> Theme {
        let pref = match self.theme {
            ThemePreference::Dark => zenterm_core::theme::ThemePreference::Dark,
            ThemePreference::Light => zenterm_core::theme::ThemePreference::Light,
            ThemePreference::System => zenterm_core::theme::ThemePreference::System,
        };
        let mut theme = Theme::resolve(pref, system_dark);
        theme.name = Cow::Owned(self.theme_name());

        // Apply primary overrides.
        if let Some(c) = parse_hex_opt(&self.primary.foreground) {
            theme.foreground = c;
        }
        if let Some(c) = parse_hex_opt(&self.primary.background) {
            theme.background = c;
        }
        if let Some(c) = parse_hex_opt(&self.primary.dim_foreground) {
            theme.dim_foreground = c;
        }
        if let Some(c) = parse_hex_opt(&self.primary.bright_foreground) {
            theme.bright_foreground = c;
        }

        // Cursor.
        if let Some(c) = parse_hex_opt(&self.cursor.cursor) {
            theme.cursor = c;
        }

        // Selection.
        if let Some(c) = parse_hex_opt(&self.selection.foreground) {
            theme.selection_fg = c;
        }
        if let Some(c) = parse_hex_opt(&self.selection.background) {
            theme.selection_bg = c;
        }

        // ANSI normal.
        apply_ansi(&mut theme.ansi_normal, &self.normal);

        // ANSI bright.
        apply_ansi(&mut theme.ansi_bright, &self.bright);

        theme
    }

    fn theme_name(&self) -> String {
        match self.theme {
            ThemePreference::Dark => "Dark (customised)".into(),
            ThemePreference::Light => "Light (customised)".into(),
            ThemePreference::System => "System (customised)".into(),
        }
    }
}

fn apply_ansi(target: &mut [Rgba; 8], src: &AnsiColors) {
    let pairs = [
        (&src.black, 0),
        (&src.red, 1),
        (&src.green, 2),
        (&src.yellow, 3),
        (&src.blue, 4),
        (&src.magenta, 5),
        (&src.cyan, 6),
        (&src.white, 7),
    ];
    for (hex, i) in pairs {
        if let Some(c) = parse_hex_opt(hex) {
            target[i] = c;
        }
    }
}

// ── Hex colour parsing ─────────────────────────────────────────────────

/// Parse a hex colour string like `"#rrggbb"` or `"#rgb"` into [`Rgba`].
///
/// Returns `None` if the string is empty, `"CellBackground"`, or
/// `"CellForeground"` (sentinel values used by Alacritty's cursor colours
/// — we just fall through to the theme default for those).
pub(crate) fn parse_hex_opt(s: &Option<String>) -> Option<Rgba> {
    match s.as_deref() {
        None | Some("") | Some("CellBackground") | Some("CellForeground") => None,
        Some(hex) => {
            match parse_hex(hex) {
                Ok(c) => Some(c),
                Err(e) => {
                    log::warn!("invalid colour {hex:?}: {e}");
                    None
                }
            }
        }
    }
}

/// Parse a `#rrggbb` or `#rgb` hex string into [`Rgba`].
fn parse_hex(hex: &str) -> Result<Rgba, String> {
    let hex = hex.trim_start_matches('#');
    let (r, g, b) = match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).map_err(|e| format!("{e}"))? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).map_err(|e| format!("{e}"))? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).map_err(|e| format!("{e}"))? * 17;
            (r, g, b)
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).map_err(|e| format!("{e}"))?;
            let g = u8::from_str_radix(&hex[2..4], 16).map_err(|e| format!("{e}"))?;
            let b = u8::from_str_radix(&hex[4..6], 16).map_err(|e| format!("{e}"))?;
            (r, g, b)
        }
        _ => return Err("expected 3 or 6 hex digits after #".into()),
    };
    Ok(Rgba::from_u8(r, g, b, 255))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_hex() {
        let c = parse_hex("#d8d8d8").unwrap();
        assert_eq!(c, Rgba::from_u8(216, 216, 216, 255));
    }

    #[test]
    fn parse_shorthand_hex() {
        let c = parse_hex("#abc").unwrap();
        assert_eq!(c, Rgba::from_u8(170, 187, 204, 255));
    }

    #[test]
    fn parse_hex_without_hash() {
        let c = parse_hex("ff0000").unwrap();
        assert_eq!(c, Rgba::from_u8(255, 0, 0, 255));
    }

    #[test]
    fn parse_hex_opt_none() {
        assert_eq!(parse_hex_opt(&None), None);
        assert_eq!(parse_hex_opt(&Some(String::new())), None);
        assert_eq!(parse_hex_opt(&Some("CellBackground".into())), None);
    }

    #[test]
    fn parse_hex_opt_valid() {
        assert_eq!(
            parse_hex_opt(&Some("#ff0080".into())),
            Some(Rgba::from_u8(255, 0, 128, 255))
        );
    }
}
