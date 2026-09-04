//! Video quality metrics (SSIM, PSNR).

use std::path::Path;

use crate::FilterError;

/// Computes video quality metrics between a reference and a distorted video.
///
/// All methods are static — there is no state to configure.
pub struct QualityMetrics;

impl QualityMetrics {
    /// Computes the mean SSIM (Structural Similarity Index Measure) over all
    /// frames between `reference` and `distorted`.
    ///
    /// Returns a value in `[0.0, 1.0]`:
    /// - `1.0` — the inputs are frame-identical.
    /// - `0.0` — no structural similarity.
    ///
    /// Uses `FFmpeg`'s `ssim` filter internally.  Both inputs must have the
    /// same frame count; if they differ the function returns an error rather
    /// than silently comparing only the overlapping portion.
    ///
    /// # Errors
    ///
    /// - [`FilterError::AnalysisFailed`] — either input file is not found, the
    ///   inputs have different frame counts, or the internal filter graph fails.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_filter::QualityMetrics;
    ///
    /// // Compare a video against itself — should return ≈ 1.0.
    /// let ssim = QualityMetrics::ssim("reference.mp4", "reference.mp4")?;
    /// assert!(ssim > 0.9999);
    /// ```
    pub fn ssim(
        reference: impl AsRef<Path>,
        distorted: impl AsRef<Path>,
    ) -> Result<f32, FilterError> {
        let reference = reference.as_ref();
        let distorted = distorted.as_ref();

        if !reference.exists() {
            return Err(FilterError::AnalysisFailed {
                reason: format!("reference file not found: {}", reference.display()),
            });
        }
        if !distorted.exists() {
            return Err(FilterError::AnalysisFailed {
                reason: format!("distorted file not found: {}", distorted.display()),
            });
        }
        super::analysis_inner::compute_ssim(reference, distorted)
    }

    /// Computes the mean PSNR (Peak Signal-to-Noise Ratio, in dB) over all
    /// frames between `reference` and `distorted`.
    ///
    /// Uses the luminance (Y-plane) PSNR as the representative value.
    ///
    /// - Identical inputs → `f32::INFINITY` (MSE = 0).
    /// - Lightly compressed → typically > 40 dB.
    /// - Heavy degradation → typically < 30 dB.
    ///
    /// Uses `FFmpeg`'s `psnr` filter internally.  Both inputs must have the
    /// same frame count; if they differ the function returns an error.
    ///
    /// # Errors
    ///
    /// - [`FilterError::AnalysisFailed`] — either input file is not found, the
    ///   inputs have different frame counts, or the internal filter graph fails.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_filter::QualityMetrics;
    ///
    /// // Compare a video against itself — should return infinity.
    /// let psnr = QualityMetrics::psnr("reference.mp4", "reference.mp4")?;
    /// assert!(psnr > 100.0 || psnr == f32::INFINITY);
    /// ```
    pub fn psnr(
        reference: impl AsRef<Path>,
        distorted: impl AsRef<Path>,
    ) -> Result<f32, FilterError> {
        let reference = reference.as_ref();
        let distorted = distorted.as_ref();

        if !reference.exists() {
            return Err(FilterError::AnalysisFailed {
                reason: format!("reference file not found: {}", reference.display()),
            });
        }
        if !distorted.exists() {
            return Err(FilterError::AnalysisFailed {
                reason: format!("distorted file not found: {}", distorted.display()),
            });
        }
        super::analysis_inner::compute_psnr(reference, distorted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_metrics_ssim_missing_reference_should_return_analysis_failed() {
        let result = QualityMetrics::ssim("does_not_exist_ref.mp4", "does_not_exist_dist.mp4");
        assert!(
            matches!(result, Err(FilterError::AnalysisFailed { .. })),
            "expected AnalysisFailed for missing reference, got {result:?}"
        );
    }

    #[test]
    fn quality_metrics_ssim_missing_distorted_should_return_analysis_failed() {
        // Reference exists (any existing file), distorted does not.
        // Use a path that is guaranteed to exist: the Cargo.toml for this crate.
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let result = QualityMetrics::ssim(&manifest, "does_not_exist_dist_99999.mp4");
        assert!(
            matches!(result, Err(FilterError::AnalysisFailed { .. })),
            "expected AnalysisFailed for missing distorted, got {result:?}"
        );
    }

    #[test]
    fn quality_metrics_psnr_missing_reference_should_return_analysis_failed() {
        let result = QualityMetrics::psnr("does_not_exist_ref.mp4", "does_not_exist_dist.mp4");
        assert!(
            matches!(result, Err(FilterError::AnalysisFailed { .. })),
            "expected AnalysisFailed for missing reference, got {result:?}"
        );
    }

    #[test]
    fn quality_metrics_psnr_missing_distorted_should_return_analysis_failed() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let result = QualityMetrics::psnr(&manifest, "does_not_exist_dist_99999.mp4");
        assert!(
            matches!(result, Err(FilterError::AnalysisFailed { .. })),
            "expected AnalysisFailed for missing distorted, got {result:?}"
        );
    }
}
