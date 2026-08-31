// 3D LUT applied via manual trilinear interpolation over an Rgba32Float 3D
// texture (a size^3 grid; texel (x, y, z) = grid point (r, g, b)). textureLoad
// (no sampler) keeps full precision and works on backends without float32
// filtering; the interpolation mirrors the CPU path for GPU/CPU agreement.

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
@group(0) @binding(1) var lut_tex: texture_3d<f32>;

fn lut_at(p: vec3<i32>) -> vec3<f32> {
    return textureLoad(lut_tex, p, 0).rgb;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureLoad(input_tex, vec2<i32>(in.position.xy), 0);
    let n = i32(textureDimensions(lut_tex).x);
    let last = f32(n - 1);

    // Map [0, 1] to grid coordinates v*(size-1); the same mapping the CPU uses.
    let c = clamp(color.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * last;
    let f = c - floor(c);
    let lo = clamp(vec3<i32>(floor(c)), vec3<i32>(0), vec3<i32>(n - 1));
    let hi = min(lo + vec3<i32>(1), vec3<i32>(n - 1));

    let c000 = lut_at(vec3<i32>(lo.x, lo.y, lo.z));
    let c100 = lut_at(vec3<i32>(hi.x, lo.y, lo.z));
    let c010 = lut_at(vec3<i32>(lo.x, hi.y, lo.z));
    let c110 = lut_at(vec3<i32>(hi.x, hi.y, lo.z));
    let c001 = lut_at(vec3<i32>(lo.x, lo.y, hi.z));
    let c101 = lut_at(vec3<i32>(hi.x, lo.y, hi.z));
    let c011 = lut_at(vec3<i32>(lo.x, hi.y, hi.z));
    let c111 = lut_at(vec3<i32>(hi.x, hi.y, hi.z));

    let c00 = mix(c000, c100, f.x);
    let c10 = mix(c010, c110, f.x);
    let c01 = mix(c001, c101, f.x);
    let c11 = mix(c011, c111, f.x);
    let c0 = mix(c00, c10, f.y);
    let c1 = mix(c01, c11, f.y);
    let graded = mix(c0, c1, f.z);

    return vec4<f32>(graded, color.a);
}
