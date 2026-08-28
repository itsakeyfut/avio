//! Integration tests for the `ff-render` CPU pipeline.
//!
//! All tests use the CPU fallback path (`RenderGraph::new_cpu` /
//! `process_cpu`) so no GPU or `wgpu` feature is required. Each test
//! verifies a measurable pixel change, not just that construction succeeds.

use ff_render::{
    AlphaMatteNode, BlendMode, BlendModeNode, ChromaKeyNode, ColorGradeNode, CrossfadeNode,
    LumaMaskNode, OverlayNode, RenderGraph, ScaleAlgorithm, ScaleNode, ShapeMaskNode,
    TransformNode, YuvFormat, YuvUploadNode,
};

// Helpers

fn solid_rgba(r: u8, g: u8, b: u8, a: u8, w: u32, h: u32) -> Vec<u8> {
    let n = (w * h * 4) as usize;
    let mut v = Vec::with_capacity(n);
    for _ in 0..(w * h) as usize {
        v.push(r);
        v.push(g);
        v.push(b);
        v.push(a);
    }
    v
}

// ColorGradeNode

#[test]
fn color_grade_node_brightness_boost_should_increase_rgb_channels() {
    let rgba = solid_rgba(100, 100, 100, 255, 4, 4);
    let graph = RenderGraph::new_cpu().push_cpu(ColorGradeNode::new(0.3, 1.0, 1.0, 0.0, 0.0));
    let out = graph.process_cpu(&rgba, 4, 4);
    assert!(
        out[0] > 100,
        "brightness +0.3 must increase R; got {}",
        out[0]
    );
    assert!(
        out[1] > 100,
        "brightness +0.3 must increase G; got {}",
        out[1]
    );
    assert!(
        out[2] > 100,
        "brightness +0.3 must increase B; got {}",
        out[2]
    );
    assert_eq!(out[3], 255, "alpha must be unchanged");
}

#[test]
fn color_grade_node_saturation_zero_should_produce_equal_rgb_channels() {
    let rgba = solid_rgba(200, 100, 50, 255, 2, 2);
    let graph = RenderGraph::new_cpu().push_cpu(ColorGradeNode::new(0.0, 0.0, 1.0, 0.0, 0.0));
    let out = graph.process_cpu(&rgba, 2, 2);
    assert_eq!(
        out[0], out[1],
        "saturation=0 must equalise R and G; got R={} G={}",
        out[0], out[1]
    );
    assert_eq!(
        out[1], out[2],
        "saturation=0 must equalise G and B; got G={} B={}",
        out[1], out[2]
    );
}

// ScaleNode

#[test]
fn scale_node_cpu_path_is_passthrough_and_returns_input_unchanged() {
    // ScaleNode CPU path is not implemented (GPU-only resize).  The node must
    // pass the input through without panicking or modifying pixels.
    let rgba = solid_rgba(128, 64, 32, 255, 4, 4);
    let graph = RenderGraph::new_cpu().push_cpu(ScaleNode::new(2, 2, ScaleAlgorithm::Bilinear));
    let out = graph.process_cpu(&rgba, 4, 4);
    assert_eq!(out, rgba, "ScaleNode CPU path must be a passthrough");
}

// OverlayNode

#[test]
fn overlay_node_fully_opaque_overlay_should_replace_base_color() {
    let base = solid_rgba(0, 0, 0, 255, 4, 4);
    let overlay = solid_rgba(200, 100, 50, 255, 4, 4);
    let node = OverlayNode::new(overlay, 4, 4);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&base, 4, 4);
    assert!(
        out[0] >= 195,
        "opaque overlay must dominate base R; got {}",
        out[0]
    );
}

// CrossfadeNode

#[test]
fn crossfade_node_half_factor_should_average_from_and_to_colors() {
    let from = solid_rgba(0, 0, 0, 255, 2, 2);
    let to = solid_rgba(200, 200, 200, 255, 2, 2);
    let node = CrossfadeNode::new(0.5, to, 2, 2);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&from, 2, 2);
    let r = out[0] as i32;
    assert!(
        (r - 100).abs() <= 5,
        "factor=0.5 must blend R to ≈100; got {r}"
    );
}

// BlendModeNode

