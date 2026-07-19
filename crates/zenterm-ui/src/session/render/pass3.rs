//! Shared rendering pass 3+4: underline, strikethrough, and cursor style
//! decorations (Beam / Underline).

use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::vte::ansi::CursorShape;

use zenterm_core::cell::UnderlineStyle;
use zenterm_core::color::Rgba;
use zenterm_render::glyph_type;
use zenterm_render::CellInstance;
use zenterm_term::GridView;

/// Emit underline, strikethrough, and cursor-style (Beam/Underline)
/// decoration quads for a single cell.
///
/// This mirrors the "Pass 3" and "Pass 4" logic from the per-char
/// render path in [`super::update_cell_instances`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_deco_for_cell(
    deco_instances: &mut Vec<CellInstance>,
    grid: &GridView,
    row: usize,
    col: usize,
    _cols: usize,
    cursor_visible: bool,
    cursor_row: usize,
    cursor_col: usize,
    cursor_shape: CursorShape,
    cursor_bg: Rgba,
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
        (cursor_bg, cell.fg)
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
        // Underline/strikethrough under the cursor uses the cursor
        // fill colour for visual continuity.
        [cursor_bg.r(), cursor_bg.g(), cursor_bg.b(), 1.0]
    } else if is_sel {
        let deco_fg = sel_fg.unwrap_or(cell.fg);
        [deco_fg.r(), deco_fg.g(), deco_fg.b(), 1.0]
    } else {
        [draw_fg.r(), draw_fg.g(), draw_fg.b(), 1.0]
    };

    // Helper to push a solid decoration quad.
    let px_to_clip_x = |px: f32| px * x_scale - 1.0;
    let px_to_clip_y = |px: f32| 1.0 - px * y_scale;

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
        let cursor_color = [cursor_bg.r(), cursor_bg.g(), cursor_bg.b(), 1.0];
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
