//! Shared glyph atlas.
//!
//! Wraps [`GlyphAtlas`] behind a `Mutex` so all terminal sessions
//! share a single font cache + texture.  Without this, each tab would
//! maintain its own `etagere::AtlasAllocator` + `cosmic-text`
//! `FontSystem`, multiplying memory by the number of tabs and forcing
//! every session to re-rasterise the same characters.
//!
//! # GPU upload coordination
//!
//! The atlas is uploaded to the GPU via a single
//! [`SharedRenderState`](zenterm_render::callback::SharedRenderState)
//! (one `AtlasUpdate` channel for the whole application).  The render
//! loop is responsible for calling [`SharedGlyphAtlas::sync_to_gpu`]
//! once per frame to push the latest atlas pixels to the GPU and
//! clear the dirty flag.
//!
//! # Locking model
//!
//! Hot paths (such as the per-cell `update_cell_instances` loop) hold
//! the lock for the whole method, which is fine in practice: the
//! atlas is only touched from the main (UI) thread, so the `Mutex` is
//! uncontended and effectively a `RefCell`.  Public convenience
//! methods ([`Self::cell_size`], [`Self::texture_size`], etc.) take
//! the lock briefly and release it.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, MutexGuard};

use zenterm_core::{Result, SubpixelLayout};
use zenterm_glyph::{GlyphAtlas, ShapedGlyph};
use zenterm_render::callback::{AtlasUpdate, SharedRenderState};use std::sync::atomic::Ordering;

/// Guard returned by [`SharedGlyphAtlas::lock`].  Provides mutable
/// access to the underlying [`GlyphAtlas`].
pub struct GlyphAtlasGuard<'a> {
    guard: MutexGuard<'a, GlyphAtlas>,
}

/// App-level shared glyph atlas, used by every terminal session.
pub struct SharedGlyphAtlas {
    inner: Mutex<GlyphAtlas>,
    shared: Arc<SharedRenderState>,
    /// Last known atlas size used to detect growth (texture resize).
    last_size: Mutex<u32>,
}

impl SharedGlyphAtlas {
    /// Create a new shared atlas and push the initial texture to the
    /// GPU so the very first `prepare()` can create its texture.
    ///
    /// `ligatures_enabled` controls whether OpenType ligature features
    /// are enabled during shaping.  See
    /// [`GlyphAtlas::ligatures_enabled`].
    pub fn new(
        font_size: f32,
        font_family: Cow<'static, str>,
        pixels_per_point: f32,
        subpixel_layout: SubpixelLayout,
        ligatures_enabled: bool,
        shared: Arc<SharedRenderState>,
    ) -> Self {
        let atlas = GlyphAtlas::new(
            font_size,
            font_family,
            pixels_per_point,
            subpixel_layout,
            ligatures_enabled,
        );

        // Pre-seed the GPU with whatever the atlas already has so the
        // first frame doesn't render with a blank texture.
        let size = atlas.texture_size;
        {
            let mut update = shared.atlas_update.lock().unwrap();
            *update = Some(AtlasUpdate {
                size,
                data: atlas.texture_data.clone(),
                resized: true,
            });
        }
        shared.atlas_dirty.store(true, Ordering::Release);

        Self {
            inner: Mutex::new(atlas),
            shared,
            last_size: Mutex::new(size),
        }
    }

