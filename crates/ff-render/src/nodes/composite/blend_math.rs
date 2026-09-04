//! CPU pixel-blend math for `BlendModeNode` (HSL conversions + per-mode blend).

use std::f32::consts::PI;

use super::blend_mode::BlendMode;
use super::composite_op::CompositeOp;

#[allow(clippy::many_single_char_names, clippy::float_cmp)]
fn rgb_to_hsl(r: f32, g: f32, b: f32) -> [f32; 3] {
    let max_c = r.max(g).max(b);
    let min_c = r.min(g).min(b);
    let l = max_c.midpoint(min_c);
    if (max_c - min_c).abs() < 1e-6 {
        return [0.0, 0.0, l];
    }
    let delta = max_c - min_c;
    let s = if l < 0.5 {
        delta / (max_c + min_c)
    } else {
        delta / (2.0 - max_c - min_c)
    };
    let h = if max_c == r {
        let raw = (g - b) / delta;
        if g >= b { raw } else { raw + 6.0 }
    } else if max_c == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    } / 6.0;
    [h, s, l]
}

fn hue_to_rgb_cpu(p: f32, q: f32, t_in: f32) -> f32 {
    let t = t_in.rem_euclid(1.0);
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 0.5 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

#[allow(clippy::many_single_char_names)]
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    if s.abs() < 1e-6 {
        return [l, l, l];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    [
        hue_to_rgb_cpu(p, q, h + 1.0 / 3.0),
        hue_to_rgb_cpu(p, q, h),
        hue_to_rgb_cpu(p, q, h - 1.0 / 3.0),
    ]
}

// Per-channel blend math.
//
// Every function below takes `a` = base (`FFmpeg`'s `A`, the `top` pad) and
// `b` = overlay (`FFmpeg`'s `B`, the `bottom` pad), matching the `DEPTH == 32`
// branch of `libavfilter/blend_modes.c` with `MAX = 1.0` and `HALF = 0.5`.
// `shaders/blend.wgsl` mirrors these one for one. Float equality is deliberate:
// the C guards are exact comparisons and shifting them to an epsilon would move
// the discontinuity.

/// `MULTIPLY(x, a, b)`.
fn multiply_f(x: f32, a: f32, b: f32) -> f32 {
    x * (a * b)
}

/// `SCREEN(x, a, b)`.
fn screen_f(x: f32, a: f32, b: f32) -> f32 {
    1.0 - x * ((1.0 - a) * (1.0 - b))
}

/// `BURN(a, b)`.
fn burn(a: f32, b: f32) -> f32 {
    if a <= 0.0 {
        a
    } else {
        (1.0 - (1.0 - b) / a).max(0.0)
    }
}

/// `DODGE(a, b)`.
fn dodge(a: f32, b: f32) -> f32 {
    if a >= 1.0 {
        a
    } else {
        (b / (1.0 - a)).min(1.0)
    }
}

/// The 8-bit sample an `Rgba8Unorm` texel carries for a normalised value.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_u8(v: f32) -> u32 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32
}

/// Normalise an 8-bit sample back to `[0, 1]`.
#[allow(clippy::cast_precision_loss)]
fn from_u8(v: u32) -> f32 {
    v as f32 / 255.0
}

fn overlay_ch(a: f32, b: f32) -> f32 {
    if a < 0.5 {
        multiply_f(2.0, a, b)
    } else {
        screen_f(2.0, a, b)
    }
}

fn hard_light_ch(a: f32, b: f32) -> f32 {
    if b < 0.5 {
        multiply_f(2.0, b, a)
    } else {
        screen_f(2.0, b, a)
    }
}

fn soft_light_ch(a: f32, b: f32) -> f32 {
    a * a + 2.0 * (b * (a * (1.0 - a)))
}

#[allow(clippy::float_cmp)]
fn divide_ch(a: f32, b: f32) -> f32 {
    if b == 0.0 { 1.0 } else { a / b }
}

#[allow(clippy::float_cmp)]
fn freeze_ch(a: f32, b: f32) -> f32 {
    if b == 0.0 {
        0.0
    } else {
        1.0 - (((1.0 - a) * (1.0 - a)) / b).min(1.0)
    }
}

#[allow(clippy::float_cmp)]
fn heat_ch(a: f32, b: f32) -> f32 {
    if a == 0.0 {
        0.0
    } else {
        1.0 - (((1.0 - b) * (1.0 - b)) / a).min(1.0)
    }
}

