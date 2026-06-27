//! A single terminal session: one PTY, one VT state machine, one
//! per-session render slice, one `CallbackHandle`.
//!
//! This struct used to live inline in [`crate::app::ZentermApp`].
//! When `config.ui.tabs_enabled = true`, multiple `TerminalSession`s
//! coexist — each in its own dock tab — and share the
//! [`SharedGpuContext`], [`SharedGlyphAtlas`], and
//! [`SharedRenderState`](zenterm_render::callback::SharedRenderState).
//!
//! # Rendering contract
//!
//! The render pipeline is unchanged from Phase 1:
//!
//! 1. [`TerminalSession::draw`] is called from the egui UI thread with
//!    the per-session `Ui`.  It builds a `Vec<CellInstance>` describing
//!    the visible cells in **clip space** (NDC, range -1..1).
//! 2. Each instance is positioned relative to the **dock viewport**,
//!    not the local session rect.  This is what allows the GPU to draw
//!    every tab in a single instanced call: a session that lives at
//!    dock pixel `(200, 0)` simply adds 200 to all of its cell
//!    `x_px` values before the clip-space conversion.
//! 3. After all sessions have been visited, the concatenated buffer is
//!    handed to the wgpu callback via the shared
//!    `SharedRenderState.instances`.  The callback draws everything
//!    with the existing instanced-quad pipeline — **no shader change
//!    is required**.
//!
//! # Side-effects
//!
//! OSC 7 (`\x1b]7;file://…\x07`) is parsed to update
//! [`TerminalSession::cwd`]; OSC 0/2 update the title used by the
//! dock tab and (legacy path) the window title.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;
use serde::{Deserialize, Serialize};

use zenterm_core::color::Rgba;
use zenterm_core::size::TermSize;
use zenterm_glyph::GlyphContentType;
use zenterm_pty::PtySession;
use zenterm_render::callback::CallbackHandle;
use zenterm_render::glyph_type;
use zenterm_render::CellInstance;
use zenterm_term::{ColorScheme, GridView, Terminal};

use crate::glyph_cache::SharedGlyphAtlas;
use crate::gpu::SharedGpuContext;

// ── SessionId ──────────────────────────────────────────────────────────

/// Unique identifier for a terminal session within an application
/// process.  Monotonically increasing; the next id is allocated by
/// the dock state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(pub u64);

impl SessionId {
    pub const fn new(id: u64) -> Self { Self(id) }
    pub const fn raw(self) -> u64 { self.0 }
}

// ── Notification state placeholder ─────────────────────────────────────

/// Per-session notification badge state.  Resolved from OSC 9 / OSC 99
/// / OSC 777 escape sequences.  Phase 2.4 (per `roadmap.md`) will
/// expand this with text payloads, timestamps, and click handlers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NotificationState {
    #[default]
    None,
    Bell,
    Pending,
}

// ── Ligature run detection ────────────────────────────────────────────

/// Find the end column of a consecutive same-style run starting at `(row, col)`.
///
/// A run is a group of cells whose characters should be shaped together
/// as a single string.  When ligature shaping is enabled, the run text
/// is passed to [`GlyphAtlas::shape_and_rasterize_run`] so that
/// OpenType ligature rules (`liga`/`clig`) can substitute multi-cell
/// glyphs (e.g. `->` → one arrow glyph).
///
/// Run boundaries occur at:
///
/// * **End of row** — no more cells.
/// * **Space character** — spaces never participate in ligatures.
/// * **Spacer cell** — a CJK / emoji wide-character continuation.
/// * **Hidden cell** — invisible content should not be shaped.
/// * **Style change** — different `bold` or `italic` flags require
///   separate shaping with different [`cosmic_text::Attrs`].
///
/// # Current behaviour
///
/// Every single character forms its own run (`run_end = col + 1`).
/// When ligature shaping is implemented, multi-character runs will
/// be detected and returned here.
fn detect_run_end(
    grid: &GridView,
    row: usize,
    start_col: usize,
    _cols: usize, // FUTURE: used in multi-run detection loop
) -> usize {
    let first = match grid.cell(row, start_col) {
        Some(c) => c,
        None => return start_col + 1,
    };

    // ── Fast path (non-ligature) ──────────────────────────────────
    // Without actual ligature shaping, every character is an
    // independent run.  The loop below is the placeholder for
    // multi-char run detection.
    //
    // FUTURE: uncomment the loop and remove the early return.
    let _ = first;
    start_col + 1

    // FUTURE (ligature shaping):
    // let mut col = start_col + 1;
    // while col < _cols {
    //     let cell = match grid.cell(row, col) {
    //         Some(c) => c,
    //         None => break,
    //     };
    //     if cell.c == ' ' || cell.is_spacer || cell.hidden {
    //         break;
    //     }
    //     if cell.bold != first.bold || cell.italic != first.italic {
    //         break;
    //     }
    //     col += 1;
    // }
    // col
}

// ── TerminalSession ────────────────────────────────────────────────────

/// All state and behaviour for a single terminal session.
pub struct TerminalSession {
    // ── Identity ─────────────────────────────────────────────────────
    pub id: SessionId,
    pub title: String,
    pub cwd: Option<PathBuf>,
    pub git_branch: Option<String>,
    pub notification: NotificationState,

