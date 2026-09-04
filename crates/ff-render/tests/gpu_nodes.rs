//! GPU-path integration tests for the `ff-render` nodes.
//!
//! Unlike `render_graph_tests.rs` (which covers only the CPU fallback
//! `process_cpu`), these exercise the real wgpu `process()` path end to end:
//! upload the input frame to a GPU texture, run the node on a real device via
//! [`RenderGraph::process_gpu`], read the output back, and assert the expected
//! pixel semantics within a tolerance that absorbs GPU/driver variance.
//!
//! Each assertion mirrors the expectation the corresponding CPU test encodes;
//! they are independent expectations, not a GPU-vs-CPU parity check. The tests
//! skip gracefully (no failure, no panic) when no GPU adapter is available.
//!
//! Requires the `wgpu` feature (see `[[test]] required-features` in Cargo.toml).

use std::sync::Arc;

use ff_render::{
    AlphaMatteNode, BlendMode, BlendModeNode, ChromaKeyNode, ColorGradeNode, CrossfadeNode,
    DissolveTransitionNode, FadeTransitionNode, LumaMaskNode, OverlayNode, RenderContext,
    RenderGraph, RenderNode, RenderNodeCpu, ScaleAlgorithm, ScaleNode, ShapeMaskNode,
    TransformNode, YuvFormat, YuvUploadNode,
};

// Helpers

/// A `w × h` buffer filled with a single RGBA colour.
fn solid_rgba(r: u8, g: u8, b: u8, a: u8, w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        v.extend_from_slice(&[r, g, b, a]);
    }
    v
}

/// A shared GPU context, or `None` when no adapter is available (CI without a
/// GPU). Every test starts with `let Some(ctx) = gpu_ctx() else { return; };`.
fn gpu_ctx() -> Option<Arc<RenderContext>> {
    match futures::executor::block_on(RenderContext::init()) {
        Ok(ctx) => Some(Arc::new(ctx)),
        Err(e) => {
            println!("skipping GPU test: no adapter ({e})");
            None
        }
    }
}

/// Upload `rgba`, run `node` on the GPU, and read the result back as RGBA.
fn run_gpu(
    ctx: &Arc<RenderContext>,
    node: impl RenderNode + 'static,
    rgba: &[u8],
    w: u32,
    h: u32,
) -> Vec<u8> {
    RenderGraph::new(Arc::clone(ctx))
        .push(node)
        .process_gpu(rgba, w, h)
        .expect("process_gpu must succeed on a graph with a GPU context")
}

/// `|a - b| <= tol` for two `u8` channel values.
fn close(a: u8, b: u8, tol: i32) -> bool {
    (i32::from(a) - i32::from(b)).abs() <= tol
}

// ColorGradeNode

#[test]
fn color_grade_gpu_brightness_boost_should_increase_rgb_channels() {
    let Some(ctx) = gpu_ctx() else { return };
    let rgba = solid_rgba(100, 100, 100, 255, 4, 4);
    let out = run_gpu(
        &ctx,
        ColorGradeNode::new(0.3, 1.0, 1.0, 0.0, 0.0),
        &rgba,
        4,
        4,
    );
    assert!(out[0] > 100, "brightness +0.3 must raise R; got {}", out[0]);
    assert!(out[1] > 100, "brightness +0.3 must raise G; got {}", out[1]);
    assert!(out[2] > 100, "brightness +0.3 must raise B; got {}", out[2]);
    assert_eq!(out[3], 255, "alpha must be unchanged");
}

#[test]
fn color_grade_gpu_saturation_zero_should_produce_equal_rgb_channels() {
    let Some(ctx) = gpu_ctx() else { return };
    // saturation is the 3rd arg: new(brightness, contrast, saturation, temp, tint).
    let rgba = solid_rgba(200, 100, 50, 255, 4, 4);
    let out = run_gpu(
        &ctx,
        ColorGradeNode::new(0.0, 1.0, 0.0, 0.0, 0.0),
        &rgba,
        4,
        4,
    );
    assert!(
        close(out[0], out[1], 3) && close(out[1], out[2], 3),
        "saturation=0 must greyscale R=G=B; got {} {} {}",
        out[0],
        out[1],
        out[2]
    );
}

// ScaleNode
//
// `run_gpu` allocates the output texture at the input dimensions, so this
// checks colour preservation through the scale sampler, not a size change.

