//! Glyph atlas — rasterizes characters with `cosmic-text` (shaping) + `swash`
//! (subpixel rasterization) and packs them into a GPU-friendly texture atlas.
//!
//! Unlike cosmic-text's built-in `SwashCache` (which hardcodes `Format::Alpha`),
//! we call swash directly with `Format::Subpixel` to get per-channel RGB coverage
//! values for LCD subpixel rendering.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use cosmic_text::{FontSystem, Metrics};
use etagere::AtlasAllocator;
use swash::scale::ScaleContext;

use zenterm_core::{HintingMode, RenderMode, SubpixelLayout};

pub mod allocate;
pub mod atlas_impl;
pub mod builtin;
pub mod rasterize;

// All `impl GlyphAtlas` blocks live in sub-modules:
// - `atlas_impl` — core methods (new, shaping, rasterization, accessors)
// - `allocate`   — texture growth (`grow_atlas`)
// - `rasterize`  — low-level swash rasterization (`rasterize_swash`)

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
pub struct GlyphEntry {    /// Allocated rectangle within the atlas texture (in pixels).
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

/// Cache key for a multi-character run that may produce ligature glyphs.
///
/// When ligature shaping is implemented, consecutive same-style characters
/// are shaped together.  The cache stores the resulting [`GlyphEntry`] list
/// so the same run is only rasterised once.
///
/// # Key fields
///
/// * `text` — the raw character sequence (e.g. `"->"`, `"!="`).
/// * `font_size_bits` — the font size in `f32::to_bits()` form, so that
///   resizing the font invalidates the cache.
///
/// # Future extension
///
/// When bold/italic style is plumbed through shaping, this key will be
/// extended with style flags so that `->` in bold gets a different (or
/// same) cache entry depending on how the font handles bold ligatures.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct RunCacheKey {
    /// The raw text spanned by the run.
    pub text: String,
    /// Font size in `f32::to_bits()` form.
    pub font_size_bits: u32,
    // FUTURE: add bold/italic style flags here when ligature-aware shaping
    // is implemented.  Different OpenType features may be active for
    // different font weights.
}

/// One glyph in a shaped run, after rasterisation and atlas packing.
///
/// When no ligature substitution occurs, a run of N characters produces N
/// [`ShapedGlyph`] entries, each covering exactly one cell.
///
/// When a ligature *does* occur (e.g. `->` becomes one glyph), a single
/// [`ShapedGlyph`] entry covers multiple source characters / cells.  The
/// renderer splits the ligature bitmap into per-cell strips by adjusting
/// UV coordinates.
///
/// # Current (preparatory) state
///
/// Without actual ligature shaping, each `ShapedGlyph` always covers
/// exactly one source character.  The `char_range` and `num_cells` fields
/// are always `0..1` / `1` respectively.
#[derive(Debug, Clone)]
pub struct ShapedGlyph {
    /// Range in the source text that this glyph originated from.
    ///
    /// * No ligature: `char_range = 0..1` for each of N glyphs.
    /// * Ligature:    `char_range = 0..N` for the single replacement glyph.
    pub char_range: std::ops::Range<usize>,

    /// Number of terminal cells this glyph covers.
    ///
    /// Equal to `char_range.end - char_range.start` for monospace fonts.
    pub num_cells: usize,

    /// X-offset of this glyph relative to the run origin, in pixels.
    ///
    /// For the first glyph in a run this is `0`.  For subsequent glyphs
    /// it is the sum of previous glyphs' advances.  Ligature glyphs
    /// always have `run_x_offset = 0`.
    pub run_x_offset: f32,

    /// The underlying atlas entry (position, metrics, content type).
    ///
    /// This is a full copy so that the renderer can build `CellInstance`
    /// data without additional atlas lookups.
    pub entry: GlyphEntry,
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
    /// Cache for multi-character runs (ligature support).
    ///
    /// Keyed by `(text, font_size_bits)`.  Each entry holds the rasterised
    /// glyphs produced by shaping the run as a whole.
    ///
    /// Currently unused — populated only when ligature shaping is enabled.
    /// See [`Self::shape_and_rasterize_run`].
    run_cache: HashMap<RunCacheKey, Vec<ShapedGlyph>>,
    /// Negative cache: runs that were shaped with ligature features but
    /// produced no actual substitution (identical to per-char baseline).
    /// Subsequent calls skip shaping entirely and fall through to per-char.
    no_effect_cache: HashSet<RunCacheKey>,
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
    /// Distance from the baseline to the top of a capital letter, in pixels.
    ///
    /// This is the *typographic* cap height (≈ 0.7 em for Menlo), which is
    /// smaller than `cell_ascent` by the "above-cap-height buffer" — the
    /// vertical room reserved for diacritics like `Á` / `Ž` that extend
    /// above capital letters.  Alacritty's block cursor stops at the cap
    /// height (not the cell top) and includes the full descent below, so
    /// the cursor visually matches the character body instead of overshooting
    /// into the "above-cap" buffer.  Cached by [`cap_height()`](Self::cap_height).
    cap_height: f32,