    // ── Per-session state ───────────────────────────────────────────
    pub terminal: Terminal,
    pub pty: PtySession,

    // ── Shared resources (Arc, owned by the app) ────────────────────
    gpu: SharedGpuContext,
    pub atlas: Arc<SharedGlyphAtlas>,
    pub callback: CallbackHandle,

    // ── Cell metrics ─────────────────────────────────────────────────
    pub cell_width: f32,
    pub cell_height: f32,

    // ── Viewport tracking (last dock viewport we rendered for) ───────
    pub last_vp_size_px: [f32; 2],
    pub last_vp_origin_px: [f32; 2],

    // ── Dock-area viewport (single callback coordinate system) ────────
    pub dock_vp_origin_px: [f32; 2],
    pub dock_vp_size_px: [f32; 2],

    // ── Per-session flags ───────────────────────────────────────────
    pub selecting: bool,
    pub terminal_dirty: bool,
    pub last_resize_at: Option<f64>,
    pub frame_count: u64,
    pub blink_interval: u64,
    pub pty_exited: bool,
    /// Whether we have already emitted [`SessionEffect::CloseWindow`] for
    /// this session.  Guards against repeated emissions across frames.
    pub exit_effect_sent: bool,

    // ── Theming ─────────────────────────────────────────────────────
    pub default_bg: egui::Color32,

    // ── Cell-instance cache (avoids full rebuild when terminal is idle) ──
    cached_bg: Vec<CellInstance>,
    cached_glyph: Vec<CellInstance>,
    cached_deco: Vec<CellInstance>,

    /// ── Title debounce ──────────────────────────────────────────────────
    ///
    /// Some shells (fish, zsh with plugins) send a transient title event
    /// (e.g. the command name "ls") just before executing a command, and
    /// then the real prompt title (e.g. "~") shortly after.  Without
    /// debouncing, both reach the UI as separate frames, causing a visible
    /// flicker.
    ///
    /// We buffer the incoming title and only apply it once it has been
    /// stable for [`TITLE_DEBOUNCE_MS`].
    pending_title: Option<(String, Instant)>,
}

/// Debounce period for window/tab title updates (milliseconds).
///
/// Shells like fish send a transient title (the command name) just before
/// executing a command, then the real prompt title shortly after.  Without
/// debouncing both reach the UI as separate frames, causing a visible
/// flicker.  This value should be longer than the typical gap between the
/// pre-exec and post-exec title events (usually < 20 ms on a local PTY).
const TITLE_DEBOUNCE_MS: f64 = 80.0;

