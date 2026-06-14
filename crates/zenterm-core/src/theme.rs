//! Terminal colour themes.
//!
//! A [`Theme`] carries all the colour information needed to render a
//! terminal session: foreground / background, cursor, selection, and the
//! full ANSI 16-colour palette.  It is designed to be cheaply cloneable
//! so each terminal tab can hold its own copy.

use std::borrow::Cow;

use crate::color::Rgba;

// ── Helper ─────────────────────────────────────────────────────────────

/// Convenience: create an opaque sRGB colour from 8-bit components.
const fn srgb(r: u8, g: u8, b: u8) -> Rgba {
    Rgba::from_u8(r, g, b, 255)
}

// ── ThemePreference ────────────────────────────────────────────────────

/// The user's preference for which theme to use.
///
/// This mirrors [`egui::ThemePreference`] so that Zenterm does not need a
/// hard dependency on egui at the core level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemePreference {
    /// Always use the dark theme.
    Dark,
    /// Always use the light theme.
    Light,
    /// Follow the OS / desktop environment setting.
    #[default]
    System,
}

// ── Theme ──────────────────────────────────────────────────────────────

/// A complete terminal colour scheme.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Human-readable name (e.g. `"Dark"`, `"Light"`, `"Catppuccin Mocha"`).
    ///
    /// `Cow::Borrowed` for built-in themes, `Cow::Owned` for custom themes
    /// constructed at runtime from the config file.
    pub name: Cow<'static, str>,

    // ── Terminal basic colours ───────────────────────────────────────
    /// Default foreground colour.
    pub foreground: Rgba,
    /// Default background colour.
    pub background: Rgba,
    /// Cursor colour.
    pub cursor: Rgba,
    /// Selection background colour.
    pub selection_bg: Rgba,
    /// Selection foreground colour.
    pub selection_fg: Rgba,

    // ── ANSI 16-colour palette ───────────────────────────────────────
    /// Normal (dark) colours: Black, Red, Green, Yellow, Blue, Magenta,
    /// Cyan, White.
    pub ansi_normal: [Rgba; 8],
    /// Bright colours: BrightBlack .. BrightWhite.
    pub ansi_bright: [Rgba; 8],

    // ── Extended terminal colours ────────────────────────────────────
    /// Dim foreground (used for "dim" SGR attribute).
    pub dim_foreground: Rgba,
    /// Bright foreground (used for "bold" SGR attribute).
    pub bright_foreground: Rgba,

    // ── UI chrome colours (reserved, for the settings panel / tabs) ──
    /// Background for UI panels (e.g. the sidebar).
    pub ui_bg: Rgba,
    /// Text colour for UI panels.
    pub ui_text: Rgba,
    /// Accent colour for UI controls.
    pub ui_accent: Rgba,
    /// Surface colour (cards, list items).
    pub ui_surface: Rgba,
}

impl Theme {
    /// Resolve a [`ThemePreference`] + system-dark-mode flag into a
    /// concrete theme.
    ///
    /// Returns an owned clone so the caller can further customise colours
    /// (e.g. apply config-file overrides) without mutating the built-in
    /// statics.
    pub fn resolve(pref: ThemePreference, system_dark: bool) -> Theme {
        match pref {
            ThemePreference::Dark => THEME_DARK.clone(),
            ThemePreference::Light => THEME_LIGHT.clone(),
            ThemePreference::System => {
                if system_dark {
                    THEME_DARK.clone()
                } else {
                    THEME_LIGHT.clone()
                }
            }
        }
    }
}

// ── Built-in presets ──────────────────────────────────────────────────

