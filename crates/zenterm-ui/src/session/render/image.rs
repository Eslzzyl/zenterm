//! Image-to-atlas quad generation for terminal cell rendering.

use zenterm_core::image::{ImageCell, ImageDataType};
use zenterm_render::{CellInstance, glyph_type};

use crate::glyph_cache::{ImageEntry, ImageKey, ImagePixels, SharedGlyphAtlas};

pub(super) type ImageEntryCache = std::collections::HashMap<(usize, ImageKey), ImageEntry>;

pub(super) fn image_source_id(img: &ImageCell) -> usize {
    std::sync::Arc::as_ptr(&img.data) as usize
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_image_quad(
    bufs: &mut Vec<Vec<CellInstance>>,
    atlas: &SharedGlyphAtlas,
    image_entries: &mut ImageEntryCache,
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
    let guard = img.data.data();
    let (pixels, img_w, img_h, img_hash) = match &*guard {
        ImageDataType::Rgba8 {
            data,
            width,
            height,
            hash,
        } => (ImagePixels::Shared(data), *width, *height, *hash),
        ImageDataType::AnimRgba8 {
            width,
            height,
            frames,
            hashes,
            ..
        } => {
            // Use the first frame for rendering (frame 0).
            // FUTURE: cycle through frames based on timing.
            let Some((data, hash)) = frames.first().zip(hashes.first()) else {
                return;
            };
            (ImagePixels::Shared(data), *width, *height, *hash)
        }
    };
    let key = ImageKey {
        hash: img_hash,
        width: img_w,
        height: img_h,
    };
    let source_id = image_source_id(img);
    let local_key = (source_id, key);
    let entry = if let Some(entry) = image_entries.get(&local_key) {
        *entry
    } else {
        let entry = atlas.ensure_image(pixels, img_w, img_h, img_hash, source_id);
        image_entries.insert(local_key, entry);
        entry
    };

    // Image textures use image-space UVs directly.
    let u_min = img.top_left.x;
    let v_min = img.top_left.y;
    let u_max = img.bottom_right.x;
    let v_max = img.bottom_right.y;

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

    let ai = entry.texture_index;
    if ai >= bufs.len() {
        bufs.resize_with(ai + 1, Vec::new);
    }
    bufs[ai].push(instance);
}
