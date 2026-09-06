//! GPU/CPU parity and fallback tests for the compositing bridge (Br5, #1628).
//!
//! Parity: for the supported v1 set (identity transform, canvas-aspect frame), the
//! GPU compositor ([`GpuCompositor`]) and the CPU preview compositor
//! (`ff_filter::RealtimeComposer`) must agree within tolerance. Both consume the same
//! `RealtimeLayer`, so they are compared directly in rgba, with no encode/decode
//! noise. Since the GPU *export* drain composites through the same [`GpuCompositor`],
//! this covers the export compositing math too; the end-to-end export-vs-force-CPU
//! smoke lives in `gpu_export_tests.rs`.
//!
//! Fallback: the GPU compositor returns `None` (never panics) for every unsupported
//! input, and the preview runner keeps advancing (never hangs) when it falls back.
//!
//! Double-gated (RK-002 / RK-020): the GPU leg needs an adapter and the CPU leg needs
//! an `FFmpeg` built with filters (`RealtimeComposer` is libavfilter-based). Each leg
//! skips gracefully when unavailable, so the suite is green on headless / minimal CI
//! and the real parity runs on a full dev build (and macOS CI). Tolerances are
//! calibrated on the dev machine (see the constants); exact pixel equality across GPU
//! drivers is explicitly not asserted (docs/rules/test.md).

#![cfg(all(feature = "gpu", feature = "preview"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use avio::{
    AnimatedValue, AnimationTrack, BlendMode, Clip, Color, CompositeOp, Easing, FilterStep,
    GpuCompositor, GpuTransition, Keyframe, PixelFormat, PlayerHandle, RealtimeLayer,
    ScaleAlgorithm, Timeline, TimelinePlayer, VideoFrame, map_transition,
};
use ff_filter::RealtimeComposer;
use ff_filter::XfadeTransition;
use ff_preview::FrameSink;
use ff_render::{
    DipToColorNode, DissolveTransitionNode, FadeTransitionNode, RenderContext, RenderGraph,
    RenderNode, WipeTransitionNode,
};

// Per-node tolerance table (#1663). Every mapped `GpuEffect` node has a probe-gated
// GPU/CPU parity test with a calibrated tolerance; `avio::gpu`'s `parity_coverage`
// meta-test enforces at compile time that every variant appears here. Columns:
// GpuEffect node | parity test | CPU reference | tolerance const | measured (this build).
//
//   ColorGrade  | color_grade_gpu_should_match_cpu_within_tolerance          | eq          | TOL_COLOR_GRADE_MEAN  | mean ~6.6
//   ColorGrade  | color_grade_temperature_tint_gpu_should_match_cpu_reference| eq + Rust   | TOL_GRADE_TEMPTINT_MEAN| mean ~0.4
//   Scale       | scale_gpu_should_match_cpu_within_tolerance                | scale       | TOL_SCALE_MEAN        | mean ~0.14
//   Blur        | blur_gpu_should_match_cpu_within_tolerance                 | gblur       | TOL_BLUR_MEAN         | mean ~9.0
//   Sharpen     | sharpen_gpu_should_match_cpu_within_tolerance              | unsharp     | TOL_SHARPEN_MEAN      | mean ~0.7
//   Vignette    | vignette_gpu_should_match_cpu_within_tolerance             | vignette    | TOL_VIGNETTE_MEAN     | calibrated
//   FilmGrain   | film_grain_gpu_should_match_cpu_grain_strength            | noise (std) | TOL_FILMGRAIN_STD     | std ~10.9/11.1
//   Glow        | glow_gpu_should_match_cpu_within_tolerance                 | gblur+blend | TOL_GLOW_MEAN         | calibrated
//   ColorWheels | color_wheels_gpu_should_match_cpu_within_tolerance         | colorbalance| TOL_COLOR_WHEELS_MEAN | calibrated
//   Curves      | curves_gpu_should_match_cpu_within_tolerance               | curves      | TOL_CURVES_MEAN       | calibrated
//   Hsl         | hsl_gpu_should_match_cpu_within_tolerance                  | hue         | TOL_HSL_MEAN          | calibrated
//   Lut         | lut_gpu_should_match_cpu_within_tolerance                  | lut3d       | TOL_LUT_MEAN          | calibrated
//   ChromaKey   | chroma_key_gpu_should_match_cpu_within_tolerance           | colorkey(2L)| TOL_CHROMAKEY_MEAN    | calibrated
//   LumaMask    | luma_mask_gpu_should_match_cpu_within_tolerance (+ _invert) | geq (2L)    | TOL_LUMAMASK_MEAN     | mean ~0.2
//   ShapeMask   | shape_mask_gpu_should_match_cpu_within_tolerance (+ _invert)| geq (2L)    | TOL_SHAPEMASK_MEAN    | mean 0.0
//   MotionBlur  | motion_blur_gpu_should_match_cpu_within_tolerance          | tblend      | TOL_MOTIONBLUR_MEAN   | mean 0.0
// (Passthrough — no effect — is covered by passthrough_gpu_should_match_cpu_within_tolerance / TOL_PASSTHROUGH_MEAN.)
//
// Tolerances (mean absolute per-channel RGB difference, 0..255). Calibrated on the
// dev machine; alpha is excluded because the two legs mean different things by it,
// not because the GPU discards it. The CPU leg composites onto an opaque `color`
// canvas and ends in `format=rgba`, so its alpha is 255 everywhere; the GPU's is
// src-over coverage since #1750, pinned by the ff-render compositor tests.
// Measured on this build: passthrough
// is pixel-exact (mean 0.0), colour grade is mean 6.6 (max 33). The thresholds keep a
// margin for GPU-driver rounding while staying tight enough to fail on a real
// divergence (a stretch/letterbox/axis-swap of the gradient is tens of levels).
const TOL_PASSTHROUGH_MEAN: f64 = 2.0; // identity passthrough: pixel-exact here
const TOL_COLOR_GRADE_MEAN: f64 = 20.0; // GPU ColorGradeNode vs FFmpeg `eq`: ~6.6 here
// Extended grade (temperature/tint): the GPU node applies the ±0.1 offset that FFmpeg `eq`
// cannot express, so the CPU reference is the neutral-eq output plus that offset applied in
// Rust (mirroring the node). With brightness/contrast/saturation neutral the node reduces to
// just the offset, so the only divergence is quantisation/driver rounding: ~0.4 here.
const TOL_GRADE_TEMPTINT_MEAN: f64 = 8.0;
const TOL_BLUR_MEAN: f64 = 20.0; // GPU GaussianBlurNode vs FFmpeg `gblur`: ~9.0 here (different kernels)
// GPU ScaleNode (bilinear) vs FFmpeg `scale` (bilinear): different resampler implementations,
// so a loose calibrated tolerance on a downscaled gradient (~0.14 here).
const TOL_SCALE_MEAN: f64 = 12.0;
// GPU SharpenNode (RGB, fixed sigma) vs FFmpeg `unsharp` (luma-only, 5x5): ~0.7 here
// (effect vs input ~7.9). Grey-calibrated: the test image is achromatic (R=G=B), where
// an all-RGB node and a luma-only filter coincide; colored edges would diverge more.
const TOL_SHARPEN_MEAN: f64 = 5.0;
// GPU VignetteNode (smoothstep radius/feather) vs FFmpeg `vignette` (cos^4 angle):
// different darkening profiles, so a loose calibrated tolerance (~21 here; effect vs
// input ~56, so a skipped vignette diverges past this). Tested on a colored gradient
// (RK-022): vignette scales every channel equally, so it is color-consistent.
const TOL_VIGNETTE_MEAN: f64 = 45.0;
// Film grain: GPU (Wang hash) and CPU (`noise` RNG) produce different patterns, so
// parity compares grain *strength* (std of output - input), not pixels. Calibrated
// (NODE_GRAIN_SCALE) so the two grain std-devs match: ~10.9 vs ~11.1 here. The margin
// covers per-run RNG variance across GPU drivers.
const TOL_FILMGRAIN_STD: f64 = 6.0;
// Glow: GPU GlowNode (extract -> blur -> add) vs the CPU compound `split`/`curves`/
// `gblur`/`blend` chain. Different bloom implementations, so a loose calibrated
// tolerance: ~4.4 here (effect vs input ~7.4). Tested on a coloured highlight
// (RK-022): glow spreads the highlight colour, so the two paths stay colour-comparable.
const TOL_GLOW_MEAN: f64 = 15.0;
// ColorWheels: GPU ColorWheelsNode (luma-region-weighted lift/gamma/gain) vs the CPU
// `curves` chain (per-channel 3-point curves). Different lift/gamma/gain models, so a
// loose calibrated tolerance: ~8.8 here (effect vs input ~7.7). Tested on a colour
// gradient (RK-022): the corrector shifts each channel, so it is colour-relevant.
const TOL_COLOR_WHEELS_MEAN: f64 = 20.0;
// Curves: GPU CurvesNode (Steffen monotone-cubic LUT) vs the CPU `curves` filter
// (its own spline). Both interpolate the same control points, so they agree closely:
// ~1.4 here (effect vs input ~23). Tested on a colour gradient (RK-022): the per-channel
// curves shift colour.
const TOL_CURVES_MEAN: f64 = 8.0;
// HSL: GPU HslNode (true HSL space) vs the CPU `hue` filter (YUV chroma rotation
// for hue/saturation, a luma add for brightness). The colour models differ but
// still agree closely for a modest adjustment: ~6.7 here (effect vs input ~21).
// Tested on a colour gradient (RK-022) where the hue/saturation shift moves every
// channel.
const TOL_HSL_MEAN: f64 = 20.0;
// LUT: GPU LutNode vs the CPU `lut3d` filter, both trilinear over the same .cube
// grid, so they agree almost exactly: ~0.27 here (effect vs input ~10, max 1). The
// RK-005-verified axis order matches FFmpeg's, so a transposition would push the
// mean to tens. Tested on a colour gradient (RK-022) with a per-channel-shifting LUT.
const TOL_LUT_MEAN: f64 = 4.0;
// ChromaKey: GPU ChromaKeyNode (RGB chroma distance) vs the CPU `chromakey` filter
// (YUV chroma distance). ChromaKey rewrites *alpha*, but the GPU compositor's blend
// shader outputs the canvas alpha, not the composited overlay alpha (blend.wgsl), so a
// composited-alpha comparison is not possible and a composited-RGB comparison of the
// keyed layer alone is vacuous (keying leaves RGB untouched). So the keyed layer is
// composited over an *opaque* background: the compositor's `mix(base, overlay,
// overlay.a)` turns the keyed alpha into an RGB difference (background shows through the
// keyed region), making an RGB parity non-vacuous. The two distance metrics differ, so
// a loose calibrated tolerance; the flat interiors agree and only the region edge
// differs between the metrics.
const TOL_CHROMAKEY_MEAN: f64 = 25.0;
// LumaMask: GPU LumaMaskNode (alpha *= own BT.709 luma) vs the CPU `geq` self-luma
// mask. Both use the same BT.709 coefficients on the same gamma-encoded RGB, so they
// agree almost exactly (tighter than ChromaKey). As with ChromaKey, the compositor
// discards composited alpha, so the keyed layer is composited over an opaque
// background to turn the luma-modulated alpha into a non-vacuous RGB difference
// (dark pixels reveal the background). Loose enough to cover the invert path's mask
// quantisation (the grey mask rounds 255-luma to a u8).
const TOL_LUMAMASK_MEAN: f64 = 6.0;
// ShapeMask: GPU ShapeMaskNode (rectangle alpha mask) vs the CPU `geq` RectMask. The
// mask is position-based (hard edges, colour-independent) and both paths use the same
// inclusive rectangle bounds, so they agree almost exactly. As with the other masks
// the compositor discards composited alpha, so the masked foreground is composited
// over an opaque background; a centre vertical band pins the rectangle's x-bounds
// (RK-015), not merely that a mask was applied.
const TOL_SHAPEMASK_MEAN: f64 = 4.0;
// MotionBlur: GPU MotionBlurNode (IIR: mix(current, accumulated prev)) vs the CPU
// `tblend` filter (FIR-2: a weighted blend of the two most recent frames). Different
// temporal models, but over two frames at shutter 180 / sub_frames 8 both reduce to
// `0.5*prev + 0.5*current`, so they align at that calibration point (the CPU tblend
// ignores sub_frames): measured mean 0.0 here (bit-exact). Temporal, so the parity drives
// two frames; a single frame is unblended on both paths and would be vacuous (RK-015).
// The margin covers GPU-driver rounding on other machines.
const TOL_MOTIONBLUR_MEAN: f64 = 15.0;
// Letterbox (#1661): the GPU fits the layer via `LayerTransform` (bilinear sampling in
// `transform.wgsl`) where the CPU uses `scale=…:force_original_aspect_ratio=decrease` +
// `pad` (sws). The fitted size mirrors FFmpeg's rounding, so the geometry is identical and
// only the resampling differs. The 16:9-into-square fixture fits 1:1 (no resampling at
// all): measured mean 0.0 here, bit-exact. The margin covers GPU-driver rounding elsewhere.
const TOL_LETTERBOX_MEAN: f64 = 12.0;
// The same parity where the fit *does* rescale (3:2 upscaled into a 100x66 band), so the
// GPU's bilinear sampler meets sws's bicubic: measured mean 0.14 (max 1) here. A band
// misplaced by one row would land far outside this, so the fitted-size rounding is what
// this number actually certifies.
const TOL_LETTERBOX_RESCALE_MEAN: f64 = 15.0;

