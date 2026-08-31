// Unsharp-mask combine pass: result = orig + (orig - blurred) * strength.
// The node runs a 2-pass separable Gaussian blur (gaussian_blur.wgsl) first, then
// this pass combines the original with the blurred result. Uses `textureLoad` so
// the two inputs are read per-texel with no filtering.

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

@group(0) @binding(0) var orig_tex: texture_2d<f32>;
@group(0) @binding(1) var blur_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> u: SharpenUniforms;

// Matches Rust #[repr(C)] SharpenUniforms.
struct SharpenUniforms {
    strength: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let coord = vec2<i32>(in.position.xy);
    let orig = textureLoad(orig_tex, coord, 0);
    let blurred = textureLoad(blur_tex, coord, 0);
    let detail = orig.rgb - blurred.rgb;
    let sharpened = clamp(orig.rgb + detail * u.strength, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(sharpened, orig.a);
}
