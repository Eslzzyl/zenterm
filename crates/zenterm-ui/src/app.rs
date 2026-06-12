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
use zenterm_glyph::{GlyphAtlas, GlyphContentType};
use zenterm_input::InputMapper;
use zenterm_pty::PtySession;
use zenterm_render::callback::{AtlasUpdate, SharedRenderState, TerminalWgpuCallback};
use zenterm_render::glyph_type;
use zenterm_render::CallbackHandle;
use zenterm_render::CellInstance;
use zenterm_term::Terminal;

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

    // ── Selection state ──────────────────────────────────────────────────
    /// True while the left mouse button is held and a drag-selection is
    /// in progress.
    selecting: bool,

    /// Set to `true` when terminal state changes (PTY data, selection,
    /// resize) so the next frame rebuilds GPU instances.
    terminal_dirty: bool,

    /// Frame counter for cursor blinking.  Incremented each frame;
    /// cursor is hidden when `(frame_count / blink_interval) % 2 == 0`.
    frame_count: u64,

    /// Number of frames between cursor blink toggles (30 frames ≈ 500 ms
    /// at 60 FPS).
    blink_interval: u64,
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
    ) -> Self {
        let size = TermSize::new(24, 80);

        let pty = PtySession::spawn(size).expect("failed to spawn PTY");
        let terminal = Terminal::new(size, Default::default());

        // Font size in physical pixels: logical points × DPI scale factor.
        // At 200% scaling, 18pt → 36 physical pixels.
        let font_size = 18.0 * pixels_per_point;
        let font_family = GlyphAtlas::default_font_family();
        let mut glyph_atlas = GlyphAtlas::new(
            font_size,
            font_family,
            pixels_per_point,
            SubpixelLayout::detect(),
        );

        // Seed the atlas with a few common ASCII characters so the first
        // frame has something to render before the user types anything.
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 .,!?;:-=+*/\\|()[]{}<>\"'`~@#$%^&_"
            .chars()
        {
            let _ = glyph_atlas.ensure_glyph(c);
        }

        let (cell_width, cell_height) = glyph_atlas.cell_size().expect("failed to measure cell size");

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
            selecting: false,
            terminal_dirty: true,
            frame_count: 0,
            blink_interval: 30,
        }
    }

    /// Pump pending PTY bytes into the terminal state machine.
    ///
    /// Also handles VT sequences that ConPTY/WinPTY sends during
    /// initialisation — most importantly **DSR** (`\x1b[6n`, "Cursor
    /// Position Report") which must be answered or the PTY may never
    /// deliver the shell prompt.
    fn pump_pty(&mut self) {
        let mut total = 0usize;
        while let Some(result) = self.pty.try_read() {
            match result {
                Ok(data) => {
                    total += data.len();

                    // ── Respond to Device Status Report ──────────────
                    // Windows ConPTY sends \x1b[6n on startup and expects
                    // \x1b[<row>;<col>R back.  Without this response the
                    // shell may never output its prompt.
                    if data.windows(4).any(|w| w == b"\x1b[6n") {
                        if let Err(e) = self.pty.write(b"\x1b[1;1R") {
                            log::error!("failed to write DSR response: {e}");
                        } else {
                            log::debug!("pump_pty: responded to DSR query");
                        }
                    }

                    self.terminal.feed(&data);
                }
                Err(e) => {
                    log::error!("PTY error: {e}");
                    break;
                }
            }
        }
        if total > 0 {
            log::debug!("pump_pty: read {} bytes from PTY", total);
            self.terminal_dirty = true;
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

        let mut bg_instances = Vec::with_capacity(rows * cols);
        let mut glyph_instances = Vec::with_capacity(rows * cols);
        let mut cursor_bg_instances = Vec::with_capacity(rows * cols);
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

                // ── Pass 1: Background quad (selected cells) ──────────
                // Full-cell solid-color rectangle.  Selection backgrounds
                // go here; cursor backgrounds are deferred to Pass 3 so
                // they render on top of all glyphs.
                if is_sel {
                    let bqx = ((col as f32 * cw).round()) * x_scale - 1.0;
                    let bqy = 1.0 - ((row as f32 * ch).round()) * y_scale;
                    let bqw = cw * x_scale;
                    let bqh = ch * y_scale;

                    bg_instances.push(CellInstance {
                        clip_pos: [bqx, bqy],
                        uv_min: [0.0; 2],
                        uv_max: [0.0; 2],
                        clip_cell_size: [bqw, bqh],
                        glyph_size: [0.0; 2],
                        glyph_offset: [0.0; 2],
                        fg_color: [sel_bg.r(), sel_bg.g(), sel_bg.b(), 1.0],
                        bg_color: [sel_bg.r(), sel_bg.g(), sel_bg.b(), 1.0],
                        flags: glyph_type::SOLID,
                    });
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
                        // offset (native-resolution text).  Positions are rounded
                        // to the nearest physical pixel so quad edges align with
                        // pixel boundaries.
                        let glyph_x_px = (col as f32 * cw + entry.bearing_x).round();
                        let glyph_y_px = (row as f32 * ch + (ch - entry.bearing_y)).round();
                        let gqx = px_to_clip_x(glyph_x_px);
                        let gqy = px_to_clip_y(glyph_y_px);
                        let gqw = atlas_w * x_scale;
                        let gqh = atlas_h * y_scale;

                        // ── UV coordinates ──────────────────────────────────
                        let u_min = (entry.atlas_rect.min.x as f32 + 0.5) / tex_size;
                        let v_min = (entry.atlas_rect.min.y as f32 + 0.5) / tex_size;
                        let u_max = (entry.atlas_rect.max.x as f32 - 0.5) / tex_size;
                        let v_max = (entry.atlas_rect.max.y as f32 - 0.5) / tex_size;

                        // Map from atlas content type to shader dispatch flag.
                        let gtype = match entry.content_type {
                            GlyphContentType::Subpixel => glyph_type::SUBPIXEL,
                            GlyphContentType::Mask => glyph_type::MASK,
                            GlyphContentType::Color => glyph_type::COLOR,
                        };

                        if is_block_cursor {
                            // ── DEBUG: log cursor geometry ─────────────
                            let cell_top = (row as f32 * ch).round();
                            let cell_bot = ((row as f32 + 1.0) * ch).round();
                            let gtop = (row as f32 * ch + (ch - entry.bearing_y)).round();
                            let gbot = gtop + atlas_h;
                            log::debug!(
                                "CURSOR row={} col={} ch={} cell=[{:.0},{:.0}] \
                                 glyph=[{:.0},{:.0}] atlas_h={} use_glyph={}",
                                row, col, ch, cell_top, cell_bot,
                                gtop, gbot, atlas_h,
                                atlas_w > 0.0 && atlas_h > 0.0,
                            );

                            // Deferred cursor block: rendered AFTER all
                            // other glyphs so it stays on top.
                            // Use CELL position + CELL size (full cell).
                            let bqy = 1.0 - cell_top * y_scale;
                            let bqx = ((col as f32 * cw).round()) * x_scale - 1.0;
                            let bqw = cw * x_scale;
                            let bqh = ch * y_scale;
                            // ── Background: SOLID fill with cell's fg colour ──
                            cursor_bg_instances.push(CellInstance {
                                clip_pos: [bqx, bqy],
                                uv_min: [0.0; 2],
                                uv_max: [0.0; 2],
                                clip_cell_size: [bqw, bqh],
                                glyph_size: [0.0; 2],
                                glyph_offset: [0.0; 2],
                                fg_color: [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0],
                                bg_color: [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0],
                                flags: glyph_type::SOLID,
                            });
                            // ── Glyph: inverse video, same pos+size ────
                            cursor_bg_instances.push(CellInstance {
                                clip_pos: [bqx, bqy],
                                uv_min: [u_min, v_min],
                                uv_max: [u_max, v_max],
                                clip_cell_size: [bqw, bqh],
                                glyph_size: [atlas_w, atlas_h],
                                glyph_offset: [entry.bearing_x, ch - entry.bearing_y],
                                fg_color: [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0],
                                bg_color: [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0],
                                flags: gtype,
                            });
                        } else if is_cursor {
                            // Non-block cursor: draw glyph normally.
                            glyph_instances.push(CellInstance {
                                clip_pos: [gqx, gqy],
                                uv_min: [u_min, v_min],
                                uv_max: [u_max, v_max],
                                clip_cell_size: [gqw, gqh],
                                glyph_size: [atlas_w, atlas_h],
                                glyph_offset: [entry.bearing_x, ch - entry.bearing_y],
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
                            glyph_size: [atlas_w, atlas_h],
                            glyph_offset: [entry.bearing_x, ch - entry.bearing_y],
                            fg_color: [glyph_fg.r(), glyph_fg.g(), glyph_fg.b(), 1.0],
                            bg_color: [sel_bg.r(), sel_bg.g(), sel_bg.b(), 1.0],
                            flags: gtype,
                        });                        } else {
                            // Normal cell.
                            glyph_instances.push(CellInstance {
                                clip_pos: [gqx, gqy],
                                uv_min: [u_min, v_min],
                                uv_max: [u_max, v_max],
                                clip_cell_size: [gqw, gqh],
                                glyph_size: [atlas_w, atlas_h],
                                glyph_offset: [entry.bearing_x, ch - entry.bearing_y],
                                fg_color: [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0],
                                bg_color: [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0],
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
                    [cell.bg.r(), cell.bg.g(), cell.bg.b(), 1.0]
                } else if is_sel {
                    [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0]
                } else {
                    [cell.fg.r(), cell.fg.g(), cell.fg.b(), 1.0]
                };

                if cell.underline {
                    let thickness = 1.0_f32.max((ch * 0.05).round());
                    let deco_y = (ch - thickness).max(0.0);
                    let dqy = px_to_clip_y((row as f32 * ch + deco_y).round());
                    let dqx = ((col as f32 * cw).round()) * x_scale - 1.0;
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
                    let deco_y = (ch * 0.45).round();
                    let dqy = px_to_clip_y((row as f32 * ch + deco_y).round());
                    let dqx = ((col as f32 * cw).round()) * x_scale - 1.0;
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

        // Concatenate: backgrounds → glyphs → cursor_bg → decorations.
        // This ensures correct z-order:
        //   1. Selection backgrounds (below all text)
        //   2. All glyphs (text from all rows)
        //   3. Cursor block background (above text, covering descenders
        //      from the row above)
        //   4. Underline / strikethrough / cursor bars (topmost)
        let bg_count = bg_instances.len();
        let glyph_count = glyph_instances.len();
        let cursor_bg_count = cursor_bg_instances.len();
        let deco_count = deco_instances.len();
        let mut instances = bg_instances;
        instances.extend(glyph_instances);
        instances.extend(cursor_bg_instances);
        instances.extend(deco_instances);
        let total_instances = instances.len();

        log::debug!(
            "update_cell_instances: {} total ({} bg + {} glyph + {} curs_bg + {} deco), \
             {} blank skipped, {} glyph failures",
            total_instances,
            bg_count,
            glyph_count,
            cursor_bg_count,
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
}

impl eframe::App for ZentermApp {
    /// Phase 1: non-UI work.
    ///
    /// This is called before [`Self::ui()`] each frame.
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // 1. Read pending PTY bytes and feed the terminal parser.
        self.pump_pty();

        // 2. Handle keyboard input.
        //
        //    Some key combinations (Ctrl+Shift+C, Ctrl+Shift+V) are
        //    terminal-emulator commands rather than shell input — check
        //    for those first by iterating events outside the ctx.input
        //    borrow so we can call ctx.copy_text() / clipboard_text().
        let (copy_requested, paste_requested) = ctx.input(|input| {
            let mut copy = false;
            let mut paste = false;
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
                    _ => {}
                }
            }
            (copy, paste)
        });

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
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let available = ui.available_size();
            let pixels_per_point = ui.ctx().pixels_per_point();
            let vp_width_px = available.x * pixels_per_point;
            let vp_height_px = available.y * pixels_per_point;

            // ── Resize terminal to match the available area ─────────────
            let cols = (vp_width_px / self.cell_width).max(10.0) as u16;
            let rows = (vp_height_px / self.cell_height).max(5.0) as u16;
            let new_size = TermSize::new(rows, cols);
            if new_size != self.terminal.size() {
                self.terminal.resize(new_size);
                self.pty.resize(new_size).ok();
                self.terminal_dirty = true;
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
            ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);

            // Register the callback shape — egui-wgpu will call
            // prepare() and paint() on it.
            ui.painter().add(callback);

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
