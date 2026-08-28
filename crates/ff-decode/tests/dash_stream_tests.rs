//! Integration tests for MPEG-DASH / MPD network stream support (issue #223).
//!
//! Tests that require a reachable DASH server are skipped gracefully when the
//! server is unavailable (e.g. in CI without a local test server — see #235).

mod fixtures;
use fixtures::*;

use ff_decode::{DecodeError, SeekMode, VideoDecoder};
use ff_format::NetworkOptions;

// File-backed decoder is not a DASH live stream

#[test]
fn file_video_decoder_should_not_be_live_dash() {
    // Re-verify the baseline: a file-backed decoder never reports is_live=true,
    // even after the DASH demuxer detection logic was added.
    let decoder = VideoDecoder::open(test_video_path())
        .build()
        .expect("Failed to open test video");
    assert!(
        !decoder.is_live(),
        "File-backed VideoDecoder must report is_live=false (DASH regression guard)"
    );
}

// MPD URL does not produce FileNotFound

#[test]
fn dash_mpd_open_should_not_return_file_not_found() {
    // Use an unreachable loopback address. The important assertion is that the
    // file-existence guard is bypassed for HTTP URLs regardless of extension.
    let result = VideoDecoder::open("http://127.0.0.1:65535/nonexistent/manifest.mpd")
        .network(NetworkOptions::default())
        .build();
    if let Err(DecodeError::FileNotFound { path }) = result {
        panic!("HTTP MPD URL must not produce FileNotFound; path={path:?}");
    }
}

// Live DASH stream detection (requires reachable DASH server)

/// Validates that a live DASH stream (`type="dynamic"` MPD) is detected as live.
///
/// Skipped gracefully when no local DASH test server is reachable (see #235).
#[test]
fn dash_live_decoder_should_report_is_live() {
    let decoder = match VideoDecoder::open("http://localhost:8080/dash/live/manifest.mpd")
        .network(NetworkOptions::default())
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: no live DASH server available ({e})");
            return;
        }
    };

    assert!(
        decoder.is_live(),
        "Live DASH (type=dynamic) must report is_live=true"
    );
}

/// Validates that `seek()` on a live DASH decoder returns `SeekNotSupported`.
///
/// Skipped gracefully when no local DASH test server is reachable (see #235).
#[test]
fn dash_live_decoder_seek_should_return_seek_not_supported() {
    let mut decoder = match VideoDecoder::open("http://localhost:8080/dash/live/manifest.mpd")
        .network(NetworkOptions::default())
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: no live DASH server available ({e})");
            return;
        }
    };

    if !decoder.is_live() {
        println!("Skipping: decoder did not detect live DASH source (VOD manifest?)");
        return;
    }

    let result = decoder.seek(std::time::Duration::from_secs(5), SeekMode::Keyframe);
    assert!(
        matches!(result, Err(DecodeError::SeekNotSupported)),
        "Expected SeekNotSupported on live DASH stream, got: {result:?}"
    );
}
