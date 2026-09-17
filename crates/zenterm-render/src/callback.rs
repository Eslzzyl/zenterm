//! [`egui_wgpu::CallbackTrait`] implementation for the terminal renderer.
//!
//! Bridges the CPU-side terminal grid data (built in [`zenterm_ui::ZentermApp`])
//! with the GPU render pass via egui's custom callback mechanism.
//!
//! # Lifecycle
//!
//! 1. [`TerminalWgpuCallback`] is created once in `ZentermApp` and stored
//!    behind an `Arc<CallbackHandle>`.
//! 2. Each egui frame, `ZentermApp` builds `Vec<CellInstance>` from visible
//!    cells and stores them in [`SharedRenderState`].
//! 3. A [`CallbackHandle`] clone (cheap `Arc` bump) is passed to
//!    [`egui_wgpu::Callback::new_paint_callback`] each frame.
//! 4. Before egui's main render pass, `prepare()` uploads the instance data
//!    and (if dirty) the glyph atlas textures to the GPU.
//! 5. During egui's render pass, `paint()` binds the pipeline and draws the
//!    instanced quads — one segment per atlas slot.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use egui::PaintCallbackInfo;
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};

use crate::atlas::{
    create_atlas_sampler, create_atlas_texture, create_image_texture, update_atlas_texture_region,
};
use crate::{AtlasRange, CellInstance, TerminalRenderPass};

/// Thread-safe shared state between `ZentermApp` (updates each frame) and
/// [`TerminalWgpuCallback`] (uploads to GPU each frame).
pub struct SharedRenderState {
    /// Per-frame instance data: cell instances + atlas ranges behind a
    /// single Mutex so callers lock once instead of acquiring two locks.
    pub frame_data: Mutex<FrameData>,
    /// Monotonically increasing generation counter.  Incremented by the UI
    /// thread whenever `instances` changes.  `prepare()` compares this with
    /// its local copy to decide whether a GPU buffer upload is needed.
    pub instance_gen: AtomicU64,
    /// Set to `true` when the glyph atlas pixel data has changed.
    pub atlas_dirty: AtomicBool,
    /// When `atlas_dirty` is true, this holds the per-slot data so
    /// `prepare()` can upload or recreate textures.
    pub atlas_update: Mutex<Option<AtlasUpdate>>,
    /// Pending terminal-image texture creations or releases.
    pub image_update: Mutex<Option<ImageTextureUpdate>>,
    /// Pending background image upload.  Written by the UI thread when the
    /// user sets/changes the background image; consumed by `prepare()`.
    pub background_data: Mutex<Option<BackgroundImageData>>,
    /// Monotonic request generation used to discard stale async decodes.
    pub background_request_gen: AtomicU64,
    /// Monotonically increasing generation counter for background texture
    /// changes.  `prepare()` checks this to decide whether to upload.
    pub background_gen: AtomicU64,
}

