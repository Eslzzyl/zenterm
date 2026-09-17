//! Image allocation and eviction in the glyph atlas.

use zenterm_core::Result;

use crate::{GlyphAtlas, GlyphContentType, GlyphEntry};

impl GlyphAtlas {
    /// `data` must be premultiplied RGBA pixels in sRGB colour space,
    /// `width` / `height` in pixels, and `hash` is the unique content
    /// identifier (computed by [`ImageDataType::hash`]).
    ///
    /// Returns the atlas [`GlyphEntry`] (atlas rect + UV helpers).
    /// If the atlas is too small it will be grown automatically.
    pub fn ensure_image(
        &mut self,
        data: &[u8],
        width: u32,
        height: u32,
        hash: [u8; 32],
    ) -> Result<GlyphEntry> {
        self.ensure_image_with_status(data, width, height, hash)
            .map(|(entry, _)| entry)
    }

    /// Ensure an image is present and report whether atlas pixels were added.
    ///
    /// The status lets the renderer synchronize image uploads even when no
    /// new glyph was rasterised in the same frame.
    pub fn ensure_image_with_status(
        &mut self,
        data: &[u8],
        width: u32,
        height: u32,
        hash: [u8; 32],
    ) -> Result<(GlyphEntry, bool)> {
        if let Some((entry, _)) = self.image_cache.get(&hash) {
            return Ok((entry.clone(), false));
        }

        let iw = width as i32;
        let ih = height as i32;

        // Allocate in the current atlas slot.
        // slot_idx is recomputed each iteration so that after grow_atlas
        // pushes a new slot we retry the fresh slot, not the old full one.
        let (allocation, slot_idx) = loop {
            let idx = self.slots.len() - 1;
            match self.slots[idx].allocator.allocate(etagere::size2(iw, ih)) {
                Some(a) => break (a, idx),
                None => self.grow_atlas()?,
            }
        };
        let rect = allocation.rectangle;

        // Copy premultiplied RGBA into the slot's texture.
        let atlas_w = self.slots[slot_idx].size as usize;
        let texture_data = &mut self.slots[slot_idx].texture_data;
        for y in 0..height {
            for x in 0..width {
                let si = ((y * width + x) * 4) as usize;
                let dx = rect.min.x as usize + x as usize;
                let dy = rect.min.y as usize + y as usize;
                let di = (dy * atlas_w + dx) * 4;
                if di + 3 < texture_data.len() && si + 3 < data.len() {
                    texture_data[di..di + 4].copy_from_slice(&data[si..si + 4]);
                }
            }
        }
        self.mark_dirty_region(slot_idx, rect);

        let entry = GlyphEntry {
            atlas_index: slot_idx as u32,
            atlas_rect: rect,
            bearing_x: 0.0,
            bearing_y: 0.0,
            advance: 0.0,
            content_type: GlyphContentType::Color,
            scale: 1.0,
        };
        self.image_cache
            .insert(hash, (entry.clone(), allocation.id));
        Ok((entry, true))
    }

    /// Remove an image from the atlas, freeing its texture slot.
    /// Called when the image is evicted from [`ImageCache`] so the GPU
    /// texture space can be reused.
    pub fn remove_image(&mut self, hash: &[u8; 32]) {
        if let Some((entry, id)) = self.image_cache.remove(hash) {
            self.slots[entry.atlas_index as usize]
                .allocator
                .deallocate(id);
        }
    }
}