#[test]
fn blend_mode_multiply_node_should_darken_base() {
    let base = solid_rgba(128, 128, 128, 255, 2, 2);
    let overlay = solid_rgba(128, 128, 128, 255, 2, 2);
    let node = BlendModeNode::new(BlendMode::Multiply, 1.0, overlay, 2, 2);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&base, 2, 2);
    assert!(
        out[0] < 128,
        "Multiply blend must darken base R; got {}",
        out[0]
    );
}

#[test]
fn blend_mode_screen_node_should_lighten_base() {
    let base = solid_rgba(100, 100, 100, 255, 2, 2);
    let overlay = solid_rgba(100, 100, 100, 255, 2, 2);
    let node = BlendModeNode::new(BlendMode::Screen, 1.0, overlay, 2, 2);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&base, 2, 2);
    assert!(
        out[0] > 100,
        "Screen blend must lighten base R; got {}",
        out[0]
    );
}

#[test]
fn blend_mode_normal_at_zero_opacity_should_leave_base_unchanged() {
    let base = solid_rgba(200, 100, 50, 255, 2, 2);
    let overlay = solid_rgba(0, 0, 0, 255, 2, 2);
    let node = BlendModeNode::new(BlendMode::Normal, 0.0, overlay, 2, 2);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&base, 2, 2);
    assert_eq!(out[0], 200, "opacity=0.0 must leave R unchanged");
    assert_eq!(out[1], 100, "opacity=0.0 must leave G unchanged");
    assert_eq!(out[2], 50, "opacity=0.0 must leave B unchanged");
}

// TransformNode

#[test]
fn transform_node_identity_cpu_should_return_input_unchanged() {
    let rgba = solid_rgba(77, 88, 99, 255, 4, 4);
    let node = TransformNode::new([0.0, 0.0], 0.0, [1.0, 1.0]);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&rgba, 4, 4);
    assert_eq!(out, rgba, "identity transform must return input unchanged");
}

// ChromaKeyNode

#[test]
fn chroma_key_node_pure_green_pixels_should_become_transparent() {
    // key_color is normalised [0.0, 1.0]; pure green = [0.0, 1.0, 0.0].
    let rgba = solid_rgba(0, 255, 0, 255, 2, 2);
    let node = ChromaKeyNode::new([0.0, 1.0, 0.0], 0.3, 0.0);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&rgba, 2, 2);
    assert_eq!(
        out[3], 0,
        "pure key color must produce alpha=0 (transparent)"
    );
}

#[test]
fn chroma_key_node_non_key_pixel_should_remain_opaque() {
    let rgba = solid_rgba(255, 0, 0, 255, 2, 2); // red — not the green key
    let node = ChromaKeyNode::new([0.0, 1.0, 0.0], 0.3, 0.0);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&rgba, 2, 2);
    assert_eq!(
        out[3], 255,
        "red pixels must not be keyed out by a green chroma key"
    );
}

// ShapeMaskNode

#[test]
fn shape_mask_node_opaque_mask_should_preserve_base_alpha() {
    let rgba = solid_rgba(128, 64, 32, 200, 2, 2);
    let mask = solid_rgba(255, 255, 255, 255, 2, 2); // white = keep
    let node = ShapeMaskNode::new(mask, 2, 2);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&rgba, 2, 2);
    assert_eq!(out[3], 200, "white mask must preserve original alpha");
}

#[test]
fn shape_mask_node_transparent_mask_should_zero_alpha() {
    let rgba = solid_rgba(128, 64, 32, 255, 2, 2);
    let mask = solid_rgba(0, 0, 0, 0, 2, 2); // transparent mask → hide
    let node = ShapeMaskNode::new(mask, 2, 2);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&rgba, 2, 2);
    assert_eq!(out[3], 0, "fully transparent mask must produce alpha=0");
}

// LumaMaskNode

#[test]
fn luma_mask_node_white_mask_should_preserve_alpha() {
    let rgba = solid_rgba(128, 64, 32, 200, 2, 2);
    let mask = solid_rgba(255, 255, 255, 255, 2, 2); // bright = luma≈1.0
    let node = LumaMaskNode::new(mask, 2, 2);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&rgba, 2, 2);
    assert_eq!(out[3], 200, "white luma mask must preserve original alpha");
}

#[test]
fn luma_mask_node_black_mask_should_zero_alpha() {
    let rgba = solid_rgba(128, 64, 32, 255, 2, 2);
    let mask = solid_rgba(0, 0, 0, 255, 2, 2); // dark = luma≈0
    let node = LumaMaskNode::new(mask, 2, 2);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&rgba, 2, 2);
    assert_eq!(out[3], 0, "black luma mask must produce alpha=0");
}