/// Pixel data for a background image to be uploaded to the GPU.
pub struct BackgroundImageData {
    /// RGBA8 sRGB pixel data (4 bytes per pixel).
    pub data: Vec<u8>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

/// Updates for exact-size terminal image textures.
pub struct ImageTextureUpdate {
    /// New image textures keyed by their stable texture index.
    pub uploads: Vec<ImageTextureData>,
    /// Image texture indices that no longer have a live CPU image reference.
    pub releases: Vec<usize>,
}

/// One exact-size RGBA image to upload to the GPU.
pub struct ImageTextureData {
    pub index: usize,
    pub width: u32,
    pub height: u32,
    /// Shared with the terminal's CPU image cache to avoid another full
    /// image-sized staging allocation while the upload is pending.
    pub data: Arc<Vec<u8>>,
}

/// Per-frame instance data shared between the UI thread and GPU prepare.
///
/// Both fields are locked together under a single [`Mutex`] so that
/// the clear → populate → consume cycle acquires one lock instead of
/// two, reducing lock contention on every frame.
pub struct FrameData {
    /// Cell instance data for the current frame — built by the UI thread,
    /// consumed by `prepare()`.
    pub instances: Vec<CellInstance>,
    /// Per-atlas-slot instance ranges describing which instances in
    /// `instances` belong to which atlas texture.
    pub atlas_ranges: Vec<AtlasRange>,
    /// When `true`, instance 0 is a BACKGROUND quad sampled from
    /// @group(1) instead of the glyph atlas.  Must be set by the UI
    /// thread every frame before the instance data is consumed.
    pub background_active: bool,
}

const INSTANCE_HIGH_WATER_CAPACITY: usize = 16 * 1024;

impl FrameData {
    /// Clear per-frame instances and release capacity left by a large frame.
    ///
    /// The previous frame length is used as the next retained size, so a
    /// one-off large render does not permanently pin a large allocation.
    pub fn clear_for_next_frame(&mut self) {
        let previous_len = self.instances.len();
        let retained = previous_len.max(80 * 24);
        if self.instances.capacity() >= INSTANCE_HIGH_WATER_CAPACITY
            && self.instances.capacity() > retained.saturating_mul(2)
        {
            self.instances.shrink_to(retained);
        }
        self.instances.clear();
        self.atlas_ranges.clear();
    }
}

/// Pixel data for one slot in the texture atlas.
#[derive(Debug, Clone)]
pub struct AtlasSlotData {
    /// Stable atlas slot index represented by this full texture payload.
    pub index: u32,
    /// Width and height of this slot (square, power of two).
    pub size: u32,
    /// RGBA pixel data of the full slot texture.
    pub data: Vec<u8>,
}

/// Pixel payload for a changed rectangular atlas region.
#[derive(Debug)]
pub struct AtlasRegionData {
    pub atlas_index: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Rows are padded to wgpu's copy-row alignment.
    pub data: Vec<u8>,
}

/// Payload for a glyph atlas texture update.
pub struct AtlasUpdate {
    /// Full payloads for newly created or rebuilt slots.
    pub slots: Vec<AtlasSlotData>,
    /// Partial payloads for already-existing slots.
    pub regions: Vec<AtlasRegionData>,
    /// Total number of slots after this update.
    pub total_slots: u32,
}

impl SharedRenderState {
    /// Create a new shared state with the given initial instance capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            frame_data: Mutex::new(FrameData {
                instances: Vec::with_capacity(capacity),
                atlas_ranges: Vec::new(),
                background_active: false,
            }),
            instance_gen: AtomicU64::new(1),
            atlas_dirty: AtomicBool::new(false),
            atlas_update: Mutex::new(None),
            image_update: Mutex::new(None),
            background_data: Mutex::new(None),
            background_request_gen: AtomicU64::new(0),
            background_gen: AtomicU64::new(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_data_releases_a_large_instance_capacity() {
        let mut frame_data = FrameData {
            instances: Vec::with_capacity(32 * 1024),
            atlas_ranges: Vec::new(),
            background_active: false,
        };
        let old_capacity = frame_data.instances.capacity();

        frame_data.clear_for_next_frame();

        assert_eq!(frame_data.instances.len(), 0);
        assert!(frame_data.instances.capacity() < old_capacity);
    }
}

/// The `egui_wgpu::CallbackTrait` implementation that bridges terminal
/// rendering into egui's wgpu pipeline.
///
/// Create one via [`TerminalWgpuCallback::new`], then wrap it with
/// [`egui_wgpu::Callback::new_paint_callback`] each frame.
pub struct TerminalWgpuCallback {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target_format: wgpu::TextureFormat,

    /// Lazily created on first `prepare()`.
    render_pass: Mutex<Option<TerminalRenderPass>>,
    /// One GPU texture per atlas slot (index matches slot index).
    atlas_textures: Mutex<Vec<wgpu::Texture>>,
    /// Views for the corresponding textures.
    atlas_views: Mutex<Vec<wgpu::TextureView>>,
    /// Exact-size terminal image textures.
    image_textures: Mutex<Vec<wgpu::Texture>>,
    /// Views for terminal image textures.
    image_views: Mutex<Vec<wgpu::TextureView>>,
    atlas_sampler: wgpu::Sampler,

    /// Number of atlas slots known to the GPU side.  When the UI thread
    /// reports more slots than this we recreate the bind groups.
    current_slot_count: AtomicU32,

    /// Shared state with the UI thread.
    shared: Arc<SharedRenderState>,

    /// Last seen instance generation — used to skip GPU upload when the
    /// instance data hasn't changed (terminal is idle).
    last_instance_gen: AtomicU64,
}

#[derive(Clone, Copy)]
enum PaintMode {
    All,
    Background,
    Cells,
}

impl TerminalWgpuCallback {
    /// Create a new terminal wgpu callback.
    ///
    /// The sampler is created immediately; the pipeline and atlas textures
    /// are created lazily on the first `prepare()` call so the caller
    /// does not need the wgpu device/queue until the first frame.
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_format: wgpu::TextureFormat,
        shared: Arc<SharedRenderState>,
    ) -> Self {
        let atlas_sampler = create_atlas_sampler(&device);

        Self {
            device,
            queue,
            target_format,
            render_pass: Mutex::new(None),
            atlas_textures: Mutex::new(Vec::new()),
            atlas_views: Mutex::new(Vec::new()),
            image_textures: Mutex::new(Vec::new()),
            image_views: Mutex::new(Vec::new()),
            atlas_sampler,
            current_slot_count: AtomicU32::new(0),
            shared,
            last_instance_gen: AtomicU64::new(0),
        }
    }
}

