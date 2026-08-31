// Motion blur via exponential-decay accumulation: blend the current frame with
// the retained previous output. `prev_weight` (uniform) is the weight of the
// accumulated frame; 0 = current frame only (no blur). The Rust side keeps the
// output as the next frame's `prev`, so the blur builds up over time.
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

@group(0) @binding(0) var tex_current: texture_2d<f32>;
@group(0) @binding(1) var tex_prev: texture_2d<f32>;
@group(0) @binding(2) var<uniform> u: MotionBlurUniforms;

struct MotionBlurUniforms {
    prev_weight: f32,
    _pad0:       f32,
    _pad1:       f32,
    _pad2:       f32,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let coord = vec2<i32>(in.position.xy);
    let current = textureLoad(tex_current, coord, 0);
    let prev = textureLoad(tex_prev, coord, 0);
    return mix(current, prev, u.prev_weight);
}
