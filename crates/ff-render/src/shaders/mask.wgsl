// Shared mask shader: ShapeMask (mode=0), LumaMask (mode=1), AlphaMatte (mode=2).
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

@group(0) @binding(0) var tex_base:    texture_2d<f32>;  // base / foreground
@group(0) @binding(1) var tex_mask:    texture_2d<f32>;  // mask / background
@group(0) @binding(2) var tex_sampler: sampler;
@group(0) @binding(3) var<uniform> u: MaskUniforms;

struct MaskUniforms {
    // 0 = ShapeMask, 1 = LumaMask, 2 = AlphaMatte
    mode: u32,
    // ShapeMask and LumaMask: 1 swaps the mask.
    invert: u32,
    // ShapeMask: the source frame's size, so `rect` can be read in its pixels.
    src_w: f32,
    src_h: f32,
    // ShapeMask: x, y, x_end, y_end in source pixels (x_end / y_end exclusive).
    rect: vec4<f32>,
}

// Fragment

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base = textureSample(tex_base, tex_sampler, in.uv);
    let mask = textureSample(tex_mask, tex_sampler, in.uv);

    var out: vec4<f32>;
    switch u.mode {
        // ShapeMask: the rectangle is evaluated here rather than sampled from an
        // uploaded mask. `floor(uv * src)` is the texel the old mask would have been
        // read at, so the same pixels are kept at any output size.
        //
        // Not quite byte-identical at an edge, though: the uploaded mask was sampled
        // linearly and then thresholded at `a > 0.004`, so an interpolated edge texel
        // counted as inside and the rectangle could grow by one texel when a node
        // upscaled in front of this one. The test here is exact.
        case 0u {
            let p = floor(in.uv * vec2<f32>(u.src_w, u.src_h));
            let inside = p.x >= u.rect.x && p.x < u.rect.z
                      && p.y >= u.rect.y && p.y < u.rect.w;
            // `!=` on bools is xor: invert swaps inside for outside.
            let alpha = select(0.0, 1.0, inside != (u.invert != 0u));
            out = vec4<f32>(base.rgb, base.a * alpha);
        }
        // LumaMask: `tex_mask` is the source frame itself, so its BT.709 luma is the
        // opacity. Inverting is `1 - luma`, which is what the old CPU-built mask
        // stored as a grey.
        case 1u {
            let luma = dot(mask.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
            let opacity = select(luma, 1.0 - luma, u.invert != 0u);
            out = vec4<f32>(base.rgb, base.a * opacity);
        }
        // AlphaMatte: Porter-Duff src-over (base = foreground, mask = background).
        case 2u {
            let alpha   = base.a;
            let out_rgb = base.rgb * alpha + mask.rgb * (1.0 - alpha);
            let out_a   = alpha + mask.a * (1.0 - alpha);
            out = vec4<f32>(out_rgb, out_a);
        }
        default {
            out = base;
        }
    }
    return out;
}
