//! Terminal state machine module.
//!
//! Wraps [`alacritty_terminal::Term`] and bridges raw PTY output into
//! structured grid state with event handling, color scheme resolution,
//! and grid snapshots for rendering.

use alacritty_terminal::grid::Dimensions;

use zenterm_core::size::TermSize;

mod color_scheme;
mod grid_view;
mod listener;
mod osc7;
mod terminal;

pub use color_scheme::ColorScheme;
pub use grid_view::{CursorInfo, GridView};
pub use terminal::Terminal;

// ── Newtype wrapper to implement `Dimensions` for `TermSize` ─────────

pub(crate) struct TermDimensions(TermSize);

impl Dimensions for TermDimensions {
    fn total_lines(&self) -> usize {
        self.0.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.0.rows as usize
    }

    fn columns(&self) -> usize {
        self.0.cols as usize
    }
}
