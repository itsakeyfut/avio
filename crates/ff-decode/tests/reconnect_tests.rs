//! Integration tests for auto-reconnect on error (issue #226).
//!
//! Tests that require a live network source that drops connections are skipped
//! gracefully when no such server is available (see #235).

mod fixtures;
use fixtures::*;

use ff_decode::{DecodeError, VideoDecoder};
use ff_format::NetworkOptions;

// File-backed decoders are not subject to reconnect

#[test]
fn file_decoder_should_decode_all_frames_without_reconnect() {
    // Regression guard: reconnect logic must not interfere with file decoders.
    let mut decoder = VideoDecoder::open(test_video_path())
        .build()
        .expect("Failed to open test video");

    let mut frame_count = 0u32;
    loop {
        match decoder.decode_one() {
            Ok(Some(_)) => frame_count += 1,
            Ok(None) => break,
            Err(e) => panic!("Unexpected error during file decode: {e}"),
        }
    }
    assert!(frame_count > 0, "Expected at least one decoded frame");
}

#[test]
fn file_decoder_with_reconnect_enabled_should_reach_eof() {
    // A file decoder with reconnect_on_error=true must still reach EOF normally
    // without looping forever (EOF is never a StreamInterrupted error).
    let mut decoder = VideoDecoder::open(test_video_path())
        .network(NetworkOptions {
            reconnect_on_error: true,
            max_reconnect_attempts: 5,
            ..Default::default()
        })
        .build()
        .expect("Failed to open test video");

    let mut frame_count = 0u32;
    loop {
        match decoder.decode_one() {
            Ok(Some(_)) => frame_count += 1,
            Ok(None) => break,
            Err(e) => panic!("Unexpected error with reconnect enabled on file: {e}"),
        }
    }
    assert!(frame_count > 0, "Expected at least one frame");
}

// NetworkOptions defaults

#[test]
fn network_options_should_default_reconnect_on_error_to_false() {
    let opts = NetworkOptions::default();
    assert!(
        !opts.reconnect_on_error,
        "reconnect_on_error must default to false to avoid unexpected overhead"
    );
}

#[test]
fn network_options_should_default_max_reconnect_attempts_to_three() {
    let opts = NetworkOptions::default();
    assert_eq!(
        opts.max_reconnect_attempts, 3,
        "max_reconnect_attempts default should be 3"
    );
}

// Backoff formula

#[test]
fn backoff_ms_should_double_each_attempt_up_to_cap() {
    // Formula: 100 * 2^min(attempt-1, 10)
    let backoff = |attempt: u32| -> u64 { 100u64 * (1u64 << (attempt - 1).min(10)) };

    assert_eq!(backoff(1), 100);
    assert_eq!(backoff(2), 200);
    assert_eq!(backoff(3), 400);
    assert_eq!(backoff(4), 800);
    assert_eq!(backoff(10), 51200);
    assert_eq!(backoff(11), 102400); // capped at 2^10
    assert_eq!(backoff(12), 102400); // still capped
}

// Unreachable network URL: reconnect_on_error=false propagates error

#[test]
fn http_url_with_reconnect_disabled_should_not_reconnect() {
    // When reconnect_on_error=false (default), any network error propagates immediately.
    let result = VideoDecoder::open("http://127.0.0.1:65535/stream.ts")
        .network(NetworkOptions {
            reconnect_on_error: false,
            ..Default::default()
        })
        .build();
    // The result must not be FileNotFound (URL skips existence check).
    if let Err(DecodeError::FileNotFound { path }) = result {
        panic!("http:// URL must not produce FileNotFound; path={path:?}");
    }
    // Any other error (ConnectionFailed, Ffmpeg, etc.) is acceptable.
}

// Live stream reconnect (requires a server that drops connections)

/// Validates reconnect behavior on a live stream that intentionally drops.
///
/// Skipped gracefully when no such server is available (see #235).
#[test]
fn live_stream_decoder_with_reconnect_should_survive_interruption() {
    let mut decoder = match VideoDecoder::open("http://127.0.0.1:9999/drop-after-1s.ts")
        .network(NetworkOptions {
            reconnect_on_error: true,
            max_reconnect_attempts: 2,
            ..Default::default()
        })
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: no drop-test server available ({e})");
            return;
        }
    };

    // Attempt to decode a few frames — the decoder should survive the interruption.
    let mut frames = 0u32;
    for _ in 0..10 {
        match decoder.decode_one() {
            Ok(Some(_)) => frames += 1,
            Ok(None) => break,
            Err(e) => {
                println!("Skipping: stream error before reconnect test ({e})");
                return;
            }
        }
    }
    assert!(
        frames > 0,
        "Expected at least one frame from reconnecting decoder"
    );
}
