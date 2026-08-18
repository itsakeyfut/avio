//! Preview generation — sprite sheets and animated GIFs.
//!
//! [`SpriteSheet`] samples evenly-spaced frames from a video and tiles them
//! into a single PNG image suitable for video-player scrub-bar hover previews.
//!
//! [`GifPreview`] generates an animated GIF from a configurable time range
//! using FFmpeg's two-pass `palettegen` + `paletteuse` approach.

mod error;
mod preview_inner;

pub use error::PreviewImageError;

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Generates a thumbnail sprite sheet from a video file.
///
/// Frames are sampled at evenly-spaced intervals across the full video
/// duration and tiled into a single PNG image of size
/// `cols × frame_width` × `rows × frame_height`.
///
/// # Examples
///
/// ```ignore
/// use ff_encode::SpriteSheet;
///
/// SpriteSheet::new("video.mp4")
///     .cols(5)
///     .rows(4)
///     .frame_width(160)
///     .frame_height(90)
///     .output("sprites.png")
///     .run()?;
/// ```
pub struct SpriteSheet {
    input: PathBuf,
    cols: u32,
    rows: u32,
    frame_width: u32,
    frame_height: u32,
    output: PathBuf,
}

impl SpriteSheet {
    /// Creates a new `SpriteSheet` for the given input file.
    ///
    /// Defaults: `cols=10`, `rows=10`, `frame_width=160`, `frame_height=90`,
    /// no output path set.
    pub fn new(input: impl AsRef<Path>) -> Self {
        Self {
            input: input.as_ref().to_path_buf(),
            cols: 10,
            rows: 10,
            frame_width: 160,
            frame_height: 90,
            output: PathBuf::new(),
        }
    }

    /// Sets the number of columns in the sprite grid (default: 10).
    #[must_use]
    pub fn cols(self, n: u32) -> Self {
        Self { cols: n, ..self }
    }

    /// Sets the number of rows in the sprite grid (default: 10).
    #[must_use]
    pub fn rows(self, n: u32) -> Self {
        Self { rows: n, ..self }
    }

    /// Sets the width of each individual thumbnail frame in pixels (default: 160).
    #[must_use]
    pub fn frame_width(self, w: u32) -> Self {
        Self {
            frame_width: w,
            ..self
        }
    }

    /// Sets the height of each individual thumbnail frame in pixels (default: 90).
    #[must_use]
    pub fn frame_height(self, h: u32) -> Self {
        Self {
            frame_height: h,
            ..self
        }
    }

    /// Sets the output path for the generated PNG file.
    #[must_use]
    pub fn output(self, path: impl AsRef<Path>) -> Self {
        Self {
            output: path.as_ref().to_path_buf(),
            ..self
        }
    }

    /// Runs the sprite sheet generation.
    ///
    /// Output image dimensions: `cols × frame_width` × `rows × frame_height`.
    ///
    /// # Errors
    ///
    /// - [`PreviewImageError::OperationFailed`] — `cols` or `rows` is zero,
    ///   `frame_width` or `frame_height` is zero, or `output` path is not set.
    /// - [`PreviewImageError::Ffmpeg`] — any FFmpeg filter graph or encoding call fails.
    pub fn run(self) -> Result<(), PreviewImageError> {
        if self.cols == 0 || self.rows == 0 {
            return Err(PreviewImageError::OperationFailed {
                reason: "cols/rows must be > 0".to_string(),
            });
        }
        if self.frame_width == 0 || self.frame_height == 0 {
            return Err(PreviewImageError::OperationFailed {
                reason: "frame_width/frame_height must be > 0".to_string(),
            });
        }
        if self.output.as_os_str().is_empty() {
            return Err(PreviewImageError::OperationFailed {
                reason: "output path not set".to_string(),
            });
        }
        preview_inner::generate_sprite_sheet(
            &self.input,
            self.cols,
            self.rows,
            self.frame_width,
            self.frame_height,
            &self.output,
        )
    }
}

/// Generates an animated GIF preview from a configurable time range.
///
/// Uses FFmpeg's two-pass `palettegen` + `paletteuse` approach for
/// high-quality colour fidelity within GIF's 256-colour limit.
///
/// # Examples
///
/// ```ignore
/// use ff_encode::GifPreview;
/// use std::time::Duration;
///
/// GifPreview::new("video.mp4")
///     .start(Duration::from_secs(10))
///     .duration(Duration::from_secs(3))
///     .fps(15.0)
///     .width(480)
///     .output("preview.gif")
///     .run()?;
/// ```
pub struct GifPreview {
    input: PathBuf,
    start: Duration,
    duration: Duration,
    fps: f64,
    width: u32,
    output: PathBuf,
}