#[test]
fn scale_gpu_solid_input_should_preserve_color() {
    let Some(ctx) = gpu_ctx() else { return };
    let rgba = solid_rgba(128, 64, 32, 255, 4, 4);
    let out = run_gpu(
        &ctx,
        ScaleNode::new(2, 2, ScaleAlgorithm::Bilinear),
        &rgba,
        4,
        4,
    );
    assert!(
        close(out[0], 128, 4) && close(out[1], 64, 4) && close(out[2], 32, 4),
        "scaling a solid colour must preserve it; got {} {} {}",
        out[0],
        out[1],
        out[2]
    );
}

// TransformNode

#[test]
fn transform_gpu_identity_should_return_input() {
    let Some(ctx) = gpu_ctx() else { return };
    let rgba = solid_rgba(77, 88, 99, 255, 4, 4);
    let out = run_gpu(
        &ctx,
        TransformNode::new([0.0, 0.0], 0.0, [1.0, 1.0]),
        &rgba,
        4,
        4,
    );
    assert!(
        close(out[0], 77, 3) && close(out[1], 88, 3) && close(out[2], 99, 3),
        "identity transform must reproduce the input; got {} {} {}",
        out[0],
        out[1],
        out[2]
    );
}

// OverlayNode

#[test]
fn overlay_gpu_opaque_overlay_should_replace_base_color() {
    let Some(ctx) = gpu_ctx() else { return };
    let base = solid_rgba(0, 0, 0, 255, 4, 4);
    let overlay = solid_rgba(200, 100, 50, 255, 4, 4);
    let out = run_gpu(&ctx, OverlayNode::new(overlay, 4, 4), &base, 4, 4);
    assert!(
        out[0] >= 190,
        "opaque overlay must dominate base R; got {}",
        out[0]
    );
}

// CrossfadeNode

#[test]
fn crossfade_gpu_half_factor_should_average_inputs() {
    let Some(ctx) = gpu_ctx() else { return };
    let from = solid_rgba(0, 0, 0, 255, 2, 2);
    let to = solid_rgba(200, 200, 200, 255, 2, 2);
    let out = run_gpu(&ctx, CrossfadeNode::new(0.5, to, 2, 2), &from, 2, 2);
    assert!(
        close(out[0], 100, 8),
        "factor=0.5 must blend R to ~100; got {}",
        out[0]
    );
}

// FadeTransitionNode

/// Clip A and clip B for the transition tests. Every channel differs between the two and
/// none repeats within a frame, so a swapped pair, a dropped channel or a transposed one
/// all show up. Mirrors the constants the CPU tests in `nodes/transition.rs` use.
const FADE_A: [u8; 4] = [10, 200, 30, 255];
const FADE_B: [u8; 4] = [210, 40, 130, 55];

/// A 2x2 frame filled with one tagged colour.
fn tagged(px: [u8; 4]) -> Vec<u8> {
    solid_rgba(px[0], px[1], px[2], px[3], 2, 2)
}

#[test]
fn fade_transition_gpu_half_progress_should_average_inputs() {
    let Some(ctx) = gpu_ctx() else { return };
    let a = tagged(FADE_A);
    let b = tagged(FADE_B);
    let out = run_gpu(&ctx, FadeTransitionNode::new(0.5, b, 2, 2), &a, 2, 2);
    for c in 0..4 {
        let want = u8::midpoint(FADE_A[c], FADE_B[c]);
        assert!(
            close(out[c], want, 8),
            "channel {c}: progress=0.5 must blend to ~{want}; got {}",
            out[c]
        );
    }
}

#[test]
fn fade_transition_gpu_progress_endpoints_should_be_each_clip() {
    // The 0.5 blend above is symmetric, so it cannot tell clip A from clip B. These
    // endpoints are what pin the direction on the GPU path.
    let Some(ctx) = gpu_ctx() else { return };
    let a = tagged(FADE_A);
    let b = tagged(FADE_B);
    let at_zero = run_gpu(
        &ctx,
        FadeTransitionNode::new(0.0, b.clone(), 2, 2),
        &a,
        2,
        2,
    );
    let at_one = run_gpu(&ctx, FadeTransitionNode::new(1.0, b, 2, 2), &a, 2, 2);
    for c in 0..4 {
        assert!(
            close(at_zero[c], FADE_A[c], 2),
            "channel {c}: progress=0 must be clip A; got {}",
            at_zero[c]
        );
        assert!(
            close(at_one[c], FADE_B[c], 2),
            "channel {c}: progress=1 must be clip B; got {}",
            at_one[c]
        );
    }
}

// DissolveTransitionNode

