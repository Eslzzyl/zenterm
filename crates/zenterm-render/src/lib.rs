//! WGPU-based terminal rendering pipeline.
//!
//! Renders the visible terminal grid as instanced quads via
//! `egui_wgpu::CallbackTrait`.  Supports multiple glyph atlas textures
//! — each texture slot gets its own bind group and instances are drawn
//! in per-slot segments (see `AtlasRange`).

pub mod atlas;
pub mod callback;
pub mod shaders;

pub use callback::{BackgroundImageData, CallbackHandle, FrameData};

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
    /// Full-viewport background image quad — sampled from background_atlas
    /// (@group(1)), blended with theme background colour using uniforms
    /// packed in fg_color.a (image_opacity) and bg_color (window_opacity + colour).
    pub const BACKGROUND: u32 = 5;
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
/// written instance buffer, one [`wgpu::BindGroup`] per atlas texture slot
/// (@group(0)), and an optional background-image bind group (@group(1)).
pub struct TerminalRenderPass {
    pipeline: wgpu::RenderPipeline,
    instance_buf: wgpu::Buffer,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    /// Shared bind-group layout for atlas slots (texture + sampler) @group(0).
    bind_group_layout: wgpu::BindGroupLayout,
    /// One bind group per atlas texture slot.
    atlas_bind_groups: Vec<wgpu::BindGroup>,
    /// Sampler shared by all atlas textures.
    atlas_sampler: wgpu::Sampler,
    /// Linear sampler for the background image (avoids aliasing).
    bg_sampler: wgpu::Sampler,
    /// Per-slot instance ranges for segmented drawing.
    atlas_ranges: Vec<AtlasRange>,
    num_instances: AtomicU32,
    max_instances: u32,

    // ── Background image (optional quad behind all cells) ─────────────
    /// Bind group layout for background image texture + sampler @group(1).
    background_bind_group_layout: wgpu::BindGroupLayout,
    /// Bind group for background image (or 1×1 dummy when inactive).
    background_bind_group: wgpu::BindGroup,
    /// Whether the current frame has a background quad at instance 0.
    background_active: bool,
    /// The uploaded background texture (kept alive).
    background_texture: Option<wgpu::Texture>,
    /// View into `background_texture`.
    background_view: Option<wgpu::TextureView>,
}

impl TerminalRenderPass {
    /// Create a new render pass.
    ///
    /// The pipeline is configured to render into a surface with the given
    /// `target_format`.  One bind group is created per `atlas_views`
    /// entry, all sharing the same [`wgpu::BindGroupLayout`] so the
    /// shader sees a uniform `texture_2d<f32>` at binding 0 regardless
    /// of which slot is active.
    ///
    /// A second bind group layout is created for the optional background
    /// image texture (@group(1) in the shader).  A dummy 1×1 white texture
    /// is bound so the pipeline is valid even when no background image
    /// is loaded.
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

        // ── @group(0): glyph atlas bind group layout ────────────────
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

        // ── Linear sampler for background image ─────────────────────
        let bg_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terminal.bg_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        // ── @group(1): background image bind group layout ───────────
        let background_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("terminal.background_bind_group_layout"),
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

