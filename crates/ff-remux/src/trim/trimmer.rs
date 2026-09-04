//! Stream-copy trimming — cut a media file to a time range without re-encoding.

use std::path::PathBuf;
use std::time::Duration;

use crate::error::RemuxError;

use super::trim_inner::{self, BsfSpec};

/// Trim a media file to a time range using stream copy (no re-encode).
///
/// Uses [`avformat_seek_file`] to seek to the start point, then copies packets
/// until the presentation timestamp exceeds the end point.  All streams
/// (video, audio, subtitles) are copied verbatim from the input.
///
/// # Example
///
/// ```ignore
/// use ff_remux::StreamCopyTrimmer;
///
/// StreamCopyTrimmer::new("input.mp4", 2.0, 7.0, "output.mp4")
///     .run()?;
/// ```
///
/// [`avformat_seek_file`]: https://ffmpeg.org/doxygen/trunk/group__lavf__decoding.html
pub struct StreamCopyTrimmer {
    input: PathBuf,
    output: PathBuf,
    start_sec: f64,
    end_sec: f64,
    bsf: BsfSpec,
}

impl StreamCopyTrimmer {
    /// Create a new `StreamCopyTrimmer`.
    ///
    /// `start_sec` and `end_sec` are absolute timestamps in seconds measured
    /// from the start of the source file.  [`run`](Self::run) returns
    /// [`RemuxError::InvalidConfig`] if `start_sec >= end_sec`.
    pub fn new(
        input: impl Into<PathBuf>,
        start_sec: f64,
        end_sec: f64,
        output: impl Into<PathBuf>,
    ) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
            start_sec,
            end_sec,
            bsf: BsfSpec::default(),
        }
    }

    /// Applies a bitstream filter chain to every video stream.
    ///
    /// `spec` is the syntax `ffmpeg -bsf` takes: a comma-separated chain whose
    /// elements may carry options, e.g. `"dump_extra"` or
    /// `"h264_metadata=level=40,extract_extradata"`.
    ///
    /// This is only for filters `FFmpeg` does **not** apply on its own. libavformat
    /// already inserts the filter a container requires — copying H.264 from MP4 into
    /// MPEG-TS produces Annex B with nothing set here — so this exists for the
    /// explicit ones (`extract_extradata`, `dump_extra`, the `*_metadata` family).
    /// See ADR-0011.
    ///
    /// An unregistered or malformed spec fails in [`run`](Self::run) with
    /// [`RemuxError::InvalidConfig`].
    #[must_use]
    pub fn video_bsf(mut self, spec: impl Into<String>) -> Self {
        self.bsf.video = Some(spec.into());
        self
    }

    /// Applies a bitstream filter chain to every audio stream.
    ///
    /// See [`video_bsf`](Self::video_bsf) for the spec syntax and when to reach for
    /// this at all.
    #[must_use]
    pub fn audio_bsf(mut self, spec: impl Into<String>) -> Self {
        self.bsf.audio = Some(spec.into());
        self
    }

    /// Execute the trim operation.
    ///
    /// # Errors
    ///
    /// - [`RemuxError::InvalidConfig`] if `start_sec >= end_sec`.
    /// - [`RemuxError::Ffmpeg`] if any `FFmpeg` API call fails.
    pub fn run(self) -> Result<(), RemuxError> {
        if self.start_sec >= self.end_sec {
            return Err(RemuxError::InvalidConfig {
                reason: format!(
                    "start_sec ({}) must be less than end_sec ({})",
                    self.start_sec, self.end_sec
                ),
            });
        }
        log::debug!(
            "stream copy trim start input={} output={} start_sec={} end_sec={}",
            self.input.display(),
            self.output.display(),
            self.start_sec,
            self.end_sec,
        );
        trim_inner::run_trim(
            &self.input,
            &self.output,
            self.start_sec,
            self.end_sec,
            &self.bsf,
        )
    }
}

// StreamCopyTrim

