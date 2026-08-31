// Per-channel tone curve via a precomputed 256-sample LUT texture. The 256x1
// Rgba8Unorm LUT holds four curves: R=red, G=green, B=blue, A=master. The master
// curve is applied to each channel first, then the per-channel curve (Photoshop
// convention). Nearest lookup: idx = round(value * 255).

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

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var lut: texture_2d<f32>;   // 256x1 RGBA LUT

fn lut_idx(value: f32) -> i32 {
    return clamp(i32(value * 255.0 + 0.5), 0, 255);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureLoad(input_tex, vec2<i32>(in.position.xy), 0);

    // Master (A channel) applied to each channel first.
    let mr = textureLoad(lut, vec2<i32>(lut_idx(color.r), 0), 0).a;
    let mg = textureLoad(lut, vec2<i32>(lut_idx(color.g), 0), 0).a;
    let mb = textureLoad(lut, vec2<i32>(lut_idx(color.b), 0), 0).a;

    // Then the per-channel curve.
    let r = textureLoad(lut, vec2<i32>(lut_idx(mr), 0), 0).r;
    let g = textureLoad(lut, vec2<i32>(lut_idx(mg), 0), 0).g;
    let b = textureLoad(lut, vec2<i32>(lut_idx(mb), 0), 0).b;

    return vec4<f32>(r, g, b, color.a);
}
