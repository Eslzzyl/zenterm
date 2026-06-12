//! Wgpu glyph-atlas texture helpers.
//!
//! Bridges [`zenmux_glyph::GlyphAtlas`] (CPU pixel data) to a wgpu 2D
//! texture for use by the terminal render pipeline.

use wgpu::{Extent3d, TexelCopyBufferLayout, TexelCopyTextureInfo};

/// Create a wgpu 2D texture + view from glyph atlas pixel data.
///
/// The texture is sized to `size × size` with format `Rgba8Unorm`.
/// We use a non-sRGB format because the atlas stores linear subpixel
/// coverage values, not sRGB-encoded colors.  The actual sRGB→linear
/// conversion for vertex colours is done in the fragment shader.
pub fn create_atlas_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    data: &[u8],
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("glyph_atlas"),
        size: Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    // Initial upload.
    queue.write_texture(
        TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * 4),
            rows_per_image: Some(size),
        },
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );

    (texture, view)
}

/// Upload updated glyph atlas data to an existing wgpu texture.
///
/// The texture must have the same size as the glyph atlas.  This is
/// cheaper than recreating the texture when only the pixel content
/// changed (e.g. a few new glyphs were rasterised without a resize).
pub fn update_atlas_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: u32,
    data: &[u8],
) {
    queue.write_texture(
        TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * 4),
            rows_per_image: Some(size),
        },
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
}

/// Create a default atlas sampler (nearest filtering, clamp-to-edge).
///
/// Nearest-neighbour filtering gives crisp, pixel-aligned glyphs
/// which is the expected look for a terminal emulator on high-DPI
/// displays.  With subpixel-rendered glyphs the RGB coverage values
/// are per-pixel, so linear interpolation would introduce colour
/// fringing — Nearest is the correct choice here.
pub fn create_atlas_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("glyph_atlas_sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    })
}