    /// Whether OpenType ligature features (`liga`/`clig`) are enabled.
    ///
    /// When `true`, calls to [`shape_and_rasterize_run`](Self::shape_and_rasterize_run)
    /// use `Shaping::Advanced` so the font's ligature substitution rules
    /// can replace multi-character sequences with a single glyph.
    ///
    /// When `false`, all shaping uses `Shaping::Basic` (fast path, no
    /// ligatures, no font fallback).
    ///
    /// Set from [`zenterm_config::font::FontConfig::ligatures`].
    ///
    /// This field is read by [`shape_and_rasterize_run`](Self::shape_and_rasterize_run)
    /// to decide whether to use `Shaping::Advanced` (ligatures on) or
    /// `Shaping::Basic` (ligatures off).
    pub ligatures_enabled: bool,

    /// Hinting mode for font rasterization.
    pub hinting_mode: HintingMode,

    /// Anti-aliasing render mode (subpixel LCD or grayscale).
    pub render_mode: RenderMode,
}

#[cfg(test)]
mod ligature_test {
    use cosmic_text::{Buffer, FontSystem, Metrics, Shaping, Attrs, Family, FontFeatures, FeatureTag};

    /// Minimal test: shape ">=" with JetBrainsMono and ligature features.
    /// This bypasses all of zenterm's code to isolate cosmic-text/harfbuzz behavior.
    fn test_ligature_shaping(text: &str, use_features: bool, font_path: &str, font_name: &str) {
        let mut db = fontdb::Database::new();
        let font_data = std::fs::read(font_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", font_path, e));
        db.load_font_data(font_data);

        let mut font_system = FontSystem::new_with_locale_and_db(
            "en-US".into(),
            db,
        );

        let metrics = Metrics::new(18.0, 22.0);
        let mut buf = Buffer::new(&mut font_system, metrics);
        buf.set_size(Some(500.0), None);

        let attrs = if use_features {
            let mut font_features = FontFeatures::new();
            font_features.enable(FeatureTag::STANDARD_LIGATURES);
            font_features.enable(FeatureTag::CONTEXTUAL_LIGATURES);
            font_features.enable(FeatureTag::CONTEXTUAL_ALTERNATES);
            font_features.enable(FeatureTag::DISCRETIONARY_LIGATURES);
            font_features.enable(FeatureTag::KERNING);
            Attrs::new()
                .family(Family::Name(font_name))
                .font_features(font_features)
        } else {
            Attrs::new()
                .family(Family::Name(font_name))
        };

        buf.set_text(text, &attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(&mut font_system, true);

        let shape = buf.lines[0].shape_opt().expect("ShapeLine not found");
        let span = &shape.spans[0];

        let total_glyphs: usize = span.words.iter().map(|w| w.glyphs.len()).sum();
        let expected = text.chars().count();
        let label = if use_features { "feat" } else { "def " };
        if total_glyphs < expected {
            eprintln!("  [{label}] {:?}: LIGATURE OK ({})", text, total_glyphs);
        } else {
            eprintln!("  [{label}] {:?}: no ligature ({})", text, total_glyphs);
        }
    }

    #[test]
    fn test_ligatures() {
        let jetbrains = (
            concat!(env!("HOME"), "/Library/Fonts/JetBrainsMonoNerdFont-Regular.ttf"),
            "JetBrainsMono Nerd Font",
        );

        let test_cases = ["->", ">=", "!=", "<=", "=>", "::", "//", "||", "&&"];

        eprintln!("\n=== JetBrainsMono Nerd Font ===");
        for text in &test_cases {
            test_ligature_shaping(text, false, jetbrains.0, jetbrains.1);
            test_ligature_shaping(text, true, jetbrains.0, jetbrains.1);
        }
    }
}
