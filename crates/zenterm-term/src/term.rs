//! High-level terminal wrapper.
//!
//! Owns the `alacritty_terminal::Term` + `vte::ansi::Processor` and bridges
//! bytes from the PTY into grid state.
//!
//! The `vte::ansi::Processor` converts raw byte streams into semantic
//! `Handler` calls on the `Term`, so we do **not** need to implement
//! `vte::Perform` ourselves.
//!
//! # Event handling
//!
//! A custom [`Listener`] replaces the default [`VoidListener`] so that
//! terminal queries (DA, DSR, DECRPM, OSC colour queries, etc.) are
//! properly answered.  Events from the `Handler` are collected via an
//! `mpsc` channel during [`Terminal::feed()`]; response bytes are returned
//! to the caller, and other side effects (title changes, clipboard ops,
//! bell, exit) are stored for the app to consume via `take_*` methods.

use std::fmt;
use std::sync::{mpsc, Arc};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Line, Point};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{ClipboardType, Config as TermConfig, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorStyle, NamedColor, Processor, Rgb};

use zenterm_core::cell::Cell;
use zenterm_core::color::Rgba;
use zenterm_core::damage::DamageSet;
use zenterm_core::position::TermPos;
use zenterm_core::size::TermSize;
use zenterm_core::theme::Theme;

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

// ── Event listener ──────────────────────────────────────────────────────

/// Collects [`Event`]s from the alacritty `Handler` via an `mpsc` channel.
///
/// The channel receiver lives in [`Terminal`] and is drained during
/// [`Terminal::feed()`] so that response bytes can be written back to the
/// PTY and other side-effects (title changes, clipboard operations, bell,
/// exit) can be handled by the application.
struct Listener {
    tx: mpsc::Sender<Event>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if self.tx.send(event).is_err() {
            log::warn!("Terminal event channel closed, dropping event");
        }
    }
}

// ── Colour scheme ───────────────────────────────────────────────────────

/// A resolved colour scheme that maps index-based colours to real RGBA values.
#[derive(Clone)]
pub struct ColorScheme {
    pub colors: Colors,
    /// Selection background colour.
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
        Self::from_theme(&zenterm_core::theme::THEME_DARK)
    }
}

impl ColorScheme {
    /// Build a colour scheme from a [`Theme`].
    ///
    /// Pre-populates the full `Colors` array so that alacritty's named-colour
    /// resolution has values for every standard slot, avoiding fallback to
    /// `named_color_default_rgb`.
    pub fn from_theme(theme: &Theme) -> Self {
        let mut colors = Colors::default();

        // ANSI normal colours (NamedColor::Black .. NamedColor::White = 0..7).
        for (i, c) in theme.ansi_normal.iter().enumerate() {
            colors[i] = Some(rgba_to_rgb(c));
        }
        // ANSI bright colours (NamedColor::BrightBlack .. NamedColor::BrightWhite = 8..15).
        for (i, c) in theme.ansi_bright.iter().enumerate() {
            colors[8 + i] = Some(rgba_to_rgb(c));
        }
        // Foreground / Background / Cursor.
        colors[NamedColor::Foreground as usize] = Some(rgba_to_rgb(&theme.foreground));
        colors[NamedColor::Background as usize] = Some(rgba_to_rgb(&theme.background));
        colors[NamedColor::Cursor as usize] = Some(rgba_to_rgb(&theme.cursor));
        // Dim / Bright foreground.
        colors[NamedColor::DimForeground as usize] = Some(rgba_to_rgb(&theme.dim_foreground));
        colors[NamedColor::BrightForeground as usize] = Some(rgba_to_rgb(&theme.bright_foreground));

        Self {
            colors,
            selection_bg: theme.selection_bg,
            selection_fg: Some(theme.selection_fg),
        }
    }

    /// Rebuild this scheme from a new theme (replaces *all* colours).
    pub fn set_theme(&mut self, theme: &Theme) {
        *self = Self::from_theme(theme);
    }
}

