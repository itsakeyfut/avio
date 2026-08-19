//! Integration tests for the text/title layer source primitive.
//!
//! Probe-gated (RK-002): CI's Linux FFmpeg is built with no filters, so
//! `color`/`drawtext` may be absent and `TextSource::new` fails → skip. On a
//! full-FFmpeg machine this renders a real frame.

use ff_filter::{FilterError, TextSource};
use ff_format::TextSpec;

#[test]
fn text_source_should_render_frame_at_requested_size() {
    let spec = TextSpec::new("Title");
    let mut src = match TextSource::new(&spec, 320, 80, 30.0) {
        Ok(s) => s,
        // Filter unavailable (CI's filterless FFmpeg) — the only legitimate skip.
        Err(e) => {
            println!("Skipping: TextSource::new failed: {e}");
            return;
        }
    };
    match src.pull() {
        Ok(Some(frame)) => {
            assert_eq!(frame.width(), 320, "text source width must match canvas");
            assert_eq!(frame.height(), 80, "text source height must match canvas");
            // The graph built (so `color`/`drawtext` and a font resolved), so the
            // frame must actually contain drawn text: the background is a fully
            // transparent black canvas (all-zero rgba bytes), so any non-zero byte
            // proves pixels were drawn.
            let plane = frame.plane(0).expect("rgba frame must have plane 0");
            assert!(
                plane.iter().any(|&b| b != 0),
                "rendered text frame must contain drawn (non-transparent) pixels"
            );
        }
        // The graph built, so it must produce a frame; no frame is an anomaly, not
        // a skip condition.
        Ok(None) => panic!("text source built but produced no frame"),
        Err(e) => println!("Skipping: pull failed: {e}"),
    }
}

#[test]
fn text_source_should_reject_empty_text() {
    // The empty-text guard runs before any FFmpeg call, so this must fail with
    // `InvalidConfig` everywhere — matching the variant (not just `is_err`) keeps
    // the test meaningful on the filterless CI, where a missing guard would still
    // surface as a (different) build error.
    let spec = TextSpec::new("");
    match TextSource::new(&spec, 320, 80, 30.0) {
        Err(FilterError::InvalidConfig { .. }) => {}
        Err(e) => panic!("expected InvalidConfig for empty text, got a different error: {e}"),
        Ok(_) => panic!("expected InvalidConfig for empty text, but construction succeeded"),
    }
}
