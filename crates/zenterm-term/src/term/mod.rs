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
mod osc;
mod terminal;

/// Maximum number of bytes retained for one unterminated escape sequence.
/// This bounds memory used by malformed or deliberately incomplete PTY data.
pub(crate) const MAX_ESCAPE_SEQUENCE_BYTES: usize = 32 * 1024 * 1024;

pub use color_scheme::ColorScheme;
pub use grid_view::{CursorInfo, GridView};
pub use terminal::{BlinkPolicy, CursorPrefs, Terminal};

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
