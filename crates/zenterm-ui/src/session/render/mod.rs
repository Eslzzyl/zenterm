//! Per-session rendering: cell-instance generation.
//!
//! The heavy lifter here is [`update_cell_instances`] which iterates
//! the visible terminal grid and produces [`CellInstance`] buffers that
//! the wgpu callback consumes.  The actual quad emission is delegated
//! to [`pass1`] (background) and [`pass3`] (decorations) so that both
//! the ligature and non-ligature paths share the same rendering logic.

mod ligature;
mod pass1;
mod pass3;

use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::vte::ansi::CursorShape;

use zenterm_glyph::GlyphContentType;
use zenterm_render::glyph_type;
use zenterm_render::CellInstance;

use super::shaping;
use super::types::TerminalSession;
use ligature::process_ligature_run;
use self::pass1::emit_background_quad;
use self::pass3::emit_deco_for_cell;

impl TerminalSession {
    /// Rebuild the cell-instance buffers for this session's visible
    /// terminal grid.
    ///
    /// `origin_px` and `size_px` describe this session's viewport within
    /// the dock coordinate system.  Cell positions are converted to
    /// clip-space relative to the **dock** viewport so that a single
    /// wgpu callback can render all tabs.
    ///
    /// Returns `true` if any instances were produced (caller should
    /// bump the instance generation counter).
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

                // ── Per-cell state ────────────────────────────────────
                let ch_char = cell.c;
                let is_blank = ch_char == ' ';
                let is_cursor = cursor_visible && row == cursor_row && col == cursor_col;
                let is_block_cursor =
                    is_cursor && matches!(cursor_shape, CursorShape::Block);
                let is_sel = sel_range.as_ref().is_some_and(|range| {
                    let grid_line = (row as i32) - (display_offset as i32);
                    let pt = alacritty_terminal::index::Point::new(
                        alacritty_terminal::index::Line(grid_line),
                        alacritty_terminal::index::Column(col),
                    );
                    range.contains(pt)
                });

                let (draw_fg, draw_bg) = if is_block_cursor {
                    (cell.bg, cell.fg)
                } else {
                    (cell.fg, cell.bg)
                };
                let draw_fg = if cell.dim {
                    zenterm_core::color::Rgba::new(
                        draw_fg.r() * 0.5,
                        draw_fg.g() * 0.5,
                        draw_fg.b() * 0.5,
                        draw_fg.a(),
                    )
                } else {
                    draw_fg
                };

                let is_hidden = cell.hidden;
                let has_deco = !matches!(cell.underline_style, zenterm_core::cell::UnderlineStyle::None)
                    || cell.strikethrough;

                // ── Run boundary detection ────────────────────────────
                let run_start = col;
                let run_end = shaping::detect_run_end(&grid, row, col, cols);

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
                    let outcome = process_ligature_run(
                        &mut atlas, &grid, row, run_start, run_end,
                        cursor_visible, cursor_row, cursor_col,
                        cursor_shape, display_offset,
                        sel_range.as_ref(), sel_bg, sel_fg,
                        default_bg, baseline, cw, ch,
                        x_off, y_off, x_scale, y_scale, cols,
                        &mut bg_instances,
                        &mut glyph_instances,
                        &mut deco_instances,
                    );
                    last_checked_run_end = outcome.last_checked;
                    if outcome.has_new_glyphs {
                        has_new_glyphs = true;
                    }
                    if outcome.handled {
                        col = outcome.run_end;
                        continue;
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
                    emit_background_quad(
                        &mut bg_instances,
                        col, row, cw, ch, num_cells,
                        cell_bg,
                        default_bg,
                        is_block_cursor,
                        x_off, y_off, x_scale, y_scale,
                    );
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

                        // ── Vertical clip (GLYPH_CLIP.md) ──
                        let glyph_bot_px = glyph_y_px + scaled_h;
                        let clipped_top = glyph_y_px.max(cell_top);
                        let clipped_bot = glyph_bot_px.min(cell_bottom);
                        let clipped_h = (clipped_bot - clipped_top).max(0.0);
                        if clipped_h < scaled_h && scaled_h > 0.0 {
                            let r_top = (clipped_top - glyph_y_px) / scaled_h;
                            let r_bot = (clipped_bot - glyph_y_px) / scaled_h;
                            let v_range = v_max - v_min;
                            v_min = v_min + r_top * v_range;
                            v_max = v_min + (r_bot - r_top) * v_range;
                            glyph_y_px = clipped_top;
                            scaled_h = clipped_h;
                        }

                        // ── Horizontal clip (GLYPH_CLIP.md) ──
                        let glyph_right_px = glyph_x_px + scaled_w;
                        let cell_right_clip = if num_cells > 1.0 {
                            // CJK / emoji: glyph should be centered in the
                            // first cell and not overflow into adjacent cells.
                            cell_left + cw
                        } else {
                            cell_right
                        };
                        let clipped_left = glyph_x_px.max(cell_left);
                        let clipped_right = glyph_right_px.min(cell_right_clip);
                        let clipped_w = (clipped_right - clipped_left).max(0.0);
                        if clipped_w < scaled_w && scaled_w > 0.0 {
                            let r_left = (clipped_left - glyph_x_px) / scaled_w;
                            let r_right = (clipped_right - glyph_x_px) / scaled_w;
                            let u_range = u_max - u_min;
                            u_min = u_min + r_left * u_range;
                            u_max = u_min + (r_right - r_left) * u_range;
                            glyph_x_px = clipped_left;
                            scaled_w = clipped_w;
                        }

                        let (glyph_fg, glyph_bg) = if is_cursor && !is_block_cursor {
                            (cell.bg, cell.fg)
                        } else if is_sel {
                            (sel_fg.unwrap_or(draw_fg), sel_bg)
                        } else {
                            (draw_fg, draw_bg)
                        };

                        glyph_instances.push(CellInstance {
                            clip_pos: [
                                glyph_x_px * x_scale - 1.0,
                                1.0 - glyph_y_px * y_scale,
                            ],
                            uv_min: [u_min, v_min],
                            uv_max: [u_max, v_max],
                            clip_cell_size: [scaled_w * x_scale, scaled_h * y_scale],
                            glyph_size: [scaled_w, scaled_h],
                            glyph_offset: [0.0, 0.0],
                            fg_color: [
                                glyph_fg.r(), glyph_fg.g(),
                                glyph_fg.b(), 1.0,
                            ],
                            bg_color: [
                                glyph_bg.r(), glyph_bg.g(),
                                glyph_bg.b(), 1.0,
                            ],
                            flags: match entry.content_type {
                                GlyphContentType::Subpixel => glyph_type::SUBPIXEL,
                                GlyphContentType::Mask => glyph_type::MASK,
                                GlyphContentType::Color => glyph_type::COLOR,
                            },
                        });
                    } else {
                        log::warn!(
                            "update_cell_instances: glyph lookup failed for ch={:?}",
                            ch_char,
                        );
                    }
                }

                // ── Pass 3+4: decorations (underline, strikethrough, cursor style) ──
                if has_deco || is_cursor {
                    emit_deco_for_cell(
                        &mut deco_instances,
                        &grid, row, col, cols,
                        cursor_visible, cursor_row, cursor_col,
                        cursor_shape, display_offset,
                        sel_range.as_ref(), sel_bg, sel_fg,
                        default_bg, baseline, ch, cw,
                        x_off, y_off,
                        x_scale, y_scale,
                    );
                }

                col += 1;
            }
        }

        // Swap cached instance buffers so the fast path can reuse them.
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
