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
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use std::sync::atomic::{AtomicU32, Ordering};
use zenterm_core::{HintingMode, RenderMode, Result, SubpixelLayout};
use zenterm_glyph::{GlyphAtlas, GlyphStyle, ShapedGlyph};
use zenterm_render::callback::{
    AtlasRegionData, AtlasSlotData, AtlasUpdate, ImageTextureData, ImageTextureUpdate,
    SharedRenderState,
};

/// Guard returned by [`SharedGlyphAtlas::lock`].  Provides mutable
/// access to the underlying [`GlyphAtlas`].
pub struct GlyphAtlasGuard<'a> {
    guard: MutexGuard<'a, GlyphAtlas>,
}

/// App-level shared glyph atlas, used by every terminal session.
pub struct SharedGlyphAtlas {
    inner: Mutex<GlyphAtlas>,
    shared: Arc<SharedRenderState>,
    /// Number of slots already represented by pending/consumed GPU textures.
    gpu_slot_count: AtomicU32,
    image_cache: Mutex<ImageGpuCache>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ImageKey {
    pub hash: [u8; 32],
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ImageEntry {
    pub texture_index: usize,
}

/// Pixel storage supplied to a new GPU image texture.
///
/// Image frames use shared storage in `ImageDataType`, so the upload can hold
/// an `Arc` without cloning the full pixel buffer.  Frame edits use
/// copy-on-write when the upload still owns a reference.
#[derive(Clone, Copy)]
pub(super) enum ImagePixels<'a> {
    Shared(&'a Arc<Vec<u8>>),
}

impl ImagePixels<'_> {
    fn into_shared(self) -> Arc<Vec<u8>> {
        match self {
            Self::Shared(data) => Arc::clone(data),
        }
    }
}

#[derive(Debug, Default)]
struct ImageGpuCache {
    entries: HashMap<ImageKey, ImageEntry>,
    source_keys: HashMap<usize, ImageKey>,
    key_refs: HashMap<ImageKey, usize>,
    next_index: usize,
    free_indices: Vec<usize>,
}

impl SharedGlyphAtlas {
    /// Create a new shared atlas and push the initial texture to the
    /// GPU so the very first `prepare()` can create its texture.
    ///
    /// `ligatures_enabled` controls whether OpenType ligature features
    /// are enabled during shaping.  See
    /// [`GlyphAtlas::ligatures_enabled`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        font_size: f32,
        font_family: Cow<'static, str>,
        pixels_per_point: f32,
        subpixel_layout: SubpixelLayout,
        ligatures_enabled: bool,
        hinting_mode: HintingMode,
        render_mode: RenderMode,
        shared: Arc<SharedRenderState>,
    ) -> Self {
        let atlas = GlyphAtlas::new(
            font_size,
            font_family,
            pixels_per_point,
            subpixel_layout,
            ligatures_enabled,
            hinting_mode,
            render_mode,
        );

        // Pre-seed the GPU with whatever the atlas already has so the
        // first frame doesn't render with a blank texture.
        let slots: Vec<AtlasSlotData> = atlas
            .slots
            .iter()
            .enumerate()
            .map(|(index, s)| AtlasSlotData {
                index: index as u32,
                size: s.size,
                data: s.texture_data.clone(),
            })
            .collect();
        let slot_count = slots.len() as u32;
        {
            let mut update = shared.atlas_update.lock().unwrap();
            *update = Some(AtlasUpdate {
                slots,
                regions: Vec::new(),
                total_slots: slot_count,
            });
        }
        shared.atlas_dirty.store(true, Ordering::Release);

        Self {
            inner: Mutex::new(atlas),
            shared,
            gpu_slot_count: AtomicU32::new(slot_count),
            image_cache: Mutex::new(ImageGpuCache::default()),
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
        self.lock()
            .guard
            .cell_size()
            .expect("glyph atlas cell_size")
    }

    /// Y offset (in pixels) from the cell top to the baseline.
    pub fn cell_baseline_offset(&self) -> f32 {
        self.lock().guard.cell_baseline_offset()
    }

    /// Read the current atlas texture size (pixels, square).
    /// Returns the size of the first (largest) slot.
    pub fn texture_size(&self) -> u32 {
        let atlas = self.inner.lock().unwrap();
        atlas.slots.first().map(|s| s.size).unwrap_or(512)
    }

    /// Push the latest atlas pixels to the GPU.
    ///
    /// Called after new glyphs were rasterised.  The caller already
    /// guarantees that `texture_data` has changed, so we unconditionally
    /// stage the payload and mark dirty for `prepare()` to pick up.
    ///
    /// Existing slots upload only their changed rectangles. Newly allocated
    /// slots still receive one full initial upload.
    pub fn sync_to_gpu(&self) {
        let mut atlas = self.inner.lock().unwrap();
        let known_slots = self.gpu_slot_count.load(Ordering::Acquire) as usize;
        let slots: Vec<AtlasSlotData> = atlas
            .slots
            .iter()
            .enumerate()
            .skip(known_slots)
            .map(|(index, s)| AtlasSlotData {
                index: index as u32,
                size: s.size,
                data: s.texture_data.clone(),
            })
            .collect();
        let total_slots = atlas.slots.len() as u32;
        let regions = atlas
            .take_dirty_regions()
            .into_iter()
            .filter_map(|region| copy_dirty_region(&atlas, region))
            .collect();
        self.gpu_slot_count.store(total_slots, Ordering::Release);

        let mut update = self.shared.atlas_update.lock().unwrap();
        if let Some(pending) = update.as_mut() {
            pending.slots.extend(slots);
            pending.regions.extend(regions);
            pending.total_slots = total_slots;
        } else {
            *update = Some(AtlasUpdate {
                slots,
                regions,
                total_slots,
            });
        }
        self.shared.atlas_dirty.store(true, Ordering::Release);
    }

    /// Remove an image from the atlas, freeing its GPU texture slot.
    /// Called when the image is evicted from the terminal's `ImageCache`.
    /// Holds the lock for the duration of the call.
    pub fn remove_image(&self, hash: &[u8; 32]) {
        let mut atlas = self.inner.lock().unwrap();
        atlas.remove_image(hash);
        drop(atlas);

        let mut cache = self.image_cache.lock().unwrap();
        let keys: Vec<ImageKey> = cache
            .entries
            .keys()
            .filter(|key| &key.hash == hash)
            .copied()
            .collect();
        let mut removed = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(entry) = cache.entries.remove(&key) {
                cache.key_refs.remove(&key);
                cache.free_indices.push(entry.texture_index);
                removed.push(entry.texture_index);
            }
        }
        cache.source_keys.retain(|_, key| key.hash != *hash);
        drop(cache);

        self.queue_image_releases(removed);
    }