/// A mask revealing every `nth` pixel, as an irregular stand-in for a real selection.
///
/// The node takes its selection as an input rather than deriving one, so these tests
/// supply their own; `avio`'s `xfade_reference_parity` is what pins the real mask
/// against `FFmpeg`.
fn nth_mask(w: u32, h: u32, nth: usize) -> Vec<u8> {
    let n = (w * h) as usize;
    let mut mask = vec![0u8; n * 4];
    for i in 0..n {
        if i % nth == 0 {
            mask[i * 4..i * 4 + 4].fill(255);
        }
    }
    mask
}

#[test]
fn dissolve_gpu_should_threshold_pixels_not_blend_them() {
    // The property that separates a dissolve from a fade, asserted on the GPU path in
    // its own right (this file states independent expectations, not GPU-vs-CPU parity).
    // Black into white makes a mixed value unmistakable.
    let Some(ctx) = gpu_ctx() else { return };
    let (w, h) = (32u32, 32u32);
    let a = solid_rgba(0, 0, 0, 255, w, h);
    let b = solid_rgba(255, 255, 255, 255, w, h);
    let out = run_gpu(
        &ctx,
        DissolveTransitionNode::new(nth_mask(w, h, 2), b, w, h),
        &a,
        w,
        h,
    );

    let reds: Vec<u8> = out.chunks_exact(4).map(|px| px[0]).collect();
    let mixed = reds.iter().filter(|v| (8..=247).contains(*v)).count();
    let revealed = reds.iter().filter(|v| **v > 247).count();
    println!(
        "dissolve GPU @50%: revealed={revealed}/{} mixed={mixed}",
        reds.len()
    );
    assert_eq!(
        mixed, 0,
        "a dissolve must never produce a mixed value; {mixed} pixels were between"
    );
    let ratio = revealed as f64 / reds.len() as f64;
    assert!(
        (0.35..=0.65).contains(&ratio),
        "an every-other-pixel mask should reveal about half of clip B, got {ratio:.3}"
    );
}

#[test]
fn dissolve_gpu_mask_endpoints_should_be_each_clip() {
    // The half case above says nothing about which way the mask reads; these pin it.
    let Some(ctx) = gpu_ctx() else { return };
    let (w, h) = (16u32, 16u32);
    let n = (w * h) as usize;
    let a = solid_rgba(0, 0, 0, 255, w, h);
    let b = solid_rgba(255, 255, 255, 255, w, h);
    let at_zero = run_gpu(
        &ctx,
        DissolveTransitionNode::new(vec![0u8; n * 4], b.clone(), w, h),
        &a,
        w,
        h,
    );
    let at_one = run_gpu(
        &ctx,
        DissolveTransitionNode::new(vec![255u8; n * 4], b, w, h),
        &a,
        w,
        h,
    );
    assert!(
        at_zero.chunks_exact(4).all(|px| px[0] < 8),
        "an unset mask must be entirely clip A"
    );
    assert!(
        at_one.chunks_exact(4).all(|px| px[0] > 247),
        "a set mask must be entirely clip B"
    );
}

// BlendModeNode

#[test]
fn blend_gpu_multiply_should_darken_base() {
    let Some(ctx) = gpu_ctx() else { return };
    let base = solid_rgba(128, 128, 128, 255, 2, 2);
    let overlay = solid_rgba(128, 128, 128, 255, 2, 2);
    let out = run_gpu(
        &ctx,
        BlendModeNode::new(BlendMode::Multiply, 1.0, overlay, 2, 2),
        &base,
        2,
        2,
    );
    assert!(out[0] < 128, "Multiply must darken base R; got {}", out[0]);
}

#[test]
fn blend_gpu_screen_should_lighten_base() {
    let Some(ctx) = gpu_ctx() else { return };
    let base = solid_rgba(100, 100, 100, 255, 2, 2);
    let overlay = solid_rgba(100, 100, 100, 255, 2, 2);
    let out = run_gpu(
        &ctx,
        BlendModeNode::new(BlendMode::Screen, 1.0, overlay, 2, 2),
        &base,
        2,
        2,
    );
    assert!(out[0] > 100, "Screen must lighten base R; got {}", out[0]);
}

#[test]
fn blend_gpu_normal_at_zero_opacity_should_leave_base_unchanged() {
    let Some(ctx) = gpu_ctx() else { return };
    let base = solid_rgba(200, 100, 50, 255, 2, 2);
    let overlay = solid_rgba(0, 0, 0, 255, 2, 2);
    let out = run_gpu(
        &ctx,
        BlendModeNode::new(BlendMode::Normal, 0.0, overlay, 2, 2),
        &base,
        2,
        2,
    );
    assert!(
        close(out[0], 200, 3) && close(out[1], 100, 3) && close(out[2], 50, 3),
        "opacity=0 must leave the base unchanged; got {} {} {}",
        out[0],
        out[1],
        out[2]
    );
}

