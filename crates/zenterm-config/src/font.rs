//! Font configuration parsed from the `[font]` section.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field, default value, or enum variant in this module,
//! update [`docs/usages/config.md`] to match.

use serde::{Deserialize, Serialize};

// ── FontConfig ─────────────────────────────────────────────────────────

/// The `[font]` section of the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontConfig {
    /// Font size in *logical pixels* at 1× DPI scaling.
    ///
    /// The final physical pixel size is `size × pixels_per_point`, so on a
    /// Retina display at 2× scaling a value of 18 produces a 36 px font.
    ///
    /// Design note: we store the same unit that the codebase has always used
    /// (`18.0 × ppp`) so that the exact default behaviour is preserved.
    /// If you prefer to think in points:  1 pt ≈ 0.75 logical-pixels on a
    /// standard 96 DPI display (18 logical-px ≈ 13.5 pt).
    #[serde(default = "default_font_size")]
    pub size: f32,

    /// The normal (regular) font.
    #[serde(default = "default_normal_font")]
    pub normal: FontDescription,

    /// Bold font face.  Falls back to `normal` when absent.
    pub bold: Option<FontDescription>,

    /// Italic font face.  Falls back to `normal` when absent.
    pub italic: Option<FontDescription>,

    /// Bold-italic font face.  Falls back to `normal` when absent.
    pub bold_italic: Option<FontDescription>,

    /// Extra horizontal / vertical spacing applied to every character
    /// (in *logical pixels* at 1× DPI).
    #[serde(default)]
    pub offset: GlyphOffset,

    /// Per-glyph offset within each cell (logical pixels at 1× DPI).
    #[serde(default)]
    pub glyph_offset: GlyphOffset,

    /// Use the built-in software renderer for box-drawing characters
    /// (U+2500–U+257F and U+2580–U+259F).  When disabled these code
    /// points are looked up from the font like any other character.
    #[serde(default = "default_builtin_box_drawing")]
    pub builtin_box_drawing: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            size: default_font_size(),
            normal: default_normal_font(),
            bold: None,
            italic: None,
            bold_italic: None,
            offset: GlyphOffset::default(),
            glyph_offset: GlyphOffset::default(),
            builtin_box_drawing: default_builtin_box_drawing(),
        }
    }
}

fn default_font_size() -> f32 {
    18.0
}

fn default_builtin_box_drawing() -> bool {
    true
}

// ── FontDescription ────────────────────────────────────────────────────

/// A font face identified by its family name and optional style.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontDescription {
    /// Font family name (e.g. `"JetBrains Mono"`, `"Menlo"`, `"monospace"`).
    pub family: String,
    /// Font style (e.g. `"Regular"`, `"Bold"`, `"Italic"`).
    /// Cosmetic / metadata only — cosmic-text resolves styles via
    /// `Attrs::weight()` and `Attrs::style()` from the family.
    pub style: Option<String>,
}

fn default_normal_font() -> FontDescription {
    FontDescription {
        family: default_font_family(),
        style: None,
    }
}

/// Platform-appropriate monospace font, matching [`GlyphAtlas::default_font_family`].
fn default_font_family() -> String {
    #[cfg(target_os = "windows")]
    {
        "Consolas".into()
    }
    #[cfg(target_os = "macos")]
    {
        "Menlo".into()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "monospace".into()
    }
}

// ── GlyphOffset ────────────────────────────────────────────────────────

/// An x/y offset applied to glyphs (in logical pixels at 1× DPI).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GlyphOffset {
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
}

impl Default for GlyphOffset {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0 }
    }
}
