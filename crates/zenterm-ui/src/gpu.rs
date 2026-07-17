//! Shared GPU context.
//!
//! A single [`SharedGpuContext`] is created when the application
//! starts and passed by reference to every [`TerminalSession`].  This
//! avoids per-session `wgpu::Device/Queue` clones (the device handle
//! is internally `Arc`-cloned by wgpu, but explicit ownership of the
//! Arc here makes it easier to reason about lifetimes and to insert
//! future GPU resource pools, fences, or async work submission).
//!
//! # Why share the device
//!
//! All wgpu resources — pipelines, bind groups, buffers, textures —
//! are tied to a specific `wgpu::Device` and `wgpu::Queue`.  Sharing
//! one pair across the entire application:
//!
//! 1. Lets the existing [`TerminalWgpuCallback`]'s `prepare()` see a
//!    single atlas + a single instance buffer that already contains
//!    every session's cells, so the wgpu draw call count stays at
//!    **one** regardless of the number of tabs.
//! 2. Avoids the cost of re-creating pipelines / bind groups per
//!    session.
//! 3. Matches wgpu's design intent: one logical device per process.
//!
//! # Shared render state
//!
//! The context also owns the `Arc<SharedRenderState>` shared between
//! the per-session callbacks and the central wgpu paint callback.
//! Each session appends cell instances to
//! [`SharedRenderState::instances`](zenterm_render::callback::SharedRenderState::instances)
//! during its own `update_cell_instances` call, and the central
//! callback consumes the concatenated buffer in its `prepare()`.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use zenterm_render::callback::SharedRenderState;
use zenterm_render::AtlasRange;

/// Application-wide GPU handle set, shared by reference across all
/// terminal sessions and the wgpu paint callback.
#[derive(Clone)]
pub struct SharedGpuContext {
    /// Logical wgpu device.  Cloning the handle is cheap; all
    /// pipelines, buffers, and textures are allocated through it.
    pub device: Arc<wgpu::Device>,

    /// Submission queue.  All GPU work (atlas upload, instance buffer
    /// update, draw calls) is enqueued here.
    pub queue: Arc<wgpu::Queue>,

    /// Target colour format of the eframe swap-chain surface.
    /// Used by [`TerminalWgpuCallback`] when creating the render
    /// pipeline so that output is correctly interpreted as sRGB
    /// (or linear, depending on eframe's configuration).
    pub target_format: wgpu::TextureFormat,

    /// Shared instance buffer + atlas upload channel.  One per
    /// application; every session appends cell instances to it.
    pub shared: Arc<SharedRenderState>,
}

impl SharedGpuContext {
    /// Build a new shared context from raw wgpu handles and an
    /// already-constructed shared render state.
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_format: wgpu::TextureFormat,
        shared: Arc<SharedRenderState>,
    ) -> Self {
        Self {
            device: Arc::new(device),
            queue: Arc::new(queue),
            target_format,
            shared,
        }
    }

    /// Clear the shared instance buffer.  Call once per frame,
    /// before any session writes its instances.
    pub fn clear_instances(&self) {
        let mut buf = self.shared.instances.lock().expect("instances poisoned");
        buf.clear();
    }

    /// Clear the shared atlas ranges.  Call alongside `clear_instances`
    /// at the start of each frame.
    pub fn clear_atlas_ranges(&self) {
        let mut buf = self.shared.atlas_ranges.lock().expect("atlas_ranges poisoned");
        buf.clear();
    }

    /// Append an atlas range to the shared list.  Called by each
    /// session after it appends its glyph instances.
    pub fn push_atlas_range(&self, range: AtlasRange) {
        let mut buf = self.shared.atlas_ranges.lock().expect("atlas_ranges poisoned");
        buf.push(range);
    }

    /// Bump the instance generation counter.  Call once per frame
    /// after all sessions have appended their instances.  The
    /// wgpu paint callback uses this to decide whether to re-upload.
    pub fn bump_instance_gen(&self) {
        self.shared.instance_gen.fetch_add(1, Ordering::Release);
    }
}
