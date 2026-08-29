// 10-bit planar YUV upload → RGBA. Planes are R16Uint (raw values 0..=1023, not
// normalised), so this samples unsigned integers and divides by `max_value` to
// reach [0, 1]. The render target is Rgba16Float, so 10-bit precision survives.
// Kept separate from the 8-bit shader because the plane textures differ in sample
// type (u32 here vs normalised f32 there).

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
@group(0) @binding(1) var cb_tex: texture_2d<u32>;
@group(0) @binding(2) var cr_tex: texture_2d<u32>;
@group(0) @binding(3) var<uniform> u: YuvUniforms;

struct YuvUniforms {
    chroma_x_div: u32,
    chroma_y_div: u32,
    // Divisor that maps a raw sample to [0, 1] (1023 for 10-bit).
    max_value: f32,
    _pad1: u32,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let luma_size = textureDimensions(y_tex);
    let px = min(
        vec2<i32>(in.uv * vec2<f32>(f32(luma_size.x), f32(luma_size.y))),
        vec2<i32>(i32(luma_size.x) - 1, i32(luma_size.y) - 1),
    );
    let y_val = f32(textureLoad(y_tex, px, 0).r) / u.max_value;

    let chroma_size = textureDimensions(cb_tex);
    let cpx = min(
        vec2<i32>(px.x / i32(u.chroma_x_div), px.y / i32(u.chroma_y_div)),
        vec2<i32>(i32(chroma_size.x) - 1, i32(chroma_size.y) - 1),
    );
    let cb = f32(textureLoad(cb_tex, cpx, 0).r) / u.max_value - 0.5;
    let cr = f32(textureLoad(cr_tex, cpx, 0).r) / u.max_value - 0.5;

    // BT.601 full-range YCbCr → linear RGB.
    let r = clamp(y_val + 1.402  * cr,              0.0, 1.0);
    let g = clamp(y_val - 0.344  * cb - 0.714 * cr, 0.0, 1.0);
    let b = clamp(y_val + 1.772  * cb,              0.0, 1.0);
    return vec4<f32>(r, g, b, 1.0);
}
