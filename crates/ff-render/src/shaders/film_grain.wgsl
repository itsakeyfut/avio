// Film grain: per-pixel pseudo-random noise added in YCbCr (BT.709) so luma and
// chroma grain can be dialled independently. The Wang hash is seeded from the
// pixel position and `frame_index`, so the pattern is deterministic per frame but
// changes every frame (no temporal sticking). The same hash runs on the CPU path.

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
@group(0) @binding(1) var<uniform> u: FilmGrainUniforms;

// Matches Rust FilmGrainUniforms (field order/size kept in sync).
struct FilmGrainUniforms {
    luma_strength:   f32,
    chroma_strength: f32,
    frame_index:     u32,
    _pad:            u32,
}

// Wang hash: fast integer hash used as a per-pixel PRNG. Kept byte-identical to
// the Rust CPU path (wrapping u32 arithmetic).
fn wang_hash(seed_in: u32) -> u32 {
    var seed = (seed_in ^ 61u) ^ (seed_in >> 16u);
    seed = seed * 9u;
    seed = seed ^ (seed >> 4u);
    seed = seed * 0x27d4eb2du;
    seed = seed ^ (seed >> 15u);
    return seed;
}

fn rand01(seed: u32) -> f32 {
    return f32(wang_hash(seed)) / 4294967295.0;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let px = vec2<u32>(vec2<i32>(in.position.xy));
    let base = px.x * 1973u + px.y * 9277u + u.frame_index * 26699u;

    // Grain centred on zero, scaled by the respective strength.
    let g_luma = (rand01(base) - 0.5) * u.luma_strength;
    let g_cb = (rand01(base + 1u) - 0.5) * u.chroma_strength;
    let g_cr = (rand01(base + 2u) - 0.5) * u.chroma_strength;

    let color = textureLoad(input_tex, vec2<i32>(in.position.xy), 0);

    // RGB -> YCbCr (BT.709), Cb/Cr centred on 0.
    var y = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    var cb = (color.b - y) / 1.8556;
    var cr = (color.r - y) / 1.5748;

    y = y + g_luma;
    cb = cb + g_cb;
    cr = cr + g_cr;

    // YCbCr -> RGB.
    let r = y + 1.5748 * cr;
    let g = y - 0.1873 * cb - 0.4681 * cr;
    let b = y + 1.8556 * cb;

    let rgb = clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb, color.a);
}