    /// Acquire exclusive access to the underlying [`GlyphAtlas`].
    /// The guard derefs to `&mut GlyphAtlas`; use it to call methods
    /// such as `ensure_glyph` and read fields like `texture_data` /
    /// `cell_baseline_offset` directly.
    pub fn lock(&self) -> GlyphAtlasGuard<'_> {
        GlyphAtlasGuard {
            guard: self.inner.lock().unwrap(),
        }
    }

    /// Measure the cell width/height from the current font metrics.
    /// Convenience wrapper for callers that don't want to hold the
    /// lock for long.
    pub fn cell_size(&self) -> (f32, f32) {
        self.lock().guard.cell_size().expect("glyph atlas cell_size")
    }

    /// Y offset (in pixels) from the cell top to the baseline.
    pub fn cell_baseline_offset(&self) -> f32 {
        self.lock().guard.cell_baseline_offset()
    }

    /// Read the current atlas texture size (pixels, square).
    pub fn texture_size(&self) -> u32 {
        self.lock().guard.texture_size
    }

    /// Push the latest atlas pixels to the GPU.
    ///
    /// Called after new glyphs were rasterised.  The caller already
    /// guarantees that `texture_data` has changed, so we unconditionally
    /// stage the payload and mark dirty for `prepare()` to pick up.
    pub fn sync_to_gpu(&self) {
        let atlas = self.inner.lock().unwrap();
        let current_size = atlas.texture_size;
        let mut last = self.last_size.lock().unwrap();
        let resized = current_size != *last;
        if resized {
            *last = current_size;
        }

        let mut update = self.shared.atlas_update.lock().unwrap();
        *update = Some(AtlasUpdate {
            size: current_size,
            data: atlas.texture_data.clone(),
            resized,
        });
        self.shared.atlas_dirty.store(true, Ordering::Release);
    }

    /// Mark the GPU copy as dirty without uploading.  Useful when
    /// the caller has already produced the new texture bytes via
    /// `ensure_glyph` and will call [`Self::sync_to_gpu`] at the end
    /// of the frame.
    pub fn mark_dirty(&self) {
        self.shared.atlas_dirty.store(true, Ordering::Release);
    }

    /// Rebuild the atlas for a new DPI scale factor.  All cached
    /// glyphs are dropped; ASCII re-seed is the caller's
    /// responsibility.
    pub fn reinit_for_dpi(
        &self,
        font_size: f32,
        font_family: Cow<'static, str>,
        pixels_per_point: f32,
        subpixel_layout: SubpixelLayout,
        ligatures_enabled: bool,
    ) -> (f32, f32) {
        let (cw, ch, size) = {
            let mut atlas = self.inner.lock().unwrap();
            *atlas = GlyphAtlas::new(
                font_size,
                font_family,
                pixels_per_point,
                subpixel_layout,
                ligatures_enabled,
            );
            let (cw, ch) = atlas.cell_size().expect("cell_size after DPI reinit");
            let size = atlas.texture_size;
            (cw, ch, size)
        };
        {
            let mut update = self.shared.atlas_update.lock().unwrap();
            *update = Some(AtlasUpdate {
                size,
                data: self.inner.lock().unwrap().texture_data.clone(),
                resized: true,
            });
        }
        self.shared.atlas_dirty.store(true, Ordering::Release);
        *self.last_size.lock().unwrap() = size;
        (cw, ch)
    }

    /// Re-seed the atlas with common ASCII characters so the first
    /// frame has something to render.
    pub fn seed_ascii(&self) {
        const ASCII: &str =
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 .,!?;:-=+*/\\|()[]{}<>\"'`~@#$%^&_";
        let mut atlas = self.inner.lock().unwrap();
        for c in ASCII.chars() {
            let _ = atlas.ensure_glyph(c);
        }
    }

    /// Shape and rasterise a run of consecutive characters.
    ///
    /// Delegates to [`GlyphAtlas::shape_and_rasterize_run`].
    /// Holds the lock for the duration of the call.
    ///
    /// # Preparatory note
    ///
    /// Currently each character is shaped individually (see the
    /// documentation on `GlyphAtlas::shape_and_rasterize_run`).
    /// When ligature shaping is implemented, this method will
    /// shape the entire `text` string as a single unit.
    pub fn shape_and_rasterize_run(&self, text: &str) -> Result<Vec<ShapedGlyph>> {
        let mut atlas = self.inner.lock().unwrap();
        let result = atlas.shape_and_rasterize_run(text)?;
        Ok(result)
    }
}

impl<'a> std::ops::Deref for GlyphAtlasGuard<'a> {
    type Target = GlyphAtlas;
    fn deref(&self) -> &Self::Target { &*self.guard }
}

impl<'a> std::ops::DerefMut for GlyphAtlasGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut *self.guard }
}


