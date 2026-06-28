//! Grid snapshot and cursor info for the renderer.

use alacritty_terminal::vte::ansi::CursorStyle;

use zenterm_core::cell::Cell;
use zenterm_core::position::TermPos;

/// Cursor information for rendering.
#[derive(Debug, Clone)]
pub struct CursorInfo {
    pub pos: TermPos,
    pub style: CursorStyle,
    pub visible: bool,
}

// ── Grid view ───────────────────────────────────────────────────────────

/// A view of the visible grid rows.
pub struct GridView<'a> {
    pub(crate) rows: &'a [Vec<Cell>],
}

impl<'a> GridView<'a> {
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn col_count(&self) -> usize {
        self.rows.first().map_or(0, |r| r.len())
    }

    pub fn cell(&self, line: usize, col: usize) -> Option<&'a Cell> {
        self.rows.get(line).and_then(|r| r.get(col))
    }

    pub fn rows(&self) -> impl Iterator<Item = &'a [Cell]> {
        self.rows.iter().map(|v| v.as_slice())
    }
}
