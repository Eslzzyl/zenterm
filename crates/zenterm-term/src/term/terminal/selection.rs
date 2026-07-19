use alacritty_terminal::index::{Column, Direction, Line, Point};
use alacritty_terminal::selection::{Selection, SelectionType};

use zenterm_core::color::Rgba;

use super::Terminal;

impl Terminal {
    // ── Selection support ──────────────────────────────────────────────────

    /// Start a new selection at the given viewport position.
    ///
    /// `line` is a viewport row (0 = top).  It is converted to grid
    /// coordinates internally so the selection tracks the correct cells
    /// even when the viewport is scrolled into history.
    pub fn start_selection(&mut self, line: usize, col: usize) {
        let display_offset = self.term.grid().display_offset();
        let grid_line = (line as i32) - (display_offset as i32);
        let point = Point::new(Line(grid_line), Column(col));
        self.term.selection = Some(Selection::new(
            SelectionType::Simple,
            point,
            Direction::Left,
        ));
    }

    /// Extend the current selection to the given viewport position.
    pub fn update_selection(&mut self, line: usize, col: usize) {
        let display_offset = self.term.grid().display_offset();
        let grid_line = (line as i32) - (display_offset as i32);
        if let Some(ref mut sel) = self.term.selection {
            let point = Point::new(Line(grid_line), Column(col));
            sel.update(point, Direction::Left);
        }
    }

    /// Clear the active selection.
    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    /// Check whether a selection is currently active.
    pub fn has_selection(&self) -> bool {
        self.term.selection.is_some()
    }

    /// Check whether a specific cell (in viewport coordinates) is within the selection range.
    pub fn is_selected(&self, line: usize, col: usize) -> bool {
        let range = match self
            .term
            .selection
            .as_ref()
            .and_then(|s| s.to_range(&self.term))
        {
            Some(r) => r,
            None => return false,
        };
        let display_offset = self.term.grid().display_offset();
        let grid_line = (line as i32) - (display_offset as i32);
        let point = Point::new(Line(grid_line), Column(col));
        range.contains(point)
    }

    /// Extract selected text as a `String`, if any selection is active.
    pub fn selected_text(&self) -> Option<String> {
        self.term.selection_to_string()
    }

    /// Return the raw selection range, if any, so callers can check
    /// cell membership without an extra `&self` borrow.
    pub fn selection_range(&self) -> Option<alacritty_terminal::selection::SelectionRange> {
        self.term
            .selection
            .as_ref()
            .and_then(|s| s.to_range(&self.term))
    }

    /// Selection background colour (RGBA).
    pub fn selection_bg(&self) -> Rgba {
        self.scheme.selection_bg
    }

    /// Selection foreground colour, if configured.  `None` means keep fg.
    pub fn selection_fg(&self) -> Option<Rgba> {
        self.scheme.selection_fg
    }
}
