//! The main eframe application for Zenterm.
//!
//! Wires together the PTY session, terminal state machine, glyph atlas,
//! GPU renderer, and input mapper into a single egui application.
//!
//! # Rendering flow
//!
//! 1. [`update()`](Self::update) — pump PTY bytes, dispatch keyboard input.
//! 2. [`ui()`](Self::ui) — build cell instance data, register the
//!    [`egui_wgpu::Callback`] that renders the terminal grid via wgpu.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use egui::Context;

use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;

use zenterm_core::{Rgba, SubpixelLayout, TermSize};
use zenterm_core::theme::{Theme, ThemePreference, THEME_DARK};
use zenterm_config::Config;
use zenterm_glyph::{GlyphAtlas, GlyphContentType};
use zenterm_input::InputMapper;
use zenterm_pty::PtySession;
use zenterm_render::callback::{AtlasUpdate, SharedRenderState, TerminalWgpuCallback};
use zenterm_render::glyph_type;
use zenterm_render::CallbackHandle;
use zenterm_render::CellInstance;
use zenterm_term::Terminal;
use zenterm_term::ColorScheme;

// ── Colour helpers ──────────────────────────────────────────────────────

/// Convert a [`Theme`] background colour to `egui::Color32`.
///
/// The theme stores colours as premultiplied `Rgba` in linear space;
/// egui expects sRGB `Color32`.  We round-trip through 8-bit sRGB, which
/// is close enough for a terminal background.
fn theme_bg_to_color32(theme: &Theme) -> egui::Color32 {
    let b = theme.background;
    egui::Color32::from_rgba_premultiplied(
        (b.r() * 255.0).round().clamp(0.0, 255.0) as u8,
        (b.g() * 255.0).round().clamp(0.0, 255.0) as u8,
        (b.b() * 255.0).round().clamp(0.0, 255.0) as u8,
        (b.a() * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

/// Convert any [`Rgba`] to `egui::Color32`.
#[allow(dead_code)]
fn rgba_to_color32(c: Rgba) -> egui::Color32 {
    egui::Color32::from_rgba_premultiplied(
        (c.r() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.g() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.b() * 255.0).round().clamp(0.0, 255.0) as u8,
        (c.a() * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

/// The top-level application state.
pub struct ZentermApp {
    terminal: Terminal,
    pty: PtySession,
    glyph_atlas: GlyphAtlas,

    // ── Wgpu resources ──────────────────────────────────────────────────
    shared: Arc<SharedRenderState>,
    callback: CallbackHandle,

    // ── Layout cache ────────────────────────────────────────────────────
    cell_width: f32,
    cell_height: f32,

    /// Last known atlas texture size (for detecting atlas growth).
    last_atlas_size: u32,

    /// DPI scaling factor (physical pixels per logical point) used to
    /// compute `font_size`.  Tracked to detect DPI changes at runtime
    /// (e.g. window moved between monitors) so we can rebuild the glyph
    /// atlas at the correct physical size.
    pixels_per_point: f32,

    /// Physical viewport size (in pixels) from the previous frame.
    /// Used to detect viewport size changes that don't alter the terminal
    /// grid dimensions (rows/cols) but still require instance buffer
    /// updates because clip-space coordinates depend on `x_scale`/`y_scale`
    /// (which are derived from `vp_width_px` / `vp_height_px`).
    last_vp_size_px: [f32; 2],

    // ── Selection state ──────────────────────────────────────────────────
    /// True while the left mouse button is held and a drag-selection is
    /// in progress.
    selecting: bool,

    /// Set to `true` when terminal state changes (PTY data, selection,
    /// resize) so the next frame rebuilds GPU instances.
    terminal_dirty: bool,

    /// Timestamp (from `ctx.input(|i| i.time)`) of the most recent terminal
    /// resize, or `None` if no resize has occurred yet.  Used to show a
    /// transient size overlay after the window is resized.
    last_resize_at: Option<f64>,

    /// Frame counter for cursor blinking.  Incremented each frame;
    /// cursor is hidden when `(frame_count / blink_interval) % 2 == 0`.
    frame_count: u64,

    /// Number of frames between cursor blink toggles (30 frames ≈ 500 ms
    /// at 60 FPS).
    blink_interval: u64,

    /// Default background colour for the terminal area and for cells
    /// whose `cell.bg` equals the resolved `NamedColor::Background`.
    ///
    /// `egui::Color32` carries 4 channels — setting alpha below 1.0
    /// makes the terminal see through to the OS desktop.  This only
    /// works in combination with `viewport.transparent(true)` in
    /// `main.rs`, which causes eframe to configure the wgpu surface
    /// with `CompositeAlphaMode::PreMultiplied`.
    ///
    /// Derived from the active [`Theme`] each frame.
    pub default_bg: egui::Color32,

    /// The active terminal colour scheme.
    ///
    /// Set during construction and updated when the system theme changes
    /// or the user explicitly switches themes.
    pub theme: Theme,

    /// The user's theme preference (Dark / Light / System).
    pub theme_preference: ThemePreference,

    /// Cached dark-mode flag from the previous frame so we can detect
    /// system-theme transitions without re-applying on every frame.
    last_system_dark: bool,

    /// Set to `true` once the PTY reader detects shell exit or the terminal
    /// emits an `Exit`/`ChildExit` event.  When `true`, [`pump_pty()`] is
    /// skipped to avoid logging "PTY reader disconnected" every frame.
    pty_exited: bool,

    /// Loaded configuration (TOML).
    config: Config,

    /// Error message to display as an overlay, typically from a failed
    /// config reload.  `None` means no error.
    error_toast: Option<String>,
}

impl ZentermApp {
    /// Create a new Zenterm application with the given wgpu resources.
    ///
    /// Spawns a shell in a PTY, sets up the terminal state machine, and
    /// pre-rasterises the initial glyph atlas.
    pub fn new_with_wgpu(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_format: wgpu::TextureFormat,
        pixels_per_point: f32,
        config: Config,
    ) -> Self {
        let size = TermSize::new(
            config.window.dimensions.lines,
            config.window.dimensions.columns,
        );

        let pty = PtySession::spawn(size).expect("failed to spawn PTY");
        let terminal = Terminal::new(size, Default::default());

        // Font size in physical pixels: config.font.size (logical units at
        // 1× DPI) × DPI scale factor.  At 200% scaling, 18 → 36 physical px.
        let font_size = config.font.size * pixels_per_point;
        let font_family = std::borrow::Cow::Owned(config.font.normal.family.clone());
        let mut glyph_atlas = GlyphAtlas::new(
            font_size,
            font_family,
            pixels_per_point,
            SubpixelLayout::detect(),
        );

        // ── Critical ordering ─────────────────────────────────────────
        // `cell_size()` measures `cell_ascent` / `cell_descent` via
        // cosmic-text, which the per-glyph `compute_glyph_scale` reads
        // while rasterising each glyph.  If we seed the atlas with ASCII
        // characters *before* measuring, those characters are rasterised
        // with `cell_ascent = 0` and end up scaled to 0.1 (clamp floor),
        // collapsing to a single dot.  Measure FIRST, seed AFTER.
        let (cell_width, cell_height) = glyph_atlas
            .cell_size()
            .expect("failed to measure cell size");

        // Seed the atlas with a few common ASCII characters so the first
        // frame has something to render before the user types anything.
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 .,!?;:-=+*/\\|()[]{}<>\"'`~@#$%^&_"
            .chars()
        {
            let _ = glyph_atlas.ensure_glyph(c);
        }

        let shared = Arc::new(SharedRenderState::new(
            size.rows as usize * size.cols as usize,
        ));

        // Build the callback; its pipeline + bind group will be created
        // lazily on the first `prepare()` call.
        let callback = TerminalWgpuCallback::new(
            device.clone(),
            queue.clone(),
            target_format,
            shared.clone(),
        );
        let callback = CallbackHandle::new(callback);

        let last_atlas_size = glyph_atlas.texture_size;

        // Signal the initial atlas data so the callback creates its
        // GPU texture and render pass on the very first prepare() call.
        {
            let mut update = shared.atlas_update.lock().unwrap();
            *update = Some(AtlasUpdate {
                size: last_atlas_size,
                data: glyph_atlas.texture_data.clone(),
                resized: true,
            });
        }
        shared.atlas_dirty.store(true, Ordering::Release);

        Self {
            terminal,
            pty,
            glyph_atlas,
            shared,
            callback,
            cell_width,
            cell_height,
            last_atlas_size,
            pixels_per_point,
            last_vp_size_px: [0.0, 0.0],
            selecting: false,
            terminal_dirty: true,
            last_resize_at: None,
            frame_count: 0,
            blink_interval: config.cursor.blink_interval,
            theme: THEME_DARK.clone(),
            theme_preference: match config.colors.theme {
                zenterm_config::colors::ThemePreference::Dark => ThemePreference::Dark,
                zenterm_config::colors::ThemePreference::Light => ThemePreference::Light,
                zenterm_config::colors::ThemePreference::System => ThemePreference::System,
            },
            last_system_dark: true,
            default_bg: theme_bg_to_color32(&THEME_DARK),
            pty_exited: false,
            config,
            error_toast: None,
        }
    }

    /// Synchronise the active theme with the user's preference and the
    /// OS system theme.
    ///
    /// Call this once per frame from [`Self::update()`].  If the resolved
    /// theme has changed, the terminal's colour scheme is rebuilt and the
    /// entire grid is marked dirty for re-rendering.
    fn sync_theme(&mut self, egui_ctx: &egui::Context) {
        // Determine whether the OS is currently in dark mode.
        // egui 0.34's `RawInput::system_theme` is populated by eframe
        // via winit's `ThemeChanged` event on all platforms.
        let system_dark = egui_ctx.input(|i| {
            match i.raw.system_theme {
                Some(egui::Theme::Dark) => true,
                Some(egui::Theme::Light) => false,
                None => true, // fallback → dark
            }
        });

        let new_theme = self.config.colors.to_theme(system_dark);

        // Detect transitions.
        let changed = new_theme != self.theme || system_dark != self.last_system_dark;

        // Always update the cached flag so we can detect future changes.
        self.last_system_dark = system_dark;

        if changed {
            log::info!(
                "theme: {} (pref={:?}, system_dark={})",
                new_theme.name,
                self.theme_preference,
                system_dark,
            );
            self.theme = new_theme.clone();
            self.default_bg = theme_bg_to_color32(&new_theme);

            // Push the new colour scheme into the terminal state machine so
            // that future `visible_cells()` calls resolve colours correctly.
            let scheme = ColorScheme::from_theme(&new_theme);
            self.terminal.set_scheme(scheme);
        }
    }

    /// Pump pending PTY bytes into the terminal state machine and write
    /// any terminal-query response bytes back to the PTY.
    ///
    /// Also polls [`PtySession::try_wait()`] to detect shell exit on
    /// platforms where the PTY reader does not automatically receive EOF
    /// when the child process terminates (notably **Windows ConPTY**).
    ///
    /// When the shell exits this sets `self.pty_exited = true`; the next
    /// frame will close the application via [`update()`].
    fn pump_pty(&mut self) {
        if self.pty_exited {
            return;
        }

        // 1. Drain available PTY bytes.
        let mut total = 0usize;
        while let Some(result) = self.pty.try_read() {
            match result {
                Ok(data) => {
                    total += data.len();

                    // Feed the terminal state machine — this also collects
                    // response bytes for any terminal queries (DA, DSR,
                    // DECRPM, OSC colour queries, …) that were embedded
                    // in the PTY output.
                    let replies = self.terminal.feed(&data);
                    if !replies.is_empty() {
                        log::debug!("pump_pty: writing {} reply bytes: {:02x?}", replies.len(), &replies);
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

        // 2. Check if the child process has exited.
        //
        //    On Unix PTY this is redundant (the reader thread above gets
        //    EOF and already signalled exit).  On Windows ConPTY the
        //    output pipe is NOT closed when the child exits, so the
        //    reader thread blocks forever — we must poll the child
        //    process directly.
        if !self.pty_exited {
            if let Some(status) = self.pty.try_wait() {
                log::info!("shell exited with status: {status:?}, closing");
                self.pty.close();
                self.pty_exited = true;
            }
        }
    }

    /// Build `CellInstance` vector from the current visible grid and
    /// store it in the shared render state.
    ///
    /// Each glyph is rendered at its **native pixel size** positioned at
    /// the bearing offset within the cell — following the approach used
    /// by Alacritty and WezTerm.  The cell background is provided by
    /// egui's `rect_filled` underneath.
    ///
    /// If the terminal cursor is visible, the cell at the cursor position
    /// has its foreground and background colours **swapped** (inverse
    /// video block cursor).  A blank cell at the cursor position is
    /// rendered as a white block.
    fn update_cell_instances(
        &mut self,
        vp_width_px: f32,
        vp_height_px: f32,
    ) {
        // Read cursor info BEFORE visible_cells() since both borrow
        // self.terminal (one mut, one immut).
        let cursor = self.terminal.cursor();
        let cursor_row = cursor.pos.line;
        let cursor_col = cursor.pos.column;

        // Handle cursor blinking.
        // cursor.style.blinking indicates the application requested a
        // blinking cursor via DECSCUSR.  We additionally force blink off
        // when the cursor is a Block (most users expect block = steady).
        let blink_on = if cursor.style.blinking && !matches!(cursor.style.shape, CursorShape::Block) {
            (self.frame_count / self.blink_interval) % 2 == 0
        } else {
            true
        };
        let cursor_visible = cursor.visible && blink_on;
        let cursor_shape = cursor.style.shape;

        // Pre-compute selection range so the inner loop does not need
        // an additional borrow on self.terminal during grid iteration.
        let sel_range: Option<SelectionRange> = self.terminal.selection_range();
        let sel_bg = self.terminal.selection_bg();
        let sel_fg = self.terminal.selection_fg();
        // Default background colour (resolved `NamedColor::Background`).
        // Cells whose `cell.bg` equals this value don't get their own
        // background quad — the terminal-wide `rect_filled` covers them.
        // Snapshotting it here avoids re-borrowing `self.terminal`
        // inside the cell-iteration loop (which holds a mutable borrow
        // via `visible_cells()`).
        let default_bg = self.terminal.default_bg();

        let grid = self.terminal.visible_cells();
        let rows = grid.row_count();
        let cols = grid.col_count();

        log::debug!(
            "update_cell_instances: {}x{} terminal, viewport {:.0}x{:.0}px, cell {:.1}x{:.1}px",
            cols,
            rows,
            vp_width_px,
            vp_height_px,
            self.cell_width,
            self.cell_height,
        );

        // Early-out for empty terminal.
        if rows == 0 || cols == 0 {
            log::warn!("update_cell_instances: empty terminal grid");
            return;
        }

        let tex_size = self.glyph_atlas.texture_size as f32;
        let cw = self.cell_width;
        let ch = self.cell_height;

        // Pre-compute pixel → clip-space conversion.
        let x_scale = 2.0 / vp_width_px;
        let y_scale = 2.0 / vp_height_px;

        // Authoritative baseline position from cosmic-text.
        // This is the y-down distance from the cell TOP to the baseline, in
        // pixels.  Glyphs are positioned such that
        //
        //   glyph_top_y = row * ch + baseline - glyph_bearing_y
        //
        // which is equivalent to alacritty's `(line+1)*ch - (ascent - descent)`
        // and wezterm's `cell_height + descender - bearing_y` (with descender
        // negative).  See `GlyphAtlas::cell_baseline_offset`.
        let baseline = self.glyph_atlas.cell_baseline_offset();
        let mut bg_instances = Vec::with_capacity(rows * cols);
        let mut glyph_instances = Vec::with_capacity(rows * cols);
        let mut deco_instances = Vec::with_capacity(rows * cols);
        let mut blank_count = 0u32;
        let mut glyph_fail = 0u32;
        let mut has_new_glyphs = false;

        for row in 0..rows {
            for col in 0..cols {
                let cell = match grid.cell(row, col) {
                    Some(c) => c,
                    None => continue,
                };

                let is_cursor = cursor_visible && row == cursor_row && col == cursor_col;
                let is_block_cursor = is_cursor && matches!(cursor_shape, CursorShape::Block);

                // Check selection membership.
                let is_sel = sel_range.as_ref().is_some_and(|range| {
                    let pt = Point::new(Line(row as i32), Column(col));
                    range.contains(pt)
                });

                let ch_char = cell.c;
                let is_blank = ch_char == ' ' && cell.bg == Rgba::BLACK && !is_cursor;

                // Block cursor: swap fg/bg so the cell renders with
                // inverted colours — exactly as Alacritty does.
                let (draw_fg, draw_bg) = if is_block_cursor {
                    (cell.bg, cell.fg)
                } else {
                    (cell.fg, cell.bg)
                };

                // Skip spacer cells of wide characters (CJK / emoji).
                // The wide char itself occupies the leading cell; spacers
                // share its glyph and don't need their own instance.
                if cell.is_spacer {
                    blank_count += 1;
                    continue;
                }

                // Hidden cells (e.g. password fields) render background
                // but not the glyph.
                if cell.hidden {
                    blank_count += 1;
                    continue;
                }

                // ── Geometry helpers ────────────────────────────────────
                // Pixel → clip-space conversion.
                let px_to_clip_x = |px: f32| px * x_scale - 1.0;
                let px_to_clip_y = |px: f32| 1.0 - px * y_scale;

                // ── Pass 1: Background quad (all cells except cursor) ──────────
                // We paint a SOLID quad for every non-cursor cell, using
                // either the selection color (for selected cells) or the
                // cell's own bg color.  Cells whose bg equals the terminal
                // default (NamedColor::Background) are SKIPPED — the
                // terminal-wide `rect_filled` at the bottom of `ui()`
                // (using `default_bg`) already covers them.  This is the
                // cosmic-term pattern (terminal_box.rs:576:
                // `if metadata.bg != default_metadata.bg`) and avoids
                // redundant work for cells that use the default colour.
                //
                // Why this matters: TUI programs like `tidev-tui` paint
                // their main area via
                //   Block::default().style(Style::default().bg(palette.background))
                // which produces SGR `\x1b[48;2;R;G;Bm` for *every* cell.
                // zenterm correctly resolves those into `cell.bg`, but if
                // we never draw a quad for non-selection cells, the
                // underlying `rect_filled` (BLACK by default) shows
                // through and the TUI's light background disappears.
                if !is_cursor || is_block_cursor {
                    let cell_bg = if is_sel { sel_bg } else { draw_bg };
                    if is_block_cursor || cell_bg != default_bg {
                        // `cw` and `ch` are both integers (see
                        // `GlyphAtlas::cell_size`), so the cell positions
                        // align perfectly with the pixel grid: no sub-pixel
                        // drift between adjacent rows or columns, and
                        // therefore no 1-px "fringe" where coloured cell
                        // backgrounds meet.  `.round()` is kept as a
                        // defensive no-op so future font-size changes
                        // still work.
                        let bg_x_px = (col as f32 * cw).round();
                        let bg_y_px = (row as f32 * ch).round();
                        let bqx = px_to_clip_x(bg_x_px);
                        let bqy = px_to_clip_y(bg_y_px);
                        let bqw = cw * x_scale;
                        let bqh = ch * y_scale;

                        bg_instances.push(CellInstance {
                            clip_pos: [bqx, bqy],
                            uv_min: [0.0; 2],
                            uv_max: [0.0; 2],
                            clip_cell_size: [bqw, bqh],
                            glyph_size: [0.0; 2],
                            glyph_offset: [0.0; 2],
                            // Pass alpha through to the shader; SOLID
                            // path will pre-multiply the colour by alpha
                            // (matching `CompositeAlphaMode::PreMultiplied`).
                            fg_color: [
                                cell_bg.r(),
                                cell_bg.g(),
                                cell_bg.b(),
                                cell_bg.a(),
                            ],
                            bg_color: [
                                cell_bg.r(),
                                cell_bg.g(),
                                cell_bg.b(),
                                cell_bg.a(),
                            ],
                            flags: glyph_type::SOLID,
                        });
                    }
                }

                // ── Pass 2: Glyph quad (non-cursor cells only) ──────────
                // Cursor cells are deferred to Pass 3b so they render
                // on top of the cursor block background.
                // Skip fully blank cells (space on default background,
                // no cursor).  Selected blank cells already got their
                // background quad in Pass 1.
                if is_blank {
                    blank_count += 1;
                } else {
                    // Look up — or rasterise — the glyph.
                    let lookup = self.glyph_atlas.ensure_glyph(ch_char);
                    if let Ok((entry, is_new)) = lookup {
                        if is_new {
                            has_new_glyphs = true;
                        }

                        let atlas_w = (entry.atlas_rect.max.x - entry.atlas_rect.min.x) as f32;
                        let atlas_h = (entry.atlas_rect.max.y - entry.atlas_rect.min.y) as f32;

                        // Every cell renders a glyph-sized quad at the bearing
                        // offset (native-resolution text).  The y position is
                        //
                        //   glyph_y_px = row * ch + baseline - bearing_y
                        //
                        // i.e. glyph_top = baseline - bearing_y, where
                        // `baseline` is the cell's baseline offset measured
                        // by cosmic-text (== ascent for the reference 'M'
                        // glyph).  For a full-height glyph with
                        // `bearing_y ≈ baseline`, this puts the glyph top at
                        // the cell top, exactly as in alacritty / wezterm.
                        // The previous formula `ch - bearing_y` implicitly
                        // placed the baseline at the cell BOTTOM, which
                        // pushed text `ch - 2*baseline ≈ descent+leading`
                        // pixels too low — that's what made the cursor
                        // appear to overlap the line above.
                        //
                        // `entry.scale` < 1.0 for CJK glyphs whose natural
                        // `placement.top` exceeds `cell_ascent`: scaling the
                        // bitmap and its bearings down makes the CJK fit
                        // inside the cell (top no longer clipped) — the same
                        // strategy wezterm uses (`glyph.scale`).  For ASCII
                        // glyphs the value is ≈ 1.0.
                        let scale = entry.scale;
                        let mut scaled_w = atlas_w * scale;
                        let mut scaled_h = atlas_h * scale;
                        let sbx = entry.bearing_x * scale;
                        let sby = entry.bearing_y * scale;

                        let mut glyph_x_px = (col as f32 * cw + sbx).round();
                        let mut glyph_y_px = (row as f32 * ch + (baseline - sby)).round();

                        // ── UV coordinates (pre-clip) ──────────────
                        let mut u_min = (entry.atlas_rect.min.x as f32 + 0.5) / tex_size;
                        let mut v_min = (entry.atlas_rect.min.y as f32 + 0.5) / tex_size;
                        let mut u_max = (entry.atlas_rect.max.x as f32 - 0.5) / tex_size;
                        let mut v_max = (entry.atlas_rect.max.y as f32 - 0.5) / tex_size;

                        // ── Clip glyph quad to cell boundaries ─────
                        // Bitmap padding (empty rows/columns from
                        // swash's bounding-box rounding) can extend
                        // beyond the cell.  Clip the quad and adjust
                        // UV so only the visible part is rendered.
                        let cell_left = col as f32 * cw;
                        let cell_top = row as f32 * ch;
                        let cell_right = cell_left + cw;
                        let cell_bottom = cell_top + ch;

                        // Vertical clip
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

                        // Horizontal clip
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

                        // Map from atlas content type to shader dispatch flag.
                        let gtype = match entry.content_type {
                            GlyphContentType::Subpixel => glyph_type::SUBPIXEL,
                            GlyphContentType::Mask => glyph_type::MASK,
                            GlyphContentType::Color => glyph_type::COLOR,
                        };

                        if is_cursor && !is_block_cursor {
                            // Non-block cursor: draw glyph normally.
                            glyph_instances.push(CellInstance {
                                clip_pos: [gqx, gqy],
                                uv_min: [u_min, v_min],
                                uv_max: [u_max, v_max],
                                clip_cell_size: [gqw, gqh],
                                glyph_size: [scaled_w, scaled_h],
                                glyph_offset: [
                                    sbx,
                                    baseline - sby,
                                ],
                                fg_color: [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0],
                                bg_color: [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0],
                                flags: gtype,
                            });
                    } else if is_sel {
                        // Selected: use configured selection colours.
                        let glyph_fg = sel_fg.unwrap_or(cell.fg);
                        glyph_instances.push(CellInstance {
                            clip_pos: [gqx, gqy],
                            uv_min: [u_min, v_min],
                            uv_max: [u_max, v_max],
                            clip_cell_size: [gqw, gqh],
                            glyph_size: [scaled_w, scaled_h],
                            glyph_offset: [
                                sbx,
                                baseline - sby,
                            ],
                            fg_color: [glyph_fg.r(), glyph_fg.g(), glyph_fg.b(), 1.0],
                            bg_color: [sel_bg.r(), sel_bg.g(), sel_bg.b(), 1.0],
                            flags: gtype,
                        });                        } else {
                            // Normal cell (including block cursor
                            // whose draw_fg/draw_bg are already swapped).
                            glyph_instances.push(CellInstance {
                                clip_pos: [gqx, gqy],
                                uv_min: [u_min, v_min],
                                uv_max: [u_max, v_max],
                                clip_cell_size: [gqw, gqh],
                                glyph_size: [scaled_w, scaled_h],
                                glyph_offset: [
                                    sbx,
                                    baseline - sby,
                                ],
                                fg_color: [draw_fg.r(), draw_fg.g(), draw_fg.b(), 1.0],
                                bg_color: [draw_bg.r(), draw_bg.g(), draw_bg.b(), 1.0],
                                flags: gtype,
                            });
                        }
                    } else {
                        glyph_fail += 1;
                    }
                }

                // ── Pass 4: Decorations (underline / strikethrough) ─────
                // Thin solid-color bars rendered on top of glyphs.
                // These are emitted even for blank cells (e.g. selected
                // blank cells with underline flags).
                let deco_color = if is_cursor {
                    // Match the glyph colour: both block and non-block
                    // cursors use cell.bg (the original, unswapped value)
                    // as the text colour.
                    [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0]
                } else if is_sel {
                    [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0]
                } else {
                    [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0]
                };

                if cell.underline {
                    let thickness = 1.0_f32.max((ch * 0.05).round());
                    // Underline sits 1 px below the baseline (i.e. just
                    // inside the descender area).  Previously this used
                    // `ch - thickness`, which put the underline flush with
                    // the cell bottom — visually wrong because the cell
                    // bottom is the *descender* area, not the underline
                    // area.  Using `baseline` (from cosmic-text) puts the
                    // underline where alacritty / wezterm put it.
                    let deco_y = baseline + 1.0;
                    let dqy = px_to_clip_y((row as f32 * ch + deco_y).round());
                    let dqx = px_to_clip_x((col as f32 * cw).round());
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
                    // Strikethrough goes through the visual middle of the
                    // x-height.  This is approximately at `baseline / 2`
                    // for a typical font, which is what the previous
                    // `ch * 0.45` formula approximated.  With the new
                    // cell geometry we anchor it relative to the baseline
                    // for stability across font-size changes.
                    let deco_y = (baseline * 0.55).round();
                    let dqy = px_to_clip_y((row as f32 * ch + deco_y).round());
                    let dqx = px_to_clip_x((col as f32 * cw).round());
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

                // ── Cursor style decorations ──────────────────────────
                // For non-Block cursors we draw a thin bar instead of the
                // full-cell inverse-video background.
                if is_cursor && !is_block_cursor {
                    let cursor_color = [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0];
                    let thickness = 2.0_f32.max((ch * 0.08).round());
                    let cx_px = (col as f32 * cw).round();
                    let cy_px = (row as f32 * ch).round();

                    match cursor_shape {
                        CursorShape::Underline => {
                            // Horizontal bar at the bottom of the cell.
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
                            // Vertical bar on the left side of the cell.
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
                        CursorShape::HollowBlock => {
                            // Border rectangle (four thin bars).
                            let border = thickness.max(1.0);
                            // Top
                            deco_instances.push(CellInstance {
                                clip_pos: [px_to_clip_x(cx_px), px_to_clip_y(cy_px)],
                                uv_min: [0.0; 2],
                                uv_max: [0.0; 2],
                                clip_cell_size: [cw * x_scale, border * y_scale],
                                glyph_size: [0.0; 2],
                                glyph_offset: [0.0; 2],
                                fg_color: cursor_color,
                                bg_color: cursor_color,
                                flags: glyph_type::SOLID,
                            });
                            // Bottom
                            deco_instances.push(CellInstance {
                                clip_pos: [px_to_clip_x(cx_px), px_to_clip_y(cy_px + ch - border)],
                                uv_min: [0.0; 2],
                                uv_max: [0.0; 2],
                                clip_cell_size: [cw * x_scale, border * y_scale],
                                glyph_size: [0.0; 2],
                                glyph_offset: [0.0; 2],
                                fg_color: cursor_color,
                                bg_color: cursor_color,
                                flags: glyph_type::SOLID,
                            });
                            // Left
                            deco_instances.push(CellInstance {
                                clip_pos: [px_to_clip_x(cx_px), px_to_clip_y(cy_px)],
                                uv_min: [0.0; 2],
                                uv_max: [0.0; 2],
                                clip_cell_size: [border * x_scale, ch * y_scale],
                                glyph_size: [0.0; 2],
                                glyph_offset: [0.0; 2],
                                fg_color: cursor_color,
                                bg_color: cursor_color,
                                flags: glyph_type::SOLID,
                            });
                            // Right
                            deco_instances.push(CellInstance {
                                clip_pos: [px_to_clip_x(cx_px + cw - border), px_to_clip_y(cy_px)],
                                uv_min: [0.0; 2],
                                uv_max: [0.0; 2],
                                clip_cell_size: [border * x_scale, ch * y_scale],
                                glyph_size: [0.0; 2],
                                glyph_offset: [0.0; 2],
                                fg_color: cursor_color,
                                bg_color: cursor_color,
                                flags: glyph_type::SOLID,
                            });
                        }
                        _ => {} // Block, Hidden handled elsewhere.
                    }
                }
            }
        }

        // Concatenate: backgrounds → glyphs → decorations.
        // This ensures correct z-order:
        //   1. Cell backgrounds (below all text)
        //   2. All glyphs (text from all rows, including block cursor
        //      whose fg/bg are already swapped)
        //   3. Underline / strikethrough / cursor bars (topmost)
        let bg_count = bg_instances.len();
        let glyph_count = glyph_instances.len();
        let deco_count = deco_instances.len();
        let mut instances = bg_instances;
        instances.extend(glyph_instances);
        instances.extend(deco_instances);
        let total_instances = instances.len();

        log::debug!(
            "update_cell_instances: {} total ({} bg + {} glyph + {} deco), \
             {} blank skipped, {} glyph failures",
            total_instances,
            bg_count,
            glyph_count,
            deco_count,
            blank_count,
            glyph_fail,
        );

        // Store for the callback's `prepare()`.
        *self.shared.instances.lock().unwrap() = instances;
        self.shared.instance_gen.fetch_add(1, Ordering::Release);

        // ── Sync glyph atlas to GPU ──────────────────────────────────
        // Upload when the atlas has grown OR when new glyphs were added.
        let current_size = self.glyph_atlas.texture_size;
        let resized = current_size != self.last_atlas_size;
        if resized {
            self.last_atlas_size = current_size;
        }
        if resized || has_new_glyphs {
            *self.shared.atlas_update.lock().unwrap() = Some(AtlasUpdate {
                size: current_size,
                data: self.glyph_atlas.texture_data.clone(),
                resized,
            });
            self.shared.atlas_dirty.store(true, Ordering::Release);
        }
    }

    /// Send an SGR mouse event to the PTY if mouse reporting is active.
    ///
    /// Format: `\x1b[<row;col;buttonM` (press) / `\x1b[<row;col;buttonm` (release).
    /// Row and column are 1-based.
    fn send_sgr_mouse(&mut self, row: usize, col: usize, button: u8, release: bool) {
        // Only send if the terminal has enabled SGR mouse mode AND
        // some form of mouse event reporting.
        let mode = self.terminal.mode();
        let mouse_active = mode.contains(TermMode::SGR_MOUSE)
            && mode.intersects(TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION);
        if !mouse_active {
            return;
        }

        // SGR: \x1b[<row;col;buttonM  (press) / m (release)
        // row/col are 1-based, button encodes which button + modifiers.
        let suffix = if release { "m" } else { "M" };
        let seq = format!("\x1b[{};{};{}{}", row + 1, col + 1, button, suffix);
        if let Err(e) = self.pty.write(seq.as_bytes()) {
            log::error!("SGR mouse write error: {e}");
        }
    }

    /// Re-initialise the glyph atlas and cell metrics for a new DPI scale
    /// factor.  Called when the window moves between monitors with different
    /// DPI settings.
    fn reinit_for_dpi(&mut self, new_ppp: f32) {
        let new_font_size = self.config.font.size * new_ppp;
        let font_family = std::borrow::Cow::Owned(self.config.font.normal.family.clone());

        // Rebuild the glyph atlas (clears all cached glyphs so they are
        // re-rasterised at the new font size on the next `ensure_glyph`).
        self.glyph_atlas = GlyphAtlas::new(
            new_font_size,
            font_family,
            new_ppp,
            SubpixelLayout::detect(),
        );

        // Re-measure cell geometry.
        let (cw, ch) = self
            .glyph_atlas
            .cell_size()
            .expect("glyph atlas cell_size after DPI reinit");
        self.cell_width = cw;
        self.cell_height = ch;

        // Re-seed the atlas with common ASCII characters so the next
        // frame has something to render before the user types anything.
        let ascii_chars =
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 .,!?;:-=+*/\\|()[]{}<>\"'`~@#$%^&_";
        for c in ascii_chars.chars() {
            let _ = self.glyph_atlas.ensure_glyph(c);
        }

        // Push the new atlas texture data to the GPU.
        let tex_size = self.glyph_atlas.texture_size;
        {
            let mut update = self.shared.atlas_update.lock().unwrap();
            *update = Some(AtlasUpdate {
                size: tex_size,
                data: self.glyph_atlas.texture_data.clone(),
                resized: true,
            });
        }
        self.shared.atlas_dirty.store(true, Ordering::Release);
        self.last_atlas_size = tex_size;

        // Mark everything dirty so the next frame rebuilds instances.
        self.terminal_dirty = true;
        self.pixels_per_point = new_ppp;

        log::info!(
            "DPI reinit: new_ppp={new_ppp:.2} font_size={new_font_size:.1} \
             cw={cw:.1} ch={ch:.1}",
        );
    }

    // ── Config hot-reload ─────────────────────────────────────────────

    /// Re-read the config file from disk and apply changes that can be
    /// applied at runtime.
    ///
    /// Settings that require a restart (window dimensions, shell, …) are
    /// silently deferred — the user will see them after the next launch.
    fn reload_config(&mut self, egui_ctx: &egui::Context) {
        match Config::reload() {
            Ok(Some(cfg)) => {
                log::info!("config reloaded, applying changes");
                let old_config = std::mem::replace(&mut self.config, cfg);

                // ── Theme changes ─────────────────────────────────────
                // Rebuild colour scheme from the new config + current
                // system theme state.
                let system_dark = egui_ctx.input(|i| {
                    match i.raw.system_theme {
                        Some(egui::Theme::Dark) => true,
                        Some(egui::Theme::Light) => false,
                        None => true,
                    }
                });
                let new_theme = self.config.colors.to_theme(system_dark);
                self.theme = new_theme.clone();
                self.default_bg = theme_bg_to_color32(&new_theme);
                let scheme = ColorScheme::from_theme(&new_theme);
                self.terminal.set_scheme(scheme);

                // ── Cursor / blink ────────────────────────────────────
                self.blink_interval = self.config.cursor.blink_interval;

                // ── Font size ─────────────────────────────────────────
                // Rebuild glyph atlas only when the *logical* size changed
                // (the physical size depends on DPI which is handled by
                // `reinit_for_dpi`).
                let font_size_changed =
                    (self.config.font.size - old_config.font.size).abs() > f32::EPSILON;
                if font_size_changed {
                    let new_font_size = self.config.font.size * self.pixels_per_point;
                    let font_family =
                        std::borrow::Cow::Owned(self.config.font.normal.family.clone());
                    self.glyph_atlas = GlyphAtlas::new(
                        new_font_size,
                        font_family,
                        self.pixels_per_point,
                        SubpixelLayout::detect(),
                    );
                    let (cw, ch) = self
                        .glyph_atlas
                        .cell_size()
                        .expect("cell_size after font-size reload");
                    self.cell_width = cw;
                    self.cell_height = ch;
                    // Re-seed ASCII.
                    for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 .,!?;:-=+*/\\|()[]{}<>\"'`~@#$%^&_"
                        .chars()
                    {
                        let _ = self.glyph_atlas.ensure_glyph(c);
                    }
                    // Push atlas to GPU.
                    let tex_size = self.glyph_atlas.texture_size;
                    {
                        let mut update = self.shared.atlas_update.lock().unwrap();
                        *update = Some(AtlasUpdate {
                            size: tex_size,
                            data: self.glyph_atlas.texture_data.clone(),
                            resized: true,
                        });
                    }
                    self.shared.atlas_dirty.store(true, Ordering::Release);
                    self.last_atlas_size = tex_size;
                }

                self.terminal_dirty = true;
                self.error_toast = None;

                log::info!("config reload complete");
            }
            Ok(None) => {
                // File removed — keep current config, just ack.
                log::info!("config file removed, keeping current settings");
                self.error_toast = None;
            }
            Err(e) => {
                log::error!("config reload failed: {e}");
                self.error_toast = Some(format!(
                    "Config error — keeping old settings:\n{}",
                    e
                ));
            }
        }
    }
}

impl eframe::App for ZentermApp {
    /// Override eframe's default clear colour.
    ///
    /// The default (`rgba(12, 12, 12, 180)`) is a dark semi-transparent
    /// grey that exists "to make shadows look right".  We return
    /// fully-transparent instead so the OS desktop shows through
    /// wherever no opaque cell background is drawn — combined with
    /// `viewport.transparent(true)` in `main.rs` (which causes eframe
    /// to configure `CompositeAlphaMode::PreMultiplied` on the wgpu
    /// surface) and our pre-multiplied SOLID shader, this gives us a
    /// terminal that can be light, dark, or fully see-through.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    /// Phase 1: non-UI work.
    ///
    /// This is called before [`Self::ui()`] each frame.
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // 0. Synchronise theme with system / user preference.
        self.sync_theme(ctx);

        // 0.5. Detect DPI changes (window moved between monitors).
        let current_ppp = ctx.pixels_per_point();
        if (current_ppp - self.pixels_per_point).abs() > 0.01 {
            self.reinit_for_dpi(current_ppp);
        }

        // 1. Read pending PTY bytes and feed the terminal parser.
        self.pump_pty();

        // 1.5. Consume terminal side-effects produced during feed().
        //
        //      These are terminal-escape-sequence-driven requests that
        //      cannot be satisfied by merely writing bytes back to the PTY.
        //
        //      — Window title changes (OSC 0 / OSC 2)
        //      — Bell (BEL)
        //      — Exit / child-exit (shell termination → close app)
        //      — Clipboard store (OSC 52)
        //      — Clipboard load (OSC 52)
        {
            // Window title.
            if let Some(title) = self.terminal.take_title() {
                log::debug!("update: setting window title to {:?}", title);
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
            }

            // Bell — log for now; a visual / audio bell can be added later.
            if self.terminal.take_bell() {
                log::debug!("update: bell");
            }

            // Exit / child-exit → close the application.
            if self.terminal.take_exit() || self.terminal.take_child_exit().is_some() {
                log::info!("update: terminal requested exit, closing");
                self.pty_exited = true;
            }

            // Clipboard store (application wants us to save text).
            if let Some(text) = self.terminal.take_clipboard_store() {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    if let Err(e) = cb.set_text(text) {
                        log::error!("failed to store clipboard text: {e}");
                    }
                }
            }

            // Clipboard load (application wants us to read clipboard and
            // send the contents back to it as an escape sequence).
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
        }

        // 1.6. Close the application if the PTY/shell has exited.
        if self.pty_exited {
            log::info!("update: shell has exited, closing window");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // 2. Handle keyboard input.
        //
        //    Some key combinations (Ctrl+Shift+C, Ctrl+Shift+V, Ctrl+Shift+R)
        //    are terminal-emulator commands rather than shell input — check
        //    for those first by iterating events outside the ctx.input
        //    borrow so we can call ctx.copy_text() / clipboard_text().
        let (copy_requested, paste_requested, reload_requested) = ctx.input(|input| {
            let mut copy = false;
            let mut paste = false;
            let mut reload = false;
            for event in &input.events {
                match event {
                    egui::Event::Key {
                        key: egui::Key::C,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.ctrl && modifiers.shift && !modifiers.alt => {
                        copy = true;
                    }
                    egui::Event::Key {
                        key: egui::Key::V,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.ctrl && modifiers.shift && !modifiers.alt => {
                        paste = true;
                    }
                    egui::Event::Key {
                        key: egui::Key::R,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.ctrl && modifiers.shift && !modifiers.alt => {
                        reload = true;
                    }
                    _ => {}
                }
            }
            (copy, paste, reload)
        });

        if reload_requested {
            self.reload_config(ctx);
        }

        if copy_requested && self.terminal.has_selection() {
            if let Some(text) = self.terminal.selected_text() {
                ctx.copy_text(text);
                // Do NOT send Ctrl+C / 0x03 to the shell when we have a
                // selection — the user intended to copy, not interrupt.
            }
        } else if paste_requested {
            // Read clipboard and send as raw bytes to the PTY.
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                if let Ok(text) = clipboard.get_text() {
                    if !text.is_empty() {
                        if let Err(e) = self.pty.write(text.as_bytes()) {
                            log::error!("PTY paste error: {e}");
                        }
                    }
                }
            }
        } else {
            // Normal input: pass all events through InputMapper → PTY.
            ctx.input(|input| {
                for event in &input.events {
                    if let Some(bytes) = InputMapper::map(event) {
                        if let Err(e) = self.pty.write(&bytes) {
                            log::error!("PTY write error: {e}");
                        }
                    }
                }
            });
        }

        // 3. Advance frame counter and trigger rebuild for cursor blink.
        self.frame_count += 1;
        let needs_blink = self.terminal.cursor().style.blinking
            && !matches!(self.terminal.cursor().style.shape, CursorShape::Block);
        if needs_blink {
            // Ensure instances are rebuilt on blink boundaries.
            self.terminal_dirty = true;
        }

        // 4. Request continuous repainting.
        ctx.request_repaint();
    }

    /// Phase 2: UI rendering.
    ///
    /// Builds cell instance data from the terminal grid and registers the
    /// wgpu-based paint callback inside a `CentralPanel`.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // ── Config error toast ────────────────────────────────────────
        // Show a non-blocking error banner at the top when a config reload
        // failed.  The user can dismiss it with the "×" button.
        if let Some(msg) = &self.error_toast.clone() {
            let resp = egui::Panel::top("config_error")
                .resizable(false)
                .show_inside(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.colored_label(egui::Color32::RED, "⚠ Config error");
                        ui.label(msg);
                        if ui.button("×").clicked() {
                            self.error_toast = None;
                        }
                    });
                });
            // Prevent the error panel from consuming space if dismissed
            // later — TopBottomPanel already handles this via the `show`
            // guard above.
            let _ = resp;
        }
        // Use `Frame::NONE` for the CentralPanel so egui's default panel
        // fill (which is opaque) doesn't cover the transparent clear and
        // the OS desktop.  Our own `rect_filled` below provides the
        // configured `default_bg` (which itself can be transparent).
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {
            let available = ui.available_size();
            let pixels_per_point = ui.ctx().pixels_per_point();
            let vp_width_px = available.x * pixels_per_point;
            let vp_height_px = available.y * pixels_per_point;

            // ── Detect viewport pixel-size changes ─────────────────────
            // Even when the terminal grid dimensions (rows/cols) don't
            // change, the clip-space coordinates of every cell depend on
            // `x_scale = 2.0 / vp_width_px` and `y_scale = 2.0 / vp_height_px`.
            // A small viewport resize changes these scales, so we must
            // rebuild instances even if the grid count stays the same.
            // Without this, stale clip-space coordinates cause sub-pixel
            // misalignment of glyphs → color fringing / blur.
            let px_size = [vp_width_px, vp_height_px];
            if px_size != self.last_vp_size_px {
                self.last_vp_size_px = px_size;
                self.terminal_dirty = true;
            }

            // ── Resize terminal to match the available area ─────────────
            let cols = (vp_width_px / self.cell_width).max(10.0) as u16;
            let rows = (vp_height_px / self.cell_height).max(5.0) as u16;
            let new_size = TermSize::new(rows, cols);
            if new_size != self.terminal.size() {
                self.terminal.resize(new_size);
                self.pty.resize(new_size).ok();
                self.terminal_dirty = true;
                self.last_resize_at = Some(ui.input(|i| i.time));
            }

            // ── Allocate terminal area and capture interactions ─────────
            let sense = egui::Sense::click_and_drag();
            let (rect, response) = ui.allocate_exact_size(available, sense);

            // ── Mouse handling (selection + SGR mouse events) ────────
            // Check whether the application has requested mouse reporting.
            let mode = self.terminal.mode();
            let mouse_reporting = mode.contains(TermMode::SGR_MOUSE)
                && mode.intersects(TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION);

            // Helper: convert pixel position to grid cell.
            let cw = self.cell_width;
            let ch = self.cell_height;
            let pixel_to_cell = move |pos: egui::Pos2| -> Option<(usize, usize)> {
                let col = ((pos.x - rect.left()) / cw) as usize;
                let row = ((pos.y - rect.top()) / ch) as usize;
                if col < cols as usize && row < rows as usize {
                    Some((row, col))
                } else {
                    None
                }
            };

            if response.drag_started() {
                if let Some(pos) = response.interact_pointer_pos() {
                    if let Some((row, col)) = pixel_to_cell(pos) {
                        if mouse_reporting {
                            // Button 0 = left, no modifiers.
                            self.send_sgr_mouse(row, col, 0, false);
                        } else {
                            // Start text selection.
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
                    // Motion with button 0 pressed = button 32.
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
                    // Release with button 0.
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

            // Left-click without drag → clear selection (only when not
            // in mouse reporting mode, otherwise the app handles it).
            if response.clicked() && !self.selecting && !mouse_reporting {
                self.terminal.clear_selection();
                self.terminal_dirty = true;
            }

            // ── Build GPU instance data from visible cells ──────────────
            // Only rebuild when terminal state actually changed.
            if self.terminal_dirty {
                self.update_cell_instances(vp_width_px, vp_height_px);
                self.terminal_dirty = false;
            }

            // ── Register the wgpu paint callback ────────────────────────
            let callback = egui_wgpu::Callback::new_paint_callback(
                rect,
                self.callback.clone(),
            );

            // Draw the terminal background underneath the cells.
            // Uses the configurable `default_bg` (defaults to BLACK, but
            // can be set to TRANSPARENT or any other colour).  This is
            // the "default background" that cells with `cell.bg ==
            // NamedColor::Background` rely on — see Pass 1 in
            // `update_cell_instances`.
            ui.painter().rect_filled(rect, 0.0, self.default_bg);

            // Register the callback shape — egui-wgpu will call
            // prepare() and paint() on it.
            ui.painter().add(callback);

            // ── Transient resize overlay ─────────────────────────────────
            // Show current rows×cols in the center of the terminal for
            // ~2 seconds after each resize, then fade out.
            if let Some(last_time) = self.last_resize_at {
                let now = ui.input(|i| i.time);
                let elapsed = (now - last_time) as f32;
                if elapsed < 2.0 {
                    let size = self.terminal.size();
                    let text = format!("{} × {}", size.cols, size.rows);
                    // Fade alpha from 1.0 → 0.0 over 2 seconds.
                    let alpha = (1.0 - elapsed / 2.0).clamp(0.0, 1.0);

                    // Semi-transparent rounded backdrop for readability.
                    let backdrop = egui::Rect::from_center_size(
                        rect.center(),
                        egui::vec2(220.0, 64.0),
                    );
                    ui.painter().rect_filled(
                        backdrop,
                        10.0,
                        egui::Color32::BLACK.gamma_multiply(alpha * 0.55),
                    );

                    // White text, fading with the backdrop.
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::proportional(28.0),
                        egui::Color32::WHITE.gamma_multiply(alpha),
                    );
                }
            }

            // ── Right-click context menu ───────────────────────────────
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
        });
    }
}
