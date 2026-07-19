// ── WGSL shaders ────────────────────────────────────────────────────────

//! WGSL shader source strings embedded in the binary.
//!
//! These are compiled into wgpu shader modules at [`TerminalRenderPass`]
//! creation time.
//!
//! [`TerminalRenderPass`]: super::TerminalRenderPass

pub(crate) const TERMINAL_VS: &str = r"
struct VertexInput {
    @location(0) pos: vec2<f32>,
};

struct InstanceInput {
    @location(1) clip_pos: vec2<f32>,
    @location(2) uv_min: vec2<f32>,
    @location(3) uv_max: vec2<f32>,
    @location(4) clip_cell_size: vec2<f32>,
    @location(5) glyph_size: vec2<f32>,
    @location(6) glyph_offset: vec2<f32>,
    @location(7) fg_color: vec4<f32>,
    @location(8) bg_color: vec4<f32>,
    @location(9) flags: u32,
};

struct Varying {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg_color: vec4<f32>,
    @location(2) bg_color: vec4<f32>,
    @location(3) flags: u32,
};

@vertex
fn vs_main(
    vert: VertexInput,
    inst: InstanceInput,
) -> Varying {
    var out: Varying;
    out.position = vec4<f32>(
        inst.clip_pos.x + vert.pos.x * inst.clip_cell_size.x,
        inst.clip_pos.y - vert.pos.y * inst.clip_cell_size.y,
        0.0,
        1.0,
    );
    out.uv = vec2<f32>(
        inst.uv_min.x + vert.pos.x * (inst.uv_max.x - inst.uv_min.x),
        inst.uv_min.y + vert.pos.y * (inst.uv_max.y - inst.uv_min.y),
    );
    out.fg_color = inst.fg_color;
    out.bg_color = inst.bg_color;
    out.flags = inst.flags;
    return out;
}
";

pub(crate) const TERMINAL_FS: &str = r"
@group(0) @binding(0) var glyph_atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;

struct Varying {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg_color: vec4<f32>,
    @location(2) bg_color: vec4<f32>,
    @location(3) flags: u32,
};

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    } else {
        return pow((c + 0.055) / 1.055, 2.4);
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        return c * 12.92;
    } else {
        return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
    }
}

@fragment
fn fs_main(in: Varying) -> @location(0) vec4<f32> {
    // Dispatch based on glyph type.
    // 0 = SUBPIXEL — LCD subpixel coverage: per-channel mix.
    // 1 = MASK     — Grayscale alpha mask: uniform coverage.
    // 2 = COLOR    — Emoji/color glyph: sample RGBA directly.
    // 3 = SOLID    — Solid color fill: no texture sample.
    // 4 = IMAGE    — Full RGBA quad from atlas: premultiplied linear → sRGB.

    // Convert vertex colours from sRGB to linear for correct blending.
    let fg_r = srgb_to_linear(in.fg_color.r);
    let fg_g = srgb_to_linear(in.fg_color.g);
    let fg_b = srgb_to_linear(in.fg_color.b);
    let bg_r = srgb_to_linear(in.bg_color.r);
    let bg_g = srgb_to_linear(in.bg_color.g);
    let bg_b = srgb_to_linear(in.bg_color.b);

    if (in.flags == 3u) {
        // SOLID fill — no texture sample.
        // Convert linear back to sRGB before premultiplication, since the
        // surface is non-sRGB (Bgra8Unorm) and the display expects gamma-
        // encoded values.
        let a = in.bg_color.a;
        let r = linear_to_srgb(bg_r) * a;
        let g = linear_to_srgb(bg_g) * a;
        let b = linear_to_srgb(bg_b) * a;
        return vec4<f32>(r, g, b, a);
    }

    let texel = textureSample(glyph_atlas, atlas_sampler, in.uv);

    if (in.flags == 2u) {
        // COLOR glyph — texel is premultiplied RGBA.
        // Un-premultiply and convert from sRGB to linear.
        let a = texel.a;
        if (a == 0.0) {
            return vec4<f32>(linear_to_srgb(bg_r), linear_to_srgb(bg_g), linear_to_srgb(bg_b), in.fg_color.a);
        }
        let c_r = srgb_to_linear(texel.r / a);
        let c_g = srgb_to_linear(texel.g / a);
        let c_b = srgb_to_linear(texel.b / a);
        // Blend against background using alpha, then convert back to sRGB.
        let r = linear_to_srgb(bg_r + (c_r - bg_r) * a);
        let g = linear_to_srgb(bg_g + (c_g - bg_g) * a);
        let b = linear_to_srgb(bg_b + (c_b - bg_b) * a);
        return vec4<f32>(r, g, b, in.fg_color.a);
    }

    if (in.flags == 1u) {
        // MASK glyph — R=G=B=alpha. Use single coverage value.
        let alpha = texel.r;
        let r = linear_to_srgb(bg_r + (fg_r - bg_r) * alpha);
        let g = linear_to_srgb(bg_g + (fg_g - bg_g) * alpha);
        let b = linear_to_srgb(bg_b + (fg_b - bg_b) * alpha);
        return vec4<f32>(r, g, b, in.fg_color.a);
    }

    if (in.flags == 4u) {
        // IMAGE — straight sRGB RGBA in the atlas (no color-space
        // conversion needed; the atlas stores sRGB data as-is).
        return texel;
    }

    // SUBPIXEL (default, flags == 0).
    // Per-channel subpixel blending in linear space.
    // The atlas stores R=red coverage, G=green coverage, B=blue coverage.
    let coverage = texel.rgb;
    let r = linear_to_srgb(mix(bg_r, fg_r, coverage.r));
    let g = linear_to_srgb(mix(bg_g, fg_g, coverage.g));
    let b = linear_to_srgb(mix(bg_b, fg_b, coverage.b));
    return vec4<f32>(r, g, b, in.fg_color.a);
}
";