impl GifPreview {
    /// Creates a new `GifPreview` for the given input file.
    ///
    /// Defaults: `start=0s`, `duration=3s`, `fps=10.0`, `width=320`,
    /// no output path set.
    pub fn new(input: impl AsRef<Path>) -> Self {
        Self {
            input: input.as_ref().to_path_buf(),
            start: Duration::ZERO,
            duration: Duration::from_secs(3),
            fps: 10.0,
            width: 320,
            output: PathBuf::new(),
        }
    }

    /// Sets the start time within the video (default: 0s).
    #[must_use]
    pub fn start(self, t: Duration) -> Self {
        Self { start: t, ..self }
    }

    /// Sets the duration of the GIF clip (default: 3s).
    #[must_use]
    pub fn duration(self, d: Duration) -> Self {
        Self {
            duration: d,
            ..self
        }
    }

    /// Sets the output frame rate in frames per second (default: 10.0).
    #[must_use]
    pub fn fps(self, fps: f64) -> Self {
        Self { fps, ..self }
    }

    /// Sets the output width in pixels (default: 320). Height is scaled
    /// proportionally, rounded to an even number.
    #[must_use]
    pub fn width(self, w: u32) -> Self {
        Self { width: w, ..self }
    }

    /// Sets the output path for the generated GIF file.
    ///
    /// The path must have a `.gif` extension.
    #[must_use]
    pub fn output(self, path: impl AsRef<Path>) -> Self {
        Self {
            output: path.as_ref().to_path_buf(),
            ..self
        }
    }

    /// Runs the GIF generation.
    ///
    /// # Errors
    ///
    /// - [`PreviewImageError::OperationFailed`] — output path not set, output
    ///   extension is not `.gif`, `fps` ≤ 0, or `width` is zero.
    /// - [`PreviewImageError::Ffmpeg`] — any FFmpeg filter graph or encoding call fails.
    pub fn run(self) -> Result<(), PreviewImageError> {
        if self.output.as_os_str().is_empty() {
            return Err(PreviewImageError::OperationFailed {
                reason: "output path not set".to_string(),
            });
        }
        if self.output.extension().and_then(|e| e.to_str()) != Some("gif") {
            return Err(PreviewImageError::OperationFailed {
                reason: "output path must have .gif extension".to_string(),
            });
        }
        if self.fps <= 0.0 {
            return Err(PreviewImageError::OperationFailed {
                reason: "fps must be positive".to_string(),
            });
        }
        if self.width == 0 {
            return Err(PreviewImageError::OperationFailed {
                reason: "width must be > 0".to_string(),
            });
        }
        preview_inner::generate_gif_preview(
            &self.input,
            self.start,
            self.duration,
            self.fps,
            self.width,
            &self.output,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprite_sheet_zero_cols_should_return_media_operation_failed() {
        let result = SpriteSheet::new("irrelevant.mp4")
            .cols(0)
            .output("out.png")
            .run();
        assert!(
            matches!(result, Err(PreviewImageError::OperationFailed { .. })),
            "expected OperationFailed for cols=0, got {result:?}"
        );
    }

    #[test]
    fn sprite_sheet_zero_frame_width_should_return_media_operation_failed() {
        let result = SpriteSheet::new("irrelevant.mp4")
            .frame_width(0)
            .output("out.png")
            .run();
        assert!(
            matches!(result, Err(PreviewImageError::OperationFailed { .. })),
            "expected OperationFailed for frame_width=0, got {result:?}"
        );
    }

    #[test]
    fn sprite_sheet_missing_output_should_return_media_operation_failed() {
        let result = SpriteSheet::new("irrelevant.mp4").run();
        assert!(
            matches!(result, Err(PreviewImageError::OperationFailed { .. })),
            "expected OperationFailed for empty output path, got {result:?}"
        );
    }

    #[test]
    fn gif_preview_non_gif_extension_should_return_media_operation_failed() {
        let result = GifPreview::new("irrelevant.mp4").output("out.mp4").run();
        assert!(
            matches!(result, Err(PreviewImageError::OperationFailed { .. })),
            "expected OperationFailed for non-.gif extension, got {result:?}"
        );
    }

    #[test]
    fn gif_preview_missing_output_should_return_media_operation_failed() {
        let result = GifPreview::new("irrelevant.mp4").run();
        assert!(
            matches!(result, Err(PreviewImageError::OperationFailed { .. })),
            "expected OperationFailed for missing output path, got {result:?}"
        );
    }

    #[test]
    fn gif_preview_zero_fps_should_return_media_operation_failed() {
        let result = GifPreview::new("irrelevant.mp4")
            .fps(0.0)
            .output("out.gif")
            .run();
        assert!(
            matches!(result, Err(PreviewImageError::OperationFailed { .. })),
            "expected OperationFailed for fps=0, got {result:?}"
        );
    }
}
