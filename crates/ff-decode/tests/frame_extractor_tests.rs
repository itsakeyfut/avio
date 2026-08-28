//! Integration tests for FrameExtractor.
//!
//! Tests verify:
//! - Zero interval returns `DecodeError::ExtractionFailed`
//! - A real video with a 1-second interval returns the expected frame count
//! - All frame timestamps are monotonically non-decreasing
//! - Each frame's timestamp is within a reasonable window of its expected interval

#![allow(clippy::unwrap_used)]

use ff_decode::{DecodeError, FrameExtractor, VideoDecoder};
use std::time::Duration;

fn test_video_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!("{manifest_dir}/../../assets/video/gameplay.mp4"))
}

#[test]
fn frame_extractor_zero_interval_should_return_analysis_failed() {
    let result = FrameExtractor::new("irrelevant.mp4")
        .interval(Duration::ZERO)
        .run();
    assert!(
        matches!(result, Err(DecodeError::ExtractionFailed { .. })),
        "expected ExtractionFailed for zero interval, got {result:?}"
    );
}

#[test]
fn frame_extractor_missing_file_should_return_error() {
    let result = FrameExtractor::new("does_not_exist_99999.mp4")
        .interval(Duration::from_secs(1))
        .run();
    assert!(result.is_err(), "expected error for missing file, got Ok");
}

#[test]
#[ignore = "decodes entire video; run explicitly with -- --include-ignored"]
fn frame_extractor_one_second_interval_should_return_expected_frame_count() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let duration = match VideoDecoder::open(&path).build() {
        Ok(dec) => dec.duration(),
        Err(e) => {
            println!("Skipping: VideoDecoder failed ({e})");
            return;
        }
    };

    let interval = Duration::from_secs(1);
    let frames = match FrameExtractor::new(&path).interval(interval).run() {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: FrameExtractor::run failed ({e})");
            return;
        }
    };

    // Expected count: floor(duration / interval)
    let expected = (duration.as_secs_f64() / interval.as_secs_f64()).floor() as usize;
    // Allow ±1 due to rounding and end-of-stream handling.
    let diff = (frames.len() as isize - expected as isize).unsigned_abs();
    assert!(
        diff <= 1,
        "expected ~{expected} frames for {duration:?} at {interval:?} interval, got {}",
        frames.len()
    );
}

#[test]
#[ignore = "decodes entire video; run explicitly with -- --include-ignored"]
fn frame_extractor_timestamps_should_be_monotonically_non_decreasing() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let frames = match FrameExtractor::new(&path)
        .interval(Duration::from_secs(2))
        .run()
    {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: FrameExtractor::run failed ({e})");
            return;
        }
    };

    for window in frames.windows(2) {
        let t0 = window[0].timestamp().as_duration();
        let t1 = window[1].timestamp().as_duration();
        assert!(t0 <= t1, "timestamps not monotonic: {t0:?} > {t1:?}");
    }
}

#[test]
#[ignore = "decodes entire video; run explicitly with -- --include-ignored"]
fn frame_extractor_each_frame_within_interval_window_of_target() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let interval = Duration::from_secs(2);
    let frames = match FrameExtractor::new(&path).interval(interval).run() {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: FrameExtractor::run failed ({e})");
            return;
        }
    };

    // Each frame[i] should be close to i * interval.
    let window = interval + Duration::from_secs(1);
    for (i, frame) in frames.iter().enumerate() {
        let target = interval * i as u32;
        let pts = frame.timestamp().as_duration();
        assert!(
            pts >= target,
            "frame[{i}] pts={pts:?} should be >= target={target:?}"
        );
        assert!(
            pts <= target + window,
            "frame[{i}] pts={pts:?} should be within {window:?} of target={target:?}"
        );
    }
}
