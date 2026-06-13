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
    /// Per-glyph scale factor applied at render time.
    ///
    /// `1.0` means "use the rasterizer's natural size".  Values > 1 enlarge a
    /// glyph (e.g. ASCII when `line_height` has been tightened to 1.0 so the
    /// glyph does not naturally fill the cell); values < 1 shrink a glyph
    /// (e.g. CJK whose `placement.top` exceeds the cell ascent, which would
    /// otherwise be clipped at the cell top).
    ///
    /// The renderer multiplies both the rendered quad's size and the bearing
    /// offsets by this value, and shrinks the sampled UV window so that the
    /// texture is sampled at its native resolution under `Nearest` filtering.
    pub scale: f32,
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
    /// Distance from the cell TOP to the baseline, in pixels.
    ///
    /// This is the authoritative baseline position produced by `cosmic-text`'s
    /// own layout pass for a full-height reference character (e.g. 'M').  The
    /// renderer positions glyphs as
    ///
    /// ```text
    /// glyph_top_y = row * cell_height + cell_ascent - glyph_bearing_y
    /// ```
    ///
    /// which is equivalent to `alacritty`'s
    /// `(line + 1) * ch - (ascent - descent)` and `wezterm`'s
    /// `cell_height + descender - bearing_y` (with `descender` negative).
    cell_ascent: f32,
    /// Distance from the baseline to the cell BOTTOM, in pixels.
    /// Mirrors `cell_ascent` and is exposed for callers that need to position
    /// decorations (underline / strikethrough) relative to the baseline.
    cell_descent: f32,
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
        // Tight line-height (1.0): the cell is now sized to exactly the
        // font's full body height (ascent + descent).  Combined with the
        // per-glyph `scale` factor computed in `rasterize_glyph` (which
        // enlarges ASCII to fill the cell, and shrinks CJK that would
        // otherwise extend above the cell top), this matches the visual
        // behaviour of alacritty / wezterm: the cursor block and the
        // character body occupy the same height with no bottom padding.
        let metrics = Metrics::new(font_size, font_size);

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
            cell_ascent: 0.0,
            cell_descent: 0.0,
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
    /// Width is the advance of a representative monospace glyph ('W'),
    /// rounded **up** to an integer pixel boundary so column-to-column
    /// background quads tile perfectly with no sub-pixel gaps or overlap.
    /// Height is the font's line height rounded **up** to an integer pixel
    /// boundary, matching the strategy used by `cosmic-term`
    /// (`(font_size * 1.4).ceil()`) and the spirit of `alacritty`'s
    /// `(line_height + offset_y).floor()`.  Integer cell dimensions are
    /// critical for the cell-background rasterizer: fractional sizes cause
    /// adjacent cells' SOLID quads to overlap by a sub-pixel in the 1-px
    /// grid, which shows up as a 1-px "fringe" between rows/columns of
    /// coloured cells.
    ///
    /// Side-effect: this also measures the cell's baseline offset by shaping
    /// a full-height reference character ('M') and reading `max_ascent` from
    /// the cosmic-text layout.  Callers must use this value to position
    /// glyphs, *not* the raw `line_height`.
    ///
    /// This method will rasterize 'W' and 'M' if they aren't cached yet.
    /// Once computed, the values are cached for subsequent calls.
    pub fn cell_size(&mut self) -> Result<(f32, f32)> {
        if self.cell_width > 0.0 && self.cell_height > 0.0 {
            return Ok((self.cell_width, self.cell_height));
        }
        // ── Order matters ─────────────────────────────────────────────
        // Measure baseline FIRST so `cell_ascent` and `cell_descent` are
        // valid by the time we rasterise 'W' below.  If we rasterised W
        // first (with both metrics still 0), `compute_glyph_scale` would
        // see `s_above = 0/placement.top = 0` and clamp W to scale 0.1,
        // making every W on screen collapse to a single invisible dot.
        self.measure_baseline()?;
        let (entry, _is_new) = self.ensure_glyph('W')?;
        // Integer cell width/height: avoid sub-pixel drift between
        // adjacent columns (width) and rows (height).
        self.cell_width = entry.advance.ceil();
        self.cell_height = self.metrics.line_height.ceil();
        // Authoritative baseline: ask cosmic-text where it would put the
        // baseline for a full-height glyph.  This is exactly the value
        // alacritty calls `ascent` and wezterm calls `ascender` — the
        // y-down distance from the cell top to the baseline.
        // (The actual `measure_baseline` call is now at the top of this
        // function so the 'W' rasterisation above sees valid metrics.)
        log::info!(
            "GlyphAtlas::cell_size: cw={:.2} ch={:.2} ascent={:.2} descent={:.2} \
             (line_height={:.2} font_size={:.2})",
            self.cell_width,
            self.cell_height,
            self.cell_ascent,
            self.cell_descent,
            self.metrics.line_height,
            self.font_size,
        );
        Ok((self.cell_width, self.cell_height))
    }

    /// Measure the cell's baseline offset (ascent) and descent via
    /// `cosmic-text`'s own layout pass.
    ///
    /// We use the string `"Mg"` (M for the full font ascent, g for the full
    /// font descent) so that `max_ascent` and `max_descent` both reflect the
    /// font's design metrics.  Using a single character like `'M'` would
    /// yield `max_descent = 0` (M has no descender), which then makes
    /// `compute_glyph_scale` compute `s_below = 0` and clamp every glyph
    /// to a tiny size — the bug visible in the screenshots where ASCII
    /// glyphs shrank to a single dot after the line-height tightening.
    ///
    /// alacritty and wezterm take the same dual-character approach (or
    /// pull the metrics directly from the font's OS/2 table).
    fn measure_baseline(&mut self) -> Result<()> {
        // Use a temporary buffer.  `line_height` here only affects inter-line
        // spacing inside cosmic-text; per-glyph metrics like `max_ascent` are
        // independent of it (they come from the shaped glyph's own font size).
        // We use a generous line height so cosmic-text doesn't truncate.
        let mut buf = Buffer::new(
            &mut self.font_system,
            Metrics::new(self.font_size, self.font_size * 2.0),
        );
        let attrs = Attrs::new().family(Family::Name(&self.font_family));
        // "Mg" — M contributes the full font ascent, g contributes the full
        // font descent (e.g. a typical 14/4 em-square).  cosmic-text's
        // layout pass exposes the *line-wide* max, so a single buffer line
        // of "Mg" gives us both numbers.
        buf.set_text("Mg", &attrs, Shaping::Basic, None);
        buf.shape_until_scroll(&mut self.font_system, true);

        let line = buf.lines[0]
            .layout_opt()
            .and_then(|l| l.first())
            .ok_or_else(|| Error::Glyph("measure_baseline: empty layout".into()))?;

        let max_ascent = line.max_ascent;
        let max_descent = line.max_descent;
        if max_ascent <= 0.0 {
            return Err(Error::Glyph(format!(
                "measure_baseline: got non-positive max_ascent (font_size={}, family={:?})",
                self.font_size, self.font_family,
            )));
        }
        self.cell_ascent = max_ascent;
        self.cell_descent = max_descent;
        Ok(())
    }

    /// Return the cached cell dimensions (must call `cell_size()` first).
    pub fn cell_dimensions(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    /// Return the cell's baseline offset: the y-down distance from the cell
    /// TOP to the baseline, in pixels.
    ///
    /// This is what callers should use to position glyphs vertically.  The
    /// standard formula is
    ///
    /// ```text
    /// glyph_top_y = row * cell_height + cell_baseline_offset() - glyph_bearing_y
    /// ```
    ///
    /// which places the baseline at `row * cell_height + cell_baseline_offset()`
    /// and the glyph top at `baseline - bearing_y`, exactly as in alacritty
    /// and wezterm.
    pub fn cell_baseline_offset(&self) -> f32 {
        self.cell_ascent
    }

    /// Return the cell's descent: the y-down distance from the baseline to
    /// the cell BOTTOM, in pixels.  Useful for placing decorations
    /// (underline, strikethrough) just below the baseline.
    pub fn cell_descent(&self) -> f32 {
        self.cell_descent
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

    /// Compute the per-glyph scale factor that makes the swash-rendered
    /// bitmap fit vertically inside the cell.
    ///
    /// In swash's coordinate convention the baseline is at `y = 0` (y-up):
    ///   bitmap top edge:    `y = placement.top`
    ///   bitmap bottom edge: `y = placement.top - placement.height`
    ///   pixels above baseline:  `above = placement.top`
    ///   pixels below baseline:  `below = placement.height - placement.top`
    ///
    /// The cell has:
    ///   pixels above baseline:  `self.cell_ascent`
    ///   pixels below baseline:  `self.cell_descent`
    ///
    /// To fit the bitmap in the cell we need
    ///   `above * s ≤ cell_ascent`  and  `below * s ≤ cell_descent`,
    /// i.e. `s ≤ min(cell_ascent / above, cell_descent / below)`.
    ///
    /// CJK glyphs (e.g. '版') typically have `placement.top` larger than
    /// the font's full em-square ascent — they are designed to fill the
    /// em-square, so their top extends above the ASCII cell ascent.  The
    /// resulting `s < 1` shrinks them so the top is no longer clipped.
    /// ASCII glyphs have `placement.top` close to the cell ascent and
    /// `s ≈ 1.0`, so they render at native size and fill the cell.
    ///
    /// The result is clamped to a sane range so a degenerate (zero-size)
    /// bitmap cannot blow up.
    fn compute_glyph_scale(&self, placement_top: f32, placement_height: f32) -> f32 {
        if placement_height <= 0.0 {
            return 1.0;
        }
        let above = placement_top.max(0.0);
        let below = (placement_height - placement_top).max(0.0);
        let s_above = if above > 0.0 {
            self.cell_ascent / above
        } else {
            f32::INFINITY
        };
        let s_below = if below > 0.0 {
            self.cell_descent / below
        } else {
            f32::INFINITY
        };
        s_above.min(s_below).clamp(0.1, 1.0)
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
                        scale: 1.0,
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
                        scale: 1.0,
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
                    scale: 1.0,
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

        // Per-glyph scale: shrink CJK that would otherwise extend above the
        // cell top, keep ASCII at native size.  See `compute_glyph_scale`
        // for the math.
        let mut scale = self.compute_glyph_scale(
            img.placement.top as f32,
            img.placement.height as f32,
        );
        let mut img = img;

        // ── Pre-scale in atlas (avoid per-instance aliasing) ─────────
        // If the computed scale is < 1.0, the glyph is "too tall" to fit
        // the cell.  The naive approach is to draw the full-size bitmap
        // into a smaller quad at render time, but with `Nearest` sampling
        // this produces visible stair-step jaggies (the CJK "毛刺" bug).
        //
        // Instead we re-rasterise the glyph at `font_size * scale` pixels
        // using the same swash path with a modified `cache_key.font_size_bits`,
        // and store THAT bitmap in the atlas.  The entry's `scale` is then
        // forced back to 1.0, so the renderer draws the pre-scaled bitmap
        // 1:1 against the screen — clean, no aliasing.
        //
        // `cache_key` is `Copy` (all fields are primitives in cosmic-text
        // 0.19), so we just rebind to a modified copy and re-invoke swash.
        if scale < 1.0 {
            let scaled_size = (self.font_size * scale).max(1.0);
            let mut scaled_key = physical_glyph.cache_key;
            scaled_key.font_size_bits = scaled_size.to_bits();
            if let Some(scaled_img) = self.rasterize_swash(&scaled_key) {
                let sw = scaled_img.placement.width;
                let sh = scaled_img.placement.height;
                if sw > 0 && sh > 0 {
                    img = scaled_img;
                    scale = 1.0;
                }
            }
        }

        self.glyph_cache.insert(
            key,
            GlyphEntry {
                atlas_rect: rectangle,
                bearing_x: img.placement.left as f32,
                bearing_y: img.placement.top as f32,
                advance,
                content_type,
                scale,
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
                    scale: 1.0,
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
                scale: 1.0,
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