#[allow(clippy::float_cmp)]
fn glow_ch(a: f32, b: f32) -> f32 {
    if a == 1.0 {
        a
    } else {
        ((b * b) / (1.0 - a)).min(1.0)
    }
}

#[allow(clippy::float_cmp)]
fn reflect_ch(a: f32, b: f32) -> f32 {
    if b == 1.0 {
        b
    } else {
        ((a * a) / (1.0 - b)).min(1.0)
    }
}

/// `A == MAX ? MAX : FFMIN(MAX, …(A > HALF) + …(A <= HALF))`. Exactly one of the
/// C's two terms is non-zero, so the clamp applies per branch.
#[allow(clippy::float_cmp)]
fn hard_overlay_ch(a: f32, b: f32) -> f32 {
    if a == 1.0 {
        1.0
    } else if a > 0.5 {
        (b / (2.0 - 2.0 * a)).min(1.0)
    } else {
        (2.0 * a * b).min(1.0)
    }
}

#[allow(clippy::float_cmp)]
fn harmonic_ch(a: f32, b: f32) -> f32 {
    if a == 0.0 && b == 0.0 {
        0.0
    } else {
        2.0 * a * b / (a + b)
    }
}

fn pin_light_ch(a: f32, b: f32) -> f32 {
    if b < 0.5 {
        a.min(2.0 * b)
    } else {
        a.max(2.0 * (b - 0.5))
    }
}

#[allow(clippy::float_cmp)]
fn soft_difference_ch(a: f32, b: f32) -> f32 {
    if a > b {
        if b == 1.0 { 0.0 } else { (a - b) / (1.0 - b) }
    } else if b == 0.0 {
        0.0
    } else {
        (b - a) / b
    }
}

fn vivid_light_ch(a: f32, b: f32) -> f32 {
    if a < 0.5 {
        burn(2.0 * a, b)
    } else {
        dodge(2.0 * (a - 0.5), b)
    }
}

/// Apply a per-channel blend function across R, G and B.
fn map3(f: impl Fn(f32, f32) -> f32, base: [f32; 3], ov: [f32; 3]) -> [f32; 3] {
    [f(base[0], ov[0]), f(base[1], ov[1]), f(base[2], ov[2])]
}

/// Apply a Porter-Duff operator to a premultiplied source and backdrop.
///
/// `s` and `d` are premultiplied colours, `sa` and `da` their alphas; the return
/// is the premultiplied output colour and its alpha. This is the W3C form
/// `Co = as * Fa * Cs + ab * Fb * Cb` with `s` and `d` substituted, so it reads
/// as `co = s * Fa + d * Fb`. `shaders/blend.wgsl` evaluates the same six
/// expressions; see [`CompositeOp`] for the per-operator formulas.
pub(super) fn composite_rgba(
    op: CompositeOp,
    s: [f32; 3],
    sa: f32,
    d: [f32; 3],
    da: f32,
) -> ([f32; 3], f32) {
    let (fa, fb) = match op {
        CompositeOp::Over => (1.0, 1.0 - sa),
        CompositeOp::Under => (1.0 - da, 1.0),
        CompositeOp::In => (da, 0.0),
        CompositeOp::Out => (1.0 - da, 0.0),
        CompositeOp::Atop => (da, 1.0 - sa),
        CompositeOp::Xor => (1.0 - da, 1.0 - sa),
    };
    (
        [
            s[0] * fa + d[0] * fb,
            s[1] * fa + d[1] * fb,
            s[2] * fa + d[2] * fb,
        ],
        sa * fa + da * fb,
    )
}

