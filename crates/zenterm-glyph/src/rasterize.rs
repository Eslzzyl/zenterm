//! Low-level swash rasterization — the final step that invokes swash's
//! [`Render`] pipeline to produce a bitmap from a shaped glyph.
//!
//! ## Gamma correction
//!
//! Swash outputs linear coverage values, but human vision is non-linear.
//! FreeType's LCD renderer applies gamma correction internally; we replicate
//! that here so CJK strokes appear with the correct visual weight.
//!
//! A gamma value of ≈1.3 is a mild correction that slightly thickens
//! mid-tone coverage values for better perceived stroke weight.

use cosmic_text::CacheKeyFlags;
use swash::scale::{Render, Source, StrikeWith};
use swash::zeno::{Angle, Format, Transform, Vector};

use zenterm_core::{HintingMode, RenderMode, SubpixelLayout};

use crate::GlyphAtlas;

/// Gamma value for subpixel coverage correction.
///
/// 1.3 is a mild correction that slightly thickens mid-tone coverage
/// values for better perceived stroke weight without significant
/// softening.  Values between 1.0 (no correction) and 1.5 (strong)
/// are common.
const SUBPIXEL_GAMMA: f32 = 1.3;

/// Apply gamma correction to a subpixel-rendered glyph image.
///
/// This converts linear coverage values (as produced by swash) into
/// gamma-corrected values that better match human perception, making
/// strokes appear with the correct visual thickness.
///
/// Only applies to `Content::SubpixelMask` — grayscale masks and color
/// bitmaps are left unchanged.
fn apply_gamma_correction(img: &mut swash::scale::image::Image) {
    if img.content != swash::scale::image::Content::SubpixelMask {
        return;
    }
    let inv_gamma = 1.0 / SUBPIXEL_GAMMA;
    for chunk in img.data.chunks_exact_mut(4) {
        let r = (chunk[0] as f32 / 255.0).powf(inv_gamma);
        let g = (chunk[1] as f32 / 255.0).powf(inv_gamma);
        let b = (chunk[2] as f32 / 255.0).powf(inv_gamma);
        chunk[0] = (r * 255.0).round() as u8;
        chunk[1] = (g * 255.0).round() as u8;
        chunk[2] = (b * 255.0).round() as u8;
    }
}

impl GlyphAtlas {
    /// Rasterize a glyph via swash with `Format::Subpixel` and
    /// gamma-corrected coverage values.
    pub(crate) fn rasterize_swash(
        &mut self,
        cache_key: &cosmic_text::CacheKey,
    ) -> Option<swash::scale::image::Image> {
        let font = self
            .font_system
            .get_font(cache_key.font_id, cache_key.font_weight)?;

        let hint = match self.hinting_mode {
            HintingMode::None => false,
            HintingMode::Full => true,
            HintingMode::Auto => !cache_key.flags.contains(CacheKeyFlags::DISABLE_HINTING),
        };

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

        let format = match self.render_mode {
            RenderMode::Subpixel => match self.subpixel_layout {
                SubpixelLayout::Rgb => Format::Subpixel,
                SubpixelLayout::Bgr => Format::subpixel_bgra(),
            },
            RenderMode::Grayscale => Format::Alpha,
        };

        log::debug!(
            "rasterize_swash: glyph_id={} format={:?} offset=({:.3},{:.3})",
            cache_key.glyph_id,
            format,
            offset.x,
            offset.y,
        );

        let mut img = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .format(format)
        .offset(offset)
        .transform(transform)
        .render(&mut scaler, cache_key.glyph_id)?;

        apply_gamma_correction(&mut img);

        Some(img)
    }
}
