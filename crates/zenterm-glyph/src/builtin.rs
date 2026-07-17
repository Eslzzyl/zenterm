//! Software-rasterized fallback for Unicode block elements.
//!
//! Terminal fonts typically render block/shade characters (U+2580–U+259F)
//! as dithered stipple patterns or with inconsistent metrics, producing
//! a "grid" visual effect instead of smooth solid blocks.
//!
//! Both Alacritty and Wezterm intercept these codepoints and replace them
//! with pixel-perfect software-rendered rectangles.  This module provides
//! the same fallback for Zenterm.
//!
//! All glyphs are rendered as **grayscale intensity masks** (1 byte/pixel,
//! 0 = transparent, 255 = fully opaque) which the shader treats as uniform
//! coverage (the `MASK` path).

use crate::GlyphContentType;

/// Parameters needed to render a built-in glyph.
pub struct BuiltinParams {
    /// Cell width in pixels.
    pub cell_width: u32,
    /// Cell height in pixels.
    pub cell_height: u32,
    /// Baseline offset: y-down distance from cell top to baseline, in pixels.
    /// Used as `bearing_y` so the glyph bitmap is vertically positioned on
    /// the baseline like a real font glyph.
    pub cell_ascent: f32,
}

/// A software-rasterized built-in glyph.
pub struct BuiltinGlyph {
    /// Width of the rendered image in pixels.
    pub width: u32,
    /// Height of the rendered image in pixels.
    pub height: u32,
    /// Grayscale pixel data (1 byte per pixel, 0–255).
    pub data: Vec<u8>,
    /// Always `GlyphContentType::Mask`.
    pub content_type: GlyphContentType,
    /// Horizontal bearing (always 0 for built-in glyphs).
    pub bearing_x: f32,
    /// Vertical bearing (always `cell_height` for built-in glyphs).
    pub bearing_y: f32,
    /// Advance width (equal to `cell_width`).
    pub advance: f32,
}

/// Check whether a character should be handled by the built-in renderer.
pub fn is_builtin(c: char) -> bool {
    matches!(
        c,
        // Block Elements (U+2580–U+259F)
        '\u{2580}'..='\u{259F}'
        // Box Drawing (U+2500–U+257F) — basic set for now
        | '\u{2500}'..='\u{257F}'
        // Full block shortcut is already in the range above,
        // but we list it explicitly for clarity:
        // U+2588 = FULL BLOCK, U+2580 = UPPER HALF BLOCK, etc.
    )
}

