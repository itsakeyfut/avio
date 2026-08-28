//! Multi-target fan-out wrapper.
//!
//! [`FanoutOutput`] delivers each frame to multiple [`StreamOutput`] targets
//! simultaneously. When one or more targets fail, all remaining targets in the
//! list still receive the frame before any error is returned.
//!
//! # Example
//!
//! ```ignore
//! use ff_stream::{FanoutOutput, LiveHlsOutput, RtmpOutput, StreamOutput};
//!
//! let hls = LiveHlsOutput::new("/var/www/live")
//!     .video(1280, 720, 30.0)
//!     .build()?;
//!
//! let rtmp = RtmpOutput::new("rtmp://ingest.example.com/live/key")
//!     .video(1280, 720, 30.0)
//!     .build()?;
//!
//! let mut fanout = FanoutOutput::new(vec![
//!     Box::new(hls),
//!     Box::new(rtmp),
//! ]);
//!
//! // for each decoded frame:
//! fanout.push_video(&video_frame)?;
//! fanout.push_audio(&audio_frame)?;
//!
//! // when done:
//! Box::new(fanout).finish()?;
//! ```

use ff_format::{AudioFrame, VideoFrame};

use crate::error::StreamError;
use crate::output::StreamOutput;

// ============================================================================
// FanoutOutput — safe multi-target wrapper
// ============================================================================

/// A [`StreamOutput`] wrapper that fans frames out to multiple targets.
///
/// Create with [`FanoutOutput::new`], passing a `Vec<Box<dyn StreamOutput>>`.
/// Every call to [`push_video`](Self::push_video), [`push_audio`](Self::push_audio),
/// or [`finish`](StreamOutput::finish) is forwarded to **all** targets in order.
///
/// ## Failure behaviour
///
/// When one or more targets return an error, the remaining targets still
/// receive the frame. All errors are collected and returned as a single
/// [`StreamError::FanoutFailure`].
pub struct FanoutOutput {
    targets: Vec<Box<dyn StreamOutput>>,
}

impl FanoutOutput {
    /// Create a new fanout output that delivers frames to all `targets`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use ff_stream::{FanoutOutput, LiveHlsOutput, StreamOutput};
    ///
    /// let out = FanoutOutput::new(vec![Box::new(hls_output)]);
    /// ```
    #[must_use]
    pub fn new(targets: Vec<Box<dyn StreamOutput>>) -> Self {
        Self { targets }
    }
}

// ============================================================================
// StreamOutput impl
// ============================================================================

