//! Ligature run processing: shape an entire run of consecutive same-style
//! characters as a single string and distribute the resulting glyphs across
//! their covering cells.
//!
//! This is a direct port of the inline ligature branch that originally lived
//! in `update_cell_instances`.  The UV computation uses `origin_to_bitmap`
//! (derived from the glyph's bearing) rather than a simple ratio of the
//! advance, and applies vertical + horizontal clipping identical to the
//! per-char render path.

use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::vte::ansi::CursorShape;

use zenterm_core::color::Rgba;
use zenterm_glyph::GlyphContentType;
use zenterm_render::glyph_type;
use zenterm_render::CellInstance;
use zenterm_term::GridView;

use super::shaping;
use super::pass1::emit_background_quad;
use super::pass3::emit_deco_for_cell;
use crate::glyph_cache::GlyphAtlasGuard;

/// Outcome of attempting to process a ligature run.
pub(crate) struct LigatureOutcome {
    /// `true` if the ligature was applied.  Caller should `continue`
    /// the outer loop with `col = run_end`.
    pub handled: bool,
    /// The run end column (valid when `handled` is true).
    pub run_end: usize,
    /// Updated `last_checked_run_end` for the outer loop.
    pub last_checked: usize,
    /// Whether new glyphs were rasterised (triggers GPU sync).
    pub has_new_glyphs: bool,
}

