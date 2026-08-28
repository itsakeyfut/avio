//! Integration tests for SRT (Secure Reliable Transport) stream input support (issue #225).
//!
//! The `srt` feature flag enables the SRT code path. Tests that require a reachable
//! SRT server are skipped gracefully when none is available (e.g. in CI — see #235).

mod fixtures;
use fixtures::*;

use ff_decode::{DecodeError, VideoDecoder};
use ff_format::NetworkOptions;

// File-backed decoder is not an SRT source

#[test]
fn file_video_decoder_should_not_be_live_srt() {
    // Regression guard: the SRT check must not affect file decoders.
    let decoder = VideoDecoder::open(test_video_path())
        .build()
        .expect("Failed to open test video");
    assert!(
        !decoder.is_live(),
        "File-backed VideoDecoder must report is_live=false (SRT regression guard)"
    );
}

// Without the srt feature, srt:// returns ConnectionFailed

#[cfg(not(feature = "srt"))]
#[test]
fn srt_url_without_feature_should_return_connection_failed() {
    let result = VideoDecoder::open("srt://127.0.0.1:4200")
        .network(NetworkOptions::default())
        .build();
    match result {
        Err(DecodeError::ConnectionFailed { message, .. }) => {
            assert!(
                message.contains("srt"),
                "ConnectionFailed message must mention the srt feature; got: {message}"
            );
        }
        Err(other) => panic!("Expected ConnectionFailed for srt:// without feature, got: {other}"),
        Ok(_) => panic!("Expected an error for unreachable SRT endpoint without feature"),
    }
}

// With the srt feature, srt:// bypasses file-existence check

#[cfg(feature = "srt")]
#[test]
fn srt_url_open_should_not_return_file_not_found() {
    // Use an unreachable loopback port. The assertion is that the
    // file-existence guard is bypassed for srt:// URLs.
    let result = VideoDecoder::open("srt://127.0.0.1:65535")
        .network(NetworkOptions::default())
        .build();
    if let Err(DecodeError::FileNotFound { path }) = result {
        panic!("srt:// URL must not produce FileNotFound; path={path:?}");
    }
}

// Non-SRT URLs are not affected by the SRT check

#[test]
fn http_url_open_should_not_be_affected_by_srt_check() {
    // Ensure the SRT-specific check does not interfere with HTTP URLs.
    let result = VideoDecoder::open("http://127.0.0.1:65535/stream.ts")
        .network(NetworkOptions::default())
        .build();
    if let Err(DecodeError::FileNotFound { path }) = result {
        panic!("http:// URL must not produce FileNotFound; path={path:?}");
    }
}

// Live SRT stream (requires reachable SRT server)

/// Validates that a live SRT stream is detected as live.
///
/// Skipped gracefully when no SRT server is reachable (see #235).
#[cfg(feature = "srt")]
#[test]
fn srt_live_decoder_should_report_is_live() {
    let decoder = match VideoDecoder::open("srt://127.0.0.1:4200")
        .network(NetworkOptions::default())
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: no SRT server available ({e})");
            return;
        }
    };

    assert!(decoder.is_live(), "SRT source must report is_live=true");
}