impl StreamOutput for FanoutOutput {
    fn push_video(&mut self, frame: &VideoFrame) -> Result<(), StreamError> {
        let total = self.targets.len();
        let mut errors: Vec<(usize, StreamError)> = Vec::new();

        for (i, target) in self.targets.iter_mut().enumerate() {
            if let Err(e) = target.push_video(frame) {
                errors.push((i, e));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(StreamError::FanoutFailure {
                failed: errors.len(),
                total,
                messages: errors
                    .into_iter()
                    .map(|(i, e)| format!("target[{i}]: {e}"))
                    .collect(),
            })
        }
    }

    fn push_audio(&mut self, frame: &AudioFrame) -> Result<(), StreamError> {
        let total = self.targets.len();
        let mut errors: Vec<(usize, StreamError)> = Vec::new();

        for (i, target) in self.targets.iter_mut().enumerate() {
            if let Err(e) = target.push_audio(frame) {
                errors.push((i, e));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(StreamError::FanoutFailure {
                failed: errors.len(),
                total,
                messages: errors
                    .into_iter()
                    .map(|(i, e)| format!("target[{i}]: {e}"))
                    .collect(),
            })
        }
    }

    fn finish(self: Box<Self>) -> Result<(), StreamError> {
        let total = self.targets.len();
        let mut errors: Vec<(usize, StreamError)> = Vec::new();

        for (i, target) in self.targets.into_iter().enumerate() {
            if let Err(e) = target.finish() {
                errors.push((i, e));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(StreamError::FanoutFailure {
                failed: errors.len(),
                total,
                messages: errors
                    .into_iter()
                    .map(|(i, e)| format!("target[{i}]: {e}"))
                    .collect(),
            })
        }
    }
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ff_format::{PixelFormat, SampleFormat, VideoFrame};

    // Mock helpers

    struct OkOutput;

    impl StreamOutput for OkOutput {
        fn push_video(&mut self, _frame: &VideoFrame) -> Result<(), StreamError> {
            Ok(())
        }

        fn push_audio(&mut self, _frame: &ff_format::AudioFrame) -> Result<(), StreamError> {
            Ok(())
        }

        fn finish(self: Box<Self>) -> Result<(), StreamError> {
            Ok(())
        }
    }

    struct FailOutput;

    impl StreamOutput for FailOutput {
        fn push_video(&mut self, _frame: &VideoFrame) -> Result<(), StreamError> {
            Err(StreamError::InvalidConfig {
                reason: "forced failure".into(),
            })
        }

        fn push_audio(&mut self, _frame: &ff_format::AudioFrame) -> Result<(), StreamError> {
            Err(StreamError::InvalidConfig {
                reason: "forced failure".into(),
            })
        }

        fn finish(self: Box<Self>) -> Result<(), StreamError> {
            Err(StreamError::InvalidConfig {
                reason: "forced failure".into(),
            })
        }
    }

    fn dummy_video_frame() -> VideoFrame {
        VideoFrame::empty(4, 4, PixelFormat::Yuv420p).expect("dummy frame allocation failed")
    }

    fn dummy_audio_frame() -> ff_format::AudioFrame {
        use ff_format::AudioFrame;
        AudioFrame::empty(1024, 2, 44100, SampleFormat::F32p)
            .expect("dummy audio frame allocation failed")
    }

    #[test]
    fn push_video_all_succeed_should_return_ok() {
        let mut fanout = FanoutOutput::new(vec![Box::new(OkOutput), Box::new(OkOutput)]);
        let frame = dummy_video_frame();
        assert!(fanout.push_video(&frame).is_ok());
    }

    #[test]
    fn push_video_one_fails_should_return_fanout_failure() {
        let mut fanout = FanoutOutput::new(vec![Box::new(OkOutput), Box::new(FailOutput)]);
        let frame = dummy_video_frame();
        let result = fanout.push_video(&frame);

        match result {
            Err(StreamError::FanoutFailure {
                failed,
                total,
                messages,
            }) => {
                assert_eq!(failed, 1, "expected 1 failure");
                assert_eq!(total, 2, "expected total=2");
                assert_eq!(messages.len(), 1);
                assert!(messages[0].contains("target[1]"), "got: {:?}", messages);
            }
            other => panic!("expected FanoutFailure, got: {other:?}"),
        }
    }

    #[test]
    fn push_audio_all_succeed_should_return_ok() {
        let mut fanout = FanoutOutput::new(vec![Box::new(OkOutput), Box::new(OkOutput)]);
        let frame = dummy_audio_frame();
        assert!(fanout.push_audio(&frame).is_ok());
    }

    #[test]
    fn push_audio_one_fails_should_return_fanout_failure() {
        let mut fanout = FanoutOutput::new(vec![Box::new(OkOutput), Box::new(FailOutput)]);
        let frame = dummy_audio_frame();
        let result = fanout.push_audio(&frame);
        assert!(
            matches!(
                result,
                Err(StreamError::FanoutFailure {
                    failed: 1,
                    total: 2,
                    ..
                })
            ),
            "got: {result:?}"
        );
    }

    #[test]
    fn finish_all_succeed_should_return_ok() {
        let fanout = FanoutOutput::new(vec![Box::new(OkOutput), Box::new(OkOutput)]);
        assert!(Box::new(fanout).finish().is_ok());
    }

    #[test]
    fn finish_one_fails_should_return_fanout_failure() {
        let fanout = FanoutOutput::new(vec![Box::new(OkOutput), Box::new(FailOutput)]);
        let result = Box::new(fanout).finish();
        assert!(
            matches!(
                result,
                Err(StreamError::FanoutFailure {
                    failed: 1,
                    total: 2,
                    ..
                })
            ),
            "got: {result:?}"
        );
    }

    #[test]
    fn new_with_empty_targets_push_video_should_return_ok() {
        let mut fanout = FanoutOutput::new(vec![]);
        let frame = dummy_video_frame();
        assert!(fanout.push_video(&frame).is_ok());
    }
}
