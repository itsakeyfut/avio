// Separable Gaussian blur: one 1D pass along `direction` (horizontal or vertical).
// The node runs this shader twice (H then V) for a full 2D Gaussian. Uses
// `textureLoad` for exact per-texel convolution with clamp-to-edge, so the GPU
// result matches the CPU reference within tolerance.

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
@group(0) @binding(1) var<uniform> u: BlurUniforms;

// Matches Rust #[repr(C)] BlurUniforms (field order/size kept in sync).
struct BlurUniforms {
    direction: vec2<f32>,          // (1,0) = horizontal, (0,1) = vertical
    tap_count: u32,                // odd, 1..=15; taps beyond it carry weight 0
    _pad: u32,
    weights: array<vec4<f32>, 4>,  // 16 weights (15 used), packed 4-per-vec4 for std140
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(input_tex));
    // @builtin(position) is the framebuffer coord (pixel_index + 0.5); truncating
    // to i32 recovers the integer pixel index.
    let center = vec2<i32>(in.position.xy);
    let dir = vec2<i32>(u.direction);              // (1,0) or (0,1)
    let half_span = i32((u.tap_count - 1u) / 2u);

    var acc = vec4<f32>(0.0);
    // Fixed bound (max 15 taps); unused taps have weight 0 and add nothing.
    for (var i: u32 = 0u; i < 15u; i = i + 1u) {
        let w = u.weights[i / 4u][i % 4u];
        // Clamp-to-edge: samples past the border reuse the border texel.
        let coord = clamp(center + dir * (i32(i) - half_span), vec2<i32>(0), dims - vec2<i32>(1));
        acc = acc + textureLoad(input_tex, coord, 0) * w;
    }
    return acc;
}
