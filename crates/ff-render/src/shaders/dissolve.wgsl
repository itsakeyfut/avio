// Per-pixel dissolve: clip B shows through wherever the supplied mask is set.
//
// The mask is built on the CPU (`ff_filter::dissolve_mask`) rather than derived here.
// FFmpeg keys its dissolve off `fract(sinf(x*12.9898 + y*78.233) * 43758.545)`, whose
// argument reaches ~110 000 at 1080p -- past where an f32 holds it steadily, so the
// value is not reproducible across implementations and a copy of the hash in WGSL would
// reveal a different set of pixels than the CPU reference does. Reading a mask makes the
// two agree by construction (#1732).

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
// Binding 3 is the uniform buffer the shared `build_pipeline` always creates. This
// shader has no use for it -- its progress is already baked into the mask -- and a
// layout entry the shader does not declare is allowed, so it is simply absent here.
@group(0) @binding(4) var tex_mask: texture_2d<f32>;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // `textureLoad` at integer coordinates, not `textureSample`: the mask is a per-pixel
    // decision, and the shared sampler filters linearly, which would blur the boundary
    // between a chosen and an unchosen pixel.
    let dims = textureDimensions(tex_mask);
    let px = vec2<i32>(clamp(
        vec2<u32>(in.uv * vec2<f32>(dims)),
        vec2<u32>(0u),
        dims - vec2<u32>(1u),
    ));
    if textureLoad(tex_mask, px, 0).r >= 0.5 {
        return textureSample(tex_b, samp, in.uv);
    }
    return textureSample(tex_a, samp, in.uv);
}
