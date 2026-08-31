// Glow pass 0: highlight extraction. Pixels whose luma is at or above `threshold`
// pass through; everything else becomes black. The result is then blurred and
// added back to the original by the later glow passes.

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
@group(0) @binding(1) var<uniform> u: ExtractUniforms;

// Matches Rust ExtractUniforms (field order/size kept in sync).
struct ExtractUniforms {
    threshold: f32,   // luma cutoff in [0, 1]; below it the pixel is suppressed
    _pad0:     f32,
    _pad1:     f32,
    _pad2:     f32,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureLoad(input_tex, vec2<i32>(in.position.xy), 0);
    let luma = dot(color.rgb, vec3<f32>(0.299, 0.587, 0.114));
    let rgb = select(vec3<f32>(0.0), color.rgb, luma >= u.threshold);
    return vec4<f32>(rgb, color.a);
}
