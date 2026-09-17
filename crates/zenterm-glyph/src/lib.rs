//! Glyph atlas — rasterizes characters with `cosmic-text` (shaping) + `swash`
//! (subpixel rasterization) and packs them into GPU-friendly texture atlases.
//!
//! Unlike cosmic-text's built-in `SwashCache` (which hardcodes `Format::Alpha`),
//! we call swash directly with `Format::Subpixel` to get per-channel RGB coverage
//! values for LCD subpixel rendering.
//!
//! # Multi-atlas architecture
//!
//! The atlas stores glyphs across multiple independent texture slots
//! ([`AtlasSlot`]), each with its own [`etagere::AtlasAllocator`] and pixel
//! buffer.  When a slot runs out of space a new, larger slot is pushed onto
//! [`GlyphAtlas::slots`].  Existing slots are *never* modified or evicted, so
//! all [`GlyphEntry`] UV coordinates remain valid forever — matching the
//! strategy used by Alacritty.
//!
//! ```text
//!   Slot 0 (512×512, mostly full)
//!   Slot 1 (1024×1024, just started)
//!     ↑ never touched again once full
//! ```

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use cosmic_text::{FontSystem, Metrics};
use etagere::AtlasAllocator;
use swash::scale::ScaleContext;

use zenterm_core::{HintingMode, RenderMode, SubpixelLayout};

/// Bound the ligature caches so arbitrary terminal output cannot retain an
/// unbounded number of owned run strings and shaped-glyph vectors.
pub(crate) const MAX_RUN_CACHE_ENTRIES: usize = 1024;
pub(crate) const MAX_NO_EFFECT_CACHE_ENTRIES: usize = 4096;
pub(crate) const MAX_CACHED_RUN_BYTES: usize = 4096;

pub mod allocate;
pub mod atlas_impl;
pub mod builtin;
pub mod font_list;
mod image_atlas;
mod metrics;
pub mod rasterize;

// All `impl GlyphAtlas` blocks live in sub-modules:
// - `atlas_impl` — construction, shaping, and glyph rasterization
// - `allocate`   — texture growth (`grow_atlas`)
// - `metrics`    — font measurements and cell geometry
// - `image_atlas` — image allocation and eviction
// - `rasterize`  — low-level swash rasterization (`rasterize_swash`)

/// The type of content stored in a glyph's atlas entry.
///
/// This determines how the fragment shader interprets the texture data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphContentType {
    /// LCD subpixel coverage (per-channel R/G/B values).  Most text.
    Subpixel,
    /// Grayscale alpha / intensity mask. Built-in block glyphs and fallback
    /// faces use this path.
    Mask,
    /// Full RGBA color.  Emoji / color glyphs.
    Color,
}

/// A single glyph's position and metrics within the atlas.
#[derive(Debug, Clone)]
pub struct GlyphEntry {
    /// Index into [`GlyphAtlas::slots`] identifying which atlas texture
    /// this glyph is stored in.  Once assigned this index never changes
    /// — the slot and its texture are kept alive indefinitely.
    pub atlas_index: u32,
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
    /// `1.0` means "use the rasterizer's natural size". Values below `1.0`
    /// are used only as a safety adaptation for an oversized fallback glyph
    /// whose bitmap would otherwise exceed the fixed primary-font cell.
    ///
    /// The renderer multiplies both the rendered quad's size and the bearing
    /// offsets by this value. Oversized fallbacks are first re-rasterized at a
    /// smaller font size; this value covers any residual rounding or hinting
    /// difference from that pass.
    pub scale: f32,
}

/// Visual font attributes that affect shaping and rasterization.
///
/// Terminal cells carry these two flags independently. Keeping them in one
/// small, hashable value lets the atlas keep regular, bold, italic, and
/// bold-italic glyphs separate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct GlyphStyle {
    pub bold: bool,
    pub italic: bool,
}

/// A single atlas texture slot — a square region of texture with its own
/// rectangle allocator and pixel data.
///
/// Each slot is an independent GPU texture.  When a slot fills up,
/// [`GlyphAtlas::grow_atlas`] pushes a new larger slot without touching
/// the existing one, so all prior [`GlyphEntry`] references remain valid.
pub struct AtlasSlot {
    /// Rectangle allocator for packing glyphs into this slot.
    pub allocator: AtlasAllocator,
    /// RGBA pixel data of this slot's texture.
    pub texture_data: Vec<u8>,
    /// Width and height of this slot (square, power of two).
    pub size: u32,
}

