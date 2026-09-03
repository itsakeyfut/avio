//! Unsafe-free helpers for the timeline presentation loop.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]
// `a`/`b` (frames), `w`/`h` (dims), `x`/`y` (coords) are the natural names here.
#![allow(clippy::many_single_char_names)]

use ff_filter::{XfadeTransition, xfade_frand};

/// Blends the outgoing frame `a` and incoming frame `b` (packed RGBA, `w*h*4`) at
/// transition progress `alpha` (`0` = all A, `1` = all B) using the `xfade` `kind`.
///
/// `dims` is the frame `(width, height)`, tupled the way the rest of this crate passes a
/// size around (`PreviewCompositor::composite` takes `canvas: (u32, u32)`).
///
/// `wipe*`, `slide*`, `dissolve` and the `fadeblack` / `fadewhite` dips are rendered
/// host-side here. `fade` and the geometric / mosaic kinds (`fadegrays`, `circleopen`,
/// `circleclose`, `pixelize`) fall through to the linear cross-blend — exact fidelity
/// for those is deferred to the GPU compositing work (#1365). Falls back to the linear
/// blend when the buffers are mismatched or their length is not `w*h*4`.
///
/// `dissolve_field` is an optional [`ff_filter::xfade_frand_field`] for these dimensions, which
/// only the `dissolve` kind reads. `None` computes the hash per pixel exactly as before,
/// so passing it changes nothing but the cost; `Some` is what keeps a 4 K dissolve inside
/// a 30 fps budget (#1736). A field whose length is not `w * h` is ignored rather than
/// indexed: the length is the whole of what ties a cached field to a frame, and a stale
/// one from a different size must not be trusted.
///
/// Public because it is the reference the GPU transition nodes are compared against:
/// `avio`'s transition parity tests run each `ff_render` node beside this function.
pub fn apply_xfade(
    kind: XfadeTransition,
    a: &[u8],
    b: &[u8],
    alpha: f32,
    dims: (u32, u32),
    dissolve_field: Option<&[f32]>,
    dst: &mut Vec<u8>,
) {
    let (w, h) = dims;
    let expected = (w as usize) * (h as usize) * 4;
    if a.len() != b.len() || a.len() != expected {
        blend_rgba(a, b, alpha, dst);
        return;
    }
    let p = alpha.clamp(0.0, 1.0);
    let (wf, hf) = (w as f32, h as f32);
    match kind {
        // `FFmpeg` computes an integer edge `z` and compares the pixel index against it
        // (`vf_xfade.c`, `WIPE*_TRANSITION`), with its own `progress` running 1 -> 0.
        // Transcribed with `progress = 1 - p`, that is the four rules below. The edge has
        // to be the integer one: comparing against `p * width` in float excludes the
        // column at `x == floor(w * p)` that `FFmpeg` includes, which is the whole of the
        // divergence this reference used to carry (#1732).
        //
        // The endpoints are asymmetric as a result -- `WipeRight` already shows one
        // column of B at `p = 0`, and `WipeLeft` still holds one column of A at `p = 1`.
        // That is `FFmpeg`'s behaviour, and matching it is the point of this function.
        XfadeTransition::WipeRight => {
            let z = (wf * p) as i64;
            wipe(a, b, w, h, dst, move |x, _| i64::from(x) <= z);
        }
        XfadeTransition::WipeLeft => {
            let z = (wf * (1.0 - p)) as i64;
            wipe(a, b, w, h, dst, move |x, _| i64::from(x) > z);
        }
        XfadeTransition::WipeDown => {
            let z = (hf * p) as i64;
            wipe(a, b, w, h, dst, move |_, y| i64::from(y) <= z);
        }
        XfadeTransition::WipeUp => {
            let z = (hf * (1.0 - p)) as i64;
            wipe(a, b, w, h, dst, move |_, y| i64::from(y) > z);
        }
        XfadeTransition::SlideLeft => slide(a, b, w, h, dst, (p * wf) as i64, 0),
        XfadeTransition::SlideRight => slide(a, b, w, h, dst, -((p * wf) as i64), 0),
        XfadeTransition::SlideUp => slide(a, b, w, h, dst, 0, (p * hf) as i64),
        XfadeTransition::SlideDown => slide(a, b, w, h, dst, 0, -((p * hf) as i64)),
        XfadeTransition::Dissolve => dissolve(a, b, w, h, dst, p, dissolve_field),
        XfadeTransition::FadeBlack => dip(a, b, [0, 0, 0], dst, p),
        XfadeTransition::FadeWhite => dip(a, b, [255, 255, 255], dst, p),
        // `Fade` and the deferred geometric/mosaic kinds (plus any future variant of
        // this `#[non_exhaustive]` enum) render as the linear cross-dissolve.
        _ => blend_rgba(a, b, alpha, dst),
    }
}

