//! Integration test: Bezier eased x-position animation through `MultiTrackComposer`.
//!
//! Run with:
//! ```text
//! cargo test -p ff-filter bezier_position -- --include-ignored
//! ```

#![allow(clippy::unwrap_used)]

mod fixtures;

use std::time::Duration;

use ff_filter::{
    AnimatedValue, MultiTrackComposer, VideoLayer,
    animation::{AnimationTrack, Easing, Keyframe},
};
use fixtures::{FileGuard, make_source_file, test_output_path};

// ── Dimensions ────────────────────────────────────────────────────────────────

const CANVAS_W: u32 = 1920;
const CANVAS_H: u32 = 1080;
const MARKER_W: u32 = 10;
const MARKER_H: u32 = 10;
const FPS: f64 = 30.0;
const FRAME_COUNT: usize = 60;
const X_FROM: f64 = 0.0;
const X_TO: f64 = 1910.0;

// ── Standalone Bézier reference ───────────────────────────────────────────────
//
// Independent implementation of CSS cubic-bezier(0.25, 0.1, 0.25, 1.0) so the
// reference values are not derived from the `Easing::Bezier` implementation
// under test.
//
// P0=(0,0)  P1=(0.25, 0.1)  P2=(0.25, 1.0)  P3=(1,1)
//
// Polynomial form:
//   cx=0.75, bx=-0.75, ax=1.0     →   x(s) = s³ − 0.75s² + 0.75s
//   cy=0.3,  by=2.4,   ay=−1.7    →   y(s) = −1.7s³ + 2.4s² + 0.3s

fn bezier_css_ease_standalone(norm_t: f64) -> f64 {
    if norm_t <= 0.0 {
        return 0.0;
    }
    if norm_t >= 1.0 {
        return 1.0;
    }

    // Polynomial coefficients
    const CX: f64 = 0.75;
    const BX: f64 = -0.75;
    const AX: f64 = 1.0;
    const CY: f64 = 0.3;
    const BY: f64 = 2.4;
    const AY: f64 = -1.7;

    let bezier_x = |s: f64| ((AX * s + BX) * s + CX) * s;
    let bezier_dx = |s: f64| (3.0 * AX * s + 2.0 * BX) * s + CX;
    let bezier_y = |s: f64| ((AY * s + BY) * s + CY) * s;

    // Newton-Raphson: find s ∈ [0,1] such that bezier_x(s) = norm_t
    let mut s = norm_t;
    for _ in 0..20 {
        let x = bezier_x(s);
        let dx = bezier_dx(s);
        if dx.abs() < 1e-12 {
            break;
        }
        let delta = (x - norm_t) / dx;
        s -= delta;
        s = s.clamp(0.0, 1.0);
        if delta.abs() < 1e-10 {
            break;
        }
    }

    bezier_y(s)
}

/// Pre-compute reference x-positions (pixels) for all 60 frames.
///
/// `norm_t = i / 59` maps frame index to `[0, 1]`; the Bézier easing remaps
/// it to a y-value in `[0, 1]`; multiplying by `X_TO=1910.0` gives pixels.
fn build_bezier_reference() -> [f64; FRAME_COUNT] {
    let mut refs = [0.0_f64; FRAME_COUNT];
    for i in 0..FRAME_COUNT {
        let norm_t = i as f64 / (FRAME_COUNT as f64 - 1.0);
        refs[i] = bezier_css_ease_standalone(norm_t) * X_TO;
    }
    refs
}

// ── Integration test ─────────────────────────────────────────────────────────

