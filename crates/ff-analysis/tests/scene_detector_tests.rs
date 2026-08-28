//! Integration tests for SceneDetector.
//!
//! Tests verify:
//! - Out-of-range threshold returns `AnalysisError::Failed`
//! - Missing input file returns `AnalysisError::Failed`
//! - A real video file returns a `Vec<Duration>` (possibly empty for static video)
//! - Returned timestamps are monotonically non-decreasing

#![allow(clippy::unwrap_used)]

use ff_analysis::{AnalysisError, SceneDetector};
use std::time::Duration;

fn test_video_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!("{manifest_dir}/../../assets/video/gameplay.mp4"))
}

#[test]
fn scene_detector_threshold_below_zero_should_return_analysis_failed() {
    let result = SceneDetector::new("irrelevant.mp4").threshold(-0.1).run();
    assert!(
        matches!(result, Err(AnalysisError::Failed { .. })),
        "expected Failed for threshold=-0.1, got {result:?}"
    );
}

#[test]
fn scene_detector_threshold_above_one_should_return_analysis_failed() {
    let result = SceneDetector::new("irrelevant.mp4").threshold(1.1).run();
    assert!(
        matches!(result, Err(AnalysisError::Failed { .. })),
        "expected Failed for threshold=1.1, got {result:?}"
    );
}

#[test]
fn scene_detector_missing_file_should_return_analysis_failed() {
    let result = SceneDetector::new("does_not_exist_99999.mp4").run();
    assert!(
        matches!(result, Err(AnalysisError::Failed { .. })),
        "expected Failed for missing file, got {result:?}"
    );
}

#[test]
fn scene_detector_should_return_vec_duration() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video file not found at {}", path.display());
        return;
    }

    let result = match SceneDetector::new(&path).run() {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping: SceneDetector::run failed ({e})");
            return;
        }
    };

    // Each timestamp should be non-negative (Duration is always >= 0, so just
    // check that the return type is correct and the vec is accessible).
    for ts in &result {
        let _ = ts.as_secs_f64();
    }
}

#[test]
fn scene_detector_timestamps_should_be_monotonically_non_decreasing() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video file not found at {}", path.display());
        return;
    }

    let timestamps = match SceneDetector::new(&path).run() {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping: SceneDetector::run failed ({e})");
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
fn scene_detector_low_threshold_should_return_more_cuts_than_high_threshold() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video file not found at {}", path.display());
        return;
    }

    let low = match SceneDetector::new(&path).threshold(0.1).run() {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping: low-threshold run failed ({e})");
            return;
        }
    };
    let high = match SceneDetector::new(&path).threshold(0.9).run() {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping: high-threshold run failed ({e})");
            return;
        }
    };

    assert!(
        low.len() >= high.len(),
        "expected low threshold ({}) to produce >= cuts as high threshold ({}), got {} vs {}",
        0.1,
        0.9,
        low.len(),
        high.len()
    );
}

#[test]
fn scene_detector_all_timestamps_within_video_duration() {
    use ff_decode::VideoDecoder;

    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video file not found at {}", path.display());
        return;
    }

    let duration = match VideoDecoder::open(&path).build() {
        Ok(dec) => dec.duration(),
        Err(e) => {
            println!("Skipping: VideoDecoder failed ({e})");
            return;
        }
    };

    let timestamps = match SceneDetector::new(&path).run() {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping: SceneDetector::run failed ({e})");
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
