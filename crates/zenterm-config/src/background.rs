//! Terminal background image configuration parsed from the `[background]` section.
//!
//! Controls the optional background image behind the terminal content.
//!
//! # Behaviour
//!
//! When `image_path` is set to a valid image file path, the image is loaded
//! and rendered as the terminal background, blended with the theme background
//! colour according to `image_opacity`.  When `image_path` is `None` or empty,
//! no background image is shown and the theme background colour is used as-is.
//!
//! The image is rendered through the wgpu rendering pipeline as a full-viewport
//! quad behind all cell instances.  This means per-cell backgrounds (selection,
//! cursor, highlighted text) render on top of the image.
//!
//! # ⚠  Maintenance note
//!
//! If you modify any field, default value, or enum variant in this module,
//! update [`docs/usages/config.md`] to match.

use serde::{Deserialize, Serialize};

// ── BackgroundConfig ─────────────────────────────────────────────────────

/// The `[background]` section of the config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundConfig {
    /// Path to a background image file.
    ///
    /// Supports any image format that the `image` crate can decode (PNG, JPEG,
    /// GIF, WebP, BMP, etc.).  The path can be absolute or relative to the
    /// config file directory.
    ///
    /// When `None` or empty, no background image is displayed and the terminal
    /// uses the theme background colour as normal.
    #[serde(default)]
    pub image_path: Option<String>,

    /// Opacity of the background image (0.0 – 1.0).
    ///
    /// At 0.0 the image is fully transparent (only the theme background colour
    /// is visible).  At 1.0 the image fully replaces the theme background
    /// colour.  Intermediate values blend the image with the theme background.
    #[serde(default = "default_image_opacity")]
    pub image_opacity: f32,

    /// How the image fits the terminal area when aspect ratios differ.
    #[serde(default)]
    pub image_mode: ImageFitMode,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            image_path: None,
            image_opacity: default_image_opacity(),
            image_mode: ImageFitMode::default(),
        }
    }
}

fn default_image_opacity() -> f32 {
    0.8
}

// ── ImageFitMode ─────────────────────────────────────────────────────────

/// How a background image is fitted to the terminal viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ImageFitMode {
    /// Scale the image to cover the entire viewport, cropping the
    /// longer dimension to preserve the aspect ratio.
    #[default]
    #[serde(rename = "Cover")]
    Cover,

    /// Scale the image to fit within the viewport, letterboxing
    /// (adding empty bands) when the aspect ratios differ.
    #[serde(rename = "Contain")]
    Contain,

    /// Stretch the image to fill the entire viewport, ignoring
    /// the aspect ratio.
    #[serde(rename = "Stretch")]
    Stretch,

    /// Center the image at its native pixel size.  If the image is
    /// larger than the viewport it is cropped; if smaller the theme
    /// background colour shows around it.
    #[serde(rename = "Center")]
    Center,
}