impl TerminalSession {
    /// Construct a new session: spawn a PTY, initialise the terminal,
    /// measure cell geometry, and wire the wgpu callback.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: SessionId,
        size: TermSize,
        scheme: ColorScheme,
        blink_interval: u64,
        default_bg: egui::Color32,
        gpu: SharedGpuContext,
        atlas: Arc<SharedGlyphAtlas>,
        callback: CallbackHandle,
    ) -> Self {
        let pty = PtySession::spawn(size).expect("failed to spawn PTY");
        let terminal = Terminal::new(size, scheme);

        let (cell_width, cell_height) = atlas.cell_size();

        // Initialise `last_vp_size_px` so the first render picks up the
        // resize correctly.  Starting at [0, 0] is fine; the first
        // `update_cell_instances` call will overwrite it.
        Self {
            id,
            title: format!("shell-{}", id.0),
            cwd: None,
            git_branch: None,
            notification: NotificationState::None,
            terminal,
            pty,
            gpu,
            atlas,
            callback,
            cell_width,
            cell_height,
            last_vp_size_px: [0.0, 0.0],
            last_vp_origin_px: [0.0, 0.0],
            dock_vp_origin_px: [0.0, 0.0],
            dock_vp_size_px: [0.0, 0.0],
            selecting: false,
            terminal_dirty: true,
            last_resize_at: None,
            frame_count: 0,
            blink_interval,
            pty_exited: false,
            exit_effect_sent: false,
            default_bg,
            cached_bg: Vec::new(),
            cached_glyph: Vec::new(),
            cached_deco: Vec::new(),
            pending_title: None,
        }
    }

    // ── Viewport (dock) helpers ─────────────────────────────────────

    /// Update the session's tracked viewport.  Called by the
    /// `TabViewer::ui` implementation before the session draws.
    pub fn set_viewport(&mut self, origin_px: [f32; 2], size_px: [f32; 2]) {
        if self.last_vp_origin_px != origin_px || self.last_vp_size_px != size_px {
            self.last_vp_origin_px = origin_px;
            self.last_vp_size_px = size_px;
            self.terminal_dirty = true;
        }
    }

    /// Set the dock-area viewport for the single-callback coordinate
    /// system.  All sessions share the same dock viewport; cell clip
    /// positions are computed relative to this rect so a single wgpu
    /// callback can render every tab.
    ///
    /// Must be called before `update_cell_instances` each frame.
    pub fn set_dock_viewport(&mut self, origin_px: [f32; 2], size_px: [f32; 2]) {
        if self.dock_vp_origin_px != origin_px || self.dock_vp_size_px != size_px {
            self.dock_vp_origin_px = origin_px;
            self.dock_vp_size_px = size_px;
            self.terminal_dirty = true;
        }
    }

    // ── PTY pump & side-effects ──────────────────────────────────────

    /// Drain pending PTY bytes into the terminal state machine, write
    /// terminal-query responses back to the PTY, and detect shell exit
    /// (the latter is required for Windows ConPTY where the output
    /// pipe is not closed on child exit).
    pub fn pump_pty(&mut self) {
        if self.pty_exited {
            return;
        }
        let mut total = 0usize;
        while let Some(result) = self.pty.try_read() {
            match result {
                Ok(data) => {
                    total += data.len();
                    let replies = self.terminal.feed(&data);
                    if !replies.is_empty() {
                        log::debug!(
                            "pump_pty: writing {} reply bytes: {:02x?}",
                            replies.len(),
                            &replies
                        );
                        if let Err(e) = self.pty.write(&replies) {
                            log::error!("failed to write pty reply: {e}");
                        }
                    }
                }
                Err(e) => {
                    log::info!("PTY session ended ({e}), exiting");
                    self.pty_exited = true;
                    self.pty.close();
                    break;
                }
            }
        }
        if total > 0 {
            log::debug!("pump_pty: read {} bytes from PTY", total);
            self.terminal_dirty = true;
        }

        if !self.pty_exited {
            if let Some(status) = self.pty.try_wait() {
                log::info!("shell exited with status: {status:?}, closing");
                self.pty.close();
                self.pty_exited = true;
            }
        }
    }

    /// Apply the side-effects produced by [`Self::pump_pty`]:
    /// window title, bell, exit, clipboard store/load, **OSC 7 cwd**.
    ///
    /// Returns `Some(side_effect)` events the caller must handle
    /// (currently: `WindowTitle` for the eframe viewport command,
    /// `CloseWindow` for shell-initiated exit).
    pub fn handle_side_effects(
        &mut self,
        egui_ctx: &egui::Context,
    ) -> Vec<SessionEffect> {
        let mut effects = Vec::new();

        // Buffer incoming title event (don't apply yet — wait for stability).
        if let Some(title) = self.terminal.take_title() {
            log::trace!("session: title event '{:?}' (debouncing)", title);
            self.pending_title = Some((title, Instant::now()));
        }

        // Apply pending title if it has been stable long enough.
        if let Some((title, at)) = &self.pending_title {
            if at.elapsed().as_secs_f64() * 1000.0 >= TITLE_DEBOUNCE_MS {
                if self.title != *title {
                    log::debug!("session: window title changed: {:?} -> {:?}", self.title, title);
                    self.title = title.clone();
                    effects.push(SessionEffect::WindowTitle(title.clone()));
                } else {
                    log::trace!("session: window title unchanged ({:?}), skipping", self.title);
                }
                self.pending_title = None;
            }
        }

        if self.terminal.take_bell() {
            log::debug!("update: bell");
            self.notification = NotificationState::Bell;
        }

        if !self.exit_effect_sent {
            if self.terminal.take_exit() || self.terminal.take_child_exit().is_some() {
                log::info!("update: terminal requested exit, closing");
                self.pty_exited = true;
            }
            if self.pty_exited {
                log::info!("handle_side_effects: session exited, emitting CloseWindow");
                self.exit_effect_sent = true;
                effects.push(SessionEffect::CloseWindow);
            }
        }

        if let Some(text) = self.terminal.take_clipboard_store() {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                if let Err(e) = cb.set_text(text) {
                    log::error!("failed to store clipboard text: {e}");
                }
            }
        }

        if let Some(formatter) = self.terminal.take_clipboard_load() {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                match cb.get_text() {
                    Ok(text) => {
                        let seq = formatter(&text);
                        if let Err(e) = self.pty.write(seq.as_bytes()) {
                            log::error!("failed to write clipboard-load response: {e}");
                        }
                    }
                    Err(e) => {
                        log::error!("failed to read clipboard for terminal: {e}");
                    }
                }
            }
        }

        // ── OSC 7: working directory (current working directory URL) ──
        if let Some(url) = self.terminal.take_current_directory() {
            if let Some(path) = osc7_url_to_path(&url) {
                self.cwd = Some(path);
            }
        }

        let _ = egui_ctx; // kept for future per-session inputs
        effects
    }

    /// Send an SGR mouse event to the PTY.
    pub fn send_sgr_mouse(&mut self, row: usize, col: usize, button: u8, release: bool) {
        let mode = self.terminal.mode();
        let mouse_active = mode.contains(TermMode::SGR_MOUSE)
            && mode.intersects(
                TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION,
            );
        if !mouse_active {
            return;
        }
        let suffix = if release { "m" } else { "M" };
        let seq = format!("\x1b[{};{};{}{}", row + 1, col + 1, button, suffix);
        if let Err(e) = self.pty.write(seq.as_bytes()) {
            log::error!("SGR mouse write error: {e}");
        }
    }

    /// Re-initialise the (shared) glyph atlas and cell metrics for a
    /// new DPI scale factor.  Called when the window moves between
    /// monitors with different DPI settings.
    pub fn reinit_for_dpi(&mut self, new_ppp: f32, ligatures_enabled: bool) {
        let new_font_size = self.config_font_size() * new_ppp;
        let font_family = std::borrow::Cow::Owned(self.config_font_family());
        let (cw, ch) = self.atlas.reinit_for_dpi(
            new_font_size,
            font_family,
            new_ppp,
            zenterm_core::SubpixelLayout::detect(),
            ligatures_enabled,
        );
        self.atlas.seed_ascii();
        // Ensure the seeded atlas reaches the GPU before the next prepare().
        self.atlas.sync_to_gpu();
        self.cell_width = cw;
        self.cell_height = ch;
        self.terminal_dirty = true;
        log::info!(
            "DPI reinit: session={} new_ppp={new_ppp:.2} font_size={new_font_size:.1} \
             cw={cw:.1} ch={ch:.1}",
            self.id.0
        );
    }

    /// Forward `apply_config_change`-style updates to per-session state.
    pub fn apply_config_change(&mut self, font_size: f32, blink_interval: u64) {
        if blink_interval != self.blink_interval {
            self.blink_interval = blink_interval;
        }
        // Font size changes that don't cross a DPI threshold are
        // ignored here: `reinit_for_dpi` handles the physical rebuild.
        let _ = font_size;
    }

    /// Read the configured font size (the session does not own a
    /// `Config`; the parent `ZentermApp` injects values via the
    /// `apply_config_change` method).
    fn config_font_size(&self) -> f32 {
        // Conservative fallback: a real implementation would
        // re-thread the Config through to the session.  For now,
        // the parent calls `reinit_for_dpi` directly when the config
        // changes; `apply_config_change` is the lightweight path.
        18.0
    }
    fn config_font_family(&self) -> String {
        "monospace".to_string()
    }

    // ── Per-session rendering ────────────────────────────────────────

    /// Resize the terminal to fit a dock-relative pixel area.
    pub fn resize_to_viewport(&mut self, size_px: [f32; 2], ppp: f32) {
        let vp_width_px = size_px[0];
        let vp_height_px = size_px[1];
        if vp_width_px <= 0.0 || vp_height_px <= 0.0 {
            return;
        }
        let cols = (vp_width_px / self.cell_width).max(10.0) as u16;
        let rows = (vp_height_px / self.cell_height).max(5.0) as u16;
        let new_size = TermSize::new(rows, cols);
        if new_size != self.terminal.size() {
            self.terminal.resize(new_size);
            self.pty.resize(new_size).ok();
            self.terminal_dirty = true;
            self.last_resize_at = Some(/* time */ 0.0); // set by caller
        }
        let _ = ppp;
    }

    /// Build GPU instance data for this session's visible cells and
    /// append it to the shared `SharedRenderState.instances` buffer.
    ///
    /// `origin_px` is the session's top-left corner in screen pixels
    /// (relative to the window origin).  `size_px` is the session's
    /// pixel size (used for resize detection, but not for clip-space
    /// conversion — that uses the shared dock viewport set via
    /// [`set_dock_viewport`]).
    ///
    /// Because all sessions share the same dock viewport, the clip-space
    /// coordinates produced here are valid for a single wgpu callback
    /// whose viewport covers the entire dock area.
    ///
    /// Returns `true` if instances were added.
    pub fn update_cell_instances(
        &mut self,
        origin_px: [f32; 2],
        size_px: [f32; 2],
    ) -> bool {
        let vp_width_px = size_px[0];
        let vp_height_px = size_px[1];
        if vp_width_px <= 0.0 || vp_height_px <= 0.0 {
            return false;
        }

        // Clip-space conversion uses the DOCK viewport (the union of
        // all tab rects) so a single wgpu callback can render every
        // tab.  The per-tab `origin_px` offsets each cell to its
        // correct screen position within the dock coordinate system.
        //
        //   dock_clip_x = (dock_px - dock_origin) * 2 / dock_size - 1
        //
        // where dock_px = tab_origin + local_cell_px.
        let dock_w = self.dock_vp_size_px[0];
        let dock_h = self.dock_vp_size_px[1];
        let dock_ox = self.dock_vp_origin_px[0];
        let dock_oy = self.dock_vp_origin_px[1];
        if dock_w <= 0.0 || dock_h <= 0.0 {
            return false;
        }
        let x_scale = 2.0 / dock_w;
        let y_scale = 2.0 / dock_h;

        // How far this session's top-left is from the dock origin.
        let x_off = origin_px[0] - dock_ox;
        let y_off = origin_px[1] - dock_oy;

        // Fast path: terminal content hasn't changed — reuse the
        // cached cell instances from the previous frame.  Cursor
        // blinking already sets `terminal_dirty = true` every
        // blink tick (see `app.rs`), so the cursor animation still
        // works correctly.
        if !self.terminal_dirty {
            let has_instances = !self.cached_bg.is_empty()
                || !self.cached_glyph.is_empty()
                || !self.cached_deco.is_empty();
            if has_instances {
                let mut buf = self
                    .gpu
                    .shared
                    .instances
                    .lock()
                    .expect("SharedRenderState.instances poisoned");
                buf.extend(&self.cached_bg);
                buf.extend(&self.cached_glyph);
                buf.extend(&self.cached_deco);
            }
            return has_instances;
        }

        let mut atlas = self.atlas.lock();
        let tex_size = atlas.texture_size as f32;
        let cw = self.cell_width;
        let ch = self.cell_height;

        // Read cursor info BEFORE visible_cells() since both borrow
        // self.terminal (one mut, one immut).
        let cursor = self.terminal.cursor();
        let cursor_row = cursor.pos.line;
        let cursor_col = cursor.pos.column;

        let blink_on = if cursor.style.blinking
            && !matches!(cursor.style.shape, CursorShape::Block)
        {
            (self.frame_count / self.blink_interval) % 2 == 0
        } else {
            true
        };
        let cursor_visible = cursor.visible && blink_on;
        let cursor_shape = cursor.style.shape;

        let sel_range: Option<SelectionRange> = self.terminal.selection_range();
        let sel_bg = self.terminal.selection_bg();
        let sel_fg = self.terminal.selection_fg();
        let default_bg = self.terminal.default_bg();

        let grid = self.terminal.visible_cells();
        let rows = grid.row_count();
        let cols = grid.col_count();
        if rows == 0 || cols == 0 {
            return false;
        }

        let baseline = atlas.cell_baseline_offset();
        let mut bg_instances: Vec<CellInstance> = Vec::with_capacity(rows * cols);
        let mut glyph_instances: Vec<CellInstance> = Vec::with_capacity(rows * cols);
        let mut deco_instances: Vec<CellInstance> = Vec::with_capacity(rows * cols);
        let mut has_new_glyphs = false;

        for row in 0..rows {
            // ── Per-row: consecutive-cells "run" iterator ──────────────
            //
            // Instead of a simple `for col in 0..cols`, we use a `while`
            // loop and detect runs via `detect_run_end`.  This prepares
            // the renderer for ligature shaping: when enabled, a run of
            // multiple same-style characters will be shaped as a single
            // string, and the resulting glyphs (which may span multiple
            // cells) are distributed across the run's cells.
            //
            // For now, each run is exactly one cell wide, so the
            // per-character behaviour is identical to the old nested
            // for loop.
            let mut col = 0;
            while col < cols {
                let cell = match grid.cell(row, col) {
                    Some(c) => c,
                    None => { col += 1; continue; },
                };

                let is_cursor = cursor_visible && row == cursor_row && col == cursor_col;
                let is_block_cursor =
                    is_cursor && matches!(cursor_shape, CursorShape::Block);

                let is_sel = sel_range.as_ref().is_some_and(|range| {
                    let pt = Point::new(Line(row as i32), Column(col));
                    range.contains(pt)
                });

                let ch_char = cell.c;
                let is_blank = ch_char == ' ' && cell.bg == Rgba::BLACK && !is_cursor;

                let (draw_fg, draw_bg) = if is_block_cursor {
                    (cell.bg, cell.fg)
                } else {
                    (cell.fg, cell.bg)
                };

                // SGR 2 (dim): reduce foreground brightness by half.
                let draw_fg = if cell.dim {
                    Rgba::new(
                        draw_fg.r() * 0.5,
                        draw_fg.g() * 0.5,
                        draw_fg.b() * 0.5,
                        draw_fg.a(),
                    )
                } else {
                    draw_fg
                };

                if cell.is_spacer {
                    col += 1;
                    continue;
                }
                if cell.hidden {
                    col += 1;
                    continue;
                }

                // ── Run boundary detection ────────────────────────────
                //
                // FUTURE (ligature shaping): when this cell is part of a
                // multi-character ligature run, `run_end` points past the
                // last cell in the run, and `run_text` is the concatenated
                // character sequence.  Instead of shaping each char
                // individually, call:
                //
                //   let shaped = atlas.shape_and_rasterize_run(&run_text)?;
                //
                // Then distribute each ShapedGlyph across its covering
                // cells (run_start .. run_start + glyph.num_cells) by
                // adjusting UV coordinates and glyph positions per-cell.
                //
                // For now, each run is 1 cell (run_end = col + 1).
                let _run_start = col;
                let _run_end = detect_run_end(&grid, row, col, cols);

                // ── Geometry helpers (dock-relative coords) ──────────
                let px_to_clip_x = |px: f32| px * x_scale - 1.0;
                let px_to_clip_y = |px: f32| 1.0 - px * y_scale;

                let num_cells: f32 = if col + 1 < cols {
                    grid.cell(row, col + 1)
                        .map_or(1.0, |c| if c.is_spacer { 2.0 } else { 1.0 })
                } else {
                    1.0
                };

                // ── Pass 1: background quad ────────────────────────
                if !is_cursor || is_block_cursor {
                    let cell_bg = if is_sel { sel_bg } else { draw_bg };
                    if is_block_cursor || cell_bg != default_bg {
                        let bg_x_px = x_off + (col as f32 * cw).round();
                        let bg_y_px = y_off + (row as f32 * ch).round();
                        let bqx = px_to_clip_x(bg_x_px);
                        let bqy = px_to_clip_y(bg_y_px);
                        let bqw = cw * num_cells * x_scale;
                        let bqh = ch * y_scale;

                        bg_instances.push(CellInstance {
                            clip_pos: [bqx, bqy],
                            uv_min: [0.0; 2],
                            uv_max: [0.0; 2],
                            clip_cell_size: [bqw, bqh],
                            glyph_size: [0.0; 2],
                            glyph_offset: [0.0; 2],
                            fg_color: [cell_bg.r(), cell_bg.g(), cell_bg.b(), cell_bg.a()],
                            bg_color: [cell_bg.r(), cell_bg.g(), cell_bg.b(), cell_bg.a()],
                            flags: glyph_type::SOLID,
                        });
                    }
                }

                // ── Pass 2: glyph quad ──────────────────────────────
                if !is_blank {
                    if let Ok((entry, is_new)) = atlas.ensure_glyph(ch_char) {
                        if is_new {
                            has_new_glyphs = true;
                        }

                        let atlas_w =
                            (entry.atlas_rect.max.x - entry.atlas_rect.min.x) as f32;
                        let atlas_h =
                            (entry.atlas_rect.max.y - entry.atlas_rect.min.y) as f32;

                        let scale = entry.scale;
                        let mut scaled_w = atlas_w * scale;
                        let mut scaled_h = atlas_h * scale;
                        let sbx = entry.bearing_x * scale;
                        let sby = entry.bearing_y * scale;

                        let mut glyph_x_px = x_off + (col as f32 * cw + sbx).round();
                        let mut glyph_y_px =
                            y_off + (row as f32 * ch + (baseline - sby)).round();

                        let mut u_min =
                            (entry.atlas_rect.min.x as f32 + 0.5) / tex_size;
                        let mut v_min =
                            (entry.atlas_rect.min.y as f32 + 0.5) / tex_size;
                        let mut u_max =
                            (entry.atlas_rect.max.x as f32 - 0.5) / tex_size;
                        let mut v_max =
                            (entry.atlas_rect.max.y as f32 - 0.5) / tex_size;

                        let cell_left = x_off + col as f32 * cw;
                        let cell_top = y_off + row as f32 * ch;
                        let cell_right = cell_left + cw * num_cells;
                        let cell_bottom = cell_top + ch;

                        let glyph_bot_px = glyph_y_px + scaled_h;
                        let clipped_top = glyph_y_px.max(cell_top);
                        let clipped_bot = glyph_bot_px.min(cell_bottom);
                        let clipped_h = (clipped_bot - clipped_top).max(0.0);
                        if clipped_h < scaled_h && scaled_h > 0.0 {
                            let r_top = (clipped_top - glyph_y_px) / scaled_h;
                            let r_bot = (clipped_bot - glyph_y_px) / scaled_h;
                            let v_range = v_max - v_min;
                            v_min = v_min + v_range * r_top;
                            v_max = v_min + v_range * (r_bot - r_top);
                            glyph_y_px = clipped_top;
                            scaled_h = clipped_h;
                        }

                        let glyph_right_px = glyph_x_px + scaled_w;
                        let clipped_left = glyph_x_px.max(cell_left);
                        let clipped_right = glyph_right_px.min(cell_right);
                        let clipped_w = (clipped_right - clipped_left).max(0.0);
                        if clipped_w < scaled_w && scaled_w > 0.0 {
                            let r_left = (clipped_left - glyph_x_px) / scaled_w;
                            let r_right = (clipped_right - glyph_x_px) / scaled_w;
                            let u_range = u_max - u_min;
                            u_min = u_min + u_range * r_left;
                            u_max = u_min + u_range * (r_right - r_left);
                            glyph_x_px = clipped_left;
                            scaled_w = clipped_w;
                        }

                        let gqx = px_to_clip_x(glyph_x_px);
                        let gqy = px_to_clip_y(glyph_y_px);
                        let gqw = scaled_w * x_scale;
                        let gqh = scaled_h * y_scale;

                        let gtype = match entry.content_type {
                            GlyphContentType::Subpixel => glyph_type::SUBPIXEL,
                            GlyphContentType::Mask => glyph_type::MASK,
                            GlyphContentType::Color => glyph_type::COLOR,
                        };

                        if is_cursor && !is_block_cursor {
                            glyph_instances.push(CellInstance {
                                clip_pos: [gqx, gqy],
                                uv_min: [u_min, v_min],
                                uv_max: [u_max, v_max],
                                clip_cell_size: [gqw, gqh],
                                glyph_size: [scaled_w, scaled_h],
                                glyph_offset: [sbx, baseline - sby],
                                fg_color: [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0],
                                bg_color: [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0],
                                flags: gtype,
                            });
                        } else if is_sel {
                            let glyph_fg = sel_fg.unwrap_or(cell.fg);
                            glyph_instances.push(CellInstance {
                                clip_pos: [gqx, gqy],
                                uv_min: [u_min, v_min],
                                uv_max: [u_max, v_max],
                                clip_cell_size: [gqw, gqh],
                                glyph_size: [scaled_w, scaled_h],
                                glyph_offset: [sbx, baseline - sby],
                                fg_color: [glyph_fg.r(), glyph_fg.g(), glyph_fg.b(), 1.0],
                                bg_color: [sel_bg.r(), sel_bg.g(), sel_bg.b(), 1.0],
                                flags: gtype,
                            });
                        } else {
                            glyph_instances.push(CellInstance {
                                clip_pos: [gqx, gqy],
                                uv_min: [u_min, v_min],
                                uv_max: [u_max, v_max],
                                clip_cell_size: [gqw, gqh],
                                glyph_size: [scaled_w, scaled_h],
                                glyph_offset: [sbx, baseline - sby],
                                fg_color: [draw_fg.r(), draw_fg.g(), draw_fg.b(), 1.0],
                                bg_color: [draw_bg.r(), draw_bg.g(), draw_bg.b(), 1.0],
                                flags: gtype,
                            });
                        }
                    } else {
                        log::trace!(
                            "update_cell_instances: glyph lookup failed for ch={:?}",
                            ch_char
                        );
                    }
                }

                // ── Pass 3: underline / strikethrough ──────────────
                let deco_color = if is_cursor {
                    [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0]
                } else if is_sel {
                    let deco_fg = sel_fg.unwrap_or(cell.fg);
                    [deco_fg.r(), deco_fg.g(), deco_fg.b(), 1.0]
                } else {
                    [draw_fg.r(), draw_fg.g(), draw_fg.b(), 1.0]
                };

                if cell.underline {
                    let thickness = 1.0_f32.max((ch * 0.05).round());
                    let deco_y = baseline + 1.0;
                    let dqy = px_to_clip_y(y_off + (row as f32 * ch + deco_y).round());
                    let dqx = px_to_clip_x(x_off + (col as f32 * cw).round());
                    let dqw = cw * x_scale;
                    let dqh = thickness * y_scale;
                    deco_instances.push(CellInstance {
                        clip_pos: [dqx, dqy],
                        uv_min: [0.0; 2],
                        uv_max: [0.0; 2],
                        clip_cell_size: [dqw, dqh],
                        glyph_size: [0.0; 2],
                        glyph_offset: [0.0; 2],
                        fg_color: deco_color,
                        bg_color: deco_color,
                        flags: glyph_type::SOLID,
                    });
                }

                if cell.strikethrough {
                    let thickness = 1.0_f32.max((ch * 0.05).round());
                    let deco_y = (baseline * 0.55).round();
                    let dqy = px_to_clip_y(y_off + (row as f32 * ch + deco_y).round());
                    let dqx = px_to_clip_x(x_off + (col as f32 * cw).round());
                    let dqw = cw * x_scale;
                    let dqh = thickness * y_scale;
                    deco_instances.push(CellInstance {
                        clip_pos: [dqx, dqy],
                        uv_min: [0.0; 2],
                        uv_max: [0.0; 2],
                        clip_cell_size: [dqw, dqh],
                        glyph_size: [0.0; 2],
                        glyph_offset: [0.0; 2],
                        fg_color: deco_color,
                        bg_color: deco_color,
                        flags: glyph_type::SOLID,
                    });
                }

                // ── Pass 4: cursor style decorations (Beam / Underline) ──
                if is_cursor && !is_block_cursor {
                    let cursor_color = [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0];
                    let thickness = 2.0_f32.max((ch * 0.08).round());
                    let cx_px = x_off + (col as f32 * cw).round();
                    let cy_px = y_off + (row as f32 * ch).round();

                    match cursor_shape {
                        CursorShape::Underline => {
                            let bar_h = thickness;
                            let bar_y = cy_px + ch - bar_h;
                            deco_instances.push(CellInstance {
                                clip_pos: [px_to_clip_x(cx_px), px_to_clip_y(bar_y)],
                                uv_min: [0.0; 2],
                                uv_max: [0.0; 2],
                                clip_cell_size: [cw * x_scale, bar_h * y_scale],
                                glyph_size: [0.0; 2],
                                glyph_offset: [0.0; 2],
                                fg_color: cursor_color,
                                bg_color: cursor_color,
                                flags: glyph_type::SOLID,
                            });
                        }
                        CursorShape::Beam => {
                            let bar_w = thickness.max(2.0);
                            deco_instances.push(CellInstance {
                                clip_pos: [px_to_clip_x(cx_px), px_to_clip_y(cy_px)],
                                uv_min: [0.0; 2],
                                uv_max: [0.0; 2],
                                clip_cell_size: [bar_w * x_scale, ch * y_scale],
                                glyph_size: [0.0; 2],
                                glyph_offset: [0.0; 2],
                                fg_color: cursor_color,
                                bg_color: cursor_color,
                                flags: glyph_type::SOLID,
                            });
                        }
                        _ => {}
                    }
                }

                // ── Advance to next cell ──────────────────────────
                // FUTURE (ligature shaping): set `col = _run_end`
                // to skip past the entire ligature run.
                col += 1;
            }
        }

        // Cache rebuilt instances for the fast-path next frame.
        std::mem::swap(&mut self.cached_bg, &mut bg_instances);
        std::mem::swap(&mut self.cached_glyph, &mut glyph_instances);
        std::mem::swap(&mut self.cached_deco, &mut deco_instances);

        // Append to the shared instance buffer in draw order.
        let mut buf = self
            .gpu
            .shared
            .instances
            .lock()
            .expect("SharedRenderState.instances poisoned");
        buf.extend(&self.cached_bg);
        buf.extend(&self.cached_glyph);
        buf.extend(&self.cached_deco);
        drop(buf);

        // Mark the GPU side as dirty (instance generation bumped by
        // the app after all sessions have appended).
        if has_new_glyphs {
            drop(atlas); // release before sync_to_gpu re-locks
            self.atlas.sync_to_gpu();
        }

        self.terminal_dirty = false;
        true
    }
}

