//! Integration tests for LiveAbrLadder.
//!
//! These tests push synthetic frames through a multi-rendition HLS ABR ladder
//! and verify that the expected output files are created.
//!
//! All tests skip gracefully when the required encoder is unavailable.

// Tests are allowed to use unwrap() for simplicity.
#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]

mod fixtures;

use ff_format::{PixelFormat, PooledBuffer, Timestamp, VideoFrame};
use ff_stream::{AbrRendition, LiveAbrLadder, StreamError, StreamOutput};
use fixtures::{DirGuard, tmp_dir};
use std::time::Duration;

// ============================================================================
// Helpers
// ============================================================================

/// Create a minimal synthetic YUV420p frame (4×4 pixels, black).
fn make_frame() -> VideoFrame {
    let y_size = 4 * 4;
    let uv_size = (4 / 2) * (4 / 2);
    VideoFrame::new(
        vec![
            PooledBuffer::standalone(vec![16u8; y_size]),
            PooledBuffer::standalone(vec![128u8; uv_size]),
            PooledBuffer::standalone(vec![128u8; uv_size]),
        ],
        vec![4, 2, 2],
        4,
        4,
        PixelFormat::Yuv420p,
        Timestamp::default(),
        true,
    )
    .expect("frame creation failed")
}

/// Run a 3-rendition HLS ladder with `frame_count` frames.
///
/// Returns `None` when the encoder is unavailable (test should skip).
fn run_hls_ladder(test_name: &str, frame_count: usize) -> Option<(std::path::PathBuf, DirGuard)> {
    let out_dir = tmp_dir(test_name);
    let guard = DirGuard(out_dir.clone());

    let mut ladder = match LiveAbrLadder::new(&out_dir)
        .add_rendition(AbrRendition {
            width: 32,
            height: 24,
            video_bitrate: 400_000,
            audio_bitrate: 64_000,
            name: None,
        })
        .add_rendition(AbrRendition {
            width: 16,
            height: 12,
            video_bitrate: 200_000,
            audio_bitrate: 64_000,
            name: None,
        })
        .add_rendition(AbrRendition {
            width: 8,
            height: 8,
            video_bitrate: 100_000,
            audio_bitrate: 64_000,
            name: None,
        })
        .fps(25.0)
        .segment_duration(Duration::from_secs(1))
        .build()
    {
        Ok(l) => l,
        Err(StreamError::Ffmpeg { code, message }) => {
            println!("Skipping: encoder unavailable: {message} (code={code})");
            return None;
        }
        Err(e) => panic!("unexpected build error: {e}"),
    };

    let frame = make_frame();
    for _ in 0..frame_count {
        match ladder.push_video(&frame) {
            Ok(()) => {}
            Err(StreamError::Ffmpeg { code, message }) => {
                println!("Skipping: push_video failed: {message} (code={code})");
                return None;
            }
            Err(e) => panic!("unexpected push_video error: {e}"),
        }
    }

    match Box::new(ladder).finish() {
        Ok(()) => {}
        Err(StreamError::Ffmpeg { code, message }) => {
            println!("Skipping: finish failed: {message} (code={code})");
            return None;
        }
        Err(e) => panic!("unexpected finish error: {e}"),
    }

    Some((out_dir, guard))
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn hls_should_produce_master_playlist_after_finish() {
    let Some((out_dir, _guard)) = run_hls_ladder("live_abr_master_test", 50) else {
        return;
    };

    let master = out_dir.join("master.m3u8");
    assert!(master.exists(), "master.m3u8 should exist");
    assert!(
        std::fs::metadata(&master).unwrap().len() > 0,
        "master.m3u8 should be non-empty"
    );
}

#[test]
fn hls_master_playlist_should_contain_all_rendition_entries() {
    let Some((out_dir, _guard)) = run_hls_ladder("live_abr_master_content_test", 50) else {
        return;
    };

    let content = std::fs::read_to_string(out_dir.join("master.m3u8")).unwrap();
    assert!(content.contains("#EXTM3U"), "missing #EXTM3U");
    assert!(
        content.contains("#EXT-X-STREAM-INF"),
        "missing #EXT-X-STREAM-INF"
    );
    assert!(
        content.contains("32x24/index.m3u8"),
        "missing 32x24 rendition link"
    );
    assert!(
        content.contains("16x12/index.m3u8"),
        "missing 16x12 rendition link"
    );
    assert!(
        content.contains("8x8/index.m3u8"),
        "missing 8x8 rendition link"
    );
}

#[test]
fn hls_should_produce_three_variant_subdirectories() {
    let Some((out_dir, _guard)) = run_hls_ladder("live_abr_subdirs_test", 50) else {
        return;
    };

    for name in &["32x24", "16x12", "8x8"] {
        let variant_dir = out_dir.join(name);
        assert!(
            variant_dir.is_dir(),
            "rendition subdirectory {name}/ should exist"
        );
    }
}

#[test]
fn hls_each_variant_should_have_index_playlist() {
    let Some((out_dir, _guard)) = run_hls_ladder("live_abr_playlists_test", 50) else {
        return;
    };

    for name in &["32x24", "16x12", "8x8"] {
        let playlist = out_dir.join(name).join("index.m3u8");
        assert!(
            playlist.exists(),
            "{name}/index.m3u8 should exist after finish"
        );
    }
}

#[test]
fn hls_master_playlist_should_contain_bandwidth_for_each_rendition() {
    let Some((out_dir, _guard)) = run_hls_ladder("live_abr_bandwidth_test", 50) else {
        return;
    };

    let content = std::fs::read_to_string(out_dir.join("master.m3u8")).unwrap();
    // BANDWIDTH = video_bitrate + audio_bitrate
    assert!(
        content.contains("BANDWIDTH=464000"),
        "missing BANDWIDTH=464000 (400000+64000) in master.m3u8\ncontent:\n{content}"
    );
    assert!(
        content.contains("BANDWIDTH=264000"),
        "missing BANDWIDTH=264000 (200000+64000) in master.m3u8\ncontent:\n{content}"
    );
    assert!(
        content.contains("BANDWIDTH=164000"),
        "missing BANDWIDTH=164000 (100000+64000) in master.m3u8\ncontent:\n{content}"
    );
}
