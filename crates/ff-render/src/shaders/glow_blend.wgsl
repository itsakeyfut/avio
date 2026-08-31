// Glow pass 2: additive blend. Adds the blurred highlights (scaled by
// `intensity`) back onto the original frame and clamps to [0, 1].

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

@group(0) @binding(0) var original_tex: texture_2d<f32>;
@group(0) @binding(1) var glow_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> u: BlendUniforms;

// Matches Rust BlendUniforms (field order/size kept in sync).
struct BlendUniforms {
    intensity: f32,   // additive weight of the glow layer
    _pad0:     f32,
    _pad1:     f32,
    _pad2:     f32,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let coord = vec2<i32>(in.position.xy);
    let base = textureLoad(original_tex, coord, 0);
    let glow = textureLoad(glow_tex, coord, 0).rgb;
    let rgb = clamp(base.rgb + glow * u.intensity, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb, base.a);
}
