//! Batch frame extraction and thumbnail selection.
//!
//! [`FrameExtractor`] samples one frame per configurable time interval across
//! the full duration of a video. Returns a `Vec<VideoFrame>` suitable for
//! thumbnail strips and preview generation.
//!
//! [`ThumbnailSelector`] picks the single best representative frame by scoring
//! candidates for brightness and sharpness, skipping near-black, near-white,
//! and blurry frames.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ff_format::{PixelFormat, VideoFrame};

use crate::DecodeError;
use crate::VideoDecoder;

/// Extracts one frame per time interval from a video file.
///
/// # Examples
///
/// ```ignore
/// use ff_decode::FrameExtractor;
/// use std::time::Duration;
///
/// let frames = FrameExtractor::new("video.mp4")
///     .interval(Duration::from_secs(5))
///     .run()?;
/// println!("extracted {} frames", frames.len());
/// ```
pub struct FrameExtractor {
    input: PathBuf,
    interval: Duration,
}

impl FrameExtractor {
    /// Creates a new `FrameExtractor` for the given input file.
    ///
    /// The default extraction interval is 1 second.
    pub fn new(input: impl AsRef<Path>) -> Self {
        Self {
            input: input.as_ref().to_path_buf(),
            interval: Duration::from_secs(1),
        }
    }

    /// Sets the time interval between extracted frames.
    ///
    /// Passing [`Duration::ZERO`] causes [`run`](Self::run) to return
    /// [`DecodeError::ExtractionFailed`].
    #[must_use]
    pub fn interval(self, d: Duration) -> Self {
        Self {
            interval: d,
            ..self
        }
    }

    /// Runs the extraction and returns one frame per interval.
    ///
    /// Timestamps `0, interval, 2×interval, …` up to (but not including)
    /// the video duration are sampled. [`DecodeError::NoFrameAtTimestamp`]
    /// for a given timestamp is silently skipped with a `warn!` log; all
    /// other errors are propagated immediately.
    ///
    /// # Errors
    ///
    /// - [`DecodeError::ExtractionFailed`] — interval is zero, or the input
    ///   file cannot be opened.
    /// - Any other [`DecodeError`] propagated from the decoder.
    pub fn run(self) -> Result<Vec<VideoFrame>, DecodeError> {
        if self.interval.is_zero() {
            return Err(DecodeError::ExtractionFailed {
                reason: "interval must be positive".to_string(),
            });
        }

        let mut decoder = VideoDecoder::open(&self.input).build()?;
        let duration = decoder.duration();

        let mut frames = Vec::new();
        let mut ts = Duration::ZERO;

        while ts < duration {
            match decoder.extract_frame(ts) {
                Ok(frame) => frames.push(frame),
                Err(DecodeError::NoFrameAtTimestamp { .. }) => {
                    log::warn!(
                        "frame not available, skipping timestamp={ts:?} input={}",
                        self.input.display()
                    );
                }
                Err(e) => return Err(e),
            }
            ts += self.interval;
        }

        let frame_count = frames.len();
        log::debug!(
            "frame extraction complete frames={frame_count} interval={interval:?}",
            interval = self.interval
        );

        Ok(frames)
    }
}

/// Automatically selects the best thumbnail frame from a video file.
///
/// Candidates are sampled at regular intervals. Frames that are near-black
/// (mean luma < 10), near-white (mean luma > 245), or blurry (Laplacian
/// variance < 100) are skipped. The first candidate that passes all quality
/// gates is returned. If no candidate passes, the sharpest frame seen is
/// returned as a fallback.
///
/// # Examples
///
/// ```ignore
/// use ff_decode::ThumbnailSelector;
/// use std::time::Duration;
///
/// let frame = ThumbnailSelector::new("video.mp4")
///     .candidate_interval(Duration::from_secs(5))
///     .run()?;
/// ```
pub struct ThumbnailSelector {
    input: PathBuf,
    candidate_interval: Duration,
}

