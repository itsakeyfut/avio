// Two-phase dip through a solid colour, reproducing FFmpeg's `fadeblack` /
// `fadewhite` (`vf_xfade.c`, FADEBLACK_TRANSITION). The colour is reached about a
// fifth of the way in and held through the middle -- not a linear ramp to the
// midpoint. Mirrors `DipToColorNode::process_cpu` so the CPU and GPU paths agree.
//
// `u.color` may sit outside [0, 1]: FFmpeg dips to luma 0 / 255, which expands past
// the displayable range. Nothing clamps until the write to the Rgba8Unorm target.
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
    // FFmpeg's progress is the complement of ours.
    let g = 1.0 - u.progress;
    let phase = 0.2;
    let s1 = smoothstep(1.0 - phase, 1.0, g);
    let s2 = smoothstep(phase, 1.0, g);
    let bg = vec4<f32>(u.color.rgb, 1.0);
    let a = textureSample(tex_a, samp, in.uv);
    let b = textureSample(tex_b, samp, in.uv);
    let leaving  = a * s1 + bg * (1.0 - s1);
    let arriving = bg * s2 + b * (1.0 - s2);
    return leaving * g + arriving * (1.0 - g);
}
