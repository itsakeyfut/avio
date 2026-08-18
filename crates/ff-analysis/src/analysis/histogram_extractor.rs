//! Per-channel color histogram extraction for video files.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ff_format::PixelFormat;

use ff_decode::VideoDecoder;

use crate::AnalysisError;

/// Per-channel color histogram for a single video frame.
///
/// Each array has 256 bins (one per 8-bit intensity level).  For an `N × M`
/// frame the sum of any channel's bins equals `N × M`.
///
/// Luma is computed as `Y = 0.299 R + 0.587 G + 0.114 B` (BT.601 coefficients).
#[derive(Debug, Clone)]
pub struct FrameHistogram {
    /// Presentation timestamp of the sampled frame.
    pub timestamp: Duration,
    /// Red-channel bin counts.
    pub r: [u32; 256],
    /// Green-channel bin counts.
    pub g: [u32; 256],
    /// Blue-channel bin counts.
    pub b: [u32; 256],
    /// Luma bin counts (BT.601 weighted average of R, G, B).
    pub luma: [u32; 256],
}

/// Extracts per-channel color histograms at configurable frame intervals.
///
/// Decodes the input video via [`VideoDecoder`] with `RGB24` output conversion
/// so that histogram accumulation is a simple one-pass loop with no additional
/// format dispatch.  `FFmpeg`'s `histogram` filter is deliberately **not** used
/// because it produces video output rather than structured data.
///
/// # Examples
///
/// ```ignore
/// use ff_analysis::HistogramExtractor;
///
/// let histograms = HistogramExtractor::new("video.mp4")
///     .interval_frames(30)
///     .run()?;
///
/// for h in &histograms {
///     println!("Frame at {:?}: r[255]={}", h.timestamp, h.r[255]);
/// }
/// ```
pub struct HistogramExtractor {
    input: PathBuf,
    interval_frames: u32,
}

impl HistogramExtractor {
    /// Creates a new extractor for the given video file.
    ///
    /// The default sampling interval is every frame (`interval_frames = 1`).
    /// Call [`interval_frames`](Self::interval_frames) to sample less frequently.
    pub fn new(input: impl AsRef<Path>) -> Self {
        Self {
            input: input.as_ref().to_path_buf(),
            interval_frames: 1,
        }
    }

    /// Sets the frame sampling interval.
    ///
    /// A value of `N` means one histogram is computed per `N` decoded frames.
    /// For example, `interval_frames(30)` on a 30 fps video yields roughly one
    /// histogram per second.
    ///
    /// Passing `0` causes [`run`](Self::run) to return
    /// [`AnalysisError::Failed`].
    ///
    /// Default: `1` (every frame).
    #[must_use]
    pub fn interval_frames(self, n: u32) -> Self {
        Self {
            interval_frames: n,
            ..self
        }
    }

    /// Runs histogram extraction and returns one [`FrameHistogram`] per
    /// sampled frame.
    ///
    /// Frames are decoded as RGB24 internally; all pixel format conversion is
    /// handled by `FFmpeg`'s software scaler.
    ///
    /// # Errors
    ///
    /// - [`AnalysisError::Failed`] — `interval_frames` is `0`, the input
    ///   file is not found, or a decode error occurs.
    /// - Any [`ff_decode::DecodeError`] propagated from [`VideoDecoder`],
    ///   wrapped in [`AnalysisError::Decode`].
    pub fn run(self) -> Result<Vec<FrameHistogram>, AnalysisError> {
        if self.interval_frames == 0 {
            return Err(AnalysisError::Failed {
                reason: "interval_frames must be non-zero".to_string(),
            });
        }
        if !self.input.exists() {
            return Err(AnalysisError::Failed {
                reason: format!("file not found: {}", self.input.display()),
            });
        }

        let mut decoder = VideoDecoder::open(&self.input)
            .output_format(PixelFormat::Rgb24)
            .build()?;

        let mut results: Vec<FrameHistogram> = Vec::new();
        let mut frame_index: u32 = 0;

        while let Some(frame) = decoder.decode_one()? {
            if frame_index.is_multiple_of(self.interval_frames)
                && let Some(hist) = compute_rgb24_histogram(&frame)
            {
                results.push(hist);
            }
            frame_index += 1;
        }

        log::debug!("histogram extraction complete frames={}", results.len());
        Ok(results)
    }
}

