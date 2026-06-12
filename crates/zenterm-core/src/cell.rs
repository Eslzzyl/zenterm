//! Terminal cell representation.
//!
//! A [`Cell`] holds the character and visual style for one position in the
//! terminal grid. This is the *rendered* cell — colors have been resolved
//! from the terminal's colour scheme into absolute RGBA values.

use crate::color::Rgba;

/// A single display cell in the terminal grid.
#[derive(Debug, Clone)]
pub struct Cell {
    /// The character to display.
    pub c: char,

    /// Resolved foreground colour.
    pub fg: Rgba,

    /// Resolved background colour.
    pub bg: Rgba,

    // ---- Style flags ----
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub inverse: bool,
    pub dim: bool,
    pub hidden: bool,

    /// True if this cell is the trailing spacer of a wide character (CJK,
    /// emoji).  Spacer cells share the glyph of the preceding cell and
    /// should be skipped during rendering.
    pub is_spacer: bool,
}

impl Cell {
    /// Create an empty cell with default styling.
    pub const fn new(c: char, fg: Rgba, bg: Rgba) -> Self {
        Self {
            c,
            fg,
            bg,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            inverse: false,
            dim: false,
            hidden: false,
            is_spacer: false,
        }
    }

    /// Create an empty default cell (blank, white on black).
    pub fn blank() -> Self {
        Self::new(' ', Rgba::WHITE, Rgba::BLACK)
    }

    /// Check whether this cell is visually empty (space character, no special
    /// background).
    pub fn is_empty(&self) -> bool {
        self.c == ' '
    }
}