/// Trim a media file to a time range using stream copy (no re-encode).
///
/// Equivalent to [`StreamCopyTrimmer`] but accepts [`Duration`] for `start` and
/// `end` instead of raw seconds, and returns
/// [`RemuxError::OperationFailed`] when the time range is invalid.
///
/// # Example
///
/// ```ignore
/// use ff_remux::StreamCopyTrim;
/// use std::time::Duration;
///
/// StreamCopyTrim::new(
///     "input.mp4",
///     Duration::from_secs(2),
///     Duration::from_secs(7),
///     "output.mp4",
/// )
/// .run()?;
/// ```
pub struct StreamCopyTrim {
    input: PathBuf,
    start: Duration,
    end: Duration,
    output: PathBuf,
    bsf: BsfSpec,
}

impl StreamCopyTrim {
    /// Create a new `StreamCopyTrim`.
    ///
    /// `start` and `end` are absolute timestamps measured from the start of
    /// the source file.  [`run`](Self::run) returns
    /// [`RemuxError::OperationFailed`] if `start >= end`.
    pub fn new(
        input: impl Into<PathBuf>,
        start: Duration,
        end: Duration,
        output: impl Into<PathBuf>,
    ) -> Self {
        Self {
            input: input.into(),
            start,
            end,
            output: output.into(),
            bsf: BsfSpec::default(),
        }
    }

    /// Applies a bitstream filter chain to every video stream.
    ///
    /// See [`StreamCopyTrimmer::video_bsf`] for the spec syntax and when it is needed.
    #[must_use]
    pub fn video_bsf(mut self, spec: impl Into<String>) -> Self {
        self.bsf.video = Some(spec.into());
        self
    }

    /// Applies a bitstream filter chain to every audio stream.
    ///
    /// See [`StreamCopyTrimmer::video_bsf`] for the spec syntax and when it is needed.
    #[must_use]
    pub fn audio_bsf(mut self, spec: impl Into<String>) -> Self {
        self.bsf.audio = Some(spec.into());
        self
    }

    /// Execute the trim operation.
    ///
    /// # Errors
    ///
    /// - [`RemuxError::OperationFailed`] if `start >= end`.
    /// - [`RemuxError::Ffmpeg`] if any `FFmpeg` API call fails.
    pub fn run(self) -> Result<(), RemuxError> {
        if self.start >= self.end {
            return Err(RemuxError::OperationFailed {
                reason: format!(
                    "start ({:?}) must be less than end ({:?})",
                    self.start, self.end
                ),
            });
        }
        let start_sec = self.start.as_secs_f64();
        let end_sec = self.end.as_secs_f64();
        log::debug!(
            "stream copy trim start input={} output={} start_sec={start_sec} end_sec={end_sec}",
            self.input.display(),
            self.output.display(),
        );
        trim_inner::run_trim(&self.input, &self.output, start_sec, end_sec, &self.bsf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_copy_trimmer_should_reject_start_greater_than_end() {
        let result = StreamCopyTrimmer::new("input.mp4", 7.0, 2.0, "output.mp4").run();
        assert!(
            matches!(result, Err(RemuxError::InvalidConfig { .. })),
            "expected InvalidConfig for start > end, got {result:?}"
        );
    }

    #[test]
    fn stream_copy_trimmer_should_reject_equal_start_and_end() {
        let result = StreamCopyTrimmer::new("input.mp4", 5.0, 5.0, "output.mp4").run();
        assert!(
            matches!(result, Err(RemuxError::InvalidConfig { .. })),
            "expected InvalidConfig for start == end, got {result:?}"
        );
    }

    #[test]
    fn stream_copy_trim_should_reject_start_greater_than_end() {
        let result = StreamCopyTrim::new(
            "input.mp4",
            Duration::from_secs(7),
            Duration::from_secs(2),
            "output.mp4",
        )
        .run();
        assert!(
            matches!(result, Err(RemuxError::OperationFailed { .. })),
            "expected MediaOperationFailed for start > end, got {result:?}"
        );
    }

    #[test]
    fn stream_copy_trim_should_reject_equal_start_and_end() {
        let result = StreamCopyTrim::new(
            "input.mp4",
            Duration::from_secs(5),
            Duration::from_secs(5),
            "output.mp4",
        )
        .run();
        assert!(
            matches!(result, Err(RemuxError::OperationFailed { .. })),
            "expected MediaOperationFailed for start == end, got {result:?}"
        );
    }
}