/// The default **dark** theme — a classic terminal look with black
/// background and light-grey text.
pub static THEME_DARK: Theme = Theme {
    name: Cow::Borrowed("Dark"),
    foreground: srgb(220, 220, 220),
    background: srgb(0, 0, 0),
    cursor: srgb(220, 220, 220),
    selection_bg: srgb(81, 108, 165),
    selection_fg: srgb(220, 220, 220),

    ansi_normal: [
        srgb(0, 0, 0),       // Black
        srgb(170, 0, 0),     // Red
        srgb(0, 170, 0),     // Green
        srgb(170, 85, 0),    // Yellow
        srgb(0, 0, 170),     // Blue
        srgb(170, 0, 170),   // Magenta
        srgb(0, 170, 170),   // Cyan
        srgb(200, 200, 200), // White
    ],
    ansi_bright: [
        srgb(85, 85, 85),    // BrightBlack
        srgb(255, 85, 85),   // BrightRed
        srgb(85, 255, 85),   // BrightGreen
        srgb(255, 255, 85),  // BrightYellow
        srgb(85, 85, 255),   // BrightBlue
        srgb(255, 85, 255),  // BrightMagenta
        srgb(85, 255, 255),  // BrightCyan
        srgb(255, 255, 255), // BrightWhite
    ],

    dim_foreground: srgb(140, 140, 140),
    bright_foreground: srgb(255, 255, 255),

    // Dark UI colours
    ui_bg: srgb(18, 18, 18),
    ui_text: srgb(200, 200, 200),
    ui_accent: srgb(81, 108, 165),
    ui_surface: srgb(30, 30, 30),
};

/// A **light** theme — white background with dark text, suitable for
/// use on a bright desktop.
///
/// ANSI color values are tuned for readability on a light background:
/// regular colours are dark enough to contrast against white, and
/// "bright" variants are *saturated* (rather than light) so they remain
/// legible instead of washing out.
pub static THEME_LIGHT: Theme = Theme {
    name: Cow::Borrowed("Light"),
    foreground: srgb(30, 30, 30),
    background: srgb(255, 255, 255),
    cursor: srgb(30, 30, 30),
    selection_bg: srgb(130, 170, 250),
    selection_fg: srgb(30, 30, 30),

    // Regular ANSI colours – dark enough to be clearly visible on white.
    ansi_normal: [
        srgb(12, 12, 12),   // Black          (#0C0C0C)
        srgb(197, 15, 31),  // Red            (#C50F1F)
        srgb(19, 161, 14),  // Green          (#13A10E)
        srgb(193, 156, 0),  // Yellow         (#C19C00)
        srgb(0, 55, 218),   // Blue           (#0037DA)
        srgb(136, 23, 152), // Magenta        (#881798)
        srgb(58, 150, 221), // Cyan           (#3A96DD)
        srgb(204, 204, 204),// White          (#CCCCCC)
    ],
    // Bright ANSI colours – saturated (not light) so they stay legible
    // against a white background.
    ansi_bright: [
        srgb(118, 118, 118),// BrightBlack    (#767676)
        srgb(231, 72, 86),  // BrightRed      (#E74856)
        srgb(22, 198, 12),  // BrightGreen    (#16C60C)
        srgb(200, 175, 0),  // BrightYellow   (#C8AF00 — darker gold)
        srgb(59, 120, 255), // BrightBlue     (#3B78FF)
        srgb(180, 0, 158),  // BrightMagenta  (#B4009E)
        srgb(97, 214, 214), // BrightCyan     (#61D6D6)
        srgb(242, 242, 242),// BrightWhite    (#F2F2F2)
    ],

    dim_foreground: srgb(140, 140, 140),
    bright_foreground: srgb(0, 0, 0),

    // Light UI colours
    ui_bg: srgb(240, 240, 240),
    ui_text: srgb(30, 30, 30),
    ui_accent: srgb(50, 100, 200),
    ui_surface: srgb(255, 255, 255),
};

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_has_black_background() {
        assert_eq!(THEME_DARK.background, Rgba::BLACK);
        assert_eq!(THEME_DARK.name.as_ref(), "Dark");
    }

    #[test]
    fn light_theme_has_white_background() {
        assert_eq!(THEME_LIGHT.background, Rgba::WHITE);
        assert_eq!(THEME_LIGHT.name.as_ref(), "Light");
    }

    #[test]
    fn resolve_system_dark_returns_dark() {
        let t = Theme::resolve(ThemePreference::System, true);
        assert_eq!(t.name.as_ref(), "Dark");
    }

    #[test]
    fn resolve_system_light_returns_light() {
        let t = Theme::resolve(ThemePreference::System, false);
        assert_eq!(t.name.as_ref(), "Light");
    }

    #[test]
    fn resolve_explicit_preference() {
        let t = Theme::resolve(ThemePreference::Dark, false);
        assert_eq!(t.name.as_ref(), "Dark");

        let t = Theme::resolve(ThemePreference::Light, true);
        assert_eq!(t.name.as_ref(), "Light");
    }
}
