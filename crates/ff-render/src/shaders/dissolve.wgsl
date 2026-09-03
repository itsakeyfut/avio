// Per-pixel dissolve: reveal clip B where a deterministic per-pixel hash falls below
// the transition progress. Unlike a cross-blend every output pixel is fully clip A or
// fully clip B, so at 50% the frame holds only the two source values and no mixture.
//
// The hash must match `ff-preview`'s host-side `hash01` bit for bit: the two render the
// same transition on the CPU and GPU paths, and a different hash would reveal a
// different *set* of pixels while still revealing the same *proportion* — a divergence
// a ratio check cannot see. WGSL `u32` arithmetic wraps, matching Rust's `wrapping_*`.
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

// ── Bindings ──────────────────────────────────────────────────────────────────

@group(0) @binding(0) var tex_a: texture_2d<f32>;
@group(0) @binding(1) var tex_b: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> u: DissolveUniforms;

struct DissolveUniforms {
    progress: f32,
    _pad0:    f32,
    _pad1:    f32,
    _pad2:    f32,
}

// ── Fragment ──────────────────────────────────────────────────────────────────

/// The same deterministic per-pixel hash in `[0, 1)` the CPU path uses.
fn hash01(x: u32, y: u32) -> f32 {
    var h: u32 = x * 374761393u + y * 668265263u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    h = h ^ (h >> 16u);
    // Top 24 bits over 2^24: max = 1 - 2^-24, so `progress > hash01` reveals every
    // pixel at progress 1 and none at progress 0.
    return f32(h >> 8u) / 16777216.0;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Integer pixel coordinates, so the hash is keyed the same way as the CPU loop's
    // `(x, y)` rather than by a float UV that would round differently.
    let dims = vec2<f32>(textureDimensions(tex_a));
    let px = vec2<u32>(floor(in.uv * dims));
    if u.progress > hash01(px.x, px.y) {
        return textureSample(tex_b, samp, in.uv);
    }
    return textureSample(tex_a, samp, in.uv);
}