impl std::fmt::Debug for AtlasSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AtlasSlot")
            .field("size", &self.size)
            .field("texture_data.len", &self.texture_data.len())
            .finish()
    }
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
///   resizing the font invalidates the cache;
/// * `style` — the bold/italic attributes used for shaping and rasterization.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RunCacheKey {
    pub text: String,
    pub font_size_bits: u32,
    pub style: GlyphStyle,
}

/// A rectangular region whose CPU atlas pixels changed since the last GPU
/// synchronization.
#[derive(Debug, Clone, Copy)]
pub struct AtlasDirtyRegion {
    pub atlas_index: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Cache key for a single character glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct GlyphCacheKey {
    pub character: char,
    pub font_size_bits: u32,
    pub style: GlyphStyle,
}

impl GlyphCacheKey {
    pub const fn new(character: char, font_size_bits: u32, style: GlyphStyle) -> Self {
        Self {
            character,
            font_size_bits,
            style,
        }
    }
}

/// One shaped glyph output by a multi-character run.
///
/// Ligature runs produce one `ShapedGlyph` per output glyph (which may
/// cover multiple source cells).  The per-char path is identical but
/// always has `num_cells == 1`.
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
///
/// Manages a set of texture slots (see [`AtlasSlot`]) and caches.  New
/// glyphs are packed into the current (last) slot; when it fills up,
/// [`grow_atlas`](Self::grow_atlas) pushes a new larger slot.
pub struct GlyphAtlas {
    pub font_system: FontSystem,
    /// Texture slots, indexed by [`GlyphEntry::atlas_index`].
    /// Slot 0 is always present; new slots are appended on demand.
    pub slots: Vec<AtlasSlot>,
    font_size: f32,
    /// Font family name used for shaping (e.g. "Consolas", "Menlo").
    font_family: Cow<'static, str>,
    /// The face selected for the primary-font metrics probe. A glyph whose
    /// resolved face differs from this one is a fallback glyph and may need
    /// fallback-specific cell adaptation.
    primary_font_id: Option<cosmic_text::fontdb::ID>,
    /// Display scale factor (physical pixels per logical point).
    pixels_per_point: f32,
    /// LCD subpixel order (RGB or BGR), auto-detected from the OS.
    subpixel_layout: SubpixelLayout,
    metrics: Metrics,
    glyph_cache: HashMap<GlyphCacheKey, GlyphEntry>,
    /// Cache for multi-character runs (ligature support).
    ///
    /// Keyed by `(text, font_size_bits, style)`.  Each entry holds the rasterised
    /// glyphs produced by shaping the run as a whole.
    ///
    /// Currently unused — populated only when ligature shaping is enabled.
    /// See [`Self::shape_and_rasterize_run`].
    run_cache: HashMap<RunCacheKey, Vec<ShapedGlyph>>,
    /// Negative cache: runs that were shaped with ligature features but
    /// produced no actual substitution (identical to per-char baseline).
    /// Subsequent calls skip shaping entirely and fall through to per-char.
    no_effect_cache: HashSet<RunCacheKey>,
    /// Cache for image data placed in the atlas, keyed by content hash.
    /// Value is `(GlyphEntry, AllocationId)` so individual images can be
    /// removed from the atlas without invalidating a whole slot.
    image_cache: HashMap<[u8; 32], (GlyphEntry, etagere::AllocId)>,
    /// Pixel regions changed since the last renderer synchronization.
    pub(crate) dirty_regions: Vec<AtlasDirtyRegion>,

    /// Swash scale context (replaces cosmic-text's `SwashCache`).
    swash_ctx: ScaleContext,
    /// Cached cell width/height in pixels, set by [`cell_size()`](Self::cell_size).
    cell_width: f32,
    cell_height: f32,
    /// Underline thickness from font metrics, in pixels.
    ///
    /// This is the font's design underline thickness (from the OS/2 or `post`
    /// table), scaled to physical pixels.  Used as the base stroke width for
    /// box-drawing characters so that rendered lines match the font's own
    /// stroke weight (matching WezTerm's approach).
    ///
    /// When zero (unset), built-in line drawing falls back to the Alacritty
    /// heuristic of `cell_width / 8`.
    underline_thickness_px: f32,
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
    /// Whether OpenType ligature features are enabled.
    pub ligatures_enabled: bool,
    /// Hinting mode for glyph rasterization.
    hinting_mode: HintingMode,
    /// Render mode (subpixel or grayscale).
    render_mode: RenderMode,
}
