//! Video stabilization — two-pass motion analysis and correction.

use std::path::Path;

use crate::FilterError;

/// Options for the first stabilization pass (motion analysis).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzeOptions {
    /// Motion shakiness level 1–10 (default: 5).
    pub shakiness: u8,
    /// Detection accuracy 1–15 (default: 15, highest quality).
    pub accuracy: u8,
    /// Step size for motion search in pixels 1–32 (default: 6).
    pub stepsize: u8,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self {
            shakiness: 5,
            accuracy: 15,
            stepsize: 6,
        }
    }
}

/// Interpolation algorithm used by [`Stabilizer::transform`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    /// Bilinear interpolation (faster, default).
    Bilinear,
    /// Bicubic interpolation (higher quality, slower).
    Bicubic,
}

/// Options for the second stabilization pass (transform application).
#[derive(Debug, Clone, PartialEq)]
pub struct StabilizeOptions {
    /// Temporal smoothing radius in frames 0–500 (default: 10).
    pub smoothing: u16,
    /// Fill stabilization borders with black instead of previous-frame content
    /// (default: true).
    pub crop_black: bool,
    /// Zoom factor: 0.0 = no zoom, positive = fixed zoom-in (default: 0.0).
    pub zoom: f32,
    /// Optimal zoom: 0 = disabled, 1 = auto-static, 2 = adaptive (default: 0).
    pub optzoom: u8,
    /// Pixel interpolation algorithm (default: [`Interpolation::Bilinear`]).
    pub interpol: Interpolation,
}

impl Default for StabilizeOptions {
    fn default() -> Self {
        Self {
            smoothing: 10,
            crop_black: true,
            zoom: 0.0,
            optzoom: 0,
            interpol: Interpolation::Bilinear,
        }
    }
}

impl StabilizeOptions {
    /// Set the fixed zoom-in factor (0.0 = no zoom).
    #[must_use]
    pub fn zoom(mut self, z: f32) -> Self {
        self.zoom = z;
        self
    }

    /// Set the auto-zoom mode: 0 = disabled, 1 = static, 2 = adaptive.
    ///
    /// Values outside 0–2 are clamped.
    #[must_use]
    pub fn optzoom(mut self, mode: u8) -> Self {
        self.optzoom = mode.clamp(0, 2);
        self
    }

    /// Set the sub-pixel interpolation algorithm used during frame warping.
    #[must_use]
    pub fn interpol(mut self, i: Interpolation) -> Self {
        self.interpol = i;
        self
    }
}

/// Two-pass video stabilization using `FFmpeg`'s `vidstabdetect` /
/// `vidstabtransform` filters.
///
/// **Pass 1**: [`Stabilizer::analyze`] — motion analysis, produces a `.trf` file.
/// **Pass 2**: [`Stabilizer::transform`] — correction, consumes the `.trf` file.
pub struct Stabilizer;

impl Stabilizer {
    /// Analyze motion in `input` and write the transform file to `output_trf`.
    ///
    /// Runs a self-contained `FFmpeg` filter graph:
    /// `movie → vidstabdetect → buffersink`.
    /// The resulting `.trf` file is consumed by [`Stabilizer::transform`] in pass 2.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Ffmpeg`] if:
    /// - `vidstabdetect` is not available in the linked `FFmpeg` build.
    /// - The input file is unreadable or does not exist.
    /// - The filter graph cannot be configured or the `.trf` file cannot be written.
    pub fn analyze(
        input: &Path,
        output_trf: &Path,
        opts: &AnalyzeOptions,
    ) -> Result<(), FilterError> {
        super::effects_inner::analyze_vidstab(input, output_trf, opts)
    }

    /// Apply motion transforms from the `.trf` file produced by [`Stabilizer::analyze`].
    ///
    /// Reads `input`, applies `vidstabtransform`, and writes the stabilized video
    /// to `output` (re-encoded with the best available H.264 encoder).
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Ffmpeg`] if:
    /// - `vidstabtransform` is not available in the linked `FFmpeg` build.
    /// - `trf_path` does not exist or is unreadable.
    /// - The input file is unreadable or does not exist.
    /// - The output file cannot be created or encoded.
    pub fn transform(
        input: &Path,
        trf_path: &Path,
        output: &Path,
        opts: &StabilizeOptions,
    ) -> Result<(), FilterError> {
        super::effects_inner::transform_vidstab(input, trf_path, output, opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_options_default_should_have_expected_values() {
        let opts = AnalyzeOptions::default();
        assert_eq!(opts.shakiness, 5);
        assert_eq!(opts.accuracy, 15);
        assert_eq!(opts.stepsize, 6);
    }

    #[test]
    fn stabilize_options_default_should_have_expected_values() {
        let opts = StabilizeOptions::default();
        assert_eq!(opts.smoothing, 10);
        assert!(opts.crop_black);
        assert!((opts.zoom - 0.0_f32).abs() < f32::EPSILON);
        assert_eq!(opts.optzoom, 0);
        assert_eq!(opts.interpol, Interpolation::Bilinear);
    }

    #[test]
    fn zoom_builder_should_set_zoom_field() {
        let opts = StabilizeOptions::default().zoom(1.5);
        assert!((opts.zoom - 1.5_f32).abs() < f32::EPSILON);
    }

    #[test]
    fn optzoom_builder_should_set_optzoom_field() {
        let opts = StabilizeOptions::default().optzoom(1);
        assert_eq!(opts.optzoom, 1);
    }

    #[test]
    fn optzoom_builder_should_clamp_above_maximum_to_two() {
        let opts = StabilizeOptions::default().optzoom(5);
        assert_eq!(opts.optzoom, 2);
    }

    #[test]
    fn interpol_builder_should_set_interpol_field() {
        let opts = StabilizeOptions::default().interpol(Interpolation::Bicubic);
        assert_eq!(opts.interpol, Interpolation::Bicubic);
    }
}