/// Effects emitted by [`TerminalSession::handle_side_effects`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEffect {
    /// The session requested a new window title (OSC 0/2).
    WindowTitle(String),
    /// The session requested the application close (terminal escape).
    CloseWindow,
}

// ── OSC 7 helpers ──────────────────────────────────────────────────────

/// Convert an OSC 7 URL (`file://host/path` or `/abs/path`) to a
/// filesystem [`PathBuf`].  Returns `None` on parse failure.
fn osc7_url_to_path(url: &str) -> Option<PathBuf> {
    if let Some(stripped) = url.strip_prefix("file://") {
        // Strip the host component (e.g. `file://localhost/...`).
        if let Some(after_host) = stripped.find('/').map(|i| &stripped[i..]) {
            return Some(PathBuf::from(percent_decode(after_host)));
        }
        return None;
    }
    if url.starts_with('/') {
        return Some(PathBuf::from(url));
    }
    None
}

/// Decode percent-encoded escapes (`%20` → space, etc.) in a URL
/// path.  Used by [`osc7_url_to_path`].
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let h = bytes.next();
            let l = bytes.next();
            if let (Some(h), Some(l)) = (h, l) {
                let hex = format!("{}{}", h as char, l as char);
                if let Ok(v) = u8::from_str_radix(&hex, 16) {
                    out.push(v as char);
                    continue;
                }
            }
            out.push('%');
        } else {
            out.push(b as char);
        }
    }
    out
}

