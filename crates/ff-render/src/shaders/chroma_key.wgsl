// ChromaKey — remove a solid colour from a texture by chroma distance.
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

@group(0) @binding(0) var tex_input:  texture_2d<f32>;
@group(0) @binding(1) var tex_sampler: sampler;
@group(0) @binding(2) var<uniform> u: ChromaKeyUniforms;

struct ChromaKeyUniforms {
    key_color: vec3<f32>,
    tolerance: f32,
    softness:  f32,
    _pad0:     f32,
    _pad1:     f32,
    _pad2:     f32,
}

// BT.709 luma coefficient
fn luma(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Chroma distance: compare colours after removing luminance.
fn chroma_dist(pixel: vec3<f32>, key: vec3<f32>) -> f32 {
    let p_chroma = pixel - luma(pixel);
    let k_chroma = key   - luma(key);
    return length(p_chroma - k_chroma);
}

// Fragment

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let pixel = textureSample(tex_input, tex_sampler, in.uv);
    let dist  = chroma_dist(pixel.rgb, u.key_color);
    // smoothstep returns 0 when dist ≤ (tolerance − softness) → key colour → transparent.
    let alpha_factor = smoothstep(u.tolerance - u.softness, u.tolerance + u.softness, dist);
    return vec4<f32>(pixel.rgb, pixel.a * alpha_factor);
}