    /// Release one image source while keeping shared content used by other
    /// sessions alive.
    pub(super) fn release_image_source(&self, source_id: usize) {
        let released = {
            let mut cache = self.image_cache.lock().unwrap();
            cache
                .source_keys
                .remove(&source_id)
                .and_then(|key| release_image_key(&mut cache, key))
                .into_iter()
                .collect()
        };
        self.queue_image_releases(released);
    }

    /// Release a deleted/evicted image only for sources owned by one
    /// terminal session.  Identical content in another session remains
    /// shared and keeps its GPU texture.
    pub(super) fn release_image_hash_for_sources(
        &self,
        hash: &[u8; 32],
        source_ids: &mut HashSet<usize>,
    ) {
        let released = {
            let mut cache = self.image_cache.lock().unwrap();
            let mut released = Vec::new();
            let mut released_sources = Vec::new();
            for source_id in source_ids.iter().copied() {
                let Some(key) = cache.source_keys.get(&source_id).copied() else {
                    continue;
                };
                if key.hash != *hash {
                    continue;
                }
                cache.source_keys.remove(&source_id);
                released_sources.push(source_id);
                if let Some(index) = release_image_key(&mut cache, key) {
                    released.push(index);
                }
            }
            for source_id in released_sources {
                source_ids.remove(&source_id);
            }
            released
        };
        self.queue_image_releases(released);
    }