impl CallbackTrait for TerminalWgpuCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        _callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        // ── 1. Update glyph atlas textures if dirty ──────────────────
        if self.shared.atlas_dirty.load(Ordering::Acquire) {
            let update = self.shared.atlas_update.lock().unwrap().take();
            if let Some(update) = update {
                log::debug!(
                    "callback prepare: atlas update total_slots={}, full_slots={}, regions={}",
                    update.total_slots,
                    update.slots.len(),
                    update.regions.len(),
                );

                let full_snapshot = update.slots.len() == update.total_slots as usize
                    && update.slots.first().is_some_and(|slot| slot.index == 0);
                let atlas_changed = full_snapshot
                    || update.total_slots != self.current_slot_count.load(Ordering::Relaxed);

                // Ensure we have GPU textures + views for every slot.
                {
                    let mut textures = self.atlas_textures.lock().unwrap();
                    let mut views = self.atlas_views.lock().unwrap();

                    if full_snapshot {
                        textures.clear();
                        views.clear();
                    }

                    for slot_data in &update.slots {
                        let i = slot_data.index as usize;
                        if i == textures.len() {
                            // Create a new texture + view for this slot.
                            let (tex, view) = create_atlas_texture(
                                &self.device,
                                &self.queue,
                                slot_data.size,
                                &slot_data.data,
                            );
                            textures.push(tex);
                            views.push(view);
                        }
                    }

                    for region in &update.regions {
                        if let Some(tex) = textures.get(region.atlas_index as usize) {
                            update_atlas_texture_region(
                                &self.queue,
                                tex,
                                region.x,
                                region.y,
                                region.width,
                                region.height,
                                &region.data,
                            );
                        }
                    }

                    // A full snapshot is the only operation that can shrink
                    // the CPU slot list (e.g. DPI reinitialisation).
                    if full_snapshot {
                        textures.truncate(update.total_slots as usize);
                        views.truncate(update.total_slots as usize);
                    }
                }

                if atlas_changed {
                    self.current_slot_count
                        .store(update.total_slots, Ordering::Relaxed);

                    // Recreate bind groups in the render pass.
                    let views = self.atlas_views.lock().unwrap();
                    let view_refs: Vec<&wgpu::TextureView> = views.iter().collect();
                    if let Ok(mut rp_guard) = self.render_pass.lock() {
                        match rp_guard.as_mut() {
                            Some(rp) => {
                                rp.update_atlas_views(&self.device, &view_refs);
                                log::debug!(
                                    "callback prepare: updated bind groups for {} slots",
                                    view_refs.len()
                                );
                            }
                            None => {
                                // First frame — create the render pass.
                                let image_views = self.image_views.lock().unwrap();
                                let image_view_refs: Vec<&wgpu::TextureView> =
                                    image_views.iter().collect();
                                *rp_guard = Some(
                                    TerminalRenderPass::new(
                                        &self.device,
                                        self.target_format,
                                        &view_refs,
                                        &image_view_refs,
                                        &self.atlas_sampler,
                                    )
                                    .expect("failed to create TerminalRenderPass"),
                                );
                                log::debug!("callback prepare: created render pass");
                            }
                        }
                    }
                }
            }
            self.shared.atlas_dirty.store(false, Ordering::Release);
        } else {
            log::trace!("callback prepare: atlas not dirty");
        }

        // ── 1b. Update exact-size terminal image textures ───────────────
        if let Some(update) = self.shared.image_update.lock().unwrap().take() {
            let mut textures = self.image_textures.lock().unwrap();
            let mut views = self.image_views.lock().unwrap();

            for index in update.releases {
                let (texture, view) =
                    create_image_texture(&self.device, &self.queue, 1, 1, &[0; 4]);
                ensure_image_texture_slot(&mut textures, &mut views, index, texture, view);
            }

            for image in update.uploads {
                let (texture, view) = create_image_texture(
                    &self.device,
                    &self.queue,
                    image.width,
                    image.height,
                    &image.data,
                );
                ensure_image_texture_slot(&mut textures, &mut views, image.index, texture, view);
            }

            if let Ok(mut rp_guard) = self.render_pass.lock()
                && let Some(ref mut rp) = *rp_guard
            {
                let view_refs: Vec<&wgpu::TextureView> = views.iter().collect();
                rp.update_image_views(&self.device, &view_refs);
            }
        }

        // ── 1c. Update background texture if dirty ─────────────────────
        {
            let _t0 = std::time::Instant::now();
            let update = self.shared.background_data.lock().unwrap().take();
            if let Some(bg_data) = update {
                log::debug!(
                    "callback prepare: background image update {}x{}",
                    bg_data.width,
                    bg_data.height,
                );
                if let Ok(mut rp_guard) = self.render_pass.lock()
                    && let Some(ref mut rp) = *rp_guard
                {
                    rp.update_background_texture(
                        &self.device,
                        &self.queue,
                        &bg_data.data,
                        bg_data.width,
                        bg_data.height,
                    );
                }
                self.shared.background_gen.fetch_add(1, Ordering::Release);
                log::debug!("bg: prepare total took {:?}", _t0.elapsed());
            }
        }

        // ── 2. Upload cell instance data ────────────────────────────────
        let current_gen = self.shared.instance_gen.load(Ordering::Acquire);
        let last_gen = self.last_instance_gen.load(Ordering::Relaxed);
        if current_gen != last_gen {
            self.last_instance_gen.store(current_gen, Ordering::Relaxed);

            // Single lock for both instances + atlas_ranges + background_active.
            let mut guard = self.shared.frame_data.lock().unwrap();
            let instances = std::mem::take(&mut guard.instances);
            let atlas_ranges = std::mem::take(&mut guard.atlas_ranges);
            let background_active = guard.background_active;
            drop(guard);

            if !instances.is_empty()
                && let Ok(rp_guard) = self.render_pass.lock()
            {
                if let Some(ref rp) = *rp_guard {
                    rp.update_instances(&self.queue, &instances);
                    log::trace!(
                        "callback prepare: uploaded {} instances (gen {})",
                        instances.len(),
                        current_gen,
                    );
                } else {
                    log::warn!("callback prepare: render_pass is None, cannot upload instances");
                }
            }

            // Pass atlas ranges and background_active to the render pass.
            if (!atlas_ranges.is_empty() || background_active)
                && let Ok(mut rp_guard) = self.render_pass.lock()
                && let Some(ref mut rp) = *rp_guard
            {
                rp.set_atlas_ranges(atlas_ranges);
                rp.set_background_active(background_active);
            }

            // Reuse the instance allocation on the next frame.  If the UI
            // thread has already started a newer frame, keep that data and
            // let this upload buffer drop instead of overwriting it.
            let mut guard = self.shared.frame_data.lock().unwrap();
            if guard.instances.is_empty() {
                guard.instances = instances;
            }
        } else {
            log::trace!(
                "callback prepare: instances unchanged (gen {}), skipping upload",
                last_gen
            );
        }

        vec![]
    }

    fn finish_prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _egui_encoder: &mut wgpu::CommandEncoder,
        _callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        vec![]
    }

    fn paint(
        &self,
        info: PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        self.paint_with_mode(info, render_pass, callback_resources, PaintMode::All);
    }
}

