//! Low-level swash rasterization — the final step that invokes swash's
//! [`Render`] pipeline to produce a bitmap from a shaped glyph.
//!
//! ## Coverage correction
//!
//! Swash outputs coverage values directly from its analytical rasterizer.
//! The display path then composites those values on a non-sRGB framebuffer.
//! A small curve on grayscale coverage restores a little of the edge weight
//! that is otherwise lost at small sizes, especially in CJK fallback faces.
//!
//! The Subpixel curve remains neutral so the composited GPU surface does not
//! receive an extra brightness boost along LCD edges.

use cosmic_text::CacheKeyFlags;
use swash::scale::{Render, Source, StrikeWith};
use swash::zeno::{Angle, Format, Transform, Vector};

use zenterm_core::{HintingMode, RenderMode, SubpixelLayout};

use crate::GlyphAtlas;

/// Gamma value for grayscale coverage correction.
///
/// Keep the swash coverage unchanged. This is the closest baseline to the
/// controlled WezTerm comparison; any weight adjustment should be validated
/// separately rather than being implicit in the default rasterizer path.
const GRAYSCALE_GAMMA: f32 = 1.0;

/// Keep the subpixel path neutral. Its per-channel values already participate
/// in a separate shader equation and should not be thickened here.
const SUBPIXEL_GAMMA: f32 = 1.0;

fn correct_coverage(coverage: u8, gamma: f32) -> u8 {
    if gamma == 1.0 {
        return coverage;
    }

    let corrected = (f32::from(coverage) / 255.0).powf(1.0 / gamma);
    (corrected * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Apply the selected coverage curve to a rasterized glyph image.
///
/// Grayscale masks get a mild weight correction. Subpixel RGB coverage keeps
/// its native values, and color bitmaps are left untouched.
///
fn apply_gamma_correction(img: &mut swash::scale::image::Image) {
    match img.content {
        swash::scale::image::Content::Mask => {
            for coverage in &mut img.data {
                *coverage = correct_coverage(*coverage, GRAYSCALE_GAMMA);
            }
        }
        swash::scale::image::Content::SubpixelMask => {
            for chunk in img.data.as_chunks_mut::<4>().0 {
                chunk[0] = correct_coverage(chunk[0], SUBPIXEL_GAMMA);
                chunk[1] = correct_coverage(chunk[1], SUBPIXEL_GAMMA);
                chunk[2] = correct_coverage(chunk[2], SUBPIXEL_GAMMA);
            }
        }
        swash::scale::image::Content::Color => {}
    }
}

impl GlyphAtlas {
    /// Rasterize a glyph via swash with the configured primary-face format
    /// and gamma-corrected coverage values. Resolved fallback faces always
    /// use `Format::Alpha` to avoid display-subpixel fringing.
    pub(crate) fn rasterize_swash(
        &mut self,
        cache_key: &cosmic_text::CacheKey,
    ) -> Option<swash::scale::image::Image> {
        let font = self
            .font_system
            .get_font(cache_key.font_id, cache_key.font_weight)?;
        // `font_id` is the face selected by cosmic-text's fallback resolver.
        // Keep the resolved family in debug logs so fallback behavior can be
        // diagnosed from the same key that is passed to swash.
        self.log_font_face("glyph rasterization", cache_key.font_id);

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

        let offset = Vector::new(cache_key.x_bin.as_float(), cache_key.y_bin.as_float());

        let transform = if cache_key.flags.contains(CacheKeyFlags::FAKE_ITALIC) {
            Some(Transform::skew(
                Angle::from_degrees(14.0),
                Angle::from_degrees(0.0),
            ))
        } else {
            None
        };

        // LCD coverage is only safe when the glyph remains tied to the
        // primary face's physical metrics and positioning. Fallback faces
        // can have different metrics and are commonly the source of visible
        // RGB fringing, so rasterize them as ordinary alpha masks.
        let is_fallback = self
            .primary_font_id
            .is_some_and(|primary| primary != cache_key.font_id);
        let format = if is_fallback {
            Format::Alpha
        } else {
            match self.render_mode {
                RenderMode::Subpixel => match self.subpixel_layout {
                    SubpixelLayout::Rgb => Format::Subpixel,
                    SubpixelLayout::Bgr => Format::subpixel_bgra(),
                },
                RenderMode::Grayscale => Format::Alpha,
            }
        };

        log::debug!(
            "rasterize_swash: glyph_id={} format={:?} fallback={} offset=({:.3},{:.3})",
            cache_key.glyph_id,
            format,
            is_fallback,
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

#[cfg(test)]
mod tests {
    use super::{GRAYSCALE_GAMMA, correct_coverage};

    #[test]
    fn coverage_curve_preserves_endpoints() {
        assert_eq!(correct_coverage(0, 1.08), 0);
        assert_eq!(correct_coverage(255, 1.08), 255);
    }

    #[test]
    fn grayscale_coverage_keeps_the_neutral_curve() {
        assert_eq!(GRAYSCALE_GAMMA, 1.0);
        let corrected: Vec<_> = (0..=255)
            .map(|v| correct_coverage(v, GRAYSCALE_GAMMA))
            .collect();
        assert_eq!(corrected, (0..=255).collect::<Vec<_>>());
    }
}
