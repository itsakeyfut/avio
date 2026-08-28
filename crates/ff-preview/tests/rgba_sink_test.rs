//! Integration test: `RgbaSink` decodes ≥10 real video frames and delivers
//! correctly-sized, non-blank RGBA buffers.
//!
//! Opens `assets/test/av_sync_test_60s.mp4` (video-only, 30 fps) via
//! [`PreviewPlayer`], records the first 10 frames, then stops the player early
//! via [`PlayerHandle::stop`]. Asserts buffer sizes and non-blank pixel
//! content for all recorded frames.
//!
//! Run with:
//! ```bash
//! cargo test -p ff-preview -- --include-ignored rgba_sink
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ff_preview::{FrameSink, PlayerHandle, PreviewPlayer};

// Asset path

fn test_file_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/av_sync_test_60s.mp4")
}

// EarlySink

/// Records `(width, height, rgba_len, any_nonzero_pixel)` for each frame.
/// Stops the player after `max_frames` frames via the shared handle.
struct EarlySink {
    records: Arc<Mutex<Vec<(u32, u32, usize, bool)>>>,
    max_frames: usize,
    handle: PlayerHandle,
}

impl FrameSink for EarlySink {
    fn push_frame(&mut self, rgba: &[u8], width: u32, height: u32, _pts: Duration) {
        let any_nonzero = rgba.iter().any(|&b| b != 0);
        let mut guard = self
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.push((width, height, rgba.len(), any_nonzero));
        if guard.len() >= self.max_frames {
            self.handle.stop();
        }
    }
}

#[test]
#[ignore = "requires assets/test/av_sync_test_60s.mp4; run with -- --include-ignored"]
fn rgba_sink_should_deliver_10_frames_with_correct_size_and_no_blank_output() {
    let path = test_file_path();
    if !path.exists() {
        println!("skipping: reference file not found at {}", path.display());
        return;
    }

    let (mut runner, handle) = match PreviewPlayer::open(&path) {
        Ok(p) => p.split(),
        Err(e) => {
            println!("skipping: {e}");
            return;
        }
    };

    let records = Arc::new(Mutex::new(Vec::<(u32, u32, usize, bool)>::new()));

    runner.set_sink(Box::new(EarlySink {
        records: Arc::clone(&records),
        max_frames: 10,
        handle: handle.clone(),
    }));
    let _ = runner.run();

    let records = records
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    assert!(
        records.len() >= 10,
        "expected ≥10 frames; got {} — sink may not have been called",
        records.len()
    );

    for (i, &(w, h, len, any_nonzero)) in records.iter().enumerate() {
        let expected_len = (w * h * 4) as usize;
        assert_eq!(
            len, expected_len,
            "frame {i}: RGBA buffer length must equal width × height × 4; \
             got len={len} expected={expected_len} ({}×{})",
            w, h
        );
        assert!(
            any_nonzero,
            "frame {i}: RGBA output is all-zero — indicates a blank or corrupt decode \
             ({}×{} frame)",
            w, h
        );
    }
}