impl TerminalWgpuCallback {
    fn paint_with_mode(
        &self,
        _info: PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        _callback_resources: &CallbackResources,
        mode: PaintMode,
    ) {
        if let Ok(rp_guard) = self.render_pass.lock()
            && let Some(ref rp) = *rp_guard
        {
            match mode {
                PaintMode::All => rp.draw_to_pass(render_pass),
                PaintMode::Background => rp.draw_background_to_pass(render_pass),
                PaintMode::Cells => rp.draw_cells_to_pass(render_pass),
            }
        }
    }
}

/// Thread-safe handle to the terminal wgpu callback.
///
/// Cheaply cloneable (`Arc` bump).  A clone is passed to
/// `egui_wgpu::Callback::new_paint_callback` each frame.
#[derive(Clone)]
pub struct CallbackHandle {
    inner: Arc<TerminalWgpuCallback>,
    mode: PaintMode,
}

impl CallbackHandle {
    /// Wrap a [`TerminalWgpuCallback`] behind an `Arc`.
    pub fn new(callback: TerminalWgpuCallback) -> Self {
        Self {
            inner: Arc::new(callback),
            mode: PaintMode::All,
        }
    }

    /// Create a callback that paints only the background quad.
    pub fn background_only(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            mode: PaintMode::Background,
        }
    }

    /// Create a callback that paints only terminal cell instances.
    pub fn cells_only(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            mode: PaintMode::Cells,
        }
    }
}

impl CallbackTrait for CallbackHandle {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        self.inner.prepare(
            device,
            queue,
            screen_descriptor,
            egui_encoder,
            callback_resources,
        )
    }

    fn finish_prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        self.inner
            .finish_prepare(device, queue, egui_encoder, callback_resources)
    }

    fn paint(
        &self,
        info: PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        self.inner
            .paint_with_mode(info, render_pass, callback_resources, self.mode)
    }
}

fn ensure_image_texture_slot(
    textures: &mut Vec<wgpu::Texture>,
    views: &mut Vec<wgpu::TextureView>,
    index: usize,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
) {
    if index == textures.len() {
        textures.push(texture);
        views.push(view);
    } else if index < textures.len() {
        textures[index] = texture;
        views[index] = view;
    } else {
        log::warn!(
            "image texture index {} has a gap after {} slots",
            index,
            textures.len()
        );
    }
}
