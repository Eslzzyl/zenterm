//! Glyph atlas — rasterizes characters with `cosmic-text` (shaping) + `swash`
//! (subpixel rasterization) and packs them into a GPU-friendly texture atlas.
//!
//! Unlike cosmic-text's built-in `SwashCache` (which hardcodes `Format::Alpha`),
//! we call swash directly with `Format::Subpixel` to get per-channel RGB coverage
//! values for LCD subpixel rendering.

use std::borrow::Cow;
use std::collections::HashMap;

use cosmic_text::{
    Attrs, Buffer, CacheKeyFlags, Family, FontSystem, Metrics, Shaping,
};
use etagere::AtlasAllocator;
use swash::scale::image::Content as SwashContent;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Angle, Format, Transform, Vector};

use zenterm_core::{Error, Result, SubpixelLayout};

pub mod builtin;

/// The type of content stored in a glyph's atlas entry.
///
/// This determines how the fragment shader interprets the texture data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphContentType {
    /// LCD subpixel coverage (per-channel R/G/B values).  Most text.
    Subpixel,
    /// Grayscale alpha / intensity mask.  Built-in block glyphs.
    Mask,
    /// Full RGBA color.  Emoji / color glyphs.
    Color,
}

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
    /// Type of content stored in the atlas for this glyph.
    pub content_type: GlyphContentType,
}

/// The glyph atlas.
pub struct GlyphAtlas {
    pub font_system: FontSystem,
    atlas: AtlasAllocator,
    /// RGBA pixel data of the atlas texture.
    ///
    /// For subpixel-rendered glyphs each pixel stores
    ///   R = red subpixel coverage,
    ///   G = green subpixel coverage,
    ///   B = blue subpixel coverage,
    ///   A = max(R,G,B)  (opaque coverage).
    ///
    /// For color glyphs (emojis) the pixel is premultiplied RGBA.
    pub texture_data: Vec<u8>,
    /// Current atlas texture size (power of two).
    pub texture_size: u32,
    font_size: f32,
    /// Font family name used for shaping (e.g. "Consolas", "Menlo").
    font_family: Cow<'static, str>,
    /// Display scale factor (physical pixels per logical point).
    pixels_per_point: f32,
    /// LCD subpixel order (RGB or BGR), auto-detected from the OS.
    subpixel_layout: SubpixelLayout,
    metrics: Metrics,
    glyph_cache: HashMap<(char, u32), GlyphEntry>,
    /// Swash scale context (replaces cosmic-text's `SwashCache`).
    swash_ctx: ScaleContext,
    /// Cached cell width/height in pixels, set by [`cell_size()`](Self::cell_size).
    cell_width: f32,
    cell_height: f32,
}

