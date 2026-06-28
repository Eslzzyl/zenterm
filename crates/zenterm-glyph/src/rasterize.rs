//! Low-level swash rasterization — the final step that invokes swash's
//! [`Render`] pipeline to produce a bitmap from a shaped glyph.

use cosmic_text::CacheKeyFlags;
use swash::scale::{Render, Source, StrikeWith};
use swash::zeno::{Angle, Format, Transform, Vector};

use zenterm_core::SubpixelLayout;

use crate::GlyphAtlas;

impl GlyphAtlas {
    /// Rasterize a glyph via swash directly with `Format::Subpixel`,
    /// bypassing cosmic-text's `SwashCache` (which hardcodes `Format::Alpha`).
    pub(crate) fn rasterize_swash(
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
