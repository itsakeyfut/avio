// Blend mode compositing.
//
// Every mode except the four HSL ones reproduces FFmpeg's `blend` filter, so a
// frame composited here matches the CPU compositor that ADR-0007 keeps as the
// correctness reference. The formulas come from the `DEPTH == 32` branch of
// `libavfilter/blend_modes.c` (identical in release/7.1 and release/8.0), which
// is already normalised to [0, 1].
//
// FFmpeg names its inputs `A` (the `top` pad) and `B` (the `bottom` pad).
// ff-filter links the canvas to `top` and the layer to `bottom`, so throughout
// this file `a` = base (canvas) and `b` = overlay (layer). The helper functions
// mirror `nodes/composite/blend_math.rs` one for one.
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

@group(0) @binding(0) var tex_base:    texture_2d<f32>;
@group(0) @binding(1) var tex_overlay: texture_2d<f32>;
@group(0) @binding(2) var tex_sampler: sampler;
@group(0) @binding(3) var<uniform> u: BlendUniforms;

struct BlendUniforms {
    // The `BlendMode` discriminant. The codes are defined by the Rust enum in
    // `nodes/composite/blend_mode.rs` and pinned by
    // `blend_mode_discriminants_should_match_the_shader_mode_codes`, so a new
    // mode needs a row there and a `case` below.
    mode:    u32,
    opacity: f32,
    _pad0:   f32,
    _pad1:   f32,
}

const PI: f32 = 3.14159265358979323846;

// HSL helpers

fn hue_to_rgb(p: f32, q: f32, t_in: f32) -> f32 {
    var t = t_in;
    if t < 0.0 { t += 1.0; }
    if t > 1.0 { t -= 1.0; }
    if t < 1.0 / 6.0 { return p + (q - p) * 6.0 * t; }
    if t < 0.5 { return q; }
    if t < 2.0 / 3.0 { return p + (q - p) * (2.0 / 3.0 - t) * 6.0; }
    return p;
}

fn rgb_to_hsl(rgb: vec3<f32>) -> vec3<f32> {
    let max_c = max(max(rgb.r, rgb.g), rgb.b);
    let min_c = min(min(rgb.r, rgb.g), rgb.b);
    let l = (max_c + min_c) * 0.5;
    if max_c == min_c {
        return vec3<f32>(0.0, 0.0, l);
    }
    let delta = max_c - min_c;
    let s = select(delta / (2.0 - max_c - min_c), delta / (max_c + min_c), l < 0.5);
    var h: f32;
    if max_c == rgb.r {
        h = (rgb.g - rgb.b) / delta + select(6.0, 0.0, rgb.g >= rgb.b);
    } else if max_c == rgb.g {
        h = (rgb.b - rgb.r) / delta + 2.0;
    } else {
        h = (rgb.r - rgb.g) / delta + 4.0;
    }
    return vec3<f32>(h / 6.0, s, l);
}

fn hsl_to_rgb(hsl: vec3<f32>) -> vec3<f32> {
    if hsl.y == 0.0 {
        return vec3<f32>(hsl.z);
    }
    let q = select(hsl.z + hsl.y - hsl.z * hsl.y, hsl.z * (1.0 + hsl.y), hsl.z < 0.5);
    let p = 2.0 * hsl.z - q;
    return vec3<f32>(
        hue_to_rgb(p, q, hsl.x + 1.0 / 3.0),
        hue_to_rgb(p, q, hsl.x),
        hue_to_rgb(p, q, hsl.x - 1.0 / 3.0),
    );
}

// FFmpeg macro equivalents
//
// Float equality is deliberate: the C guards are exact comparisons, and widening
// them to an epsilon would move the discontinuity.

fn multiply_f(x: f32, a: f32, b: f32) -> f32 {
    return x * (a * b);
}

fn screen_f(x: f32, a: f32, b: f32) -> f32 {
    return 1.0 - x * ((1.0 - a) * (1.0 - b));
}

fn burn(a: f32, b: f32) -> f32 {
    if a <= 0.0 { return a; }
    return max(0.0, 1.0 - (1.0 - b) / a);
}

fn dodge(a: f32, b: f32) -> f32 {
    if a >= 1.0 { return a; }
    return min(1.0, b / (1.0 - a));
}

