//! Async video encoder backed by a bounded `tokio::sync::mpsc` channel.

use ff_format::VideoFrame;

use super::builder::{VideoEncoder, VideoEncoderBuilder};
use crate::EncodeError;
use crate::async_encoder::{AsyncEncoder, SyncEncoder};

impl SyncEncoder<VideoFrame> for VideoEncoder {
    fn push_frame(&mut self, frame: &VideoFrame) -> Result<(), EncodeError> {
        self.push_video(frame)
    }

    fn drain_and_finish(self) -> Result<(), EncodeError> {
        self.finish()
    }
}

/// Async wrapper around [`VideoEncoder`].
///
/// Frames are queued into a bounded channel (capacity 8) and encoded by a
/// dedicated worker thread. When the channel is full, [`push`] suspends the
/// caller, providing natural back-pressure.
///
/// # Construction
///
/// Use [`VideoEncoder::create`] to configure the encoder, then call
/// [`AsyncVideoEncoder::from_builder`]:
///
/// ```ignore
/// use ff_encode::{AsyncVideoEncoder, VideoEncoder, VideoCodec};
///
/// let mut encoder = AsyncVideoEncoder::from_builder(
///     VideoEncoder::create("output.mp4")
///         .video(1920, 1080, 30.0)
///         .video_codec(VideoCodec::H264),
/// )?;
///
/// encoder.push(frame).await?;
/// encoder.finish().await?;
/// ```
///
/// # Back-pressure
///
/// The internal channel holds at most 8 frames. Once that buffer is full,
/// [`push`] yields until the worker drains a slot. This prevents unbounded
/// memory growth when the encoder cannot keep up with the frame rate.
///
/// [`push`]: AsyncVideoEncoder::push
pub struct AsyncVideoEncoder {
    inner: AsyncEncoder<VideoFrame>,
}

impl std::fmt::Debug for AsyncVideoEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncVideoEncoder").finish_non_exhaustive()
    }
}

impl AsyncVideoEncoder {
    /// Builds an async encoder from a configured builder.
    ///
    /// Consumes the builder, validates the configuration, opens the output
    /// file, and starts the worker thread. The worker runs the synchronous
    /// `FFmpeg` encode loop in the background.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] if the builder configuration is invalid or
    /// the output file cannot be created.
    pub fn from_builder(builder: VideoEncoderBuilder) -> Result<Self, EncodeError> {
        let encoder = builder.build()?;
        Ok(Self {
            inner: AsyncEncoder::new(encoder),
        })
    }

    /// Queues a video frame for encoding.
    ///
    /// If the internal channel (capacity 8) is full, this method suspends
    /// the caller until the worker drains a slot.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError::WorkerPanicked`] if the worker thread has
    /// exited unexpectedly.
    pub async fn push(&mut self, frame: VideoFrame) -> Result<(), EncodeError> {
        self.inner.push(frame).await
    }

    /// Signals end-of-stream, flushes remaining frames, and writes the file trailer.
    ///
    /// Drops the channel sender (signalling EOF to the worker), then waits
    /// for the worker thread to finish without blocking the async executor.
    /// Any error from the worker is propagated back to the caller.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] if encoding fails during flush or if the
    /// worker thread panicked.
    pub async fn finish(self) -> Result<(), EncodeError> {
        self.inner.finish().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Compile-time proof that AsyncVideoEncoder satisfies Send.
    fn _assert_send() {
        fn is_send<T: Send>() {}
        is_send::<AsyncVideoEncoder>();
    }

    #[test]
    fn from_builder_should_fail_on_invalid_config() {
        // A builder with no streams configured is rejected at build time,
        // not in the worker thread — the error surfaces from from_builder.
        let result = AsyncVideoEncoder::from_builder(VideoEncoder::create("out.mp4"));
        assert!(
            result.is_err(),
            "expected error for unconfigured builder, got Ok"
        );
        assert!(
            matches!(result.unwrap_err(), EncodeError::InvalidConfig { .. }),
            "expected InvalidConfig"
        );
    }
}
