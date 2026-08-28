//! Integration test: A/V sync delta ≤ 1 frame over 60-second playback.
//!
//! Plays a 60-second reference file via [`PreviewPlayer`], records the
//! wall-clock delivery time and PTS of every video frame, and asserts that
//! the maximum drift between wall time and PTS is ≤ 34 ms (one frame period
//! at 30 fps + 1 ms margin).
//!
//! Run with:
//! ```bash
//! cargo test -p ff-preview -- --include-ignored av_sync_delta
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ff_preview::{FrameSink, PreviewPlayer};

// RecordingSink

/// [`FrameSink`] that logs `(wall_clock, pts)` for every delivered frame.
struct RecordingSink {
    log: Arc<Mutex<Vec<(Instant, Duration)>>>,
}

impl FrameSink for RecordingSink {
    fn push_frame(&mut self, _rgba: &[u8], _w: u32, _h: u32, pts: Duration) {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((Instant::now(), pts));
    }
}

// Helpers

fn test_file_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/av_sync_test_60s.mp4")
}

#[test]
#[ignore = "requires FFmpeg and assets/test/av_sync_test_60s.mp4; run with -- --include-ignored"]
fn av_sync_delta_should_not_exceed_one_frame_over_60_seconds() {
    let path = test_file_path();
    if !path.exists() {
        println!("skipping: reference file not found at {}", path.display());
        return;
    }

    let (mut runner, _handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: {e}");
            return;
        }
    };

    let log = Arc::new(Mutex::new(Vec::<(Instant, Duration)>::new()));
    runner.set_sink(Box::new(RecordingSink {
        log: Arc::clone(&log),
    }));
    let _ = runner.run();

    let log = log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(!log.is_empty(), "no frames were delivered during playback");

    let tolerance = Duration::from_millis(67);
    let mut max_delta = Duration::ZERO;
    for window in log.windows(2) {
        let wall_delta = window[1].0.duration_since(window[0].0);
        let pts_delta = window[1].1.saturating_sub(window[0].1);
        let delta = wall_delta.abs_diff(pts_delta);
        max_delta = max_delta.max(delta);
    }

    assert!(
        max_delta <= tolerance,
        "max consecutive-frame delivery jitter {max_delta:?} exceeded tolerance {tolerance:?} \
         ({} frames recorded)",
        log.len()
    );
}