/// The 8-bit samples an Rgba8Unorm texel carries, for the bitwise modes.
fn to_u8v(v: vec3<f32>) -> vec3<u32> {
    return vec3<u32>(clamp(v, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0 + 0.5);
}

fn from_u8v(v: vec3<u32>) -> vec3<f32> {
    return vec3<f32>(v) / 255.0;
}

// Per-channel blend helpers

fn overlay_ch(a: f32, b: f32) -> f32 {
    if a < 0.5 { return multiply_f(2.0, a, b); }
    return screen_f(2.0, a, b);
}

fn hard_light_ch(a: f32, b: f32) -> f32 {
    if b < 0.5 { return multiply_f(2.0, b, a); }
    return screen_f(2.0, b, a);
}

fn divide_ch(a: f32, b: f32) -> f32 {
    if b == 0.0 { return 1.0; }
    return a / b;
}

fn freeze_ch(a: f32, b: f32) -> f32 {
    if b == 0.0 { return 0.0; }
    return 1.0 - min(((1.0 - a) * (1.0 - a)) / b, 1.0);
}

fn heat_ch(a: f32, b: f32) -> f32 {
    if a == 0.0 { return 0.0; }
    return 1.0 - min(((1.0 - b) * (1.0 - b)) / a, 1.0);
}

fn glow_ch(a: f32, b: f32) -> f32 {
    if a == 1.0 { return a; }
    return min(1.0, (b * b) / (1.0 - a));
}

fn reflect_ch(a: f32, b: f32) -> f32 {
    if b == 1.0 { return b; }
    return min(1.0, (a * a) / (1.0 - b));
}

// The C is `A == MAX ? MAX : FFMIN(MAX, …*(A > HALF) + …*(A <= HALF))`. Exactly
// one of its two terms is non-zero, so the clamp applies per branch.
fn hard_overlay_ch(a: f32, b: f32) -> f32 {
    if a == 1.0 { return 1.0; }
    if a > 0.5 { return min(1.0, b / (2.0 - 2.0 * a)); }
    return min(1.0, 2.0 * a * b);
}

fn harmonic_ch(a: f32, b: f32) -> f32 {
    if a == 0.0 && b == 0.0 { return 0.0; }
    return 2.0 * a * b / (a + b);
}

fn soft_difference_ch(a: f32, b: f32) -> f32 {
    if a > b {
        if b == 1.0 { return 0.0; }
        return (a - b) / (1.0 - b);
    }
    if b == 0.0 { return 0.0; }
    return (b - a) / b;
}

fn vivid_light_ch(a: f32, b: f32) -> f32 {
    if a < 0.5 { return burn(2.0 * a, b); }
    return dodge(2.0 * (a - 0.5), b);
}

// Fragment

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base    = textureSample(tex_base,    tex_sampler, in.uv);
    let overlay = textureSample(tex_overlay, tex_sampler, in.uv);
    let a = base.rgb;
    let b = overlay.rgb;

    var blend_rgb: vec3<f32>;
    switch u.mode {
        // Normal
        case 0u  { blend_rgb = b; }
        // Multiply
        case 1u  { blend_rgb = a * b; }
        // Screen
        case 2u  { blend_rgb = 1.0 - (1.0 - a) * (1.0 - b); }
        // Overlay
        case 3u  {
            blend_rgb = vec3<f32>(
                overlay_ch(a.r, b.r), overlay_ch(a.g, b.g), overlay_ch(a.b, b.b),
            );
        }
        // SoftLight
        case 4u  { blend_rgb = a * a + 2.0 * (b * (a * (1.0 - a))); }
        // HardLight
        case 5u  {
            blend_rgb = vec3<f32>(
                hard_light_ch(a.r, b.r), hard_light_ch(a.g, b.g), hard_light_ch(a.b, b.b),
            );
        }
        // ColorDodge
        case 6u  {
            blend_rgb = vec3<f32>(dodge(a.r, b.r), dodge(a.g, b.g), dodge(a.b, b.b));
        }
        // ColorBurn
        case 7u  {
            blend_rgb = vec3<f32>(burn(a.r, b.r), burn(a.g, b.g), burn(a.b, b.b));
        }
        // Difference
        case 8u  { blend_rgb = abs(a - b); }
        // Exclusion
        case 9u  { blend_rgb = a + b - 2.0 * a * b; }
        // Add
        case 10u { blend_rgb = min(vec3<f32>(1.0), a + b); }
        // Subtract
        case 11u { blend_rgb = max(vec3<f32>(0.0), a - b); }
        // Darken
        case 12u { blend_rgb = min(a, b); }
        // Lighten
        case 13u { blend_rgb = max(a, b); }
        // Hue: overlay hue + base saturation + base lightness
        case 14u {
            let base_hsl    = rgb_to_hsl(a);
            let overlay_hsl = rgb_to_hsl(b);
            blend_rgb = hsl_to_rgb(vec3<f32>(overlay_hsl.x, base_hsl.y, base_hsl.z));
        }
        // Saturation: base hue + overlay saturation + base lightness
        case 15u {
            let base_hsl    = rgb_to_hsl(a);
            let overlay_hsl = rgb_to_hsl(b);
            blend_rgb = hsl_to_rgb(vec3<f32>(base_hsl.x, overlay_hsl.y, base_hsl.z));
        }
        // Color: overlay hue + overlay saturation + base lightness
        case 16u {
            let base_hsl    = rgb_to_hsl(a);
            let overlay_hsl = rgb_to_hsl(b);
            blend_rgb = hsl_to_rgb(vec3<f32>(overlay_hsl.x, overlay_hsl.y, base_hsl.z));
        }
        // Luminosity: base hue + base saturation + overlay lightness
        case 17u {
            let base_hsl    = rgb_to_hsl(a);
            let overlay_hsl = rgb_to_hsl(b);
            blend_rgb = hsl_to_rgb(vec3<f32>(base_hsl.x, base_hsl.y, overlay_hsl.z));
        }
        // And
        case 18u { blend_rgb = from_u8v(to_u8v(a) & to_u8v(b)); }
        // Average
        case 19u { blend_rgb = (a + b) / 2.0; }
        // Bleach
        case 20u { blend_rgb = (1.0 - b) + (1.0 - a) - 1.0; }
        // Divide
        case 21u {
            blend_rgb = vec3<f32>(
                divide_ch(a.r, b.r), divide_ch(a.g, b.g), divide_ch(a.b, b.b),
            );
        }
        // Extremity
        case 22u { blend_rgb = abs(1.0 - a - b); }
        // Freeze
        case 23u {
            blend_rgb = vec3<f32>(
                freeze_ch(a.r, b.r), freeze_ch(a.g, b.g), freeze_ch(a.b, b.b),
            );
        }
        // Geometric
        case 24u {
            blend_rgb = sqrt(max(a, vec3<f32>(0.0)) * max(b, vec3<f32>(0.0)));
        }
        // Glow
        case 25u {
            blend_rgb = vec3<f32>(glow_ch(a.r, b.r), glow_ch(a.g, b.g), glow_ch(a.b, b.b));
        }
        // GrainExtract
        case 26u { blend_rgb = 0.5 + a - b; }
        // GrainMerge
        case 27u { blend_rgb = a + b - 0.5; }
        // HardMix
        case 28u {
            blend_rgb = select(vec3<f32>(1.0), vec3<f32>(0.0), a < (1.0 - b));
        }
        // HardOverlay
        case 29u {
            blend_rgb = vec3<f32>(
                hard_overlay_ch(a.r, b.r),
                hard_overlay_ch(a.g, b.g),
                hard_overlay_ch(a.b, b.b),
            );
        }
        // Harmonic
        case 30u {
            blend_rgb = vec3<f32>(
                harmonic_ch(a.r, b.r), harmonic_ch(a.g, b.g), harmonic_ch(a.b, b.b),
            );
        }
        // Heat
        case 31u {
            blend_rgb = vec3<f32>(heat_ch(a.r, b.r), heat_ch(a.g, b.g), heat_ch(a.b, b.b));
        }
        // Interpolate
        case 32u { blend_rgb = (2.0 - cos(a * PI) - cos(b * PI)) * 0.25; }
        // LinearLight. The C branches on `b < HALF`, but MAX == 2*HALF makes both
        // arms `b + 2a - 1`; the 8-bit path differs by one LSB.
        case 33u { blend_rgb = b + 2.0 * a - 1.0; }
        // Multiply128
        case 34u { blend_rgb = (a - 0.5) * b / 0.125 + 0.5; }
        // Negation
        case 35u { blend_rgb = 1.0 - abs(1.0 - a - b); }
        // Or
        case 36u { blend_rgb = from_u8v(to_u8v(a) | to_u8v(b)); }
        // Phoenix
        case 37u { blend_rgb = min(a, b) - max(a, b) + 1.0; }
        // PinLight
        case 38u {
            blend_rgb = select(
                max(a, 2.0 * (b - 0.5)),
                min(a, 2.0 * b),
                b < vec3<f32>(0.5),
            );
        }
        // Reflect
        case 39u {
            blend_rgb = vec3<f32>(
                reflect_ch(a.r, b.r), reflect_ch(a.g, b.g), reflect_ch(a.b, b.b),
            );
        }
        // SoftDifference
        case 40u {
            blend_rgb = vec3<f32>(
                soft_difference_ch(a.r, b.r),
                soft_difference_ch(a.g, b.g),
                soft_difference_ch(a.b, b.b),
            );
        }
        // Stain
        case 41u { blend_rgb = 2.0 - a - b; }
        // VividLight
        case 42u {
            blend_rgb = vec3<f32>(
                vivid_light_ch(a.r, b.r),
                vivid_light_ch(a.g, b.g),
                vivid_light_ch(a.b, b.b),
            );
        }
        // Xor
        case 43u { blend_rgb = from_u8v(to_u8v(a) ^ to_u8v(b)); }
        default  { blend_rgb = b; }
    }

    // Apply opacity: modulate blend result against base using overlay.a * opacity.
    // FFmpeg computes `dst = A + (expr - A) * opacity`, and A is the base here.
    // The clamp reproduces the float-to-8-bit conversion for the modes the
    // `DEPTH == 32` branch leaves outside [0, 1].
    let effective_alpha = overlay.a * u.opacity;
    let out_rgb = mix(base.rgb, blend_rgb, effective_alpha);
    return vec4<f32>(clamp(out_rgb, vec3<f32>(0.0), vec3<f32>(1.0)), base.a);
}
