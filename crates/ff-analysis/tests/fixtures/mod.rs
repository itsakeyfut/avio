//! Test fixtures and helpers for ff-decode integration tests.
//!
//! This module provides common utilities for testing video and audio decoding:
//! - Asset path helpers (test files, videos, audio)
//! - Decoder creation helpers with default settings
//! - Assertions and validation helpers

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ff_decode::{AudioDecoder, HardwareAccel, VideoDecoder};

// ============================================================================
// Asset Path Helpers
// ============================================================================

/// Returns the path to the test assets directory.
pub fn assets_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(format!("{}/../../assets", manifest_dir))
}

/// Returns the path to the test video file.
pub fn test_video_path() -> PathBuf {
    assets_dir().join("video/gameplay.mp4")
}

/// Returns the path to the test audio file.
pub fn test_audio_path() -> PathBuf {
    assets_dir().join("audio/konekonoosanpo.mp3")
}

/// Returns the path to the test JPEG image file.
pub fn test_jpeg_path() -> PathBuf {
    assets_dir().join("img/hello-triangle.jpg")
}

// ============================================================================
// Decoder Creation Helpers
// ============================================================================

/// Creates a basic video decoder with default settings.
///
/// Uses software decoding (no hardware acceleration) for consistent behavior
/// across different test environments.
pub fn create_decoder() -> Result<VideoDecoder, ff_decode::DecodeError> {
    VideoDecoder::open(&test_video_path())
        .hardware_accel(HardwareAccel::None)
        .build()
}

/// Creates a basic audio decoder with default settings.
pub fn create_audio_decoder() -> Result<AudioDecoder, ff_decode::DecodeError> {
    AudioDecoder::open(&test_audio_path()).build()
}

// ============================================================================
// In-process HTTP test server
// ============================================================================

/// Handles one HTTP connection: reads request headers, parses an optional
/// `Range:` header, and writes a `200 OK` or `206 Partial Content` response.
///
/// Range support is required because `gameplay.mp4` has its `moov` atom at
/// the end of the file; FFmpeg issues a Range request to seek backwards to it.
fn handle_request(tcp: &mut TcpStream, path: &std::path::Path) {
    // Read request headers using a borrowed BufReader so we can write to
    // `tcp` directly after the reader is dropped (no try_clone needed).
    let mut range_start: Option<u64> = None;
    let mut range_end: Option<u64> = None;

    {
        let mut reader = BufReader::new(&*tcp);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if line == "\r\n" {
                break; // blank line = end of HTTP headers
            }
            // Parse "Range: bytes=X-Y" or "Range: bytes=X-"
            if line.to_ascii_lowercase().starts_with("range:") {
                let value = line["range:".len()..].trim();
                if let Some(bytes) = value.strip_prefix("bytes=") {
                    let mut parts = bytes.splitn(2, '-');
                    if let Some(start_str) = parts.next() {
                        range_start = start_str.trim().parse().ok();
                    }
                    if let Some(end_str) = parts.next() {
                        let end_str = end_str.trim();
                        if !end_str.is_empty() {
                            range_end = end_str.parse().ok();
                        }
                    }
                }
            }
        }
    } // reader dropped here; tcp borrow released

    let body = std::fs::read(path).unwrap_or_default();
    let total = body.len() as u64;

    if let Some(start) = range_start {
        // Range request → 206 Partial Content
        let end = range_end
            .unwrap_or(total.saturating_sub(1))
            .min(total.saturating_sub(1));
        let start = start.min(total);
        let chunk = &body[start as usize..=(end as usize).min(body.len().saturating_sub(1))];
        let header = format!(
            "HTTP/1.1 206 Partial Content\r\n\
             Content-Type: video/mp4\r\n\
             Content-Range: bytes {start}-{end}/{total}\r\n\
             Content-Length: {len}\r\n\
             Accept-Ranges: bytes\r\n\
             Connection: close\r\n\
             \r\n",
            len = chunk.len(),
        );
        let _ = tcp.write_all(header.as_bytes());
        let _ = tcp.write_all(chunk);
    } else {
        // Full request → 200 OK
        let header = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: video/mp4\r\n\
             Content-Length: {total}\r\n\
             Accept-Ranges: bytes\r\n\
             Connection: close\r\n\
             \r\n",
        );
        let _ = tcp.write_all(header.as_bytes());
        let _ = tcp.write_all(&body);
    }
}

/// A minimal single-threaded HTTP/1.1 file server for testing.
///
/// Binds to `127.0.0.1:0` (OS-assigned ephemeral port) and serves the given
/// file for every `GET` connection. Supports `Range:` requests so FFmpeg can
/// seek into files whose `moov` atom is at the end (non-fast-start MP4s).
/// Multiple sequential connections are handled so FFmpeg's internal probe +
/// stream round-trip succeeds.
///
/// The server thread is stopped and joined when this value is dropped.
pub struct TestHttpServer {
    port: u16,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TestHttpServer {
    /// Start serving `path` over HTTP on a loopback port chosen by the OS.
    ///
    /// Panics if the listener cannot be bound (extremely unlikely on any
    /// standard OS).
    pub fn serve(path: impl AsRef<std::path::Path>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test HTTP server");
        let port = listener.local_addr().expect("local_addr").port();
        let running = Arc::new(AtomicBool::new(true));
        let running2 = Arc::clone(&running);
        let path = path.as_ref().to_path_buf();

        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if !running2.load(Ordering::SeqCst) {
                    break;
                }
                match stream {
                    Ok(mut tcp) => handle_request(&mut tcp, &path),
                    Err(_) => break,
                }
                if !running2.load(Ordering::SeqCst) {
                    break;
                }
            }
        });

        Self {
            port,
            running,
            thread: Some(thread),
        }
    }

    /// Returns the base URL to pass to `VideoDecoder::open()`,
    /// e.g. `"http://127.0.0.1:54321/video.mp4"`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/video.mp4", self.port)
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Poke the listener to unblock `listener.incoming()`.
        let _ = TcpStream::connect(format!("127.0.0.1:{}", self.port));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
