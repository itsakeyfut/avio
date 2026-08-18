//! Black-frame detection.

#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::AnalysisError;

/// Detects black intervals in a video file and returns their start timestamps.
///
/// Uses `FFmpeg`'s `blackdetect` filter to identify frames or segments where
/// the proportion of "black" pixels exceeds `threshold`.  One [`Duration`] is
/// returned per detected black interval (the start of that interval).
///
/// # Examples
///
/// ```ignore
/// use ff_analysis::BlackFrameDetector;
///
/// let black_starts = BlackFrameDetector::new("video.mp4")
///     .threshold(0.1)
///     .run()?;
///
/// for ts in &black_starts {
///     println!("Black interval starts at {:?}", ts);
/// }
/// ```
pub struct BlackFrameDetector {
    input: PathBuf,
    threshold: f64,
}

impl BlackFrameDetector {
    /// Creates a new detector for the given video file.
    ///
    /// The default threshold is `0.1` (10% of pixels must be below the
    /// blackness cutoff for a frame to count as black).
    pub fn new(input: impl AsRef<Path>) -> Self {
        Self {
            input: input.as_ref().to_path_buf(),
            threshold: 0.1,
        }
    }

    /// Sets the luminance threshold for black-pixel detection.
    ///
    /// Must be in the range `[0.0, 1.0]`.  Higher values make the detector
    /// more permissive (more frames qualify as black); lower values are
    /// stricter.  Passing a value outside this range causes
    /// [`run`](Self::run) to return [`AnalysisError::Failed`].
    ///
    /// Default: `0.1`.
    #[must_use]
    pub fn threshold(self, t: f64) -> Self {
        Self {
            threshold: t,
            ..self
        }
    }

    /// Runs black-frame detection and returns the start [`Duration`] of each
    /// detected black interval.
    ///
    /// # Errors
    ///
    /// - [`AnalysisError::Failed`] — `threshold` outside `[0.0, 1.0]`,
    ///   input file not found, or an internal filter-graph error.
    pub fn run(self) -> Result<Vec<Duration>, AnalysisError> {
        if !(0.0..=1.0).contains(&self.threshold) {
            return Err(AnalysisError::Failed {
                reason: format!("threshold must be in [0.0, 1.0], got {}", self.threshold),
            });
        }
        if !self.input.exists() {
            return Err(AnalysisError::Failed {
                reason: format!("file not found: {}", self.input.display()),
            });
        }
        // SAFETY: detect_black_frames_unsafe manages all raw pointer lifetimes
        // according to the avfilter ownership rules documented in analysis_inner.
        // The path is valid for the duration of the call.
        unsafe { super::analysis_inner::detect_black_frames_unsafe(&self.input, self.threshold) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_frame_detector_invalid_threshold_below_zero_should_return_analysis_failed() {
        let result = BlackFrameDetector::new("irrelevant.mp4")
            .threshold(-0.1)
            .run();
        assert!(
            matches!(result, Err(AnalysisError::Failed { .. })),
            "expected Failed for threshold=-0.1, got {result:?}"
        );
    }

    #[test]
    fn black_frame_detector_invalid_threshold_above_one_should_return_analysis_failed() {
        let result = BlackFrameDetector::new("irrelevant.mp4")
            .threshold(1.1)
            .run();
        assert!(
            matches!(result, Err(AnalysisError::Failed { .. })),
            "expected Failed for threshold=1.1, got {result:?}"
        );
    }

    #[test]
    fn black_frame_detector_missing_file_should_return_analysis_failed() {
        let result = BlackFrameDetector::new("does_not_exist_99999.mp4").run();
        assert!(
            matches!(result, Err(AnalysisError::Failed { .. })),
            "expected Failed for missing file, got {result:?}"
        );
    }

    #[test]
    fn black_frame_detector_boundary_thresholds_should_be_valid() {
        // 0.0 and 1.0 are valid thresholds; errors come from missing file, not threshold.
        let r0 = BlackFrameDetector::new("irrelevant.mp4")
            .threshold(0.0)
            .run();
        let r1 = BlackFrameDetector::new("irrelevant.mp4")
            .threshold(1.0)
            .run();
        assert!(
            matches!(r0, Err(AnalysisError::Failed { .. })),
            "expected Failed (file not found) for threshold=0.0, got {r0:?}"
        );
        assert!(
            matches!(r1, Err(AnalysisError::Failed { .. })),
            "expected Failed (file not found) for threshold=1.0, got {r1:?}"
        );
    }
}
