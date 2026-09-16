//! Terminal colour configuration parsed from the TOML config file.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field, default value, or enum variant in this module,
//! update [`docs/usages/config.md`] to match.
//!
//! Each section mirrors the `[colors]` table in `zenterm.toml`.
//! Colour overrides are optional — `None` means "use the selected palette".

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

use zenterm_core::color::Rgba;
use zenterm_core::theme::Theme;

// ── Top-level colors table ─────────────────────────────────────────────

/// The `[colors]` section of the config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorsConfig {
    /// Appearance mode: `"Dark"`, `"Light"`, or `"System"`.
    pub appearance: ThemePreference,

    /// Colour theme family. Each family provides light and dark variants.
    pub palette: ColorTheme,

    /// Core foreground / background colours.
    ///
    /// These are optional overrides on top of the selected palette.
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
}

impl Default for ColorsConfig {
    fn default() -> Self {
        Self {
            appearance: ThemePreference::System,
            palette: ColorTheme::Default,
            primary: PrimaryColors::default(),
            cursor: CursorColors::default(),
            selection: SelectionColors::default(),
            normal: AnsiColors::default(),
            bright: AnsiColors::default(),
        }
    }
}

/// A colour theme family with both light and dark variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ColorTheme {
    #[default]
    #[serde(rename = "Default")]
    Default,
    #[serde(rename = "Nord")]
    Nord,
    #[serde(rename = "Forest")]
    Forest,
    #[serde(rename = "Sepia")]
    Sepia,
}

impl ColorTheme {
    pub const ALL: [Self; 4] = [Self::Default, Self::Nord, Self::Forest, Self::Sepia];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Nord => "Nord",
            Self::Forest => "Forest",
            Self::Sepia => "Sepia",
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

// ── Cursor colours ─────────────────────────────────────────────────────

/// `[colors.cursor]` — colours for the terminal cursor.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CursorColors {
    /// Colour for the text under the cursor.  `"CellBackground"` means
    /// "use the cell's background colour" (inverse video).
    pub text: Option<String>,
    /// Colour for the cursor cell itself.  `"CellForeground"` means
    /// "use the cell's foreground colour".
    pub cursor: Option<String>,
}

// ── Selection colours ──────────────────────────────────────────────────

/// `[colors.selection]` — colours for selected text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SelectionColors {
    /// Foreground colour of selected text.
    pub foreground: Option<String>,
    /// Background colour of selected text.
    pub background: Option<String>,
}

// ── ANSI 16-colour palette ─────────────────────────────────────────────

/// The 8-colour ANSI palette (used for both `[colors.normal]` and
/// `[colors.bright]`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

// ── Conversion helpers ─────────────────────────────────────────────────

impl ColorsConfig {
    /// Build a [`Theme`] from this config section, using the built-in
    /// theme as a base and overlaying any custom colours that were set.
    pub fn to_theme(&self, system_dark: bool) -> Theme {
        let pref = match self.appearance {
            ThemePreference::Dark => zenterm_core::theme::ThemePreference::Dark,
            ThemePreference::Light => zenterm_core::theme::ThemePreference::Light,
            ThemePreference::System => zenterm_core::theme::ThemePreference::System,
        };
        let mut theme = Theme::resolve(pref, system_dark);
        apply_palette(
            &mut theme,
            self.palette,
            self.appearance_is_dark(system_dark),
        );

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
            theme.cursor_bg = c;
        }
        if let Some(c) = parse_hex_opt(&self.cursor.text) {
            theme.cursor_fg = c;
        }

        // Selection.
        if let Some(c) = parse_hex_opt(&self.selection.foreground) {
            theme.selection_fg = c;
        }
        let custom_selection_background = if let Some(c) = parse_hex_opt(&self.selection.background)
        {
            theme.selection_bg = c;
            true
        } else {
            false
        };

        // ANSI normal.
        apply_ansi(&mut theme.ansi_normal, &self.normal);

        // ANSI bright.
        apply_ansi(&mut theme.ansi_bright, &self.bright);

        // Derive UI chrome from the resolved terminal theme when custom
        // foreground/background colours or non-default palettes are used.
        if self.palette != ColorTheme::Default || self.has_custom_colors() {
            let dark_mode = theme.background.r() < 0.5;
            theme.ui_text = theme.foreground;
            theme.ui_bg = blend_rgba(
                theme.background,
                theme.foreground,
                if dark_mode { 0.08 } else { 0.04 },
            );
            theme.ui_surface = blend_rgba(
                theme.background,
                theme.foreground,
                if dark_mode { 0.16 } else { 0.10 },
            );
            if custom_selection_background {
                theme.ui_accent = theme.selection_bg;
            }
        }

        theme.name = Cow::Owned(self.theme_name());
        theme
    }

    fn theme_name(&self) -> String {
        if self.has_custom_colors() {
            format!("{} (customised)", self.palette.label())
        } else {
            self.palette.label().into()
        }
    }

    fn appearance_is_dark(&self, system_dark: bool) -> bool {
        match self.appearance {
            ThemePreference::Dark => true,
            ThemePreference::Light => false,
            ThemePreference::System => system_dark,
        }
    }

    /// Whether any individual colour override is active.
    pub fn has_custom_colors(&self) -> bool {
        self.primary != PrimaryColors::default()
            || self.cursor != CursorColors::default()
            || self.selection != SelectionColors::default()
            || self.normal != AnsiColors::default()
            || self.bright != AnsiColors::default()
    }
}