    fn queue_image_releases(&self, removed: Vec<usize>) {
        if removed.is_empty() {
            return;
        }
        let mut update = self.shared.image_update.lock().unwrap();
        if let Some(pending) = update.as_mut() {
            pending.releases.extend(removed);
        } else {
            *update = Some(ImageTextureUpdate {
                uploads: Vec::new(),
                releases: removed,
            });
        }
    }

    /// Ensure an image has an exact-size GPU texture and return its stable
    /// texture index for the render range.
    pub(super) fn ensure_image(
        &self,
        pixels: ImagePixels<'_>,
        width: u32,
        height: u32,
        hash: [u8; 32],
        source_id: usize,
    ) -> ImageEntry {
        let key = ImageKey {
            hash,
            width,
            height,
        };
        let mut cache = self.image_cache.lock().unwrap();
        let mut released = Vec::new();
        let previous_key = cache.source_keys.insert(source_id, key);
        if previous_key == Some(key) {
            if let Some(entry) = cache.entries.get(&key) {
                return *entry;
            }
        } else if let Some(previous_key) = previous_key
            && let Some(index) = release_image_key(&mut cache, previous_key)
        {
            released.push(index);
        }

        let (entry, upload_needed) = if let Some(entry) = cache.entries.get(&key) {
            (*entry, false)
        } else {
            let texture_index = cache.free_indices.pop().unwrap_or_else(|| {
                let index = cache.next_index;
                cache.next_index += 1;
                index
            });
            let entry = ImageEntry { texture_index };
            cache.entries.insert(key, entry);
            (entry, true)
        };
        *cache.key_refs.entry(key).or_default() += 1;
        drop(cache);

        let upload = upload_needed.then(|| ImageTextureData {
            index: entry.texture_index,
            width,
            height,
            data: pixels.into_shared(),
        });
        if upload.is_some() || !released.is_empty() {
            let mut update = self.shared.image_update.lock().unwrap();
            if let Some(pending) = update.as_mut() {
                pending.releases.extend(released);
                if let Some(upload) = upload {
                    pending.uploads.push(upload);
                }
            } else {
                *update = Some(ImageTextureUpdate {
                    uploads: upload.into_iter().collect(),
                    releases: released,
                });
            }
        }
        entry
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
    #[allow(clippy::too_many_arguments)]
    pub fn reinit_for_dpi(
        &self,
        font_size: f32,
        font_family: Cow<'static, str>,
        pixels_per_point: f32,
        subpixel_layout: SubpixelLayout,
        ligatures_enabled: bool,
        hinting_mode: HintingMode,
        render_mode: RenderMode,
    ) -> (f32, f32) {
        let (cw, ch) = {
            let mut atlas = self.inner.lock().unwrap();
            *atlas = GlyphAtlas::new(
                font_size,
                font_family,
                pixels_per_point,
                subpixel_layout,
                ligatures_enabled,
                hinting_mode,
                render_mode,
            );
            let (cw, ch) = atlas.cell_size().expect("cell_size after DPI reinit");
            (cw, ch)
        };
        let atlas = self.inner.lock().unwrap();
        let slots: Vec<AtlasSlotData> = atlas
            .slots
            .iter()
            .enumerate()
            .map(|(index, s)| AtlasSlotData {
                index: index as u32,
                size: s.size,
                data: s.texture_data.clone(),
            })
            .collect();
        {
            let mut update = self.shared.atlas_update.lock().unwrap();
            *update = Some(AtlasUpdate {
                slots,
                regions: Vec::new(),
                total_slots: atlas.slots.len() as u32,
            });
        }
        self.gpu_slot_count.store(0, Ordering::Release);
        self.shared.atlas_dirty.store(true, Ordering::Release);
        (cw, ch)
    }

    /// Re-seed the atlas with common ASCII characters so the first
    /// frame has something to render.
    pub fn seed_ascii(&self) {
        const ASCII: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 .,!?;:-=+*/\\|()[]{}<>\"'`~@#$%^&_";
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
    pub fn shape_and_rasterize_run(&self, text: &str) -> Result<(Vec<ShapedGlyph>, bool, bool)> {
        self.shape_and_rasterize_run_with_style(text, GlyphStyle::default())
    }

    /// Shape and rasterise a run using the requested terminal style.
    pub fn shape_and_rasterize_run_with_style(
        &self,
        text: &str,
        style: GlyphStyle,
    ) -> Result<(Vec<ShapedGlyph>, bool, bool)> {
        let mut atlas = self.inner.lock().unwrap();
        let result = atlas.shape_and_rasterize_run_with_style(text, style)?;
        Ok(result)
    }
}

fn copy_dirty_region(
    atlas: &GlyphAtlas,
    region: zenterm_glyph::AtlasDirtyRegion,
) -> Option<AtlasRegionData> {
    let slot = atlas.slots.get(region.atlas_index as usize)?;
    let slot_size = slot.size as usize;
    let x = region.x as usize;
    let y = region.y as usize;
    let width = region.width as usize;
    let height = region.height as usize;
    if width == 0
        || height == 0
        || x.checked_add(width)? > slot_size
        || y.checked_add(height)? > slot_size
    {
        return None;
    }

    let row_bytes = width.checked_mul(4)?;
    let padded_row_bytes = row_bytes.div_ceil(256) * 256;
    let mut data = Vec::with_capacity(padded_row_bytes.checked_mul(height)?);
    for row in 0..height {
        let start = ((y + row) * slot_size + x).checked_mul(4)?;
        let end = start.checked_add(row_bytes)?;
        data.extend_from_slice(slot.texture_data.get(start..end)?);
        data.resize(data.len() + padded_row_bytes - row_bytes, 0);
    }

    Some(AtlasRegionData {
        atlas_index: region.atlas_index,
        x: region.x,
        y: region.y,
        width: region.width,
        height: region.height,
        data,
    })
}

fn release_image_key(cache: &mut ImageGpuCache, key: ImageKey) -> Option<usize> {
    let ref_count = cache.key_refs.get_mut(&key)?;
    *ref_count = ref_count.saturating_sub(1);
    if *ref_count != 0 {
        return None;
    }
    cache.key_refs.remove(&key);
    cache.entries.remove(&key).map(|entry| {
        cache.free_indices.push(entry.texture_index);
        entry.texture_index
    })
}

impl<'a> std::ops::Deref for GlyphAtlasGuard<'a> {
    type Target = GlyphAtlas;
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<'a> std::ops::DerefMut for GlyphAtlasGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: u8) -> ImageKey {
        ImageKey {
            hash: [value; 32],
            width: 64,
            height: 64,
        }
    }

    #[test]
    fn releasing_last_source_returns_texture_slot_to_pool() {
        let old_key = key(1);
        let mut cache = ImageGpuCache {
            entries: HashMap::from([(old_key, ImageEntry { texture_index: 3 })]),
            source_keys: HashMap::from([(7, old_key)]),
            key_refs: HashMap::from([(old_key, 1)]),
            next_index: 4,
            free_indices: Vec::new(),
        };

        assert_eq!(release_image_key(&mut cache, old_key), Some(3));
        assert!(!cache.entries.contains_key(&old_key));
        assert!(!cache.key_refs.contains_key(&old_key));
        assert_eq!(cache.free_indices, vec![3]);
    }

    #[test]
    fn retaining_another_source_keeps_shared_texture() {
        let shared_key = key(2);
        let mut cache = ImageGpuCache {
            entries: HashMap::from([(shared_key, ImageEntry { texture_index: 5 })]),
            source_keys: HashMap::new(),
            key_refs: HashMap::from([(shared_key, 2)]),
            next_index: 6,
            free_indices: Vec::new(),
        };

        assert_eq!(release_image_key(&mut cache, shared_key), None);
        assert_eq!(cache.key_refs.get(&shared_key), Some(&1));
        assert!(cache.entries.contains_key(&shared_key));
    }

    #[test]
    fn image_upload_reuses_shared_cpu_pixels() {
        let data = Arc::new(vec![7u8; 1024]);
        let upload = ImagePixels::Shared(&data).into_shared();

        assert!(Arc::ptr_eq(&data, &upload));
        assert_eq!(Arc::strong_count(&data), 2);
    }
}