        // Create a 1×1 dummy white texture so the background bind group
        // is always valid (wgpu requires all bind groups in the pipeline
        // layout to be bound for every draw call).
        let dummy_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terminal.background_dummy"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let dummy_view = dummy_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let background_bind_group =
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terminal.background_bind_group_dummy"),
                layout: &background_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&dummy_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&bg_sampler),
                    },
                ],
            });

        // ── Pipeline layout (two bind group layouts) ────────────────
        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("terminal.pipeline_layout"),
                bind_group_layouts: &[
                    Some(&bind_group_layout),
                    Some(&background_bind_group_layout),
                ],
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
            bg_sampler,
            atlas_ranges: Vec::new(),
            num_instances: AtomicU32::new(0),
            max_instances,
            background_bind_group_layout,
            background_bind_group,
            background_active: false,
            background_texture: Some(dummy_tex),
            background_view: Some(dummy_view),
        })
    }

    /// Signal whether the current frame has a background quad at instance 0.
    pub fn set_background_active(&mut self, active: bool) {
        self.background_active = active;
    }

    /// Upload a new background image texture and rebuild @group(1).
    ///
    /// `data` must be RGBA8 sRGB pixel data, `width` × `height`.
    pub fn update_background_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
        width: u32,
        height: u32,
    ) {
        let _t0 = std::time::Instant::now();
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terminal.background"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        log::debug!("bg: create_texture {:?}", _t0.elapsed());
        // Pad bytes_per_row to 256-byte alignment (required by wgpu).
        let _t1 = std::time::Instant::now();
        let unpadded = width * 4;
        let padding = (256 - unpadded % 256) % 256;
        let bytes_per_row = unpadded + padding;
        let padded_size = bytes_per_row as u64 * height as u64;
        // Build a padded copy so wgpu doesn't reject the upload on
        // backends that require tight alignment (D3D12, Vulkan).
        let padded = if padding > 0 {
            let mut buf = Vec::with_capacity(padded_size as usize);
            for row in 0..height as usize {
                buf.extend_from_slice(&data[row * unpadded as usize..(row + 1) * unpadded as usize]);
                buf.extend(std::iter::repeat(0u8).take(padding as usize));
            }
            buf
        } else {
            data.to_vec()
        };
        log::debug!("bg: pad/copy {}x{} (pad={}) took {:?}", width, height, padding, _t1.elapsed());
        let _t2 = std::time::Instant::now();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &padded,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        log::debug!("bg: write_texture took {:?}", _t2.elapsed());

        let _t3 = std::time::Instant::now();
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terminal.background_bind_group"),
            layout: &self.background_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.bg_sampler),
                },
            ],
        });
        log::debug!("bg: create_bind_group took {:?}", _t3.elapsed());
        log::debug!("bg: total GPU update took {:?}", _t0.elapsed());

        self.background_bind_group = bg;
        self.background_texture = Some(tex);
        self.background_view = Some(view);
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
    /// When `background_active` is true, instance 0 is a BACKGROUND
    /// quad sampled from `@group(1)` (the background image texture).
    /// It is drawn first with any valid `@group(0)` (it doesn't sample
    /// glyph_atlas).  Cell instances start at offset 1.
    pub fn draw_to_pass(&self, rpass: &mut wgpu::RenderPass) {
        let count = self.num_instances.load(Ordering::Acquire);
        if count == 0 {
            return;
        }
        rpass.set_pipeline(&self.pipeline);
        rpass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        rpass.set_vertex_buffer(1, self.instance_buf.slice(..));
        rpass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);

        // Bind @group(1) once for all draw calls (background texture
        // or 1×1 dummy).  Cell-instance draw paths do not sample it.
        rpass.set_bind_group(1, &self.background_bind_group, &[]);

        let bg_active = self.background_active;
        let start: u32 = if bg_active { 1 } else { 0 };

        // ── Draw background quad (instance 0) ───────────────────────
        if bg_active {
            // Bind any valid @group(0) — the BACKGROUND shader path
            // does not sample glyph_atlas, but wgpu validation requires
            // the binding to be present.
            if let Some(bg) = self.atlas_bind_groups.first() {
                rpass.set_bind_group(0, bg, &[]);
            }
            rpass.draw_indexed(0..6, 0, 0..1);
        }

        if count <= start {
            return;
        }

        // ── Draw cell instances (offset by `start`) ─────────────────
        if self.atlas_ranges.is_empty() || self.atlas_bind_groups.is_empty() {
            // Single-slot / empty fast path.
            if let Some(bg) = self.atlas_bind_groups.first() {
                rpass.set_bind_group(0, bg, &[]);
            }
            rpass.draw_indexed(0..6, 0, start..count);
            return;
        }

        // Multi-slot segmented draw.  Gaps between ranges (flat
        // instances such as SOLID bg/deco that aren't in any range)
        // are drawn with bind group 0 — they don't sample the texture.
        let mut drawn_end = start;
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