#[derive(Clone, Copy)]
struct PaletteColors {
    foreground: &'static str,
    background: &'static str,
    cursor: &'static str,
    selection: &'static str,
    selection_fg: &'static str,
    normal: [&'static str; 8],
    bright: [&'static str; 8],
}

fn apply_palette(theme: &mut Theme, palette: ColorTheme, dark: bool) {
    let Some(colors) = palette_colors(palette, dark) else {
        theme.name = Cow::Owned(palette.label().into());
        return;
    };

    theme.foreground = parse_builtin(colors.foreground);
    theme.background = parse_builtin(colors.background);
    theme.cursor_bg = parse_builtin(colors.cursor);
    theme.cursor_fg = parse_builtin(colors.background);
    theme.selection_bg = parse_builtin(colors.selection);
    theme.selection_fg = parse_builtin(colors.selection_fg);
    theme.ansi_normal = colors.normal.map(parse_builtin);
    theme.ansi_bright = colors.bright.map(parse_builtin);
    theme.dim_foreground = blend_rgba(theme.foreground, theme.background, 0.55);
    theme.bright_foreground = if dark {
        Rgba::from_u8(255, 255, 255, 255)
    } else {
        Rgba::from_u8(25, 32, 40, 255)
    };
    theme.name = Cow::Owned(palette.label().into());
}

fn parse_builtin(hex: &str) -> Rgba {
    parse_hex(hex).expect("built-in palette contains a valid colour")
}

fn palette_colors(palette: ColorTheme, dark: bool) -> Option<PaletteColors> {
    match (palette, dark) {
        (ColorTheme::Default, _) => None,
        (ColorTheme::Nord, true) => Some(PaletteColors {
            foreground: "#d8dee9",
            background: "#2e3440",
            cursor: "#88c0d0",
            selection: "#434c5e",
            selection_fg: "#eceff4",
            normal: [
                "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0",
                "#e5e9f0",
            ],
            bright: [
                "#4c566a", "#d08770", "#b1d196", "#f0d399", "#8fbcdb", "#c39aba", "#8fcedd",
                "#eceff4",
            ],
        }),
        (ColorTheme::Nord, false) => Some(PaletteColors {
            foreground: "#263445",
            background: "#f4f7fb",
            cursor: "#3478b9",
            selection: "#d8e8f7",
            selection_fg: "#263445",
            normal: [
                "#354052", "#c04f5e", "#3d7a57", "#9a7316", "#2f68a2", "#8c5a9e", "#267f86",
                "#607080",
            ],
            bright: [
                "#718096", "#d96573", "#5d9f72", "#b58b2a", "#4b8cc7", "#a776b5", "#3b9da4",
                "#263445",
            ],
        }),
        (ColorTheme::Forest, true) => Some(PaletteColors {
            foreground: "#d8e8dc",
            background: "#101914",
            cursor: "#7dc9a2",
            selection: "#284437",
            selection_fg: "#d8e8dc",
            normal: [
                "#1f2a23", "#e27d8c", "#98c379", "#d8b46c", "#78a9c8", "#c39ac9", "#70c5b3",
                "#d8e8dc",
            ],
            bright: [
                "#526258", "#f29aa7", "#b5e890", "#efd08c", "#9bc9e6", "#dfb3e4", "#94e3cf",
                "#f0faf2",
            ],
        }),
        (ColorTheme::Forest, false) => Some(PaletteColors {
            foreground: "#244037",
            background: "#f3faf7",
            cursor: "#2f8f6b",
            selection: "#d4ede2",
            selection_fg: "#244037",
            normal: [
                "#30483e", "#b64e5a", "#397a58", "#997326", "#3c6fa3", "#865a98", "#2f8176",
                "#668077",
            ],
            bright: [
                "#81988d", "#d76570", "#55a276", "#b28d3e", "#5a91c2", "#a374b2", "#4ca99d",
                "#244037",
            ],
        }),
        (ColorTheme::Sepia, true) => Some(PaletteColors {
            foreground: "#ead8bf",
            background: "#201b16",
            cursor: "#d5a36a",
            selection: "#4b3828",
            selection_fg: "#ead8bf",
            normal: [
                "#3b3027", "#c46f63", "#93a36a", "#d2a95c", "#7fa3a8", "#b18ca9", "#7db1a8",
                "#ead8bf",
            ],
            bright: [
                "#746253", "#e18a7b", "#b3c485", "#edc879", "#9bc5c9", "#cba8c4", "#98d0c4",
                "#fff4df",
            ],
        }),
        (ColorTheme::Sepia, false) => Some(PaletteColors {
            foreground: "#4b3828",
            background: "#fbf5ea",
            cursor: "#b06f3c",
            selection: "#ead7c0",
            selection_fg: "#4b3828",
            normal: [
                "#5c4a3a", "#a64b3c", "#5e7a49", "#9b6a1f", "#4d6f8f", "#80618a", "#4f817c",
                "#85705d",
            ],
            bright: [
                "#9b856f", "#c96554", "#78995b", "#bd8932", "#6b91b2", "#a27caf", "#6aa49d",
                "#4b3828",
            ],
        }),
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

fn blend_rgba(a: Rgba, b: Rgba, amount: f32) -> Rgba {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |x: f32, y: f32| x + (y - x) * amount;
    Rgba::rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
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
        Some(hex) => match parse_hex(hex) {
            Ok(c) => Some(c),
            Err(e) => {
                log::warn!("invalid colour {hex:?}: {e}");
                None
            }
        },
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
