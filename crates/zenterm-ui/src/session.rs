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

use zenterm_core::cell::UnderlineStyle;
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

// ── Ligature helpers ───────────────────────────────────────────────────

/// Compute the actual number of terminal-grid cells covered by a shaped
/// glyph's character range.  This is grid-aware: CJK/emoji characters
/// (whose right neighbour is a spacer) count as 2 cells, ASCII as 1.
fn glyph_grid_num_cells(
    grid: &GridView,
    row: usize,
    run_start: usize,
    char_range: &std::ops::Range<usize>,
    cols: usize,
) -> usize {
    let mut total = 0usize;
    for ci in char_range.clone() {
        let col = run_start + ci;
        // If this char's right neighbour is a spacer, it's a wide
        // character (CJK / emoji) and contributes 2 cells.
        if col + 1 < cols {
            if let Some(next) = grid.cell(row, col + 1) {
                if next.is_spacer {
                    total += 2;
                    continue;
                }
            }
        }
        total += 1;
    }
    total
}

/// Emit underline, strikethrough, and cursor-style (Beam/Underline)
/// decoration quads for a single cell.
///
/// This mirrors the "Pass 3" and "Pass 4" logic from the per-char
/// render path in [`TerminalSession::update_cell_instances`].
#[allow(clippy::too_many_arguments)]
fn emit_deco_for_cell(
    deco_instances: &mut Vec<CellInstance>,
    grid: &GridView,
    row: usize,
    col: usize,
    _cols: usize,
    cursor_visible: bool,
    cursor_row: usize,
    cursor_col: usize,
    cursor_shape: CursorShape,
    _display_offset: usize,
    sel_range: Option<&SelectionRange>,
    _sel_bg: Rgba,
    sel_fg: Option<Rgba>,
    _default_bg: Rgba,
    baseline: f32,
    ch: f32,
    cw: f32,
    x_off: f32,
    y_off: f32,
    px_to_clip_x: &impl Fn(f32) -> f32,
    px_to_clip_y: &impl Fn(f32) -> f32,
    x_scale: f32,
    y_scale: f32,
) {
    let cell = match grid.cell(row, col) {
        Some(c) => c,
        None => return,
    };

    if cell.hidden {
        return;
    }

    let is_cursor = cursor_visible && row == cursor_row && col == cursor_col;
    let is_block_cursor = is_cursor && matches!(cursor_shape, CursorShape::Block);
    let is_sel = sel_range.is_some_and(|range| {
        let grid_line = (row as i32) - (_display_offset as i32);
        let pt = Point::new(Line(grid_line), Column(col));
        range.contains(pt)
    });

    let (draw_fg, _draw_bg) = if is_block_cursor {
        (cell.bg, cell.fg)
    } else {
        (cell.fg, cell.bg)
    };
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

    // ── Pass 3: underline / strikethrough ──────────────
    let deco_color = if is_cursor {
        [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0]
    } else if is_sel {
        let deco_fg = sel_fg.unwrap_or(cell.fg);
        [deco_fg.r(), deco_fg.g(), deco_fg.b(), 1.0]
    } else {
        [draw_fg.r(), draw_fg.g(), draw_fg.b(), 1.0]
    };

    // Helper to push a solid decoration quad.
    let mut push_deco = |y_offset: f32, dqw: f32, dqh: f32| {
        let dqy = px_to_clip_y(y_off + (row as f32 * ch + y_offset).round());
        let dqx = px_to_clip_x(x_off + (col as f32 * cw).round());
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
    };

    let thickness = 1.0_f32.max((ch * 0.05).round());
    let cell_w = cw * x_scale;
    let cell_h = thickness * y_scale;
    match cell.underline_style {
        UnderlineStyle::None => {}
        UnderlineStyle::Normal => {
            push_deco(baseline + 1.0, cell_w, cell_h);
        }
        UnderlineStyle::Double => {
            push_deco(baseline + 1.0, cell_w, cell_h);
            push_deco(baseline + 3.0, cell_w, cell_h);
        }
        UnderlineStyle::Curly | UnderlineStyle::Dotted | UnderlineStyle::Dashed => {
            push_deco(baseline + 1.0, cell_w, cell_h);
        }
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
}

/// Quick check: only multi-char runs containing ASCII punctuation or
/// operators are worth shaping with [`Shaping::Advanced`](cosmic_text::Shaping::Advanced).
/// Everything else can take the per-char fast path.
///
/// This avoids pointless `cosmic-text` `Buffer` allocation + shaping for
/// plain alphanumeric sequences that never form ligatures.
fn might_ligate(text: &str) -> bool {
    text.len() > 1 && text.bytes().any(|b| b.is_ascii_punctuation())
}

/// Extract the concatenated character text for a run of cells.
///
/// The returned string is passed to
/// [`GlyphAtlas::shape_and_rasterize_run`] for ligature-aware shaping.
fn extract_run_text(grid: &GridView, row: usize, start: usize, end: usize) -> String {
    let mut s = String::with_capacity(end - start);
    for col in start..end {
        if let Some(cell) = grid.cell(row, col) {
            s.push(cell.c);
        }
    }
    s
}

/// Detect the end of a consecutive-cell "run" for ligature shaping.
///
/// Starting at `start_col`, walk forward while cells share the same style
/// (same `bold`, `italic` flags) and are not spaces, spacers, or hidden.
/// Returns the first column *past* the run (i.e. `end_col` such that
/// `start_col .. end_col` is the run range).
///
/// When ligature shaping is enabled, the run text is passed to
/// [`GlyphAtlas::shape_and_rasterize_run`] so that OpenType ligature
/// rules (`liga`/`clig`) can substitute multi-cell glyphs (e.g. `->` →
/// one arrow glyph).
///
/// Run boundaries occur at:
///
/// * **End of row** — no more cells.
/// * **Space character** — spaces never participate in ligatures.
/// * **Spacer cell** — a CJK / emoji wide-character continuation.
/// * **Hidden cell** — invisible content should not be shaped.
/// * **Style change** — different `bold` or `italic` flags require
///   separate shaping with different [`cosmic_text::Attrs`].
fn detect_run_end(
    grid: &GridView,
    row: usize,
    start_col: usize,
    cols: usize,
) -> usize {
    let first = match grid.cell(row, start_col) {
        Some(c) => c,
        None => return start_col + 1,
    };

    // Wide characters (CJK, emoji) occupy 2 cells: the character cell
    // followed by a spacer.  Always return them as single-cell runs so
    // that the per-char path handles them with the correct 2-cell width
    // (num_cells from is_spacer check).  If a wide char were part of a
    // ligature run, its strip would get 1-cell width instead.
    if start_col + 1 < cols {
        if let Some(next) = grid.cell(row, start_col + 1) {
            if next.is_spacer {
                return start_col + 1;
            }
        }
    }

    let mut col = start_col + 1;
    while col < cols {
        let cell = match grid.cell(row, col) {
            Some(c) => c,
            None => break,
        };
        // Spaces never participate in ligatures.
        if cell.c == ' ' || cell.is_spacer || cell.hidden {
            break;
        }
        // Style boundary: different weight/style needs separate shaping.
        if cell.bold != first.bold || cell.italic != first.italic {
            break;
        }
        // Wide character (CJK / emoji) check: if this cell's right
        // neighbour is a spacer, the cell occupies 2 cells and must
        // form its own single-cell run so that the per-char path
        // handles it with the correct 2-cell width.
        if col + 1 < cols {
            if let Some(next) = grid.cell(row, col + 1) {
                if next.is_spacer {
                    break;
                }
            }
        }
        col += 1;
    }
    col
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

    // ── Scrollbar state ────────────────────────────────────────────────
    scrollbar_dragging: bool,
    scrollbar_drag_start_y: f32,
    scrollbar_drag_start_offset: usize,
}

/// Pixel width of the overlay scrollbar.
const SCROLLBAR_WIDTH: f32 = 10.0;

/// Minimum pixel height of the scrollbar thumb.
const SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 24.0;

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
            scrollbar_dragging: false,
            scrollbar_drag_start_y: 0.0,
            scrollbar_drag_start_offset: 0,
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
    ///
    /// `time` is the current UI time (from `ui.input(|i| i.time)`) used to
    /// timestamp the resize so the transient size overlay can fade out.
    pub fn resize_to_viewport(&mut self, size_px: [f32; 2], ppp: f32, time: f64) {
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
            self.last_resize_at = Some(time);
        }
        let _ = ppp;
    }

    /// Render the transient resize overlay — a centered "cols × rows"
    /// label with a semi-transparent backdrop that appears for ~2 s
    /// after the most recent terminal resize, then disappears abruptly.
    ///
    /// The backdrop colour adapts to the terminal background: a dark
    /// backdrop with light text for light terminals, and a light backdrop
    /// with dark text for dark terminals, ensuring it's always readable.
    ///
    /// This method is a no-op if no resize has occurred within the
    /// display window.  Call it **after** the terminal content has been
    /// painted so the overlay appears on top.
    pub fn render_resize_overlay(&self, ui: &egui::Ui, rect: egui::Rect) {
        let last_time = match self.last_resize_at {
            Some(t) => t,
            None => return,
        };
        let now = ui.input(|i| i.time);
        let elapsed = (now - last_time) as f32;
        if elapsed < 0.0 || elapsed >= 2.0 {
            return;
        }

        let size = self.terminal.size();
        let text = format!("{} × {}", size.cols, size.rows);

        // Choose backdrop and text colours based on terminal background
        // luminance so the overlay is legible on both light and dark
        // terminals.
        let bg = self.default_bg;
        let lum = 0.299 * bg.r() as f32 / 255.0
            + 0.587 * bg.g() as f32 / 255.0
            + 0.114 * bg.b() as f32 / 255.0;
        let (backdrop_color, text_color) = if lum < 0.5 {
            // Dark terminal → light backdrop with dark text.
            (
                egui::Color32::WHITE.gamma_multiply(0.55),
                egui::Color32::BLACK,
            )
        } else {
            // Light terminal → dark backdrop with light text.
            (
                egui::Color32::BLACK.gamma_multiply(0.55),
                egui::Color32::WHITE,
            )
        };

        // Semi-transparent rounded backdrop, centred in `rect`.
        let backdrop = egui::Rect::from_center_size(rect.center(), egui::vec2(160.0, 44.0));
        ui.painter().rect_filled(backdrop, 8.0, backdrop_color);

        // Text label (fully opaque during the display window).
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(20.0),
            text_color,
        );
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
        let display_offset = self.terminal.display_offset();

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
            // Track the last run_end that was checked for ligatures.
            // Once we find that a run has no actual multi-cell ligature
            // glyphs, we skip re-checking subsequent cells of the same
            // run to avoid redundant shaping + atlas allocations.
            let mut last_checked_run_end: usize = 0;
            while col < cols {
                let cell = match grid.cell(row, col) {
                    Some(c) => c,
                    None => { col += 1; continue; },
                };

                let is_cursor = cursor_visible && row == cursor_row && col == cursor_col;
                let is_block_cursor =
                    is_cursor && matches!(cursor_shape, CursorShape::Block);

                let is_sel = sel_range.as_ref().is_some_and(|range| {
                    let grid_line = (row as i32) - (display_offset as i32);
                    let pt = Point::new(Line(grid_line), Column(col));
                    range.contains(pt)
                });

                let ch_char = cell.c;
                let is_blank = ch_char == ' ' && cell.bg == Rgba::BLACK && !is_cursor;
                let is_hidden = cell.hidden;
                let is_spacer = cell.is_spacer;

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

                if is_spacer {
                    col += 1;
                    continue;
                }

                // ── Run boundary detection ────────────────────────────
                let run_start = col;
                let run_end = detect_run_end(&grid, row, col, cols);

                // ── Geometry helpers (dock-relative coords) ──────────
                let px_to_clip_x = |px: f32| px * x_scale - 1.0;
                let px_to_clip_y = |px: f32| 1.0 - px * y_scale;

                // ── Ligature branch ─────────────────────────────────
                //
                // When ligature shaping is active and the run contains
                // ASCII punctuation, shape the entire run as a single
                // string, then distribute the resulting glyphs across
                // their covering cells via per-cell strip splitting.
                //
                // Background quads and decorations are emitted per cell
                // inside this branch so that cursor / selection colours
                // are applied independently to each cell.
                let ligatures_enabled = atlas.ligatures_enabled;
                // Skip the ligature branch if we already checked this
                // exact run (col..run_end) on a previous iteration and
                // found no actual multi-cell ligature glyphs.
                let ligature_eligible = ligatures_enabled
                    && run_end > run_start + 1
                    && !is_blank
                    && run_end != last_checked_run_end;
                if ligature_eligible {
                    let run_text = extract_run_text(&grid, row, run_start, run_end);
                    if might_ligate(&run_text) {
                        log::debug!(
                            "ligature ENTER: row={row} run={run_start}..{run_end} \
                             text={run_text:?}",
                        );
                        match atlas.shape_and_rasterize_run(&run_text) {
                            Ok(shaped) => {
                                // Only use the ligature branch when there are
                                // actual multi-cell ligature glyphs.  Runs
                                // without real ligatures (all num_cells == 1)
                                // fall through to the per-char path, which
                                // renders identically to the original code.
                                let has_ligature =
                                    shaped.iter().any(|sg| sg.num_cells > 1);
                                if !has_ligature {
                                    log::debug!(
                                        "ligature no-op: row={row} run={run_start}..{run_end} \
                                         no multi-cell glyphs",
                                    );
                                    // Remember that this run has no actual
                                    // ligatures so subsequent cells don't
                                    // re-enter the branch.
                                    last_checked_run_end = run_end;
                                } else {                                    log::debug!(
                                        "ligature OK: row={row} run={run_start}..{run_end} \
                                         glyphs={} -> SKIP per-char path",
                                        shaped.len(),
                                    );
                                    let mut strip_col = run_start;

                                    for sg in &shaped {
                                        let cell_base = run_start + sg.char_range.start;

                                        // Advance past any gap between shaped glyphs
                                        // (should not happen for continuous runs, but
                                        //  be safe).
                                        if cell_base > strip_col {
                                            for ccol in strip_col..cell_base {
                                                emit_deco_for_cell(
                                                    &mut deco_instances,
                                                    &grid, row, ccol, cols,
                                                    cursor_visible, cursor_row, cursor_col,
                                                    cursor_shape, display_offset,
                                                    sel_range.as_ref(), sel_bg, sel_fg,
                                                    default_bg, baseline, ch, cw,
                                                    x_off, y_off,
                                                    &px_to_clip_x, &px_to_clip_y,
                                                    x_scale, y_scale,
                                                );
                                            }
                                            strip_col = cell_base;
                                        }

                                        // Background quads + glyph strips for each cell
                                        // this glyph covers.  Use the grid-aware
                                        // num_cells so that CJK/emoji characters
                                        // (2 cells wide) are handled correctly.
                                        let actual_num_cells = glyph_grid_num_cells(
                                            &grid, row, run_start,
                                            &sg.char_range, cols,
                                        );
                                        for cell_offset in 0..actual_num_cells {
                                            let cell_col = cell_base + cell_offset;
                                            let c = grid.cell(row, cell_col)
                                                .unwrap_or(cell); // fallback to current cell

                                            // ── Per-cell cursor / selection state ──
                                            let c_is_cursor = cursor_visible
                                                && row == cursor_row
                                                && cell_col == cursor_col;
                                            let c_is_block = c_is_cursor
                                                && matches!(cursor_shape, CursorShape::Block);
                                            let c_is_sel = sel_range.as_ref().is_some_and(|range| {
                                                let grid_line =
                                                    (row as i32) - (display_offset as i32);
                                                let pt = Point::new(
                                                    Line(grid_line),
                                                    Column(cell_col),
                                                );
                                                range.contains(pt)
                                            });

                                            let (c_fg, c_bg) = if c_is_block {
                                                (c.bg, c.fg)
                                            } else {
                                                (c.fg, c.bg)
                                            };
                                            let c_draw_fg = if c.dim {
                                                Rgba::new(
                                                    c_fg.r() * 0.5,
                                                    c_fg.g() * 0.5,
                                                    c_fg.b() * 0.5,
                                                    c_fg.a(),
                                                )
                                            } else {
                                                c_fg
                                            };
                                            let c_bg_color = if c_is_sel {
                                                sel_bg
                                            } else {
                                                c_bg
                                            };

                                            // ── Pass 1: background quad ──
                                            if c_is_block || c_bg_color != default_bg {
                                                let bg_x = x_off + (cell_col as f32 * cw).round();
                                                let bg_y = y_off + (row as f32 * ch).round();
                                                bg_instances.push(CellInstance {
                                                    clip_pos: [
                                                        px_to_clip_x(bg_x),
                                                        px_to_clip_y(bg_y),
                                                    ],
                                                    uv_min: [0.0; 2],
                                                    uv_max: [0.0; 2],
                                                    clip_cell_size: [cw * x_scale, ch * y_scale],
                                                    glyph_size: [0.0; 2],
                                                    glyph_offset: [0.0; 2],
                                                    fg_color: [
                                                        c_bg_color.r(),
                                                        c_bg_color.g(),
                                                        c_bg_color.b(),
                                                        c_bg_color.a(),
                                                    ],
                                                    bg_color: [
                                                        c_bg_color.r(),
                                                        c_bg_color.g(),
                                                        c_bg_color.b(),
                                                        c_bg_color.a(),
                                                    ],
                                                    flags: glyph_type::SOLID,
                                                });
                                            }

                                            // ── Pass 2: glyph strip ──
                                            let atlas_rect = &sg.entry.atlas_rect;
                                            let a_left = atlas_rect.min.x as f32;
                                            let a_top = atlas_rect.min.y as f32;
                                            let a_bot = atlas_rect.max.y as f32;

                                            // Strip boundaries in pixels, relative to the
                                            // glyph origin.
                                            let strip_left = cell_offset as f32 * cw;
                                            let strip_right = (cell_offset + 1) as f32 * cw;
                                            // Clamp strip to glyph advance so we never
                                            // sample beyond the glyph's right edge.
                                            let strip_right =
                                                strip_right.min(sg.entry.advance);

                                            let strip_w = strip_right - strip_left;

                                            // UV coordinates for this strip: a horizontal
                                            // slice of the glyph's atlas rectangle.
                                            let mut u_min =
                                                (a_left + 0.5 + strip_left) / tex_size;
                                            let mut u_max =
                                                (a_left + 0.5 + strip_right) / tex_size;
                                            let mut v_min =
                                                (a_top + 0.5) / tex_size;
                                            let mut v_max =
                                                (a_bot - 0.5) / tex_size;

                                            let glyph_h = (a_bot - a_top) as f32;
                                            let sbx = sg.entry.bearing_x;
                                            let sby = sg.entry.bearing_y;

                                            // glyph_offset.x: the bearing is always
                                            // relative to this cell's origin, so we use
                                            // the raw bearing (not adjusted by strip_left).
                                            // The UV coordinates below select the correct
                                            // horizontal slice of the glyph texture.
                                            let gox = sbx;
                                            let goy = baseline - sby;

                                            let mut glyph_x_px =
                                                x_off + (cell_col as f32 * cw + gox).round();
                                            let mut glyph_y_px =
                                                y_off + (row as f32 * ch + goy).round();

                                            let mut scaled_w = strip_w;
                                            let mut scaled_h = glyph_h;

                                            // ── Vertical clip (GLYPH_CLIP.md) ──
                                            let cell_left = x_off + cell_col as f32 * cw;
                                            let cell_top = y_off + row as f32 * ch;
                                            let cell_right = cell_left + cw;
                                            let cell_bottom = cell_top + ch;

                                            let glyph_bot = glyph_y_px + scaled_h;
                                            let clipped_top = glyph_y_px.max(cell_top);
                                            let clipped_bot = glyph_bot.min(cell_bottom);
                                            let clipped_h =
                                                (clipped_bot - clipped_top).max(0.0);
                                            if clipped_h < scaled_h && scaled_h > 0.0 {
                                                let r_top = (clipped_top - glyph_y_px)
                                                    / scaled_h;
                                                let r_bot = (clipped_bot - glyph_y_px)
                                                    / scaled_h;
                                                let v_range = v_max - v_min;
                                                v_min = v_min + v_range * r_top;
                                                v_max = v_min + v_range * (r_bot - r_top);
                                                glyph_y_px = clipped_top;
                                                scaled_h = clipped_h;
                                            }

                                            // ── Horizontal clip (GLYPH_CLIP.md) ──
                                            let glyph_right = glyph_x_px + scaled_w;
                                            let clipped_left = glyph_x_px.max(cell_left);
                                            let clipped_right = glyph_right.min(cell_right);
                                            let clipped_w =
                                                (clipped_right - clipped_left).max(0.0);
                                            if clipped_w < scaled_w && scaled_w > 0.0 {
                                                let r_left = (clipped_left - glyph_x_px)
                                                    / scaled_w;
                                                let r_right = (clipped_right - glyph_x_px)
                                                    / scaled_w;
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

                                            let gtype = match sg.entry.content_type {
                                                GlyphContentType::Subpixel
                                                    => glyph_type::SUBPIXEL,
                                                GlyphContentType::Mask
                                                    => glyph_type::MASK,
                                                GlyphContentType::Color
                                                    => glyph_type::COLOR,
                                            };

                                            let (glyph_fg, glyph_bg) = if c_is_cursor
                                                && !c_is_block
                                            {
                                                (c.bg, c.fg)
                                            } else if c_is_sel {
                                                (
                                                    sel_fg.unwrap_or(c.fg),
                                                    sel_bg,
                                                )
                                            } else {
                                                (c_draw_fg, c_bg_color)
                                            };

                                            glyph_instances.push(CellInstance {
                                                clip_pos: [gqx, gqy],
                                                uv_min: [u_min, v_min],
                                                uv_max: [u_max, v_max],
                                                clip_cell_size: [gqw, gqh],
                                                glyph_size: [scaled_w, scaled_h],
                                                glyph_offset: [gox, goy],
                                                fg_color: [
                                                    glyph_fg.r(),
                                                    glyph_fg.g(),
                                                    glyph_fg.b(),
                                                    1.0,
                                                ],
                                                bg_color: [
                                                    glyph_bg.r(),
                                                    glyph_bg.g(),
                                                    glyph_bg.b(),
                                                    1.0,
                                                ],
                                                flags: gtype,
                                            });

                                            // ── Pass 3+4: decorations ──
                                            emit_deco_for_cell(
                                                &mut deco_instances,
                                                &grid, row, cell_col, cols,
                                                cursor_visible, cursor_row, cursor_col,
                                                cursor_shape, display_offset,
                                                sel_range.as_ref(), sel_bg, sel_fg,
                                                default_bg, baseline, ch, cw,
                                                x_off, y_off,
                                                &px_to_clip_x, &px_to_clip_y,
                                                x_scale, y_scale,
                                            );

                                            strip_col = cell_col + 1;
                                        }
                                    }

                                    // Any remaining cells in the run beyond the
                                    // last shaped glyph.
                                    for ccol in strip_col..run_end {
                                        emit_deco_for_cell(
                                            &mut deco_instances,
                                            &grid, row, ccol, cols,
                                            cursor_visible, cursor_row, cursor_col,
                                            cursor_shape, display_offset,
                                            sel_range.as_ref(), sel_bg, sel_fg,
                                            default_bg, baseline, ch, cw,
                                            x_off, y_off,
                                            &px_to_clip_x, &px_to_clip_y,
                                            x_scale, y_scale,
                                        );
                                    }

                                    col = run_end;
                                    continue;
                                }
                            }
                            Err(e) => {
                                log::warn!(
                                    "shape_and_rasterize_run failed for \
                                     run={:?}: {e:?}; falling back to per-char",
                                    run_text,
                                );
                            }
                        }
                    }
                }

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

                // SGR 8 (conceal / hidden): render background but skip glyph + decorations.
                if is_hidden {
                    col += 1;
                    continue;
                }

                // ── Pass 2: glyph quad ──────────────────────────────
                if !is_blank {
                    log::debug!(
                        "per-char glyph: row={row} col={col} ch={ch_char:?} \
                         run_start={run_start} run_end={run_end}",
                    );
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

                // Helper to push a solid decoration quad.
                let mut push_deco = |y_offset: f32, dqw: f32, dqh: f32| {
                    let dqy = px_to_clip_y(y_off + (row as f32 * ch + y_offset).round());
                    let dqx = px_to_clip_x(x_off + (col as f32 * cw).round());
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
                };

                let thickness = 1.0_f32.max((ch * 0.05).round());
                let cell_w = cw * x_scale;
                let cell_h = thickness * y_scale;
                match cell.underline_style {
                    UnderlineStyle::None => {}
                    UnderlineStyle::Normal => {
                        push_deco(baseline + 1.0, cell_w, cell_h);
                    }
                    UnderlineStyle::Double => {
                        // Two lines: one at baseline+1, one at baseline+3.
                        push_deco(baseline + 1.0, cell_w, cell_h);
                        push_deco(baseline + 3.0, cell_w, cell_h);
                    }
                    // Curly, dotted, dashed: fall back to a normal underline
                    // so the decoration is at least visible.
                    UnderlineStyle::Curly
                    | UnderlineStyle::Dotted
                    | UnderlineStyle::Dashed => {
                        push_deco(baseline + 1.0, cell_w, cell_h);
                    }
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
                // Note: the ligature branch above uses `col = run_end; continue`,
                // so this per-char `col += 1` is only reached for single-cell runs.
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
    /// Behaviour:
    ///
    /// * If the terminal has `SGR_MOUSE` enabled, every pointer event is
    ///   encoded as an SGR escape sequence and written to the PTY.
    /// * Otherwise, click-drag performs text selection; single click
    ///   clears the selection.
    /// * The overlay scrollbar (right edge) supports draggable thumb,
    ///   track-click page-up/down, and mouse-wheel scrolling.
    /// * Dragging a selection beyond the top/bottom edge scrolls the
    ///   viewport automatically.
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
        let ppp = ui.ctx().pixels_per_point();
        let rows = self.terminal.size().rows as usize;
        let cols = self.terminal.size().cols as usize;
        let _ = size_px;

        // ── Scrollbar geometry ───────────────────────────────────────────
        let sb_rect = egui::Rect::from_min_max(
            egui::pos2(rect.right() - SCROLLBAR_WIDTH, rect.top()),
            egui::pos2(rect.right(), rect.bottom()),
        );
        let cell_area = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.right() - SCROLLBAR_WIDTH, rect.bottom()),
        );

        // ── Scrollbar: click / drag / track-click ──────────────────────
        if let Some(pos) = response.interact_pointer_pos() {
            if sb_rect.contains(pos) {
                // ── Drag start on the scrollbar ──
                if response.drag_started() {
                    self.scrollbar_dragging = true;
                    self.scrollbar_drag_start_y = pos.y;
                    self.scrollbar_drag_start_offset = self.terminal.display_offset();
                }
                // ── Track-click (above/below thumb) → page up/down ──
                if response.clicked() {
                    let hist = self.terminal.history_size();
                    if hist > 0 {
                        let (thumb, _) =
                            Self::scrollbar_thumb_rect(sb_rect, self.terminal.size().rows as usize, hist, self.terminal.display_offset());
                        if pos.y < thumb.top() {
                            self.terminal.scroll_display(rows as i32);
                        } else if pos.y > thumb.bottom() {
                            self.terminal.scroll_display(-(rows as i32));
                        }
                        self.terminal_dirty = true;
                    }
                }
                return; // scrollbar area: don't process cell events
            }
        }

        // ── Scrollbar: drag thumb update (tracked even if pointer left the bar) ──
        if self.scrollbar_dragging {
            if let Some(pos) = response.interact_pointer_pos() {
                let hist = self.terminal.history_size() as f32;
                if hist > 0.0 {
                    let dy = pos.y - self.scrollbar_drag_start_y;
                    let ratio_delta = dy / sb_rect.height();
                    let offset_delta = (ratio_delta * hist) as i32;
                    let target = (self.scrollbar_drag_start_offset as i32 - offset_delta)
                        .clamp(0, hist as i32);
                    let cur = self.terminal.display_offset() as i32;
                    if target != cur {
                        self.terminal.scroll_display(target - cur);
                        self.terminal_dirty = true;
                    }
                }
            }
            if response.drag_stopped() {
                self.scrollbar_dragging = false;
            }
        }

        // ── Mouse wheel scrolling ──────────────────────────────────────
        if response.hovered() || self.scrollbar_dragging {
            let total_scroll: f32 = ui.ctx().input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::MouseWheel { delta, unit, .. } => {
                            let y = delta.y;
                            match unit {
                                egui::MouseWheelUnit::Line => Some(y),
                                // y is in points.  Divide by a fraction of
                                // cell-height so even small per-event deltas
                                // produce a non‑zero line count.
                                egui::MouseWheelUnit::Point => Some(y * 4.0 / ch),
                                egui::MouseWheelUnit::Page => Some(y * rows as f32),
                            }
                        }
                        _ => None,
                    })
                    .sum()
            });
            if total_scroll.abs() > 0.0 {
                // Consume scroll events so egui doesn't pass them through.
                ui.ctx()
                    .input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::MouseWheel { .. })));
                // Do not scroll while an alternate-screen app is running.
                if !mode.contains(TermMode::ALT_SCREEN) {
                    let lines = total_scroll.round() as i32;
                    if lines != 0 {
                        self.terminal.scroll_display(lines);
                        self.terminal_dirty = true;
                    }
                }
            }
        }

        // ── Pointer → cell coordinate helpers ──────────────────────────
        // NOTE: cw/ch are in physical pixels, but pos is in logical points.
        // Multiply by ppp to convert before dividing.
        let pixel_to_cell = |pos: egui::Pos2| -> Option<(usize, usize)> {
            let col = ((pos.x - cell_area.left()) * ppp / cw) as usize;
            let row = ((pos.y - cell_area.top()) * ppp / ch) as usize;
            if col < cols && row < rows {
                Some((row, col))
            } else {
                None
            }
        };

        // Clamped version: returns the nearest cell even when outside the area.
        let pixel_to_cell_clamped = |pos: egui::Pos2| -> (usize, usize) {
            let col = ((pos.x - cell_area.left()) * ppp / cw).round() as usize;
            let row = ((pos.y - cell_area.top()) * ppp / ch).round() as usize;
            (row.min(rows.saturating_sub(1)), col.min(cols.saturating_sub(1)))
        };

        // ── Drag start / selection ─────────────────────────────────────
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

        // ── Drag update (selection or edge-scroll) ─────────────────────
        if response.dragged() {
            if mouse_reporting {
                if let Some(pos) = response.interact_pointer_pos() {
                    if let Some((row, col)) = pixel_to_cell(pos) {
                        self.send_sgr_mouse(row, col, 32, false);
                    }
                }
            } else if self.selecting {
                if let Some(pos) = response.interact_pointer_pos() {
                    // Normal: pointer inside the cell grid → update selection.
                    if let Some((row, col)) = pixel_to_cell(pos) {
                        self.terminal.update_selection(row, col);
                        self.terminal_dirty = true;
                    } else {
                        // Edge-scroll: pointer is outside the cell grid.
                        let rel_y = pos.y - cell_area.top();
                        let clamped = pixel_to_cell_clamped(pos);
                        if rel_y < 0.0 {
                            // Above top → scroll up.
                            let dist = -rel_y;
                            let lines = (dist * ppp / ch).ceil().max(1.0) as i32;
                            self.terminal.scroll_display(lines);
                            self.terminal.update_selection(0, clamped.1);
                        } else {
                            // Below bottom → scroll down.
                            let dist = pos.y - cell_area.bottom();
                            let lines = (dist * ppp / ch).ceil().max(1.0) as i32;
                            self.terminal.scroll_display(-lines);
                            self.terminal
                                .update_selection(rows.saturating_sub(1), clamped.1);
                        }
                        self.terminal_dirty = true;
                    }
                }
            }
        }

        // ── Drag stop ──────────────────────────────────────────────────
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

        // ── Single click clears selection ──────────────────────────────
        if response.clicked() && !self.selecting && !mouse_reporting {
            self.terminal.clear_selection();
            self.terminal_dirty = true;
        }
    }

    /// Compute the scrollbar thumb rectangle.
    fn scrollbar_thumb_rect(
        track: egui::Rect,
        screen_lines: usize,
        history_size: usize,
        display_offset: usize,
    ) -> (egui::Rect, f32) {
        let total = (history_size + screen_lines).max(1);
        let thumb_ratio = screen_lines as f32 / total as f32;
        let thumb_h = (track.height() * thumb_ratio).max(SCROLLBAR_MIN_THUMB_HEIGHT);
        let avail = track.height() - thumb_h;
        let pos_ratio = if history_size > 0 {
            (history_size - display_offset) as f32 / history_size as f32
        } else {
            1.0
        };
        let thumb_y = track.top() + avail * pos_ratio;
        let thumb = egui::Rect::from_min_max(
            egui::pos2(track.left(), thumb_y),
            egui::pos2(track.right(), (thumb_y + thumb_h).min(track.bottom())),
        );
        (thumb, thumb_h)
    }

    /// Render a custom overlay scrollbar on the right edge of the terminal area.
    pub fn render_scrollbar(&mut self, ui: &egui::Ui, rect: egui::Rect) {
        let history = self.terminal.history_size();
        let screen = self.terminal.size().rows as usize;
        let total = history + screen;
        if total == 0 {
            return;
        }

        let track = egui::Rect::from_min_max(
            egui::pos2(rect.right() - SCROLLBAR_WIDTH, rect.top()),
            egui::pos2(rect.right(), rect.bottom()),
        );

        let (thumb, _thumb_h) =
            Self::scrollbar_thumb_rect(track, screen, history, self.terminal.display_offset());

        let active = self.scrollbar_dragging || ui.rect_contains_pointer(track);

        // Track background.
        ui.painter()
            .rect_filled(track, 0.0, egui::Color32::from_black_alpha(if active { 40 } else { 15 }));

        // Thumb – only draw when there is actually something to scroll.
        if screen < total {
            ui.painter().rect_filled(
                thumb,
                4.0,
                egui::Color32::from_gray(if active { 160 } else { 100 }),
            );
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

