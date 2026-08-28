//! Frame sink types for ff-preview.
//!
//! [`FrameSink`] is the primary trait for receiving decoded video frames.
//! [`RgbaSink`] is the reference implementation that stores the latest frame
//! behind an [`Arc<Mutex>`] for rendering-thread access.

use std::sync::{Arc, Mutex};
use std::time::Duration;

// FrameSink

/// A sink that receives decoded video frames as contiguous RGBA bytes.
///
/// Implementations must be `Send` — [`PlayerRunner`](super::PlayerRunner) calls
/// `push_frame` from a dedicated presentation thread.
///
/// # Threading
///
/// `push_frame` is called exclusively from [`PlayerRunner::run`](super::PlayerRunner::run).
/// Do **not** call back into [`PlayerRunner`](super::PlayerRunner) from inside
/// `push_frame` — this will deadlock.
pub trait FrameSink: Send {
    /// Receive a video frame at its presentation time.
    ///
    /// `rgba` is a contiguous, row-major RGBA buffer:
    /// - 4 bytes per pixel (R, G, B, A), alpha always 255
    /// - Total size: `width * height * 4` bytes
    /// - Row stride: `width * 4` bytes (no padding)
    fn push_frame(&mut self, rgba: &[u8], width: u32, height: u32, pts: Duration);

    /// Called when playback ends (EOF or [`PlayerHandle::stop`](super::PlayerHandle::stop)). Default: no-op.
    ///
    /// Implementations should flush any pending output here.
    fn flush(&mut self) {}
}

// RgbaFrame / RgbaSink

/// A decoded video frame as contiguous RGBA bytes.
///
/// Produced by [`RgbaSink`] and stored behind an [`Arc<Mutex>`] so it can be
/// shared safely with a rendering thread.
pub struct RgbaFrame {
    /// Row-major RGBA pixel data.
    ///
    /// Total size: `width * height * 4` bytes. Each pixel is 4 bytes
    /// (R, G, B, A) with alpha always 255.
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Presentation timestamp of the frame.
    pub pts: Duration,
}

/// Reference [`FrameSink`] implementation that stores the latest frame in a
/// shared [`Arc<Mutex<Option<RgbaFrame>>>`].
///
/// Clone [`frame_handle`](Self::frame_handle) to share access with a rendering
/// thread:
///
/// ```ignore
/// let sink   = RgbaSink::new();
/// let handle = sink.frame_handle();
/// player.set_sink(Box::new(sink));
///
/// // In the render loop (any thread):
/// if let Some(frame) = handle.lock().unwrap().as_ref() {
///     upload_to_gpu(&frame.data, frame.width, frame.height);
/// }
/// ```
///
/// Only the **latest** frame is stored — not a queue. Renderers typically only
/// need the current frame, not a backlog.
pub struct RgbaSink {
    /// Shared storage for the most recently received RGBA frame.
    pub last_frame: Arc<Mutex<Option<RgbaFrame>>>,
}

impl RgbaSink {
    /// Create a new `RgbaSink` with an empty frame store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_frame: Arc::new(Mutex::new(None)),
        }
    }

    /// Clone the [`Arc`] for sharing with the rendering thread.
    #[must_use]
    pub fn frame_handle(&self) -> Arc<Mutex<Option<RgbaFrame>>> {
        Arc::clone(&self.last_frame)
    }
}

impl Default for RgbaSink {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameSink for RgbaSink {
    fn push_frame(&mut self, rgba: &[u8], width: u32, height: u32, pts: Duration) {
        let mut guard = self
            .last_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(RgbaFrame {
            data: rgba.to_vec(),
            width,
            height,
            pts,
        });
    }
    // flush() inherits the default no-op
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_sink_should_be_object_safe() {
        // Verify the trait is object-safe: this must compile.
        let _: Option<Box<dyn FrameSink>> = None;
    }

    #[test]
    fn frame_sink_flush_default_should_be_a_noop() {
        struct NoFlushSink;
        impl FrameSink for NoFlushSink {
            fn push_frame(&mut self, _rgba: &[u8], _width: u32, _height: u32, _pts: Duration) {}
            // flush() intentionally NOT overridden — test the default is safe to call.
        }
        let mut sink = NoFlushSink;
        sink.flush(); // must not panic
    }

    #[test]
    fn rgba_sink_should_store_latest_frame_on_push() {
        let mut sink = RgbaSink::new();
        let handle = sink.frame_handle();

        // Before any push, the frame is None.
        assert!(
            handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none(),
            "frame_handle must be None before any push"
        );

        let rgba: Vec<u8> = vec![255u8, 0, 0, 255, 0, 255, 0, 255]; // 2 × 1 RGBA
        let pts = Duration::from_millis(100);
        sink.push_frame(&rgba, 2, 1, pts);

        let guard = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let frame = guard.as_ref().expect("frame must be Some after push");

        assert_eq!(frame.width, 2);
        assert_eq!(frame.height, 1);
        assert_eq!(frame.pts, pts);
        assert_eq!(frame.data, rgba);
    }

    #[test]
    fn rgba_sink_should_replace_frame_on_second_push() {
        let mut sink = RgbaSink::new();
        let handle = sink.frame_handle();

        let first: Vec<u8> = vec![1, 2, 3, 255];
        let second: Vec<u8> = vec![9, 8, 7, 255];
        let pts1 = Duration::from_millis(0);
        let pts2 = Duration::from_millis(33);

        sink.push_frame(&first, 1, 1, pts1);
        sink.push_frame(&second, 1, 1, pts2);

        let guard = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let frame = guard.as_ref().expect("frame must be Some after two pushes");
        assert_eq!(
            frame.data, second,
            "latest push must overwrite previous frame"
        );
        assert_eq!(frame.pts, pts2);
    }

    #[test]
    fn rgba_sink_default_should_equal_new() {
        let a = RgbaSink::new();
        let b = RgbaSink::default();
        // Both must start with None.
        assert!(
            a.frame_handle()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        );
        assert!(
            b.frame_handle()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        );
    }
}
