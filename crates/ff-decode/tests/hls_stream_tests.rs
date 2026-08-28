//! Integration tests for HLS / M3U8 network stream support (issue #222).
//!
//! Tests that require a reachable HLS server are skipped gracefully when the
//! server is unavailable (e.g. in CI without a local test server — see #235).

mod fixtures;
use fixtures::*;

use ff_decode::{AudioDecoder, DecodeError, SeekMode, VideoDecoder};
use ff_format::NetworkOptions;

// is_live detection on file-backed decoders

#[test]
fn file_video_decoder_should_not_be_live() {
    let decoder = VideoDecoder::open(test_video_path())
        .build()
        .expect("Failed to open test video");
    assert!(
        !decoder.is_live(),
        "File-backed VideoDecoder must report is_live=false"
    );
}

#[test]
fn file_audio_decoder_should_not_be_live() {
    let decoder = AudioDecoder::open(test_audio_path())
        .build()
        .expect("Failed to open test audio");
    assert!(
        !decoder.is_live(),
        "File-backed AudioDecoder must report is_live=false"
    );
}

// Seek is allowed on file-backed decoders

#[test]
fn file_video_decoder_seek_should_not_return_seek_not_supported() {
    let mut decoder = VideoDecoder::open(test_video_path())
        .build()
        .expect("Failed to open test video");
    let result = decoder.seek(std::time::Duration::from_millis(100), SeekMode::Keyframe);
    assert!(
        !matches!(result, Err(DecodeError::SeekNotSupported)),
        "File-backed seek must not return SeekNotSupported, got: {result:?}"
    );
}

#[test]
fn file_audio_decoder_seek_should_not_return_seek_not_supported() {
    let mut decoder = AudioDecoder::open(test_audio_path())
        .build()
        .expect("Failed to open test audio");
    let result = decoder.seek(std::time::Duration::from_millis(100), SeekMode::Keyframe);
    assert!(
        !matches!(result, Err(DecodeError::SeekNotSupported)),
        "File-backed audio seek must not return SeekNotSupported, got: {result:?}"
    );
}

// Network URL does not produce FileNotFound

#[test]
fn hls_video_open_should_not_return_file_not_found() {
    // Use an unreachable loopback address — the important assertion is that
    // the file-existence guard is bypassed for HTTP URLs.
    let result = VideoDecoder::open("http://127.0.0.1:65535/nonexistent/index.m3u8")
        .network(NetworkOptions::default())
        .build();
    if let Err(DecodeError::FileNotFound { path }) = result {
        panic!("HTTP URL must not produce FileNotFound; path={path:?}");
    }
}

#[test]
fn hls_audio_open_should_not_return_file_not_found() {
    let result = AudioDecoder::open("http://127.0.0.1:65535/nonexistent/audio.m3u8")
        .network(NetworkOptions::default())
        .build();
    if let Err(DecodeError::FileNotFound { path }) = result {
        panic!("HTTP URL must not produce FileNotFound; path={path:?}");
    }
}

// Seek guard on live decoder (requires reachable HLS server)

/// Validates that `seek()` on a live HLS decoder returns `SeekNotSupported`.
///
/// Skipped gracefully when no local HLS test server is reachable (see #235).
#[test]
fn hls_live_decoder_seek_should_return_seek_not_supported() {
    let mut decoder = match VideoDecoder::open("http://localhost:8080/live/index.m3u8")
        .network(NetworkOptions::default())
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: no live HLS server available ({e})");
            return;
        }
    };

    if !decoder.is_live() {
        println!("Skipping: decoder did not detect live source (VOD playlist?)");
        return;
    }

    let result = decoder.seek(std::time::Duration::from_secs(5), SeekMode::Keyframe);
    assert!(
        matches!(result, Err(DecodeError::SeekNotSupported)),
        "Expected SeekNotSupported on live HLS stream, got: {result:?}"
    );
}
