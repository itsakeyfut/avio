//! Unsafe-free helpers for the timeline presentation loop.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]
// `a`/`b` (frames), `w`/`h` (dims), `x`/`y` (coords) are the natural names here.
#![allow(clippy::many_single_char_names)]

use ff_filter::XfadeTransition;

/// Blends the outgoing frame `a` and incoming frame `b` (packed RGBA, `w*h*4`) at
/// transition progress `alpha` (`0` = all A, `1` = all B) using the `xfade` `kind`.
///
/// `wipe*`, `slide*`, `dissolve` and the `fadeblack` / `fadewhite` dips are rendered
/// host-side here. `fade` and the geometric / mosaic kinds (`fadegrays`, `circleopen`,
/// `circleclose`, `pixelize`) fall through to the linear cross-blend — exact fidelity
/// for those is deferred to the GPU compositing work (#1365). Falls back to the linear
/// blend when the buffers are mismatched or their length is not `w*h*4`.
///
/// Public because it is the reference the GPU transition nodes are compared against:
/// `avio`'s transition parity tests run each `ff_render` node beside this function.
pub fn apply_xfade(
    kind: XfadeTransition,
    a: &[u8],
    b: &[u8],
    alpha: f32,
    w: u32,
    h: u32,
    dst: &mut Vec<u8>,
) {
    let expected = (w as usize) * (h as usize) * 4;
    if a.len() != b.len() || a.len() != expected {
        blend_rgba(a, b, alpha, dst);
        return;
    }
    let p = alpha.clamp(0.0, 1.0);
    let (wf, hf) = (w as f32, h as f32);
    match kind {
        XfadeTransition::WipeRight => wipe(a, b, w, h, dst, |x, _| (x as f32) < p * wf),
        XfadeTransition::WipeLeft => wipe(a, b, w, h, dst, |x, _| (x as f32) >= (1.0 - p) * wf),
        XfadeTransition::WipeDown => wipe(a, b, w, h, dst, |_, y| (y as f32) < p * hf),
        XfadeTransition::WipeUp => wipe(a, b, w, h, dst, |_, y| (y as f32) >= (1.0 - p) * hf),
        XfadeTransition::SlideLeft => slide(a, b, w, h, dst, (p * wf) as i64, 0),
        XfadeTransition::SlideRight => slide(a, b, w, h, dst, -((p * wf) as i64), 0),
        XfadeTransition::SlideUp => slide(a, b, w, h, dst, 0, (p * hf) as i64),
        XfadeTransition::SlideDown => slide(a, b, w, h, dst, 0, -((p * hf) as i64)),
        XfadeTransition::Dissolve => dissolve(a, b, w, h, dst, p),
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

/// Dissolve: reveal `b` per-pixel once progress passes a deterministic per-pixel
/// threshold (a plausible noise dissolve, not `FFmpeg`'s exact PRNG).
fn dissolve(a: &[u8], b: &[u8], w: u32, h: u32, dst: &mut Vec<u8>, p: f32) {
    dst.resize(a.len(), 0);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let src = if p > hash01(x, y) { b } else { a };
            dst[i..i + 4].copy_from_slice(&src[i..i + 4]);
        }
    }
}

