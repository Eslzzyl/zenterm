//! Wgpu glyph-atlas texture helpers.
//!
//! Bridges [`zenterm_glyph::GlyphAtlas`] (CPU pixel data) to a wgpu 2D
//! texture for use by the terminal render pipeline.

use wgpu::{Extent3d, TexelCopyBufferLayout, TexelCopyTextureInfo};

fn aligned_row_bytes(width: u32) -> u32 {
    width.saturating_mul(4).div_ceil(256) * 256
}

fn padded_rgba_rows(width: u32, height: u32, data: &[u8]) -> Vec<u8> {
    let tight = width as usize * 4;
    let padded = aligned_row_bytes(width) as usize;
    if tight == padded {
        return data.to_vec();
    }

    let mut result = Vec::with_capacity(padded * height as usize);
    for row in 0..height as usize {
        let start = row * tight;
        result.extend_from_slice(&data[start..start + tight]);
        result.resize(result.len() + padded - tight, 0);
    }
    result
}

/// Create a texture for one terminal image at its exact pixel dimensions.
pub fn create_image_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    data: &[u8],
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terminal.image"),
        size: Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bytes_per_row = aligned_row_bytes(width);
    let padded = padded_rgba_rows(width, height, data);
    queue.write_texture(
        TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &padded,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(height),
        },
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

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

/// Upload one changed rectangular region to an existing atlas texture.
///
/// The caller supplies rows padded to wgpu's copy-row alignment.  Keeping the
/// region narrow avoids cloning and uploading the entire atlas slot when a
/// glyph or image is added.
pub fn update_atlas_texture_region(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    data: &[u8],
) {
    let tight_bytes_per_row = width.saturating_mul(4);
    let bytes_per_row = tight_bytes_per_row.div_ceil(256) * 256;
    queue.write_texture(
        TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        data,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(height),
        },
        Extent3d {
            width,
            height,
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