// `manual_midpoint`: `Average` stays written as the C's `(A + B) / 2` so it reads
// as the same expression as the shader and the reference. Neither can overflow
// for inputs in `[0, 1]`, and `midpoint` would differ from the shader by an ulp.
#[allow(
    clippy::many_single_char_names,
    clippy::too_many_lines,
    clippy::manual_midpoint
)]
pub(super) fn blend_rgb(mode: BlendMode, base: [f32; 3], ov: [f32; 3]) -> [f32; 3] {
    let [br, bg, bb] = base;
    let [or, og, ob] = ov;
    match mode {
        BlendMode::Normal => ov,
        BlendMode::Multiply => map3(|a, b| multiply_f(1.0, a, b), base, ov),
        BlendMode::Screen => map3(|a, b| screen_f(1.0, a, b), base, ov),
        BlendMode::Overlay => map3(overlay_ch, base, ov),
        BlendMode::SoftLight => map3(soft_light_ch, base, ov),
        BlendMode::HardLight => map3(hard_light_ch, base, ov),
        BlendMode::ColorDodge => map3(dodge, base, ov),
        BlendMode::ColorBurn => map3(burn, base, ov),
        BlendMode::Difference => map3(|a, b| (a - b).abs(), base, ov),
        BlendMode::Exclusion => map3(|a, b| a + b - 2.0 * a * b, base, ov),
        BlendMode::Add => map3(|a, b| (a + b).min(1.0), base, ov),
        BlendMode::Subtract => map3(|a, b| (a - b).max(0.0), base, ov),
        BlendMode::Darken => map3(f32::min, base, ov),
        BlendMode::Lighten => map3(f32::max, base, ov),
        BlendMode::And => map3(|a, b| from_u8(to_u8(a) & to_u8(b)), base, ov),
        BlendMode::Average => map3(|a, b| (a + b) / 2.0, base, ov),
        BlendMode::Bleach => map3(|a, b| (1.0 - b) + (1.0 - a) - 1.0, base, ov),
        BlendMode::Divide => map3(divide_ch, base, ov),
        BlendMode::Extremity => map3(|a, b| (1.0 - a - b).abs(), base, ov),
        BlendMode::Freeze => map3(freeze_ch, base, ov),
        BlendMode::Geometric => map3(|a, b| (a.max(0.0) * b.max(0.0)).sqrt(), base, ov),
        BlendMode::Glow => map3(glow_ch, base, ov),
        BlendMode::GrainExtract => map3(|a, b| 0.5 + a - b, base, ov),
        BlendMode::GrainMerge => map3(|a, b| a + b - 0.5, base, ov),
        BlendMode::HardMix => map3(|a, b| if a < 1.0 - b { 0.0 } else { 1.0 }, base, ov),
        BlendMode::HardOverlay => map3(hard_overlay_ch, base, ov),
        BlendMode::Harmonic => map3(harmonic_ch, base, ov),
        BlendMode::Heat => map3(heat_ch, base, ov),
        BlendMode::Interpolate => map3(
            |a, b| (2.0 - (a * PI).cos() - (b * PI).cos()) * 0.25,
            base,
            ov,
        ),
        // The C branches on `b < HALF`, but `MAX == 2 * HALF` makes both arms
        // `b + 2a - 1`; see the `LinearLight` doc comment.
        BlendMode::LinearLight => map3(|a, b| b + 2.0 * a - 1.0, base, ov),
        BlendMode::Multiply128 => map3(|a, b| (a - 0.5) * b / 0.125 + 0.5, base, ov),
        BlendMode::Negation => map3(|a, b| 1.0 - (1.0 - a - b).abs(), base, ov),
        BlendMode::Or => map3(|a, b| from_u8(to_u8(a) | to_u8(b)), base, ov),
        BlendMode::Phoenix => map3(|a, b| a.min(b) - a.max(b) + 1.0, base, ov),
        BlendMode::PinLight => map3(pin_light_ch, base, ov),
        BlendMode::Reflect => map3(reflect_ch, base, ov),
        BlendMode::SoftDifference => map3(soft_difference_ch, base, ov),
        BlendMode::Stain => map3(|a, b| 2.0 - a - b, base, ov),
        BlendMode::VividLight => map3(vivid_light_ch, base, ov),
        BlendMode::Xor => map3(|a, b| from_u8(to_u8(a) ^ to_u8(b)), base, ov),
        BlendMode::Hue => {
            let [_bh, bs, bl] = rgb_to_hsl(br, bg, bb);
            let [oh, _, _] = rgb_to_hsl(or, og, ob);
            hsl_to_rgb(oh, bs, bl)
        }
        BlendMode::Saturation => {
            let [bh, bs, bl] = rgb_to_hsl(br, bg, bb);
            let [_, os, _] = rgb_to_hsl(or, og, ob);
            let _ = bs;
            hsl_to_rgb(bh, os, bl)
        }
        BlendMode::Color => {
            let [_, _, bl] = rgb_to_hsl(br, bg, bb);
            let [oh, os, _] = rgb_to_hsl(or, og, ob);
            hsl_to_rgb(oh, os, bl)
        }
        BlendMode::Luminosity => {
            let [bh, bs, _] = rgb_to_hsl(br, bg, bb);
            let [_, _, ol] = rgb_to_hsl(or, og, ob);
            hsl_to_rgb(bh, bs, ol)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(mode, base, overlay, expected)`.
    type Case = (BlendMode, [f32; 3], [f32; 3], [f32; 3]);

    /// Every non-HSL mode against three colour pairs.
    ///
    /// The expected values were produced by transcribing `blend_modes.c`'s
    /// `DEPTH == 32` expressions a **second time**, separately from the Rust
    /// above, so a mistranscription here fails the test instead of passing
    /// silently. What this does *not* prove is agreement with a running
    /// `FFmpeg`; that comparison belongs to the reference-image suite (#1671).
    ///
    /// Channel values sit at least 16 LSB away from 0, 128 and 255 so the
    /// discontinuous modes (`HardMix`, `PinLight`, `VividLight`, `HardOverlay`,
    /// `SoftDifference`) are never evaluated next to a branch boundary, and both
    /// sides of every `< 0.5` test are exercised across the pairs.
    // The values are generated, so they carry full `f32` precision and no digit
    // separators; grouping them by hand would invite a transcription error in the
    // one place this file cannot afford one.
    #[allow(clippy::unreadable_literal, clippy::excessive_precision)]
    #[rustfmt::skip]
    const CASES: &[Case] = &[
        (BlendMode::Normal, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.6901961, 0.2509804, 0.0941176]),
        (BlendMode::Normal, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.1882353, 0.9098039, 0.4392157]),
        (BlendMode::Normal, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.3764706, 0.1254902, 0.8156863]),
        (BlendMode::Multiply, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.0866128, 0.0944867, 0.0738178]),
        (BlendMode::Multiply, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.1535409, 0.0856286, 0.2480277]),
        (BlendMode::Multiply, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.2125952, 0.0866128, 0.2047213]),
        (BlendMode::Screen, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.7290734, 0.5329642, 0.8046136]),
        (BlendMode::Screen, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.8503806, 0.9182930, 0.7558939]),
        (BlendMode::Screen, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.7285813, 0.7290734, 0.8619454]),
        (BlendMode::Overlay, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.1732257, 0.1889735, 0.6092272]),
        (BlendMode::Overlay, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.7007612, 0.1712572, 0.5117878]),
        (BlendMode::Overlay, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.4571626, 0.4581469, 0.4094425]),
        (BlendMode::SoftLight, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.1672353, 0.2595606, 0.6469910]),
        (BlendMode::SoftLight, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.7219435, 0.1639970, 0.5348227]),
        (BlendMode::SoftLight, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.5039756, 0.5300366, 0.3696716]),
        (BlendMode::HardLight, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.4581469, 0.1889735, 0.1476355]),
        (BlendMode::HardLight, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.3070819, 0.8365859, 0.4960554]),
        (BlendMode::HardLight, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.4251903, 0.1732257, 0.7238908]),
        (BlendMode::ColorDodge, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.7892377, 0.4025157, 0.4363636]),
        (BlendMode::ColorDodge, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [1.0000000, 1.0000000, 1.0000000]),
        (BlendMode::ColorDodge, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.8648649, 0.4050633, 1.0000000]),
        (BlendMode::ColorBurn, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.0000000, 0.0000000, 0.0000000]),
        (BlendMode::ColorBurn, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.0048077, 0.0416667, 0.0069444]),
        (BlendMode::ColorBurn, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.0000000, 0.0000000, 0.2656250]),
        (BlendMode::Difference, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.5647059, 0.1254902, 0.6901961]),
        (BlendMode::Difference, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.6274510, 0.8156863, 0.1254902]),
        (BlendMode::Difference, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.1882353, 0.5647059, 0.5647059]),
        (BlendMode::Exclusion, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.6424606, 0.4384775, 0.7307958]),
        (BlendMode::Exclusion, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.6968397, 0.8326644, 0.5078662]),
        (BlendMode::Exclusion, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.5159862, 0.6424606, 0.6572241]),
        (BlendMode::Add, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.8156863, 0.6274510, 0.8784314]),
        (BlendMode::Add, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [1.0000000, 1.0000000, 1.0000000]),
        (BlendMode::Add, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.9411765, 0.8156863, 1.0000000]),
        (BlendMode::Subtract, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.0000000, 0.1254902, 0.6901961]),
        (BlendMode::Subtract, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.6274510, 0.0000000, 0.1254902]),
        (BlendMode::Subtract, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.1882353, 0.5647059, 0.0000000]),
        (BlendMode::Darken, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.1254902, 0.2509804, 0.0941176]),
        (BlendMode::Darken, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.1882353, 0.0941176, 0.4392157]),
        (BlendMode::Darken, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.3764706, 0.1254902, 0.2509804]),
        (BlendMode::Lighten, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.6901961, 0.3764706, 0.7843137]),
        (BlendMode::Lighten, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.8156863, 0.9098039, 0.5647059]),
        (BlendMode::Lighten, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.5647059, 0.6901961, 0.8156863]),
        (BlendMode::And, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.1254902, 0.2509804, 0.0313725]),
        (BlendMode::And, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.0627451, 0.0313725, 0.0627451]),
        (BlendMode::And, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.0000000, 0.1254902, 0.2509804]),
        (BlendMode::Average, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.4078431, 0.3137255, 0.4392157]),
        (BlendMode::Average, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.5019608, 0.5019608, 0.5019608]),
        (BlendMode::Average, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.4705882, 0.4078431, 0.5333333]),
        (BlendMode::Bleach, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.1843137, 0.3725490, 0.1215686]),
        (BlendMode::Bleach, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [-0.0039216, -0.0039216, -0.0039216]),
        (BlendMode::Bleach, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.0588235, 0.1843137, -0.0666667]),
        (BlendMode::Divide, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.1818182, 1.5000000, 8.3333333]),
        (BlendMode::Divide, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [4.3333333, 0.1034483, 1.2857143]),
        (BlendMode::Divide, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [1.5000000, 5.5000000, 0.3076923]),
        (BlendMode::Extremity, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.1843137, 0.3725490, 0.1215686]),
        (BlendMode::Extremity, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.0039216, 0.0039216, 0.0039216]),
        (BlendMode::Extremity, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.0588235, 0.1843137, 0.0666667]),
        (BlendMode::Freeze, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.0000000, 0.0000000, 0.5057190]),
        (BlendMode::Freeze, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.8195261, 0.0980223, 0.5685924]),
        (BlendMode::Freeze, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.4966912, 0.2351716, 0.3121983]),
        (BlendMode::Geometric, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.2943006, 0.3073869, 0.2716942]),
        (BlendMode::Geometric, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.3918430, 0.2926237, 0.4980238]),
        (BlendMode::Geometric, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.4610804, 0.2943006, 0.4524613]),
        (BlendMode::Glow, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.5447287, 0.1010236, 0.0410695]),
        (BlendMode::Glow, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.1922403, 0.9137425, 0.4431726]),
        (BlendMode::Glow, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.3255962, 0.0508315, 0.8882866]),
        (BlendMode::GrainExtract, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [-0.0647059, 0.6254902, 1.1901961]),
        (BlendMode::GrainExtract, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [1.1274510, -0.3156863, 0.6254902]),
        (BlendMode::GrainExtract, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.6882353, 1.0647059, -0.0647059]),
        (BlendMode::GrainMerge, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.3156863, 0.1274510, 0.3784314]),
        (BlendMode::GrainMerge, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.5039216, 0.5039216, 0.5039216]),
        (BlendMode::GrainMerge, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.4411765, 0.3156863, 0.5666667]),
        (BlendMode::HardMix, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.0000000, 0.0000000, 0.0000000]),
        (BlendMode::HardMix, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [1.0000000, 1.0000000, 1.0000000]),
        (BlendMode::HardMix, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.0000000, 0.0000000, 1.0000000]),
        (BlendMode::HardOverlay, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.1732257, 0.1889735, 0.2181818]),
        (BlendMode::HardOverlay, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.5106383, 0.1712572, 0.5045045]),
        (BlendMode::HardOverlay, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.4324324, 0.2025316, 0.4094425]),
        (BlendMode::Harmonic, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.2123680, 0.3011765, 0.1680672]),
        (BlendMode::Harmonic, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.3058824, 0.1705882, 0.4941176]),
        (BlendMode::Harmonic, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.4517647, 0.2123680, 0.3838524]),
        (BlendMode::Heat, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.2351716, 0.0000000, 0.0000000]),
        (BlendMode::Heat, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.1921380, 0.9135621, 0.4431100]),
        (BlendMode::Heat, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.3115196, 0.0000000, 0.8646446]),
        (BlendMode::Interpolate, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.4098259, 0.2291659, 0.4556190]),
        (BlendMode::Interpolate, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.5017013, 0.5008793, 0.5030203]),
        (BlendMode::Interpolate, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.4558678, 0.4098259, 0.5330159]),
        (BlendMode::LinearLight, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [-0.0588235, 0.0039216, 0.6627451]),
        (BlendMode::LinearLight, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.8196078, 0.0980392, 0.5686275]),
        (BlendMode::LinearLight, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.5058824, 0.5058824, 0.3176471]),
        (BlendMode::Multiply128, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [-1.5678816, 0.2519723, 0.7140715]),
        (BlendMode::Multiply128, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.9753864, -2.4541869, 0.7273587]),
        (BlendMode::Multiply128, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.6948789, 0.6909419, -1.1249750]),
        (BlendMode::Negation, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.8156863, 0.6274510, 0.8784314]),
        (BlendMode::Negation, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.9960784, 0.9960784, 0.9960784]),
        (BlendMode::Negation, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.9411765, 0.8156863, 0.9333333]),
        (BlendMode::Or, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.6901961, 0.3764706, 0.8470588]),
        (BlendMode::Or, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.9411765, 0.9725490, 0.9411765]),
        (BlendMode::Or, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.9411765, 0.6901961, 0.8156863]),
        (BlendMode::Phoenix, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.4352941, 0.8745098, 0.3098039]),
        (BlendMode::Phoenix, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.3725490, 0.1843137, 0.8745098]),
        (BlendMode::Phoenix, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.8117647, 0.4352941, 0.4352941]),
        (BlendMode::PinLight, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.3803922, 0.3764706, 0.1882353]),
        (BlendMode::PinLight, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.3764706, 0.8196078, 0.5647059]),
        (BlendMode::PinLight, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.5647059, 0.2509804, 0.6313725]),
        (BlendMode::Reflect, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.0508315, 0.1892208, 0.6790595]),
        (BlendMode::Reflect, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.8196268, 0.0982097, 0.5686549]),
        (BlendMode::Reflect, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.5114317, 0.5447287, 0.3417605]),
        (BlendMode::SoftDifference, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.8181818, 0.1675393, 0.7619048]),
        (BlendMode::SoftDifference, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.7729469, 0.8965517, 0.2237762]),
        (BlendMode::SoftDifference, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.3018868, 0.6457399, 0.6923077]),
        (BlendMode::Stain, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [1.1843137, 1.3725490, 1.1215686]),
        (BlendMode::Stain, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.9960784, 0.9960784, 0.9960784]),
        (BlendMode::Stain, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [1.0588235, 1.1843137, 0.9333333]),
        (BlendMode::VividLight, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.0000000, 0.0052083, 0.2181818]),
        (BlendMode::VividLight, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.5106383, 0.5208333, 0.5045045]),
        (BlendMode::VividLight, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.4324324, 0.2025316, 0.6328125]),
        (BlendMode::Xor, [32.0 / 255.0, 96.0 / 255.0, 200.0 / 255.0], [176.0 / 255.0, 64.0 / 255.0, 24.0 / 255.0], [0.5647059, 0.1254902, 0.8156863]),
        (BlendMode::Xor, [208.0 / 255.0, 24.0 / 255.0, 144.0 / 255.0], [48.0 / 255.0, 232.0 / 255.0, 112.0 / 255.0], [0.8784314, 0.9411765, 0.8784314]),
        (BlendMode::Xor, [144.0 / 255.0, 176.0 / 255.0, 64.0 / 255.0], [96.0 / 255.0, 32.0 / 255.0, 208.0 / 255.0], [0.9411765, 0.5647059, 0.5647059]),
    ];

    fn close3(got: [f32; 3], want: [f32; 3]) -> bool {
        (0..3).all(|c| (got[c] - want[c]).abs() < 1e-4)
    }

    #[test]
    fn blend_rgb_should_match_the_ffmpeg_reference_for_every_mode() {
        for &(mode, base, ov, want) in CASES {
            let got = blend_rgb(mode, base, ov);
            assert!(
                close3(got, want),
                "{mode:?}: expected {want:?}, got {got:?} (base {base:?}, overlay {ov:?})"
            );
        }
    }

    #[test]
    fn blend_rgb_reference_table_should_cover_every_non_hsl_mode() {
        let mut modes: Vec<u32> = CASES.iter().map(|&(m, ..)| m as u32).collect();
        modes.sort_unstable();
        modes.dedup();
        // 44 variants less the four HSL modes, which have no `all_mode` token
        // (#1219) and are covered by the GPU/CPU agreement test instead.
        assert_eq!(modes.len(), 40, "a new mode needs rows in CASES");
        assert_eq!(CASES.len(), modes.len() * 3);
    }

    /// `(op, s, sa, d, da, expected co, expected ao)` for the Porter-Duff
    /// operators.
    ///
    /// The expected values were produced by transcribing the W3C `Fa`/`Fb` table
    /// a **second time**, in its straight form `Co = as * Fa * Cs + ab * Fb * Cb`,
    /// while [`composite_rgba`] works in the premultiplied form. Agreement
    /// between the two routes is evidence neither dropped a factor.
    ///
    /// Both alphas sit strictly inside `(0, 1)` and differ, or `In`, `Out` and
    /// `Atop` degenerate; the colours differ per channel so a dropped channel
    /// shows (RK-022). All six operators give distinct results on every row.
    #[allow(clippy::unreadable_literal, clippy::excessive_precision)]
    #[rustfmt::skip]
    const COMPOSITE_CASES: &[(CompositeOp, [f32; 3], f32, [f32; 3], f32, [f32; 3], f32)] = &[
        (CompositeOp::Over, [0.4800000, 0.1800000, 0.0600000], 0.6000, [0.0800000, 0.2800000, 0.3600000], 0.4000, [0.5120000, 0.2920000, 0.2040000], 0.7600000),
        (CompositeOp::Over, [0.0375000, 0.1375000, 0.2125000], 0.2500, [0.6750000, 0.3375000, 0.1500000], 0.7500, [0.5437500, 0.3906250, 0.3250000], 0.8125000),
        (CompositeOp::Over, [0.4500000, 0.8100000, 0.3150000], 0.9000, [0.0525000, 0.0150000, 0.0900000], 0.1500, [0.4552500, 0.8115000, 0.3240000], 0.9150000),
        (CompositeOp::Under, [0.4800000, 0.1800000, 0.0600000], 0.6000, [0.0800000, 0.2800000, 0.3600000], 0.4000, [0.3680000, 0.3880000, 0.3960000], 0.7600000),
        (CompositeOp::Under, [0.0375000, 0.1375000, 0.2125000], 0.2500, [0.6750000, 0.3375000, 0.1500000], 0.7500, [0.6843750, 0.3718750, 0.2031250], 0.8125000),
        (CompositeOp::Under, [0.4500000, 0.8100000, 0.3150000], 0.9000, [0.0525000, 0.0150000, 0.0900000], 0.1500, [0.4350000, 0.7035000, 0.3577500], 0.9150000),
        (CompositeOp::In, [0.4800000, 0.1800000, 0.0600000], 0.6000, [0.0800000, 0.2800000, 0.3600000], 0.4000, [0.1920000, 0.0720000, 0.0240000], 0.2400000),
        (CompositeOp::In, [0.0375000, 0.1375000, 0.2125000], 0.2500, [0.6750000, 0.3375000, 0.1500000], 0.7500, [0.0281250, 0.1031250, 0.1593750], 0.1875000),
        (CompositeOp::In, [0.4500000, 0.8100000, 0.3150000], 0.9000, [0.0525000, 0.0150000, 0.0900000], 0.1500, [0.0675000, 0.1215000, 0.0472500], 0.1350000),
        (CompositeOp::Out, [0.4800000, 0.1800000, 0.0600000], 0.6000, [0.0800000, 0.2800000, 0.3600000], 0.4000, [0.2880000, 0.1080000, 0.0360000], 0.3600000),
        (CompositeOp::Out, [0.0375000, 0.1375000, 0.2125000], 0.2500, [0.6750000, 0.3375000, 0.1500000], 0.7500, [0.0093750, 0.0343750, 0.0531250], 0.0625000),
        (CompositeOp::Out, [0.4500000, 0.8100000, 0.3150000], 0.9000, [0.0525000, 0.0150000, 0.0900000], 0.1500, [0.3825000, 0.6885000, 0.2677500], 0.7650000),
        (CompositeOp::Atop, [0.4800000, 0.1800000, 0.0600000], 0.6000, [0.0800000, 0.2800000, 0.3600000], 0.4000, [0.2240000, 0.1840000, 0.1680000], 0.4000000),
        (CompositeOp::Atop, [0.0375000, 0.1375000, 0.2125000], 0.2500, [0.6750000, 0.3375000, 0.1500000], 0.7500, [0.5343750, 0.3562500, 0.2718750], 0.7500000),
        (CompositeOp::Atop, [0.4500000, 0.8100000, 0.3150000], 0.9000, [0.0525000, 0.0150000, 0.0900000], 0.1500, [0.0727500, 0.1230000, 0.0562500], 0.1500000),
        (CompositeOp::Xor, [0.4800000, 0.1800000, 0.0600000], 0.6000, [0.0800000, 0.2800000, 0.3600000], 0.4000, [0.3200000, 0.2200000, 0.1800000], 0.5200000),
        (CompositeOp::Xor, [0.0375000, 0.1375000, 0.2125000], 0.2500, [0.6750000, 0.3375000, 0.1500000], 0.7500, [0.5156250, 0.2875000, 0.1656250], 0.6250000),
        (CompositeOp::Xor, [0.4500000, 0.8100000, 0.3150000], 0.9000, [0.0525000, 0.0150000, 0.0900000], 0.1500, [0.3877500, 0.6900000, 0.2767500], 0.7800000),
    ];

    #[test]
    fn composite_rgba_should_match_the_porter_duff_reference() {
        for &(op, s, sa, d, da, want_co, want_ao) in COMPOSITE_CASES {
            let (co, ao) = composite_rgba(op, s, sa, d, da);
            assert!(
                close3(co, want_co),
                "{op:?}: expected colour {want_co:?}, got {co:?} (s {s:?} sa {sa}, d {d:?} da {da})"
            );
            assert!(
                (ao - want_ao).abs() < 1e-4,
                "{op:?}: expected alpha {want_ao}, got {ao}"
            );
        }
    }

    #[test]
    fn composite_reference_table_should_cover_every_operator() {
        let mut ops: Vec<u32> = COMPOSITE_CASES.iter().map(|&(op, ..)| op as u32).collect();
        ops.sort_unstable();
        ops.dedup();
        assert_eq!(ops.len(), 6, "a new operator needs rows in COMPOSITE_CASES");
        assert_eq!(COMPOSITE_CASES.len(), ops.len() * 3);
    }

    /// `Over` has to stay exactly what the shader wrote before #1670,
    /// `mix(d, blend, sa)` and `sa + da * (1 - sa)`, because the 44-mode
    /// agreement test and the parity suite are the regression net for it.
    #[test]
    fn composite_rgba_over_should_equal_the_pre_1670_expression() {
        for &(_, s, sa, d, da, ..) in COMPOSITE_CASES {
            let (co, ao) = composite_rgba(CompositeOp::Over, s, sa, d, da);
            for c in 0..3 {
                // `s` is the premultiplied blend result, so `blend = s / sa`.
                let blend = s[c] / sa;
                let mixed = d[c] + (blend - d[c]) * sa;
                assert!(
                    (co[c] - mixed).abs() < 1e-5,
                    "Over channel {c}: {co:?} vs mix() {mixed}"
                );
            }
            assert!((ao - (sa + da * (1.0 - sa))).abs() < 1e-6, "Over alpha");
        }
    }

    /// Each guarded formula has an exact-equality escape in the C that a
    /// mid-range colour pair never reaches, so the table above cannot cover it.
    #[test]
    fn blend_rgb_should_take_the_guarded_branch_at_each_singularity() {
        let zero = [0.0; 3];
        let one = [1.0; 3];
        let half = [0.5; 3];

        assert!(close3(blend_rgb(BlendMode::Divide, half, zero), one));
        assert!(close3(blend_rgb(BlendMode::Freeze, half, zero), zero));
        assert!(close3(blend_rgb(BlendMode::Heat, zero, half), zero));
        assert!(close3(blend_rgb(BlendMode::Glow, one, half), one));
        assert!(close3(blend_rgb(BlendMode::Reflect, half, one), one));
        assert!(close3(blend_rgb(BlendMode::HardOverlay, one, half), one));
        assert!(close3(blend_rgb(BlendMode::Harmonic, zero, zero), zero));
        assert!(close3(blend_rgb(BlendMode::ColorDodge, one, half), one));
        assert!(close3(blend_rgb(BlendMode::ColorBurn, zero, half), zero));
        // `SoftDifference`'s other guard (`overlay == 1` on the `base > overlay`
        // side) is unreachable for in-range inputs: `base > 1` cannot hold.
        assert!(close3(
            blend_rgb(BlendMode::SoftDifference, zero, zero),
            zero
        ));
    }

    /// The `DEPTH == 32` branch applies no `CLIP`, so these modes deliberately
    /// leave `[0, 1]`; the shader's final `clamp` and the `Rgba8Unorm` write are
    /// what bring them back, reproducing `FFmpeg`'s float-to-8-bit conversion.
    #[test]
    fn blend_rgb_should_leave_the_unclamped_modes_outside_the_unit_range() {
        let stain = blend_rgb(BlendMode::Stain, [0.1; 3], [0.1; 3]);
        assert!(
            stain[0] > 1.0,
            "Stain must exceed 1 for dark inputs; got {}",
            stain[0]
        );
        let bleach = blend_rgb(BlendMode::Bleach, [0.9; 3], [0.9; 3]);
        assert!(
            bleach[0] < 0.0,
            "Bleach must go below 0 for bright inputs; got {}",
            bleach[0]
        );
    }
}