/// Two-phase dip: clip A fades to `color`, then `color` fades to clip B. `p = 0.5` is
/// the fully solid frame. Alpha rides along so a dip over transparency stays sane.
fn dip(a: &[u8], b: &[u8], color: [u8; 3], dst: &mut Vec<u8>, p: f32) {
    dst.resize(a.len(), 0);
    let solid = [color[0], color[1], color[2], 255u8];
    // First half blends A -> colour, second half colour -> B; both legs run at twice
    // the outer rate so the dip is complete exactly at the midpoint.
    let second_half = p >= 0.5;
    let (clip, t) = if second_half {
        (b, (p - 0.5) * 2.0)
    } else {
        (a, p * 2.0)
    };
    for (i, out) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let j = i * 4;
        for c in 0..4 {
            // Phase 1 runs clip -> colour, phase 2 colour -> clip; the direction is
            // fixed for the whole frame, so only the endpoints swap here.
            let (from, to) = if second_half {
                (f32::from(solid[c]), f32::from(clip[j + c]))
            } else {
                (f32::from(clip[j + c]), f32::from(solid[c]))
            };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                out[c] = (from + (to - from) * t + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// A cheap deterministic per-pixel hash in `[0.0, 1.0)`.
///
/// Mirrored bit-for-bit by `ff_render`'s `DissolveTransitionNode` and its
/// `dissolve.wgsl`, so the CPU and GPU dissolves reveal the same pixels rather than
/// merely the same *proportion* of them. Changing it here changes only this copy;
/// `avio`'s transition parity suite is what catches the drift, and that needs a GPU
/// adapter to run.
fn hash01(x: u32, y: u32) -> f32 {
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    // Top 24 bits over 2^24: max = (2^24 - 1) / 2^24 = 1 - 2^-24, exactly
    // representable in f32 and strictly < 1.0, so `dissolve`'s `p > hash01`
    // reveals every pixel at p = 1.0 (the all-B boundary holds exactly).
    (h >> 8) as f32 / (1u32 << 24) as f32
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
        apply_xfade(XfadeTransition::Fade, &a, &b, 0.5, 4, 1, &mut x);
        blend_rgba(&a, &b, 0.5, &mut y);
        assert_eq!(x, y, "Fade == linear blend");
    }

    #[test]
    fn apply_xfade_wiperight_half_should_fill_b_from_the_left() {
        let (a, b) = (frame(RED), frame(BLUE));
        let mut dst = Vec::new();
        apply_xfade(XfadeTransition::WipeRight, &a, &b, 0.5, 4, 1, &mut dst);
        // edge = 2.0 → cols 0,1 = B (blue), cols 2,3 = A (red).
        assert_eq!(&dst[0..4], &BLUE);
        assert_eq!(&dst[4..8], &BLUE);
        assert_eq!(&dst[8..12], &RED);
        assert_eq!(&dst[12..16], &RED);
    }

    #[test]
    fn apply_xfade_wipeleft_half_should_fill_b_from_the_right() {
        let (a, b) = (frame(RED), frame(BLUE));
        let mut dst = Vec::new();
        apply_xfade(XfadeTransition::WipeLeft, &a, &b, 0.5, 4, 1, &mut dst);
        // edge = 2.0 → cols 0,1 = A, cols 2,3 = B.
        assert_eq!(&dst[0..4], &RED);
        assert_eq!(&dst[8..12], &BLUE);
    }

    #[test]
    fn apply_xfade_boundaries_should_be_all_a_or_all_b() {
        let (a, b) = (frame(RED), frame(BLUE));
        for kind in [
            XfadeTransition::WipeRight,
            XfadeTransition::WipeLeft,
            XfadeTransition::WipeUp,
            XfadeTransition::WipeDown,
            XfadeTransition::SlideLeft,
            XfadeTransition::SlideRight,
            XfadeTransition::SlideUp,
            XfadeTransition::SlideDown,
            XfadeTransition::Dissolve,
        ] {
            let mut dst = Vec::new();
            apply_xfade(kind, &a, &b, 0.0, 4, 1, &mut dst);
            assert_eq!(dst, a, "{kind:?} at progress 0 = all A");
            apply_xfade(kind, &a, &b, 1.0, 4, 1, &mut dst);
            assert_eq!(dst, b, "{kind:?} at progress 1 = all B");
        }
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
        apply_xfade(XfadeTransition::WipeDown, &a, &b, 0.5, 2, 4, &mut dst);
        // edge = 2.0 → rows 0,1 = B (base_b 99), rows 2,3 = A (base_b 0).
        let px = |x: u32, y: u32| {
            let i = ((y * 2 + x) * 4) as usize;
            dst[i + 2] // the base-B channel identifies the source frame
        };
        assert_eq!(px(0, 0), 99, "row 0 = B");
        assert_eq!(px(1, 1), 99, "row 1 = B");
        assert_eq!(px(0, 2), 0, "row 2 = A");
        assert_eq!(px(1, 3), 0, "row 3 = A");
    }

    #[test]
    fn apply_xfade_slideleft_mid_should_shift_a_left_and_slide_b_in_from_the_right() {
        // 4x1: SlideLeft translates A left by p*w; B fills from the right edge.
        let (a, b) = (tagged(4, 1, 0), tagged(4, 1, 99));
        let mut dst = Vec::new();
        apply_xfade(XfadeTransition::SlideLeft, &a, &b, 0.5, 4, 1, &mut dst);
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
        apply_xfade(XfadeTransition::Dissolve, &a, &b, 0.5, 16, 16, &mut dst);
        let has_a = dst.chunks_exact(4).any(|p| p[2] == 0);
        let has_b = dst.chunks_exact(4).any(|p| p[2] == 99);
        assert!(has_a && has_b, "mid-progress dissolve mixes both A and B");
    }

    #[test]
    fn apply_xfade_deferred_kind_should_fall_back_to_fade() {
        let (a, b) = (frame(RED), frame(BLUE));
        let (mut x, mut y) = (Vec::new(), Vec::new());
        apply_xfade(XfadeTransition::Pixelize, &a, &b, 0.3, 4, 1, &mut x);
        blend_rgba(&a, &b, 0.3, &mut y);
        assert_eq!(x, y, "deferred kinds render as the linear fade");
    }

    #[test]
    fn apply_xfade_mismatched_buffers_should_fall_back_to_linear() {
        let a = frame(RED);
        let b = vec![9u8, 9];
        let mut dst = Vec::new();
        apply_xfade(XfadeTransition::WipeRight, &a, &b, 0.5, 4, 1, &mut dst);
        assert_eq!(dst, a, "mismatched → linear fallback copies A");
    }
}
