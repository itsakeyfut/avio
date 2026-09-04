// Full-screen quad vertex shader.
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

// Fragment

@group(0) @binding(0) var tex_from: texture_2d<f32>;
@group(0) @binding(1) var tex_to:   texture_2d<f32>;
@group(0) @binding(2) var tex_sampler: sampler;
@group(0) @binding(3) var<uniform> u: CrossfadeUniforms;

struct CrossfadeUniforms {
    factor: f32,
    _pad0:  f32,
    _pad1:  f32,
    _pad2:  f32,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let from_color = textureSample(tex_from, tex_sampler, in.uv);
    let to_color   = textureSample(tex_to,   tex_sampler, in.uv);
    return mix(from_color, to_color, u.factor);
}