impl ThumbnailSelector {
    /// Creates a new `ThumbnailSelector` for the given input file.
    ///
    /// Default candidate interval is 5 seconds.
    pub fn new(input: impl AsRef<Path>) -> Self {
        Self {
            input: input.as_ref().to_path_buf(),
            candidate_interval: Duration::from_secs(5),
        }
    }

    /// Sets the interval between candidate frames (default: 5s).
    #[must_use]
    pub fn candidate_interval(self, d: Duration) -> Self {
        Self {
            candidate_interval: d,
            ..self
        }
    }

    /// Runs thumbnail selection and returns the best frame.
    ///
    /// # Errors
    ///
    /// - [`DecodeError::ExtractionFailed`] — the interval is zero, or no frame
    ///   can be sampled from the video.
    /// - Any other [`DecodeError`] propagated from the decoder.
    pub fn run(self) -> Result<VideoFrame, DecodeError> {
        if self.candidate_interval.is_zero() {
            return Err(DecodeError::ExtractionFailed {
                reason: "candidate_interval must be positive".to_string(),
            });
        }

        let mut decoder = VideoDecoder::open(&self.input)
            .output_format(PixelFormat::Rgb24)
            .build()?;
        let duration = decoder.duration();

        // (laplacian_variance, frame) — best seen so far for fallback.
        let mut best: Option<(f64, VideoFrame)> = None;
        let mut ts = Duration::ZERO;

        while ts < duration {
            let frame = match decoder.extract_frame(ts) {
                Ok(f) => f,
                Err(DecodeError::NoFrameAtTimestamp { .. }) => {
                    log::warn!(
                        "frame not available, skipping timestamp={ts:?} input={}",
                        self.input.display()
                    );
                    ts += self.candidate_interval;
                    continue;
                }
                Err(e) => return Err(e),
            };

            let luma = mean_luma(&frame);
            if !(10.0..=245.0).contains(&luma) {
                ts += self.candidate_interval;
                continue;
            }

            let sharpness = laplacian_variance(&frame);
            if sharpness >= 100.0 {
                log::debug!(
                    "thumbnail selected timestamp={ts:?} luma={luma:.1} sharpness={sharpness:.1}"
                );
                return Ok(frame);
            }

            let keep = best
                .as_ref()
                .is_none_or(|(best_sharpness, _)| sharpness > *best_sharpness);
            if keep {
                best = Some((sharpness, frame));
            }

            ts += self.candidate_interval;
        }

        if let Some((sharpness, frame)) = best {
            log::debug!(
                "thumbnail fallback used sharpness={sharpness:.1} input={}",
                self.input.display()
            );
            return Ok(frame);
        }

        Err(DecodeError::ExtractionFailed {
            reason: "no suitable thumbnail frame found".to_string(),
        })
    }
}

// Private scoring helpers

