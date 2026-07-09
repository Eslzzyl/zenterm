//! Terminal state machine and public API.
//!
//! Wraps [`alacritty_terminal::Term`] + [`vte::ansi::Processor`] and provides
//! methods for feeding bytes, resizing, scrolling, and reading the grid.

use std::sync::{mpsc, Arc};

use alacritty_terminal::event::{Event, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{ClipboardType, Config as TermConfig, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};

use zenterm_core::cell::{Cell, UnderlineStyle};
use zenterm_core::color::Rgba;
use zenterm_core::damage::DamageSet;
use zenterm_core::position::TermPos;
use zenterm_core::size::TermSize;

use super::color_scheme::{named_color_default_rgb, ColorScheme};
use super::grid_view::{CursorInfo, GridView};
use super::listener::Listener;
use super::osc7::scan_osc7;
use super::TermDimensions;

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
    /// Most recent OSC 9 / OSC 777 desktop notification.
    /// Populated by [`Self::feed`]; consumed via [`Self::take_notification`].
    pending_notification: Option<(String, String)>,
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
            pending_notification: None,
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

        // ── OSC 9 / OSC 777 (desktop notification) scan ─────────────────
        if let Some(notif) = scan_osc9_or_777(bytes) {
            self.pending_notification = Some(notif);
        }

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

    /// Return the visible text of a viewport row as a `String`.
    pub fn line_text(&self, row: usize) -> String {
        use alacritty_terminal::index::{Column, Line};
        let cols = self.term.columns();
        let display_offset = self.term.grid().display_offset();
        let grid_line = Line(row as i32 - display_offset as i32);
        let mut text = String::with_capacity(cols);
        for col in 0..cols {
            text.push(self.term.grid()[grid_line][Column(col)].c);
        }
        text
    }

    // ── Scrollback / display offset ─────────────────────────────────────

    /// Scroll the viewport by `count` lines.
    ///
    /// Positive = scroll up (into history), negative = scroll down (toward bottom).
    /// Returns `true` if the display offset actually changed.
    pub fn scroll_display(&mut self, count: i32) -> bool {
        let old = self.term.grid().display_offset();
        self.term.scroll_display(Scroll::Delta(count));
        if self.term.grid().display_offset() != old {
            self.damage.mark_all();
            return true;
        }
        false
    }

    /// Jump to the bottom of the scrollback (latest output).
    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
        self.damage.mark_all();
    }

    /// Jump to the top of the scrollback (oldest history).
    pub fn scroll_to_top(&mut self) {
        self.term.scroll_display(Scroll::Top);
        self.damage.mark_all();
    }

    /// Number of lines currently in scrollback history.
    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    /// Current scroll position. 0 = at bottom, larger = scrolled into history.
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Whether the viewport is at the bottom (showing latest output).
    pub fn is_at_bottom(&self) -> bool {
        self.term.grid().display_offset() == 0
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

        // Clear the damage set — it has been consumed by the re-resolution above.
        self.damage.clear();

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

    /// Take a pending desktop notification (title, body) from OSC 9/777.
    pub fn take_notification(&mut self) -> Option<(String, String)> {
        self.pending_notification.take()
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

        let underline_style = if flags.contains(Flags::DOUBLE_UNDERLINE) {
            UnderlineStyle::Double
        } else if flags.contains(Flags::UNDERCURL) {
            UnderlineStyle::Curly
        } else if flags.contains(Flags::DOTTED_UNDERLINE) {
            UnderlineStyle::Dotted
        } else if flags.contains(Flags::DASHED_UNDERLINE) {
            UnderlineStyle::Dashed
        } else if flags.contains(Flags::UNDERLINE) {
            UnderlineStyle::Normal
        } else {
            UnderlineStyle::None
        };

        Cell {
            c,
            fg: if flags.contains(Flags::INVERSE) { bg } else { fg },
            bg: if flags.contains(Flags::INVERSE) { fg } else { bg },
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
            underline_style,
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

/// Scan for OSC 9 (iTerm2) or OSC 777 (urxvt) desktop notification sequences.
///
/// Recognised forms:
///
/// ```text
/// ESC ] 9 ; body BEL                 (OSC 9 — title = app name, body = text)
/// ESC ] 777 ; notify ; title ; body BEL   (OSC 777 — title + body)
/// ```
///
/// Returns `Some((title, body))` or `None`.
fn scan_osc9_or_777(bytes: &[u8]) -> Option<(String, String)> {
    // Find `ESC ]` introducer (0x1B 0x5D).
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == 0x1B && bytes[i + 1] == b']' {
            let rest = &bytes[i + 2..];
            if rest.starts_with(b"9;") {
                // OSC 9 — body only
                let body = read_osc_string(&rest[2..])?;
                if body.is_empty() {
                    return None;
                }
                return Some(("Zenterm".into(), body));
            }
            if rest.starts_with(b"777;") {
                // OSC 777 — semicolon-separated args
                let payload = read_osc_string(&rest[4..])?;
                let mut parts = payload.splitn(3, ';');
                let _maybe_notify = parts.next(); // "notify"
                let title = parts.next().unwrap_or("").to_string();
                let body = parts.next().unwrap_or("").to_string();
                return Some((title, body));
            }
        }
        i += 1;
    }
    None
}

/// Read bytes from `start` until a BEL (0x07) or ST (ESC \) terminator.
/// Returns `None` if unterminated.
fn read_osc_string(start: &[u8]) -> Option<String> {
    let mut end = 0;
    while end < start.len() {
        if start[end] == 0x07 {
            return std::str::from_utf8(&start[..end]).ok().map(|s| s.to_string());
        }
        if start[end] == 0x1B && end + 1 < start.len() && start[end + 1] == b'\\' {
            return std::str::from_utf8(&start[..end]).ok().map(|s| s.to_string());
        }
        end += 1;
    }
    None
}