/// Every blend mode, shader against the Rust implementation.
///
/// `blend_math.rs`'s table pins the Rust to `FFmpeg`'s formulas; this pins the
/// shader to the Rust, so the two together tie the GPU output to `FFmpeg` (#1669).
///
/// The colour pairs are the same ones that table uses. They are **coloured, not
/// grey** (RK-022), and every channel sits at least 16 LSB away from 0, 128 and
/// 255: `HardMix`, `PinLight`, `VividLight`, `HardOverlay` and `SoftDifference`
/// are discontinuous at those points, and a one-LSB sampling difference there
/// would flip the branch and produce a large, flaky delta. Across the nine
/// (base, overlay) channel combinations both sides of every `< 0.5` branch are
/// exercised, so no mode's case is vacuous (RK-015).
///
/// A 3x1 texture puts every fragment centre exactly on a texel centre, so the
/// linear sampler returns exact texels and no interpolation error enters the
/// comparison.
#[test]
fn blend_gpu_should_match_the_cpu_path_for_every_mode() {
    let Some(ctx) = gpu_ctx() else { return };

    let base: Vec<u8> = [
        [32u8, 96, 200, 255],
        [208, 24, 144, 255],
        [144, 176, 64, 255],
    ]
    .concat();
    let overlay: Vec<u8> = [
        [176u8, 64, 24, 255],
        [48, 232, 112, 255],
        [96, 32, 208, 255],
    ]
    .concat();

    for mode in ALL_BLEND_MODES {
        let node = BlendModeNode::new(mode, 1.0, overlay.clone(), 3, 1);
        let mut cpu = base.clone();
        node.process_cpu(&mut cpu, 3, 1);
        let gpu = run_gpu(&ctx, node, &base, 3, 1);

        for px in 0..3 {
            for ch in 0..3 {
                let i = px * 4 + ch;
                assert!(
                    close(gpu[i], cpu[i], 2),
                    "{mode:?} pixel {px} channel {ch}: GPU {} vs CPU {}",
                    gpu[i],
                    cpu[i]
                );
            }
        }
    }
}

/// Kept in the same order as the `BlendMode` discriminants; the enum's own
/// `blend_mode_discriminants_should_match_the_shader_mode_codes` pins those.
const ALL_BLEND_MODES: [BlendMode; 44] = [
    BlendMode::Normal,
    BlendMode::Multiply,
    BlendMode::Screen,
    BlendMode::Overlay,
    BlendMode::SoftLight,
    BlendMode::HardLight,
    BlendMode::ColorDodge,
    BlendMode::ColorBurn,
    BlendMode::Difference,
    BlendMode::Exclusion,
    BlendMode::Add,
    BlendMode::Subtract,
    BlendMode::Darken,
    BlendMode::Lighten,
    BlendMode::Hue,
    BlendMode::Saturation,
    BlendMode::Color,
    BlendMode::Luminosity,
    BlendMode::And,
    BlendMode::Average,
    BlendMode::Bleach,
    BlendMode::Divide,
    BlendMode::Extremity,
    BlendMode::Freeze,
    BlendMode::Geometric,
    BlendMode::Glow,
    BlendMode::GrainExtract,
    BlendMode::GrainMerge,
    BlendMode::HardMix,
    BlendMode::HardOverlay,
    BlendMode::Harmonic,
    BlendMode::Heat,
    BlendMode::Interpolate,
    BlendMode::LinearLight,
    BlendMode::Multiply128,
    BlendMode::Negation,
    BlendMode::Or,
    BlendMode::Phoenix,
    BlendMode::PinLight,
    BlendMode::Reflect,
    BlendMode::SoftDifference,
    BlendMode::Stain,
    BlendMode::VividLight,
    BlendMode::Xor,
];

// LumaMaskNode

#[test]
fn luma_mask_gpu_white_mask_should_preserve_alpha() {
    let Some(ctx) = gpu_ctx() else { return };
    let rgba = solid_rgba(128, 64, 32, 200, 2, 2);
    let mask = solid_rgba(255, 255, 255, 255, 2, 2);
    let out = run_gpu(&ctx, LumaMaskNode::new(mask, 2, 2), &rgba, 2, 2);
    assert!(
        close(out[3], 200, 4),
        "white luma mask must keep alpha ~200; got {}",
        out[3]
    );
}

