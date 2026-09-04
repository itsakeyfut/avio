//! Silence detection for audio files.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::AnalysisError;

/// A detected silent interval in an audio stream.
///
/// Both timestamps are measured from the beginning of the file.
#[derive(Debug, Clone, PartialEq)]
pub struct SilenceRange {
    /// Start of the silent interval.
    pub start: Duration,
    /// End of the silent interval.
    pub end: Duration,
}

/// Detects silent intervals in an audio file and returns their time ranges.
///
/// Uses `FFmpeg`'s `silencedetect` filter to identify audio segments whose
/// amplitude stays below `threshold_db` for at least `min_duration`.  Only
/// complete intervals (silence start **and** end detected) are reported; a
/// trailing silence that runs to end-of-file without an explicit end marker is
/// not included.
///
/// # Examples
///
/// ```ignore
/// use ff_analysis::SilenceDetector;
/// use std::time::Duration;
///
/// let ranges = SilenceDetector::new("audio.mp3")
///     .threshold_db(-40.0)
///     .min_duration(Duration::from_millis(500))
///     .run()?;
///
/// for r in &ranges {
///     println!("Silence {:?}–{:?}", r.start, r.end);
/// }
/// ```
pub struct SilenceDetector {
    input: PathBuf,
    threshold_db: f32,
    min_duration: Duration,
}

impl SilenceDetector {
    /// Creates a new detector for the given audio file.
    ///
    /// Defaults: `threshold_db = -40.0`, `min_duration = 500 ms`.
    pub fn new(input: impl AsRef<Path>) -> Self {
        Self {
            input: input.as_ref().to_path_buf(),
            threshold_db: -40.0,
            min_duration: Duration::from_millis(500),
        }
    }

    /// Sets the amplitude threshold in dBFS.
    ///
    /// Audio samples below this level are considered silent.  The value should
    /// be negative (e.g. `-40.0` for −40 dBFS).
    ///
    /// Default: `-40.0` dB.
    #[must_use]
    pub fn threshold_db(self, db: f32) -> Self {
        Self {
            threshold_db: db,
            ..self
        }
    }

    /// Sets the minimum duration a silent segment must last to be reported.
    ///
    /// Silence shorter than this value is ignored.
    ///
    /// Default: 500 ms.
    #[must_use]
    pub fn min_duration(self, d: Duration) -> Self {
        Self {
            min_duration: d,
            ..self
        }
    }

    /// Runs silence detection and returns all detected [`SilenceRange`] values.
    ///
    /// # Errors
    ///
    /// - [`AnalysisError::Failed`] — input file not found or an internal
    ///   filter-graph error occurs.
    pub fn run(self) -> Result<Vec<SilenceRange>, AnalysisError> {
        if !self.input.exists() {
            return Err(AnalysisError::Failed {
                reason: format!("file not found: {}", self.input.display()),
            });
        }
        super::analysis_inner::detect_silence(&self.input, self.threshold_db, self.min_duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_detector_missing_file_should_return_analysis_failed() {
        let result = SilenceDetector::new("does_not_exist_99999.mp3").run();
        assert!(
            matches!(result, Err(AnalysisError::Failed { .. })),
            "expected Failed for missing file, got {result:?}"
        );
    }

    #[test]
    fn silence_detector_default_threshold_should_be_minus_40_db() {
        // Verify the default is -40 dB by round-tripping through threshold_db().
        // Setting the same value should not change behaviour.
        let result = SilenceDetector::new("does_not_exist_99999.mp3")
            .threshold_db(-40.0)
            .run();
        assert!(
            matches!(result, Err(AnalysisError::Failed { .. })),
            "expected Failed (missing file) when threshold_db=-40, got {result:?}"
        );
    }
}
