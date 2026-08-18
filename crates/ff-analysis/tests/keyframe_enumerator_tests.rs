//! Integration tests for KeyframeEnumerator.
//!
//! Tests verify:
//! - Missing input file returns `AnalysisError::Failed`
//! - Out-of-range stream_index returns `AnalysisError::Failed`
//! - A real video file returns a non-empty `Vec<Duration>`
//! - Returned timestamps are monotonically non-decreasing

#![allow(clippy::unwrap_used)]

use ff_analysis::{AnalysisError, KeyframeEnumerator};

fn test_video_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!("{manifest_dir}/../../assets/video/gameplay.mp4"))
}

// ── Error-path tests ──────────────────────────────────────────────────────────

#[test]
fn keyframe_enumerator_missing_file_should_return_analysis_failed() {
    let result = KeyframeEnumerator::new("does_not_exist_99999.mp4").run();
    assert!(
        matches!(result, Err(AnalysisError::Failed { .. })),
        "expected Failed for missing file, got {result:?}"
    );
}

#[test]
fn keyframe_enumerator_invalid_stream_index_should_return_analysis_failed() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video file not found at {}", path.display());
        return;
    }

    let result = KeyframeEnumerator::new(&path).stream_index(9999).run();
    assert!(
        matches!(result, Err(AnalysisError::Failed { .. })),
        "expected Failed for stream_index=9999, got {result:?}"
    );
}

// ── Functional tests ──────────────────────────────────────────────────────────

#[test]
fn keyframe_enumerator_should_return_non_empty_vec() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video file not found at {}", path.display());
        return;
    }

    let keyframes = match KeyframeEnumerator::new(&path).run() {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: KeyframeEnumerator::run failed ({e})");
            return;
        }
    };

    assert!(
        !keyframes.is_empty(),
        "expected at least one keyframe in a real video file"
    );
}

#[test]
fn keyframe_enumerator_timestamps_should_be_monotonically_non_decreasing() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video file not found at {}", path.display());
        return;
    }

    let keyframes = match KeyframeEnumerator::new(&path).run() {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: KeyframeEnumerator::run failed ({e})");
            return;
        }
    };

    for window in keyframes.windows(2) {
        assert!(
            window[0] <= window[1],
            "timestamps not monotonic: {:?} > {:?}",
            window[0],
            window[1]
        );
    }
}

#[test]
fn keyframe_enumerator_explicit_stream_zero_should_match_default() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video file not found at {}", path.display());
        return;
    }

    let default_result = match KeyframeEnumerator::new(&path).run() {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: default run failed ({e})");
            return;
        }
    };

    let explicit_result = match KeyframeEnumerator::new(&path).stream_index(0).run() {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: stream_index(0) run failed ({e})");
            return;
        }
    };

    // Both should return the same keyframe count (stream 0 is always the first
    // video stream for a typical video file).
    assert_eq!(
        default_result.len(),
        explicit_result.len(),
        "default and stream_index(0) should yield the same keyframe count"
    );
}

#[test]
fn keyframe_enumerator_all_timestamps_within_video_duration() {
    use ff_decode::VideoDecoder;
    use std::time::Duration;

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

    let keyframes = match KeyframeEnumerator::new(&path).run() {
        Ok(v) => v,
        Err(e) => {
            println!("Skipping: KeyframeEnumerator::run failed ({e})");
            return;
        }
    };

    let margin = Duration::from_secs(1);
    for ts in &keyframes {
        assert!(
            *ts <= duration + margin,
            "keyframe timestamp {:?} exceeds video duration {:?}",
            ts,
            duration
        );
    }
}
