//! EBU R128 loudness measurement.

use std::path::{Path, PathBuf};

use crate::FilterError;

/// Result of an EBU R128 loudness measurement.
///
/// Values are computed by `FFmpeg`'s `ebur128` filter over the entire
/// duration of the input file.
#[derive(Debug, Clone, PartialEq)]
pub struct LoudnessResult {
    /// Integrated loudness in LUFS (ITU-R BS.1770-4).
    ///
    /// [`f32::NEG_INFINITY`] when the audio is silence or the measurement
    /// could not be computed.
    pub integrated_lufs: f32,
    /// Loudness range (LRA) in LU.
    pub lra: f32,
    /// True peak in dBTP.
    ///
    /// [`f32::NEG_INFINITY`] when the measurement could not be computed.
    pub true_peak_dbtp: f32,
}

/// Measures EBU R128 integrated loudness, loudness range, and true peak.
///
/// Uses `FFmpeg`'s `ebur128=metadata=1:peak=true` filter graph internally.
/// The analysis is self-contained — no external decoder is required.
///
/// # Examples
///
/// ```ignore
/// use ff_filter::LoudnessMeter;
///
/// let result = LoudnessMeter::new("audio.mp3").measure()?;
/// println!("Integrated: {:.1} LUFS", result.integrated_lufs);
/// println!("LRA: {:.1} LU", result.lra);
/// println!("True peak: {:.1} dBTP", result.true_peak_dbtp);
/// ```
pub struct LoudnessMeter {
    input: PathBuf,
}

impl LoudnessMeter {
    /// Creates a new meter for the given audio or video file.
    pub fn new(input: impl AsRef<Path>) -> Self {
        Self {
            input: input.as_ref().to_path_buf(),
        }
    }

    /// Runs EBU R128 loudness analysis and returns the result.
    ///
    /// # Errors
    ///
    /// - [`FilterError::AnalysisFailed`] — input file not found, unsupported
    ///   format, or the filter graph could not be constructed.
    pub fn measure(self) -> Result<LoudnessResult, FilterError> {
        if !self.input.exists() {
            return Err(FilterError::AnalysisFailed {
                reason: format!("file not found: {}", self.input.display()),
            });
        }
        super::analysis_inner::measure_loudness(&self.input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudness_meter_missing_file_should_return_analysis_failed() {
        let result = LoudnessMeter::new("does_not_exist_99999.mp3").measure();
        assert!(
            matches!(result, Err(FilterError::AnalysisFailed { .. })),
            "expected AnalysisFailed for missing file, got {result:?}"
        );
    }
}
