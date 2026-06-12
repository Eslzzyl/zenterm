//! High-level terminal wrapper.
//!
//! Owns the `alacritty_terminal::Term` + `vte::ansi::Processor` and bridges
//! bytes from the PTY into grid state.
//!
//! The `vte::ansi::Processor` converts raw byte streams into semantic
//! `Handler` calls on the `Term`, so we do **not** need to implement
//! `vte::Perform` ourselves.

use std::fmt;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Line, Point};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config as TermConfig, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorStyle, NamedColor, Processor, Rgb};

use zenmux_core::cell::Cell;
use zenmux_core::color::Rgba;
use zenmux_core::damage::DamageSet;
use zenmux_core::position::TermPos;
use zenmux_core::size::TermSize;

// ── Newtype wrapper to implement `Dimensions` for `TermSize` ─────────

struct TermDimensions(TermSize);

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

// ── Colour scheme ───────────────────────────────────────────────────────

/// A resolved colour scheme that maps index-based colours to real RGBA values.
#[derive(Clone)]
pub struct ColorScheme {
    pub colors: Colors,
    /// Selection background colour.  Defaults to a blue-ish tint.
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
        Self {
            colors: Colors::default(),
            selection_bg: Rgba::from_u8(60, 100, 180, 255),
            selection_fg: None,
        }
    }
}

// ── Cursor info ─────────────────────────────────────────────────────────

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
    rows: &'a [Vec<Cell>],
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

// ── Terminal state machine ──────────────────────────────────────────────

/// The terminal state machine.
///
/// Owns `alacritty_terminal::Term` for grid state and `vte::ansi::Processor`
/// for byte processing.
pub struct Terminal {
    term: Term<VoidListener>,
    processor: Processor,
    damage: DamageSet,
    scheme: ColorScheme,
    grid_cache: Vec<Vec<Cell>>,
}

impl Terminal {
    /// Create a new terminal with the given dimensions.
    pub fn new(size: TermSize, scheme: ColorScheme) -> Self {
        let config = TermConfig::default();
        let dim = TermDimensions(size);
        let term = Term::new(config, &dim, VoidListener);

        let cols = dim.columns();
        let rows = dim.screen_lines();

        Self {
            term,
            processor: Processor::new(),
            damage: DamageSet::new(rows),
            scheme,
            grid_cache: vec![vec![Cell::blank(); cols]; rows],
        }
    }

    /// Feed raw bytes from the PTY into the VT processor.
    ///
    /// The processor calls `Handler` methods on the inner `Term`, updating
    /// grid state.  Damage is propagated from `alacritty_terminal`'s
    /// internal tracking so only changed rows are re-resolved.
    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        log::debug!("Terminal::feed: {} bytes: {:02x?}", bytes.len(), bytes);
        self.processor.advance(&mut self.term, bytes);

