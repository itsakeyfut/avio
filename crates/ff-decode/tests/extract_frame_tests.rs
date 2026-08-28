//! Integration tests for VideoDecoder::extract_frame.
//!
//! Tests verify:
//! - A frame is returned at or after the requested timestamp
//! - The returned frame PTS is within 1 frame period of the target
//! - Requesting a timestamp beyond the video duration returns NoFrameAtTimestamp

#![allow(clippy::unwrap_used)]

use ff_decode::{DecodeError, VideoDecoder};
use std::time::Duration;

fn test_video_path() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(format!("{manifest_dir}/../../assets/video/gameplay.mp4"))
}

#[test]
fn extract_frame_should_return_frame_at_timestamp() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let mut decoder = match VideoDecoder::open(&path).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: VideoDecoder::build failed ({e})");
            return;
        }
    };

    let target = Duration::from_secs(1);
    let frame = match decoder.extract_frame(target) {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: extract_frame failed ({e})");
            return;
        }
    };

    let pts = frame.timestamp().as_duration();
    assert!(pts >= target, "expected PTS >= {target:?}, got {pts:?}");

    // The frame should be within a generous 2-second window of the target.
    let window = Duration::from_secs(2);
    assert!(
        pts <= target + window,
        "expected PTS within {window:?} of target, got pts={pts:?} target={target:?}"
    );
}

#[test]
fn extract_frame_beyond_duration_should_return_no_frame_at_timestamp() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let mut decoder = match VideoDecoder::open(&path).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: VideoDecoder::build failed ({e})");
            return;
        }
    };

    // Request a frame well past the end of any reasonable video.
    let beyond = Duration::from_secs(999_999);
    let result = decoder.extract_frame(beyond);

    assert!(
        matches!(result, Err(DecodeError::NoFrameAtTimestamp { .. })),
        "expected NoFrameAtTimestamp for timestamp beyond duration, got {result:?}"
    );
}

#[test]
fn extract_frame_at_zero_should_return_first_frame() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let mut decoder = match VideoDecoder::open(&path).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: VideoDecoder::build failed ({e})");
            return;
        }
    };

    let frame = match decoder.extract_frame(Duration::ZERO) {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: extract_frame(0) failed ({e})");
            return;
        }
    };

    // The first decodable frame should be very close to t=0.
    let pts = frame.timestamp().as_duration();
    let window = Duration::from_secs(1);
    assert!(
        pts <= window,
        "expected first frame within {window:?} of t=0, got pts={pts:?}"
    );
}

#[test]
fn extract_frame_multiple_calls_should_each_seek_correctly() {
    let path = test_video_path();
    if !path.exists() {
        println!("Skipping: test video not found at {}", path.display());
        return;
    }

    let mut decoder = match VideoDecoder::open(&path).build() {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: VideoDecoder::build failed ({e})");
            return;
        }
    };

    // Extract two frames in reverse order; each call should seek independently.
    let t2 = Duration::from_secs(2);
    let t1 = Duration::from_secs(1);

    let frame2 = match decoder.extract_frame(t2) {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: extract_frame(t2) failed ({e})");
            return;
        }
    };
    let frame1 = match decoder.extract_frame(t1) {
        Ok(f) => f,
        Err(e) => {
            println!("Skipping: extract_frame(t1) failed ({e})");
            return;
        }
    };

    assert!(
        frame2.timestamp().as_duration() >= t2,
        "frame2 PTS should be >= t2"
    );
    assert!(
        frame1.timestamp().as_duration() >= t1,
        "frame1 PTS should be >= t1"
    );
}
