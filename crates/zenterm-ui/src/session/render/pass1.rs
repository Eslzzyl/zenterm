//! Shared rendering pass 1: background quad.

use zenterm_core::color::Rgba;
use zenterm_render::glyph_type;
use zenterm_render::CellInstance;

/// Emit a solid-colour background quad for a single cell.
///
/// Skips emission when the background matches `default_bg` (no-op
/// optimisation), unless `force` is set (block cursor rendering).
pub(crate) fn emit_background_quad(
    instances: &mut Vec<CellInstance>,
    col: usize,
    row: usize,
    cell_w: f32,
    cell_h: f32,
    num_cells: f32,
    color: Rgba,
    default_bg: Rgba,
    force: bool,
    x_off: f32,
    y_off: f32,
    x_scale: f32,
    y_scale: f32,
) {
    if !force && color == default_bg {
        return;
    }
    let bg_x = x_off + (col as f32 * cell_w).round();
    let bg_y = y_off + (row as f32 * cell_h).round();
    instances.push(CellInstance {
        clip_pos: [bg_x * x_scale - 1.0, 1.0 - bg_y * y_scale],
        uv_min: [0.0; 2],
        uv_max: [0.0; 2],
        clip_cell_size: [cell_w * num_cells * x_scale, cell_h * y_scale],
        glyph_size: [0.0; 2],
        glyph_offset: [0.0; 2],
        fg_color: [color.r(), color.g(), color.b(), color.a()],
        bg_color: [color.r(), color.g(), color.b(), color.a()],
        flags: glyph_type::SOLID,
    });
}
