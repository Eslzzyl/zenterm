//! Terminal colour themes.
//!
//! A [`Theme`] carries all the colour information needed to render a
//! terminal session: foreground / background, cursor, selection, and the
//! full ANSI 16-colour palette.  It is designed to be cheaply cloneable
//! so each terminal tab can hold its own copy.
//!
//! # Cursor colours
//!
//! The cursor is modelled as a **fill** (`cursor_bg`) and an optional
//! **text** (`cursor_fg`).  When `cursor_fg` is fully transparent it
//! means "use the cell's own foreground colour" (the classic inverse-video
//! cursor behaviour).  Opaque `cursor_fg` overrides the text colour of
//! the character under the block cursor.

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
    /// Cursor fill colour (background of the cursor cell).
    pub cursor_bg: Rgba,
    /// Cursor text colour.  Fully transparent → use the cell's own
    /// foreground (classic inverse-video behaviour).
    pub cursor_fg: Rgba,
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
    /// Accent colour for interactive UI elements.
    pub ui_accent: Rgba,
    /// Surface colour for cards / list items.
    pub ui_surface: Rgba,
}

impl Theme {
    /// Resolve a [`ThemePreference`] + system-dark-mode flag into a
    /// concrete [`Theme`].
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

// ── Built-in themes ───────────────────────────────────────────────────

/// Built-in dark theme (sRGB approximate colour palette).
pub static THEME_DARK: Theme = Theme {
    name: Cow::Borrowed("Dark"),

    // Terminal basics.
    foreground: srgb(216, 222, 233),
    background: srgb(17, 19, 24),
    cursor_bg: srgb(167, 139, 250),
    cursor_fg: srgb(17, 19, 24),
    selection_bg: srgb(57, 48, 82),
    selection_fg: srgb(245, 243, 255),

    // ANSI normal.
    ansi_normal: [
        srgb(27, 31, 39),    // Black
        srgb(224, 108, 117), // Red
        srgb(152, 195, 121), // Green
        srgb(229, 192, 123), // Yellow
        srgb(97, 175, 239),  // Blue
        srgb(198, 120, 221), // Magenta
        srgb(86, 182, 194),  // Cyan
        srgb(171, 178, 191), // White
    ],

    // ANSI bright.
    ansi_bright: [
        srgb(92, 99, 112),   // BrightBlack
        srgb(255, 123, 134), // BrightRed
        srgb(181, 224, 144), // BrightGreen
        srgb(255, 213, 138), // BrightYellow
        srgb(124, 199, 255), // BrightBlue
        srgb(226, 154, 255), // BrightMagenta
        srgb(115, 224, 232), // BrightCyan
        srgb(255, 255, 255), // BrightWhite
    ],

    // Extended.
    dim_foreground: srgb(142, 151, 166),
    bright_foreground: srgb(255, 255, 255),

    // UI chrome.
    ui_bg: srgb(23, 26, 33),
    ui_text: srgb(216, 222, 233),
    ui_accent: srgb(167, 139, 250),
    ui_surface: srgb(32, 37, 48),
};

/// Built-in light theme (sRGB approximate colour palette).
pub static THEME_LIGHT: Theme = Theme {
    name: Cow::Borrowed("Light"),

    // Terminal basics.
    foreground: srgb(39, 49, 59),
    background: srgb(251, 250, 247),
    cursor_bg: srgb(102, 87, 179),
    cursor_fg: srgb(251, 250, 247),
    selection_bg: srgb(221, 216, 243),
    selection_fg: srgb(39, 49, 59),

    // ANSI normal.
    ansi_normal: [
        srgb(46, 52, 61),    // Black
        srgb(180, 67, 71),   // Red
        srgb(63, 126, 70),   // Green
        srgb(145, 105, 27),  // Yellow
        srgb(53, 92, 154),   // Blue
        srgb(127, 72, 139),  // Magenta
        srgb(38, 119, 128),  // Cyan
        srgb(113, 121, 130), // White
    ],

    // ANSI bright.
    ansi_bright: [
        srgb(105, 112, 123), // BrightBlack
        srgb(209, 87, 91),   // BrightRed
        srgb(83, 155, 91),   // BrightGreen
        srgb(171, 128, 40),  // BrightYellow
        srgb(72, 116, 185),  // BrightBlue
        srgb(151, 88, 166),  // BrightMagenta
        srgb(48, 145, 155),  // BrightCyan
        srgb(39, 49, 59),    // BrightWhite
    ],

    // Extended.
    dim_foreground: srgb(111, 120, 130),
    bright_foreground: srgb(25, 32, 40),

    // UI chrome.
    ui_bg: srgb(243, 241, 237),
    ui_text: srgb(39, 49, 59),
    ui_accent: srgb(102, 87, 179),
    ui_surface: srgb(235, 232, 226),
};

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_has_deep_background() {
        assert_eq!(THEME_DARK.background, Rgba::from_u8(17, 19, 24, 255));
        assert_eq!(THEME_DARK.name.as_ref(), "Dark");
    }

    #[test]
    fn light_theme_has_soft_background() {
        assert_eq!(THEME_LIGHT.background, Rgba::from_u8(251, 250, 247, 255));
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
