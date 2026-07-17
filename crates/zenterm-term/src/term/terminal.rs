//! Terminal state machine and public API.
//!
//! Wraps [`alacritty_terminal::Term`] + [`vte::ansi::Processor`] and provides
//! methods for feeding bytes, resizing, scrolling, and reading the grid.

use std::collections::HashMap;
use std::sync::{mpsc, Arc};

use alacritty_terminal::event::{Event, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point};

use zenterm_core::image::ImageCell;

use crate::image::kitty::{self, KittyAccumulator, KittyImage};
use crate::image::sixel::{self, SixelBuilder};
use crate::image::{PlacementParams, PlacementStyle, assign_image_to_cells};
use crate::image::ImageCache;
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

    // ── Image protocol state ────────────────────────────────────────────
    pub(crate) image_cache: ImageCache,
    /// Hashes of images that were removed and whose GPU atlas slots need
    /// to be freed.  Drained by the UI layer each frame.
    pub pending_image_deallocations: Vec<[u8; 32]>,
    /// Image placements keyed by grid (line, col) so they follow content
    /// during scroll.  `line` is a grid-relative `Line.0` (may be negative
    /// when viewport is at bottom).
    pub(crate) image_placements: HashMap<(i32, usize), ImageCell>,
    /// Accumulator for multi-chunk Kitty image transmissions.
    #[allow(dead_code)]
    kitty_accumulator: KittyAccumulator,
    /// Cell pixel dimensions (set by the UI layer).
    pub cell_pixel_width: u32,
    pub cell_pixel_height: u32,

    // ── Total text-area pixel dimensions ───────────────────────────
    /// Total text-area width in pixels (set by the UI layer on resize).
    pub pixel_width: u32,
    /// Total text-area height in pixels (set by the UI layer on resize).
    pub pixel_height: u32,

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
            image_cache: ImageCache::new(),
            image_placements: HashMap::new(),
            pending_image_deallocations: Vec::new(),
            kitty_accumulator: KittyAccumulator::default(),
            cell_pixel_width: 0,
            cell_pixel_height: 0,
            pixel_width: size.pixel_width as u32,
            pixel_height: size.pixel_height as u32,
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
        let start = std::time::Instant::now();
        log::debug!("Terminal::feed: {} bytes: {:02x?}", bytes.len(), bytes);

        // Response bytes collected during processing; written back to PTY.
        let mut replies = Vec::new();

        // ── APC / DCS scan ──────────────────────────────────────────────
        // Use memchr to efficiently find ESC bytes (0x1b) that start APC
        // (ESC _ G ... ST) and DCS (ESC P ... ST) sequences, instead of
        // scanning byte-by-byte which is O(n²) in the naive loop.
        let t_apc_start = std::time::Instant::now();
        let esc_positions = memchr::memchr_iter(0x1b, bytes);
        let mut prev_end: Option<usize> = None;
        for esc_pos in esc_positions {
            // Skip positions we've already consumed as part of a prior match.
            if prev_end.is_some_and(|end| esc_pos < end) {
                continue;
            }
            if esc_pos + 2 >= bytes.len() {
                break;
            }
            // Check for APC: ESC _ G
            if bytes[esc_pos + 1] == b'_' && bytes[esc_pos + 2] == b'G' {
                log::debug!(
                    "[img] APC found at offset={}, remaining={} bytes",
                    esc_pos, bytes.len() - esc_pos,
                );
                let payload_start = esc_pos + 2;
                // Find the string terminator ST: ESC \
                if let Some(st_rel) = bytes[payload_start..].windows(2).position(|w| w == [0x1b, b'\\']) {
                    let payload = &bytes[payload_start..payload_start + st_rel];
                    if let Some(cmd) = KittyImage::parse_apc(payload) {
                        log::debug!(
                            "[img] Kitty APC parsed: variant={}, payload_len={}",
                            kitty_cmd_variant_name(&cmd), payload.len(),
                        );
                        let reply = self.handle_kitty_command(cmd);
                        if let Some(r) = reply {
                            log::debug!(
                                "[img] Kitty query response: {} bytes",
                                r.len(),
                            );
                            replies.extend_from_slice(r.as_bytes());
                        }
                    } else {
                        log::warn!(
                            "[img] Kitty APC parse FAILED, first 80 bytes: {:?}",
                            String::from_utf8_lossy(&payload[..payload.len().min(80)]),
                        );
                    }
                    prev_end = Some(payload_start + st_rel + 2);
                }
            }
            // Check for DCS: ESC P (Sixel)
            if bytes[esc_pos + 1] == b'P' {
                let param_start = esc_pos + 2;
                let mut j = param_start;
                while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'q' {
                    let payload_start = j + 1;
                    if let Some(st_rel) = bytes[payload_start..].windows(2).position(|w| w == [0x1b, b'\\']) {
                        let params = sixel::parse_dcs_params(&bytes[param_start..j]);
                        self.handle_sixel(&bytes[payload_start..payload_start + st_rel], &params);
                        prev_end = Some(payload_start + st_rel + 2);
                    }
                }
            }
            // Check for CSI 16 t (Report Cell Size in pixels).
            // vte 0.15.0 does not dispatch param=16 for final byte 't',
            // so we handle it here directly.
            if bytes[esc_pos + 1] == b'[' {
                // ── CSI 2 J : Erase Display — clear image placements ──
                if esc_pos + 3 < bytes.len()
                    && bytes[esc_pos + 2] == b'2'
                    && bytes[esc_pos + 3] == b'J'
                {
                    if !self.image_placements.is_empty() {
                        log::debug!(
                            "[img] CSI 2J (Erase Display): clearing {} image placements",
                            self.image_placements.len(),
                        );
                        self.image_placements.clear();
                    }
                }

                let mut j = esc_pos + 2;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b't' && j > esc_pos + 2 {
                    if let Ok(param_str) = std::str::from_utf8(&bytes[esc_pos + 2..j]) {
                        if param_str == "16" {
                            let cols = self.term.columns();
                            let rows = self.term.screen_lines();
                            let cell_w = if cols > 0 { self.pixel_width / cols as u32 } else { 0 };
                            let cell_h = if rows > 0 { self.pixel_height / rows as u32 } else { 0 };
                            let response = format!("\x1b[6;{};{}t", cell_h, cell_w);
                            log::info!(
                                "[img] CSI 16t response: cell_w={cell_w}, cell_h={cell_h}, pixel={}x{}, grid={}x{}",
                                self.pixel_width, self.pixel_height, cols, rows,
                            );
                            replies.extend_from_slice(response.as_bytes());
                            prev_end = Some(j + 1);
                        }
                    }
                }
            }
        }
        let t_apc_elapsed = t_apc_start.elapsed();

        // ── OSC 9 / OSC 777 (desktop notification) scan ─────────────────
        let t_osc_start = std::time::Instant::now();
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
        let t_osc_elapsed = t_osc_start.elapsed();

        // ── VT parser ───────────────────────────────────────────────────
        let t_vt_start = std::time::Instant::now();
        self.processor.advance(&mut self.term, bytes);
        let t_vt_elapsed = t_vt_start.elapsed();

        // Propagate damage from alacritty_terminal's internal tracker.
        // Each VT operation (write char, cursor move, scroll, etc.)
        // already marks the affected lines — we just read them out.
        let t_damage_start = std::time::Instant::now();
        match self.term.damage() {
            TermDamage::Full => self.damage.mark_all(),
            TermDamage::Partial(iter) => {
                for line in iter {
                    self.damage.mark(line.line);
                }
            }
        }
        self.term.reset_damage();
        let t_damage_elapsed = t_damage_start.elapsed();

        // ── Drain the event channel ────────────────────────────────────
        // The custom `Listener` (above) receives every `Event::PtyWrite`,
        // `ColorRequest`, etc. that the `Handler` emits.  We process them
        // here and return the collected response bytes.
        let t_evt_start = std::time::Instant::now();
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
                    let cols = self.term.columns() as u16;
                    let rows = self.term.screen_lines() as u16;
                    let cell_w = if cols > 0 { (self.pixel_width / cols as u32) as u16 } else { 0 };
                    let cell_h = if rows > 0 { (self.pixel_height / rows as u32) as u16 } else { 0 };
                    let size = WindowSize {
                        num_lines: rows,
                        num_cols: cols,
                        cell_width: cell_w,
                        cell_height: cell_h,
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
        let t_evt_elapsed = t_evt_start.elapsed();

        let elapsed = start.elapsed();
        if elapsed > std::time::Duration::from_millis(50) {
            log::warn!(
                "[perf] Terminal::feed({} bytes) took {:?} (apc_scan={:?} osc_scan={:?} vt_parse={:?} damage={:?} events={:?})",
                bytes.len(), elapsed,
                t_apc_elapsed, t_osc_elapsed, t_vt_elapsed, t_damage_elapsed, t_evt_elapsed,
            );
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
        self.image_placements.clear();
        self.damage.mark_all();
        self.pixel_width = size.pixel_width as u32;
        self.pixel_height = size.pixel_height as u32;
    }

    /// Get the current terminal size (in cells and pixels).
    pub fn size(&self) -> TermSize {
        TermSize::new(
            self.term.screen_lines() as u16,
            self.term.columns() as u16,
            self.pixel_width as u16,
            self.pixel_height as u16,
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

    /// Return the number of active image placements (for diagnostics).
    pub fn image_placements_count(&self) -> usize {
        self.image_placements.len()
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

        // Attach image placements (keyed by grid line) to the grid cache.
        let display_offset = grid.display_offset() as i32;
        for (&(grid_line, col), img_cell) in &self.image_placements {
            let viewport_row = grid_line + display_offset;
            if viewport_row >= 0 && (viewport_row as usize) < self.grid_cache.len() {
                let row = viewport_row as usize;
                if col < self.grid_cache[row].len() {
                    self.grid_cache[row][col].image = Some(img_cell.clone());
                }
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
            image: None,
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

// ── APC / DCS scan helpers ─────────────────────────────────────────────

/// Scan for the next Kitty APC sequence starting at `offset`.
/// Returns `(payload, end_pos)` where `end_pos` is the byte after `\x1b\\`.


// ── Kitty protocol handler ─────────────────────────────────────────────

impl Terminal {
    /// Handle a parsed Kitty image command.
    /// Returns `Some(response_bytes)` for `a=q` queries that must be
    /// written back to the PTY.
    fn handle_kitty_command(&mut self, cmd: KittyImage) -> Option<String> {
        // Feed through the accumulator to support multi-chunk transmissions.
        let assembled = match self.kitty_accumulator.feed(cmd) {
            Ok(Some(assembled)) => assembled,
            Ok(None) => return None, // waiting for more chunks
            Err(e) => {
                log::error!("[img] kitty accumulator error: {e}");
                return None;
            }
        };

        log::debug!(
            "[img] handle_kitty_command: variant={}, cache_images={}, placements={}",
            kitty_cmd_variant_name(&assembled),
            self.image_cache.all_hashes().len(),
            self.image_placements.len(),
        );

        match assembled {
            KittyImage::TransmitData { transmit, verbosity } => {
                log::debug!(
                    "[img] TransmitData: fmt={:?}, w={:?}, h={:?}, id={:?}, num={:?}",
                    transmit.format, transmit.width, transmit.height,
                    transmit.image_id, transmit.image_number,
                );
                if verbosity != kitty::KittyImageVerbosity::Quiet {
                    match kitty::decode_image_data(transmit, &mut self.image_cache) {
                        Ok(id) => log::debug!("[img] TransmitData decode OK, image_id={id}"),
                        Err(e) => log::error!("[img] TransmitData decode FAILED: {e}"),
                    }
                } else {
                    let _ = kitty::decode_image_data(transmit, &mut self.image_cache);
                }
            }
            KittyImage::TransmitDataAndDisplay { transmit, placement, .. } => {
                log::debug!(
                    "[img] TransmitDataAndDisplay: fmt={:?}, w={:?}, h={:?}, id={:?}, num={:?}",
                    transmit.format, transmit.width, transmit.height,
                    transmit.image_id, transmit.image_number,
                );
                match kitty::decode_image_data(transmit, &mut self.image_cache) {
                    Ok(image_id) => {
                        log::debug!("[img] decode OK, image_id={image_id}, calling kitty_place_image");
                        self.kitty_place_image(Some(image_id), None, placement);
                    }
                    Err(e) => log::error!("[img] decode FAILED: {e}"),
                }
            }
            KittyImage::Display { image_id, image_number, placement, .. } => {
                log::debug!("[img] Display: image_id={image_id:?}, num={image_number:?}");
                self.kitty_place_image(image_id, image_number, placement);
            }
            KittyImage::Delete { what, .. } => {
                log::debug!("[img] Delete");
                self.handle_kitty_delete(what);
            }
            KittyImage::Query { transmit } => {
                log::debug!(
                    "[img] Query: id={:?}, num={:?}",
                    transmit.image_id, transmit.image_number,
                );
                // Respond with OK (we support the protocol).
                return Some(kitty::kitty_response(
                    transmit.image_id,
                    transmit.image_number,
                    "OK",
                ));
            }
            KittyImage::TransmitFrame { transmit, frame, .. } => {
                log::debug!("[img] TransmitFrame");
                if let Err(e) = kitty::decode_image_frame(transmit, frame, &mut self.image_cache) {
                    log::error!("[img] frame transmit FAILED: {e}");
                }
            }
            KittyImage::ComposeFrame { frame, .. } => {
                log::debug!("[img] ComposeFrame");
                if let Err(e) = kitty::handle_compose_frame(frame, &mut self.image_cache) {
                    log::error!("[img] compose frame FAILED: {e}");
                }
            }
        }
        None
    }

    fn kitty_place_image(
        &mut self,
        image_id: Option<u32>,
        image_number: Option<u32>,
        placement: kitty::KittyImagePlacement,
    ) {
        let id = self.image_cache.assign_id(image_id, image_number);
        log::debug!(
            "[img] kitty_place_image: resolved_id={id}, image_id={image_id:?}, \
             num={image_number:?}, cell_pixel={}x{}, do_not_move={}",
            self.cell_pixel_width, self.cell_pixel_height,
            placement.do_not_move_cursor,
        );
        let data = match self.image_cache.get(id) {
            Some(d) => d.clone(),
            None => {
                log::error!("[img] kitty place: image id {id} not found in cache");
                return;
            }
        };

        let img_w = data.data().width();
        let img_h = data.data().height();

        if self.cell_pixel_width == 0 || self.cell_pixel_height == 0 {
            log::warn!(
                "[img] kitty_place_image: cell_pixel is 0 ({}x{}), SKIPPING placement",
                self.cell_pixel_width, self.cell_pixel_height,
            );
            return;
        }

        let cursor = self.cursor();
        let cols = self.term.columns();
        let rows = self.term.screen_lines();

        let params = PlacementParams {
            columns: placement.columns.map(|c| c as usize),
            rows: placement.rows.map(|r| r as usize),
            source_x: placement.x,
            source_y: placement.y,
            source_w: placement.w,
            source_h: placement.h,
            cell_padding_left: placement.x_offset.unwrap_or(0) as u16,
            cell_padding_top: placement.y_offset.unwrap_or(0) as u16,
            z_index: placement.z_index.unwrap_or(0),
            do_not_move_cursor: placement.do_not_move_cursor,
            image_id: Some(id),
            placement_id: placement.placement_id,
            style: PlacementStyle::Kitty,
        };

        let result = assign_image_to_cells(
            data,
            img_w,
            img_h,
            &params,
            self.cell_pixel_width,
            self.cell_pixel_height,
            cursor.pos.column,
            cursor.pos.line.min(rows.saturating_sub(1)),
            cols,
            rows,
        );

        // Store placements keyed by grid-relative line so they follow
        // content when the viewport scrolls.
        let display_offset = self.term.grid().display_offset() as i32;
        for (col, viewport_row, cell) in &result.cells {
            // viewport_row is in [0, screen_lines).  Convert to grid line.
            let grid_line = *viewport_row as i32 - display_offset;
            self.image_placements.insert((grid_line, *col), cell.clone());
        }

        let new_cursor = if result.move_cursor {
            let new_col = (cursor.pos.column + result.width_in_cells).min(cols.saturating_sub(1));
            let new_row = (cursor.pos.line + result.height_in_cells)
                .saturating_sub(1)
                .min(rows.saturating_sub(1));
            (new_col, new_row)
        } else {
            (cursor.pos.column, cursor.pos.line)
        };
        log::debug!(
            "[img] placed {} cells ({}x{}), total_placements={}, \
             img={}x{}px, cursor ({},{})→({},{})",
            result.cells.len(), result.width_in_cells, result.height_in_cells,
            self.image_placements.len(),
            img_w, img_h,
            cursor.pos.column, cursor.pos.line,
            new_cursor.0, new_cursor.1,
        );

        if result.move_cursor {
            // Kitty moves cursor to after the bottom-right of the image.
            self.term.grid_mut().cursor.point.column = alacritty_terminal::index::Column(new_cursor.0);
            self.term.grid_mut().cursor.point.line = alacritty_terminal::index::Line(new_cursor.1 as i32);
        }

        self.damage.mark_all();
    }

    fn handle_kitty_delete(&mut self, what: kitty::KittyImageDelete) {
        match what {
            kitty::KittyImageDelete::All { delete } => {
                self.image_placements.clear();
                if delete {
                    // Collect all hashes before clearing for atlas cleanup.
                    let hashes: Vec<[u8; 32]> = self.image_cache.all_hashes();
                    self.pending_image_deallocations.extend(hashes);
                    self.image_cache.clear();
                }
            }
            kitty::KittyImageDelete::ByImageId { image_id, placement_id, delete } => {
                self.image_placements.retain(|_, v| {
                    if v.image_id != Some(image_id) { return true; }
                    placement_id.map_or(false, |p| v.placement_id != Some(p))
                });
                if delete {
                    if let Some(hash) = self.image_cache.remove(image_id) {
                        self.pending_image_deallocations.push(hash);
                    }
                }
            }
            kitty::KittyImageDelete::ByImageNumber { image_number: _, placement_id, delete } => {
                // Look up the image_id from the number mapping.
                // We don't store number_to_id in ImageCache publicly, so for now
                // scan placements by image data hash (approximate).
                // TODO: store number_to_id mapping publicly.
                let ids: Vec<u32> = self.image_placements.iter()
                    .filter(|(_, v)| v.placement_id == placement_id)
                    .map(|(_, v)| v.image_id)
                    .flatten()
                    .collect();
                for id in ids {
                    self.image_placements.retain(|_, v| v.image_id != Some(id));
                    if delete {
                        self.image_cache.remove(id);
                    }
                }
            }
            kitty::KittyImageDelete::AtCursorPosition { delete } => {
                let cursor = self.cursor();
                self.image_placements.retain(|&(line, col), _| {
                    let viewport_row = line + self.term.grid().display_offset() as i32;
                    viewport_row != cursor.pos.line as i32 || col != cursor.pos.column
                });
                if delete {
                    // Can't delete data without knowing the image_id.
                    log::warn!("kitty delete AtCursorPosition with delete=true: image_id unknown");
                }
            }
            kitty::KittyImageDelete::DeleteAt { x, y, delete } => {
                let display_offset = self.term.grid().display_offset() as i32;
                let del_grid_line = y as i32 - display_offset;
                self.image_placements.retain(|&(line, col), _| {
                    !(line == del_grid_line && col == x as usize)
                });
                if delete {
                    log::warn!("kitty delete DeleteAt with delete=true: image_id unknown");
                }
            }
            kitty::KittyImageDelete::DeleteColumn { x, delete: _ } => {
                let display_offset = self.term.grid().display_offset() as i32;
                self.image_placements.retain(|&(line, _), _| {
                    let viewport_row = line + display_offset;
                    viewport_row != x as i32
                });
            }
            kitty::KittyImageDelete::DeleteRow { y, delete: _ } => {
                self.image_placements.retain(|&(_, col), _| col != y as usize);
            }
            kitty::KittyImageDelete::DeleteZ { z, delete: _ } => {
                self.image_placements.retain(|_, v| v.z_index != z);
            }
        }
        self.damage.mark_all();
    }

    /// Handle a sixel image transmission.
    fn handle_sixel(&mut self, payload: &[u8], params: &[i64]) {
        if self.cell_pixel_width == 0 || self.cell_pixel_height == 0 {
            log::warn!("sixel: cell pixel size not set, skipping");
            return;
        }

        let mut builder = SixelBuilder::new(params);
        for &b in payload {
            builder.push(b);
        }
        builder.finish();

        match sixel::render_sixel(&builder.sixel) {
            Ok(data) => {
                let cursor = self.cursor();
                let cols = self.term.columns();
                let rows = self.term.screen_lines();
                let img_w = data.data().width();
                let img_h = data.data().height();

                let par = PlacementParams {
                    columns: None,
                    rows: None,
                    source_x: None,
                    source_y: None,
                    source_w: None,
                    source_h: None,
                    cell_padding_left: 0,
                    cell_padding_top: 0,
                    z_index: 0, // sixel is behind text
                    do_not_move_cursor: false,
                    image_id: None,
                    placement_id: None,
                    style: PlacementStyle::Sixel,
                };

                let result = assign_image_to_cells(
                    data,
                    img_w,
                    img_h,
                    &par,
                    self.cell_pixel_width,
                    self.cell_pixel_height,
                    cursor.pos.column,
                    cursor.pos.line.min(rows.saturating_sub(1)),
                    cols,
                    rows,
                );

                let display_offset = self.term.grid().display_offset() as i32;
                for (col, viewport_row, cell) in &result.cells {
                    let grid_line = *viewport_row as i32 - display_offset;
                    self.image_placements.insert((grid_line, *col), cell.clone());
                }
                self.damage.mark_all();
            }
            Err(e) => log::error!("sixel render: {e}"),
        }
    }
}

// ── Diagnostic helpers ────────────────────────────────────────────────

fn kitty_cmd_variant_name(cmd: &KittyImage) -> &'static str {
    match cmd {
        KittyImage::TransmitData { .. } => "TransmitData",
        KittyImage::TransmitDataAndDisplay { .. } => "TransmitDataAndDisplay",
        KittyImage::Display { .. } => "Display",
        KittyImage::Delete { .. } => "Delete",
        KittyImage::Query { .. } => "Query",
        KittyImage::TransmitFrame { .. } => "TransmitFrame",
        KittyImage::ComposeFrame { .. } => "ComposeFrame",
    }
}