/// Attempt to shape and render a ligature run.
///
/// Returns [`LigatureOutcome`] indicating whether the run was handled
/// and any state updates for the outer loop.
#[allow(clippy::too_many_arguments)]
pub(crate) fn process_ligature_run(
    atlas: &mut GlyphAtlasGuard<'_>,
    grid: &GridView<'_>,
    row: usize,
    run_start: usize,
    run_end: usize,
    cursor_visible: bool,
    cursor_row: usize,
    cursor_col: usize,
    cursor_shape: CursorShape,
    display_offset: usize,
    sel_range: Option<&SelectionRange>,
    sel_bg: Rgba,
    sel_fg: Option<Rgba>,
    default_bg: Rgba,
    baseline: f32,
    cw: f32,
    ch: f32,
    x_off: f32,
    y_off: f32,
    x_scale: f32,
    y_scale: f32,
    cols: usize,
    bg_instances: &mut Vec<CellInstance>,
    // Per-atlas-slot glyph instance caches, indexed by atlas_index.
    // The caller ensures the outer vec is large enough.
    glyph_instances: &mut Vec<Vec<CellInstance>>,
    deco_instances: &mut Vec<CellInstance>,
) -> LigatureOutcome {
    let run_text = shaping::extract_run_text(grid, row, run_start, run_end);
    if !shaping::might_ligate(&run_text) {
        return LigatureOutcome {
            handled: false,
            run_end,
            last_checked: 0,
            has_new_glyphs: false,
        };
    }

    log::debug!(
        "ligature ENTER: row={row} run={run_start}..{run_end} \
         text={run_text:?}",
    );

    match atlas.shape_and_rasterize_run(&run_text) {
        Ok((shaped, atlas_modified, had_effect)) => {
            let mut has_new_glyphs = false;
            if atlas_modified {
                has_new_glyphs = true;
            }

            // ── Fast-path: skip if no effect ──
            let cursor_in_run = cursor_visible
                && row == cursor_row
                && cursor_col >= run_start
                && cursor_col < run_end;

            if cursor_in_run || !had_effect {
                return LigatureOutcome {
                    handled: false,
                    run_end,
                    last_checked: run_end,
                    has_new_glyphs,
                };
            }

            // ── Use the shaped run result ──
            let mut strip_col = run_start;

            for sg in &shaped {
                let cell_base = run_start + sg.char_range.start;

                // Advance past any gap between shaped glyphs.
                if cell_base > strip_col {
                    for ccol in strip_col..cell_base {
                        emit_deco_for_cell(
                            deco_instances,
                            grid, row, ccol, cols,
                            cursor_visible, cursor_row, cursor_col,
                            cursor_shape, display_offset,
                            sel_range, sel_bg, sel_fg,
                            default_bg, baseline, ch, cw,
                            x_off, y_off,
                            x_scale, y_scale,
                        );
                    }
                    strip_col = cell_base;
                }

                let actual_num_cells = shaping::glyph_grid_num_cells(
                    grid, row, run_start, &sg.char_range, cols,
                );
                for cell_offset in 0..actual_num_cells {
                    let cell_col = cell_base + cell_offset;
                    let c = grid.cell(row, cell_col)
                        .unwrap_or_else(|| grid.cell(row, run_start).unwrap());

                    // ── Per-cell cursor / selection state ──
                    let c_is_cursor = cursor_visible
                        && row == cursor_row
                        && cell_col == cursor_col;
                    let c_is_block = c_is_cursor
                        && matches!(cursor_shape, CursorShape::Block);
                    let c_is_sel = sel_range.is_some_and(|range| {
                        let grid_line = (row as i32) - (display_offset as i32);
                        let pt = alacritty_terminal::index::Point::new(
                            alacritty_terminal::index::Line(grid_line),
                            alacritty_terminal::index::Column(cell_col),
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
                    emit_background_quad(
                        bg_instances,
                        cell_col, row, cw, ch, 1.0,
                        c_bg_color,
                        default_bg,
                        c_is_block,
                        x_off, y_off, x_scale, y_scale,
                    );

                    // ── Pass 2: glyph strip ──
                    let atlas_rect = &sg.entry.atlas_rect;
                    let a_left = atlas_rect.min.x as f32;
                    let a_right = atlas_rect.max.x as f32;
                    let a_top = atlas_rect.min.y as f32;
                    let a_bot = atlas_rect.max.y as f32;
                    let slot_size = atlas.slots[sg.entry.atlas_index as usize].size as f32;

                    let (mut u_min, mut u_max, strip_w);
                    if actual_num_cells > 1 {
                        let strip_left = cell_offset as f32 * cw;
                        let mut strip_right = (cell_offset + 1) as f32 * cw;
                        strip_right = strip_right.min(sg.entry.advance);
                        let sw = strip_right - strip_left;
                        // UV: a horizontal slice of the atlas.
                        // origin_to_bitmap shifts the UV origin to the bitmap
                        let origin_to_bitmap = sg.entry.bearing_x;
                        u_min = (a_left + 0.5 + strip_left + origin_to_bitmap) / slot_size;
                        u_max = (a_left + 0.5 + strip_right + origin_to_bitmap) / slot_size;
                        strip_w = sw;
                    } else {
                        u_min = (a_left + 0.5) / slot_size;
                        u_max = (a_right - 0.5) / slot_size;
                        strip_w = sg.entry.advance;
                    }

                    let mut v_min = (a_top + 0.5) / slot_size;
                    let mut v_max = (a_bot - 0.5) / slot_size;
                    let glyph_h = (a_bot - a_top) as f32;
                    let sbx = sg.entry.bearing_x;
                    let sby = sg.entry.bearing_y;

                    // glyph_offset: the bearing is always relative to this
                    // cell's origin.  The UV coordinates above select the
                    // correct horizontal slice of the glyph texture.
                    let gox = sbx;
                    let goy = baseline - sby;

                    let mut glyph_x_px = x_off + (cell_col as f32 * cw + gox).round();
                    let mut glyph_y_px = y_off + (row as f32 * ch + goy).round();

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

                    // ── Horizontal clip (GLYPH_CLIP.md) ──
                    let glyph_right = glyph_x_px + scaled_w;
                    let clipped_left = if gox >= 0.0 {
                        glyph_x_px.max(cell_left)
                    } else {
                        glyph_x_px
                    };
                    let clipped_right = glyph_right.min(cell_right);
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

                    let gqx = glyph_x_px * x_scale - 1.0;
                    let gqy = 1.0 - glyph_y_px * y_scale;
                    let gqw = scaled_w * x_scale;
                    let gqh = scaled_h * y_scale;

                    let gtype = match sg.entry.content_type {
                        GlyphContentType::Subpixel => glyph_type::SUBPIXEL,
                        GlyphContentType::Mask => glyph_type::MASK,
                        GlyphContentType::Color => glyph_type::COLOR,
                    };

                    let (glyph_fg, glyph_bg) = if c_is_cursor && !c_is_block {
                        (c.bg, c.fg)
                    } else if c_is_sel {
                        (sel_fg.unwrap_or(c.fg), sel_bg)
                    } else {
                        (c_draw_fg, c_bg_color)
                    };

                    let ai = sg.entry.atlas_index as usize;
                    if ai >= glyph_instances.len() {
                        glyph_instances.resize_with(ai + 1, Vec::new);
                    }
                    glyph_instances[ai].push(CellInstance {
                        clip_pos: [gqx, gqy],
                        uv_min: [u_min, v_min],
                        uv_max: [u_max, v_max],
                        clip_cell_size: [gqw, gqh],
                        glyph_size: [scaled_w, scaled_h],
                        glyph_offset: [gox, goy],
                        fg_color: [
                            glyph_fg.r(), glyph_fg.g(),
                            glyph_fg.b(), 1.0,
                        ],
                        bg_color: [
                            glyph_bg.r(), glyph_bg.g(),
                            glyph_bg.b(), 1.0,
                        ],
                        flags: gtype,
                    });

                    // ── Pass 3+4: decorations ──
                    emit_deco_for_cell(
                        deco_instances,
                        grid, row, cell_col, cols,
                        cursor_visible, cursor_row, cursor_col,
                        cursor_shape, display_offset,
                        sel_range, sel_bg, sel_fg,
                        default_bg, baseline, ch, cw,
                        x_off, y_off,
                        x_scale, y_scale,
                    );

                    strip_col = cell_col + 1;
                }
            }

            // Any remaining cells in the run beyond the last shaped glyph.
            for ccol in strip_col..run_end {
                emit_deco_for_cell(
                    deco_instances,
                    grid, row, ccol, cols,
                    cursor_visible, cursor_row, cursor_col,
                    cursor_shape, display_offset,
                    sel_range, sel_bg, sel_fg,
                    default_bg, baseline, ch, cw,
                    x_off, y_off,
                    x_scale, y_scale,
                );
            }

            LigatureOutcome {
                handled: true,
                run_end,
                last_checked: run_end,
                has_new_glyphs,
            }
        }
        Err(e) => {
            log::warn!(
                "shape_and_rasterize_run failed for \
                 run={:?}: {e:?}; falling back to per-char",
                run_text,
            );
            LigatureOutcome {
                handled: false,
                run_end,
                last_checked: 0,
                has_new_glyphs: false,
            }
        }
    }
}
