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
//!    and (if dirty) the glyph atlas texture to the GPU.
//! 5. During egui's render pass, `paint()` binds the pipeline and draws the
//!    instanced quads.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use egui::PaintCallbackInfo;
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};

use crate::atlas::{create_atlas_sampler, create_atlas_texture, update_atlas_texture};
use crate::{CellInstance, TerminalRenderPass};

/// Thread-safe shared state between `ZentermApp` (updates each frame) and
/// [`TerminalWgpuCallback`] (uploads to GPU each frame).
pub struct SharedRenderState {
    /// Cell instance data for the current frame — built by the UI thread,
    /// consumed by `prepare()`.
    pub instances: Mutex<Vec<CellInstance>>,
    /// Monotonically increasing generation counter.  Incremented by the UI
    /// thread whenever `instances` changes.  `prepare()` compares this with
    /// its local copy to decide whether a GPU buffer upload is needed.
    pub instance_gen: AtomicU64,
    /// Set to `true` when the glyph atlas pixel data has changed.
    pub atlas_dirty: AtomicBool,
    /// When `atlas_dirty` is true, this holds the new atlas size and pixel
    /// data so `prepare()` can re-upload.
    pub atlas_update: Mutex<Option<AtlasUpdate>>,
}

/// Payload for a glyph atlas texture update.
pub struct AtlasUpdate {
    /// Current atlas texture size (width == height, power of two).
    pub size: u32,
    /// RGBA pixel data of the full atlas.
    pub data: Vec<u8>,
    /// Whether the atlas was resized (requires full texture recreation).
    pub resized: bool,
}

impl SharedRenderState {
    /// Create a new shared state with the given initial instance capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            instances: Mutex::new(Vec::with_capacity(capacity)),
            instance_gen: AtomicU64::new(1),
            atlas_dirty: AtomicBool::new(false),
            atlas_update: Mutex::new(None),
        }
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
    /// Atlas GPU texture — recreated on atlas resize.
    atlas_texture: Mutex<Option<wgpu::Texture>>,
    atlas_view: Mutex<Option<wgpu::TextureView>>,
    atlas_sampler: wgpu::Sampler,

    /// Current atlas size (pixels, width == height).  Used to detect
    /// atlas growth that requires a texture recreation.
    current_atlas_size: AtomicU32,

    /// Shared state with the UI thread.
    shared: Arc<SharedRenderState>,

    /// Last seen instance generation — used to skip GPU upload when the
    /// instance data hasn't changed (terminal is idle).
    last_instance_gen: AtomicU64,
}

impl TerminalWgpuCallback {
    /// Create a new terminal wgpu callback.
    ///
    /// The sampler is created immediately; the pipeline and atlas texture
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
            atlas_texture: Mutex::new(None),
            atlas_view: Mutex::new(None),
            atlas_sampler,
            current_atlas_size: AtomicU32::new(0),
            shared,
            last_instance_gen: AtomicU64::new(0), // First frame: 1 ≠ 0 → upload.
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
        // ── 1. Update glyph atlas texture if dirty ──────────────────────
        if self.shared.atlas_dirty.load(Ordering::Acquire) {
            let update = self.shared.atlas_update.lock().unwrap().take();
            if let Some(update) = update {
                log::debug!(
                    "callback prepare: atlas update size={} resized={}",
                    update.size,
                    update.resized,
                );
                if update.resized || self.current_atlas_size.load(Ordering::Relaxed) != update.size
                {
                    // Atlas grew — recreate the texture + view.
                    let (tex, view) = create_atlas_texture(
                        &self.device,
                        &self.queue,
                        update.size,
                        &update.data,
                    );
                    self.current_atlas_size.store(update.size, Ordering::Relaxed);

                    // Recreate the render pass with the new texture view.
                    if let Ok(mut rp_guard) = self.render_pass.lock() {
                        *rp_guard = Some(
                            TerminalRenderPass::new(
                                &self.device,
                                self.target_format,
                                &view,
                                &self.atlas_sampler,
                            )
                            .expect("failed to recreate TerminalRenderPass after atlas resize"),
                        );
                    }
                    *self.atlas_texture.lock().unwrap() = Some(tex);
                    *self.atlas_view.lock().unwrap() = Some(view);
                    log::debug!("callback prepare: created atlas texture + render pass");
                } else {
                    // Same size — just upload new pixels.
                    if let Some(ref tex) = *self.atlas_texture.lock().unwrap() {
                        update_atlas_texture(&self.queue, tex, update.size, &update.data);
                        log::debug!("callback prepare: updated atlas texture pixels");
                    }
                }
            }
            self.shared.atlas_dirty.store(false, Ordering::Release);
        } else {
            log::trace!("callback prepare: atlas not dirty");
        }

        // ── 2. Upload cell instance data ────────────────────────────────
        let current_gen = self.shared.instance_gen.load(Ordering::Acquire);
        let last_gen = self.last_instance_gen.load(Ordering::Relaxed);
        if current_gen != last_gen {
            // Instances changed since last frame — upload to GPU buffer.
            self.last_instance_gen.store(current_gen, Ordering::Relaxed);

            // Take ownership instead of cloning — the UI thread has
            // already bumped instance_gen to signal that this frame's
            // data is ready, and won't touch the Vec again until the
            // next frame's clear_instances().
            let instances = {
                let mut guard = self.shared.instances.lock().unwrap();
                std::mem::take(&mut *guard)
            };
            if !instances.is_empty() {
                if let Ok(rp_guard) = self.render_pass.lock() {
                    if let Some(ref rp) = *rp_guard {
                        rp.update_instances(&self.queue, &instances);
                        log::trace!(
                            "callback prepare: uploaded {} instances (gen {})",
                            instances.len(),
                            current_gen,
                        );
                    } else {
                        log::warn!(
                            "callback prepare: render_pass is None, cannot upload instances"
                        );
                    }
                }
            } else {
                log::trace!("callback prepare: no instances to upload");
            }
        } else {
            log::trace!(
                "callback prepare: instances unchanged (gen {}), skipping upload",
                current_gen,
            );
        }

        Vec::new()
    }

    fn paint(
        &self,
        _info: PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        _callback_resources: &CallbackResources,
    ) {
        if let Ok(rp_guard) = self.render_pass.lock() {
            if let Some(ref rp) = *rp_guard {
                rp.draw_to_pass(render_pass);
                log::trace!("callback paint: drew instances");
            } else {
                log::warn!("callback paint: render_pass is None, skipping draw");
            }
        }
    }
}

// Safety: all interior mutability uses `Mutex` and `Atomic*`.
// wgpu types are `Send + Sync`.
unsafe impl Send for TerminalWgpuCallback {}
unsafe impl Sync for TerminalWgpuCallback {}

/// A cheaply-cloneable handle to a [`TerminalWgpuCallback`].
///
/// [`egui_wgpu::Callback::new_paint_callback`] takes ownership of the
/// callback each frame, so we wrap it in an `Arc` and hand out clones.
#[derive(Clone)]
pub struct CallbackHandle {
    inner: Arc<TerminalWgpuCallback>,
}

impl CallbackHandle {
    /// Wrap a [`TerminalWgpuCallback`] behind an `Arc`.
    pub fn new(callback: TerminalWgpuCallback) -> Self {
        Self {
            inner: Arc::new(callback),
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
        self.inner
            .prepare(device, queue, screen_descriptor, egui_encoder, callback_resources)
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
        self.inner.paint(info, render_pass, callback_resources)
    }
}
