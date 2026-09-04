// Full-screen quad vertex shader shared by all single-pass nodes.
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_idx: u32) -> VertexOutput {
    // Two triangles covering the full NDC clip space.
    // NDC Y is up; texture UV Y is down — mapping is inverted on Y.
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

// Fragment

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var tex_sampler: sampler;
@group(0) @binding(2) var<uniform> u: ColorGradeUniforms;

struct ColorGradeUniforms {
    brightness:  f32,
    contrast:    f32,
    saturation:  f32,
    temperature: f32,
    tint:        f32,
    _pad0:       f32,
    _pad1:       f32,
    _pad2:       f32,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(input_tex, tex_sampler, in.uv);
    var rgb = color.rgb;

    // Brightness: additive offset.
    rgb = rgb + u.brightness;

    // Contrast: pivot at 0.5.
    rgb = (rgb - 0.5) * u.contrast + 0.5;

    // Temperature (warm/cool): shift R and B in opposite directions.
    rgb.r = rgb.r + u.temperature * 0.1;
    rgb.b = rgb.b - u.temperature * 0.1;

    // Tint (green ↔ magenta): shift G.
    rgb.g = rgb.g + u.tint * 0.1;

    // Saturation: blend toward luma (BT.709 coefficients).
    let luma = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    rgb = mix(vec3<f32>(luma), rgb, u.saturation);

    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb, color.a);
}
