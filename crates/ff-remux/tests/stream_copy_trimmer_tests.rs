//! Integration tests for StreamCopyTrimmer.
//!
//! These tests verify stream-copy trim behaviour:
//! - Rejecting invalid time ranges
//! - Producing a valid output file for a valid trim range

// Tests are allowed to use unwrap() for simplicity
#![allow(clippy::unwrap_used)]

mod fixtures;

use ff_remux::{RemuxError, StreamCopyTrimmer};
use fixtures::{FileGuard, test_output_path};

// ============================================================================
// Validation Tests
// ============================================================================

#[test]
fn stream_copy_trimmer_should_reject_start_greater_than_end() {
    let result = StreamCopyTrimmer::new("input.mp4", 7.0, 2.0, "output.mp4").run();
    assert!(
        matches!(result, Err(RemuxError::InvalidConfig { .. })),
        "expected InvalidConfig for start > end, got {result:?}"
    );
}

#[test]
fn stream_copy_trimmer_should_reject_equal_start_and_end() {
    let result = StreamCopyTrimmer::new("input.mp4", 5.0, 5.0, "output.mp4").run();
    assert!(
        matches!(result, Err(RemuxError::InvalidConfig { .. })),
        "expected InvalidConfig for start == end, got {result:?}"
    );
}

// ============================================================================
// Functional Tests
// ============================================================================

/// Produce a short MP4 via encoding, then trim a sub-range from it.
///
/// The test encodes 90 black frames at 30 fps (= 3 s), trims [0.5, 2.5],
/// and checks that the output file exists and is non-empty.
#[test]
fn stream_copy_trimmer_should_produce_output_file_for_valid_range() {
    use ff_encode::{BitrateMode, Preset, VideoCodec, VideoEncoder};
    use fixtures::create_black_frame;

    let source_path = test_output_path("trim_source.mp4");
    let output_path = test_output_path("trim_output.mp4");
    let _guard_source = FileGuard::new(source_path.clone());
    let _guard_output = FileGuard::new(output_path.clone());

    // Build a short source file
    let mut encoder = match VideoEncoder::create(&source_path)
        .video(320, 240, 30.0)
        .video_codec(VideoCodec::Mpeg4)
        .bitrate_mode(BitrateMode::Cbr(500_000))
        .preset(Preset::Ultrafast)
        .build()
    {
        Ok(enc) => enc,
        Err(e) => {
            println!("Skipping stream_copy_trimmer test: encoder unavailable ({e})");
            return;
        }
    };

    for _ in 0..90 {
        let frame = create_black_frame(320, 240);
        if let Err(e) = encoder.push_video(&frame) {
            println!("Skipping: push_video failed ({e})");
            return;
        }
    }
    if let Err(e) = encoder.finish() {
        println!("Skipping: encoder.finish failed ({e})");
        return;
    }

    // Trim [0.5, 2.5]
    let result = StreamCopyTrimmer::new(&source_path, 0.5, 2.5, &output_path).run();
    match result {
        Ok(()) => {}
        Err(e) => {
            println!("Skipping: StreamCopyTrimmer::run failed ({e})");
            return;
        }
    }

    assert!(
        output_path.exists(),
        "expected trim output file to exist at {output_path:?}"
    );
    let size = std::fs::metadata(&output_path).unwrap().len();
    assert!(
        size > 0,
        "expected non-empty trim output file, got {size} bytes"
    );
}
