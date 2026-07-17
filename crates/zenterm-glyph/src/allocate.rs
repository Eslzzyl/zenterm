//! Atlas texture allocation — pushing a new texture slot when the current
//! slot has no room for a glyph.
//!
//! Unlike the old single-atlas approach (which replaced the allocator and
//! invalidated all existing UVs), this pushes a new [`AtlasSlot`] onto
//! [`GlyphAtlas::slots`] without touching existing slots.  Prior glyph
//! entries continue to reference their original slot and remain valid.

use zenterm_core::{Error, Result};

use crate::{AtlasSlot, GlyphAtlas};

impl GlyphAtlas {
    /// Append a new, larger texture slot.
    ///
    /// Existing slots are unchanged — all prior [`GlyphEntry`] UV
    /// coordinates remain valid.
    pub(crate) fn grow_atlas(&mut self) -> Result<()> {
        let current_size = self
            .slots
            .last()
            .map(|s| s.size)
            .unwrap_or(512);
        let new_size = (current_size * 2).min(4096);

        if new_size > 4096 || new_size == current_size {
            return Err(Error::Glyph(
                "glyph atlas slots exceed maximum texture size (4096)".into(),
            ));
        }

        log::info!(
            "grow_atlas: pushing slot {} ({}×{})",
            self.slots.len(),
            new_size,
            new_size,
        );

        let allocator = etagere::AtlasAllocator::new(etagere::size2(
            new_size as i32,
            new_size as i32,
        ));
        let texture_data = vec![0u8; (new_size * new_size * 4) as usize];

        self.slots.push(AtlasSlot {
            allocator,
            texture_data,
            size: new_size,
        });

        Ok(())
    }
}