// AlphaMatteNode

#[test]
fn alpha_matte_node_transparent_fg_should_reveal_background() {
    let fg = solid_rgba(255, 0, 0, 0, 2, 2); // fully transparent red
    let bg = solid_rgba(0, 0, 255, 255, 2, 2); // opaque blue background
    let node = AlphaMatteNode::new(bg, 2, 2);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&fg, 2, 2);
    assert!(
        out[2] > 200,
        "transparent fg must show blue background; got B={}",
        out[2]
    );
}

#[test]
fn alpha_matte_node_opaque_fg_should_show_foreground() {
    let fg = solid_rgba(255, 0, 0, 255, 2, 2); // fully opaque red
    let bg = solid_rgba(0, 0, 255, 255, 2, 2); // opaque blue background
    let node = AlphaMatteNode::new(bg, 2, 2);
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&fg, 2, 2);
    // Opaque fg dominates: R should be high, B should be low
    assert!(
        out[0] > 200,
        "opaque red fg must dominate; got R={}",
        out[0]
    );
    assert!(
        out[2] < 50,
        "opaque red fg must hide blue background; got B={}",
        out[2]
    );
}

// YuvUploadNode

#[test]
fn yuv_upload_node_cpu_black_frame_should_produce_near_black_rgba() {
    let mut node = YuvUploadNode::new(YuvFormat::Yuv420p, 4, 4);
    // BT.601 black: Y=16, Cb=128, Cr=128
    node.set_planes(vec![16u8; 4 * 4], vec![128u8; 2 * 2], vec![128u8; 2 * 2]);
    let dummy = vec![0u8; 4 * 4 * 4];
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&dummy, 4, 4);
    assert!(
        out[0] < 20,
        "Y=16 must produce near-black R; got {}",
        out[0]
    );
    assert!(
        out[1] < 20,
        "Y=16 must produce near-black G; got {}",
        out[1]
    );
    assert!(
        out[2] < 20,
        "Y=16 must produce near-black B; got {}",
        out[2]
    );
    assert_eq!(out[3], 255, "alpha must be 255");
}

#[test]
fn yuv_upload_node_cpu_white_frame_should_produce_near_white_rgba() {
    let mut node = YuvUploadNode::new(YuvFormat::Yuv420p, 4, 4);
    // BT.601 white: Y=235, Cb=128, Cr=128
    node.set_planes(vec![235u8; 4 * 4], vec![128u8; 2 * 2], vec![128u8; 2 * 2]);
    let dummy = vec![0u8; 4 * 4 * 4];
    let graph = RenderGraph::new_cpu().push_cpu(node);
    let out = graph.process_cpu(&dummy, 4, 4);
    assert!(
        out[0] > 230,
        "Y=235 must produce near-white R; got {}",
        out[0]
    );
    assert!(
        out[1] > 230,
        "Y=235 must produce near-white G; got {}",
        out[1]
    );
    assert!(
        out[2] > 230,
        "Y=235 must produce near-white B; got {}",
        out[2]
    );
}

// Multi-node pipeline

#[test]
fn multi_node_pipeline_brightness_then_multiply_should_accumulate() {
    // Start with mid-grey (128). Boost brightness +0.2 → ~179.
    // Multiply with 50%-grey overlay → ~179 * 0.5 ≈ 89. Net result < 128.
    let base = solid_rgba(128, 128, 128, 255, 2, 2);
    let overlay = solid_rgba(128, 128, 128, 255, 2, 2);
    let graph = RenderGraph::new_cpu()
        .push_cpu(ColorGradeNode::new(0.2, 1.0, 1.0, 0.0, 0.0))
        .push_cpu(BlendModeNode::new(BlendMode::Multiply, 1.0, overlay, 2, 2));
    let out = graph.process_cpu(&base, 2, 2);
    assert!(
        out[0] < 128,
        "brightness+multiply pipeline must reduce R below 128; got {}",
        out[0]
    );
}

#[test]
fn render_graph_empty_pipeline_should_return_input_unchanged() {
    let rgba = solid_rgba(99, 111, 123, 200, 2, 2);
    let graph = RenderGraph::new_cpu();
    let out = graph.process_cpu(&rgba, 2, 2);
    assert_eq!(out, rgba, "empty graph must return input unchanged");
}
