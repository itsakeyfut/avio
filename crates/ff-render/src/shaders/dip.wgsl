// Two-phase dip to a solid colour: clip A fades to the dip colour over the first
// half of progress, then the colour fades to clip B over the second half.
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
@group(0) @binding(3) var<uniform> u: DipUniforms;

struct DipUniforms {
    progress: f32,
    _pad0:    f32,
    _pad1:    f32,
    _pad2:    f32,
    color:    vec4<f32>,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let dip = vec4<f32>(u.color.rgb, 1.0);
    if (u.progress < 0.5) {
        let color_a = textureSample(tex_a, samp, in.uv);
        return mix(color_a, dip, u.progress * 2.0);
    }
    let color_b = textureSample(tex_b, samp, in.uv);
    return mix(dip, color_b, (u.progress - 0.5) * 2.0);
}
