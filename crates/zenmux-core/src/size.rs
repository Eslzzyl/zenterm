//! Terminal size in rows and columns.

/// Size of a terminal in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    /// Number of visible rows.
    pub rows: u16,
    /// Number of visible columns.
    pub cols: u16,
}

impl TermSize {
    /// Create a new terminal size.
    pub const fn new(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }
}

impl Default for TermSize {
    fn default() -> Self {
        Self::new(24, 80)
    }
}