/// Render a built-in glyph for the given character.
///
/// Returns `None` if the character is not in the built-in range (callers
/// should check [`is_builtin`] first to avoid this).
pub fn render(c: char, params: &BuiltinParams) -> Option<BuiltinGlyph> {
    let w = params.cell_width;
    let h = params.cell_height;
    let by = params.cell_ascent;

    match c {
        // ── Full block █ (U+2588) ────────────────────────────────────
        '\u{2588}' => Some(full_block(w, h, by)),

        // ── Shade characters ─────────────────────────────────────────
        // ░ Light shade (U+2591) — 25% intensity
        // ▒ Medium shade (U+2592) — 50% intensity
        // ▓ Dark shade (U+2593) — 75% intensity
        '\u{2591}' => Some(solid_fill(w, h, by, 64)),   // 25%
        '\u{2592}' => Some(solid_fill(w, h, by, 128)),  // 50%
        '\u{2593}' => Some(solid_fill(w, h, by, 192)),  // 75%

        // ── Half blocks ──────────────────────────────────────────────
        // ▀ Upper half block (U+2580)
        '\u{2580}' => Some(half_block(w, h, by, Half::Upper)),
        // ▄ Lower half block (U+2584)
        '\u{2584}' => Some(half_block(w, h, by, Half::Lower)),
        // ▌ Left half block (U+258C)
        '\u{258c}' => Some(half_block(w, h, by, Half::Left)),
        // ▐ Right half block (U+2590)
        '\u{2590}' => Some(half_block(w, h, by, Half::Right)),

        // ── Quadrants (U+2596–U+259F) ────────────────────────────────
        // ▖ Lower left quadrant
        '\u{2596}' => Some(quadrant_lower_left(w, h, by)),
        // ▗ Lower right quadrant
        '\u{2597}' => Some(quadrant_lower_right(w, h, by)),
        // ▘ Upper left quadrant
        '\u{2598}' => Some(quadrant_upper_left(w, h, by)),
        // ▙ Upper left + lower left + lower right
        '\u{2599}' => Some(quadrant_three(w, h, by, true, true, false, true)),
        // ▚ Upper left + lower right
        '\u{259a}' => Some(quadrant_two_diagonal(w, h, by)),
        // ▛ Upper left + upper right + lower left
        '\u{259b}' => Some(quadrant_three(w, h, by, true, true, true, false)),
        // ▜ Upper left + upper right + lower right
        '\u{259c}' => Some(quadrant_three(w, h, by, true, true, false, true)), // same as ▙ but mirrored
        // ▝ Upper right quadrant
        '\u{259d}' => Some(quadrant_upper_right(w, h, by)),
        // ▞ Upper right + lower left
        '\u{259e}' => Some(quadrant_two_diagonal_mirror(w, h, by)),
        // ▟ Upper right + lower left + lower right
        '\u{259f}' => Some(quadrant_three(w, h, by, false, true, true, true)),

        // ── Box drawing (basic horizontal/vertical) ──────────────────
        // Light ─ (U+2500)
        '\u{2500}' => Some(hline(w, h, by, true)),
        // Light │ (U+2502)
        '\u{2502}' => Some(vline(w, h, by, true)),
        // Heavy ━ (U+2501)
        '\u{2501}' => Some(hline(w, h, by, false)),
        // Heavy ┃ (U+2503)
        '\u{2503}' => Some(vline(w, h, by, false)),

        // ── Box drawing corners ──────────────────────────────────────
        // Light ┌ (U+250C)
        '\u{250c}' => Some(corner(w, h, by, Corner::DownRight, true)),
        // Light ┐ (U+2510)
        '\u{2510}' => Some(corner(w, h, by, Corner::DownLeft, true)),
        // Light └ (U+2514)
        '\u{2514}' => Some(corner(w, h, by, Corner::UpRight, true)),
        // Light ┘ (U+2518)
        '\u{2518}' => Some(corner(w, h, by, Corner::UpLeft, true)),

        // Heavy ┏ (U+250F)
        '\u{250f}' => Some(corner(w, h, by, Corner::DownRight, false)),
        // Heavy ┓ (U+2513)
        '\u{2513}' => Some(corner(w, h, by, Corner::DownLeft, false)),
        // Heavy ┗ (U+2517)
        '\u{2517}' => Some(corner(w, h, by, Corner::UpRight, false)),
        // Heavy ┛ (U+251B)
        '\u{251b}' => Some(corner(w, h, by, Corner::UpLeft, false)),

        // ── T-junctions (light) ──────────────────────────────────────
        // ├ (U+251C)
        '\u{251c}' => Some(t_junction(w, h, by, true, TType::Left)),
        // ┤ (U+2524)
        '\u{2524}' => Some(t_junction(w, h, by, true, TType::Right)),
        // ┬ (U+252C)
        '\u{252c}' => Some(t_junction(w, h, by, true, TType::Down)),
        // ┴ (U+2534)
        '\u{2534}' => Some(t_junction(w, h, by, true, TType::Up)),
        // ┼ (U+253C)
        '\u{253c}' => Some(cross(w, h, by, true)),

        // ── Cross (heavy) ────────────────────────────────────────────
        // ╋ (U+254B)
        '\u{254b}' => Some(cross(w, h, by, false)),

        _ => None,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Stroke thickness for box drawing, proportional to cell width.
///
/// Uses one eighth of the cell width (matching Alacritty's approach in
/// `builtin_font.rs:calculate_stroke_size`), with a minimum of 1px.
fn line_width(w: u32, _h: u32) -> u32 {
    (w as f32 / 8.0).round().max(1.0) as u32
}

/// Set a single pixel in the buffer.
fn set_pixel(buf: &mut [u8], w: u32, _h: u32, x: u32, y: u32, val: u8) {
    let idx = (y * w + x) as usize;
    if idx < buf.len() {
        buf[idx] = val;
    }
}

/// Draw a filled rectangle region.
fn fill_region(buf: &mut [u8], buf_w: u32, _buf_h: u32,
               x: u32, y: u32, rw: u32, rh: u32, val: u8) {
    for row in y..y + rh {
        for col in x..x + rw {
            set_pixel(buf, buf_w, _buf_h, col, row, val);
        }
    }
}

/// Draw a horizontal line segment starting at `start_x` for `length` pixels.
fn draw_hline_segment(
    buf: &mut [u8],
    w: u32,
    h: u32,
    start_x: u32,
    y: u32,
    length: u32,
    thickness: u32,
    val: u8,
) {
    if length == 0 {
        return;
    }
    for t in 0..thickness {
        let row = y + t;
        if row < h {
            let end_x = (start_x + length).min(w);
            if start_x < end_x {
                fill_region(buf, w, h, start_x, row, end_x - start_x, 1, val);
            }
        }
    }
}

/// Draw a vertical line segment starting at `start_y` for `length` pixels.
fn draw_vline_segment(
    buf: &mut [u8],
    w: u32,
    h: u32,
    x: u32,
    start_y: u32,
    length: u32,
    thickness: u32,
    val: u8,
) {
    if length == 0 {
        return;
    }
    for t in 0..thickness {
        let col = x + t;
        if col < w {
            let end_y = (start_y + length).min(h);
            if start_y < end_y {
                fill_region(buf, w, h, col, start_y, 1, end_y - start_y, val);
            }
        }
    }
}

/// Draw a horizontal line across the full width at row `y`.
fn draw_hline(buf: &mut [u8], w: u32, h: u32, y: u32, thickness: u32, val: u8) {
    draw_hline_segment(buf, w, h, 0, y, w, thickness, val);
}

/// Draw a vertical line across the full height at column `x`.
fn draw_vline(buf: &mut [u8], w: u32, h: u32, x: u32, thickness: u32, val: u8) {
    draw_vline_segment(buf, w, h, x, 0, h, thickness, val);
}

// ── Glyph generators ────────────────────────────────────────────────────

/// Create a fully opaque block (█).
fn full_block(w: u32, h: u32, by: f32) -> BuiltinGlyph {
    let data = vec![255u8; (w * h) as usize];
    builtin_result(w, h, by, data)
}

/// Create a solid fill with given intensity.
fn solid_fill(w: u32, h: u32, by: f32, intensity: u8) -> BuiltinGlyph {
    let data = vec![intensity; (w * h) as usize];
    builtin_result(w, h, by, data)
}

enum Half {
    Upper,
    Lower,
    Left,
    Right,
}

/// Create a half-block glyph (▀ ▄ ▌ ▐).
fn half_block(w: u32, h: u32, by: f32, which: Half) -> BuiltinGlyph {
    let mut data = vec![0u8; (w * h) as usize];
    match which {
        Half::Upper => {
            fill_region(&mut data, w, h, 0, 0, w, h / 2, 255);
        }
        Half::Lower => {
            fill_region(&mut data, w, h, 0, h / 2, w, h - h / 2, 255);
        }
        Half::Left => {
            fill_region(&mut data, w, h, 0, 0, w / 2, h, 255);
        }
        Half::Right => {
            fill_region(&mut data, w, h, w / 2, 0, w - w / 2, h, 255);
        }
    }
    builtin_result(w, h, by, data)
}

// ── Quadrant helpers ────────────────────────────────────────────────────

fn quad_rect(w: u32, h: u32) -> (u32, u32) {
    ((w + 1) / 2, (h + 1) / 2)
}

fn quadrant_lower_left(w: u32, h: u32, by: f32) -> BuiltinGlyph {
    let (hw, hh) = quad_rect(w, h);
    let mut data = vec![0u8; (w * h) as usize];
    fill_region(&mut data, w, h, 0, hh, hw, h - hh, 255);
    builtin_result(w, h, by, data)
}

fn quadrant_lower_right(w: u32, h: u32, by: f32) -> BuiltinGlyph {
    let (hw, hh) = quad_rect(w, h);
    let mut data = vec![0u8; (w * h) as usize];
    fill_region(&mut data, w, h, hw, hh, w - hw, h - hh, 255);
    builtin_result(w, h, by, data)
}

fn quadrant_upper_left(w: u32, h: u32, by: f32) -> BuiltinGlyph {
    let (hw, hh) = quad_rect(w, h);
    let mut data = vec![0u8; (w * h) as usize];
    fill_region(&mut data, w, h, 0, 0, hw, hh, 255);
    builtin_result(w, h, by, data)
}

fn quadrant_upper_right(w: u32, h: u32, by: f32) -> BuiltinGlyph {
    let (hw, hh) = quad_rect(w, h);
    let mut data = vec![0u8; (w * h) as usize];
    fill_region(&mut data, w, h, hw, 0, w - hw, hh, 255);
    builtin_result(w, h, by, data)
}

fn quadrant_two_diagonal(w: u32, h: u32, by: f32) -> BuiltinGlyph {
    let (hw, hh) = quad_rect(w, h);
    let mut data = vec![0u8; (w * h) as usize];
    fill_region(&mut data, w, h, 0, 0, hw, hh, 255);       // UL
    fill_region(&mut data, w, h, hw, hh, w - hw, h - hh, 255); // LR
    builtin_result(w, h, by, data)
}

fn quadrant_two_diagonal_mirror(w: u32, h: u32, by: f32) -> BuiltinGlyph {
    let (hw, hh) = quad_rect(w, h);
    let mut data = vec![0u8; (w * h) as usize];
    fill_region(&mut data, w, h, hw, 0, w - hw, hh, 255);  // UR
    fill_region(&mut data, w, h, 0, hh, hw, h - hh, 255);  // LL
    builtin_result(w, h, by, data)
}

#[allow(clippy::too_many_arguments)]
fn quadrant_three(
    w: u32, h: u32, by: f32,
    ul: bool, ur: bool, ll: bool, lr: bool,
) -> BuiltinGlyph {
    let (hw, hh) = quad_rect(w, h);
    let mut data = vec![0u8; (w * h) as usize];
    if ul { fill_region(&mut data, w, h, 0, 0, hw, hh, 255); }
    if ur { fill_region(&mut data, w, h, hw, 0, w - hw, hh, 255); }
    if ll { fill_region(&mut data, w, h, 0, hh, hw, h - hh, 255); }
    if lr { fill_region(&mut data, w, h, hw, hh, w - hw, h - hh, 255); }
    builtin_result(w, h, by, data)
}

// ── Box drawing helpers ─────────────────────────────────────────────────

/// Horizontal line (─ ━).
fn hline(w: u32, h: u32, by: f32, heavy: bool) -> BuiltinGlyph {
    let mut data = vec![0u8; (w * h) as usize];
    let lw = line_width(w, h);
    let sw = if heavy { lw * 2 } else { lw };
    let y = h / 2;
    draw_hline(&mut data, w, h, y.saturating_sub(sw / 2), sw, 255);
    builtin_result(w, h, by, data)
}

/// Vertical line (│ ┃).
fn vline(w: u32, h: u32, by: f32, heavy: bool) -> BuiltinGlyph {
    let mut data = vec![0u8; (w * h) as usize];
    let lw = line_width(w, h);
    let sw = if heavy { lw * 2 } else { lw };
    let x = w / 2;
    draw_vline(&mut data, w, h, x.saturating_sub(sw / 2), sw, 255);
    builtin_result(w, h, by, data)
}

enum Corner { DownRight, DownLeft, UpRight, UpLeft }

/// Box drawing corner (┌ ┐ └ ┘) and heavy variants (┏ ┓ ┗ ┛).
///
/// `heavy=false` → light stroke, `heavy=true` → double-width heavy stroke.
fn corner(w: u32, h: u32, by: f32, which: Corner, heavy: bool) -> BuiltinGlyph {
    let mut data = vec![0u8; (w * h) as usize];
    let lw = line_width(w, h);
    let sw = if heavy { lw * 2 } else { lw };
    let cx = w / 2;
    let cy = h / 2;

    // Each corner only draws 2 of the 4 possible segments (matching
    // Alacritty's 4-segment model and WezTerm's Poly path approach):
    //
    //   │
    // ──┼──  h1=left hline, h2=right hline
    //   │    v1=top vline, v2=bottom vline
    //
    //   ┌ = h2 + v2    ┐ = h1 + v2
    //   └ = h2 + v1    ┘ = h1 + v1
    match which {
        Corner::DownRight => {
            // ┌: right horizontal + bottom vertical
            draw_hline_segment(&mut data, w, h, cx, cy.saturating_sub(sw / 2),
                               w.saturating_sub(cx), sw, 255);
            draw_vline_segment(&mut data, w, h, cx.saturating_sub(sw / 2), cy,
                               h.saturating_sub(cy), sw, 255);
        }
        Corner::DownLeft => {
            // ┐: left horizontal + bottom vertical
            draw_hline_segment(&mut data, w, h, 0, cy.saturating_sub(sw / 2),
                               cx, sw, 255);
            draw_vline_segment(&mut data, w, h, cx.saturating_sub(sw / 2), cy,
                               h.saturating_sub(cy), sw, 255);
        }
        Corner::UpRight => {
            // └: right horizontal + top vertical
            draw_hline_segment(&mut data, w, h, cx, cy.saturating_sub(sw / 2),
                               w.saturating_sub(cx), sw, 255);
            draw_vline_segment(&mut data, w, h, cx.saturating_sub(sw / 2), 0,
                               cy, sw, 255);
        }
        Corner::UpLeft => {
            // ┘: left horizontal + top vertical
            draw_hline_segment(&mut data, w, h, 0, cy.saturating_sub(sw / 2),
                               cx, sw, 255);
            draw_vline_segment(&mut data, w, h, cx.saturating_sub(sw / 2), 0,
                               cy, sw, 255);
        }
    }
    builtin_result(w, h, by, data)
}

enum TType { Left, Right, Up, Down }

/// Box drawing T-junction (├ ┤ ┬ ┴).
///
/// `heavy=false` → light stroke, `heavy=true` → double-width heavy stroke.
fn t_junction(w: u32, h: u32, by: f32, heavy: bool, ttype: TType) -> BuiltinGlyph {
    let mut data = vec![0u8; (w * h) as usize];
    let lw = line_width(w, h);
    let sw = if heavy { lw * 2 } else { lw };
    let cx = w / 2;
    let cy = h / 2;

    match ttype {
        TType::Left => {
            // ├: full vertical + right horizontal
            draw_vline(&mut data, w, h, cx.saturating_sub(sw / 2), sw, 255);
            draw_hline_segment(&mut data, w, h, cx, cy.saturating_sub(sw / 2),
                               w.saturating_sub(cx), sw, 255);
        }
        TType::Right => {
            // ┤: full vertical + left horizontal
            draw_vline(&mut data, w, h, cx.saturating_sub(sw / 2), sw, 255);
            draw_hline_segment(&mut data, w, h, 0, cy.saturating_sub(sw / 2),
                               cx, sw, 255);
        }
        TType::Down => {
            // ┬: full horizontal + bottom vertical
            draw_hline(&mut data, w, h, cy.saturating_sub(sw / 2), sw, 255);
            draw_vline_segment(&mut data, w, h, cx.saturating_sub(sw / 2), cy,
                               h.saturating_sub(cy), sw, 255);
        }
        TType::Up => {
            // ┴: full horizontal + top vertical
            draw_hline(&mut data, w, h, cy.saturating_sub(sw / 2), sw, 255);
            draw_vline_segment(&mut data, w, h, cx.saturating_sub(sw / 2), 0,
                               cy, sw, 255);
        }
    }
    builtin_result(w, h, by, data)
}

/// Box drawing cross (┼ ╋).
///
/// `heavy=false` → light stroke, `heavy=true` → double-width heavy stroke.
fn cross(w: u32, h: u32, by: f32, heavy: bool) -> BuiltinGlyph {
    let mut data = vec![0u8; (w * h) as usize];
    let lw = line_width(w, h);
    let sw = if heavy { lw * 2 } else { lw };
    let cx = w / 2;
    let cy = h / 2;
    draw_hline(&mut data, w, h, cy.saturating_sub(sw / 2), sw, 255);
    draw_vline(&mut data, w, h, cx.saturating_sub(sw / 2), sw, 255);
    builtin_result(w, h, by, data)
}

// ── Result builder ───────────────────────────────────────────────────────

fn builtin_result(w: u32, h: u32, bearing_y: f32, data: Vec<u8>) -> BuiltinGlyph {
    BuiltinGlyph {
        width: w,
        height: h,
        data,
        content_type: GlyphContentType::Mask,
        bearing_x: 0.0,
        bearing_y,
        advance: w as f32,
    }
}
