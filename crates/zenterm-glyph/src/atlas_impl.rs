//! GlyphAtlas core implementation — construction, shaping, rasterization,
//! and atlas management.
//!
//! This module contains the bulk of [`GlyphAtlas`] methods: font setup,
//! character shaping, run-based ligature shaping, per-glyph rasterization,
//! built-in block glyph rasterization, and accessors for cell metrics.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use cosmic_text::{
    Attrs, Buffer, CacheKeyFlags, Family, FeatureTag, FontFeatures, FontSystem, Metrics, Shaping,
    Wrap,
};
use etagere::AtlasAllocator;
use swash::scale::ScaleContext;
use swash::scale::image::Content as SwashContent;

use zenterm_core::{Error, HintingMode, RenderMode, Result, SubpixelLayout};

use crate::builtin;
use crate::{
    AtlasDirtyRegion, AtlasSlot, GlyphAtlas, GlyphCacheKey, GlyphContentType, GlyphEntry,
    GlyphStyle, MAX_CACHED_RUN_BYTES, MAX_NO_EFFECT_CACHE_ENTRIES, MAX_RUN_CACHE_ENTRIES,
    RunCacheKey, ShapedGlyph,
};

/// Convert an LCD coverage image to a grayscale mask before any geometric
/// scaling. RGB coverage is tied to physical display subpixels and becomes
/// invalid when the bitmap is sampled at a different size or position.
fn desubpixelize(img: &mut swash::scale::image::Image) {
    if img.content != SwashContent::SubpixelMask {
        return;
    }

    let mut mask = Vec::with_capacity(img.data.len() / 4);
    for chunk in img.data.as_chunks::<4>().0 {
        mask.push(chunk[0].max(chunk[1]).max(chunk[2]));
    }
    img.data = mask;
    img.content = SwashContent::Mask;
}

impl GlyphAtlas {
    /// Mark a rectangular atlas region for a partial GPU upload.
    pub(crate) fn mark_dirty_region(&mut self, atlas_index: usize, rectangle: etagere::Rectangle) {
        let width = rectangle.max.x.saturating_sub(rectangle.min.x) as u32;
        let height = rectangle.max.y.saturating_sub(rectangle.min.y) as u32;
        if width == 0 || height == 0 {
            return;
        }
        self.dirty_regions.push(AtlasDirtyRegion {
            atlas_index: atlas_index as u32,
            x: rectangle.min.x as u32,
            y: rectangle.min.y as u32,
            width,
            height,
        });
    }

    /// Take all pending pixel regions for the next GPU synchronization.
    pub fn take_dirty_regions(&mut self) -> Vec<AtlasDirtyRegion> {
        std::mem::take(&mut self.dirty_regions)
    }