/// A deterministic non-uniform pattern: R ramps in x, G in y, B in x+y. A stretch,
/// letterbox, axis-swap, or channel bug shows up here where a flat fill would not
/// (RK-015).
fn gradient_rgba(w: u32, h: u32) -> Vec<u8> {
    let (wu, hu) = (w as usize, h as usize);
    let mut v = Vec::with_capacity(wu * hu * 4);
    for y in 0..hu {
        for x in 0..wu {
            let r = (x * 255 / (wu - 1).max(1)) as u8;
            let g = (y * 255 / (hu - 1).max(1)) as u8;
            let b = ((x + y) * 255 / (wu + hu - 2).max(1)) as u8;
            v.extend_from_slice(&[r, g, b, 255]);
        }
    }
    v
}

/// A high-frequency `block`-sized checkerboard (black/white). A blur visibly smooths
/// it, so a parity test on it is non-vacuous (RK-015): a GPU that failed to blur would
/// keep the sharp squares and diverge far from the CPU-blurred reference.
fn checkerboard_rgba(w: u32, h: u32, block: u32) -> Vec<u8> {
    let (wu, hu) = (w as usize, h as usize);
    let mut v = Vec::with_capacity(wu * hu * 4);
    for y in 0..h {
        for x in 0..w {
            let on = ((x / block) + (y / block)) % 2 == 0;
            let c = if on { 255u8 } else { 0u8 };
            v.extend_from_slice(&[c, c, c, 255]);
        }
    }
    v
}

/// Two flat halves: the left is pure key green (0x00FF00), the right a non-key red.
/// ChromaKey must drive the left half transparent (revealing the background it is
/// composited over) and leave the right opaque, so a parity over this fixture is
/// non-vacuous (RK-015): a GPU that failed to key would keep green in the left half and
/// diverge from the CPU reference.
fn key_split_rgba(w: u32, h: u32) -> Vec<u8> {
    let (wu, hu) = (w as usize, h as usize);
    let mut v = Vec::with_capacity(wu * hu * 4);
    for _y in 0..hu {
        for x in 0..wu {
            if x < wu / 2 {
                v.extend_from_slice(&[0, 255, 0, 255]); // key: pure green
            } else {
                v.extend_from_slice(&[220, 40, 40, 255]); // non-key: red
            }
        }
    }
    v
}

/// Three vertical bands driving the luma mask non-vacuously (RK-015 / RK-022):
/// white (luma 1.0 -> opaque), pure red (BT.709 luma ~0.21 -> a *partial* alpha), and
/// black (luma 0.0 -> transparent). The red band is the important one: its mid alpha
/// exercises the continuous blend (a threshold key would clear it entirely), and its
/// value depends on the luma *coefficients* (BT.709 red 0.2126 vs BT.601 0.299 differ
/// by ~20 levels), so GPU and CPU using different coefficients would diverge here.
fn luma_bands_rgba(w: u32, h: u32) -> Vec<u8> {
    let (wu, hu) = (w as usize, h as usize);
    let third = wu / 3;
    let mut v = Vec::with_capacity(wu * hu * 4);
    for _y in 0..hu {
        for x in 0..wu {
            let px = if x < third {
                [255, 255, 255, 255] // white: luma 1 -> opaque
            } else if x < 2 * third {
                [255, 0, 0, 255] // pure red: mid luma -> partial alpha
            } else {
                [0, 0, 0, 255] // black: luma 0 -> transparent
            };
            v.extend_from_slice(&px);
        }
    }
    v
}

/// Mean RGB over the vertical band `[x0, x1)` (all rows). The three-band luma fixture
/// is checked band-by-band so the partial-alpha middle band can be asserted directly.
fn band_mean_rgb(buf: &[u8], w: u32, x0: u32, x1: u32) -> [f64; 3] {
    let wu = w as usize;
    let (x0, x1) = (x0 as usize, x1 as usize);
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for (i, px) in buf.chunks_exact(4).enumerate() {
        let x = i % wu;
        if x >= x0 && x < x1 {
            for c in 0..3 {
                sum[c] += u64::from(px[c]);
            }
            n += 1;
        }
    }
    let d = n.max(1) as f64;
    [sum[0] as f64 / d, sum[1] as f64 / d, sum[2] as f64 / d]
}

