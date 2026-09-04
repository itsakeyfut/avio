// Affine 2D transform: translate, rotate (radians), scale.
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

// Bindings

@group(0) @binding(0) var tex_input:   texture_2d<f32>;
@group(0) @binding(1) var tex_sampler: sampler;
@group(0) @binding(2) var<uniform> u: TransformUniforms;

struct TransformUniforms {
    translate: vec2<f32>,  // UV-space offset (positive → shift right/down)
    rotate:    f32,        // Rotation in radians (counter-clockwise)
    _pad0:     f32,
    scale:     vec2<f32>,  // Scale factors (1.0 = no change)
    _pad1:     f32,
    _pad2:     f32,
}

// Fragment

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Apply inverse transform to the screen UV to find where to sample.
    let center = vec2<f32>(0.5, 0.5);
    var uv_c = in.uv - center;

    // Inverse scale.
    uv_c = uv_c / max(u.scale, vec2<f32>(0.0001));

    // Inverse rotation (negate angle).
    let cos_a = cos(-u.rotate);
    let sin_a = sin(-u.rotate);
    uv_c = vec2<f32>(
        uv_c.x * cos_a - uv_c.y * sin_a,
        uv_c.x * sin_a + uv_c.y * cos_a,
    );

    // Inverse translation.
    let sample_uv = uv_c + center - u.translate;

    // Return transparent outside [0, 1].
    if any(sample_uv < vec2<f32>(0.0)) || any(sample_uv > vec2<f32>(1.0)) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    return textureSample(tex_input, tex_sampler, sample_uv);
}