/// Computes R, G, B, and luma histograms for a single `RGB24` frame.
///
/// Returns `None` when the frame is not `RGB24` or when plane data is
/// unavailable.
pub(super) fn compute_rgb24_histogram(frame: &ff_format::VideoFrame) -> Option<FrameHistogram> {
    if frame.format() != PixelFormat::Rgb24 {
        return None;
    }
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let plane = frame.plane(0)?;
    let stride = frame.stride(0)?;

    let mut r = [0u32; 256];
    let mut g = [0u32; 256];
    let mut b = [0u32; 256];
    let mut luma = [0u32; 256];

    for row in 0..height {
        let row_start = row * stride;
        for col in 0..width {
            let offset = row_start + col * 3;
            let rv = plane[offset];
            let gv = plane[offset + 1];
            let bv = plane[offset + 2];
            // f32 can represent all u8 values exactly (mantissa is 23 bits, u8 needs only 8).
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let lv = (0.299_f32
                .mul_add(
                    f32::from(rv),
                    0.587_f32.mul_add(f32::from(gv), 0.114 * f32::from(bv)),
                )
                .round() as usize)
                .min(255);
            r[usize::from(rv)] += 1;
            g[usize::from(gv)] += 1;
            b[usize::from(bv)] += 1;
            luma[lv] += 1;
        }
    }

    Some(FrameHistogram {
        timestamp: frame.timestamp().as_duration(),
        r,
        g,
        b,
        luma,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_extractor_missing_file_should_return_analysis_failed() {
        let result = HistogramExtractor::new("does_not_exist_99999.mp4").run();
        assert!(
            matches!(result, Err(AnalysisError::Failed { .. })),
            "expected Failed for missing file, got {result:?}"
        );
    }

    #[test]
    fn histogram_extractor_zero_interval_should_return_analysis_failed() {
        let result = HistogramExtractor::new("irrelevant.mp4")
            .interval_frames(0)
            .run();
        assert!(
            matches!(result, Err(AnalysisError::Failed { .. })),
            "expected Failed for interval_frames=0, got {result:?}"
        );
    }

    #[test]
    fn histogram_solid_red_frame_should_have_r255_peak() {
        use ff_format::{PixelFormat, PooledBuffer, Timestamp, VideoFrame};

        let w = 4u32;
        let h = 4u32;
        let stride = w as usize * 3;
        // Solid red: R=255, G=0, B=0.
        let mut data = vec![0u8; stride * h as usize];
        for pixel in data.chunks_mut(3) {
            pixel[0] = 255;
        }
        let frame = VideoFrame::new(
            vec![PooledBuffer::standalone(data)],
            vec![stride],
            w,
            h,
            PixelFormat::Rgb24,
            Timestamp::default(),
            false,
        )
        .unwrap();

        let hist = compute_rgb24_histogram(&frame).unwrap();
        let total = w * h;
        assert_eq!(
            hist.r[255], total,
            "r[255] should equal total pixels for solid-red frame"
        );
        assert_eq!(
            hist.g[0], total,
            "g[0] should equal total pixels for solid-red frame"
        );
        assert_eq!(
            hist.b[0], total,
            "b[0] should equal total pixels for solid-red frame"
        );
    }

    #[test]
    fn histogram_bin_sum_should_equal_total_pixels() {
        use ff_format::{PixelFormat, PooledBuffer, Timestamp, VideoFrame};

        let w = 8u32;
        let h = 6u32;
        let stride = w as usize * 3;
        let mut data = vec![0u8; stride * h as usize];
        for (i, pixel) in data.chunks_mut(3).enumerate() {
            pixel[0] = (i.wrapping_mul(17) % 256) as u8;
            pixel[1] = (i.wrapping_mul(37) % 256) as u8;
            pixel[2] = (i.wrapping_mul(53) % 256) as u8;
        }
        let frame = VideoFrame::new(
            vec![PooledBuffer::standalone(data)],
            vec![stride],
            w,
            h,
            PixelFormat::Rgb24,
            Timestamp::default(),
            false,
        )
        .unwrap();

        let hist = compute_rgb24_histogram(&frame).unwrap();
        let total = w * h;
        assert_eq!(
            hist.r.iter().sum::<u32>(),
            total,
            "r bin sum should equal total pixels"
        );
        assert_eq!(
            hist.g.iter().sum::<u32>(),
            total,
            "g bin sum should equal total pixels"
        );
        assert_eq!(
            hist.b.iter().sum::<u32>(),
            total,
            "b bin sum should equal total pixels"
        );
        assert_eq!(
            hist.luma.iter().sum::<u32>(),
            total,
            "luma bin sum should equal total pixels"
        );
    }
}
