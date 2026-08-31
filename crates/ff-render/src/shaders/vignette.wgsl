// Position-based radial vignette: darkens toward the corners with a smooth
// falloff. Purely per-texel (the output pixel depends only on its own colour and
// position), so it uses `textureLoad` and needs no sampler.

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
@group(0) @binding(1) var<uniform> u: VignetteUniforms;

// Matches Rust VignetteUniforms (field order/size kept in sync).
struct VignetteUniforms {
    radius:   f32,   // normalised distance where darkening begins (0..1)
    strength: f32,   // maximum darkening at the corners (0 = off, 1 = to black)
    feather:  f32,   // width of the falloff band past `radius`
    _pad:     f32,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Distance from centre, normalised so a corner is ~1.0.
    let d = length(in.uv - vec2<f32>(0.5)) * 2.0;
    // Guard against feather == 0 making smoothstep's edges coincide (0/0).
    let edge1 = u.radius + max(u.feather, 1e-5);
    let v = smoothstep(u.radius, edge1, d);
    let factor = 1.0 - v * u.strength;

    let color = textureLoad(input_tex, vec2<i32>(in.position.xy), 0);
    return vec4<f32>(color.rgb * factor, color.a);
}