/// Per-pixel hard-edge pick between `a` and `b` (wipes).
fn wipe(a: &[u8], b: &[u8], w: u32, h: u32, dst: &mut Vec<u8>, is_b: impl Fn(u32, u32) -> bool) {
    dst.resize(a.len(), 0);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let src = if is_b(x, y) { b } else { a };
            dst[i..i + 4].copy_from_slice(&src[i..i + 4]);
        }
    }
}

/// Slide: the outgoing frame `a` translates by `(dx, dy)`; where it leaves the frame
/// the incoming frame `b` has slid into view from the opposite edge.
fn slide(a: &[u8], b: &[u8], w: u32, h: u32, dst: &mut Vec<u8>, dx: i64, dy: i64) {
    dst.resize(a.len(), 0);
    let (wi, hi) = (i64::from(w), i64::from(h));
    for y in 0..hi {
        for x in 0..wi {
            let (sx, sy) = (x + dx, y + dy);
            let (src, ux, uy) = if (0..wi).contains(&sx) && (0..hi).contains(&sy) {
                (a, sx, sy)
            } else {
                (b, sx.rem_euclid(wi), sy.rem_euclid(hi))
            };
            let di = ((y * wi + x) * 4) as usize;
            let si = ((uy * wi + ux) * 4) as usize;
            dst[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
}

/// Dissolve: reveal `b` at every pixel whose [`xfade_frand`] value has been passed by
/// `p`, which is `FFmpeg`'s own selection.
///
/// `vf_xfade.c` writes it as `smooth = frand(x,y)*2 + progress*2 - 1.5` and picks A
/// where `smooth >= 0.5`; with its `progress = 1 - p` that reduces to B where
/// [`xfade_frand`]`(x, y) < p`.
///
/// `field` is the same selection tabulated ([`ff_filter::xfade_frand_field`]); it is used only when
/// its length matches this frame, so a field left over from another size falls back to
/// computing rather than reading the wrong pixel.
fn dissolve(a: &[u8], b: &[u8], w: u32, h: u32, dst: &mut Vec<u8>, p: f32, field: Option<&[f32]>) {
    dst.resize(a.len(), 0);
    let field = field.filter(|f| f.len() == (w as usize) * (h as usize));
    for y in 0..h {
        for x in 0..w {
            let n = (y * w + x) as usize;
            let frand = match field {
                Some(f) => f[n],
                None => xfade_frand(x, y),
            };
            let i = n * 4;
            let src = if frand < p { b } else { a };
            dst[i..i + 4].copy_from_slice(&src[i..i + 4]);
        }
    }
}

/// Two-phase dip through `color`, exactly as `FFmpeg`'s `fadeblack` / `fadewhite`.
///
/// `vf_xfade.c` (`FADEBLACK_TRANSITION`) nests three `mix`es around two `smoothstep`s
/// with a fixed `phase` of 0.2, where `mix(a, b, m) = a*m + b*(1-m)` and its `progress`
/// runs 1 -> 0:
///
/// ```text
/// mix(mix(A, bg, smoothstep(1-phase, 1, progress)),
///     mix(bg, B, smoothstep(phase,   1, progress)), progress)
/// ```
///
/// The shape that produces is *not* a linear dip: it reaches the solid colour by about
/// a fifth of the way in and holds it through the middle, where a linear dip would only
/// touch it instantaneously at the midpoint. Measured against a real export, the linear
/// version this replaced diverged by a mean of 78 (#1732).
///
/// Alpha rides along with `bg` fully opaque, so a dip over transparency stays sane.
///
/// `color` is `FFmpeg`'s dip *luma level* rather than a displayable colour; see
/// [`expand_luma`] for why the distinction matters.
fn dip(a: &[u8], b: &[u8], color: [u8; 3], dst: &mut Vec<u8>, p: f32) {
    /// `FFmpeg`'s fixed dip phase: the fraction of the transition spent reaching the
    /// solid colour at each end.
    const PHASE: f32 = 0.2;

    dst.resize(a.len(), 0);
    let bg = [
        expand_luma(color[0]),
        expand_luma(color[1]),
        expand_luma(color[2]),
        255.0,
    ];
    // `FFmpeg`'s progress is the complement of ours, and both `smoothstep`s are constant
    // over the frame, so they are evaluated once here rather than per pixel.
    let g = 1.0 - p;
    let s1 = smoothstep(1.0 - PHASE, 1.0, g);
    let s2 = smoothstep(PHASE, 1.0, g);
    for (i, out) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let j = i * 4;
        for c in 0..4 {
            let av = f32::from(a[j + c]);
            let bv = f32::from(b[j + c]);
            let out_a = av * s1 + bg[c] * (1.0 - s1);
            let out_b = bg[c] * s2 + bv * (1.0 - s2);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                // Truncating, not rounding: `FFmpeg` assigns the float result straight
                // into a `uint8_t`, and the GPU node is pinned to this function.
                out[c] = (out_a * g + out_b * (1.0 - g)).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// The full-range RGB value a limited-range luma level expands to, kept unclamped.
///
/// `FFmpeg` dips toward `black[0] = 0` and `white[0] = max_value` (`vf_xfade.c`
/// `config_output`) -- **luma 0 and 255, not video black and white**, which in a
/// limited-range `yuv420p` pipeline sit outside the displayable range. It then mixes in
/// that space and the conversion to RGB happens afterwards, so a reference that mixes
/// toward RGB 0 / 255 instead lands up to `16 * 255 / 219 ~= 18.6` away in the middle of
/// the dip. That was measured exactly: mixing toward RGB black gave a worst-frame mean of
/// 18.7 against a real export, and mixing toward this expanded value gives 2.7, which is
/// the encode round trip's own floor (#1732).
///
/// Mixing in RGB commutes with the conversion because it is affine, so expanding the
/// endpoint here and clamping only after the blend reproduces `FFmpeg` exactly.
fn expand_luma(level: u8) -> f32 {
    (f32::from(level) - 16.0) * 255.0 / 219.0
}

/// `FFmpeg`'s `smoothstep` (`vf_xfade.c`), the Hermite curve `t*t*(3-2t)` over a
/// clamped `t`.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Blend two packed-RGBA buffers: `dst[i] = (1 − alpha) · a[i] + alpha · b[i]`.
///
/// If `a` and `b` have different lengths, `dst` is set to a copy of `a`.
/// The alpha channel (byte index 3, 7, 11, …) is blended identically to the
/// colour channels so that transparency transitions work correctly.
pub(super) fn blend_rgba(a: &[u8], b: &[u8], alpha: f32, dst: &mut Vec<u8>) {
    if a.len() != b.len() {
        dst.resize(a.len(), 0);
        dst.copy_from_slice(a);
        return;
    }
    dst.resize(a.len(), 0);
    let inv = 1.0_f32 - alpha;
    for ((d, av), bv) in dst.iter_mut().zip(a.iter()).zip(b.iter()) {
        *d = (f32::from(*av) * inv + f32::from(*bv) * alpha) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff_filter::{xfade_frand, xfade_frand_field};

    #[test]
    fn blend_rgba_at_zero_alpha_should_return_a() {
        let a = vec![200u8, 100, 50, 255];
        let b = vec![0u8, 0, 0, 255];
        let mut dst = Vec::new();
        blend_rgba(&a, &b, 0.0, &mut dst);
        assert_eq!(dst, a);
    }

    #[test]
    fn blend_rgba_at_full_alpha_should_return_b() {
        let a = vec![0u8, 0, 0, 255];
        let b = vec![200u8, 100, 50, 255];
        let mut dst = Vec::new();
        blend_rgba(&a, &b, 1.0, &mut dst);
        assert_eq!(dst, b);
    }

    #[test]
    fn blend_rgba_at_half_alpha_should_average() {
        let a = vec![100u8, 200, 0, 255];
        let b = vec![200u8, 0, 100, 255];
        let mut dst = Vec::new();
        blend_rgba(&a, &b, 0.5, &mut dst);
        // (100 * 0.5 + 200 * 0.5) as u8 = 150
        assert_eq!(dst[0], 150);
        // (200 * 0.5 + 0 * 0.5) as u8 = 100
        assert_eq!(dst[1], 100);
    }

    #[test]
    fn blend_rgba_mismatched_lengths_should_copy_a() {
        let a = vec![1u8, 2, 3, 4];
        let b = vec![5u8, 6];
        let mut dst = Vec::new();
        blend_rgba(&a, &b, 0.5, &mut dst);
        assert_eq!(dst, a);
    }

    // apply_xfade (kind-aware transition blend)

    // A 4x1 packed-RGBA frame filled with one colour.
    fn frame(color: [u8; 4]) -> Vec<u8> {
        color.repeat(4)
    }
    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    #[test]
    fn apply_xfade_fade_should_match_linear_blend() {
        let (a, b) = (frame(RED), frame(BLUE));
        let (mut x, mut y) = (Vec::new(), Vec::new());
        apply_xfade(XfadeTransition::Fade, &a, &b, 0.5, (4, 1), None, &mut x);
        blend_rgba(&a, &b, 0.5, &mut y);
        assert_eq!(x, y, "Fade == linear blend");
    }

    #[test]
    fn apply_xfade_wiperight_half_should_fill_b_up_to_ffmpegs_integer_column() {
        let (a, b) = (frame(RED), frame(BLUE));
        let mut dst = Vec::new();
        apply_xfade(
            XfadeTransition::WipeRight,
            &a,
            &b,
            0.5,
            (4, 1),
            None,
            &mut dst,
        );
        // `FFmpeg` takes clip B where `x <= z` with the integer `z = width * progress`,
        // so `z = 2` puts *three* columns on B, not two. The inclusive comparison is the
        // whole of the one-column divergence #1732 fixed; a `x < w * p` threshold gives
        // 0,1 here and drifts from the export.
        assert_eq!(&dst[0..4], &BLUE, "col 0 = B");
        assert_eq!(&dst[4..8], &BLUE, "col 1 = B");
        assert_eq!(&dst[8..12], &BLUE, "col 2 = B (x <= z, inclusive)");
        assert_eq!(&dst[12..16], &RED, "col 3 = A");
    }

    #[test]
    fn apply_xfade_wipeleft_half_should_fill_b_past_ffmpegs_integer_column() {
        let (a, b) = (frame(RED), frame(BLUE));
        let mut dst = Vec::new();
        apply_xfade(
            XfadeTransition::WipeLeft,
            &a,
            &b,
            0.5,
            (4, 1),
            None,
            &mut dst,
        );
        // The mirror of `WipeRight`, and deliberately *not* its exact complement:
        // `FFmpeg` takes clip B where `x > z` with `z = width * (1 - progress) = 2`, so
        // only column 3 flips. The two rules together leave column 2 on B for one and on
        // A for the other at the same progress -- FFmpeg's asymmetry, reproduced.
        assert_eq!(&dst[0..4], &RED, "col 0 = A");
        assert_eq!(&dst[4..8], &RED, "col 1 = A");
        assert_eq!(&dst[8..12], &RED, "col 2 = A (x > z, exclusive)");
        assert_eq!(&dst[12..16], &BLUE, "col 3 = B");
    }

    #[test]
    fn apply_xfade_boundaries_should_be_all_a_or_all_b() {
        // The kinds whose endpoints are clean. The four `FFmpeg` wipes are not among
        // them -- see `apply_xfade_wipe_endpoints_should_keep_ffmpegs_edge_column`.
        let (a, b) = (frame(RED), frame(BLUE));
        for kind in [
            XfadeTransition::SlideLeft,
            XfadeTransition::SlideRight,
            XfadeTransition::SlideUp,
            XfadeTransition::SlideDown,
            XfadeTransition::Dissolve,
        ] {
            let mut dst = Vec::new();
            apply_xfade(kind, &a, &b, 0.0, (4, 1), None, &mut dst);
            assert_eq!(dst, a, "{kind:?} at progress 0 = all A");
            apply_xfade(kind, &a, &b, 1.0, (4, 1), None, &mut dst);
            assert_eq!(dst, b, "{kind:?} at progress 1 = all B");
        }
    }

    #[test]
    fn apply_xfade_wipe_endpoints_should_keep_ffmpegs_edge_column() {
        // `FFmpeg`'s integer edge with a strict comparison never quite empties: at
        // progress 0 `WipeRight` already shows column 0 of B (`x <= 0`), and at progress
        // 1 `WipeLeft` still holds column 0 of A (`x > 0`). Reproducing that is the point
        // -- the export lands on FFmpeg's pixels, not on a tidier convention. In a real
        // transition neither endpoint is rendered anyway: the window runs
        // `0 .. (n-1)/n`.
        let (a, b) = (frame(RED), frame(BLUE));
        let mut dst = Vec::new();

        apply_xfade(
            XfadeTransition::WipeRight,
            &a,
            &b,
            0.0,
            (4, 1),
            None,
            &mut dst,
        );
        assert_eq!(&dst[0..4], &BLUE, "WipeRight at 0 keeps column 0 on B");
        assert_eq!(&dst[4..8], &RED, "the rest is still A");

        apply_xfade(
            XfadeTransition::WipeLeft,
            &a,
            &b,
            1.0,
            (4, 1),
            None,
            &mut dst,
        );
        assert_eq!(&dst[0..4], &RED, "WipeLeft at 1 keeps column 0 on A");
        assert_eq!(&dst[4..8], &BLUE, "the rest has flipped to B");
    }

    // A `w`x`h` packed-RGBA frame where pixel (x, y) is a distinct colour, so a
    // test can tell which source and which coordinate a destination pixel came
    // from. Encodes `x` in R and `y` in G.
    fn tagged(w: u32, h: u32, base_b: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&[x as u8, y as u8, base_b, 255]);
            }
        }
        v
    }

    #[test]
    fn apply_xfade_wipedown_half_should_fill_b_from_the_top_rows() {
        // 2x4 frame (h=4): a vertical wipe must split by *row*, not column.
        let (a, b) = (tagged(2, 4, 0), tagged(2, 4, 99));
        let mut dst = Vec::new();
        apply_xfade(
            XfadeTransition::WipeDown,
            &a,
            &b,
            0.5,
            (2, 4),
            None,
            &mut dst,
        );
        // `FFmpeg` takes clip B where `y <= z` with the integer `z = height * progress`,
        // so `z = 2` puts rows 0..=2 on B and leaves only row 3 on A -- the same
        // inclusive edge as `WipeRight`, on the other axis (#1732).
        let px = |x: u32, y: u32| {
            let i = ((y * 2 + x) * 4) as usize;
            dst[i + 2] // the base-B channel identifies the source frame
        };
        assert_eq!(px(0, 0), 99, "row 0 = B");
        assert_eq!(px(1, 1), 99, "row 1 = B");
        assert_eq!(px(0, 2), 99, "row 2 = B (y <= z, inclusive)");
        assert_eq!(px(1, 3), 0, "row 3 = A");
    }

    #[test]
    fn apply_xfade_slideleft_mid_should_shift_a_left_and_slide_b_in_from_the_right() {
        // 4x1: SlideLeft translates A left by p*w; B fills from the right edge.
        let (a, b) = (tagged(4, 1, 0), tagged(4, 1, 99));
        let mut dst = Vec::new();
        apply_xfade(
            XfadeTransition::SlideLeft,
            &a,
            &b,
            0.5,
            (4, 1),
            None,
            &mut dst,
        );
        // dx = 2. dst[x] = A[x+2] for x+2 < 4, else B[(x+2) mod 4].
        let src = |x: usize| dst[x * 4 + 2]; // base-B channel: 0 = A, 99 = B
        let col = |x: usize| dst[x * 4]; // R channel = source x-coordinate
        assert_eq!(src(0), 0, "col 0 from A");
        assert_eq!(col(0), 2, "col 0 = A[2] (shifted left by 2)");
        assert_eq!(src(1), 0, "col 1 from A");
        assert_eq!(col(1), 3, "col 1 = A[3]");
        assert_eq!(src(2), 99, "col 2 from B (slid in)");
        assert_eq!(src(3), 99, "col 3 from B (slid in)");
    }

    #[test]
    fn apply_xfade_dissolve_mid_should_mix_a_and_b() {
        // A larger frame so the per-pixel hash yields both A and B at p=0.5.
        let (a, b) = (tagged(16, 16, 0), tagged(16, 16, 99));
        let mut dst = Vec::new();
        apply_xfade(
            XfadeTransition::Dissolve,
            &a,
            &b,
            0.5,
            (16, 16),
            None,
            &mut dst,
        );
        let has_a = dst.chunks_exact(4).any(|p| p[2] == 0);
        let has_b = dst.chunks_exact(4).any(|p| p[2] == 99);
        assert!(has_a && has_b, "mid-progress dissolve mixes both A and B");
    }

    #[test]
    fn apply_xfade_dissolve_should_follow_ffmpegs_own_noise() {
        // Tolerance-free: every pixel must agree with `xfade_frand` exactly, which is
        // what makes the preview show the export's pixel *set* and not merely the same
        // proportion of them. The hash this replaced revealed a different set entirely
        // (mean 54 against a real export).
        let (w, h) = (16u32, 16u32);
        let n = (w * h) as usize;
        let a: Vec<u8> = [0u8, 0, 0, 255].repeat(n);
        let b: Vec<u8> = [255u8, 255, 255, 255].repeat(n);
        let mut dst = Vec::new();
        let p = 0.5;
        apply_xfade(XfadeTransition::Dissolve, &a, &b, p, (w, h), None, &mut dst);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let want = if xfade_frand(x, y) < p { 255 } else { 0 };
                assert_eq!(dst[i], want, "pixel ({x}, {y}) must follow xfade_frand");
            }
        }
    }

    #[test]
    fn apply_xfade_dissolve_with_a_cached_field_should_match_the_uncached_path() {
        // What discharges "the rendered pixels are unchanged" (#1736). The parity suites
        // keep calling this with `None`, so they check the *uncached* selection against
        // FFmpeg; this checks the cached one against that, which makes the pair a chain
        // rather than a circle.
        //
        // Non-square on purpose: a transposed lookup reads the wrong pixel at every
        // coordinate but keeps the right length, and `w == h` would hide it.
        const W: u32 = 7;
        const H: u32 = 5;
        let a = tagged(W, H, 0);
        let b = tagged(W, H, 128);
        let field = xfade_frand_field(W, H);

        for p in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let (mut cached, mut uncached) = (Vec::new(), Vec::new());
            apply_xfade(
                XfadeTransition::Dissolve,
                &a,
                &b,
                p,
                (W, H),
                None,
                &mut uncached,
            );
            apply_xfade(
                XfadeTransition::Dissolve,
                &a,
                &b,
                p,
                (W, H),
                Some(&field),
                &mut cached,
            );
            assert_eq!(
                cached, uncached,
                "the cached field must select byte-identically at progress {p}"
            );
        }
    }

    #[test]
    fn apply_xfade_dissolve_should_ignore_a_field_of_the_wrong_size() {
        // A field left over from another frame size must not be indexed. The length is
        // the whole of what ties a cached field to a frame (RK-025), so a mismatch falls
        // back to computing rather than reading a neighbouring pixel or panicking.
        const W: u32 = 7;
        const H: u32 = 5;
        let a = tagged(W, H, 0);
        let b = tagged(W, H, 128);
        let stale = xfade_frand_field(W + 1, H + 1);

        let (mut got, mut want) = (Vec::new(), Vec::new());
        apply_xfade(
            XfadeTransition::Dissolve,
            &a,
            &b,
            0.5,
            (W, H),
            None,
            &mut want,
        );
        apply_xfade(
            XfadeTransition::Dissolve,
            &a,
            &b,
            0.5,
            (W, H),
            Some(&stale),
            &mut got,
        );
        assert_eq!(
            got, want,
            "a field sized for another frame must be refused, not indexed"
        );
    }

    #[test]
    fn apply_xfade_dip_should_hold_the_colour_through_the_middle() {
        // The shape that distinguishes `FFmpeg`'s phased dip from a linear one: the
        // solid colour is reached early and held, rather than touched only at the
        // midpoint. Sampling across progress, the darkest frame must land in the first
        // phase -- a linear dip bottoms out at 0.5.
        let a = frame(RED);
        let b = frame(BLUE);
        let mut dst = Vec::new();
        let mut darkest = (u8::MAX, 0u32);
        for i in 1..=9u32 {
            let p = i as f32 / 10.0;
            apply_xfade(
                XfadeTransition::FadeBlack,
                &a,
                &b,
                p,
                (4, 1),
                None,
                &mut dst,
            );
            let luma = dst[0].max(dst[1]).max(dst[2]);
            if luma < darkest.0 {
                darkest = (luma, i);
            }
        }
        assert!(
            darkest.1 <= 3,
            "the dip must bottom out in its first phase, got progress 0.{}",
            darkest.1
        );
    }

    #[test]
    fn apply_xfade_deferred_kind_should_fall_back_to_fade() {
        let (a, b) = (frame(RED), frame(BLUE));
        let (mut x, mut y) = (Vec::new(), Vec::new());
        apply_xfade(XfadeTransition::Pixelize, &a, &b, 0.3, (4, 1), None, &mut x);
        blend_rgba(&a, &b, 0.3, &mut y);
        assert_eq!(x, y, "deferred kinds render as the linear fade");
    }

    #[test]
    fn apply_xfade_mismatched_buffers_should_fall_back_to_linear() {
        let a = frame(RED);
        let b = vec![9u8, 9];
        let mut dst = Vec::new();
        apply_xfade(
            XfadeTransition::WipeRight,
            &a,
            &b,
            0.5,
            (4, 1),
            None,
            &mut dst,
        );
        assert_eq!(dst, a, "mismatched → linear fallback copies A");
    }
}
