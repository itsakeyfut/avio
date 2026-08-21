//! CPU pixel-blend math for `BlendModeNode` (HSL conversions + per-mode blend).

use super::blend_mode::BlendMode;

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

fn overlay_ch(b: f32, o: f32) -> f32 {
    if b < 0.5 {
        2.0 * b * o
    } else {
        1.0 - 2.0 * (1.0 - b) * (1.0 - o)
    }
}

fn soft_light_d(b: f32) -> f32 {
    if b <= 0.25 {
        ((16.0 * b - 12.0) * b + 4.0) * b
    } else {
        b.sqrt()
    }
}

fn soft_light_ch(b: f32, o: f32) -> f32 {
    if o <= 0.5 {
        b - (1.0 - 2.0 * o) * b * (1.0 - b)
    } else {
        b + (2.0 * o - 1.0) * (soft_light_d(b) - b)
    }
}

#[allow(clippy::many_single_char_names)]
pub(super) fn blend_rgb(mode: BlendMode, base: [f32; 3], ov: [f32; 3]) -> [f32; 3] {
    let [br, bg, bb] = base;
    let [or, og, ob] = ov;
    match mode {
        BlendMode::Normal => ov,
        BlendMode::Multiply => [br * or, bg * og, bb * ob],
        BlendMode::Screen => [
            1.0 - (1.0 - br) * (1.0 - or),
            1.0 - (1.0 - bg) * (1.0 - og),
            1.0 - (1.0 - bb) * (1.0 - ob),
        ],
        BlendMode::Overlay => [overlay_ch(br, or), overlay_ch(bg, og), overlay_ch(bb, ob)],
        BlendMode::SoftLight => [
            soft_light_ch(br, or),
            soft_light_ch(bg, og),
            soft_light_ch(bb, ob),
        ],
        BlendMode::HardLight => [overlay_ch(or, br), overlay_ch(og, bg), overlay_ch(ob, bb)],
        BlendMode::ColorDodge => [
            (br / (1.0 - or + 1e-4)).clamp(0.0, 1.0),
            (bg / (1.0 - og + 1e-4)).clamp(0.0, 1.0),
            (bb / (1.0 - ob + 1e-4)).clamp(0.0, 1.0),
        ],
        BlendMode::ColorBurn => [
            (1.0 - (1.0 - br) / (or + 1e-4)).clamp(0.0, 1.0),
            (1.0 - (1.0 - bg) / (og + 1e-4)).clamp(0.0, 1.0),
            (1.0 - (1.0 - bb) / (ob + 1e-4)).clamp(0.0, 1.0),
        ],
        BlendMode::Difference => [(br - or).abs(), (bg - og).abs(), (bb - ob).abs()],
        BlendMode::Exclusion => [
            br + or - 2.0 * br * or,
            bg + og - 2.0 * bg * og,
            bb + ob - 2.0 * bb * ob,
        ],
        BlendMode::Add => [
            (br + or).clamp(0.0, 1.0),
            (bg + og).clamp(0.0, 1.0),
            (bb + ob).clamp(0.0, 1.0),
        ],
        BlendMode::Subtract => [
            (br - or).clamp(0.0, 1.0),
            (bg - og).clamp(0.0, 1.0),
            (bb - ob).clamp(0.0, 1.0),
        ],
        BlendMode::Darken => [br.min(or), bg.min(og), bb.min(ob)],
        BlendMode::Lighten => [br.max(or), bg.max(og), bb.max(ob)],
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
