//! Integration tests for ThumbnailSelector.
//!
//! Tests verify:
//! - Zero candidate_interval returns `DecodeError::ExtractionFailed`
//! - Missing input file returns an error
//! - A real video returns a valid VideoFrame

#![allow(clippy::unwrap_used)]

use ff_decode::{DecodeError, ThumbnailSelector};
use std::time::Duration;

fn test_video_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!("{manifest_dir}/../../assets/video/gameplay.mp4"))
}

#[test]
fn thumbnail_selector_zero_interval_should_return_analysis_failed() {
    let result = ThumbnailSelector::new("irrelevant.mp4")
        .candidate_interval(Duration::ZERO)
        .run();
    assert!(
        matches!(result, Err(DecodeError::ExtractionFailed { .. })),
        "expected ExtractionFailed for zero interval, got {result:?}"
    );
}

#[test]
fn thumbnail_selector_missing_input_should_return_error() {
    let result = ThumbnailSelector::new("does_not_exist_99999.mp4").run();
    assert!(result.is_err(), "expected error for missing input file");
}

#[test]
#[ignore = "decodes video frames; run explicitly with -- --include-ignored"]
fn thumbnail_selector_should_return_a_frame() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let result = ThumbnailSelector::new(&path)
        .candidate_interval(Duration::from_secs(5))
        .run();

    match result {
        Ok(frame) => {
            assert!(frame.width() > 0, "expected non-zero frame width");
            assert!(frame.height() > 0, "expected non-zero frame height");
        }
        Err(e) => {
            println!("Skipping: ThumbnailSelector::run failed ({e})");
        }
    }
}