        // Propagate damage from alacritty_terminal's internal tracker.
        // Each VT operation (write char, cursor move, scroll, etc.)
        // already marks the affected lines — we just read them out.
        match self.term.damage() {
            TermDamage::Full => self.damage.mark_all(),
            TermDamage::Partial(iter) => {
                for line in iter {
                    self.damage.mark(line.line);
                }
            }
        }
        self.term.reset_damage();
    }

    /// Resize the terminal grid.
    pub fn resize(&mut self, size: TermSize) {
        let dim = TermDimensions(size);
        let cols = dim.columns();
        let rows = dim.screen_lines();

        self.term.resize(dim);
        self.damage.resize(rows);
        self.grid_cache.resize(rows, vec![Cell::blank(); cols]);
        for row in self.grid_cache.iter_mut() {
            row.resize(cols, Cell::blank());
        }
        self.damage.mark_all();
    }

    /// Get the current terminal size (in cells).
    pub fn size(&self) -> TermSize {
        TermSize::new(
            self.term.screen_lines() as u16,
            self.term.columns() as u16,
        )
    }

    /// Get a view of the visible grid with resolved colours.
    ///
    /// Only dirty rows are re-converted; clean rows come from the cache.
    pub fn visible_cells(&mut self) -> GridView<'_> {
        let cols = self.term.columns();
        let screen_lines = self.term.screen_lines();

        // Collect dirty row indices first to avoid borrow conflicts.
        let dirty: Vec<usize> = self.damage.iter().collect();
        let grid = self.term.grid();

        for &row_idx in &dirty {
            if row_idx >= screen_lines {
                continue;
            }
            let grid_line = Line(row_idx as i32 - grid.display_offset() as i32);
            for col_idx in 0..cols.min(self.grid_cache[row_idx].len()) {
                let alacell = &grid[grid_line][Column(col_idx)];
                self.grid_cache[row_idx][col_idx] = self.resolve_cell(alacell);
            }
        }

        GridView {
            rows: &self.grid_cache[..screen_lines.min(self.grid_cache.len())],
        }
    }

    /// Drain the current damage set (marking everything clean).
    pub fn drain_damage(&mut self) -> DamageSet {
        let mut ds = DamageSet::new(self.term.screen_lines());
        std::mem::swap(&mut ds, &mut self.damage);
        ds
    }

    /// Get cursor information.
    pub fn cursor(&self) -> CursorInfo {
        let point = self.term.grid().cursor.point;
        // Convert from absolute grid line to viewport row so the
        // caller can compare directly with visual row indices.
        let display_offset = self.term.grid().display_offset();
        let viewport_line = point.line.0 + display_offset as i32;
        CursorInfo {
            pos: TermPos::new(viewport_line.max(0) as usize, point.column.0),
            style: self.term.cursor_style(),
            visible: self.term.mode().contains(TermMode::SHOW_CURSOR),
        }
    }

    /// Get terminal mode flags (needed by the input mapper).
    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    // ── Selection support ──────────────────────────────────────────────────

    /// Start a new selection at the given grid position.
    pub fn start_selection(&mut self, line: usize, col: usize) {
        let point = Point::new(Line(line as i32), Column(col));
        self.term.selection = Some(Selection::new(
            SelectionType::Simple,
            point,
            Direction::Left,
        ));
    }

    /// Extend the current selection to the given grid position.
    pub fn update_selection(&mut self, line: usize, col: usize) {
        if let Some(ref mut sel) = self.term.selection {
            let point = Point::new(Line(line as i32), Column(col));
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

    /// Check whether a specific cell is within the selection range.
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
        let point = Point::new(Line(line as i32), Column(col));
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

    // ---- Helpers ----

    fn resolve_cell(&self, alacell: &alacritty_terminal::term::cell::Cell) -> Cell {
        let c = alacell.c;
        let fg = self.resolve_color(alacell.fg);
        let bg = self.resolve_color(alacell.bg);
        let flags = alacell.flags;

        Cell {
            c,
            fg: if flags.contains(Flags::INVERSE) { bg } else { fg },
            bg: if flags.contains(Flags::INVERSE) { fg } else { bg },
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
            underline: flags.contains(Flags::UNDERLINE),
            strikethrough: flags.contains(Flags::STRIKEOUT),
            inverse: flags.contains(Flags::INVERSE),
            dim: flags.contains(Flags::DIM),
            hidden: flags.contains(Flags::HIDDEN),
            is_spacer: flags.contains(Flags::WIDE_CHAR_SPACER),
        }
    }

    fn resolve_color(&self, color: Color) -> Rgba {
        match color {
            Color::Named(named) => {
                let rgb = self.scheme.colors[named]
                    .unwrap_or_else(|| named_color_default_rgb(named));
                Rgba::from_u8(rgb.r, rgb.g, rgb.b, 255)
            }
            Color::Spec(rgb) => Rgba::from_u8(rgb.r, rgb.g, rgb.b, 255),
            Color::Indexed(idx) => self.scheme.colors[idx as usize]
                .map(|rgb| Rgba::from_u8(rgb.r, rgb.g, rgb.b, 255))
                .unwrap_or(Rgba::WHITE),
        }
    }
}

fn named_color_default_rgb(named: NamedColor) -> Rgb {
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