    /// Create a new glyph atlas with the given font size (in pixels),
    /// font family, and LCD subpixel layout.
    ///
    /// `ligatures_enabled` controls whether OpenType ligature features
    /// are used during shaping.  See the [`ligatures_enabled`] field.
    ///
    /// The atlas starts with a single 512×512 slot and grows by pushing
    /// additional slots as needed (see [`grow_atlas`](Self::grow_atlas)).
    pub fn new(
        font_size: f32,
        font_family: Cow<'static, str>,
        pixels_per_point: f32,
        subpixel_layout: SubpixelLayout,
        ligatures_enabled: bool,
        hinting_mode: HintingMode,
        render_mode: RenderMode,
    ) -> Self {
        log::info!(
            "GlyphAtlas: font_size={font_size:.1} family={font_family:?} \
             pixels_per_point={pixels_per_point:.2} subpixel={subpixel_layout:?} \
             ligatures={ligatures_enabled} hinting={hinting_mode:?} render={render_mode:?}",
        );
        let font_system = FontSystem::new();
        // Initial line_height = font_size (1.0×).  This is intentionally
        // tight — the real line_height is computed in cell_size() after
        // measure_baseline() reads the font's actual ascent + descent.
        let metrics = Metrics::new(font_size, font_size);

        let initial_size: u32 = 512;
        let allocator =
            AtlasAllocator::new(etagere::size2(initial_size as i32, initial_size as i32));
        let texture_data = vec![0u8; (initial_size * initial_size * 4) as usize];
        let first_slot = AtlasSlot {
            allocator,
            texture_data,
            size: initial_size,
        };

        Self {
            font_system,
            slots: vec![first_slot],
            font_size,
            font_family,
            primary_font_id: None,
            pixels_per_point,
            subpixel_layout,
            metrics,
            glyph_cache: HashMap::new(),
            run_cache: HashMap::new(),
            no_effect_cache: HashSet::new(),
            image_cache: HashMap::new(),
            dirty_regions: Vec::new(),
            swash_ctx: ScaleContext::new(),
            cell_width: 0.0,
            cell_height: 0.0,
            underline_thickness_px: 0.0,
            cell_ascent: 0.0,
            cell_descent: 0.0,
            cap_height: 0.0,
            ligatures_enabled,
            hinting_mode,
            render_mode,
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
        self.ensure_glyph_with_style(c, GlyphStyle::default())
    }

    /// Ensure a character with the requested terminal style is rasterised.
    ///
    /// The no-style [`Self::ensure_glyph`] wrapper remains the regular-font
    /// fast path used by atlas seeding and metric probes.
    pub fn ensure_glyph_with_style(
        &mut self,
        c: char,
        style: GlyphStyle,
    ) -> Result<(&GlyphEntry, bool)> {
        let key = GlyphCacheKey::new(c, self.font_size.to_bits(), style);
        let is_new = !self.glyph_cache.contains_key(&key);

        if is_new {
            self.rasterize_glyph(c, style)?;
        }

        Ok((self.glyph_cache.get(&key).unwrap(), is_new))
    }

    /// Shape text with basic features (no ligatures) and return glyph IDs.
    ///
    /// This is a lightweight shaping — no layout, no rasterization, no atlas
    /// allocation.  Used as a baseline to detect whether ligature features
    /// actually changed any glyphs.
    fn baseline_glyph_ids(&mut self, text: &str, style: GlyphStyle) -> Vec<u16> {
        let mut buf = Buffer::new(&mut self.font_system, self.metrics);
        buf.set_size(Some(self.font_size), None);
        buf.set_wrap(Wrap::None);
        let attrs = Attrs::new()
            .family(Family::Name(&self.font_family))
            .weight(if style.bold {
                cosmic_text::Weight::BOLD
            } else {
                cosmic_text::Weight::NORMAL
            })
            .style(if style.italic {
                cosmic_text::Style::Italic
            } else {
                cosmic_text::Style::Normal
            });
        buf.set_text(text, &attrs, Shaping::Basic, None);
        buf.shape_until_scroll(&mut self.font_system, true);
        buf.lines[0]
            .shape_opt()
            .map(|sl| {
                sl.spans
                    .iter()
                    .flat_map(|s| s.words.iter())
                    .flat_map(|w| w.glyphs.iter())
                    .map(|g| g.glyph_id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Shape and rasterise a run of consecutive characters.
    ///
    /// A "run" is a group of characters with the same visual style
    /// (same font, same bold/italic state) that can be shaped together
    /// as a single string.  When ligatures are enabled, `cosmic-text`'s
    /// `Shaping::Advanced` consults the font's OpenType `liga`/`clig`
    /// tables and may substitute multi-character sequences with a
    /// single glyph (e.g. `->` → one arrow glyph).
    ///
    /// # Behaviour
    ///
    /// If [`Self::ligatures_enabled`] is `true`, the full `text` string
    /// is shaped with `Shaping::Advanced` so the font's OpenType ligature
    /// rules apply.  When a ligature substitution occurs (a single
    /// `LayoutGlyph` covering `end - start > 1` source characters), one
    /// [`ShapedGlyph`] with `num_cells > 1` is produced.  The renderer
    /// splits the bitmap into per-cell strips (see Phase D of
    /// `LIGATURE.md`).
    ///
    /// When `ligatures_enabled` is `false`, shaping still goes through
    /// `cosmic-text`'s `Buffer` but with `Shaping::Basic`.  Each source
    /// character produces exactly one `ShapedGlyph` with `num_cells = 1`,
    /// identical to the old per-char path.
    ///
    /// # Returns
    ///
    /// A tuple of:
    /// - A vector of [`ShapedGlyph`] entries, one per glyph in the shaped
    ///   output.  When no ligatures occur this equals the source character
    ///   count.
    /// - A `bool` indicating whether the atlas was actually modified
    ///   (`true` = cache miss, new glyphs rasterised; `false` = cache hit,
    ///   no atlas change).  Callers should use this to decide whether to
    ///   upload the atlas texture to the GPU.
    /// - A `bool` indicating whether the shaping produced a meaningful
    ///   change from the per-char baseline (`true` = ligature or contextual
    ///   alternate occurred; `false` = output is identical to per-char).
    ///   When `false`, callers may discard the result and use the per-char
    ///   path instead, avoiding double atlas allocation.
    ///
    /// # Errors
    ///
    /// Atlas allocation failures propagate up.
    pub fn shape_and_rasterize_run(
        &mut self,
        text: &str,
    ) -> Result<(Vec<ShapedGlyph>, bool, bool)> {
        self.shape_and_rasterize_run_with_style(text, GlyphStyle::default())
    }

    /// Shape and rasterise a run using the requested terminal style.
    pub fn shape_and_rasterize_run_with_style(
        &mut self,
        text: &str,
        style: GlyphStyle,
    ) -> Result<(Vec<ShapedGlyph>, bool, bool)> {
        // ── Cache lookup ──────────────────────────────────────────────
        let key = RunCacheKey {
            text: text.to_string(),
            font_size_bits: self.font_size.to_bits(),
            style,
        };

        // Check the run cache first.
        if let Some(cached) = self.run_cache.get(&key) {
            return Ok((cached.clone(), false, true));
        }

        // Check the negative cache: if this run was previously shaped and
        // found to have no substitution, skip shaping entirely.
        if self.no_effect_cache.contains(&key) {
            return Ok((Vec::new(), false, false));
        }

        // ── Shape the whole run via cosmic-text Buffer ───────────────
        let shaping = if self.ligatures_enabled {
            Shaping::Advanced
        } else {
            Shaping::Basic
        };

        let mut buf = Buffer::new(&mut self.font_system, self.metrics);
        buf.set_size(Some(self.font_size), None);
        buf.set_wrap(Wrap::None);
        let mut font_features = FontFeatures::new();
        if self.ligatures_enabled {
            font_features.enable(FeatureTag::STANDARD_LIGATURES);
            font_features.enable(FeatureTag::CONTEXTUAL_LIGATURES);
            font_features.enable(FeatureTag::CONTEXTUAL_ALTERNATES);
            font_features.enable(FeatureTag::DISCRETIONARY_LIGATURES);
            font_features.enable(FeatureTag::KERNING);
        }
        let attrs = Attrs::new()
            .family(Family::Name(&self.font_family))
            .weight(if style.bold {
                cosmic_text::Weight::BOLD
            } else {
                cosmic_text::Weight::NORMAL
            })
            .style(if style.italic {
                cosmic_text::Style::Italic
            } else {
                cosmic_text::Style::Normal
            })
            .font_features(font_features);
        log::info!(
            "[lig-diag] attrs features={} tags={:?}",
            attrs.font_features.features.len(),
            attrs
                .font_features
                .features
                .iter()
                .map(|f| std::str::from_utf8(f.tag.as_bytes()).unwrap_or("?"))
                .collect::<Vec<_>>(),
        );
        buf.set_text(text, &attrs, shaping, None);
        buf.shape_until_scroll(&mut self.font_system, true);

        // ── Diagnostic: inspect ShapeLine words/glyphs ────────────
        // This reveals whether harfbuzz produced ligature substitutions
        // at the shaping level (before layout).
        if let Some(shape_line) = buf.lines[0].shape_opt() {
            for (si, span) in shape_line.spans.iter().enumerate() {
                log::info!(
                    "[lig-diag]   span[{si}] words={} level={:?}",
                    span.words.len(),
                    span.level,
                );
                for (wi, word) in span.words.iter().enumerate() {
                    let word_start = word.glyphs.first().map(|g| g.start).unwrap_or(0);
                    let word_end = word.glyphs.last().map(|g| g.end).unwrap_or(0);
                    let word_text = &text[word_start..word_end];
                    let glyph_info: Vec<String> = word
                        .glyphs
                        .iter()
                        .map(|g| format!("g_id={} {}..{}", g.glyph_id, g.start, g.end))
                        .collect();
                    log::info!(
                        "[lig-diag]     word[{wi}] blank={} text={word_text:?} glyphs={} {:?}",
                        word.blank,
                        word.glyphs.len(),
                        glyph_info,
                    );
                }
            }
        } else {
            log::warn!("[lig-diag]   shape_opt() is NONE after shaping");
        }

        let lines = buf.lines.len();
        let all_glyphs: Vec<&cosmic_text::LayoutGlyph> = if lines > 0 {
            match buf.lines[0].layout_opt() {
                Some(runs) => runs.iter().flat_map(|run| &run.glyphs).collect(),
                None => {
                    log::warn!(
                        "[lig-diag] shape_and_rasterize_run: layout_opt() is NONE \
                         for text={text:?} (lines={lines} shaping={shaping:?}). \
                         shape_until_scroll may not have produced layout.",
                    );
                    Vec::new()
                }
            }
        } else {
            log::warn!(
                "shape_and_rasterize_run: buf.lines is EMPTY after shaping {text:?} \
                 (size={:?} shaping={shaping:?})",
                self.font_size,
            );
            Vec::new()
        };

        log::info!(
            "[lig-diag] shape_and_rasterize_run: text={text:?} lines={lines} \
             total_glyphs={} shaping={shaping:?} ligatures={} font={:?}",
            all_glyphs.len(),
            self.ligatures_enabled,
            self.font_family,
        );

        // ── Detect whether substitution actually occurred ──────────
        // If all glyphs are single-cell and the count matches, the
        // result may still differ from per-char (contextual alternates).
        // Use a lightweight baseline shaping to compare glyph IDs.
        let all_single_cell = all_glyphs.iter().all(|g| (g.end - g.start) == 1);
        let count_matches = all_glyphs.len() == text.chars().count();
        let had_effect = if all_single_cell && count_matches && self.ligatures_enabled {
            // Ambiguous: could be contextual alternates or no substitution.
            // Compare glyph IDs against the per-char baseline.
            let baseline_ids = self.baseline_glyph_ids(text, style);
            let shaped_ids: Vec<u16> = all_glyphs.iter().map(|g| g.glyph_id).collect();
            shaped_ids != baseline_ids
        } else {
            // Either there's a multi-cell glyph (true ligature) or glyph
            // count differs from char count (ligature merged or expanded).
            // Either way, something changed, so it's an effective ligature.
            // Also, if ligatures are disabled globally, there's no effect.
            self.ligatures_enabled
        };

        // ── Rasterise each layout glyph into the atlas ──────────────
        let mut shaped: Vec<ShapedGlyph> = Vec::with_capacity(all_glyphs.len());
        let mut run_x_offset: f32 = 0.0;

        for g in all_glyphs {
            let num_cells = g.end - g.start;
            let advance = g.w;

            log::debug!(
                "  glyph: start={} end={} num_cells={} advance={:.1}",
                g.start,
                g.end,
                num_cells,
                advance,
            );

            // Physical glyph for swash rasterization.
            let mut phys = g.physical((0.0, 0.0), 1.0);

            // Disable hinting at high DPI (wezterm strategy).
            if self.pixels_per_point > 1.04 {
                phys.cache_key.flags.insert(CacheKeyFlags::DISABLE_HINTING);
            }

            // Rasterize via swash and pack into atlas.
            let entry = self.rasterize_swash_entry(&phys.cache_key, advance)?;

            // ── Populate glyph_cache for single-cell glyphs ─────────
            // When a glyph covers exactly one cell (no ligature
            // substitution), write its entry into `glyph_cache` so that
            // the per-char `ensure_glyph` path finds it cached and skips
            // re-shaping + re-rasterizing.  This eliminates the double
            // atlas allocation that would otherwise occur when the
            // ligature branch falls through to per-char rendering.
            if num_cells == 1 {
                for ci in g.start..g.end {
                    if let Some(c) = text[ci..].chars().next() {
                        let char_key = GlyphCacheKey::new(c, self.font_size.to_bits(), style);
                        self.glyph_cache.entry(char_key).or_insert(entry.clone());
                    }
                }
            }

            shaped.push(ShapedGlyph {
                char_range: g.start..g.end,
                num_cells,
                run_x_offset,
                entry,
            });

            run_x_offset += advance;
        }

        if had_effect {
            // Only cache runs that actually produced a ligature/substitution.
            self.cache_run(key, shaped.clone());
        } else {
            // Cache the "no effect" result so future calls skip shaping.
            self.cache_no_effect(key);
        }

        Ok((shaped, true, had_effect))
    }

    /// Cache a shaped run unless it is too large or the bounded cache is full.
    /// Eviction only affects the shaping fast path; a later cache miss
    /// produces the same glyph output by running the normal shaping path.
    fn cache_run(&mut self, key: RunCacheKey, shaped: Vec<ShapedGlyph>) {
        if key.text.len() > MAX_CACHED_RUN_BYTES {
            return;
        }
        if self.run_cache.len() >= MAX_RUN_CACHE_ENTRIES
            && let Some(old_key) = self.run_cache.keys().next().cloned()
        {
            self.run_cache.remove(&old_key);
        }
        self.run_cache.insert(key, shaped);
    }

    /// Cache a negative shaping result with the same bounded policy.
    fn cache_no_effect(&mut self, key: RunCacheKey) {
        if key.text.len() > MAX_CACHED_RUN_BYTES {
            return;
        }
        if self.no_effect_cache.len() >= MAX_NO_EFFECT_CACHE_ENTRIES
            && let Some(old_key) = self.no_effect_cache.iter().next().cloned()
        {
            self.no_effect_cache.remove(&old_key);
        }
        self.no_effect_cache.insert(key);
    }

    /// Rasterize a glyph at a cell-safe size when the first pass shows that a
    /// fallback face would overflow the primary-font cell. Re-rasterizing is
    /// preferable to shrinking a large bitmap with the GPU's nearest sampler;
    /// the residual `GlyphEntry::scale` remains as a safety net for hinting
    /// and integer-rounding differences at the adjusted size.
    fn rasterize_cell_glyph(
        &mut self,
        cache_key: &cosmic_text::CacheKey,
    ) -> Option<swash::scale::image::Image> {
        let img = self.rasterize_swash(cache_key)?;
        let scale =
            self.glyph_scale_to_cell(cache_key.font_id, img.placement.top, img.placement.height);
        if scale >= 1.0 {
            return Some(img);
        }

        let source_size = f32::from_bits(cache_key.font_size_bits);
        let adjusted_size = source_size * scale;
        if adjusted_size <= 0.0 || adjusted_size >= source_size {
            return Some(img);
        }

        let mut adjusted_key = *cache_key;
        adjusted_key.font_size_bits = adjusted_size.to_bits();
        match self.rasterize_swash(&adjusted_key) {
            Some(adjusted_img) => {
                log::debug!(
                    "fallback glyph rerasterized for cell: glyph_id={} size={source_size:.2} \
                     adjusted_size={adjusted_size:.2} initial_scale={scale:.3}",
                    cache_key.glyph_id,
                );
                Some(adjusted_img)
            }
            None => Some(img),
        }
    }

    /// Rasterize a physical glyph (identified by [`cosmic_text::CacheKey`])
    /// into the atlas and return a [`GlyphEntry`] describing its position
    /// and metrics.
    ///
    /// This is the same rasterization + atlas-packing logic that
    /// [`rasterize_glyph`](Self::rasterize_glyph) uses for single
    /// characters, factored out so that the run-based shaping path
    /// (`shape_and_rasterize_run`) can rasterize each `LayoutGlyph`
    /// without re-shaping.
    fn rasterize_swash_entry(
        &mut self,
        cache_key: &cosmic_text::CacheKey,
        advance: f32,
    ) -> Result<GlyphEntry> {
        let mut img = match self.rasterize_cell_glyph(cache_key) {
            Some(img) => img,
            None => {
                log::debug!(
                    "rasterize_swash_entry: no image for glyph_id={} font_id={:?}",
                    cache_key.glyph_id,
                    cache_key.font_id,
                );
                return Ok(GlyphEntry {
                    atlas_index: 0,
                    atlas_rect: etagere::Rectangle {
                        min: etagere::Point::new(0, 0),
                        max: etagere::Point::new(0, 0),
                    },
                    bearing_x: 0.0,
                    bearing_y: 0.0,
                    advance,
                    content_type: GlyphContentType::Subpixel,
                    scale: 1.0,
                });
            }
        };

        let width = img.placement.width as i32;
        let height = img.placement.height as i32;

        if width <= 0 || height <= 0 {
            return Ok(GlyphEntry {
                atlas_index: 0,
                atlas_rect: etagere::Rectangle {
                    min: etagere::Point::new(0, 0),
                    max: etagere::Point::new(0, 0),
                },
                bearing_x: img.placement.left as f32,
                bearing_y: img.placement.top as f32,
                advance,
                content_type: GlyphContentType::Subpixel,
                scale: 1.0,
            });
        }

        // Allocate in the current (last) atlas slot.
        // If full, push a new larger slot — existing slots are unchanged.
        // slot_idx is recomputed each iteration so that after grow_atlas
        // pushes a new slot we retry the fresh slot, not the old full one.
        let allocation = loop {
            let idx = self.slots.len() - 1;
            match self.slots[idx]
                .allocator
                .allocate(etagere::size2(width, height))
            {
                Some(id) => break (id, idx),
                None => self.grow_atlas()?,
            }
        };
        let (allocation, slot_idx) = allocation;
        let rectangle = self.slots[slot_idx].allocator.get(allocation.id);

        let scale =
            self.glyph_scale_to_cell(cache_key.font_id, img.placement.top, img.placement.height);
        if scale < 1.0 && img.content == SwashContent::SubpixelMask {
            desubpixelize(&mut img);
            log::debug!(
                "fallback glyph switched to grayscale: glyph_id={} scale={scale:.3}",
                cache_key.glyph_id,
            );
        }

        // Copy pixels into the slot's RGBA texture data.
        let atlas_w = self.slots[slot_idx].size as usize;
        let texture_data = &mut self.slots[slot_idx].texture_data;

        match img.content {
            SwashContent::SubpixelMask => {
                // Subpixel data is 4 bytes/pixel: R,G,B = coverage, A=0.
                for (i, chunk) in img.data.as_chunks::<4>().0.iter().enumerate() {
                    let px = (rectangle.min.x as usize) + (i % width as usize);
                    let py = (rectangle.min.y as usize) + (i / width as usize);
                    let idx = (py * atlas_w + px) * 4;
                    if idx + 3 < texture_data.len() {
                        let r = chunk[0];
                        let g = chunk[1];
                        let b = chunk[2];
                        let a = r.max(g).max(b);
                        texture_data[idx] = r;
                        texture_data[idx + 1] = g;
                        texture_data[idx + 2] = b;
                        texture_data[idx + 3] = a;
                    }
                }
            }
            SwashContent::Mask => {
                // Grayscale alpha mask (1 byte/pixel).
                for (i, &coverage) in img.data.iter().enumerate() {
                    let px = (rectangle.min.x as usize) + (i % width as usize);
                    let py = (rectangle.min.y as usize) + (i / width as usize);
                    let idx = (py * atlas_w + px) * 4;
                    if idx + 3 < texture_data.len() {
                        texture_data[idx] = coverage;
                        texture_data[idx + 1] = coverage;
                        texture_data[idx + 2] = coverage;
                        texture_data[idx + 3] = 255;
                    }
                }
            }
            SwashContent::Color => {
                // Color glyphs (emojis): premultiplied RGBA, 4 bytes/pixel.
                for (i, chunk) in img.data.as_chunks::<4>().0.iter().enumerate() {
                    let px = (rectangle.min.x as usize) + (i % width as usize);
                    let py = (rectangle.min.y as usize) + (i / width as usize);
                    let idx = (py * atlas_w + px) * 4;
                    if idx + 3 < texture_data.len() {
                        texture_data[idx..idx + 4].copy_from_slice(chunk);
                    }
                }
            }
        }

        self.mark_dirty_region(slot_idx, rectangle);

        let content_type = match img.content {
            SwashContent::SubpixelMask => GlyphContentType::Subpixel,
            SwashContent::Mask => GlyphContentType::Mask,
            SwashContent::Color => GlyphContentType::Color,
        };

        if scale < 1.0 {
            log::debug!(
                "fallback glyph constrained: glyph_id={} top={} height={} scale={scale:.3}",
                cache_key.glyph_id,
                img.placement.top,
                img.placement.height,
            );
        }

        Ok(GlyphEntry {
            atlas_index: slot_idx as u32,
            atlas_rect: rectangle,
            bearing_x: img.placement.left as f32,
            bearing_y: img.placement.top as f32,
            advance,
            content_type,
            scale,
        })
    }

    /// Rasterize a single character using swash, pack it into the atlas, and
    /// cache it. The raster format is selected from the resolved face: the
    /// primary face may use LCD subpixel coverage, while fallback faces use a
    /// grayscale mask.
    ///
    /// Unicode block/shade characters (U+2500–U+259F) are intercepted and
    /// rendered via the built-in software rasterizer ([`builtin`] module)
    /// instead of the system font, giving pixel-perfect solid blocks.
    fn rasterize_glyph(&mut self, c: char, style: GlyphStyle) -> Result<()> {
        let key = GlyphCacheKey::new(c, self.font_size.to_bits(), style);

        // ── 0. Built-in block glyphs ──────────────────────────────
        // Intercept before cosmic-text so we get pixel-perfect solid
        // rectangles instead of font-provided dither patterns.
        if builtin::is_builtin(c) && self.cell_width > 0.0 && self.cell_height > 0.0 {
            return self.rasterize_builtin(c, style);
        }

        // ── 1. Shape the character (cosmic-text Buffer) ───────────────
        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        buffer.set_size(Some(self.font_size), None);
        let shaping = if c.is_ascii_graphic() || c == ' ' {
            Shaping::Basic
        } else {
            Shaping::Advanced
        };
        let attrs = Attrs::new()
            .family(Family::Name(&self.font_family))
            .weight(if style.bold {
                cosmic_text::Weight::BOLD
            } else {
                cosmic_text::Weight::NORMAL
            })
            .style(if style.italic {
                cosmic_text::Style::Italic
            } else {
                cosmic_text::Style::Normal
            });
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
                        atlas_index: 0,
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

        // ── 3. Rasterize via swash with the face-appropriate format ────
        let img = self.rasterize_cell_glyph(&physical_glyph.cache_key);

        let mut img = match img {
            Some(img) => img,
            None => {
                self.glyph_cache.insert(
                    key,
                    GlyphEntry {
                        atlas_index: 0,
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
                    atlas_index: 0,
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

        // ── 4. Allocate in current atlas slot ──────────────────────────
        // slot_idx is recomputed each iteration so that after grow_atlas
        // pushes a new slot we retry the fresh slot, not the old full one.
        let allocation = loop {
            let idx = self.slots.len() - 1;
            match self.slots[idx]
                .allocator
                .allocate(etagere::size2(width, height))
            {
                Some(id) => break (id, idx),
                None => self.grow_atlas()?,
            }
        };
        let (allocation, slot_idx) = allocation;
        let rectangle = self.slots[slot_idx].allocator.get(allocation.id);

        let scale = self.glyph_scale_to_cell(
            physical_glyph.cache_key.font_id,
            img.placement.top,
            img.placement.height,
        );
        if scale < 1.0 && img.content == SwashContent::SubpixelMask {
            desubpixelize(&mut img);
            log::debug!("fallback glyph switched to grayscale: char={c:?} scale={scale:.3}",);
        }

        // ── 5. Copy pixels into the slot's RGBA texture data ───────────
        let atlas_w = self.slots[slot_idx].size as usize;
        let texture_data = &mut self.slots[slot_idx].texture_data;

        match img.content {
            SwashContent::SubpixelMask => {
                // Subpixel data is 4 bytes/pixel: R,G,B = coverage, A=0.
                // We store RGB coverage directly and set A = max(R,G,B).
                for (i, chunk) in img.data.as_chunks::<4>().0.iter().enumerate() {
                    let px = (rectangle.min.x as usize) + (i % width as usize);
                    let py = (rectangle.min.y as usize) + (i / width as usize);
                    let idx = (py * atlas_w + px) * 4;
                    if idx + 3 < texture_data.len() {
                        let r = chunk[0];
                        let g = chunk[1];
                        let b = chunk[2];
                        let a = r.max(g).max(b);
                        texture_data[idx] = r;
                        texture_data[idx + 1] = g;
                        texture_data[idx + 2] = b;
                        texture_data[idx + 3] = a;
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
                    if idx + 3 < texture_data.len() {
                        texture_data[idx] = coverage;
                        texture_data[idx + 1] = coverage;
                        texture_data[idx + 2] = coverage;
                        texture_data[idx + 3] = 255;
                    }
                }
            }
            SwashContent::Color => {
                // Color glyphs (emojis): premultiplied RGBA data, 4 bytes/pixel.
                for (i, chunk) in img.data.as_chunks::<4>().0.iter().enumerate() {
                    let px = (rectangle.min.x as usize) + (i % width as usize);
                    let py = (rectangle.min.y as usize) + (i / width as usize);
                    let idx = (py * atlas_w + px) * 4;
                    if idx + 3 < texture_data.len() {
                        texture_data[idx..idx + 4].copy_from_slice(chunk);
                    }
                }
            }
        }

        self.mark_dirty_region(slot_idx, rectangle);

        // Derive content type from the swash image.
        let content_type = match img.content {
            SwashContent::SubpixelMask => GlyphContentType::Subpixel,
            SwashContent::Mask => GlyphContentType::Mask,
            SwashContent::Color => GlyphContentType::Color,
        };

        if scale < 1.0 {
            log::debug!(
                "fallback glyph constrained: char={c:?} glyph_id={} top={} height={} scale={scale:.3}",
                physical_glyph.cache_key.glyph_id,
                img.placement.top,
                img.placement.height,
            );
        }

        self.glyph_cache.insert(
            key,
            GlyphEntry {
                atlas_index: slot_idx as u32,
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
    fn rasterize_builtin(&mut self, c: char, style: GlyphStyle) -> Result<()> {
        let key = GlyphCacheKey::new(c, self.font_size.to_bits(), style);
        let cw = self.cell_width.ceil() as u32;
        let ch = self.cell_height.ceil() as u32;

        let params = builtin::BuiltinParams {
            cell_width: cw,
            cell_height: ch,
            cell_ascent: self.cell_ascent,
            underline_thickness: self.underline_thickness_px,
        };
        let glyph = builtin::render(c, &params)
            .ok_or_else(|| Error::Glyph(format!("builtin render failed for U+{:04X}", c as u32)))?;

        let width = glyph.width as i32;
        let height = glyph.height as i32;

        if width <= 0 || height <= 0 {
            self.glyph_cache.insert(
                key,
                GlyphEntry {
                    atlas_index: 0,
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

        // Allocate in the current atlas slot.
        // slot_idx is recomputed each iteration so that after grow_atlas
        // pushes a new slot we retry the fresh slot, not the old full one.
        let allocation = loop {
            let idx = self.slots.len() - 1;
            match self.slots[idx]
                .allocator
                .allocate(etagere::size2(width, height))
            {
                Some(id) => break (id, idx),
                None => self.grow_atlas()?,
            }
        };
        let (allocation, slot_idx) = allocation;
        let rectangle = self.slots[slot_idx].allocator.get(allocation.id);

        // Copy grayscale pixel data into the slot's RGBA texture.
        let atlas_w = self.slots[slot_idx].size as usize;
        let texture_data = &mut self.slots[slot_idx].texture_data;
        for y in 0..height {
            for x in 0..width {
                let src_idx = (y * width + x) as usize;
                let dst_x = rectangle.min.x as usize + x as usize;
                let dst_y = rectangle.min.y as usize + y as usize;
                let dst_idx = (dst_y * atlas_w + dst_x) * 4;
                let coverage = glyph.data[src_idx];
                if dst_idx + 3 < texture_data.len() {
                    // Store coverage in all three RGB channels so both
                    // SUBPIXEL and MASK shader paths work.  A is opaque.
                    texture_data[dst_idx] = coverage;
                    texture_data[dst_idx + 1] = coverage;
                    texture_data[dst_idx + 2] = coverage;
                    texture_data[dst_idx + 3] = 255;
                }
            }
        }

        self.mark_dirty_region(slot_idx, rectangle);

        self.glyph_cache.insert(
            key,
            GlyphEntry {
                atlas_index: slot_idx as u32,
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
}
