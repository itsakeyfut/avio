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
    LumaMaskNode, OverlayNode, RenderContext, RenderGraph, RenderNode, ScaleAlgorithm, ScaleNode,
    ShapeMaskNode, TransformNode, YuvFormat, YuvUploadNode,
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
