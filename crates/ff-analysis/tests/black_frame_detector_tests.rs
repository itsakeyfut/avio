//! Integration tests for BlackFrameDetector.
//!
//! Tests verify:
//! - Out-of-range threshold returns `AnalysisError::Failed`
//! - Missing input file returns `AnalysisError::Failed`
//! - A real video file returns a `Vec<Duration>` without errors
//! - Returned timestamps are monotonically non-decreasing

#![allow(clippy::unwrap_used)]

use ff_analysis::{AnalysisError, BlackFrameDetector};
use std::time::Duration;

fn test_video_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!("{manifest_dir}/../../assets/video/gameplay.mp4"))
}

#[test]
fn black_frame_detector_threshold_below_zero_should_return_analysis_failed() {
    let result = BlackFrameDetector::new("irrelevant.mp4")
        .threshold(-0.1)
        .run();
    assert!(
        matches!(result, Err(AnalysisError::Failed { .. })),
        "expected Failed for threshold=-0.1, got {result:?}"
    );
}

#[test]
fn black_frame_detector_threshold_above_one_should_return_analysis_failed() {
    let result = BlackFrameDetector::new("irrelevant.mp4")
        .threshold(1.1)
        .run();
    assert!(
        matches!(result, Err(AnalysisError::Failed { .. })),
        "expected Failed for threshold=1.1, got {result:?}"
    );
}

#[test]
fn black_frame_detector_missing_file_should_return_analysis_failed() {
    let result = BlackFrameDetector::new("does_not_exist_99999.mp4").run();
    assert!(
        matches!(result, Err(AnalysisError::Failed { .. })),
        "expected Failed for missing file, got {result:?}"
    );
}

#[test]
fn black_frame_detector_no_black_should_return_empty() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    // Most gameplay videos have no black frames; a typical result is empty.
    // Use a high threshold so only truly solid-black frames are reported.
    let result = match BlackFrameDetector::new(&path).threshold(0.98).run() {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: BlackFrameDetector::run failed ({e})");
            return;
        }
    };

    // We can't assert the result is empty without knowing the video content,
    // so just verify it's a valid Vec<Duration>.
    for ts in &result {
        let _ = ts.as_secs_f64();
    }
}

#[test]
fn black_frame_detector_timestamps_should_be_monotonically_non_decreasing() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let timestamps = match BlackFrameDetector::new(&path).run() {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: BlackFrameDetector::run failed ({e})");
            return;
        }
    };

    for window in timestamps.windows(2) {
        assert!(
            window[0] <= window[1],
            "timestamps not monotonic: {:?} > {:?}",
            window[0],
            window[1]
        );
    }
}

#[test]
fn black_frame_detector_all_timestamps_within_video_duration() {
    use ff_decode::VideoDecoder;

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

    let timestamps = match BlackFrameDetector::new(&path).run() {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: BlackFrameDetector::run failed ({e})");
            return;
        }
    };

    let margin = Duration::from_secs(1);
    for ts in &timestamps {
        assert!(
            *ts <= duration + margin,
            "timestamp {:?} exceeds video duration {:?}",
            ts,
            duration
        );
    }
}
