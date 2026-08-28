//! Integration tests for `LiveDashOutput` (mirrors live_hls_tests.rs).
//!
//! Pushes synthetic video and audio frames through a `LiveDashOutput` and verifies
//! that the resulting `manifest.mpd` playlist and `.m4s` segment files are correctly
//! structured.  All tests skip gracefully when the required encoder is unavailable.

// Tests are allowed to use unwrap() / expect() for simplicity.
#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]

mod fixtures;
use fixtures::DirGuard;

use ff_format::{AudioFrame, PixelFormat, SampleFormat, VideoFrame};
use ff_stream::{LiveDashOutput, StreamOutput};
use std::time::Duration;

// ============================================================================
// Helpers
// ============================================================================

fn make_video_frame(pts_ms: i64, width: u32, height: u32) -> VideoFrame {
    VideoFrame::new_black(width, height, PixelFormat::Yuv420p, pts_ms)
}

fn make_audio_frame(pts_ms: i64, sample_rate: u32, channels: u32) -> AudioFrame {
    AudioFrame::new_silent(sample_rate, channels, SampleFormat::F32p, pts_ms)
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn live_dash_output_should_generate_valid_manifest_and_segments() {
    let out_dir = tempfile::tempdir().expect("temp dir");
    let _guard = DirGuard(out_dir.path().to_path_buf());

    let mut dash = match LiveDashOutput::new(out_dir.path())
        .segment_duration(Duration::from_secs(2))
        .video(640, 360, 30.0)
        .audio(44100, 2)
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    // Push 30 seconds of synthetic frames (30 fps × 30 s = 900 video frames).
    // One audio frame is pushed per video-second.
    for frame_idx in 0..900_u64 {
        let pts_ms = (frame_idx * 1000 / 30) as i64;
        dash.push_video(&make_video_frame(pts_ms, 640, 360))
            .expect("push_video");
        if frame_idx.is_multiple_of(30) {
            dash.push_audio(&make_audio_frame(pts_ms, 44100, 2))
                .expect("push_audio");
        }
    }
    Box::new(dash).finish().expect("finish");

    // assertions
    let manifest = out_dir.path().join("manifest.mpd");
    assert!(manifest.exists(), "manifest.mpd must exist after finish()");

    let content = std::fs::read_to_string(&manifest).unwrap();
    assert!(
        content.contains("<?xml") || content.contains("<MPD"),
        "manifest must be a valid DASH MPD document; got: {content}"
    );

    // At least one .m4s segment file must have been written.
    let m4s_files: Vec<_> = std::fs::read_dir(out_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |x| x == "m4s"))
        .collect();
    assert!(
        !m4s_files.is_empty(),
        "at least one .m4s segment file must exist after finish()"
    );
}
