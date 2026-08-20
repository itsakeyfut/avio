//! Encoding progress tracking.

use std::time::Duration;

/// Encoding progress information.
///
/// Provides real-time information about the encoding process,
/// including frames encoded, bytes written, and time estimates.
#[derive(Debug, Clone)]
pub struct EncodeProgress {
    /// Number of frames encoded so far
    pub frames_encoded: u64,

    /// Total number of frames to encode (if known)
    pub total_frames: Option<u64>,

    /// Number of bytes written to output file
    pub bytes_written: u64,

    /// Current encoding bitrate (bits per second)
    pub current_bitrate: u64,

    /// Time elapsed since encoding started
    pub elapsed: Duration,

    /// Estimated remaining time (if known)
    pub remaining: Option<Duration>,

    /// Current encoding FPS (frames per second).
    ///
    /// Calculated as: `frames_encoded / elapsed_seconds`
    ///
    /// This represents the actual encoding speed, which may differ from
    /// the target video FPS. Higher values indicate faster encoding.
    pub current_fps: f64,
}

impl EncodeProgress {
    /// Calculate progress percentage (0.0 - 100.0).
    ///
    /// Returns 0.0 if total frames is unknown.
    #[must_use]
    pub fn percent(&self) -> f64 {
        match self.total_frames {
            Some(total) if total > 0 => {
                #[allow(clippy::cast_precision_loss)]
                let percent = self.frames_encoded as f64 / total as f64 * 100.0;
                percent
            }
            _ => 0.0,
        }
    }
}

/// `EncodeProgress` callback trait for monitoring encoding progress.
///
/// Implement this trait to receive real-time encoding progress updates
/// and optionally support encoding cancellation.
///
/// # Examples
///
/// ```ignore
/// use ff_encode::{EncodeProgress, EncodeProgressCallback};
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicBool, Ordering};
///
/// struct MyEncodeProgressHandler {
///     cancelled: Arc<AtomicBool>,
/// }
///
/// impl EncodeProgressCallback for MyEncodeProgressHandler {
///     fn on_progress(&mut self, progress: &EncodeProgress) {
///         println!("Encoded {} frames at {:.1} fps",
///             progress.frames_encoded,
///             progress.current_fps
///         );
///     }
///
///     fn should_cancel(&self) -> bool {
///         self.cancelled.load(Ordering::Relaxed)
///     }
/// }
/// ```
pub trait EncodeProgressCallback: Send {
    /// Called when encoding progress is updated.
    ///
    /// This method is called periodically during encoding to report progress.
    /// The frequency of calls depends on the encoding speed and frame rate.
    ///
    /// # Arguments
    ///
    /// * `progress` - Current encoding progress information
    fn on_progress(&mut self, progress: &EncodeProgress);

    /// Check if encoding should be cancelled.
    ///
    /// The encoder will check this method periodically during encoding.
    /// Return `true` to request cancellation of the encoding process.
    ///
    /// # Returns
    ///
    /// `true` to cancel encoding, `false` to continue
    ///
    /// # Default Implementation
    ///
    /// The default implementation returns `false` (never cancel).
    fn should_cancel(&self) -> bool {
        false
    }
}

/// Implement `EncodeProgressCallback` for closures.
///
/// This allows using simple closures as progress callbacks without
/// needing to define a custom struct implementing the trait.
impl<F> EncodeProgressCallback for F
where
    F: FnMut(&EncodeProgress) + Send,
{
    fn on_progress(&mut self, progress: &EncodeProgress) {
        self(progress);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_percent() {
        let progress = EncodeProgress {
            frames_encoded: 50,
            total_frames: Some(100),
            bytes_written: 1_000_000,
            current_bitrate: 8_000_000,
            elapsed: Duration::from_secs(2),
            remaining: Some(Duration::from_secs(2)),
            current_fps: 25.0,
        };

        assert!((progress.percent() - 50.0).abs() < 0.001);
    }

    #[test]
    fn test_progress_percent_unknown_total() {
        let progress = EncodeProgress {
            frames_encoded: 50,
            total_frames: None,
            bytes_written: 1_000_000,
            current_bitrate: 8_000_000,
            elapsed: Duration::from_secs(2),
            remaining: None,
            current_fps: 25.0,
        };

        assert!((progress.percent() - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_progress_percent_zero_total() {
        let progress = EncodeProgress {
            frames_encoded: 0,
            total_frames: Some(0),
            bytes_written: 0,
            current_bitrate: 0,
            elapsed: Duration::from_secs(0),
            remaining: None,
            current_fps: 0.0,
        };

        assert!((progress.percent() - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_progress_callback_closure() {
        let mut called = false;
        let mut callback = |progress: &EncodeProgress| {
            called = true;
            assert_eq!(progress.frames_encoded, 42);
        };

        let progress = EncodeProgress {
            frames_encoded: 42,
            total_frames: Some(100),
            bytes_written: 500_000,
            current_bitrate: 4_000_000,
            elapsed: Duration::from_secs(1),
            remaining: Some(Duration::from_secs(1)),
            current_fps: 42.0,
        };

        callback.on_progress(&progress);
        assert!(called);
    }

    #[test]
    fn test_progress_callback_should_cancel_default() {
        let callback = |_progress: &EncodeProgress| {};
        assert!(!callback.should_cancel());
    }

    #[test]
    fn test_progress_callback_custom_impl() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        struct TestCallback {
            counter: Arc<AtomicU64>,
            cancelled: Arc<AtomicBool>,
        }

        impl EncodeProgressCallback for TestCallback {
            fn on_progress(&mut self, progress: &EncodeProgress) {
                self.counter
                    .store(progress.frames_encoded, Ordering::Relaxed);
            }

            fn should_cancel(&self) -> bool {
                self.cancelled.load(Ordering::Relaxed)
            }
        }

        let counter = Arc::new(AtomicU64::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));

        let mut callback = TestCallback {
            counter: counter.clone(),
            cancelled: cancelled.clone(),
        };

        let progress = EncodeProgress {
            frames_encoded: 100,
            total_frames: Some(200),
            bytes_written: 1_000_000,
            current_bitrate: 8_000_000,
            elapsed: Duration::from_secs(2),
            remaining: Some(Duration::from_secs(2)),
            current_fps: 50.0,
        };

        // Test progress callback
        callback.on_progress(&progress);
        assert_eq!(counter.load(Ordering::Relaxed), 100);

        // Test cancellation
        assert!(!callback.should_cancel());
        cancelled.store(true, Ordering::Relaxed);
        assert!(callback.should_cancel());
    }
}
