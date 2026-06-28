//! Resolved colour scheme that maps terminal colour indices to RGBA values.

use std::fmt;

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{NamedColor, Rgb};

use zenterm_core::color::Rgba;
use zenterm_core::theme::Theme;

/// A resolved colour scheme that maps index-based colours to real RGBA values.
#[derive(Clone)]
pub struct ColorScheme {
    pub colors: Colors,
    /// Selection background colour.
    pub selection_bg: Rgba,
    /// Selection foreground colour.  `None` means keep the cell's fg.
    pub selection_fg: Option<Rgba>,
}

impl fmt::Debug for ColorScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ColorScheme").finish_non_exhaustive()
    }
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self::from_theme(&zenterm_core::theme::THEME_DARK)
    }
}

impl ColorScheme {
    /// Build a colour scheme from a [`Theme`].
    ///
    /// Pre-populates the full `Colors` array so that alacritty's named-colour
    /// resolution has values for every standard slot, avoiding fallback to
    /// `named_color_default_rgb`.
    pub fn from_theme(theme: &Theme) -> Self {
        let mut colors = Colors::default();

        // ANSI normal colours (NamedColor::Black .. NamedColor::White = 0..7).
        for (i, c) in theme.ansi_normal.iter().enumerate() {
            colors[i] = Some(rgba_to_rgb(c));
        }
        // ANSI bright colours (NamedColor::BrightBlack .. NamedColor::BrightWhite = 8..15).
        for (i, c) in theme.ansi_bright.iter().enumerate() {
            colors[8 + i] = Some(rgba_to_rgb(c));
        }
        // Foreground / Background / Cursor.
        colors[NamedColor::Foreground as usize] = Some(rgba_to_rgb(&theme.foreground));
        colors[NamedColor::Background as usize] = Some(rgba_to_rgb(&theme.background));
        colors[NamedColor::Cursor as usize] = Some(rgba_to_rgb(&theme.cursor));
        // Dim / Bright foreground.
        colors[NamedColor::DimForeground as usize] = Some(rgba_to_rgb(&theme.dim_foreground));
        colors[NamedColor::BrightForeground as usize] = Some(rgba_to_rgb(&theme.bright_foreground));

        // 256-colour palette: 6×6×6 colour cube (indices 16-231).
        let mut idx = 16;
        for r in 0..6 {
            for g in 0..6 {
                for b in 0..6 {
                    let rgb = Rgb {
                        r: if r == 0 { 0 } else { r * 40 + 55 },
                        g: if g == 0 { 0 } else { g * 40 + 55 },
                        b: if b == 0 { 0 } else { b * 40 + 55 },
                    };
                    colors[idx] = Some(rgb);
                    idx += 1;
                }
            }
        }

        // Grayscale ramp (indices 232-255).
        for i in 0..24 {
            let v = (i * 10 + 8) as u8;
            colors[232 + i] = Some(Rgb { r: v, g: v, b: v });
        }

        Self {
            colors,
            selection_bg: theme.selection_bg,
            selection_fg: Some(theme.selection_fg),
        }
    }

    /// Rebuild this scheme from a new theme (replaces *all* colours).
    pub fn set_theme(&mut self, theme: &Theme) {
        *self = Self::from_theme(theme);
    }
}

/// Convert our internal `Rgba` to alacritty's `Rgb`.
fn rgba_to_rgb(c: &Rgba) -> Rgb {
    Rgb {
        r: (c.r() * 255.0).round() as u8,
        g: (c.g() * 255.0).round() as u8,
        b: (c.b() * 255.0).round() as u8,
    }
}
pub(crate) fn named_color_default_rgb(named: NamedColor) -> Rgb {
    match named {
        NamedColor::Black => Rgb { r: 0, g: 0, b: 0 },
        NamedColor::Red => Rgb { r: 170, g: 0, b: 0 },
        NamedColor::Green => Rgb { r: 0, g: 170, b: 0 },
        NamedColor::Yellow => Rgb { r: 170, g: 170, b: 0 },
        NamedColor::Blue => Rgb { r: 0, g: 0, b: 170 },
        NamedColor::Magenta => Rgb { r: 170, g: 0, b: 170 },
        NamedColor::Cyan => Rgb { r: 0, g: 170, b: 170 },
        NamedColor::White => Rgb { r: 200, g: 200, b: 200 },
        NamedColor::BrightBlack => Rgb { r: 85, g: 85, b: 85 },
        NamedColor::BrightRed => Rgb { r: 255, g: 85, b: 85 },
        NamedColor::BrightGreen => Rgb { r: 85, g: 255, b: 85 },
        NamedColor::BrightYellow => Rgb { r: 255, g: 255, b: 85 },
        NamedColor::BrightBlue => Rgb { r: 85, g: 85, b: 255 },
        NamedColor::BrightMagenta => Rgb { r: 255, g: 85, b: 255 },
        NamedColor::BrightCyan => Rgb { r: 85, g: 255, b: 255 },
        NamedColor::BrightWhite => Rgb { r: 255, g: 255, b: 255 },
        // Terminal-default colours used when no colour scheme is configured.
        NamedColor::Foreground => Rgb { r: 220, g: 220, b: 220 }, // light grey
        NamedColor::Background => Rgb { r: 0, g: 0, b: 0 },      // black
        NamedColor::Cursor => Rgb { r: 220, g: 220, b: 220 },    // same as fg
        NamedColor::DimForeground => Rgb { r: 140, g: 140, b: 140 },
        NamedColor::BrightForeground => Rgb { r: 255, g: 255, b: 255 },
        _ => Rgb { r: 255, g: 255, b: 255 },
    }
}
