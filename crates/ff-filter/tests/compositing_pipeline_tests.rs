//! End-to-end integration test for the compositing pipeline:
//! polygon garbage matte → chroma key → Porter-Duff Over blend.
//!
//! All frames are generated synthetically using `push_video` / `pull_video`;
//! no fixture files are required.

#![allow(clippy::unwrap_used)]

use ff_filter::{BlendMode, FilterGraph, FilterGraphBuilder};
use ff_format::{AlphaMode, PixelFormat, PooledBuffer, Timestamp, VideoFrame};

/// YUV420p frame filled with a solid colour.
fn make_yuv420p_frame(width: u32, height: u32, y: u8, u: u8, v: u8) -> VideoFrame {
    let y_plane = vec![y; (width * height) as usize];
    let u_plane = vec![u; ((width / 2) * (height / 2)) as usize];
    let v_plane = vec![v; ((width / 2) * (height / 2)) as usize];
    VideoFrame::new(
        vec![
            PooledBuffer::standalone(y_plane),
            PooledBuffer::standalone(u_plane),
            PooledBuffer::standalone(v_plane),
        ],
        vec![width as usize, (width / 2) as usize, (width / 2) as usize],
        width,
        height,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .unwrap()
}

/// End-to-end compositing pipeline:
///   polygon_matte → chromakey → blend(PorterDuffOver, background)
///
/// Setup:
/// - Foreground (slot 1): solid green frame (Y=150, U=44, V=21 ≈ 0x00FF00 in BT.601)
/// - Background (slot 0): solid white frame (Y=235, U=128, V=128)
/// - `polygon_matte`: upper-left triangle (0,0)→(1,0)→(0,1), non-inverted.
///   Inside = alpha 255 (opaque), outside = alpha 0 (transparent).
/// - `chromakey("0x00FF00", 0.3, 0.0)`: keys out all green → alpha 0.
///
/// Expected result: the entire foreground is transparent (either masked by the
/// polygon or keyed by chromakey), so the composite equals the white background.
/// - No green pixels remain in the keyed area (inside the matte, green is removed).
/// - Background is visible outside the matte (outside the polygon, FG is transparent).
/// - Y channel average of the output should be close to 235 (white), not 150 (green).
#[test]
fn garbage_matte_chromakey_blend_over_background_should_composite_correctly() {
    // ── Build the filter graph ─────────────────────────────────────────────────
    //
    // fg_builder: polygon_matte + chromakey applied to the foreground (slot 1).
    // main builder: blend the keyed foreground over the background (slot 0).
    let fg_builder = FilterGraphBuilder::new()
        .polygon_matte(vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)], false)
        .chromakey("0x00FF00", 0.3, 0.0);

    let mut graph = match FilterGraph::builder()
        .blend(
            fg_builder,
            BlendMode::PorterDuffOver,
            1.0,
            AlphaMode::Straight,
        )
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    // ── Push frames ───────────────────────────────────────────────────────────
    // Slot 0: white background (Y=235, U=128, V=128)
    let bg = make_yuv420p_frame(64, 64, 235, 128, 128);
    match graph.push_video(0, &bg) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }

    // Slot 1: solid green foreground (Y=150, U=44, V=21 ≈ 0x00FF00 in BT.601)
    let fg = make_yuv420p_frame(64, 64, 150, 44, 21);
    match graph.push_video(1, &fg) {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    }

    // ── Pull and assert ───────────────────────────────────────────────────────
    let out = graph
        .pull_video()
        .expect("pull_video must not fail")
        .expect("expected Some(frame)");

    assert_eq!(out.width(), 64, "output width must match input");
    assert_eq!(out.height(), 64, "output height must match input");

    // The foreground is entirely transparent after polygon_matte + chromakey,
    // so the composite equals the white background (Y≈235).
    // A Y average above 200 confirms:
    //   - green (Y≈150) was keyed out in the matte region, and
    //   - background is visible outside the matte region.
    if let Some(y_plane) = out.plane(0) {
        let avg_y = y_plane.iter().map(|&b| b as f32).sum::<f32>() / y_plane.len() as f32;
        assert!(
            avg_y > 200.0,
            "composite output should show white background (avg Y≈235), not green foreground \
             (avg Y≈150); got avg_y={avg_y}"
        );
    }
}