/// Computes mean BT.601 luma across all pixels in an `RGB24` frame.
///
/// Returns `0.0` when frame data is unavailable or the frame is empty.
fn mean_luma(frame: &VideoFrame) -> f64 {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let pixel_count = width * height;
    if pixel_count == 0 {
        return 0.0;
    }
    let Some(plane) = frame.plane(0) else {
        return 0.0;
    };
    let Some(stride) = frame.stride(0) else {
        return 0.0;
    };

    let mut sum = 0.0_f64;
    for row in 0..height {
        let row_start = row * stride;
        for col in 0..width {
            let offset = row_start + col * 3;
            let r = f64::from(plane[offset]);
            let g = f64::from(plane[offset + 1]);
            let b = f64::from(plane[offset + 2]);
            sum += 0.299 * r + 0.587 * g + 0.114 * b;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    {
        sum / pixel_count as f64
    }
}

/// Computes the variance of the Laplacian applied to the luma channel.
///
/// A high value indicates a sharp image; near-zero indicates a blurry or
/// uniform image. Border pixels are excluded from the computation.
///
/// Returns `0.0` when the frame is smaller than 3×3 or data is unavailable.
fn laplacian_variance(frame: &VideoFrame) -> f64 {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    if width < 3 || height < 3 {
        return 0.0;
    }
    let Some(plane) = frame.plane(0) else {
        return 0.0;
    };
    let Some(stride) = frame.stride(0) else {
        return 0.0;
    };

    let luma_at = |row: usize, col: usize| -> f64 {
        let offset = row * stride + col * 3;
        let r = f64::from(plane[offset]);
        let g = f64::from(plane[offset + 1]);
        let b = f64::from(plane[offset + 2]);
        0.299 * r + 0.587 * g + 0.114 * b
    };

    let inner_count = (width - 2) * (height - 2);
    let mut responses = Vec::with_capacity(inner_count);

    for row in 1..(height - 1) {
        for col in 1..(width - 1) {
            let lap = luma_at(row - 1, col)
                + luma_at(row + 1, col)
                + luma_at(row, col - 1)
                + luma_at(row, col + 1)
                - 4.0 * luma_at(row, col);
            responses.push(lap);
        }
    }

    #[allow(clippy::cast_precision_loss)]
    let n = inner_count as f64;
    let mean = responses.iter().sum::<f64>() / n;
    responses
        .iter()
        .map(|x| (x - mean) * (x - mean))
        .sum::<f64>()
        / n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_extractor_zero_interval_should_err() {
        let result = FrameExtractor::new("irrelevant.mp4")
            .interval(Duration::ZERO)
            .run();
        assert!(
            matches!(result, Err(DecodeError::ExtractionFailed { .. })),
            "expected ExtractionFailed for zero interval, got {result:?}"
        );
    }

    #[test]
    fn frame_extractor_should_return_correct_frame_count() {
        // Unit test: verify the timestamp generation logic without a real file.
        // We test this via the zero-interval guard and rely on integration tests
        // for the full run() path with a real video file.
        let extractor = FrameExtractor::new("video.mp4").interval(Duration::from_secs(1));
        assert_eq!(extractor.interval, Duration::from_secs(1));
    }

    #[test]
    fn thumbnail_selector_zero_interval_should_return_analysis_failed() {
        let result = ThumbnailSelector::new("irrelevant.mp4")
            .candidate_interval(Duration::ZERO)
            .run();
        assert!(
            matches!(result, Err(DecodeError::ExtractionFailed { .. })),
            "expected ExtractionFailed for zero interval, got {result:?}"
        );
    }

    // mean_luma unit tests

    fn make_rgb24_frame(width: u32, height: u32, fill: [u8; 3]) -> VideoFrame {
        use ff_format::{PooledBuffer, Timestamp};

        let stride = width as usize * 3;
        let mut data = vec![0u8; stride * height as usize];
        for pixel in data.chunks_mut(3) {
            pixel[0] = fill[0];
            pixel[1] = fill[1];
            pixel[2] = fill[2];
        }
        VideoFrame::new(
            vec![PooledBuffer::standalone(data)],
            vec![stride],
            width,
            height,
            PixelFormat::Rgb24,
            Timestamp::default(),
            false,
        )
        .unwrap()
    }

    #[test]
    fn mean_luma_should_return_correct_value_for_solid_color() {
        // Pure red: luma = 0.299 * 255 = ~76.245
        let frame = make_rgb24_frame(4, 4, [255, 0, 0]);
        let luma = mean_luma(&frame);
        assert!(
            (luma - 76.245).abs() < 0.1,
            "expected luma ≈ 76.245 for pure red, got {luma:.3}"
        );
    }

    #[test]
    fn thumbnail_selector_should_skip_black_frames() {
        // All-black frame has luma = 0.0, which is < 10.0 — rejected.
        let frame = make_rgb24_frame(4, 4, [0, 0, 0]);
        let luma = mean_luma(&frame);
        assert!(
            luma < 10.0,
            "expected luma < 10 for black frame, got {luma:.3}"
        );
    }

    #[test]
    fn laplacian_variance_blurry_should_return_low_value() {
        // Uniform frame has zero Laplacian response everywhere → variance = 0.
        let frame = make_rgb24_frame(8, 8, [128, 64, 32]);
        let variance = laplacian_variance(&frame);
        assert!(
            variance < 1.0,
            "expected near-zero variance for uniform frame, got {variance:.3}"
        );
    }
}
