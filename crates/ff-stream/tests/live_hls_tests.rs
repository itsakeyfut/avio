//! Integration tests for `LiveHlsOutput` (issue #236).
//!
//! Pushes synthetic video and audio frames through a `LiveHlsOutput` and verifies
//! that the resulting `index.m3u8` playlist and `.ts` segment files are correctly
//! structured.  All tests skip gracefully when the required encoder is unavailable.

// Tests are allowed to use unwrap() / expect() for simplicity.
#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]

mod fixtures;
use fixtures::DirGuard;

use ff_format::{AudioFrame, PixelFormat, SampleFormat, VideoFrame};
use ff_stream::{LiveHlsOutput, StreamOutput};
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
fn live_hls_output_should_generate_valid_m3u8_and_segments() {
    let out_dir = tempfile::tempdir().expect("temp dir");
    let _guard = DirGuard(out_dir.path().to_path_buf());

    let mut hls = match LiveHlsOutput::new(out_dir.path())
        .segment_duration(Duration::from_secs(2))
        .playlist_size(3)
        .video(640, 360, 30.0)
        .audio(44100, 2)
        .build()
    {
        Ok(h) => h,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };

    // Push 30 seconds of synthetic frames (30 fps × 30 s = 900 video frames).
    // One audio frame is pushed per video-second.
    for frame_idx in 0..900_u64 {
        let pts_ms = (frame_idx * 1000 / 30) as i64;
        hls.push_video(&make_video_frame(pts_ms, 640, 360))
            .expect("push_video");
        if frame_idx % 30 == 0 {
            hls.push_audio(&make_audio_frame(pts_ms, 44100, 2))
                .expect("push_audio");
        }
    }
    Box::new(hls).finish().expect("finish");

    // assertions
    let playlist = out_dir.path().join("index.m3u8");
    assert!(playlist.exists(), "index.m3u8 must exist after finish()");

    let content = std::fs::read_to_string(&playlist).unwrap();
    assert!(content.contains("#EXTM3U"), "m3u8 must start with #EXTM3U");
    assert!(
        content.contains("#EXT-X-ENDLIST") || content.contains("#EXTINF"),
        "m3u8 must contain segment entries"
    );

    // Sliding window: at most 3 segment references in the playlist.
    let segment_count = content.lines().filter(|l| l.ends_with(".ts")).count();
    assert!(
        segment_count <= 3,
        "sliding window must not exceed playlist_size=3 (got {segment_count})"
    );
    assert!(segment_count >= 1, "at least one segment must be present");

    // The HLS muxer deletes old segments on rotation (when a new segment is
    // started).  The final `av_write_trailer` closes the last segment without
    // triggering one more rotation, so the segment just before the sliding
    // window may still be present on disk.  Allow playlist_size + 1 files.
    let ts_files: Vec<_> = std::fs::read_dir(out_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |x| x == "ts"))
        .collect();
    let playlist_size = 3;
    assert!(
        ts_files.len() <= playlist_size + 1,
        "at most {} .ts files should remain on disk (got {})",
        playlist_size + 1,
        ts_files.len()
    );
}
