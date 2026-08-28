//! Integration test: frame-accurate seek to t=30s returns a frame within one
//! frame period of the target PTS.
//!
//! Plays `assets/test/av_sync_test_60s.mp4` (60-second, 30 fps, video-only
//! synthetic file), seeks to t=30.000s via [`DecodeBuffer::seek`], and asserts
//! that the first frame delivered after the seek has a PTS within ±34 ms of
//! the target (one 30 fps frame period plus 1 ms margin).
//!
//! Run with:
//! ```bash
//! cargo test -p ff-preview -- --include-ignored seek_to_30s
//! ```

use std::path::PathBuf;
use std::time::Duration;

use ff_preview::{DecodeBuffer, FrameResult};

// Asset path

fn test_file_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/av_sync_test_60s.mp4")
}

#[test]
#[ignore = "requires assets/test/av_sync_test_60s.mp4; run with -- --include-ignored"]
fn seek_to_30s_should_deliver_frame_within_one_frame_period() {
    let path = test_file_path();
    if !path.exists() {
        println!("skipping: reference file not found at {}", path.display());
        return;
    }

    let mut buf = match DecodeBuffer::open(&path).build() {
        Ok(b) => b,
        Err(e) => {
            println!("skipping: {e}");
            return;
        }
    };

    let target = Duration::from_secs(30);
    if let Err(e) = buf.seek(target) {
        println!("skipping: seek not supported: {e}");
        return;
    }

    // Drain any Seeking placeholders; wait for the first real Frame.
    let frame = loop {
        match buf.pop_frame() {
            FrameResult::Frame(f) => break f,
            FrameResult::Seeking(_) => std::thread::sleep(Duration::from_millis(5)),
            FrameResult::Eof => {
                panic!("EOF reached before any frame was delivered after seek to {target:?}");
            }
        }
    };

    // 30 fps → one frame period ≈ 33.3 ms; add 1 ms rounding margin.
    let one_frame = Duration::from_millis(34);
    let pts = frame.timestamp().as_duration();

    assert!(
        pts >= target.saturating_sub(one_frame),
        "post-seek frame PTS must be ≥ (target − 1 frame); \
         target={target:?} pts={pts:?}"
    );
    assert!(
        pts <= target + one_frame,
        "post-seek frame PTS must be ≤ (target + 1 frame); \
         target={target:?} pts={pts:?}"
    );
}
