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

use zenterm_core::Rgba;
use zenterm_core::image::ImageCell;
use zenterm_core::image::ImageDataType;
use zenterm_glyph::GlyphContentType;
use zenterm_render::glyph_type;
use zenterm_render::{AtlasRange, CellInstance};

use super::shaping;
use super::types::{TerminalSession, UrlSpan};
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
        if self.pty_exited || !self.terminal_dirty {
            let has_instances = !self.cached_bg.is_empty()
                || self.cached_glyph_per_atlas.iter().any(|v| !v.is_empty())
                || !self.cached_deco.is_empty()
                || self.cached_image_below.iter().any(|v| !v.is_empty())
                || self.cached_image_above.iter().any(|v| !v.is_empty());
            if has_instances {
                let mut fd = self
                    .gpu
                    .shared
                    .frame_data
                    .lock()
                    .expect("frame_data poisoned");
                fd.instances.extend(&self.cached_bg);
                // Append per-atlas image instances (z < 0).
                for (slot_idx, instances) in self.cached_image_below.iter().enumerate() {
                    if instances.is_empty() {
                        continue;
                    }
                    let start = fd.instances.len() as u32;
                    fd.instances.extend(instances);
                    fd.atlas_ranges.push(AtlasRange {
                        atlas_index: slot_idx,
                        start,
                        count: instances.len() as u32,
                    });
                }
                // Append per-atlas glyph instances.
                for (slot_idx, instances) in self.cached_glyph_per_atlas.iter().enumerate() {
                    if instances.is_empty() {
                        continue;
                    }
                    let start = fd.instances.len() as u32;
                    fd.instances.extend(instances);
                    fd.atlas_ranges.push(AtlasRange {
                        atlas_index: slot_idx,
                        start,
                        count: instances.len() as u32,
                    });
                }
                fd.instances.extend(&self.cached_deco);
                // Append per-atlas image instances (z >= 0).
                for (slot_idx, instances) in self.cached_image_above.iter().enumerate() {
                    if instances.is_empty() {
                        continue;
                    }
                    let start = fd.instances.len() as u32;
                    fd.instances.extend(instances);
                    fd.atlas_ranges.push(AtlasRange {
                        atlas_index: slot_idx,
                        start,
                        count: instances.len() as u32,
                    });
                }
            }
            return has_instances;
        }

        let mut atlas = self.atlas.lock();
        let cw = self.cell_width;
        let ch = self.cell_height;

        // Read cursor info BEFORE visible_cells() since both borrow
        // self.terminal (one mut, one immut).
        // Drain pending image atlas deallocations (images removed by kitty
        // delete commands).
        for hash in self.terminal.pending_image_deallocations.drain(..) {
            self.atlas.remove_image(&hash);
        }

        let cursor = self.terminal.cursor();
        let cursor_row = cursor.pos.line;
        let cursor_orig_col = cursor.pos.column;
        let cursor_bg = cursor.cursor_bg;
        let cursor_fg = cursor.cursor_fg;
        // cursor_col is set below once cols is available.

        let blink_on = if cursor.style.blinking
            && !matches!(cursor.style.shape, CursorShape::Block)
        {
            // Time-based blink phase: toggle every blink_interval ms.
            // Uses `blink_epoch` as a fixed reference point so the phase
            // is consistent regardless of frame rate.  This replaces the
            // old `frame_count / blink_interval` approach which required
            // incrementing a counter every frame.
            let elapsed = self.blink_epoch.elapsed().as_millis();
            let period = (self.blink_interval as u128).max(100) * 2;
            (elapsed % period) < period / 2
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

        // When IME preedit is active, advance the visual cursor to the
        // end of the composing text so the cursor follows the input.
        let preedit_advance = self
            .preedit_text
            .as_ref()
            .map(|t| t.chars().count())
            .unwrap_or(0);
        let cursor_col = (cursor_orig_col + preedit_advance).min(cols.saturating_sub(1));

        let baseline = atlas.cell_baseline_offset();

        // Reuse cached instance buffers — clear instead of re-allocating.
        let instances_cap = rows * cols;
        self.cached_bg.clear();
        for v in &mut self.cached_glyph_per_atlas {
            v.clear();
        }
        for v in &mut self.cached_image_below {
            v.clear();
        }
        for v in &mut self.cached_image_above {
            v.clear();
        }
        self.cached_deco.clear();
        if self.cached_bg.capacity() < instances_cap {
            self.cached_bg.reserve(instances_cap - self.cached_bg.capacity());
        }
        if self.cached_deco.capacity() < instances_cap {
            self.cached_deco.reserve(instances_cap - self.cached_deco.capacity());
        }
        if self.cached_image_below.capacity() < instances_cap {
            self.cached_image_below.reserve(
                instances_cap - self.cached_image_below.capacity(),
            );
        }
        if self.cached_image_above.capacity() < instances_cap {
            self.cached_image_above.reserve(
                instances_cap - self.cached_image_above.capacity(),
            );
        }
        if self.cached_deco.capacity() < instances_cap {
            self.cached_deco.reserve(instances_cap - self.cached_deco.capacity());
        }
        // Shrink cached buffers when the grid shrinks significantly
        // (capacity > 2x needed) to avoid retaining large allocations
        // after window resize.
        let shrink_threshold = instances_cap.saturating_mul(2);
        if self.cached_bg.capacity() > shrink_threshold {
            self.cached_bg.shrink_to(instances_cap);
        }
        if self.cached_deco.capacity() > shrink_threshold {
            self.cached_deco.shrink_to(instances_cap);
        }
        if self.cached_image_below.capacity() > shrink_threshold {
            self.cached_image_below.shrink_to(instances_cap);
        }
        if self.cached_image_above.capacity() > shrink_threshold {
            self.cached_image_above.shrink_to(instances_cap);
        }
        let mut has_new_glyphs = false;
        let mut img_below_count: usize = 0;
        let mut img_above_count: usize = 0;

        // ── URL span detection (for hover underline) ──────────────────
        self.url_spans.clear();
        let hovered_url: Option<(usize, usize)> = if self.url_hover_underline {
            // Re-scan visible rows with linkify.
            let finder = linkify::LinkFinder::new();
            for r in 0..rows {
                let mut line = String::with_capacity(cols);
                for c in 0..cols {
                    if let Some(cell) = grid.cell(r, c) {
                        line.push(cell.c);
                    }
                }
                for link in finder.links(&line) {
                    let start_col = line[..link.start()].chars().count();
                    let end_col = line[..link.end()].chars().count();
                    log::debug!(
                        "url scan: row={} col={}-{} url={} line={:?}",
                        r, start_col, end_col, link.as_str(), &line,
                    );
                    self.url_spans.push(UrlSpan {
                        row: r,
                        col_start: start_col,
                        col_end: end_col,
                        url: link.as_str().to_string(),
                    });
                }
            }
            log::debug!("url scan: {} spans, hover_cell={:?}", self.url_spans.len(), self.hover_cell);
            // Find which URL (if any) the cursor is hovering over.
            self.hover_cell.and_then(|(hr, hc)| {
                let matched = self.url_spans
                    .iter()
                    .find(|span| span.row == hr && hc >= span.col_start && hc < span.col_end);
                log::debug!("hover match: row={} col={} matched={:?}", hr, hc, matched.as_ref().map(|s| (s.row, s.col_start, s.col_end)));
                matched.map(|span| (span.row, span.col_end))
            })
        } else {
            None
        };

        // ── Cursor line highlight (OSC 1337 HighlightCursorLine) ─────
        // Emit a full-width background quad at the cursor row.
        if self.highlight_cursor_line && cursor_visible {
            // Pick a subtle highlight colour based on background luminance.
            let bg = default_bg;
            let luminance = 0.299 * bg.r() + 0.587 * bg.g() + 0.114 * bg.b();
            let highlight = if luminance > 0.5 {
                // Light background → dark highlight with low alpha.
                Rgba::new(0.0, 0.0, 0.0, 0.08)
            } else {
                // Dark background → light highlight with low alpha.
                Rgba::new(1.0, 1.0, 1.0, 0.08)
            };
            // Only emit when the colour is different from default_bg
            // (always true here due to alpha, but force=false so the
            // emit_background_quad function can skip if they match).
            emit_background_quad(
                &mut self.cached_bg,
                0,             // col
                cursor_row,    // row
                self.cell_width,
                self.cell_height,
                cols as f32,   // num_cells — highlight spans full row width
                highlight,
                default_bg,
                false,
                x_off, y_off, x_scale, y_scale,
                self.window_opacity,
            );        }

        for row in 0..rows {
            let mut col = 0;
            let mut last_checked_run_end: usize = 0;
            while col < cols {
                let cell = match grid.cell(row, col) {
                    Some(c) => c,
                    None => { col += 1; continue; },
                };

                let mut ch_char = cell.c;

                let is_preedit = self.preedit_text.as_ref().is_some_and(|preedit| {
                    row == cursor_row
                        && col >= cursor_orig_col
                        && col - cursor_orig_col < preedit.chars().count()
                });
                if is_preedit {
                    if let Some(ref preedit) = self.preedit_text {
                        if let Some(c) = preedit.chars().nth(col - cursor_orig_col) {
                            ch_char = c;
                        }
                    }
                }

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
                    // Use the theme/OSC-specified cursor colours.  When
                    // cursor_fg is None, fall back to the cell's own
                    // foreground (classic inverse-video behaviour).
                    let cf = cursor_fg.unwrap_or(cell.fg);
                    (cf, cursor_bg)
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
                    || cell.strikethrough
                    || is_preedit;

                let run_start = col;
                let run_end = shaping::detect_run_end(&grid, row, col, cols);

                // ── URL hover underline ──────────────────────────────────
                // Must be BEFORE the ligature branch, which can skip over
                // multiple cells via `col = run_end; continue`.
                log::debug!("url_check: hovered_url={:?} row={} col={} terminal_dirty={}",
                    hovered_url, row, col, self.terminal_dirty);
                if let Some((url_row, url_col_end)) = hovered_url {
                    if row == url_row && col < url_col_end {
                        log::debug!("url_underline: emit row={} col={} end={}", row, col, url_col_end);
                        let thickness = 1.0_f32.max((ch * 0.06).round());
                        let deco_y = y_off + row as f32 * ch + baseline + 0.5;
                        let deco_x = x_off + col as f32 * cw;
                        self.cached_deco.push(CellInstance {
                            clip_pos: [
                                deco_x * x_scale - 1.0,
                                1.0 - deco_y * y_scale,
                            ],
                            uv_min: [0.0; 2],
                            uv_max: [0.0; 2],
                            clip_cell_size: [cw * x_scale, thickness * y_scale],
                            glyph_size: [0.0; 2],
                            glyph_offset: [0.0; 2],
                            fg_color: [draw_fg.r(), draw_fg.g(), draw_fg.b(), 1.0],
                            bg_color: [draw_fg.r(), draw_fg.g(), draw_fg.b(), 1.0],
                            flags: glyph_type::SOLID,
                        });
                    }
                }

                let ligatures_enabled = atlas.ligatures_enabled;
                let ligature_eligible = ligatures_enabled
                    && run_end > run_start + 1
                    && !is_blank
                    && run_end != last_checked_run_end;
                if ligature_eligible {
                    let outcome = process_ligature_run(
                        &mut atlas, &grid, row, run_start, run_end,
                        cursor_visible, cursor_row, cursor_col,
                        cursor_shape, cursor_bg, display_offset,
                        sel_range.as_ref(), sel_bg, sel_fg,
                        default_bg, baseline, cw, ch,
                        x_off, y_off, x_scale, y_scale, cols,
                        &mut self.cached_bg,
                        &mut self.cached_glyph_per_atlas,
                        &mut self.cached_deco,
                        self.window_opacity,
                    );
                    last_checked_run_end = outcome.last_checked;
                    if outcome.has_new_glyphs {
                        has_new_glyphs = true;
                    }
                    if outcome.handled {
                        // ── URL underline for ligature-skipped cells ─────
                        // The ligature branch jumps to run_end, skipping
                        // all cells in (run_start .. run_end).  Any URL
                        // underline for those cells must be emitted here.
                        if let Some((url_row, url_col_end)) = hovered_url {
                            if row == url_row {
                                let emit_start = run_start.max(0);
                                let emit_end = outcome.run_end.min(url_col_end);
                                for c in emit_start..emit_end {
                                    log::debug!("url_underline: ligature-bypass row={} col={} end={}", row, c, url_col_end);
                                    let thickness = 1.0_f32.max((ch * 0.06).round());
                                    let deco_y = y_off + row as f32 * ch + baseline + 0.5;
                                    let deco_x = x_off + c as f32 * cw;
                                    self.cached_deco.push(CellInstance {
                                        clip_pos: [
                                            deco_x * x_scale - 1.0,
                                            1.0 - deco_y * y_scale,
                                        ],
                                        uv_min: [0.0; 2],
                                        uv_max: [0.0; 2],
                                        clip_cell_size: [cw * x_scale, thickness * y_scale],
                                        glyph_size: [0.0; 2],
                                        glyph_offset: [0.0; 2],
                                        fg_color: [draw_fg.r(), draw_fg.g(), draw_fg.b(), 1.0],
                                        bg_color: [draw_fg.r(), draw_fg.g(), draw_fg.b(), 1.0],
                                        flags: glyph_type::SOLID,
                                    });
                                }
                            }
                        }
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

                if !is_cursor || is_block_cursor {
                    let cell_bg = if is_sel { sel_bg } else { draw_bg };
                    emit_background_quad(
                        &mut self.cached_bg,
                        col, row, cw, ch, num_cells,
                        cell_bg,
                        default_bg,
                        is_block_cursor,
                        x_off, y_off, x_scale, y_scale,
                        self.window_opacity,
                    );
                }

                // ── Image quads (z < 0: behind text) ────────────────────
                if let Some(ref img) = cell.image {
                    if img.z_index < 0 {
                        emit_image_quad(
                            &mut self.cached_image_below, &mut atlas, img, col, row,
                            cw, ch, x_off, y_off, x_scale, y_scale,
                        );
                        img_below_count += 1;
                    }
                }

                if is_hidden {
                    col += 1;
                    continue;
                }

                if !is_blank {
                    log::debug!(
                        "per-char glyph: row={row} col={col} ch={ch_char:?} \
                         run_start={run_start} run_end={run_end}",
                    );
                    // Extract glyph entry data in a sub-scope so the
                    // mutable borrow on `atlas` is released before we
                    // access `atlas.slots` below.
                    let (ai, ar, scale, sbx, sby, ct) = {
                        if let Ok((entry, is_new)) = atlas.ensure_glyph(ch_char) {
                            if is_new {
                                has_new_glyphs = true;
                            }
                            (
                                entry.atlas_index,
                                entry.atlas_rect,
                                entry.scale,
                                entry.bearing_x * entry.scale,
                                entry.bearing_y * entry.scale,
                                entry.content_type,
                            )
                        } else {
                            log::warn!(
                                "glyph lookup failed for ch={ch_char:?}",
                            );
                            col += 1;
                            continue;
                        }
                    };

                        let atlas_w = (ar.max.x - ar.min.x) as f32;
                        let atlas_h = (ar.max.y - ar.min.y) as f32;

                        let mut scaled_w = atlas_w * scale;
                        let mut scaled_h = atlas_h * scale;

                        let mut glyph_x_px =
                            x_off + (col as f32 * cw + sbx).round();
                        let mut glyph_y_px =
                            y_off + (row as f32 * ch + (baseline - sby)).round();

                        let slot_size = atlas.slots[ai as usize].size as f32;
                        let mut u_min =
                            (ar.min.x as f32 + 0.5) / slot_size;
                        let mut v_min =
                            (ar.min.y as f32 + 0.5) / slot_size;
                        let mut u_max =
                            (ar.max.x as f32 - 0.5) / slot_size;
                        let mut v_max =
                            (ar.max.y as f32 - 0.5) / slot_size;

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
                            v_min = v_min + r_top * v_range;
                            v_max = v_min + (r_bot - r_top) * v_range;
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
                            u_min = u_min + r_left * u_range;
                            u_max = u_min + (r_right - r_left) * u_range;
                            glyph_x_px = clipped_left;
                            scaled_w = clipped_w;
                        }

                        let (glyph_fg, glyph_bg) = if is_cursor && !is_block_cursor {
                            // Underline / Beam cursor: the glyph itself
                            // uses the cursor fill colour as a visual
                            // indicator.
                            (cursor_bg, cell.fg)
                        } else if is_sel {
                            (sel_fg.unwrap_or(draw_fg), sel_bg)
                        } else {
                            (draw_fg, draw_bg)
                        };

                        // Ensure per-atlas cache vec is large enough.
                        let ai_usize = ai as usize;
                        if ai_usize >= self.cached_glyph_per_atlas.len() {
                            self.cached_glyph_per_atlas.resize_with(ai_usize + 1, Vec::new);
                        }
                        self.cached_glyph_per_atlas[ai_usize].push(CellInstance {
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
                            flags: match ct {
                                GlyphContentType::Subpixel => glyph_type::SUBPIXEL,
                                GlyphContentType::Mask => glyph_type::MASK,
                                GlyphContentType::Color => glyph_type::COLOR,
                            },
                        });
                    }

                if has_deco || is_cursor {
                    emit_deco_for_cell(
                        &mut self.cached_deco,
                        &grid, row, col, cols,
                        cursor_visible, cursor_row, cursor_col,
                        cursor_shape, cursor_bg, display_offset,
                        sel_range.as_ref(), sel_bg, sel_fg,
                        default_bg, baseline, ch, cw,
                        x_off, y_off,
                        x_scale, y_scale,
                    );
                }

                if is_preedit {
                    let thickness = 1.0_f32.max((ch * 0.05).round());
                    let deco_y_px = y_off + row as f32 * ch + baseline + 1.0;
                    let deco_x_px = x_off + col as f32 * cw;
                    self.cached_deco.push(CellInstance {
                        clip_pos: [
                            deco_x_px * x_scale - 1.0,
                            1.0 - deco_y_px * y_scale,
                        ],
                        uv_min: [0.0; 2],
                        uv_max: [0.0; 2],
                        clip_cell_size: [cw * x_scale, thickness * y_scale],
                        glyph_size: [0.0; 2],
                        glyph_offset: [0.0; 2],
                        fg_color: [draw_fg.r(), draw_fg.g(), draw_fg.b(), 1.0],
                        bg_color: [draw_fg.r(), draw_fg.g(), draw_fg.b(), 1.0],
                        flags: glyph_type::SOLID,
                    });
                }

                // ── Image quads (z >= 0: on top of text) ────────────────
                if let Some(ref img) = cell.image {
                    if img.z_index >= 0 {
                        emit_image_quad(
                            &mut self.cached_image_above, &mut atlas, img, col, row,
                            cw, ch, x_off, y_off, x_scale, y_scale,
                        );
                        img_above_count += 1;
                    }
                }

                col += 1;
            }
        }

        if img_below_count > 0 || img_above_count > 0 {
            log::trace!(
                "[img] render frame: below={}, above={}, total_placements={}, dirty={}",
                img_below_count, img_above_count,
                self.terminal.image_placements_count(),
                self.terminal_dirty,
            );
        }

        // Append to the shared instance buffer in draw order.
        let mut fd = self
            .gpu
            .shared
            .frame_data
            .lock()
            .expect("frame_data poisoned");
        fd.instances.extend(&self.cached_bg);
        // Append per-atlas image instances (z < 0).
        for (slot_idx, instances) in self.cached_image_below.iter().enumerate() {
            if instances.is_empty() {
                continue;
            }
            let start = fd.instances.len() as u32;
            fd.instances.extend(instances);
            fd.atlas_ranges.push(AtlasRange {
                atlas_index: slot_idx,
                start,
                count: instances.len() as u32,
            });
        }
        // Append per-atlas glyph instances.
        for (slot_idx, instances) in self.cached_glyph_per_atlas.iter().enumerate() {
            if instances.is_empty() {
                continue;
            }
            let start = fd.instances.len() as u32;
            fd.instances.extend(instances);
            fd.atlas_ranges.push(AtlasRange {
                atlas_index: slot_idx,
                start,
                count: instances.len() as u32,
            });
        }
        fd.instances.extend(&self.cached_deco);
        // Append per-atlas image instances (z >= 0).
        for (slot_idx, instances) in self.cached_image_above.iter().enumerate() {
            if instances.is_empty() {
                continue;
            }
            let start = fd.instances.len() as u32;
            fd.instances.extend(instances);
            fd.atlas_ranges.push(AtlasRange {
                atlas_index: slot_idx,
                start,
                count: instances.len() as u32,
            });
        }
        drop(fd);
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

/// Emit a [`CellInstance`] for an [`ImageCell`] attached to a grid cell.
fn emit_image_quad(
    bufs: &mut Vec<Vec<CellInstance>>,
    atlas: &mut zenterm_glyph::GlyphAtlas,
    img: &ImageCell,
    col: usize,
    row: usize,
    cw: f32,
    ch: f32,
    x_off: f32,
    y_off: f32,
    x_scale: f32,
    y_scale: f32,
) {
    let (pixels, img_w, img_h, img_hash) = {
        let guard = img.data.data();
        match &*guard {
            ImageDataType::Rgba8 { data, width, height, hash } => {
                (data.clone(), *width, *height, *hash)
            }
            ImageDataType::AnimRgba8 { width, height, frames, hashes, .. } => {
                // Use the first frame for rendering (frame 0).
                // FUTURE: cycle through frames based on timing.
                (frames[0].clone(), *width, *height, hashes[0])
            }
        }
    };

    let entry = match atlas.ensure_image(&pixels, img_w, img_h, img_hash) {
        Ok(e) => e,
        Err(_) => return,
    };

    let slot_size = atlas.slots[entry.atlas_index as usize].size as f32;
    let ax = entry.atlas_rect.min.x as f32;
    let ay = entry.atlas_rect.min.y as f32;

    // Map ImageCell UV (image-space) → atlas UV.
    let u_min = (ax + img.top_left.x * img_w as f32) / slot_size;
    let v_min = (ay + img.top_left.y * img_h as f32) / slot_size;
    let u_max = (ax + img.bottom_right.x * img_w as f32) / slot_size;
    let v_max = (ay + img.bottom_right.y * img_h as f32) / slot_size;

    // Cell position in pixels (dock-relative).
    let clip_x = x_off + col as f32 * cw;
    let clip_y = y_off + row as f32 * ch;

    let instance = CellInstance {
        clip_pos: [clip_x * x_scale - 1.0, 1.0 - clip_y * y_scale],
        uv_min: [u_min, v_min],
        uv_max: [u_max, v_max],
        clip_cell_size: [cw * x_scale, ch * y_scale],
        glyph_size: [cw, ch],
        glyph_offset: [0.0, 0.0],
        fg_color: [1.0, 1.0, 1.0, 1.0],
        bg_color: [0.0, 0.0, 0.0, img.z_index as f32],
        flags: glyph_type::IMAGE,
    };

    let ai = entry.atlas_index as usize;
    if ai >= bufs.len() {
        bufs.resize_with(ai + 1, Vec::new);
    }
    bufs[ai].push(instance);
}
