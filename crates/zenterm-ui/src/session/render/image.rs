//! Image-to-atlas quad generation for terminal cell rendering.

use zenterm_core::image::{ImageCell, ImageDataType};
use zenterm_render::{CellInstance, glyph_type};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_image_quad(
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
            ImageDataType::Rgba8 {
                data,
                width,
                height,
                hash,
            } => (data.clone(), *width, *height, *hash),
            ImageDataType::AnimRgba8 {
                width,
                height,
                frames,
                hashes,
                ..
            } => {
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
