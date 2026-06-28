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

#[cfg(test)]
mod tests {
    use super::*;
    use zenterm_core::color::Rgba;

    #[test]
    fn emits_when_color_different_from_default() {
        let mut v = vec![];
        emit_background_quad(&mut v, 0, 0, 10.0, 20.0, 1.0,
            Rgba::new(1.0, 0.0, 0.0, 1.0),  // red
            Rgba::new(0.0, 0.0, 0.0, 1.0),  // default = black
            false, 0.0, 0.0, 2.0, 2.0);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].flags, glyph_type::SOLID);
    }

    #[test]
    fn skips_when_color_matches_default_and_not_force() {
        let mut v = vec![];
        emit_background_quad(&mut v, 0, 0, 10.0, 20.0, 1.0,
            Rgba::new(0.0, 0.0, 0.0, 1.0),
            Rgba::new(0.0, 0.0, 0.0, 1.0),  // same
            false, 0.0, 0.0, 2.0, 2.0);
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn force_overrides_default_bg_skip() {
        let mut v = vec![];
        emit_background_quad(&mut v, 0, 0, 10.0, 20.0, 1.0,
            Rgba::new(0.0, 0.0, 0.0, 1.0),
            Rgba::new(0.0, 0.0, 0.0, 1.0),  // same
            true,  // force
            0.0, 0.0, 2.0, 2.0);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn clip_position_computed_correctly() {
        let mut v = vec![];
        // col=1, row=2, cw=10, ch=20, x_off=5, y_off=5, x_scale=2, y_scale=2
        // bg_x = 5 + (1*10).round() = 15
        // bg_y = 5 + (2*20).round() = 45
        // clip_pos = [15*2-1=29, 1-45*2=-89]
        emit_background_quad(&mut v, 1, 2, 10.0, 20.0, 1.0,
            Rgba::new(1.0, 0.0, 0.0, 1.0),
            Rgba::new(0.0, 0.0, 0.0, 1.0),
            false, 5.0, 5.0, 2.0, 2.0);
        assert_eq!(v.len(), 1);
        let eps = 0.001;
        assert!((v[0].clip_pos[0] - 29.0).abs() < eps);
        assert!((v[0].clip_pos[1] - (-89.0)).abs() < eps);
    }

    #[test]
    fn wide_char_num_cells_doubles_width() {
        let mut v = vec![];
        // num_cells=2 should make clip_cell_size.x = 10*2*2 = 40
        emit_background_quad(&mut v, 0, 0, 10.0, 20.0, 2.0,
            Rgba::new(1.0, 0.0, 0.0, 1.0),
            Rgba::new(0.0, 0.0, 0.0, 1.0),
            false, 0.0, 0.0, 2.0, 2.0);
        assert_eq!(v.len(), 1);
        assert!((v[0].clip_cell_size[0] - 40.0).abs() < 0.001);
        assert!((v[0].clip_cell_size[1] - 40.0).abs() < 0.001);
    }

    #[test]
    fn fg_and_bg_color_match_input() {
        let mut v = vec![];
        let red = Rgba::new(1.0, 0.0, 0.0, 1.0);
        emit_background_quad(&mut v, 0, 0, 10.0, 20.0, 1.0,
            red, Rgba::new(0.0, 0.0, 0.0, 1.0),
            true, 0.0, 0.0, 2.0, 2.0);
        assert_eq!(v.len(), 1);
        assert!((v[0].fg_color[0] - 1.0).abs() < 0.001);
        assert!((v[0].fg_color[1]).abs() < 0.001);
        assert!((v[0].fg_color[2]).abs() < 0.001);
        assert_eq!(v[0].bg_color, v[0].fg_color);
    }
}

