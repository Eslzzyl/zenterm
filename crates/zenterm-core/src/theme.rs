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
                if system_dark { THEME_DARK.clone() } else { THEME_LIGHT.clone() }
            }
        }
    }
}

// ── Built-in themes ───────────────────────────────────────────────────

/// Built-in dark theme (sRGB approximate colour palette).
pub static THEME_DARK: Theme = Theme {
    name: Cow::Borrowed("Dark"),

    // Terminal basics.
    foreground: srgb(220, 220, 220),
    background: srgb(0, 0, 0),
    cursor_bg: srgb(220, 220, 220),
    cursor_fg: Rgba::TRANSPARENT,
    selection_bg: srgb(80, 80, 100),
    selection_fg: srgb(220, 220, 220),

    // ANSI normal.
    ansi_normal: [
        srgb(0, 0, 0),       // Black
        srgb(200, 50, 50),   // Red
        srgb(80, 180, 80),   // Green
        srgb(200, 180, 50),  // Yellow
        srgb(50, 100, 200),  // Blue
        srgb(180, 60, 180),  // Magenta
        srgb(50, 170, 180),  // Cyan
        srgb(190, 190, 190), // White
    ],

    // ANSI bright.
    ansi_bright: [
        srgb(100, 100, 100), // BrightBlack
        srgb(255, 80, 80),   // BrightRed
        srgb(100, 255, 100), // BrightGreen
        srgb(255, 255, 80),  // BrightYellow
        srgb(80, 130, 255),  // BrightBlue
        srgb(255, 80, 255),  // BrightMagenta
        srgb(80, 255, 255),  // BrightCyan
        srgb(255, 255, 255), // BrightWhite
    ],

    // Extended.
    dim_foreground: srgb(140, 140, 140),
    bright_foreground: srgb(255, 255, 255),

    // UI chrome.
    ui_bg: srgb(30, 30, 30),
    ui_text: srgb(200, 200, 200),
    ui_accent: srgb(60, 120, 220),
    ui_surface: srgb(45, 45, 45),
};

/// Built-in light theme (sRGB approximate colour palette).
pub static THEME_LIGHT: Theme = Theme {
    name: Cow::Borrowed("Light"),

    // Terminal basics.
    foreground: srgb(30, 30, 30),
    background: srgb(255, 255, 255),
    cursor_bg: srgb(30, 30, 30),
    cursor_fg: Rgba::TRANSPARENT,
    selection_bg: srgb(180, 180, 220),
    selection_fg: srgb(30, 30, 30),

    // ANSI normal.
    ansi_normal: [
        srgb(0, 0, 0),       // Black
        srgb(200, 50, 50),   // Red
        srgb(80, 180, 80),   // Green
        srgb(180, 160, 40),  // Yellow
        srgb(50, 80, 180),   // Blue
        srgb(160, 50, 160),  // Magenta
        srgb(40, 150, 160),  // Cyan
        srgb(180, 180, 180), // White
    ],

    // ANSI bright.
    ansi_bright: [
        srgb(100, 100, 100), // BrightBlack
        srgb(255, 60, 60),   // BrightRed
        srgb(60, 220, 60),   // BrightGreen
        srgb(220, 220, 60),  // BrightYellow
        srgb(60, 100, 220),  // BrightBlue
        srgb(220, 60, 220),  // BrightMagenta
        srgb(60, 220, 220),  // BrightCyan
        srgb(255, 255, 255), // BrightWhite
    ],

    // Extended.
    dim_foreground: srgb(120, 120, 120),
    bright_foreground: srgb(0, 0, 0),

    // UI chrome.
    ui_bg: srgb(235, 235, 235),
    ui_text: srgb(30, 30, 30),
    ui_accent: srgb(60, 100, 200),
    ui_surface: srgb(220, 220, 220),
};

// ── Tests ─────────────────────────────────────────────────────────────

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
