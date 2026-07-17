//! WGPU-based terminal rendering pipeline.
//!
//! Renders the visible terminal grid as instanced quads via
//! `egui_wgpu::CallbackTrait`.  Supports multiple glyph atlas textures
//! — each texture slot gets its own bind group and instances are drawn
//! in per-slot segments (see `AtlasRange`).

pub mod atlas;
pub mod callback;
pub mod shaders;

pub use callback::CallbackHandle;

use std::sync::atomic::{AtomicU32, Ordering};

use wgpu::util::DeviceExt;

use zenterm_core::Result;

use crate::shaders::{TERMINAL_FS, TERMINAL_VS};

/// Glyph type flags for per-instance shader dispatch.
///
/// These tell the fragment shader how to interpret the texture data
/// sampled from the glyph atlas.  Stored in the low 8 bits of
/// [`CellInstance::flags`]; the upper bits are reserved for
/// [`AtlasRange::atlas_index`] in the CPU grouping layer.
pub mod glyph_type {
    /// Default: LCD subpixel coverage (R=red, G=green, B=blue).
    /// The shader does per-channel `mix(bg, fg, coverage)`.
    pub const SUBPIXEL: u32 = 0;
    /// Grayscale alpha mask: R=G=B=A, use `max(r,g,b)` as alpha.
    /// Built-in block glyphs (▀▄▒▓█ etc.) use this path.
    pub const MASK: u32 = 1;
    /// Color glyph (emoji): texture holds actual RGBA premultiplied.
    /// Output directly without fg/bg mixing.
    pub const COLOR: u32 = 2;
    /// Solid color fill — no texture sampling. Outputs `bg_color`
    /// directly. Used for selection highlight and cursor backgrounds.
    pub const SOLID: u32 = 3;
    /// Full RGBA image — texture sample outputs premultiplied linear RGBA.
    /// Used for Kitty / iTerm / Sixel image placement.
    pub const IMAGE: u32 = 4;
}

/// Per-instance GPU data for one cell quad.
///
/// All spatial values are in **clip space** (NDC, range -1 to 1) so the
/// vertex shader can pass them directly through without knowing the
/// viewport size.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CellInstance {
    /// Clip-space top-left corner of the cell: (x, y).
    pub clip_pos: [f32; 2],
    /// Glyph UV coordinates of the lower-left corner: (u_min, v_min).
    pub uv_min: [f32; 2],
    /// Glyph UV coordinates of the upper-right corner: (u_max, v_max).
    pub uv_max: [f32; 2],
    /// Cell size in clip-space units: (width, height).
    pub clip_cell_size: [f32; 2],
    /// Glyph bitmap size in pixels: (width, height).
    /// Used to render the glyph at native resolution instead of stretching
    /// to fill the cell.
    pub glyph_size: [f32; 2],
    /// Glyph offset within the cell in pixels: (x, y).
    /// Computed from bearing_x and (cell_height - bearing_y) so the glyph
    /// is positioned on the baseline like a real terminal.
    pub glyph_offset: [f32; 2],
    /// Foreground colour (RGBA).
    pub fg_color: [f32; 4],
    /// Background colour (RGBA).
    pub bg_color: [f32; 4],
    /// Glyph type flag — one of [`glyph_type::SUBPIXEL`],
    /// [`glyph_type::MASK`], [`glyph_type::COLOR`], [`glyph_type::SOLID`].
    pub flags: u32,
}

/// Describes a contiguous range of instances in the GPU buffer that
/// belong to one atlas texture slot.
#[derive(Debug, Clone)]
pub struct AtlasRange {
    /// Index into [`TerminalRenderPass::atlas_bind_groups`].
    pub atlas_index: usize,
    /// Start offset (in instances) within the GPU instance buffer.
    pub start: u32,
    /// Number of instances in this range.
    pub count: u32,
}

/// The terminal render pass.
///
/// Owns the wgpu pipeline, static vertex/index buffers, a dynamically
/// written instance buffer, and one [`wgpu::BindGroup`] per atlas
/// texture slot.
pub struct TerminalRenderPass {
    pipeline: wgpu::RenderPipeline,
    instance_buf: wgpu::Buffer,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    /// Shared bind-group layout (one texture + one sampler).
    bind_group_layout: wgpu::BindGroupLayout,
    /// One bind group per atlas texture slot.
    atlas_bind_groups: Vec<wgpu::BindGroup>,
    /// Sampler shared by all atlas textures.
    atlas_sampler: wgpu::Sampler,
    /// Per-slot instance ranges for segmented drawing.
    atlas_ranges: Vec<AtlasRange>,
    num_instances: AtomicU32,
    max_instances: u32,
}