impl GlyphAtlas {
    /// Create a new glyph atlas with the given font size (in pixels),
    /// font family, and LCD subpixel layout.
    ///
    /// The atlas starts at 512×512 and grows as needed.
    pub fn new(
        font_size: f32,
        font_family: Cow<'static, str>,
        pixels_per_point: f32,
        subpixel_layout: SubpixelLayout,
    ) -> Self {
        log::info!(
            "GlyphAtlas: font_size={font_size:.1} family={font_family:?} \
             pixels_per_point={pixels_per_point:.2} subpixel={subpixel_layout:?}",
        );
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
            font_family,
            pixels_per_point,
            subpixel_layout,
            metrics,
            glyph_cache: HashMap::new(),
            swash_ctx: ScaleContext::new(),
            cell_width: 0.0,
            cell_height: 0.0,
        }
    }

    /// Return the platform-appropriate default monospace font family.
    ///
    /// Matches the strategy used by Alacritty: each platform gets its
    /// standard monospace font — Consolas on Windows, Menlo on macOS,
    /// and the fontconfig `monospace` generic family on Linux.
    pub fn default_font_family() -> Cow<'static, str> {
        if cfg!(target_os = "windows") {
            Cow::Borrowed("Consolas")
        } else if cfg!(target_os = "macos") {
            Cow::Borrowed("Menlo")
        } else {
            Cow::Borrowed("monospace")
        }
    }

    /// Ensure the given character is rasterised and packed into the atlas.
    ///
    /// Returns `true` if a new glyph was rasterised (caller should mark
    /// the atlas texture as dirty so it gets uploaded to the GPU).
    pub fn ensure_glyph(&mut self, c: char) -> Result<(&GlyphEntry, bool)> {
        let key = (c, self.font_size.to_bits());
        let is_new = !self.glyph_cache.contains_key(&key);

        if is_new {
            self.rasterize_glyph(c)?;
        }

        Ok((self.glyph_cache.get(&key).unwrap(), is_new))
    }

    /// Font metrics for layout calculations.
    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// Font size in pixels.
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Returns the cell size (width, height) in pixels.
    ///
    /// Width is the advance of a representative monospace glyph ('W').
    /// Height is the font's line height (font_size × 1.2).
    /// This method will rasterize 'W' if it isn't cached yet.
    /// Once computed, the values are cached for subsequent calls.
    pub fn cell_size(&mut self) -> Result<(f32, f32)> {
        if self.cell_width > 0.0 && self.cell_height > 0.0 {
            return Ok((self.cell_width, self.cell_height));
        }
        let (entry, _is_new) = self.ensure_glyph('W')?;
        self.cell_width = entry.advance;
        self.cell_height = self.metrics.line_height;
        Ok((self.cell_width, self.cell_height))
    }

    /// Return the cached cell dimensions (must call `cell_size()` first).
    pub fn cell_dimensions(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
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

    /// Rasterize a single character using swash with `Format::Subpixel`,
    /// pack it into the atlas, and cache it.
    ///
    /// Unicode block/shade characters (U+2500–U+259F) are intercepted and
    /// rendered via the built-in software rasterizer ([`builtin`] module)
    /// instead of the system font, giving pixel-perfect solid blocks.
    fn rasterize_glyph(&mut self, c: char) -> Result<()> {
        let key = (c, self.font_size.to_bits());

        // ── 0. Built-in block glyphs ──────────────────────────────
        // Intercept before cosmic-text so we get pixel-perfect solid
        // rectangles instead of font-provided dither patterns.
        if builtin::is_builtin(c) && self.cell_width > 0.0 && self.cell_height > 0.0 {
            return self.rasterize_builtin(c);
        }

        // ── 1. Shape the character (cosmic-text Buffer) ───────────────
        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        buffer.set_size(Some(self.font_size), None);
        let shaping = if c.is_ascii_graphic() || c == ' ' {
            Shaping::Basic
        } else {
            Shaping::Advanced
        };
        let attrs = Attrs::new().family(Family::Name(&self.font_family));
        buffer.set_text(&c.to_string(), &attrs, shaping, None);
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
                        content_type: GlyphContentType::Subpixel,
                    },
                );
                return Ok(());
            }
        };

        // ── 2. Get physical glyph (with cache_key) ───────────────────
        let mut physical_glyph = gl.physical((0.0, 0.0), 1.0);
        let advance = gl.w;

        // Disable hinting at high DPI (wezterm strategy).
        if self.pixels_per_point > 1.04 {
            physical_glyph
                .cache_key
                .flags
                .insert(CacheKeyFlags::DISABLE_HINTING);
        }

        // ── 3. Rasterize via swash with Format::Subpixel ─────────────
        let img = self.rasterize_swash(&physical_glyph.cache_key);

        let img = match img {
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
                        advance,
                        content_type: GlyphContentType::Subpixel,
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
                    advance,
                    content_type: GlyphContentType::Subpixel,
                },
            );
            return Ok(());
        }

        // ── 4. Allocate in atlas ─────────────────────────────────────
        let allocation = loop {
            match self.atlas.allocate(etagere::size2(width, height)) {
                Some(id) => break id,
                None => self.grow_atlas()?,
            }
        };
        let rectangle = self.atlas.get(allocation.id);

        // ── 5. Copy pixels into the RGBA atlas ───────────────────────
        let atlas_w = self.texture_size as usize;

        match img.content {
            SwashContent::SubpixelMask => {
                // Subpixel data is 4 bytes/pixel: R,G,B = coverage, A=0.
                // We store RGB coverage directly and set A = max(R,G,B).
                for (i, chunk) in img.data.chunks_exact(4).enumerate() {
                    let px = (rectangle.min.x as usize) + (i % width as usize);
                    let py = (rectangle.min.y as usize) + (i / width as usize);
                    let idx = (py * atlas_w + px) * 4;
                    if idx + 3 < self.texture_data.len() {
                        let r = chunk[0];
                        let g = chunk[1];
                        let b = chunk[2];
                        let a = r.max(g).max(b);
                        self.texture_data[idx] = r;
                        self.texture_data[idx + 1] = g;
                        self.texture_data[idx + 2] = b;
                        self.texture_data[idx + 3] = a;
                    }
                }
            }
            SwashContent::Mask => {
                // Grayscale alpha mask (1 byte/pixel).
                // Store coverage in all three RGB channels so both the
                // SUBPIXEL and MASK shader paths work correctly.
                // A channel is opaque since coverage is in RGB.
                for (i, &coverage) in img.data.iter().enumerate() {
                    let px = (rectangle.min.x as usize) + (i % width as usize);
                    let py = (rectangle.min.y as usize) + (i / width as usize);
                    let idx = (py * atlas_w + px) * 4;
                    if idx + 3 < self.texture_data.len() {
                        self.texture_data[idx] = coverage;
                        self.texture_data[idx + 1] = coverage;
                        self.texture_data[idx + 2] = coverage;
                        self.texture_data[idx + 3] = 255;
                    }
                }
            }
            SwashContent::Color => {
                // Color glyphs (emojis): premultiplied RGBA data, 4 bytes/pixel.
                for (i, chunk) in img.data.chunks_exact(4).enumerate() {
                    let px = (rectangle.min.x as usize) + (i % width as usize);
                    let py = (rectangle.min.y as usize) + (i / width as usize);
                    let idx = (py * atlas_w + px) * 4;
                    if idx + 3 < self.texture_data.len() {
                        self.texture_data[idx..idx + 4].copy_from_slice(chunk);
                    }
                }
            }
        }

        // Derive content type from the swash image.
        let content_type = match img.content {
            SwashContent::SubpixelMask => GlyphContentType::Subpixel,
            SwashContent::Mask => GlyphContentType::Mask,
            SwashContent::Color => GlyphContentType::Color,
        };

        self.glyph_cache.insert(
            key,
            GlyphEntry {
                atlas_rect: rectangle,
                bearing_x: img.placement.left as f32,
                bearing_y: img.placement.top as f32,
                advance,
                content_type,
            },
        );

        Ok(())
    }

    /// Rasterize a built-in block/shade character directly into the atlas
    /// without going through the system font.
    fn rasterize_builtin(&mut self, c: char) -> Result<()> {
        let key = (c, self.font_size.to_bits());
        let cw = self.cell_width.ceil() as u32;
        let ch = self.cell_height.ceil() as u32;

        let params = builtin::BuiltinParams { cell_width: cw, cell_height: ch };
        let glyph = builtin::render(c, &params).ok_or_else(|| {
            Error::Glyph(format!("builtin render failed for U+{:04X}", c as u32))
        })?;

        let width = glyph.width as i32;
        let height = glyph.height as i32;

        if width <= 0 || height <= 0 {
            self.glyph_cache.insert(
                key,
                GlyphEntry {
                    atlas_rect: etagere::Rectangle {
                        min: etagere::Point::new(0, 0),
                        max: etagere::Point::new(0, 0),
                    },
                    bearing_x: glyph.bearing_x,
                    bearing_y: glyph.bearing_y,
                    advance: glyph.advance,
                    content_type: glyph.content_type,
                },
            );
            return Ok(());
        }

        // Allocate in atlas.
        let allocation = loop {
            match self.atlas.allocate(etagere::size2(width, height)) {
                Some(id) => break id,
                None => self.grow_atlas()?,
            }
        };
        let rectangle = self.atlas.get(allocation.id);

        // Copy grayscale pixel data into the RGBA atlas.
        let atlas_w = self.texture_size as usize;
        for y in 0..height {
            for x in 0..width {
                let src_idx = (y * width + x) as usize;
                let dst_x = rectangle.min.x as usize + x as usize;
                let dst_y = rectangle.min.y as usize + y as usize;
                let dst_idx = (dst_y * atlas_w + dst_x) * 4;
                let coverage = glyph.data[src_idx];
                if dst_idx + 3 < self.texture_data.len() {
                    // Store coverage in all three RGB channels so both
                    // SUBPIXEL and MASK shader paths work.  A is opaque.
                    self.texture_data[dst_idx] = coverage;
                    self.texture_data[dst_idx + 1] = coverage;
                    self.texture_data[dst_idx + 2] = coverage;
                    self.texture_data[dst_idx + 3] = 255;
                }
            }
        }

        self.glyph_cache.insert(
            key,
            GlyphEntry {
                atlas_rect: rectangle,
                bearing_x: glyph.bearing_x,
                bearing_y: glyph.bearing_y,
                advance: glyph.advance,
                content_type: glyph.content_type,
            },
        );

        Ok(())
    }

    /// Rasterize a glyph via swash directly with `Format::Subpixel`,
    /// bypassing cosmic-text's `SwashCache` (which hardcodes `Format::Alpha`).
    fn rasterize_swash(
        &mut self,
        cache_key: &cosmic_text::CacheKey,
    ) -> Option<swash::scale::image::Image> {
        let font = self
            .font_system
            .get_font(cache_key.font_id, cache_key.font_weight)?;

        let hint = !cache_key.flags.contains(CacheKeyFlags::DISABLE_HINTING);

        let mut scaler = self
            .swash_ctx
            .builder(font.as_swash())
            .size(f32::from_bits(cache_key.font_size_bits))
            .hint(hint)
            .build();

        let offset = Vector::new(
            cache_key.x_bin.as_float(),
            cache_key.y_bin.as_float(),
        );

        let transform = if cache_key.flags.contains(CacheKeyFlags::FAKE_ITALIC) {
            Some(Transform::skew(
                Angle::from_degrees(14.0),
                Angle::from_degrees(0.0),
            ))
        } else {
            None
        };

        let format = match self.subpixel_layout {
            SubpixelLayout::Rgb => Format::Subpixel,
            SubpixelLayout::Bgr => Format::subpixel_bgra(),
        };

        log::debug!(
            "rasterize_swash: glyph_id={} format={:?} offset=({:.3},{:.3})",
            cache_key.glyph_id,
            format,
            offset.x,
            offset.y,
        );

        Render::new(&[
            // Color outline with the first palette (cosmic-text source order).
            Source::ColorOutline(0),
            // Color bitmap with best fit selection mode.
            Source::ColorBitmap(StrikeWith::BestFit),
            // Standard scalable outline.
            Source::Outline,
        ])
        .format(format)
        .offset(offset)
        .transform(transform)
        .render(&mut scaler, cache_key.glyph_id)
    }
}