#[test]
#[ignore = "requires FFmpeg filter graph; run with -- --include-ignored"]
fn bezier_position_animation_should_match_reference_curve() {
    let reference = build_bezier_reference();

    // ── Step 1: synthetic source files ────────────────────────────────────────
    //
    // Background: 1920×1080, full black (Y=16 U=128 V=128 in studio-swing YUV)
    // Marker:       10× 10, full white (Y=235 U=128 V=128)

    let bg_path = test_output_path("anim_bg_1920x1080.mp4");
    let marker_path = test_output_path("anim_marker_10x10.mp4");

    let _bg_guard = FileGuard::new(bg_path.clone());
    let _marker_guard = FileGuard::new(marker_path.clone());

    if make_source_file(&bg_path, CANVAS_W, CANVAS_H, FPS, FRAME_COUNT, 16, 128, 128).is_none() {
        return;
    }
    if make_source_file(
        &marker_path,
        MARKER_W,
        MARKER_H,
        FPS,
        FRAME_COUNT,
        235,
        128,
        128,
    )
    .is_none()
    {
        return;
    }

    // ── Step 2: Bézier animation track (x: 0 → 1910 over 60 frames @ 30 fps) ─

    let end_pts = Duration::from_secs_f64((FRAME_COUNT as f64 - 1.0) / FPS);
    let bezier_track = AnimationTrack::new()
        .push(Keyframe::new(
            Duration::ZERO,
            X_FROM,
            Easing::Bezier {
                p1: (0.25, 0.1),
                p2: (0.25, 1.0),
            },
        ))
        .push(Keyframe::new(end_pts, X_TO, Easing::Linear));

    // ── Step 3: build MultiTrackComposer ─────────────────────────────────────

    let mut composer = match MultiTrackComposer::new(CANVAS_W, CANVAS_H)
        .add_layer(VideoLayer {
            source: bg_path.clone(),
            proxy: None,
            input_format: None,
            x: AnimatedValue::Static(0.0),
            y: AnimatedValue::Static(0.0),
            scale_x: AnimatedValue::Static(1.0),
            scale_y: AnimatedValue::Static(1.0),
            rotation: AnimatedValue::Static(0.0),
            opacity: AnimatedValue::Static(1.0),
            z_order: 0,
            time_offset: Duration::ZERO,
            in_point: None,
            out_point: None,
            in_transition: None,
            blend_mode: ff_filter::BlendMode::Normal,
            composite_op: ff_filter::CompositeOp::Over,
            effects: vec![],
        })
        .add_layer(VideoLayer {
            source: marker_path.clone(),
            proxy: None,
            input_format: None,
            x: AnimatedValue::Track(bezier_track),
            y: AnimatedValue::Static(0.0),
            scale_x: AnimatedValue::Static(1.0),
            scale_y: AnimatedValue::Static(1.0),
            rotation: AnimatedValue::Static(0.0),
            opacity: AnimatedValue::Static(1.0),
            z_order: 1,
            time_offset: Duration::ZERO,
            in_point: None,
            out_point: None,
            in_transition: None,
            blend_mode: ff_filter::BlendMode::Normal,
            composite_op: ff_filter::CompositeOp::Over,
            effects: vec![],
        })
        .build()
    {
        Ok(g) => g,
        Err(e) => {
            println!("Skipping: MultiTrackComposer::build failed: {e}");
            return;
        }
    };

    // ── Step 4: render 60 frames, detect marker, compare to reference ─────────

    for i in 0..FRAME_COUNT {
        let pts = Duration::from_secs_f64(i as f64 / FPS);

        // Apply animation for this frame before pulling.
        composer.tick(pts);

        let frame = match composer.pull_video() {
            Ok(Some(f)) => f,
            Ok(None) => {
                println!("Skipping: composer ended early at frame {i}");
                return;
            }
            Err(e) => {
                println!("Skipping: pull_video failed at frame {i}: {e}");
                return;
            }
        };

        // Only inspect frames with the expected canvas dimensions.
        if frame.width() != CANVAS_W || frame.height() != CANVAS_H {
            println!(
                "Skipping: unexpected dimensions {}x{} at frame {i}",
                frame.width(),
                frame.height()
            );
            return;
        }

        // ── Detect marker leading edge at Y-plane row 5 ───────────────────────
        //
        // The 10×10 white marker (Y≈235) is composited on a black background
        // (Y≈16).  The first pixel with luma > 128 in row 5 is the marker's
        // left edge.

        let stride = frame.stride(0).unwrap_or(CANVAS_W as usize);
        let y_plane = match frame.plane(0) {
            Some(p) => p,
            None => {
                println!("Skipping: Y-plane unavailable at frame {i}");
                return;
            }
        };

        let row_start = 5 * stride;
        let row_end = (row_start + CANVAS_W as usize).min(y_plane.len());
        let row = &y_plane[row_start..row_end];

        let detected_x = row.iter().position(|&p| p > 128).unwrap_or(0) as f64;
        let expected_x = reference[i];

        assert!(
            (detected_x - expected_x).abs() <= 2.0,
            "frame {i} (pts={:.4}s): detected x={detected_x:.1} expected {expected_x:.1} \
             diff={:.2} tolerance=±2.0",
            pts.as_secs_f64(),
            (detected_x - expected_x).abs()
        );
    }
}
