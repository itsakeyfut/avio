// 10-bit semi-planar (P010) YUV upload → RGBA. Two things separate this from the
// planar 10-bit shader, and both are why it is a file of its own rather than a
// branch:
//
//   1. Chroma arrives as one Rg16Uint plane instead of two R16Uint planes — Cb
//      is the `.r` channel and Cr the `.g` channel of the same texel — so the
//      bind group has one texture fewer (binding 2 is absent; the uniform stays
//      at binding 3 to match the other upload shaders).
//   2. P010 is MSB-aligned: the 10 significant bits sit in the *high* bits of
//      each 16-bit sample with the low 6 zeroed. Yuv420p10le instead stores
//      0..=1023 in the low bits and needs no shift.
//
// The render target is Rgba16Float, so 10-bit precision survives the upload.

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_idx: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0,  1.0), vec2<f32>( 1.0,  1.0), vec2<f32>(-1.0, -1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>( 1.0,  1.0), vec2<f32>( 1.0, -1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    var out: VertexOutput;
    out.position = vec4<f32>(positions[vertex_idx], 0.0, 1.0);
    out.uv = uvs[vertex_idx];
    return out;
}

@group(0) @binding(0) var y_tex:  texture_2d<u32>;
@group(0) @binding(1) var uv_tex: texture_2d<u32>;
@group(0) @binding(3) var<uniform> u: YuvUniforms;

struct YuvUniforms {
    chroma_x_div: u32,
    chroma_y_div: u32,
    // Divisor that maps a raw sample to [0, 1] (1023, applied after the shift).
    max_value: f32,
    _pad1: u32,
}

// Drop P010's zeroed low 6 bits, then normalise. Matches FFmpeg's own P010LE
// pixel descriptor (depth = 10, shift = 6), so the result agrees with the
// planar 10-bit path sample for sample.
fn p010_norm(sample: u32, max_value: f32) -> f32 {
    return f32(sample >> 6u) / max_value;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let luma_size = textureDimensions(y_tex);
    let px = min(
        vec2<i32>(in.uv * vec2<f32>(f32(luma_size.x), f32(luma_size.y))),
        vec2<i32>(i32(luma_size.x) - 1, i32(luma_size.y) - 1),
    );
    let y_val = p010_norm(textureLoad(y_tex, px, 0).r, u.max_value);

    let chroma_size = textureDimensions(uv_tex);
    let cpx = min(
        vec2<i32>(px.x / i32(u.chroma_x_div), px.y / i32(u.chroma_y_div)),
        vec2<i32>(i32(chroma_size.x) - 1, i32(chroma_size.y) - 1),
    );
    // One texel carries both chroma samples: Cb in .r, Cr in .g.
    let chroma = textureLoad(uv_tex, cpx, 0);
    let cb = p010_norm(chroma.r, u.max_value) - 0.5;
    let cr = p010_norm(chroma.g, u.max_value) - 0.5;

    // BT.601 full-range YCbCr → linear RGB.
    let r = clamp(y_val + 1.402  * cr,              0.0, 1.0);
    let g = clamp(y_val - 0.344  * cb - 0.714 * cr, 0.0, 1.0);
    let b = clamp(y_val + 1.772  * cb,              0.0, 1.0);
    return vec4<f32>(r, g, b, 1.0);
}
