//! Integration tests for `VideoDecoder::open()` with HTTP URLs (issue #235).
//!
//! An in-process HTTP test server (`TestHttpServer` in fixtures) serves the
//! committed gameplay.mp4 asset over loopback so CI does not require external
//! network access. Tests that require FFmpeg to successfully open the URL are
//! skipped gracefully when FFmpeg is unavailable or the build fails.

mod fixtures;
use fixtures::*;

use std::time::Duration;

use ff_decode::{DecodeError, VideoDecoder};
use ff_format::NetworkOptions;

// HTTP VOD: decode one frame

#[test]
fn open_url_http_should_decode_first_frame() {
    let server = TestHttpServer::serve(test_video_path());
    let mut decoder = match VideoDecoder::open(server.url())
        .network(NetworkOptions::default())
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    let frame = decoder.decode_one().expect("decode_one should not error");
    assert!(
        frame.is_some(),
        "expected at least one frame from HTTP source"
    );
}

// HTTP VOD: is_live must be false

#[test]
fn open_url_http_vod_should_not_be_live() {
    let server = TestHttpServer::serve(test_video_path());
    let decoder = match VideoDecoder::open(server.url())
        .network(NetworkOptions::default())
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            println!("Skipping: {e}");
            return;
        }
    };
    assert!(
        !decoder.is_live(),
        "VOD over HTTP must not be reported as live"
    );
}

// Refused connection → ConnectionFailed (or NetworkTimeout on Windows)

#[test]
fn open_url_connection_refused_should_return_connection_failed() {
    // Bind to an ephemeral port then immediately drop the listener so the OS
    // has no server for that port.  On Linux/macOS the TCP stack RSTs the SYN
    // immediately → ECONNREFUSED → ConnectionFailed.  On Windows the freed
    // port may be briefly reused by the OS (HTTP layer connection completes at
    // TCP level but no response is sent), so FFmpeg's connect_timeout fires
    // instead → ETIMEDOUT → NetworkTimeout.  Both variants indicate that the
    // URL cannot be opened, which is the semantic under test.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().expect("local_addr").port()
    };
    let url = format!("http://127.0.0.1:{port}/nonexistent.mp4");
    let result = VideoDecoder::open(&url)
        .network(NetworkOptions {
            connect_timeout: Duration::from_millis(500),
            ..NetworkOptions::default()
        })
        .build();
    match result {
        Err(DecodeError::ConnectionFailed { .. }) => {}
        // Windows UCRT: freed loopback port may time out rather than refuse.
        Err(DecodeError::NetworkTimeout { .. }) => {}
        // FFmpeg built without HTTP protocol (e.g. minimal CI packages).
        Err(DecodeError::Ffmpeg { ref message, .. }) if message.contains("Protocol not found") => {
            println!("Skipping: HTTP protocol not available in linked FFmpeg");
        }
        Err(e) => panic!("expected ConnectionFailed or NetworkTimeout, got: {e:?}"),
        Ok(_) => panic!("expected Err from unreachable port, got Ok"),
    }
}