// Re-export for use by `app.rs` / `tab_viewer.rs` / `sidebar.rs`:
//   - The session also needs access to `terminal.size()`, `pty.write`,
//     and `terminal.cursor()` for the keyboard input mapper.  These
//     stay in scope via `Terminal` and `PtySession` re-exports above.

// ── Per-tab mouse handling + context menu ─────────────────────────────

impl TerminalSession {
    /// Handle mouse events for this session's cell rectangle.
    ///
    /// Behaviour matches the original single-terminal path in
    /// `app.rs::ui()`:
    ///
    /// * If the terminal has `SGR_MOUSE` enabled, every pointer
    ///   event is encoded as an SGR escape sequence and written to
    ///   the PTY.
    /// * Otherwise, click-drag performs text selection; single click
    ///   clears the selection.
    pub fn handle_mouse(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        size_px: [f32; 2],
        response: &egui::Response,
    ) {
        let mode = self.terminal.mode();
        let mouse_reporting = mode.contains(TermMode::SGR_MOUSE)
            && mode.intersects(
                TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION,
            );

        let cw = self.cell_width;
        let ch = self.cell_height;
        let rows = self.terminal.size().rows as usize;
        let cols = self.terminal.size().cols as usize;
        let _ = size_px;
        let _ = ui;

        // Pointer position in cell coordinates.  Returns `None` when
        // the pointer is outside the cell grid (e.g. scrollback area).
        let pixel_to_cell = |pos: egui::Pos2| -> Option<(usize, usize)> {
            let col = ((pos.x - rect.left()) / cw) as usize;
            let row = ((pos.y - rect.top()) / ch) as usize;
            if col < cols && row < rows {
                Some((row, col))
            } else {
                None
            }
        };

        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some((row, col)) = pixel_to_cell(pos) {
                    if mouse_reporting {
                        self.send_sgr_mouse(row, col, 0, false);
                    } else {
                        self.terminal.clear_selection();
                        self.terminal.start_selection(row, col);
                        self.selecting = true;
                        self.terminal_dirty = true;
                    }
                }
            }
        }
        if response.dragged() {
            if mouse_reporting {
                if let Some(pos) = response.interact_pointer_pos() {
                    if let Some((row, col)) = pixel_to_cell(pos) {
                        self.send_sgr_mouse(row, col, 32, false);
                    }
                }
            } else if self.selecting {
                if let Some(pos) = response.interact_pointer_pos() {
                    if let Some((row, col)) = pixel_to_cell(pos) {
                        self.terminal.update_selection(row, col);
                        self.terminal_dirty = true;
                    }
                }
            }
        }
        if response.drag_stopped() {
            if mouse_reporting {
                if let Some(pos) = response.interact_pointer_pos() {
                    if let Some((row, col)) = pixel_to_cell(pos) {
                        self.send_sgr_mouse(row, col, 0, true);
                    }
                }
            } else {
                self.selecting = false;
                self.terminal_dirty = true;
            }
        }
        if response.clicked() && !self.selecting && !mouse_reporting {
            self.terminal.clear_selection();
            self.terminal_dirty = true;
        }
    }

    /// Render the right-click context menu (Copy / Paste).
    pub fn render_context_menu(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
    ) {
        response.context_menu(|ctx_ui| {
            if self.terminal.has_selection() {
                if ctx_ui.button("Copy").clicked() {
                    if let Some(text) = self.terminal.selected_text() {
                        ctx_ui.ctx().copy_text(text);
                    }
                    ctx_ui.close();
                }
            }
            if ctx_ui.button("Paste").clicked() {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    if let Ok(text) = clipboard.get_text() {
                        if !text.is_empty() {
                            if let Err(e) = self.pty.write(text.as_bytes()) {
                                log::error!("PTY paste error: {e}");
                            }
                        }
                    }
                }
                ctx_ui.close();
            }
        });
        let _ = ui;
    }
}

