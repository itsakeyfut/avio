//! Integration tests for UDP/MPEG-TS stream input support (issue #224).
//!
//! Tests that require a reachable UDP multicast source are skipped gracefully
//! when unavailable (e.g. in CI without a local IPTV feed — see #235).

mod fixtures;
use fixtures::*;

use ff_decode::{DecodeError, VideoDecoder};
use ff_format::NetworkOptions;

// File-backed decoder is unaffected by UDP changes

#[test]
fn file_video_decoder_should_not_be_live_udp_regression() {
    // Regression guard: the UDP buffer-size logic must not affect file decoders.
    let decoder = VideoDecoder::open(test_video_path())
        .build()
        .expect("Failed to open test video");
    assert!(
        !decoder.is_live(),
        "File-backed VideoDecoder must report is_live=false (UDP regression guard)"
    );
}

// UDP URL bypasses file-existence check

#[test]
fn udp_url_open_should_not_return_file_not_found() {
    // Use an unreachable loopback port. The assertion is that the
    // file-existence guard is bypassed for udp:// URLs.
    let result = VideoDecoder::open("udp://127.0.0.1:65535")
        .network(NetworkOptions::default())
        .build();
    if let Err(DecodeError::FileNotFound { path }) = result {
        panic!("udp:// URL must not produce FileNotFound; path={path:?}");
    }
}

#[test]
fn udp_multicast_url_open_should_not_return_file_not_found() {
    // Common IPTV multicast address format — must not trigger FileNotFound.
    let result = VideoDecoder::open("udp://224.0.0.1:1234")
        .network(NetworkOptions::default())
        .build();
    if let Err(DecodeError::FileNotFound { path }) = result {
        panic!("udp:// multicast URL must not produce FileNotFound; path={path:?}");
    }
}

// Non-UDP URLs are not affected

#[test]
fn http_url_open_should_not_return_file_not_found() {
    // Ensure the UDP-specific logic does not break other schemes.
    let result = VideoDecoder::open("http://127.0.0.1:65535/stream.ts")
        .network(NetworkOptions::default())
        .build();
    if let Err(DecodeError::FileNotFound { path }) = result {
        panic!("http:// URL must not produce FileNotFound; path={path:?}");
    }
}

// Live UDP stream (requires reachable source)

/// Validates that a live UDP/MPEG-TS stream is detected as live.
///
/// Skipped gracefully when no multicast source is reachable (see #235).
#[test]
fn udp_live_decoder_should_report_is_live() {
    let decoder = match VideoDecoder::open("udp://224.0.0.1:1234")
        .network(NetworkOptions::default())
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: no UDP multicast source available ({e})");
            return;
        }
    };

    assert!(
        decoder.is_live(),
        "UDP/MPEG-TS source must report is_live=true"
    );
}
