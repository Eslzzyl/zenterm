//! Glyph atlas — rasterizes characters with `cosmic-text` and packs them
//! into a GPU-friendly texture atlas using `etagere`.

use std::collections::HashMap;

use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache};
use etagere::AtlasAllocator;

use zenmux_core::{Error, Result};

/// A single glyph's position and metrics within the atlas.
#[derive(Debug, Clone)]
pub struct GlyphEntry {
    /// Allocated rectangle within the atlas texture (in pixels).
    pub atlas_rect: etagere::Rectangle,
    /// Horizontal bearing (pixels from origin to glyph left edge).
    pub bearing_x: f32,
    /// Vertical bearing (pixels from baseline to glyph top).
    pub bearing_y: f32,
    /// Advance width (pixels to move cursor after this glyph).
    pub advance: f32,
}

/// The glyph atlas.
pub struct GlyphAtlas {
    pub font_system: FontSystem,
    atlas: AtlasAllocator,
    /// RGBA pixel data of the atlas texture.
    pub texture_data: Vec<u8>,
    /// Current atlas texture size (power of two).
    pub texture_size: u32,
    font_size: f32,
    metrics: Metrics,
    glyph_cache: HashMap<(char, u32), GlyphEntry>,
    swash_cache: SwashCache,
}

impl GlyphAtlas {
    /// Create a new glyph atlas with the given font size (in pixels).
    ///
    /// The atlas starts at 512×512 and grows as needed.
    pub fn new(font_size: f32) -> Self {
        let font_system = FontSystem::new();
        let metrics = Metrics::new(font_size, font_size * 1.2);

        let initial_size: u32 = 512;
        let atlas = AtlasAllocator::new(etagere::size2(initial_size as i32, initial_size as i32));
        let texture_data = vec![0u8; (initial_size * initial_size * 4) as usize];

        Self {
            font_system,
            atlas,
            texture_data,
            texture_size: initial_size,
            font_size,
            metrics,
            glyph_cache: HashMap::new(),
            swash_cache: SwashCache::new(),
        }
    }

    /// Ensure the given character is rasterised and packed into the atlas.
    pub fn ensure_glyph(&mut self, c: char) -> Result<&GlyphEntry> {
        let key = (c, self.font_size.to_bits());

        if !self.glyph_cache.contains_key(&key) {
            self.rasterize_glyph(c)?;
        }

        Ok(self.glyph_cache.get(&key).unwrap())
    }

    /// Font metrics for layout calculations.
    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// Font size in pixels.
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Grow the atlas texture.
    fn grow_atlas(&mut self) -> Result<()> {
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
        Ok(())
    }

    /// Rasterize a single character, pack it into the atlas, and cache it.
    fn rasterize_glyph(&mut self, c: char) -> Result<()> {
        let key = (c, self.font_size.to_bits());

        // ── 1. Shape the character ────────────────────────────────────
        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        buffer.set_size(Some(self.font_size), None);
        let attrs = Attrs::new();
        buffer.set_text(&c.to_string(), &attrs, Shaping::Basic, None);
        buffer.shape_until_scroll(&mut self.font_system, true);

        let glyphs = buffer.lines[0]
            .layout_opt()
            .and_then(|lines| lines.first())
            .map(|line| &line.glyphs[..])
            .unwrap_or_default();

        let gl = match glyphs.first() {
            Some(g) => g,
            None => {
                self.glyph_cache.insert(
                    key,
                    GlyphEntry {
                        atlas_rect: etagere::Rectangle {
                            min: etagere::Point::new(0, 0),
                            max: etagere::Point::new(0, 0),
                        },
                        bearing_x: 0.0,
                        bearing_y: 0.0,
                        advance: 0.0,
                    },
                );
                return Ok(());
            }
        };

        // ── 2. Get physical glyph (with cache_key) ───────────────────
        let physical_glyph = gl.physical((0.0, 0.0), 1.0);
        let advance = gl.w; // glyph width approximates advance for monospace

        // ── 3. Rasterize via SwashCache ───────────────────────────────
        let physical = self
            .swash_cache
            .get_image(&mut self.font_system, physical_glyph.cache_key)
            .clone();

        let img = match physical {
            Some(img) => img,
            None => {
                self.glyph_cache.insert(
                    key,
                    GlyphEntry {
                        atlas_rect: etagere::Rectangle {
                            min: etagere::Point::new(0, 0),
                            max: etagere::Point::new(0, 0),
                        },
                        bearing_x: 0.0,
                        bearing_y: 0.0,
                        advance: advance,
                    },
                );
                return Ok(());
            }
        };

        let width = img.placement.width as i32;
        let height = img.placement.height as i32;

        if width <= 0 || height <= 0 {
            self.glyph_cache.insert(
                key,
                GlyphEntry {
                    atlas_rect: etagere::Rectangle {
                        min: etagere::Point::new(0, 0),
                        max: etagere::Point::new(0, 0),
                    },
                    bearing_x: img.placement.left as f32,
                    bearing_y: img.placement.top as f32,
                    advance: advance,
                },
            );
            return Ok(());
        }

        // ── 4. Allocate in atlas ──────────────────────────────────────
        let allocation = loop {
            match self.atlas.allocate(etagere::size2(width, height)) {
                Some(id) => break id,
                None => self.grow_atlas()?,
            }
        };

        let rectangle = self.atlas.get(allocation.id);

        // ── 5. Copy pixels into the RGBA atlas ────────────────────────
        let atlas_w = self.texture_size as usize;
        for (i, &alpha) in img.data.iter().enumerate() {
            let px = (rectangle.min.x as usize) + (i % width as usize);
            let py = (rectangle.min.y as usize) + (i / width as usize);
            let idx = (py * atlas_w + px) * 4;
            if idx + 3 < self.texture_data.len() {
                self.texture_data[idx] = 255;
                self.texture_data[idx + 1] = 255;
                self.texture_data[idx + 2] = 255;
                self.texture_data[idx + 3] = alpha;
            }
        }

        self.glyph_cache.insert(
            key,
            GlyphEntry {
                atlas_rect: rectangle,
                bearing_x: img.placement.left as f32,
                bearing_y: img.placement.top as f32,
                advance: advance,
            },
        );

        Ok(())
    }
}