impl TerminalRenderPass {
    /// Create a new render pass.
    ///
    /// The pipeline is configured to render into a surface with the given
    /// `target_format`.  One bind group is created per `atlas_views`
    /// entry, all sharing the same [`wgpu::BindGroupLayout`] so the
    /// shader sees a uniform `texture_2d<f32>` at binding 0 regardless
    /// of which slot is active.
    pub fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        atlas_views: &[&wgpu::TextureView],
        sampler: &wgpu::Sampler,
    ) -> Result<Self> {
        let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminal.vs"),
            source: wgpu::ShaderSource::Wgsl(TERMINAL_VS.into()),
        });

        let fs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminal.fs"),
            source: wgpu::ShaderSource::Wgsl(TERMINAL_FS.into()),
        });

        // Full-screen quad vertices (two triangles).
        let vertices: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terminal.vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terminal.indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Instance buffer.
        let max_instances = 40_000u32;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terminal.instances"),
            size: (max_instances as u64) * std::mem::size_of::<CellInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Bind group layout (shared by all slots).
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("terminal.bind_group_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        // One bind group per texture view.
        let atlas_bind_groups: Vec<wgpu::BindGroup> = atlas_views
            .iter()
            .map(|view| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("terminal.atlas_bind_group"),
                    layout: &bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                    ],
                })
            })
            .collect();

        // Pipeline layout.
        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("terminal.pipeline_layout"),
                bind_group_layouts: &[Some(&bind_group_layout)],
                immediate_size: 0,
            });

        // Render pipeline with both vertex and instance buffer layouts.
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terminal.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vs_module,
                entry_point: Some("vs_main".into()),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    // Vertex buffer (per-vertex quad corners)
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<[f32; 2]>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        }],
                    },
                    // Instance buffer (per-instance cell data)
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<CellInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 8,
                                shader_location: 2,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 16,
                                shader_location: 3,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 24,
                                shader_location: 4,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 32,
                                shader_location: 5,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 40,
                                shader_location: 6,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 48,
                                shader_location: 7,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 64,
                                shader_location: 8,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Uint32,
                                offset: 80,
                                shader_location: 9,
                            },
                        ],
                    },
                ],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &fs_module,
                entry_point: Some("fs_main".into()),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            cache: None,
            multiview_mask: None,
        });
        Ok(Self {
            pipeline,
            instance_buf,
            vertex_buf,
            index_buf,
            bind_group_layout,
            atlas_bind_groups,
            atlas_sampler: sampler.clone(),
            atlas_ranges: Vec::new(),
            num_instances: AtomicU32::new(0),
            max_instances,
        })
    }

    /// Replace the set of atlas texture views (e.g. when a new slot
    /// was pushed onto the atlas).  Creates one bind group per view.
    pub fn update_atlas_views(
        &mut self,
        device: &wgpu::Device,
        atlas_views: &[&wgpu::TextureView],
    ) {
        self.atlas_bind_groups = atlas_views
            .iter()
            .map(|view| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("terminal.atlas_bind_group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.atlas_sampler),
                        },
                    ],
                })
            })
            .collect();
    }

    /// Set the per-slot instance ranges for segmented drawing.
    pub fn set_atlas_ranges(&mut self, ranges: Vec<AtlasRange>) {
        self.atlas_ranges = ranges;
    }

    /// Write a new set of cell instances to the GPU instance buffer.
    ///
    /// `instances` must not exceed `max_instances` (40 000).
    pub fn update_instances(&self, queue: &wgpu::Queue, instances: &[CellInstance]) {
        let count = instances.len() as u32;
        if count > self.max_instances {
            log::warn!(
                "instance count {} exceeds max {}, truncating",
                count,
                self.max_instances
            );
            let truncated = &instances[..self.max_instances as usize];
            queue.write_buffer(
                &self.instance_buf,
                0,
                bytemuck::cast_slice(truncated),
            );
            self.num_instances.store(self.max_instances, Ordering::Release);
            return;
        }
        if count > 0 {
            queue.write_buffer(
                &self.instance_buf,
                0,
                bytemuck::cast_slice(instances),
            );
        }
        self.num_instances.store(count, Ordering::Release);
    }

    /// Draw into an existing render pass.
    ///
    /// When multiple atlas slots are active the draw is segmented by
    /// [`AtlasRange`], binding the corresponding texture for each
    /// segment.  Instances not covered by any range (such as SOLID
    /// background and decoration quads that don't sample the atlas
    /// texture) are drawn with bind group 0.
    ///
    /// The common case (single slot, no ranges) takes the fast path
    /// with one bind + one draw call.
    pub fn draw_to_pass(&self, rpass: &mut wgpu::RenderPass) {
        let count = self.num_instances.load(Ordering::Acquire);
        if count == 0 {
            return;
        }
        rpass.set_pipeline(&self.pipeline);
        rpass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        rpass.set_vertex_buffer(1, self.instance_buf.slice(..));
        rpass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);

        if self.atlas_ranges.is_empty() || self.atlas_bind_groups.is_empty() {
            // Single-slot / empty fast path.
            if let Some(bg) = self.atlas_bind_groups.first() {
                rpass.set_bind_group(0, bg, &[]);
            }
            rpass.draw_indexed(0..6, 0, 0..count);
            return;
        }

        // Multi-slot segmented draw.  Gaps between ranges (flat
        // instances such as SOLID bg/deco that aren't in any range)
        // are drawn with bind group 0 — they don't sample the texture.
        let mut drawn_end = 0u32;
        for range in &self.atlas_ranges {
            if range.count == 0 {
                continue;
            }
            if range.atlas_index >= self.atlas_bind_groups.len() {
                log::warn!(
                    "draw_to_pass: atlas_index {} out of range ({} bind groups)",
                    range.atlas_index,
                    self.atlas_bind_groups.len()
                );
                continue;
            }

            // Draw gap before this range (flat instances).
            if range.start > drawn_end {
                rpass.set_bind_group(0, &self.atlas_bind_groups[0], &[]);
                rpass.draw_indexed(0..6, 0, drawn_end..range.start);
            }

            // Draw this atlas range with its texture.
            rpass.set_bind_group(0, &self.atlas_bind_groups[range.atlas_index], &[]);
            rpass.draw_indexed(0..6, 0, range.start..range.start + range.count);
            drawn_end = range.start + range.count;
        }

        // Draw remaining instances after the last range.
        if drawn_end < count {
            rpass.set_bind_group(0, &self.atlas_bind_groups[0], &[]);
            rpass.draw_indexed(0..6, 0, drawn_end..count);
        }
    }

    /// Maximum number of instances this render pass can hold.
    pub fn max_instances(&self) -> u32 {
        self.max_instances
    }
}
