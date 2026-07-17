//! Terminal size in rows, columns, and pixels.

/// Size of a terminal in cells and pixels.
///
/// The pixel dimensions represent the total text-area size.
/// They are propagated to the PTY so that applications can query
/// the cell size via `TIOCGWINSZ` / CSI 16 t.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    /// Number of visible rows.
    pub rows: u16,
    /// Number of visible columns.
    pub cols: u16,
    /// Total text-area width in pixels (0 = unknown / not yet set).
    pub pixel_width: u16,
    /// Total text-area height in pixels (0 = unknown / not yet set).
    pub pixel_height: u16,
}

impl TermSize {
    /// Create a new terminal size.
    pub const fn new(rows: u16, cols: u16, pixel_width: u16, pixel_height: u16) -> Self {
        Self { rows, cols, pixel_width, pixel_height }
    }
}

impl Default for TermSize {
    fn default() -> Self {
        Self::new(24, 80, 0, 0)
    }
}
