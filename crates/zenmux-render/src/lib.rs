//! WGPU-based terminal rendering pipeline.
//!
//! Renders the visible terminal grid as instanced quads in a single draw
//! call via `egui_wgpu::CallbackTrait`.

use wgpu::util::DeviceExt;

use zenmux_core::Result;

/// Per-instance GPU data for one cell quad.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CellInstance {
    /// Screen-space position (top-left of cell, in pixels).
    screen_pos: [f32; 2],
    /// UV rectangle in the glyph atlas: [u_min, v_min, u_max, v_max].
    glyph_uv: [f32; 4],
    /// Foreground colour (RGBA).
    fg_color: [f32; 4],
    /// Background colour (RGBA).
    bg_color: [f32; 4],
}

/// The terminal render pass.
pub struct TerminalRenderPass {
    pipeline: wgpu::RenderPipeline,
    instance_buf: wgpu::Buffer,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    atlas_bind_group: wgpu::BindGroup,
    num_instances: u32,
    max_instances: u32,
}

impl TerminalRenderPass {
    /// Create a new render pass.
    pub fn new(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        atlas_view: &wgpu::TextureView,
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

        // Bind group layout.
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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

        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terminal.atlas_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });

        // Pipeline layout.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terminal.pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terminal.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vs_module,
                entry_point: Some("vs_main".into()),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        }],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<CellInstance>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 8,
                                shader_location: 2,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 24,
                                shader_location: 3,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 40,
                                shader_location: 4,
                            },
                        ],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs_module,
                entry_point: Some("fs_main".into()),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });

        Ok(Self {
            pipeline,
            instance_buf,
            vertex_buf,
            index_buf,
            atlas_bind_group,
            num_instances: 0,
            max_instances,
        })
    }

    /// Record the draw commands into an encoder.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        if self.num_instances == 0 {
            return;
        }

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("terminal.render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &self.atlas_bind_group, &[]);
        rpass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        rpass.set_vertex_buffer(1, self.instance_buf.slice(..));
        rpass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        rpass.draw_indexed(0..6, 0, 0..self.num_instances);
    }
}

// ── WGSL shaders ────────────────────────────────────────────────────────

const TERMINAL_VS: &str = r"
struct VertexInput {
    @location(0) pos: vec2<f32>,
};

struct InstanceInput {
    @location(1) screen_pos: vec2<f32>,
    @location(2) glyph_uv: vec4<f32>,
    @location(3) fg_color: vec4<f32>,
    @location(4) bg_color: vec4<f32>,
};

struct Varying {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg_color: vec4<f32>,
    @location(2) bg_color: vec4<f32>,
};

@vertex
fn vs_main(
    vert: VertexInput,
    inst: InstanceInput,
) -> Varying {
    var out: Varying;
    let cell_size = inst.glyph_uv.zw;
    out.position = vec4<f32>(
        (inst.screen_pos.x + vert.pos.x * cell_size.x) / 960.0 - 1.0,
        1.0 - (inst.screen_pos.y + vert.pos.y * cell_size.y) / 540.0,
        0.0,
        1.0,
    );
    out.uv = vec2<f32>(
        inst.glyph_uv.x + vert.pos.x * (inst.glyph_uv.z - inst.glyph_uv.x),
        inst.glyph_uv.y + vert.pos.y * (inst.glyph_uv.w - inst.glyph_uv.y),
    );
    out.fg_color = inst.fg_color;
    out.bg_color = inst.bg_color;
    return out;
}
";

const TERMINAL_FS: &str = r"
@group(0) @binding(0) var glyph_atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;

struct Varying {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg_color: vec4<f32>,
    @location(2) bg_color: vec4<f32>,
};

@fragment
fn fs_main(var: Varying) -> @location(0) vec4<f32> {
    let alpha = textureSample(glyph_atlas, atlas_sampler, var.uv).a;
    let color = mix(var.bg_color, var.fg_color, alpha);
    return color;
}
";
