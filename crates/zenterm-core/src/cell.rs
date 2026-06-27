//! Terminal cell representation.
//!
//! A [`Cell`] holds the character and visual style for one position in the
//! terminal grid. This is the *rendered* cell — colors have been resolved
//! from the terminal's colour scheme into absolute RGBA values.

use std::fmt;

use crate::color::Rgba;

/// Style of underline decoration for a terminal cell.
///
/// Maps to alacritty_terminal's `Flags::*_UNDERLINE` bits.  The Kitty
/// extended underline styles (SGR 4:1–4:5) are the primary source:
///
/// | SGR   | Variant | Alacritty flag       |
/// |-------|---------|----------------------|
/// | 4     | Normal  | `UNDERLINE`          |
/// | 4:2   | Double  | `DOUBLE_UNDERLINE`   |
/// | 4:3   | Curly   | `UNDERCURL`          |
/// | 4:4   | Dotted  | `DOTTED_UNDERLINE`   |
/// | 4:5   | Dashed  | `DASHED_UNDERLINE`   |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlineStyle {
    /// No underline.
    None,
    /// Normal / single underline (SGR 4).
    Normal,
    /// Double underline (SGR 21, SGR 4:2).
    Double,
    /// Curly / wavy underline (SGR 4:3).
    Curly,
    /// Dotted underline (SGR 4:4).
    Dotted,
    /// Dashed underline (SGR 4:5).
    Dashed,
}

impl fmt::Display for UnderlineStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Normal => write!(f, "normal"),
            Self::Double => write!(f, "double"),
            Self::Curly => write!(f, "curly"),
            Self::Dotted => write!(f, "dotted"),
            Self::Dashed => write!(f, "dashed"),
        }
    }
}

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
    /// Underline style (normal, double, curly, dotted, dashed).
    pub underline_style: UnderlineStyle,
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
            underline_style: UnderlineStyle::None,
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