#[test]
fn luma_mask_gpu_black_mask_should_zero_alpha() {
    let Some(ctx) = gpu_ctx() else { return };
    let rgba = solid_rgba(128, 64, 32, 255, 2, 2);
    let mask = solid_rgba(0, 0, 0, 255, 2, 2);
    let out = run_gpu(&ctx, LumaMaskNode::new(mask, 2, 2), &rgba, 2, 2);
    assert!(
        close(out[3], 0, 4),
        "black luma mask must zero alpha; got {}",
        out[3]
    );
}

// ShapeMaskNode

#[test]
fn shape_mask_gpu_opaque_mask_should_preserve_alpha() {
    let Some(ctx) = gpu_ctx() else { return };
    let rgba = solid_rgba(128, 64, 32, 200, 2, 2);
    let mask = solid_rgba(255, 255, 255, 255, 2, 2);
    let out = run_gpu(&ctx, ShapeMaskNode::new(mask, 2, 2), &rgba, 2, 2);
    assert!(
        close(out[3], 200, 4),
        "opaque shape mask must keep alpha ~200; got {}",
        out[3]
    );
}

#[test]
fn shape_mask_gpu_transparent_mask_should_zero_alpha() {
    let Some(ctx) = gpu_ctx() else { return };
    let rgba = solid_rgba(128, 64, 32, 255, 2, 2);
    let mask = solid_rgba(0, 0, 0, 0, 2, 2);
    let out = run_gpu(&ctx, ShapeMaskNode::new(mask, 2, 2), &rgba, 2, 2);
    assert!(
        close(out[3], 0, 4),
        "transparent shape mask must zero alpha; got {}",
        out[3]
    );
}

// AlphaMatteNode

#[test]
fn alpha_matte_gpu_transparent_fg_should_reveal_background() {
    let Some(ctx) = gpu_ctx() else { return };
    let fg = solid_rgba(255, 0, 0, 0, 2, 2); // fully transparent red
    let bg = solid_rgba(0, 0, 255, 255, 2, 2); // opaque blue
    let out = run_gpu(&ctx, AlphaMatteNode::new(bg, 2, 2), &fg, 2, 2);
    assert!(
        out[2] > 200,
        "transparent fg must show the blue background; got B={}",
        out[2]
    );
}

// ChromaKeyNode

#[test]
fn chroma_key_gpu_pure_key_color_should_become_transparent() {
    let Some(ctx) = gpu_ctx() else { return };
    let rgba = solid_rgba(0, 255, 0, 255, 2, 2); // pure green
    let out = run_gpu(
        &ctx,
        ChromaKeyNode::new([0.0, 1.0, 0.0], 0.3, 0.0),
        &rgba,
        2,
        2,
    );
    assert!(
        close(out[3], 0, 4),
        "pure key colour must key out (alpha ~0); got {}",
        out[3]
    );
}

// YuvUploadNode

#[test]
fn yuv_upload_gpu_black_frame_should_produce_near_black_rgba() {
    let Some(ctx) = gpu_ctx() else { return };
    let mut node = YuvUploadNode::new(YuvFormat::Yuv420p, 4, 4);
    // BT.601 black: Y=16, Cb=Cr=128.
    node.set_planes(vec![16u8; 4 * 4], vec![128u8; 2 * 2], vec![128u8; 2 * 2]);
    let dummy = solid_rgba(0, 0, 0, 0, 4, 4);
    let out = run_gpu(&ctx, node, &dummy, 4, 4);
    assert!(
        out[0] < 25 && out[1] < 25 && out[2] < 25,
        "Y=16 must be near-black; got {} {} {}",
        out[0],
        out[1],
        out[2]
    );
    assert!(close(out[3], 255, 4), "alpha must be ~255; got {}", out[3]);
}

#[test]
fn yuv_upload_gpu_white_frame_should_produce_near_white_rgba() {
    let Some(ctx) = gpu_ctx() else { return };
    let mut node = YuvUploadNode::new(YuvFormat::Yuv420p, 4, 4);
    // BT.601 white: Y=235, Cb=Cr=128.
    node.set_planes(vec![235u8; 4 * 4], vec![128u8; 2 * 2], vec![128u8; 2 * 2]);
    let dummy = solid_rgba(0, 0, 0, 0, 4, 4);
    let out = run_gpu(&ctx, node, &dummy, 4, 4);
    assert!(
        out[0] > 225 && out[1] > 225 && out[2] > 225,
        "Y=235 must be near-white; got {} {} {}",
        out[0],
        out[1],
        out[2]
    );
}
