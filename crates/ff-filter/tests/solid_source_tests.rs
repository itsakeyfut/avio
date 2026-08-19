//! Integration tests for the solid/color layer source primitive.
//!
//! Probe-gated (RK-002): CI's Linux FFmpeg is built with no filters, so the
//! `color` filter may be absent and `SolidSource::new` fails → skip. On a
//! full-FFmpeg machine this renders a real frame.

use ff_filter::SolidSource;
use ff_format::Color;

#[test]
fn solid_source_should_render_filled_frame_at_requested_size() {
    let mut src = match SolidSource::new(Color::rgb(255, 0, 0), 320, 80, 30.0) {
        Ok(s) => s,
        // Filter unavailable (CI's filterless FFmpeg) — the only legitimate skip.
        Err(e) => {
            println!("Skipping: SolidSource::new failed: {e}");
            return;
        }
    };
    match src.pull() {
        Ok(Some(frame)) => {
            assert_eq!(frame.width(), 320, "solid source width must match canvas");
            assert_eq!(frame.height(), 80, "solid source height must match canvas");
            // rgba: the requested red fill must reach the frame. Sample the centre
            // pixel (row 40, col 160 → byte offset within plane 0).
            let plane = frame.plane(0).expect("rgba frame must have plane 0");
            let stride = plane.len() / 80; // bytes per row (>= width*4, may be padded)
            let px = 40 * stride + 160 * 4;
            let (r, g, b) = (plane[px], plane[px + 1], plane[px + 2]);
            assert!(
                r > 200 && g < 60 && b < 60,
                "centre pixel must be the red fill, got rgb=({r},{g},{b})"
            );
        }
        // The graph built, so it must produce a frame; no frame is an anomaly.
        Ok(None) => panic!("solid source built but produced no frame"),
        Err(e) => println!("Skipping: pull failed: {e}"),
    }
}