/// Convert our internal `Rgba` to alacritty's `Rgb`.
fn rgba_to_rgb(c: &Rgba) -> Rgb {
    Rgb {
        r: (c.r() * 255.0).round() as u8,
        g: (c.g() * 255.0).round() as u8,
        b: (c.b() * 255.0).round() as u8,
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
    term: Term<Listener>,
    rx: mpsc::Receiver<Event>,
    processor: Processor,
    damage: DamageSet,
    scheme: ColorScheme,
    grid_cache: Vec<Vec<Cell>>,

    // ── Pending side-effects (consumed by the app after each feed()) ────
    pending_title: Option<String>,
    pending_bell: bool,
    pending_exit: bool,
    pending_child_exit: Option<std::process::ExitStatus>,
    pending_clipboard_store: Option<String>,
    pending_clipboard_load: Option<Arc<dyn Fn(&str) -> String + Sync + Send + 'static>>,
    /// Most recent OSC 7 working-directory URL (e.g. `file://host/path`).
    /// Populated by [`Self::feed`] by scanning the input stream for
    /// `\x1b]7;…\x07` / `\x1b]7;…\x1b\\` sequences.  Consumed via
    /// [`Self::take_current_directory`].
    pending_current_directory: Option<String>,
}

impl Terminal {
    /// Create a new terminal with the given dimensions.
    pub fn new(size: TermSize, scheme: ColorScheme) -> Self {
        let config = TermConfig::default();
        let dim = TermDimensions(size);

        // Create the event channel and listener — this replaces the previous
        // `VoidListener` so that terminal queries (DA, DSR, DECRPM, OSC
        // colour queries, …) are properly answered.
        let (tx, rx) = mpsc::channel();
        let listener = Listener { tx };
        let term = Term::new(config, &dim, listener);

        let cols = dim.columns();
        let rows = dim.screen_lines();

        Self {
            term,
            rx,
            processor: Processor::new(),
            damage: DamageSet::new(rows),
            scheme,
            grid_cache: vec![vec![Cell::blank(); cols]; rows],
            pending_title: None,
            pending_bell: false,
            pending_exit: false,
            pending_child_exit: None,
            pending_clipboard_store: None,
            pending_clipboard_load: None,
            pending_current_directory: None,
        }
    }

    /// Feed raw bytes from the PTY into the VT processor.
    ///
    /// The processor calls `Handler` methods on the inner `Term`, updating
    /// grid state.  Damage is propagated from `alacritty_terminal`'s
    /// internal tracking so only changed rows are re-resolved.
    ///
    /// Returns response bytes that the caller **must** write back to the PTY
    /// (terminal query replies such as DA, DSR, DECRPM, OSC colour reports,
    /// clipboard load, …).  Other side-effects (title changes, bell, exit,
    /// clipboard store) are stored internally and can be retrieved via the
    /// `take_*` methods after this call.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        if bytes.is_empty() {
            return Vec::new();
        }
        log::debug!("Terminal::feed: {} bytes: {:02x?}", bytes.len(), bytes);

        // ── OSC 7 (current working directory) scan ──────────────────────
        // alacritty_terminal does not emit an `Event` for OSC 7, so we
        // scan the input stream ourselves.  Many shells (fish, zsh with
        // `set_term_title` patches, bash-preexec, etc.) emit
        //     ESC ] 7 ; file://host/path BEL   (or ESC \)
        // whenever the CWD changes.  We store the *most recent* one.
        if let Some(url) = scan_osc7(bytes) {
            self.pending_current_directory = Some(url);
        }

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

        // ── Drain the event channel ────────────────────────────────────
        // The custom `Listener` (above) receives every `Event::PtyWrite`,
        // `ColorRequest`, etc. that the `Handler` emits.  We process them
        // here and return the collected response bytes.
        let mut replies = Vec::new();
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::PtyWrite(text) => {
                    log::debug!("Terminal::feed: PtyWrite({:?})", text);
                    replies.extend_from_slice(text.as_bytes());
                }
                Event::ColorRequest(index, formatter) => {
                    log::debug!("Terminal::feed: ColorRequest(index={})", index);
                    let colors = self.term.colors();
                    if let Some(rgb) = colors[index] {
                        let response = formatter(rgb);
                        replies.extend_from_slice(response.as_bytes());
                    }
                }
                Event::TextAreaSizeRequest(formatter) => {
                    log::debug!("Terminal::feed: TextAreaSizeRequest");
                    let size = WindowSize {
                        num_lines: self.term.screen_lines() as u16,
                        num_cols: self.term.columns() as u16,
                        cell_width: 0,
                        cell_height: 0,
                    };
                    let response = formatter(size);
                    replies.extend_from_slice(response.as_bytes());
                }
                Event::ClipboardStore(_ty, text) => {
                    log::debug!(
                        "Terminal::feed: ClipboardStore({}, {} bytes)",
                        match _ty {
                            ClipboardType::Clipboard => "clipboard",
                            ClipboardType::Selection => "selection",
                        },
                        text.len(),
                    );
                    self.pending_clipboard_store = Some(text);
                }
                Event::ClipboardLoad(_ty, formatter) => {
                    log::debug!("Terminal::feed: ClipboardLoad");
                    self.pending_clipboard_load = Some(formatter);
                }
                Event::Title(title) => {
                    log::debug!("Terminal::feed: Title({:?})", title);
                    self.pending_title = Some(title);
                }
                Event::ResetTitle => {
                    log::debug!("Terminal::feed: ResetTitle (ignored — keep current title)");
                    // Do NOT overwrite the current title.  Some shells / prompt
                    // frameworks use the title-stack push/pop mechanism
                    // (DECPRA `ESC [ 22 t` / DECRPRA `ESC [ 23 t`) to save
                    // and restore the title around command execution.  If the
                    // stack entry is `None` (the terminal's initial state),
                    // popping it sends `ResetTitle` which would briefly flash
                    // "Zenterm" every time a command finishes.  Ignoring it
                    // lets the last non-ResetTitle value persist.
                }
                Event::Bell => {
                    log::debug!("Terminal::feed: Bell");
                    self.pending_bell = true;
                }
                Event::Exit => {
                    log::debug!("Terminal::feed: Exit");
                    self.pending_exit = true;
                }
                Event::ChildExit(status) => {
                    log::debug!("Terminal::feed: ChildExit({:?})", status);
                    self.pending_child_exit = Some(status);
                }
                Event::CursorBlinkingChange
                | Event::MouseCursorDirty
                | Event::Wakeup => {
                    // These events are handled internally by the term or
                    // are noise that we don't need to act on.
                }
            }
        }

        replies
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

    /// Replace the colour scheme (e.g. when the user switches themes).
    ///
    /// Marks the entire grid as dirty so cells are re-resolved next frame.
    pub fn set_scheme(&mut self, scheme: ColorScheme) {
        self.scheme = scheme;
        self.damage.mark_all();
    }

    /// Get the current colour scheme (for inspection).
    pub fn scheme(&self) -> &ColorScheme {
        &self.scheme
    }

    // ── Pending side-effect accessors ──────────────────────────────────
    //
    // These are populated during [`Self::feed()`] and should be queried by
    // the application after each feed call so it can react to terminal
    // requests that cannot be satisfied by merely writing bytes back to the
    // PTY.

    /// Take a pending window title change, if any.
    pub fn take_title(&mut self) -> Option<String> {
        self.pending_title.take()
    }

    /// Take a pending bell request.
    pub fn take_bell(&mut self) -> bool {
        let val = self.pending_bell;
        self.pending_bell = false;
        val
    }

    /// Take a pending exit request.
    pub fn take_exit(&mut self) -> bool {
        let val = self.pending_exit;
        self.pending_exit = false;
        val
    }

    /// Take a pending child-exit notification.
    pub fn take_child_exit(&mut self) -> Option<std::process::ExitStatus> {
        self.pending_child_exit.take()
    }

    /// Take text that the terminal wants stored in the system clipboard.
    pub fn take_clipboard_store(&mut self) -> Option<String> {
        self.pending_clipboard_store.take()
    }

    /// Take the most recent OSC 7 working-directory URL (if any).
    ///
    /// The value is the raw URL as emitted by the application
    /// (typically `file://host/path` or just `/abs/path`); callers are
    /// responsible for URL-decoding and stripping the host component.
    /// Returns `None` if no new OSC 7 was seen since the last call.
    pub fn take_current_directory(&mut self) -> Option<String> {
        self.pending_current_directory.take()
    }

    /// Take a clipboard-load request.
    ///
    /// The returned closure is a formatter: the application should read the
    /// current system clipboard text and pass it to the closure.  The
    /// closure returns the escape-sequence bytes that must be written back
    /// to the PTY.
    pub fn take_clipboard_load(
        &mut self,
    ) -> Option<Arc<dyn Fn(&str) -> String + Sync + Send + 'static>> {
        self.pending_clipboard_load.take()
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

    /// Default background colour — the resolved `NamedColor::Background`.
    ///
    /// Cells whose `cell.bg` equals this value don't need their own
    /// background quad: the terminal-wide `rect_filled` (or, with
    /// `viewport.transparent(true)`, the OS desktop through a
    /// transparent clear) already covers them.  This is the same
    /// pattern cosmic-term uses in `terminal_box.rs:576`
    /// (`if metadata.bg != default_metadata.bg`).
    pub fn default_bg(&self) -> Rgba {
        self.resolve_color(Color::Named(NamedColor::Background))
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

// ── OSC 7 scanner ──────────────────────────────────────────────────────

/// Find the first OSC 7 sequence in `bytes` and return its URL
/// payload (without the OSC introducer or terminator).
///
/// Recognised forms:
///
/// ```text
/// ESC ] 7 ; <url> BEL         (iTerm2 / most shells)
/// ESC ] 7 ; <url> ESC \       (ECMA-48 string terminator)
/// ```
///
/// Returns `None` if no well-formed OSC 7 is found.  The scan is
/// byte-oriented and intentionally cheap (no regex, no allocation
/// beyond the returned `String`).
fn scan_osc7(bytes: &[u8]) -> Option<String> {
    // Find `ESC ] 7 ;` introducer.
    let mut i = 0;
    while i + 3 < bytes.len() {
        if bytes[i] == 0x1B
            && bytes[i + 1] == b']'
            && bytes[i + 2] == b'7'
            && bytes[i + 3] == b';'
        {
            // Found the start.  Read until BEL or ST.
            let payload_start = i + 4;
            let mut j = payload_start;
            while j < bytes.len() {
                if bytes[j] == 0x07 {
                    // BEL terminator.
                    let payload = &bytes[payload_start..j];
                    if let Ok(s) = std::str::from_utf8(payload) {
                        return Some(s.to_string());
                    }
                    return None;
                }
                if bytes[j] == 0x1B && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                    // ST terminator.
                    let payload = &bytes[payload_start..j];
                    if let Ok(s) = std::str::from_utf8(payload) {
                        return Some(s.to_string());
                    }
                    return None;
                }
                j += 1;
            }
            // Unterminated — give up on this attempt.
            return None;
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod osc7_tests {
    use super::scan_osc7;

    #[test]
    fn parses_bel_terminated() {
        let bytes = b"\x1b]7;file://localhost/Users/me\x07";
        assert_eq!(scan_osc7(bytes).as_deref(), Some("file://localhost/Users/me"));
    }

    #[test]
    fn parses_st_terminated() {
        let bytes = b"\x1b]7;file://h/p\x1b\\";
        assert_eq!(scan_osc7(bytes).as_deref(), Some("file://h/p"));
    }

    #[test]
    fn finds_osc7_among_other_bytes() {
        let bytes = b"hello\x1b[31mred\x1b[0m\x1b]7;file://x/y\x07done";
        assert_eq!(scan_osc7(bytes).as_deref(), Some("file://x/y"));
    }

    #[test]
    fn no_osc7_returns_none() {
        let bytes = b"just normal bytes";
        assert_eq!(scan_osc7(bytes), None);
    }

    #[test]
    fn unterminated_returns_none() {
        let bytes = b"\x1b]7;file://x/y";
        assert_eq!(scan_osc7(bytes), None);
    }
}
