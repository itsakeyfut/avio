// Directional wipe: reveals clip B behind a moving edge. The threshold `center`
// sweeps from beyond the far corner (all A at progress 0) to beyond the near
// corner (all B at progress 1), so the endpoints are exact for any angle. `hw`
// (the smoothstep half-width) is floored to a small epsilon so a zero softness
// still gives a near-hard, division-safe edge.
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

@group(0) @binding(0) var tex_a: texture_2d<f32>;
@group(0) @binding(1) var tex_b: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> u: WipeUniforms;

struct WipeUniforms {
    progress: f32,
    softness: f32,
    angle:    f32,
    _pad:     f32,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let a = vec2<f32>(cos(u.angle), sin(u.angle));
    let reach = 0.5 * (abs(a.x) + abs(a.y));
    let hw = max(u.softness, 1e-3);
    let center = mix(0.5 + reach + hw, 0.5 - reach - hw, u.progress);
    let proj = dot(in.uv - vec2<f32>(0.5), a) + 0.5;
    let mask = smoothstep(center - hw, center + hw, proj);
    let color_a = textureSample(tex_a, samp, in.uv);
    let color_b = textureSample(tex_b, samp, in.uv);
    return mix(color_a, color_b, mask);
}