/// Mean RGB over the rectangular region `[x0, x1) x [y0, y1)`. The shape-mask fixture
/// uses a centre box, so both its x- and y-bounds must be checked (a band would leave
/// one axis unexercised — a transposition / y-offset bug would slip through).
fn box_mean_rgb(buf: &[u8], w: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> [f64; 3] {
    let wu = w as usize;
    let (x0, y0, x1, y1) = (x0 as usize, y0 as usize, x1 as usize, y1 as usize);
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for (i, px) in buf.chunks_exact(4).enumerate() {
        let (x, y) = (i % wu, i / wu);
        if x >= x0 && x < x1 && y >= y0 && y < y1 {
            for c in 0..3 {
                sum[c] += u64::from(px[c]);
            }
            n += 1;
        }
    }
    let d = n.max(1) as f64;
    [sum[0] as f64 / d, sum[1] as f64 / d, sum[2] as f64 / d]
}

/// A flat, fully opaque `[r, g, b]` fill: used as an opaque background layer so a keyed
/// foreground shows the background through its transparent regions.
fn solid_rgba(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        v.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    v
}

/// Standard deviation of the signed per-channel difference `out - input` over the
/// RGB channels. For an added-noise effect this is the grain magnitude; the film
/// grain parity compares this (not pixels, since the GPU and CPU use different RNGs
/// so their grain patterns never match).
fn std_delta_rgb(out: &[u8], input: &[u8]) -> f64 {
    let mut diffs = Vec::with_capacity(out.len() / 4 * 3);
    for (po, pi) in out.chunks_exact(4).zip(input.chunks_exact(4)) {
        for c in 0..3 {
            diffs.push(f64::from(i16::from(po[c]) - i16::from(pi[c])));
        }
    }
    let n = diffs.len() as f64;
    if n == 0.0 {
        return 0.0;
    }
    let mean = diffs.iter().sum::<f64>() / n;
    let var = diffs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n;
    var.sqrt()
}

/// Mean absolute per-channel difference over the RGB channels only (skips alpha).
fn mean_abs_diff_rgb(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "buffers must be the same length");
    let mut sum = 0u64;
    let mut n = 0u64;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for c in 0..3 {
            sum += u64::from(pa[c].abs_diff(pb[c]));
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { sum as f64 / n as f64 }
}

/// Mean RGB over the pixels in one half (`key_half`: x < w/2 = the keyed green half,
/// else the non-key red half). The chroma-key non-vacuity guard uses this to prove the
/// background shows through the keyed half (RGB near the background) but not the other.
fn region_mean_rgb(buf: &[u8], w: u32, key_half: bool) -> [f64; 3] {
    let wu = w as usize;
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for (i, px) in buf.chunks_exact(4).enumerate() {
        let x = i % wu;
        if (x < wu / 2) == key_half {
            for c in 0..3 {
                sum[c] += u64::from(px[c]);
            }
            n += 1;
        }
    }
    let d = n.max(1) as f64;
    [sum[0] as f64 / d, sum[1] as f64 / d, sum[2] as f64 / d]
}

fn max_abs_diff_rgb(a: &[u8], b: &[u8]) -> u8 {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .flat_map(|(pa, pb)| (0..3).map(move |c| pa[c].abs_diff(pb[c])))
        .max()
        .unwrap_or(0)
}

/// A canvas-sized, identity, single base layer carrying `effects` (the v1 supported
/// shape the GPU path renders without falling back).
fn base_layer(w: u32, h: u32, effects: Vec<FilterStep>) -> RealtimeLayer {
    RealtimeLayer {
        width: w,
        height: h,
        pixel_format: PixelFormat::Rgba,
        effects,
        opacity: AnimatedValue::Static(1.0),
        x: AnimatedValue::Static(0.0),
        y: AnimatedValue::Static(0.0),
        scale_x: AnimatedValue::Static(1.0),
        scale_y: AnimatedValue::Static(1.0),
        rotation: AnimatedValue::Static(0.0),
        blend_mode: BlendMode::Normal,
        composite_op: CompositeOp::Over,
    }
}

/// Composites one layer + frame on the CPU (`RealtimeComposer`) to rgba, or `None`
/// when `FFmpeg` filters are unavailable (skip).
fn cpu_composite(layer: &RealtimeLayer, frame: &VideoFrame, canvas: (u32, u32)) -> Option<Vec<u8>> {
    let mut composer =
        RealtimeComposer::with_canvas(std::slice::from_ref(layer), Some(canvas)).ok()?;
    composer.push_layer(0, frame).ok()?;
    composer.pull().ok()??.to_rgba()
}

/// Composites two layers (bottom first) + their frames on the CPU (`RealtimeComposer`)
/// to rgba, or `None` when `FFmpeg` filters are unavailable (skip).
fn cpu_composite2(
    layers: &[&RealtimeLayer],
    frames: &[&VideoFrame],
    canvas: (u32, u32),
) -> Option<Vec<u8>> {
    let owned: Vec<RealtimeLayer> = layers.iter().map(|l| (*l).clone()).collect();
    let mut composer = RealtimeComposer::with_canvas(&owned, Some(canvas)).ok()?;
    for (i, frame) in frames.iter().enumerate() {
        composer.push_layer(i, frame).ok()?;
    }
    composer.pull().ok()??.to_rgba()
}

/// Composites one layer + frame on the GPU (`GpuCompositor`) to rgba, or `None` when
/// no adapter is present or the layer is unsupported (fallback).
fn gpu_composite(
    gpu: &mut GpuCompositor,
    layer: &RealtimeLayer,
    frame: &VideoFrame,
    canvas: (u32, u32),
) -> Option<Vec<u8>> {
    let (rgba, _w, _h) = gpu.composite(&[(layer, frame)], canvas, Duration::ZERO)?;
    Some(rgba)
}

/// Composites two layers (bottom first) + their frames on the GPU, or `None` on
/// fallback / no adapter.
fn gpu_composite2(
    gpu: &mut GpuCompositor,
    layers: &[&RealtimeLayer],
    frames: &[&VideoFrame],
    canvas: (u32, u32),
) -> Option<Vec<u8>> {
    let pairs: Vec<(&RealtimeLayer, &VideoFrame)> =
        layers.iter().copied().zip(frames.iter().copied()).collect();
    let (rgba, _w, _h) = gpu.composite(&pairs, canvas, Duration::ZERO)?;
    Some(rgba)
}

/// Bounding box of the pixels that carry picture, as `(x0, y0, x1, y1)` inclusive.
///
/// The placement tests assert this as well as a mean diff: a mean-only assertion passes
/// for two *equally wrong* renders, and a layer in the wrong place is exactly that
/// failure (RK-015).
fn lit_bbox(buf: &[u8], w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            if buf[i] > 32 || buf[i + 1] > 32 || buf[i + 2] > 32 {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    any.then_some((x0, y0, x1, y1))
}

/// A `w` x `h` frame filled with one grey level; `0` reads as "no picture" to
/// [`lit_bbox`], which is what makes a placement bounding box readable.
fn flat_frame(w: u32, h: u32, v: u8) -> VideoFrame {
    VideoFrame::from_rgba(w, h, solid_rgba(w, h, [v; 3])).unwrap()
}

#[test]
fn overlay_placement_gpu_should_match_cpu_within_tolerance() {
    // #1633: a positioned, scaled overlay used to force the whole frame to the CPU. The
    // CPU places it by stretching to the base size, scaling by `base * scale`, and
    // overlaying its top-left at (x, y) -- measured as (10, 4)..(41, 35) inclusive for
    // this fixture, which is what both routes must now produce.
    let canvas = (64, 64);
    let base = flat_frame(64, 64, 0);
    let over = flat_frame(64, 64, 255);
    let bottom = base_layer(64, 64, vec![]);
    let mut top = base_layer(64, 64, vec![]);
    top.x = AnimatedValue::Static(10.0);
    top.y = AnimatedValue::Static(4.0);
    top.scale_x = AnimatedValue::Static(0.5);
    top.scale_y = AnimatedValue::Static(0.5);

    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite2(&[&bottom, &top], &[&base, &over], canvas) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite2(&mut gpu, &[&bottom, &top], &[&base, &over], canvas) else {
        panic!("a positioned overlay must now composite on the GPU");
    };

    let want = (10u32, 4u32, 41u32, 35u32);
    assert_eq!(
        lit_bbox(&gpu_out, canvas.0, canvas.1),
        Some(want),
        "the GPU must put the overlay where the CPU was measured to"
    );
    assert_eq!(
        lit_bbox(&cpu, canvas.0, canvas.1),
        Some(want),
        "the CPU fixture must still land where it was measured"
    );
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!("overlay placement GPU vs CPU: mean={mean:.3}");
    assert!(
        mean <= TOL_LETTERBOX_MEAN,
        "placement diverged: mean={mean}"
    );
}

#[test]
fn overlay_over_a_letterboxed_base_gpu_should_match_cpu_within_tolerance() {
    // The fixture that pins the multiplier against the **base** size rather than the
    // canvas: a 0.5-scaled overlay over a 64x32 base in a 64x64 canvas was measured at
    // (0, 16)..(31, 31) inclusive. Scaled against the canvas it would be twice as tall
    // and would not sit inside the base's letterbox band at all.
    let canvas = (64, 64);
    let base = flat_frame(64, 32, 0);
    let over = flat_frame(64, 64, 255);
    let bottom = base_layer(64, 32, vec![]);
    let mut top = base_layer(64, 64, vec![]);
    top.scale_x = AnimatedValue::Static(0.5);
    top.scale_y = AnimatedValue::Static(0.5);

    let Some(mut gpu) = GpuCompositor::new() else {
        return;
    };
    let Some(cpu) = cpu_composite2(&[&bottom, &top], &[&base, &over], canvas) else {
        return;
    };
    let Some(gpu_out) = gpu_composite2(&mut gpu, &[&bottom, &top], &[&base, &over], canvas) else {
        panic!("an overlay over a letterboxed base must composite on the GPU");
    };

    let want = (0u32, 16u32, 31u32, 31u32);
    assert_eq!(lit_bbox(&gpu_out, canvas.0, canvas.1), Some(want));
    assert_eq!(lit_bbox(&cpu, canvas.0, canvas.1), Some(want));
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!("overlay over letterboxed base GPU vs CPU: mean={mean:.3}");
    assert!(
        mean <= TOL_LETTERBOX_MEAN,
        "placement diverged: mean={mean}"
    );
}

#[test]
fn offset_overlay_over_a_letterboxed_base_gpu_should_match_cpu_within_tolerance() {
    // The fixture a mutation check said was missing: the offset must be measured against
    // the **base** size, and only a fixture with a non-canvas base *and* a non-zero
    // offset can tell that apart from measuring against the canvas. Base 64x32 in a
    // 64x64 canvas, overlay scaled 0.5 at (10, 4) in base pixels -> the band is rows
    // 16..47, and the overlay sits at (10, 20)..(41, 35) inside it.
    let canvas = (64, 64);
    let base = flat_frame(64, 32, 0);
    let over = flat_frame(64, 64, 255);
    let bottom = base_layer(64, 32, vec![]);
    let mut top = base_layer(64, 64, vec![]);
    top.x = AnimatedValue::Static(10.0);
    top.y = AnimatedValue::Static(4.0);
    top.scale_x = AnimatedValue::Static(0.5);
    top.scale_y = AnimatedValue::Static(0.5);

    let Some(mut gpu) = GpuCompositor::new() else {
        return;
    };
    let Some(cpu) = cpu_composite2(&[&bottom, &top], &[&base, &over], canvas) else {
        return;
    };
    let Some(gpu_out) = gpu_composite2(&mut gpu, &[&bottom, &top], &[&base, &over], canvas) else {
        panic!("an offset overlay over a letterboxed base must composite on the GPU");
    };

    let gpu_box = lit_bbox(&gpu_out, canvas.0, canvas.1);
    let cpu_box = lit_bbox(&cpu, canvas.0, canvas.1);
    println!("offset overlay over letterboxed base: gpu={gpu_box:?} cpu={cpu_box:?}");
    assert_eq!(
        gpu_box, cpu_box,
        "the offset must be measured against the base size, not the canvas"
    );
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!("offset overlay GPU vs CPU: mean={mean:.3}");
    assert!(
        mean <= TOL_LETTERBOX_MEAN,
        "placement diverged: mean={mean}"
    );
}

#[test]
fn an_overlay_spilling_outside_the_base_should_fall_back() {
    // `overlay` clips the layer to the base-sized accumulator on the CPU, while the GPU
    // draws straight into the canvas and would show the overhang. Nothing to map that
    // to, so it falls back (RK-020). Found by a mutation check: no fixture crossed the
    // base edge, so nothing pinned it.
    let canvas = (64, 64);
    let base = flat_frame(64, 64, 0);
    let over = flat_frame(64, 64, 255);
    let bottom = base_layer(64, 64, vec![]);
    let mut top = base_layer(64, 64, vec![]);
    top.x = AnimatedValue::Static(40.0); // 40 + 0.5 * 64 = 72 > 64
    top.scale_x = AnimatedValue::Static(0.5);
    top.scale_y = AnimatedValue::Static(0.5);

    let Some(mut gpu) = GpuCompositor::new() else {
        return;
    };
    assert!(
        gpu_composite2(&mut gpu, &[&bottom, &top], &[&base, &over], canvas).is_none(),
        "an overlay hanging off the base edge must fall back"
    );
}

#[test]
fn a_positioned_single_layer_should_composite_on_the_gpu_unmoved() {
    // A lone layer is the compositor's *base*, whose transform the CPU ignores entirely
    // (measured). So the GPU must ignore it too -- and, since both agree, must stop
    // falling back. Asserting it renders identically to the same layer left alone is
    // what pins "ignored" rather than "applied a little".
    let canvas = (64, 64);
    let frame = VideoFrame::from_rgba(64, 64, gradient_rgba(64, 64)).unwrap();
    let plain = base_layer(64, 64, vec![]);
    let mut moved = base_layer(64, 64, vec![]);
    moved.x = AnimatedValue::Static(10.0);
    moved.y = AnimatedValue::Static(4.0);
    moved.scale_x = AnimatedValue::Static(0.5);
    moved.scale_y = AnimatedValue::Static(0.5);
    moved.rotation = AnimatedValue::Static(30.0);

    let Some(mut gpu) = GpuCompositor::new() else {
        return;
    };
    let Some(reference) = gpu_composite(&mut gpu, &plain, &frame, canvas) else {
        panic!("an identity layer must composite");
    };
    let Some(got) = gpu_composite(&mut gpu, &moved, &frame, canvas) else {
        panic!("a positioned base layer must no longer fall back (#1633)");
    };
    assert_eq!(
        got, reference,
        "a base layer transform is ignored on the CPU, so the GPU must not apply it"
    );
}

#[test]
fn a_rotated_overlay_should_still_fall_back() {
    // Deliberately unmapped: the CPU `rotate` fills the corners it exposes with
    // `fillcolor` while the GPU transform leaves them transparent (RK-020). This pins
    // that lifting the placement gate did not quietly enable rotation too.
    let canvas = (64, 64);
    let base = flat_frame(64, 64, 0);
    let over = flat_frame(64, 64, 255);
    let bottom = base_layer(64, 64, vec![]);
    let mut top = base_layer(64, 64, vec![]);
    top.rotation = AnimatedValue::Static(30.0);

    let Some(mut gpu) = GpuCompositor::new() else {
        return;
    };
    assert!(
        gpu_composite2(&mut gpu, &[&bottom, &top], &[&base, &over], canvas).is_none(),
        "a rotated overlay has no GPU equivalent and must fall back"
    );
}

#[test]
fn passthrough_gpu_should_match_input_within_tolerance() {
    // The broadest guard (adapter only, no filters): an identity, canvas-sized layer
    // composited on the GPU must reproduce the input. Catches stretch / colour-space /
    // corruption regressions.
    let (w, h) = (64, 48);
    let input = gradient_rgba(w, h);
    let frame = VideoFrame::from_rgba(w, h, input.clone()).unwrap();
    let layer = base_layer(w, h, vec![]);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported passthrough layer must composite on the GPU");
    };
    assert_eq!(out.len(), input.len());
    let mean = mean_abs_diff_rgb(&out, &input);
    println!(
        "passthrough GPU vs input: mean={mean:.3} max={}",
        max_abs_diff_rgb(&out, &input)
    );
    assert!(
        mean <= TOL_PASSTHROUGH_MEAN,
        "GPU passthrough diverged from the input beyond tolerance: mean={mean}"
    );
}

#[test]
fn passthrough_gpu_should_match_cpu_within_tolerance() {
    // The literal preview parity: GPU compositor vs CPU RealtimeComposer for the
    // supported passthrough case. Double-gated (adapter + filters).
    let (w, h) = (64, 48);
    let frame = VideoFrame::from_rgba(w, h, gradient_rgba(w, h)).unwrap();
    let layer = base_layer(w, h, vec![]);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported passthrough layer must composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "passthrough GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_PASSTHROUGH_MEAN,
        "GPU and CPU passthrough diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn letterbox_gpu_should_match_cpu_within_tolerance() {
    // #1661: a 16:9 layer on a square canvas used to fall back to CPU, because the
    // compositor stretched it where the CPU path fits and pads. It now letterboxes, so
    // both routes must agree. Double-gated (adapter + filters).
    let (fw, fh) = (64, 36);
    let canvas = (64, 64);
    let frame = VideoFrame::from_rgba(fw, fh, gradient_rgba(fw, fh)).unwrap();
    let layer = base_layer(fw, fh, vec![]);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, canvas) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, canvas) else {
        panic!("a 16:9 layer on a square canvas must now composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());

    // The bars are (64 - 36) / 2 = 14 rows top and bottom.
    let bar = (canvas.1 - fh) / 2;
    let top = box_mean_rgb(&gpu_out, canvas.0, 0, 0, canvas.0, bar);
    let bottom = box_mean_rgb(&gpu_out, canvas.0, 0, canvas.1 - bar, canvas.0, canvas.1);
    let band = box_mean_rgb(&gpu_out, canvas.0, 0, bar, canvas.0, canvas.1 - bar);
    println!("letterbox GPU bars: top={top:?} bottom={bottom:?} band={band:?}");
    // Non-vacuity, and the acceptance criterion itself: a *stretched* render fills the
    // canvas, so its bar rows carry picture, while an all-black render would trivially
    // match a black reference. Asserting both ends pins the geometry, not just the diff.
    // RGB only: the compositor writes the canvas alpha to every output pixel (RK-024).
    for (name, mean) in [("top", top), ("bottom", bottom)] {
        assert!(
            mean.iter().all(|c| *c < 2.0),
            "the {name} letterbox bar must be black, got {mean:?} (a stretch fills it)"
        );
    }
    assert!(
        band.iter().any(|c| *c > 32.0),
        "the letterboxed picture band must carry the source, got {band:?}"
    );

    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "letterbox GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_LETTERBOX_MEAN,
        "GPU and CPU letterbox diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn letterbox_gpu_should_match_cpu_when_the_fit_rescales() {
    // The 16:9-into-square case above fits 1:1 (64x36 into a 64x36 band), so it drives
    // the placement but neither the resampling nor the size rounding. This one does
    // both: a 3:2 source into a 100x100 canvas fits to 100x66 — the exact height is
    // 66.67, which `av_rescale` takes to 67 and `force_divisible_by=2` back down to 66.
    // Getting that rounding wrong offsets the whole band by a row against the CPU leg.
    let (fw, fh) = (30, 20);
    let canvas = (100, 100);
    let frame = VideoFrame::from_rgba(fw, fh, gradient_rgba(fw, fh)).unwrap();
    let layer = base_layer(fw, fh, vec![]);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, canvas) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, canvas) else {
        panic!("a 3:2 layer on a square canvas must composite on the GPU");
    };

    // The fitted band is 66 rows, so the bars are (100 - 66) / 2 = 17 rows each.
    let bar = (canvas.1 - 66) / 2;
    let top = box_mean_rgb(&gpu_out, canvas.0, 0, 0, canvas.0, bar);
    let band = box_mean_rgb(&gpu_out, canvas.0, 0, bar, canvas.0, canvas.1 - bar);
    println!("rescaled letterbox GPU bars: top={top:?} band={band:?}");
    assert!(
        top.iter().all(|c| *c < 2.0),
        "the bar above a rescaled fit must be black, got {top:?}"
    );
    assert!(
        band.iter().any(|c| *c > 32.0),
        "the rescaled picture band must carry the source, got {band:?}"
    );

    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "rescaled letterbox GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_LETTERBOX_RESCALE_MEAN,
        "GPU and CPU rescaled letterbox diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn color_grade_gpu_should_match_cpu_within_tolerance() {
    // ColorGrade sanity: GPU ColorGradeNode vs FFmpeg `eq`. Different implementations,
    // so a loose tolerance (calibrated). Double-gated.
    let (w, h) = (64, 48);
    let frame = VideoFrame::from_rgba(w, h, gradient_rgba(w, h)).unwrap();
    let layer = base_layer(
        w,
        h,
        vec![FilterStep::Eq {
            brightness: 0.1,
            contrast: 1.2,
            saturation: 1.1,
            temperature: 0.0,
            tint: 0.0,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported colour-graded layer must composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "color-grade GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_COLOR_GRADE_MEAN,
        "GPU and CPU colour grade diverged beyond tolerance: mean={mean}"
    );
}

/// Applies the `ColorGradeNode`'s temperature/tint offset (`r += temp*0.1`, `b -= temp*0.1`,
/// `g += tint*0.1`, per pixel, clamped) to an rgba buffer, mirroring the node so the CPU
/// parity reference reflects a leg FFmpeg `eq` cannot express.
fn apply_temp_tint_offset(rgba: &[u8], temperature: f32, tint: f32) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        let r = f32::from(px[0]) / 255.0 + temperature * 0.1;
        let g = f32::from(px[1]) / 255.0 + tint * 0.1;
        let b = f32::from(px[2]) / 255.0 - temperature * 0.1;
        px[0] = (r.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        px[1] = (g.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        px[2] = (b.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        // alpha unchanged
    }
    out
}

#[test]
fn color_grade_temperature_tint_gpu_should_match_cpu_reference() {
    // AC3: temperature/tint must reach the GPU ColorGradeNode (not the old hardcoded 0.0).
    // FFmpeg `eq` cannot express the node's ±0.1 offset model, so the CPU reference is the
    // neutral-eq CPU output plus that offset applied in Rust (Q3 decision). Isolate temp/tint
    // with neutral brightness/contrast/saturation, so the node reduces to just the offset and
    // the post-offset reference is order-faithful. Double-gated (adapter + filters).
    let (w, h) = (64, 48);
    let frame = VideoFrame::from_rgba(w, h, gradient_rgba(w, h)).unwrap();
    let (temperature, tint) = (0.6_f32, -0.4_f32);
    let layer = base_layer(
        w,
        h,
        vec![FilterStep::Eq {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature,
            tint,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    // The CPU leg must actually run, not silently skip = false green (RK-024 / RK-002).
    let Some(cpu_neutral) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // FFmpeg filters unavailable: skip
    };
    // The CPU `eq` ignores temperature/tint (GPU-only), so its output must stay ~neutral; the
    // reference adds the node's offset on top of it.
    let reference = apply_temp_tint_offset(&cpu_neutral, temperature, tint);
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported colour-graded layer must composite on the GPU");
    };
    // Non-vacuity: temperature/tint must actually shift the frame (temp>0 raises R / lowers B,
    // tint<0 lowers G), so a hardcoded-0.0 regression (GPU == neutral) fails both guards.
    assert!(
        mean_abs_diff_rgb(&reference, &cpu_neutral) > 5.0,
        "the temperature/tint offset must visibly shift the reference"
    );
    assert!(
        mean_abs_diff_rgb(&gpu_out, &cpu_neutral) > 5.0,
        "the GPU grade must apply temperature/tint (not the old hardcoded 0.0)"
    );
    let mean = mean_abs_diff_rgb(&gpu_out, &reference);
    println!(
        "grade temp/tint GPU vs reference: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &reference)
    );
    assert!(
        mean <= TOL_GRADE_TEMPTINT_MEAN,
        "GPU temperature/tint grade diverged from the reference: mean={mean}"
    );
}

#[test]
fn blur_gpu_should_match_cpu_within_tolerance() {
    // Gaussian blur parity: GPU GaussianBlurNode (map_scene maps `GBlur`) vs the CPU
    // `gblur` filter. Different kernel implementations, so a loose calibrated
    // tolerance. Double-gated (adapter + filters); a high-frequency checkerboard makes
    // it non-vacuous (a GPU that did not blur would diverge far).
    let (w, h) = (64, 48);
    let frame = VideoFrame::from_rgba(w, h, checkerboard_rgba(w, h, 4)).unwrap();
    let layer = base_layer(w, h, vec![FilterStep::GBlur { sigma: 3.0 }]);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported blur layer must composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "blur GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_BLUR_MEAN,
        "GPU and CPU blur diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn scale_gpu_should_match_cpu_within_tolerance() {
    // Scale parity: GPU ScaleNode (map_scene maps `Scale`) vs the CPU `scale` filter,
    // both bilinear. Downscale a 64x48 gradient to 32x24 (same 4:3 aspect as the canvas,
    // so the compositor does not fall back) and compare at the target size. Different
    // resampler implementations, so a loose calibrated tolerance. Double-gated.
    let (sw, sh) = (64, 48);
    let (tw, th) = (32, 24);
    let frame = VideoFrame::from_rgba(sw, sh, gradient_rgba(sw, sh)).unwrap();
    let layer = base_layer(
        sw,
        sh,
        vec![FilterStep::Scale {
            width: tw,
            height: th,
            algorithm: ScaleAlgorithm::Bilinear,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (tw, th)) else {
        return; // filters unavailable
    };
    let Some((gpu_out, gw, gh)) = gpu.composite(&[(&layer, &frame)], (tw, th), Duration::ZERO)
    else {
        panic!("a supported scaled layer must composite on the GPU");
    };
    // The Scale node actually ran: the output is the target size, not the source size.
    assert_eq!(
        (gw, gh),
        (tw, th),
        "the scaled output must be the target size"
    );
    assert_eq!(gpu_out.len(), cpu.len());
    // Non-vacuity (RK-015): the downscaled gradient is not flat, so a bypassed scale (or a
    // constant frame) would not produce this spread.
    assert!(
        max_abs_diff_rgb(&gpu_out, &vec![gpu_out[0]; gpu_out.len()]) > 20,
        "the scaled gradient must vary across the frame (non-vacuous)"
    );
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "scale GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_SCALE_MEAN,
        "GPU and CPU scale diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn sharpen_gpu_should_match_cpu_within_tolerance() {
    // Sharpen parity: GPU SharpenNode (map_scene maps `Unsharp`, luma-only, fixed
    // sigma) vs the CPU `unsharp` filter (YUV, 5x5). The algorithms differ, so a
    // loose calibrated tolerance. Double-gated (adapter + filters).
    //
    // A *mid-tone* checkerboard (90/160, not 0/255): a pure black/white board is
    // saturated, so an unsharp overshoot clamps and the effect is a no-op (a vacuous
    // test). Mid-tone edges have headroom, so sharpening visibly overshoots them and
    // a GPU that did not sharpen would diverge (non-vacuous, RK-015).
    let (w, h) = (64, 48);
    let mut mid = vec![0u8; (w * h * 4) as usize];
    for (i, px) in mid.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = i as u32 % w;
        let y = i as u32 / w;
        let v = if (x / 8 + y / 8) % 2 == 0 {
            90u8
        } else {
            160u8
        };
        *px = [v, v, v, 255];
    }
    let input = mid.clone();
    let frame = VideoFrame::from_rgba(w, h, mid).unwrap();
    let layer = base_layer(
        w,
        h,
        vec![FilterStep::Unsharp {
            luma_strength: 0.8,
            chroma_strength: 0.0,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported sharpen layer must composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    let effect = mean_abs_diff_rgb(&gpu_out, &input);
    println!(
        "sharpen GPU vs CPU: mean={mean:.3} max={} (GPU vs input: {effect:.3})",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    // Non-vacuous (RK-015): the sharpen must actually change the frame, so a GPU that
    // silently skipped it would fail here even though the flat regions match the CPU.
    assert!(
        effect > 1.0,
        "the GPU sharpen must visibly alter the mid-tone edges; got {effect}"
    );
    assert!(
        mean <= TOL_SHARPEN_MEAN,
        "GPU and CPU sharpen diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn vignette_gpu_should_match_cpu_within_tolerance() {
    // Vignette parity: GPU VignetteNode (map_scene maps `VignetteAnimated`) vs the CPU
    // `vignette` filter. The falloff profiles differ (smoothstep vs cos^4), so a loose
    // calibrated tolerance. Double-gated (adapter + filters).
    //
    // A colored gradient (RK-022): a vignette scales every channel by the same factor,
    // so GPU (all-RGB node) and CPU (FFmpeg) stay color-consistent here, unlike a
    // channel-subset effect. The corners darken, so a GPU that skipped the vignette
    // would keep the bright gradient and diverge (non-vacuous, RK-015).
    let (w, h) = (64, 48);
    let input = gradient_rgba(w, h);
    let frame = VideoFrame::from_rgba(w, h, input.clone()).unwrap();
    let layer = base_layer(
        w,
        h,
        vec![FilterStep::VignetteAnimated {
            amount: AnimatedValue::Static(0.8),
            x0: 0.0,
            y0: 0.0,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported vignette layer must composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    let effect = mean_abs_diff_rgb(&gpu_out, &input);
    println!(
        "vignette GPU vs CPU: mean={mean:.3} max={} (GPU vs input: {effect:.3})",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    // Non-vacuous (RK-015): the vignette must actually darken the frame.
    assert!(
        effect > 2.0,
        "the GPU vignette must visibly darken the frame; got {effect}"
    );
    assert!(
        mean <= TOL_VIGNETTE_MEAN,
        "GPU and CPU vignette diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn film_grain_gpu_should_match_cpu_grain_strength() {
    // Film grain parity is statistical, not per-pixel: the GPU node (Wang hash) and
    // the CPU `noise` filter use different RNGs, so their grain patterns never match.
    // Instead compare the grain *magnitude* (std of output - input) on a flat mid-grey
    // frame, which both must move by a comparable amount. Double-gated (adapter +
    // filters). Non-vacuous (RK-015): a GPU that added no grain gives std 0 and fails.
    let (w, h) = (64, 48);
    let mut input = vec![0u8; (w * h * 4) as usize];
    for px in input.as_chunks_mut::<4>().0 {
        *px = [128, 128, 128, 255];
    }
    let frame = VideoFrame::from_rgba(w, h, input.clone()).unwrap();
    let layer = base_layer(
        w,
        h,
        vec![FilterStep::FilmGrain {
            luma_strength: 20.0,
            chroma_strength: 20.0,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported film-grain layer must composite on the GPU");
    };
    let std_gpu = std_delta_rgb(&gpu_out, &input);
    let std_cpu = std_delta_rgb(&cpu, &input);
    println!("filmgrain grain std: gpu={std_gpu:.3} cpu={std_cpu:.3}");
    // Non-vacuous: both paths must actually add grain.
    assert!(
        std_gpu > 1.0 && std_cpu > 1.0,
        "both paths must add visible grain; gpu={std_gpu} cpu={std_cpu}"
    );
    assert!(
        (std_gpu - std_cpu).abs() <= TOL_FILMGRAIN_STD,
        "GPU and CPU grain strength diverged: gpu={std_gpu} cpu={std_cpu}"
    );
}

#[test]
fn glow_gpu_should_match_cpu_within_tolerance() {
    // Glow parity: GPU GlowNode (map_scene maps `Glow`) vs the CPU compound glow.
    // Different bloom implementations, so a loose calibrated tolerance. Double-gated
    // (adapter + filters).
    //
    // A coloured highlight (bright cyan rect on a dark background, RK-022): glow
    // extracts the highlight, blurs it, and adds it back, spreading its colour. The
    // corners around the rect gain a glow halo, so a GPU that skipped the glow would
    // keep the dark background and diverge (non-vacuous, RK-015). radius <= 20 keeps
    // it within the GPU blur node's sigma range.
    let (w, h) = (64, 48);
    let mut input = vec![0u8; (w * h * 4) as usize];
    for (i, px) in input.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = i as u32 % w;
        let y = i as u32 / w;
        px[3] = 255;
        if (24..40).contains(&x) && (16..32).contains(&y) {
            *px = [40, 230, 230, 255]; // bright cyan highlight
        }
    }
    let frame = VideoFrame::from_rgba(w, h, input.clone()).unwrap();
    let layer = base_layer(
        w,
        h,
        vec![FilterStep::Glow {
            threshold: 0.5,
            radius: 8.0,
            intensity: 1.0,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported glow layer must composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    let effect = mean_abs_diff_rgb(&gpu_out, &input);
    println!(
        "glow GPU vs CPU: mean={mean:.3} max={} (GPU vs input: {effect:.3})",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    // Non-vacuous (RK-015): the glow must actually change the frame.
    assert!(
        effect > 2.0,
        "the GPU glow must visibly add a halo; got {effect}"
    );
    assert!(
        mean <= TOL_GLOW_MEAN,
        "GPU and CPU glow diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn color_wheels_gpu_should_match_cpu_within_tolerance() {
    // ColorWheels parity: GPU ColorWheelsNode vs the CPU `curves`-based ThreeWayCC.
    // The lift/gamma/gain models differ (luma-region weighting vs per-channel curves),
    // so a loose calibrated tolerance. Double-gated (adapter + filters). A colour
    // gradient (RK-022) exercises the per-channel correction; the corrector changes the
    // frame, so a GPU that skipped it would diverge (non-vacuous, RK-015).
    let (w, h) = (64, 48);
    let input = gradient_rgba(w, h);
    let frame = VideoFrame::from_rgba(w, h, input.clone()).unwrap();
    // Warm shadows lift + brighter midtones + slightly boosted highlights.
    let layer = base_layer(
        w,
        h,
        vec![FilterStep::ThreeWayCC {
            lift: ff_filter::Rgb {
                r: 1.1,
                g: 1.0,
                b: 0.95,
            },
            gamma: ff_filter::Rgb {
                r: 1.2,
                g: 1.1,
                b: 1.0,
            },
            gain: ff_filter::Rgb {
                r: 1.1,
                g: 1.05,
                b: 1.0,
            },
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported ColorWheels layer must composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    let effect = mean_abs_diff_rgb(&gpu_out, &input);
    println!(
        "color-wheels GPU vs CPU: mean={mean:.3} max={} (GPU vs input: {effect:.3})",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    // Non-vacuous (RK-015): the corrector must actually change the frame.
    assert!(
        effect > 2.0,
        "the GPU ColorWheels must visibly grade the frame; got {effect}"
    );
    assert!(
        mean <= TOL_COLOR_WHEELS_MEAN,
        "GPU and CPU ColorWheels diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn curves_gpu_should_match_cpu_within_tolerance() {
    // Curves parity: GPU CurvesNode (map_scene maps `Curves`) vs the CPU `curves`
    // filter. Both interpolate the same control points but with different splines
    // (Steffen monotone cubic vs FFmpeg's), so a loose calibrated tolerance.
    // Double-gated (adapter + filters). A colour gradient (RK-022) exercises the
    // per-channel tone shift; the curve changes the frame, so a GPU that skipped it
    // would diverge (non-vacuous, RK-015).
    let (w, h) = (64, 48);
    let input = gradient_rgba(w, h);
    let frame = VideoFrame::from_rgba(w, h, input.clone()).unwrap();
    // Lifted-midtones master curve + a slight red boost.
    let layer = base_layer(
        w,
        h,
        vec![FilterStep::Curves {
            master: vec![(0.0, 0.0), (0.5, 0.62), (1.0, 1.0)],
            r: vec![(0.0, 0.05), (1.0, 1.0)],
            g: vec![],
            b: vec![],
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported Curves layer must composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    let effect = mean_abs_diff_rgb(&gpu_out, &input);
    println!(
        "curves GPU vs CPU: mean={mean:.3} max={} (GPU vs input: {effect:.3})",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    // Non-vacuous (RK-015): the curve must actually change the frame.
    assert!(
        effect > 2.0,
        "the GPU curves must visibly grade the frame; got {effect}"
    );
    assert!(
        mean <= TOL_CURVES_MEAN,
        "GPU and CPU curves diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn hsl_gpu_should_match_cpu_within_tolerance() {
    // HSL parity: GPU HslNode (map_scene maps `Hsl`) vs the CPU `hue` filter. The
    // node works in HSL space; `hue` works in YUV (chroma rotation + a luma-add
    // brightness), so they only agree within a wide calibrated tolerance.
    // Double-gated (adapter + filters). A colour gradient (RK-022) exercises the
    // hue/saturation shift across channels; the adjustment changes the frame, so a
    // GPU that skipped it would diverge (non-vacuous, RK-015).
    let (w, h) = (64, 48);
    let input = gradient_rgba(w, h);
    let frame = VideoFrame::from_rgba(w, h, input.clone()).unwrap();
    // Modest hue rotation + saturation boost + a small lightness lift.
    let layer = base_layer(
        w,
        h,
        vec![FilterStep::Hsl {
            hue: 20.0,
            saturation: 1.2,
            lightness: 0.05,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite(&layer, &frame, (w, h)) else {
        return; // filters unavailable
    };
    let Some(gpu_out) = gpu_composite(&mut gpu, &layer, &frame, (w, h)) else {
        panic!("a supported Hsl layer must composite on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    let effect = mean_abs_diff_rgb(&gpu_out, &input);
    println!(
        "hsl GPU vs CPU: mean={mean:.3} max={} (GPU vs input: {effect:.3})",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    // Non-vacuous (RK-015): the HSL adjustment must actually change the frame.
    assert!(
        effect > 2.0,
        "the GPU HSL adjustment must visibly change the frame; got {effect}"
    );
    assert!(
        mean <= TOL_HSL_MEAN,
        "GPU and CPU HSL diverged beyond tolerance: mean={mean}"
    );
}

/// Writes a size-`n` `.cube` LUT to a temp file whose grid applies `shift(r,g,b)`
/// (each output channel in `[0, 1]`), returning the path. Red-fastest order (the
/// `.cube` convention).
fn write_cube(n: usize, shift: impl Fn(f32, f32, f32) -> [f32; 3]) -> std::path::PathBuf {
    let last = (n - 1) as f32;
    let mut s = format!("LUT_3D_SIZE {n}\n");
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let o = shift(r as f32 / last, g as f32 / last, b as f32 / last);
                s.push_str(&format!("{} {} {}\n", o[0], o[1], o[2]));
            }
        }
    }
    let path = std::env::temp_dir().join(format!("avio_parity_{}.cube", std::process::id()));
    std::fs::write(&path, s).expect("write temp cube");
    path
}

#[test]
fn lut_gpu_should_match_cpu_within_tolerance() {
    // LUT parity: GPU LutNode (map_scene maps `Lut3d`) vs the CPU `lut3d` filter,
    // both loading the same .cube and interpolating trilinearly. Double-gated
    // (adapter + filters). A colour gradient (RK-022) with a per-channel-shifting
    // LUT; the LUT changes the frame, so a GPU that skipped it would diverge
    // (non-vacuous, RK-015).
    let (w, h) = (64, 48);
    let input = gradient_rgba(w, h);
    let frame = VideoFrame::from_rgba(w, h, input.clone()).unwrap();
    // A gentle per-channel curve: lift red, keep green, pull blue.
    let path = write_cube(17, |r, g, b| [(r * 1.1).min(1.0), g, (b * 0.85).max(0.0)]);
    let layer = base_layer(
        w,
        h,
        vec![FilterStep::Lut3d {
            path: path.to_string_lossy().into_owned(),
        }],
    );
    let result = (|| {
        let mut gpu = GpuCompositor::new()?; // no adapter
        let cpu = cpu_composite(&layer, &frame, (w, h))?; // filters unavailable
        let gpu_out = gpu_composite(&mut gpu, &layer, &frame, (w, h))
            .expect("a supported Lut layer must composite on the GPU");
        Some((cpu, gpu_out))
    })();
    let _ = std::fs::remove_file(&path);
    let Some((cpu, gpu_out)) = result else {
        return; // adapter or filters unavailable
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    let effect = mean_abs_diff_rgb(&gpu_out, &input);
    println!(
        "lut GPU vs CPU: mean={mean:.3} max={} (GPU vs input: {effect:.3})",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    // Non-vacuous (RK-015): the LUT must actually change the frame.
    assert!(
        effect > 2.0,
        "the GPU LUT must visibly change the frame; got {effect}"
    );
    assert!(
        mean <= TOL_LUT_MEAN,
        "GPU and CPU LUT diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn chroma_key_gpu_should_match_cpu_within_tolerance() {
    // ChromaKey parity: GPU ChromaKeyNode (map_scene maps `ChromaKey`) vs the CPU
    // `chromakey` filter. ChromaKey rewrites alpha, but the GPU compositor's blend
    // shader outputs the canvas alpha rather than the composited overlay alpha
    // (blend.wgsl), so parity cannot compare composited alpha and a composited-RGB
    // comparison of the keyed layer alone is vacuous. Instead the keyed foreground is
    // composited over an *opaque* background: the compositor's `mix(base, overlay,
    // overlay.a)` turns the keyed alpha into an RGB difference (background shows through
    // the keyed green half), making an RGB parity non-vacuous (RK-015). Double-gated
    // (adapter + filters).
    let (w, h) = (64, 48);
    let bg_rgb = [30u8, 60, 200]; // opaque blue background
    let bg = VideoFrame::from_rgba(w, h, solid_rgba(w, h, bg_rgb)).unwrap();
    // Foreground: left half pure key green, right half non-key red.
    let fg = VideoFrame::from_rgba(w, h, key_split_rgba(w, h)).unwrap();
    let bg_layer = base_layer(w, h, vec![]);
    let fg_layer = base_layer(
        w,
        h,
        vec![FilterStep::ChromaKey {
            color: "0x00FF00".to_string(),
            similarity: 0.3,
            blend: 0.1,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite2(&[&bg_layer, &fg_layer], &[&bg, &fg], (w, h)) else {
        return; // filters unavailable
    };
    let Some((gpu_out, _, _)) = gpu.composite(
        &[(&bg_layer, &bg), (&fg_layer, &fg)],
        (w, h),
        Duration::ZERO,
    ) else {
        panic!("a supported chroma-key composite must render on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    // Non-vacuity guard (RK-015): the keyed green half must show the blue background
    // through it, while the non-key red half stays red. A GPU that failed to key would
    // keep green in the left half and diverge here.
    let key_half = region_mean_rgb(&gpu_out, w, true);
    let non_key_half = region_mean_rgb(&gpu_out, w, false);
    println!("chroma-key GPU key_half={key_half:?} non_key_half={non_key_half:?}");
    assert!(
        key_half[2] > 128.0 && key_half[1] < 128.0,
        "the keyed green half must reveal the blue background; got {key_half:?}"
    );
    assert!(
        non_key_half[0] > 128.0 && non_key_half[2] < 128.0,
        "the non-key red half must stay red; got {non_key_half:?}"
    );
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "chroma-key GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_CHROMAKEY_MEAN,
        "GPU and CPU chroma-key composite diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn luma_mask_gpu_should_match_cpu_within_tolerance() {
    // LumaMask parity: GPU LumaMaskNode (map_scene maps `LumaMask`) vs the CPU `geq`
    // self-luma mask. Both multiply alpha by the frame's own BT.709 luma. As with
    // ChromaKey the compositor discards composited alpha, so the masked foreground is
    // composited over an opaque background: the bright (opaque) half shows the
    // foreground, the dark (transparent) half reveals the background, making an RGB
    // parity non-vacuous (RK-015). Double-gated (adapter + filters).
    let (w, h) = (64, 48);
    let third = w / 3;
    let bg_rgb = [30u8, 60, 200]; // opaque blue background
    let bg = VideoFrame::from_rgba(w, h, solid_rgba(w, h, bg_rgb)).unwrap();
    // Foreground: white | pure red (mid luma) | black bands.
    let fg = VideoFrame::from_rgba(w, h, luma_bands_rgba(w, h)).unwrap();
    let bg_layer = base_layer(w, h, vec![]);
    let fg_layer = base_layer(w, h, vec![FilterStep::LumaMask { invert: false }]);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite2(&[&bg_layer, &fg_layer], &[&bg, &fg], (w, h)) else {
        return; // filters unavailable
    };
    let Some((gpu_out, _, _)) = gpu.composite(
        &[(&bg_layer, &bg), (&fg_layer, &fg)],
        (w, h),
        Duration::ZERO,
    ) else {
        panic!("a supported luma-mask composite must render on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    // Non-vacuity guards (RK-015 / RK-022): the white band stays opaque, the black band
    // reveals the background, and the red band is a *partial* blend (not pure red, not
    // pure background) whose exact value depends on the BT.709 coefficients. A GPU that
    // skipped the mask, thresholded instead of blending, or used other coefficients
    // would diverge from the CPU reference.
    let white = band_mean_rgb(&gpu_out, w, 0, third);
    let red = band_mean_rgb(&gpu_out, w, third, 2 * third);
    let black = band_mean_rgb(&gpu_out, w, 2 * third, w);
    println!("luma-mask GPU white={white:?} red={red:?} black={black:?}");
    assert!(
        white[0] > 192.0 && white[2] > 192.0,
        "the white band must stay opaque white; got {white:?}"
    );
    assert!(
        black[2] > 128.0 && black[0] < 128.0,
        "the black band must reveal the blue background; got {black:?}"
    );
    // Partial blend: red pulled in above the background red (30), but the blue
    // background still dominant (mid alpha ~54/255) — a threshold key would leave this
    // pure background instead.
    assert!(
        red[0] > 50.0 && red[0] < 200.0 && red[2] > 120.0,
        "the red band must be a partial fg/bg blend; got {red:?}"
    );
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "luma-mask GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_LUMAMASK_MEAN,
        "GPU and CPU luma-mask composite diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn luma_mask_invert_gpu_should_match_cpu_within_tolerance() {
    // The invert branch drives distinct new code: the GPU builds a grey `255 - luma`
    // mask and the CPU `geq` uses `255 - luma`. With invert the bright half becomes
    // transparent (reveals the background) and the dark half stays opaque, the mirror
    // of the non-inverted test. Double-gated (adapter + filters).
    let (w, h) = (64, 48);
    let third = w / 3;
    let bg = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [30, 60, 200])).unwrap();
    let fg = VideoFrame::from_rgba(w, h, luma_bands_rgba(w, h)).unwrap();
    let bg_layer = base_layer(w, h, vec![]);
    let fg_layer = base_layer(w, h, vec![FilterStep::LumaMask { invert: true }]);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite2(&[&bg_layer, &fg_layer], &[&bg, &fg], (w, h)) else {
        return; // filters unavailable
    };
    let Some((gpu_out, _, _)) = gpu.composite(
        &[(&bg_layer, &bg), (&fg_layer, &fg)],
        (w, h),
        Duration::ZERO,
    ) else {
        panic!("a supported inverted luma-mask composite must render on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    // Inverted mirror: the white band now reveals the background, the red band becomes
    // mostly opaque (alpha ~201/255), and the black band stays opaque black.
    let white = band_mean_rgb(&gpu_out, w, 0, third);
    let red = band_mean_rgb(&gpu_out, w, third, 2 * third);
    let black = band_mean_rgb(&gpu_out, w, 2 * third, w);
    println!("luma-mask(invert) GPU white={white:?} red={red:?} black={black:?}");
    assert!(
        white[2] > 128.0 && white[0] < 128.0,
        "the white band must reveal the blue background when inverted; got {white:?}"
    );
    assert!(
        black[0] < 64.0 && black[2] < 64.0,
        "the black band must stay opaque black when inverted; got {black:?}"
    );
    // Inverted red band is mostly opaque red (mid alpha flips to ~201).
    assert!(
        red[0] > 128.0 && red[2] < 128.0,
        "the inverted red band must be mostly opaque red; got {red:?}"
    );
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "luma-mask(invert) GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_LUMAMASK_MEAN,
        "GPU and CPU inverted luma-mask diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn shape_mask_gpu_should_match_cpu_within_tolerance() {
    // ShapeMask parity: GPU ShapeMaskNode (map_scene maps `RectMask`) vs the CPU `geq`
    // rectangle mask. The mask keeps the interior opaque and clears the exterior, so
    // the masked foreground is composited over an opaque background: the rectangle
    // interior shows the foreground, the exterior reveals the background. A centre
    // *box* (bounded in x AND y) pins both axes (RK-015): a full-height band would
    // leave the y-bounds and any x<->y / width<->height transposition unexercised.
    // Double-gated (adapter + filters).
    let (w, h) = (64, 48);
    let (rx, ry, rw, rh) = (w / 4, h / 4, w / 2, h / 2); // centre box: x[16,48) y[12,36)
    let bg = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [30, 60, 200])).unwrap(); // blue
    let fg = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [220, 40, 40])).unwrap(); // red
    let bg_layer = base_layer(w, h, vec![]);
    let fg_layer = base_layer(
        w,
        h,
        vec![FilterStep::RectMask {
            x: rx,
            y: ry,
            width: rw,
            height: rh,
            invert: false,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite2(&[&bg_layer, &fg_layer], &[&bg, &fg], (w, h)) else {
        return; // filters unavailable
    };
    let Some((gpu_out, _, _)) = gpu.composite(
        &[(&bg_layer, &bg), (&fg_layer, &fg)],
        (w, h),
        Duration::ZERO,
    ) else {
        panic!("a supported shape-mask composite must render on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    // Non-vacuity guards (RK-015): the box interior keeps the red foreground; the
    // corner (outside in both x and y) reveals the blue background. Driving both axes
    // catches a y-offset or transposition bug that a full-height band would miss.
    let interior = box_mean_rgb(&gpu_out, w, rx, ry, rx + rw, ry + rh);
    let corner = box_mean_rgb(&gpu_out, w, 0, 0, rx, ry);
    println!("shape-mask GPU interior={interior:?} corner={corner:?}");
    assert!(
        interior[0] > 128.0 && interior[2] < 128.0,
        "the box interior must keep the red foreground; got {interior:?}"
    );
    assert!(
        corner[2] > 128.0 && corner[0] < 128.0,
        "the corner must reveal the blue background; got {corner:?}"
    );
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "shape-mask GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_SHAPEMASK_MEAN,
        "GPU and CPU shape-mask composite diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn shape_mask_invert_gpu_should_match_cpu_within_tolerance() {
    // Inverted mirror: the box interior is cleared (reveals the background) and the
    // exterior keeps the foreground. Drives the shader's invert branch and the geq
    // inverted inside/outside, in both axes. Double-gated (adapter + filters).
    let (w, h) = (64, 48);
    let (rx, ry, rw, rh) = (w / 4, h / 4, w / 2, h / 2);
    let bg = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [30, 60, 200])).unwrap();
    let fg = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [220, 40, 40])).unwrap();
    let bg_layer = base_layer(w, h, vec![]);
    let fg_layer = base_layer(
        w,
        h,
        vec![FilterStep::RectMask {
            x: rx,
            y: ry,
            width: rw,
            height: rh,
            invert: true,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let Some(cpu) = cpu_composite2(&[&bg_layer, &fg_layer], &[&bg, &fg], (w, h)) else {
        return; // filters unavailable
    };
    let Some((gpu_out, _, _)) = gpu.composite(
        &[(&bg_layer, &bg), (&fg_layer, &fg)],
        (w, h),
        Duration::ZERO,
    ) else {
        panic!("a supported inverted shape-mask composite must render on the GPU");
    };
    assert_eq!(gpu_out.len(), cpu.len());
    let interior = box_mean_rgb(&gpu_out, w, rx, ry, rx + rw, ry + rh);
    let corner = box_mean_rgb(&gpu_out, w, 0, 0, rx, ry);
    println!("shape-mask(invert) GPU interior={interior:?} corner={corner:?}");
    assert!(
        interior[2] > 128.0 && interior[0] < 128.0,
        "the inverted box interior must reveal the blue background; got {interior:?}"
    );
    assert!(
        corner[0] > 128.0 && corner[2] < 128.0,
        "outside the inverted box must keep the red foreground; got {corner:?}"
    );
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu);
    println!(
        "shape-mask(invert) GPU vs CPU: mean={mean:.3} max={}",
        max_abs_diff_rgb(&gpu_out, &cpu)
    );
    assert!(
        mean <= TOL_SHAPEMASK_MEAN,
        "GPU and CPU inverted shape-mask diverged beyond tolerance: mean={mean}"
    );
}

#[test]
fn effect_cache_should_reuse_for_identical_const_effects() {
    // #1634: compositing the same const-effect layer twice must reuse the cached graph
    // and produce identical, correctly-effected output (not stale/garbage on the reuse).
    // Adapter-gated only (no CPU filters needed).
    let (w, h) = (64, 48);
    let input = gradient_rgba(w, h);
    let frame = VideoFrame::from_rgba(w, h, input.clone()).unwrap();
    let layer = base_layer(
        w,
        h,
        vec![FilterStep::Eq {
            brightness: 0.15,
            contrast: 1.2,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let out1 = gpu_composite(&mut gpu, &layer, &frame, (w, h)).expect("first composite");
    let out2 = gpu_composite(&mut gpu, &layer, &frame, (w, h)).expect("second composite (cached)");
    assert_eq!(
        out1, out2,
        "the cached reuse must match the first render exactly"
    );
    // Non-vacuous: the effect actually changed the frame (so a stale/empty cache would
    // be a visible divergence, not a no-op).
    assert!(
        mean_abs_diff_rgb(&out1, &input) > 2.0,
        "the colour grade must visibly change the frame"
    );
}

#[test]
fn effect_cache_should_rebuild_on_changed_effect() {
    // #1634 / RK-015: with the same compositor and input but a *different* effect param,
    // the cache must rebuild — the two outputs must differ. If the cache wrongly reused
    // the first graph, they would be equal and this fails.
    let (w, h) = (64, 48);
    let frame = VideoFrame::from_rgba(w, h, gradient_rgba(w, h)).unwrap();
    let dark = base_layer(
        w,
        h,
        vec![FilterStep::Eq {
            brightness: -0.3,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
        }],
    );
    let bright = base_layer(
        w,
        h,
        vec![FilterStep::Eq {
            brightness: 0.3,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let out_dark = gpu_composite(&mut gpu, &dark, &frame, (w, h)).expect("dark composite");
    let out_bright = gpu_composite(&mut gpu, &bright, &frame, (w, h)).expect("bright composite");
    assert!(
        mean_abs_diff_rgb(&out_dark, &out_bright) > 10.0,
        "a changed brightness must rebuild the cached graph and change the output"
    );
}

#[test]
fn effect_cache_should_not_panic_on_layer_count_change() {
    // #1634: the position-keyed cache resets when the layer count changes; shrinking the
    // count must not index out of bounds.
    let (w, h) = (64, 48);
    let fa = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [200, 40, 40])).unwrap();
    let fb = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [40, 40, 200])).unwrap();
    let eq = || {
        base_layer(
            w,
            h,
            vec![FilterStep::Eq {
                brightness: 0.1,
                contrast: 1.0,
                saturation: 1.0,
                temperature: 0.0,
                tint: 0.0,
            }],
        )
    };
    let (la, lb) = (eq(), eq());
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    // Two layers, then one: the cache (len 2) must reset to len 1 without panicking.
    assert!(
        gpu.composite(&[(&la, &fa), (&lb, &fb)], (w, h), Duration::ZERO)
            .is_some(),
        "two-layer composite must render"
    );
    assert!(
        gpu.composite(&[(&la, &fa)], (w, h), Duration::ZERO)
            .is_some(),
        "shrinking to one layer must not panic and must render"
    );
}

#[test]
fn effect_cache_should_rebuild_on_input_dimension_change() {
    // #1634 / RK-020: the cache key includes the input size, so reusing a graph across a
    // different frame size (a same-effect cut to a different resolution at the same layer
    // position) must rebuild — not wrap the read-back at the stale first-frame dimensions
    // (or reuse a dimension-sized mask). Without the size in the key the second composite
    // would return None (byte-length mismatch) or wrong dimensions.
    let (w1, h1) = (64, 48);
    let (w2, h2) = (32, 24);
    let eq = |w, h| {
        base_layer(
            w,
            h,
            vec![FilterStep::Eq {
                brightness: 0.15,
                contrast: 1.1,
                saturation: 1.0,
                temperature: 0.0,
                tint: 0.0,
            }],
        )
    };
    let f1 = VideoFrame::from_rgba(w1, h1, gradient_rgba(w1, h1)).unwrap();
    let f2 = VideoFrame::from_rgba(w2, h2, gradient_rgba(w2, h2)).unwrap();
    let (l1, l2) = (eq(w1, h1), eq(w2, h2));
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    // Prime the position-0 cache with the large frame, then composite a smaller frame
    // with the same effect fingerprint on the same compositor.
    gpu.composite(&[(&l1, &f1)], (w1, h1), Duration::ZERO)
        .expect("large composite primes the cache");
    let (small, sw, sh) = gpu
        .composite(&[(&l2, &f2)], (w2, h2), Duration::ZERO)
        .expect("smaller composite must rebuild, not reuse stale dims");
    assert_eq!((sw, sh), (w2, h2));
    // A fresh compositor (no stale cache) is the ground truth for the small frame.
    let mut fresh = GpuCompositor::new().expect("adapter present");
    let (reference, _, _) = fresh
        .composite(&[(&l2, &f2)], (w2, h2), Duration::ZERO)
        .expect("fresh composite");
    assert_eq!(
        small, reference,
        "the resized reuse must rebuild and match a fresh render"
    );
}

#[test]
fn composite_owned_should_match_borrowed_for_no_effects() {
    // #1634: the owned entry point (which moves the frame, avoiding a clone on the
    // no-effects path) must produce the same output as the borrowing `composite`.
    let (w, h) = (64, 48);
    let frame = VideoFrame::from_rgba(w, h, gradient_rgba(w, h)).unwrap();
    let layer = base_layer(w, h, vec![]);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let (borrowed, _, _) = gpu
        .composite(&[(&layer, &frame)], (w, h), Duration::ZERO)
        .expect("borrowed composite");
    let (owned, _, _) = gpu
        .composite_owned(vec![(&layer, frame.clone())], (w, h), Duration::ZERO)
        .expect("owned composite");
    assert_eq!(
        borrowed, owned,
        "composite_owned must match the borrowing composite output"
    );
}

/// A single `MotionBlur` layer over two frames on the CPU (`RealtimeComposer`, one
/// persistent `tblend` graph), returning the second (blended) frame's rgba, or `None`
/// when `FFmpeg` filters are unavailable (skip). The first pull is the unblended seed.
fn cpu_motion_blur_two_frames(
    layer: &RealtimeLayer,
    f1: &VideoFrame,
    f2: &VideoFrame,
    canvas: (u32, u32),
) -> Option<Vec<u8>> {
    let mut composer =
        RealtimeComposer::with_canvas(std::slice::from_ref(layer), Some(canvas)).ok()?;
    composer.push_layer(0, f1).ok()?;
    let _ = composer.pull().ok()?; // discard the first (unblended) frame
    composer.push_layer(0, f2).ok()?;
    composer.pull().ok()??.to_rgba()
}

/// A single `MotionBlur` layer over two frames on the GPU (`GpuCompositor`, whose cached
/// graph reuse lets the stateful node accumulate), returning the second frame's rgba.
/// `None` when no adapter is present or the layer is unsupported.
fn gpu_motion_blur_two_frames(
    gpu: &mut GpuCompositor,
    layer: &RealtimeLayer,
    f1: &VideoFrame,
    f2: &VideoFrame,
    canvas: (u32, u32),
) -> Option<Vec<u8>> {
    gpu.composite(&[(layer, f1)], canvas, Duration::ZERO)?; // seed the accumulation
    let (rgba, _, _) = gpu.composite(&[(layer, f2)], canvas, Duration::from_millis(33))?;
    Some(rgba)
}

/// A single `MotionBlur` layer (shutter 180, sub_frames 8): the calibration point where
/// the GPU IIR node and the CPU FIR-2 `tblend` both reduce to `0.5*prev + 0.5*current`.
fn motion_blur_layer(w: u32, h: u32) -> RealtimeLayer {
    base_layer(
        w,
        h,
        vec![FilterStep::MotionBlur {
            shutter_angle_degrees: 180.0,
            sub_frames: 8,
        }],
    )
}

#[test]
fn motion_blur_gpu_should_match_cpu_within_tolerance() {
    // AC3 (Br5): the GPU MotionBlurNode and the CPU `tblend` agree within a calibrated
    // tolerance. Temporal, so two frames of different solid colours make the trail
    // observable (a single frame is unblended on both = vacuous, RK-015). Double-gated.
    let (w, h) = (16, 16);
    let f1 = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [200, 0, 0])).unwrap(); // red
    let f2 = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [0, 0, 200])).unwrap(); // blue
    let layer = motion_blur_layer(w, h);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let gpu_out = gpu_motion_blur_two_frames(&mut gpu, &layer, &f1, &f2, (w, h))
        .expect("gpu motion blur second frame");
    // The CPU leg must actually run, not silently skip = false green (RK-024 / RK-002).
    let Some(cpu_out) = cpu_motion_blur_two_frames(&layer, &f1, &f2, (w, h)) else {
        return; // FFmpeg filters unavailable: skip
    };
    // Non-vacuity: the blue frame's trail must carry red from the previous frame (its own
    // input red is 0), so the GPU output's red channel is ~100, not 0.
    assert!(
        gpu_out[0] > 40,
        "the red frame must leave a trail on the blue frame (red > 0); got {}",
        gpu_out[0]
    );
    let mean = mean_abs_diff_rgb(&gpu_out, &cpu_out);
    assert!(
        mean <= TOL_MOTIONBLUR_MEAN,
        "GPU/CPU motion blur must agree within {TOL_MOTIONBLUR_MEAN}; got {mean}"
    );
}

#[test]
fn motion_blur_should_accumulate_across_frames() {
    // #1653: the effect-graph cache reuse within a clip is what lets the stateful
    // MotionBlurNode accumulate. Composite white then black at layer 0: the second output
    // must carry a fading trail from the white frame. Adapter-gated (pure GPU).
    let (w, h) = (8, 8);
    let white = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [255, 255, 255])).unwrap();
    let black = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [0, 0, 0])).unwrap();
    let layer = motion_blur_layer(w, h);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    let out = gpu_motion_blur_two_frames(&mut gpu, &layer, &white, &black, (w, h))
        .expect("gpu motion blur second frame");
    assert!(
        out[0] > 0,
        "the white frame must leave a trail on the black frame; got {}",
        out[0]
    );
}

#[test]
fn reset_effect_cache_should_reset_motion_blur_accumulation() {
    // RK-025: resetting the effect cache at a clip boundary drops the stateful node, so a
    // new clip's first frame is unblended (no white trail bleeds across the cut). Without
    // the reset the third composite would reuse the accumulated trail. Adapter-gated.
    let (w, h) = (8, 8);
    let white = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [255, 255, 255])).unwrap();
    let black = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [0, 0, 0])).unwrap();
    let layer = motion_blur_layer(w, h);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    // Clip A: accumulate a white trail, then cut.
    gpu.composite(&[(&layer, &white)], (w, h), Duration::ZERO)
        .expect("clip A frame 1");
    gpu.composite(&[(&layer, &black)], (w, h), Duration::from_millis(33))
        .expect("clip A frame 2");
    gpu.reset_effect_cache();
    // Clip B first frame (black): a fresh node has no history, so it is unblended.
    let (out, _, _) = gpu
        .composite(&[(&layer, &black)], (w, h), Duration::from_millis(66))
        .expect("clip B frame 1");
    assert!(
        out[0] <= 2,
        "after a reset the new clip's first frame must be unblended (no trail); got {}",
        out[0]
    );
}

/// A layer with a constant shutter, for comparing against an animated one.
fn constant_motion_blur_layer(w: u32, h: u32, degrees: f32) -> RealtimeLayer {
    base_layer(
        w,
        h,
        vec![FilterStep::MotionBlur {
            shutter_angle_degrees: degrees,
            sub_frames: 8,
        }],
    )
}

/// A layer whose shutter ramps `from` -> `to` over one second (#1705).
fn animated_motion_blur_layer(w: u32, h: u32, from: f64, to: f64) -> RealtimeLayer {
    let track = ff_filter::AnimationTrack::new()
        .push(ff_filter::Keyframe::new(
            Duration::ZERO,
            from,
            ff_filter::Easing::Linear,
        ))
        .push(ff_filter::Keyframe::new(
            Duration::from_secs(1),
            to,
            ff_filter::Easing::Linear,
        ));
    base_layer(
        w,
        h,
        vec![FilterStep::MotionBlurAnimated {
            shutter_angle: ff_filter::AnimatedValue::Track(track),
            sub_frames: 8,
        }],
    )
}

#[test]
fn an_animated_shutter_should_keep_accumulating_the_trail() {
    // The acceptance criterion, and the reason the shutter is pushed into the live
    // node rather than the node being rebuilt around it: a rebuild would drop the
    // trail every frame, so the output would equal the input every frame.
    //
    // White seed, then black frames with a shutter ramping down from 360. A retained
    // trail means the black frames read brighter than black.
    let (w, h) = (8, 8);
    let white = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [255, 255, 255])).unwrap();
    let black = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [0, 0, 0])).unwrap();
    let layer = animated_motion_blur_layer(w, h, 360.0, 0.0);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };
    gpu.composite(&[(&layer, &white)], (w, h), Duration::ZERO)
        .expect("seed");
    let (second, _, _) = gpu
        .composite(&[(&layer, &black)], (w, h), Duration::from_millis(33))
        .expect("second frame");
    assert!(
        second[0] > 8,
        "the seeded trail must survive the shutter changing; got {}",
        second[0]
    );
}

#[test]
fn an_animated_shutter_should_be_observable_per_frame() {
    // The other half: the shutter has to change what is rendered, per frame.
    //
    // The discriminator is the **first** frame after a white seed, where the two
    // shutters are furthest apart: a constant 360 retains the seed in full (bright),
    // while a ramp starting at 0 retains almost none of it (black). They converge
    // later, which is why an earlier version of this test — comparing the twentieth
    // frame, by which point both had decayed to 0 — passed without proving anything.
    let (w, h) = (8, 8);
    let white = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [255, 255, 255])).unwrap();
    let black = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [0, 0, 0])).unwrap();
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };

    // The first frame after the seed, for a layer's shutter.
    let mut first_after_seed = |layer: &RealtimeLayer| -> u8 {
        gpu.reset_effect_cache();
        gpu.composite(&[(layer, &white)], (w, h), Duration::ZERO)
            .expect("seed");
        let (out, _, _) = gpu
            .composite(&[(layer, &black)], (w, h), Duration::from_millis(33))
            .expect("first frame after the seed");
        out[0]
    };

    let held = first_after_seed(&constant_motion_blur_layer(w, h, 360.0));
    let ramping_up = first_after_seed(&animated_motion_blur_layer(w, h, 0.0, 360.0));
    println!("first frame after the seed: constant360={held} ramp0to360={ramping_up}");
    assert!(
        held > 200,
        "a constant 360 shutter retains the white seed in full; got {held}"
    );
    assert!(
        ramping_up < 64,
        "a shutter still near 0 one frame in retains almost none of it; got {ramping_up}"
    );
}

#[test]
fn a_cached_effect_graph_should_survive_a_different_stack_at_the_same_position() {
    // What this pins is that a composite of a *different shape* no longer throws the
    // cache away. It used to: the cache was a `Vec` resized per composite, and any
    // change in the layer count cleared every entry. On the multi-track export path
    // that happens twice per output frame, because it alternates a one-layer solo
    // composite with an N-layer stack (#1770).
    //
    // The motion-blur trail is the observable: it lives in the cached graph, so an
    // eviction shows up as a frame that lost its trail.
    let (w, h) = (8, 8);
    let white = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [255, 255, 255])).unwrap();
    let black = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [0, 0, 0])).unwrap();
    let blurred = constant_motion_blur_layer(w, h, 360.0);
    let plain = base_layer(w, h, Vec::new());
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };

    // One layer: seed the trail with white.
    gpu.composite(&[(&blurred, &white)], (w, h), Duration::ZERO)
        .expect("seed the one-layer entry");
    // Two layers: a different stack, so a different cache entry. Position 0 is the
    // same, which is exactly the collision the layout key exists to prevent.
    gpu.composite(
        &[(&blurred, &black), (&plain, &black)],
        (w, h),
        Duration::from_millis(33),
    )
    .expect("a two-layer stack at the same position");
    // One layer again: the seeded trail must still be there.
    let (out, _, _) = gpu
        .composite(&[(&blurred, &black)], (w, h), Duration::from_millis(66))
        .expect("back to one layer");

    println!("one-layer trail after a two-layer composite: {}", out[0]);
    assert!(
        out[0] > 8,
        "the one-layer graph must survive a composite of a different shape; a lost          trail reads as an unblended frame, and this one read {}",
        out[0]
    );
}

// Transition mapping parity (#1657)

/// Mean per-channel difference allowed between a GPU transition node and the CPU
/// reference the preview runs (`ff_preview::apply_xfade`).
///
/// Measured **0.000 (max 0)** here for all eight kinds at progress 0 / 0.5 / 1: the
/// wipes and the dissolve are hard per-pixel picks, and the fade and dip lerps round
/// the same way on both sides. The margin covers a driver that rounds a lerp the other
/// way (±1 per channel) and nothing more.
///
/// Deliberately not slack. A wholesale divergence — a dissolve hash that reveals a
/// different *set* of pixels, or a mirrored wipe — lands near 127 and any bound would
/// catch it; what a loose bound would hide is a *partial* regression, such as the dip's
/// phase split shifting by a frame. That is the failure this suite exists to reject.
const TOL_TRANSITION_MEAN: f64 = 2.0;

/// A GPU context for the transition nodes, or `None` without an adapter.
fn transition_ctx() -> Option<std::sync::Arc<RenderContext>> {
    RenderContext::init_blocking().ok().map(std::sync::Arc::new)
}

/// Runs `node` over clip A on the GPU and reads the result back.
fn run_transition_gpu(
    ctx: &std::sync::Arc<RenderContext>,
    node: impl RenderNode + 'static,
    a: &[u8],
    w: u32,
    h: u32,
) -> Vec<u8> {
    RenderGraph::new(std::sync::Arc::clone(ctx))
        .push(node)
        .process_gpu(a, w, h)
        .expect("process_gpu must succeed on a graph with a GPU context")
}

/// Runs the GPU node `kind` maps to, or `None` when it is a CPU-only kind.
fn gpu_transition_output(
    ctx: &std::sync::Arc<RenderContext>,
    kind: XfadeTransition,
    a: &[u8],
    b: &[u8],
    progress: f32,
    w: u32,
    h: u32,
) -> Option<Vec<u8>> {
    let out = match map_transition(kind)? {
        GpuTransition::Fade => run_transition_gpu(
            ctx,
            FadeTransitionNode::new(progress, b.to_vec(), w, h),
            a,
            w,
            h,
        ),
        // The mask comes from the same function the CPU reference uses, which is the
        // point: `FFmpeg`'s dissolve noise is not reproducible in `WGSL` (its argument
        // outgrows `f32` well before 1080p), so both paths read one CPU-built selection
        // instead of two implementations hoping to agree (#1732).
        GpuTransition::Dissolve => run_transition_gpu(
            ctx,
            DissolveTransitionNode::new(ff_filter::dissolve_mask(w, h, progress), b.to_vec(), w, h),
            a,
            w,
            h,
        ),
        // Zero softness: the CPU reference has a hard edge, so a feathered GPU edge
        // would diverge along the seam for reasons that have nothing to do with the
        // mapping under test.
        GpuTransition::Wipe { angle } => run_transition_gpu(
            ctx,
            WipeTransitionNode::new(progress, 0.0, angle, b.to_vec(), w, h),
            a,
            w,
            h,
        ),
        GpuTransition::Dip { color } => run_transition_gpu(
            ctx,
            DipToColorNode::new(progress, color, b.to_vec(), w, h),
            a,
            w,
            h,
        ),
    };
    Some(out)
}

/// The CPU reference the preview actually runs for `kind`.
fn cpu_transition_output(
    kind: XfadeTransition,
    a: &[u8],
    b: &[u8],
    progress: f32,
    w: u32,
    h: u32,
) -> Vec<u8> {
    let mut dst = Vec::new();
    ff_preview::apply_xfade(kind, a, b, progress, (w, h), None, &mut dst);
    dst
}

#[test]
fn every_mapped_transition_should_match_the_cpu_reference() {
    // AC2: each mapped kind, against the CPU path the GPU node replaces. Progress 0.5
    // plus both endpoints — 0 and 1 are what pin direction, since several kinds look
    // alike at the midpoint (RK-015).
    //
    // For `dissolve` this is also the check that `dissolve.wgsl`'s hash matches the Rust
    // one bit for bit: a different hash reveals the same *proportion* of clip B while
    // choosing a different *set* of pixels, so only a per-pixel comparison sees it.
    let Some(ctx) = transition_ctx() else {
        return; // no adapter
    };
    let (w, h) = (32u32, 32u32);
    let a = gradient_rgba(w, h);
    let b = checkerboard_rgba(w, h, 4);

    for kind in [
        XfadeTransition::Fade,
        XfadeTransition::Dissolve,
        XfadeTransition::WipeLeft,
        XfadeTransition::WipeRight,
        XfadeTransition::WipeUp,
        XfadeTransition::WipeDown,
        XfadeTransition::FadeBlack,
        XfadeTransition::FadeWhite,
    ] {
        for progress in [0.0f32, 0.5, 1.0] {
            let gpu = gpu_transition_output(&ctx, kind, &a, &b, progress, w, h)
                .unwrap_or_else(|| panic!("{kind:?} must map to a GPU node"));
            let cpu = cpu_transition_output(kind, &a, &b, progress, w, h);
            assert_eq!(gpu.len(), cpu.len(), "{kind:?} @{progress}: size mismatch");
            let mean = mean_abs_diff_rgb(&gpu, &cpu);
            println!(
                "{kind:?} @{progress}: mean={mean:.3} max={}",
                max_abs_diff_rgb(&gpu, &cpu)
            );
            assert!(
                mean <= TOL_TRANSITION_MEAN,
                "{kind:?} @{progress}: GPU and CPU diverged, mean={mean}"
            );
        }
    }
}

#[test]
fn cpu_only_transitions_should_not_map_to_a_gpu_node() {
    // The other half of AC2, and the reason the suite above can be strict: a kind with
    // no faithful node returns `None` instead of an approximation.
    for kind in [
        XfadeTransition::SlideLeft,
        XfadeTransition::SlideRight,
        XfadeTransition::SlideUp,
        XfadeTransition::SlideDown,
        XfadeTransition::CircleOpen,
        XfadeTransition::CircleClose,
        XfadeTransition::FadeGrays,
        XfadeTransition::Pixelize,
    ] {
        assert!(
            map_transition(kind).is_none(),
            "{kind:?} must stay on the CPU path"
        );
    }
}

#[test]
fn gpu_compositor_should_fall_back_not_panic_for_unsupported_inputs() {
    // Every unsupported input must return None (CPU fallback), never panic (AC2).
    // Adapter-gated (the gate lives past GpuCompositor::new).
    let (w, h) = (64, 48);
    let frame = VideoFrame::from_rgba(w, h, gradient_rgba(w, h)).unwrap();
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };

    // A positioned layer and a mixed-aspect stack both used to be listed here. #1633
    // removed those fallbacks rather than fixing them, because neither was rendering
    // anything wrong: a lone layer is the *base*, whose transform the CPU ignores, and
    // an overlay is stretched to the base size, so its own aspect never survives to
    // disagree with the base band. They are covered now by
    // `a_positioned_single_layer_should_composite_on_the_gpu_unmoved` and
    // `overlay_over_a_letterboxed_base_gpu_should_match_cpu_within_tolerance`, which
    // assert the placement against the CPU instead of asserting a refusal.
    //
    // A **rotated overlay** is still genuinely unmappable, so it takes their place here.
    let over = flat_frame(w, h, 255);
    let bottom = base_layer(w, h, vec![]);
    let mut spun = base_layer(w, h, vec![]);
    spun.rotation = AnimatedValue::Static(30.0);
    assert!(
        gpu.composite(&[(&bottom, &frame), (&spun, &over)], (w, h), Duration::ZERO)
            .is_none(),
        "a rotated overlay must fall back"
    );

    // An aspect so extreme that the fitted height rounds away to nothing (#1661): a
    // 64x1 frame fits to 64x0 in a square canvas. A zero scale does not fail in
    // `transform.wgsl` — it floors the divisor at 1e-4 and samples the layer entirely
    // out of range — so without the guard this would render a silently black frame
    // rather than fall back (RK-020).
    let sliver = VideoFrame::from_rgba(w, 1, gradient_rgba(w, 1)).unwrap();
    let sliver_layer = base_layer(w, 1, vec![]);
    assert!(
        gpu_composite(&mut gpu, &sliver_layer, &sliver, (w, w)).is_none(),
        "an aspect whose fit rounds away to nothing must fall back"
    );

    // No blend-mode case here: since #1669 every `ff_filter::BlendMode` maps to a
    // GPU node, so none can trigger a fallback. `map_blend_mode`'s remaining
    // `None` arm covers a mode a future FFmpeg adds, and
    // `map_scene_should_map_every_blend_mode` pins that all 40 map today.

    // Unsupported effect (no GPU node). `Hue` stood here until #1630 mapped it onto
    // `HslNode`; `XFade` needs the two-input `CrossfadeNode`, which the per-layer
    // plan has no second input for, so it is the current example of a step with no
    // mapping at all.
    let xfade = base_layer(
        w,
        h,
        vec![FilterStep::XFade {
            transition: XfadeTransition::Fade,
            duration: 1.0,
            offset: 0.0,
        }],
    );
    assert!(
        gpu_composite(&mut gpu, &xfade, &frame, (w, h)).is_none(),
        "an unsupported effect must fall back"
    );
}

#[test]
#[ignore = "requires the color filter + a GPU adapter; run with -- --include-ignored"]
fn preview_runner_should_fall_back_and_advance_on_unsupported_clip() {
    // A positioned clip makes the GPU compositor return None every frame, so the
    // runner falls back to its CPU compositor. It must keep advancing and terminate
    // (never hang) — the RK-019 hang guard, now over the GPU-attached runner. Skips
    // without an adapter or filters.
    if GpuCompositor::new().is_none() {
        println!("skipping: no GPU adapter");
        return;
    }

    struct PtsSink {
        pts: Arc<Mutex<Vec<Duration>>>,
        handle: PlayerHandle,
    }
    impl FrameSink for PtsSink {
        fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, pts: Duration) {
            let mut log = self
                .pts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log.push(pts);
            if log.len() >= 12 {
                self.handle.stop();
            }
        }
    }

    let timeline = Timeline::builder()
        .canvas(64, 48)
        .frame_rate(30.0)
        .video_track(vec![
            Clip::solid(Color::rgb(20, 120, 200))
                .trim(Duration::ZERO, Duration::from_secs(1))
                .with_position(10.0, 0.0),
        ])
        .build()
        .expect("timeline build failed");

    let (mut runner, handle) = match TimelinePlayer::open(&timeline) {
        Ok(p) => p,
        Err(e) => {
            println!("skipping: open failed (filters unavailable?): {e}");
            return;
        }
    };
    let pts = Arc::new(Mutex::new(Vec::<Duration>::new()));
    runner.set_sink(Box::new(PtsSink {
        pts: Arc::clone(&pts),
        handle: handle.clone(),
    }));
    // If this returns at all, the GPU-fallback runner did not hang.
    let _ = runner.run();

    let pts = pts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if pts.is_empty() {
        println!("skipping: color filter unavailable (no frames rendered)");
        return;
    }
    assert!(
        pts.last() > pts.first(),
        "GPU-fallback playback PTS must advance: {:?}",
        &pts[..pts.len().min(6)]
    );
}

/// A cached effect graph must keep following the frame, not a mask baked from the
/// frame that built it (#1710).
///
/// `LumaMask` used to be excluded from the per-layer graph cache because its node
/// carried a mask buffer built from one frame's pixels. The shader reads the source
/// frame now, so the exclusion is gone -- and this is what stands behind that: two
/// different frames go through the *same* `GpuCompositor`, so the second one runs on
/// the graph the first one built. A node that baked again would mask frame 2 by
/// frame 1's luma, which is exactly the pattern the two halves below detect.
///
/// Composited over an opaque background so the keyed alpha becomes an RGB difference
/// (RK-024): a mask writes alpha only, and the blend shader outputs the canvas alpha.
/// Adapter-gated; no CPU filters needed, since the reference is the *other* frame.
#[test]
fn a_cached_luma_mask_graph_should_follow_the_frame() {
    let (w, h) = (64, 48);
    let bg = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [30, 60, 200])).unwrap(); // blue
    // Two frames with opposite bright halves.
    let split = |left_bright: bool| {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _y in 0..h {
            for x in 0..w {
                let bright = (x < w / 2) == left_bright;
                let c = if bright { 255 } else { 0 };
                v.extend_from_slice(&[c, c, c, 255]);
            }
        }
        VideoFrame::from_rgba(w, h, v).unwrap()
    };
    let (first, second) = (split(true), split(false));
    let bg_layer = base_layer(w, h, vec![]);
    let fg_layer = base_layer(w, h, vec![FilterStep::LumaMask { invert: false }]);
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };

    let mut composite = |fg: &VideoFrame| {
        gpu.composite(&[(&bg_layer, &bg), (&fg_layer, fg)], (w, h), Duration::ZERO)
            .map(|(out, _, _)| out)
    };
    let Some(out1) = composite(&first) else {
        panic!("a supported luma-mask composite must render on the GPU");
    };
    let Some(out2) = composite(&second) else {
        panic!("the second frame must render on the same GPU path");
    };

    let (l1, r1) = (
        region_mean_rgb(&out1, w, true),
        region_mean_rgb(&out1, w, false),
    );
    let (l2, r2) = (
        region_mean_rgb(&out2, w, true),
        region_mean_rgb(&out2, w, false),
    );
    println!("cached luma mask: frame1 l={l1:?} r={r1:?} | frame2 l={l2:?} r={r2:?}");

    // Frame 1: bright left stays opaque white, dark right reveals the blue background.
    assert!(
        l1[0] > 192.0 && l1[2] > 192.0,
        "frame 1's bright half must stay white; got {l1:?}"
    );
    assert!(
        r1[2] > 128.0 && r1[0] < 128.0,
        "frame 1's dark half must reveal the background; got {r1:?}"
    );
    // Frame 2 is the mirror. A baked mask would keep masking by frame 1's luma, so the
    // left half would show frame 2's own black pixels (blue near zero) instead of the
    // background, and the right half would be hidden rather than white.
    assert!(
        l2[2] > 128.0 && l2[0] < 128.0,
        "frame 2's dark half must reveal the background, not a stale mask; got {l2:?}"
    );
    assert!(
        r2[0] > 192.0 && r2[2] > 192.0,
        "frame 2's bright half must stay white; got {r2:?}"
    );
}

/// An animated rectangle must move through a cached graph (#1710).
///
/// The rectangle reaches the shader as a uniform, so a changed rectangle is a
/// parameter pushed into the live node rather than a rebuilt graph
/// (`param_update` covers that half deterministically). This is the rendered half:
/// the same compositor at two times must keep two different regions.
#[test]
fn an_animated_rectangle_should_move_through_a_cached_graph() {
    let (w, h) = (64, 48);
    let quarter = w / 4;
    let span = Duration::from_secs(1);
    let bg = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [30, 60, 200])).unwrap(); // blue
    let fg = VideoFrame::from_rgba(w, h, solid_rgba(w, h, [220, 40, 40])).unwrap(); // red
    let track = |from: f64, to: f64| {
        AnimatedValue::Track(
            AnimationTrack::new()
                .push(Keyframe::new(Duration::ZERO, from, Easing::Linear))
                .push(Keyframe::new(span, to, Easing::Linear)),
        )
    };
    let bg_layer = base_layer(w, h, vec![]);
    // A quarter-width box sliding from the left quarter to the right quarter.
    let fg_layer = base_layer(
        w,
        h,
        vec![FilterStep::RectMaskAnimated {
            x: track(0.0, f64::from(3 * quarter)),
            y: AnimatedValue::Static(0.0),
            width: AnimatedValue::Static(f64::from(quarter)),
            height: AnimatedValue::Static(f64::from(h)),
            invert: false,
        }],
    );
    let Some(mut gpu) = GpuCompositor::new() else {
        return; // no adapter
    };

    let mut at = |t: Duration| {
        gpu.composite(&[(&bg_layer, &bg), (&fg_layer, &fg)], (w, h), t)
            .map(|(out, _, _)| out)
    };
    let Some(start) = at(Duration::ZERO) else {
        panic!("a supported animated rect-mask composite must render on the GPU");
    };
    let Some(end) = at(span) else {
        panic!("the second time must render on the same GPU path");
    };

    let left_at_start = band_mean_rgb(&start, w, 0, quarter);
    let right_at_start = band_mean_rgb(&start, w, 3 * quarter, w);
    let left_at_end = band_mean_rgb(&end, w, 0, quarter);
    let right_at_end = band_mean_rgb(&end, w, 3 * quarter, w);
    println!(
        "animated rect: t=0 l={left_at_start:?} r={right_at_start:?} | t=1s l={left_at_end:?} r={right_at_end:?}"
    );

    let red = |c: [f64; 3]| c[0] > 128.0 && c[2] < 128.0;
    let blue = |c: [f64; 3]| c[2] > 128.0 && c[0] < 128.0;
    assert!(
        red(left_at_start) && blue(right_at_start),
        "at t=0 the box keeps the left quarter; got l={left_at_start:?} r={right_at_start:?}"
    );
    assert!(
        blue(left_at_end) && red(right_at_end),
        "at t=1s the box must have moved to the right quarter; got l={left_at_end:?} r={right_at_end:?}"
    );
}
