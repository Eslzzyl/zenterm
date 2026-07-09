//! Atlas texture allocation — growing the underlying [`etagere::AtlasAllocator`]
//! when there is no room for a new glyph.

use etagere::AtlasAllocator;

use zenterm_core::{Error, Result};

use crate::GlyphAtlas;

impl GlyphAtlas {
    pub(crate) fn grow_atlas(&mut self) -> Result<()> {
        let new_size = self.texture_size * 2;
        if new_size > 4096 {
            return Err(Error::Glyph(
                "glyph atlas exceeds maximum size (4096)".into(),
            ));
        }
        self.atlas = AtlasAllocator::new(etagere::size2(new_size as i32, new_size as i32));
        self.texture_data
            .resize((new_size * new_size * 4) as usize, 0);
        self.texture_size = new_size;
        self.glyph_cache.clear();
        self.run_cache.clear();
        self.no_effect_cache.clear();
        self.image_cache.clear();
        Ok(())
    }
}
