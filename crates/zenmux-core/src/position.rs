//! Position within the terminal grid.

use crate::size::TermSize;

/// A position (line, column) in the terminal grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TermPos {
    /// Row (0 = topmost visible row).
    pub line: usize,
    /// Column (0 = leftmost column).
    pub column: usize,
}

impl TermPos {
    /// Create a new position.
    pub const fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }

    /// Check whether this position is within the given bounds.
    pub fn is_visible(&self, size: &TermSize) -> bool {
        self.line < size.rows as usize && self.column < size.cols as usize
    }
}
