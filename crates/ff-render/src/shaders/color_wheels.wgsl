// Three-way colour corrector: shadows lift, midtones gamma, highlights gain.
// Each adjustment is weighted by a luminance region (shadows = low luma,
// midtones = mid luma, highlights = high luma) so the wheels affect their
// tonal range. Applied in sequence lift -> gamma -> gain, per channel.

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
@group(0) @binding(1) var<uniform> u: ColorWheelsUniforms;

// Matches Rust ColorWheelsUniforms (each vec3 padded to 16 bytes for std140).
struct ColorWheelsUniforms {
    shadows_lift:    vec3<f32>,
    _p0:             f32,
    midtones_gamma:  vec3<f32>,
    _p1:             f32,
    highlights_gain: vec3<f32>,
    _p2:             f32,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureLoad(input_tex, vec2<i32>(in.position.xy), 0);
    let luma = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));

    let shadow_w = 1.0 - smoothstep(0.0, 0.5, luma);
    let highlight_w = smoothstep(0.5, 1.0, luma);
    let mid_w = clamp(1.0 - shadow_w - highlight_w, 0.0, 1.0);

    var rgb = color.rgb;
    // Lift (shadows): additive.
    rgb = rgb + u.shadows_lift * shadow_w;
    // Gamma (midtones): blended toward the gamma-corrected value.
    let gamma_rgb = pow(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0 / u.midtones_gamma);
    rgb = mix(rgb, gamma_rgb, mid_w);
    // Gain (highlights): multiplicative.
    rgb = rgb * (vec3<f32>(1.0) + (u.highlights_gain - 1.0) * highlight_w);

    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb, color.a);
}
