//! Error types for ff-preview.

use std::path::PathBuf;

use ff_format::{ErrorSeverity, MediaError};
use thiserror::Error;

/// Errors that can occur during preview and proxy operations.
#[derive(Debug, Error)]
pub enum PreviewError {
    /// The media file was not found at the specified path.
    #[error("file not found: path={path}")]
    FileNotFound {
        /// Path that was not found.
        path: PathBuf,
    },

    /// The media file has no video stream.
    #[error("no video stream found: path={path}")]
    NoVideoStream {
        /// Path to the media file.
        path: PathBuf,
    },

    /// A seek operation failed.
    #[error("seek failed: target={target:?} reason={reason}")]
    SeekFailed {
        /// Target timestamp of the failed seek.
        target: std::time::Duration,
        /// Human-readable reason for the failure.
        reason: String,
    },

    /// An underlying decode error occurred.
    #[error("decode failed: {0}")]
    Decode(#[from] ff_decode::DecodeError),

    /// A raw `FFmpeg` error.
    ///
    /// `code` is the negative integer returned by the `FFmpeg` API, or `0` when no
    /// numeric code is available. `message` is from `av_strerror` or an internal
    /// description.
    #[error("ffmpeg error: {message} (code={code})")]
    Ffmpeg {
        /// Raw `FFmpeg` error code (negative i32). `0` when no numeric code is available.
        code: i32,
        /// Human-readable message from `av_strerror` or an internal description.
        message: String,
    },

    /// A probe error while analysing the media file.
    #[error("probe failed: {0}")]
    Probe(#[from] ff_probe::ProbeError),

    /// A proxy generation pipeline error.
    #[cfg(feature = "proxy")]
    #[error("pipeline failed: {0}")]
    Pipeline(#[from] ff_pipeline::PipelineError),

    /// An I/O error during file operations.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A seek target lies outside the valid range of the timeline.
    #[error("seek out of range: pts={pts:?}")]
    SeekOutOfRange {
        /// The requested presentation timestamp that fell outside all clips.
        pts: std::time::Duration,
    },
}

impl MediaError for PreviewError {
    fn severity(&self) -> ErrorSeverity {
        match self {
            Self::Decode(e) => e.severity(),
            Self::Probe(e) => e.severity(),
            #[cfg(feature = "proxy")]
            Self::Pipeline(e) => e.severity(),
            Self::SeekFailed { .. } => ErrorSeverity::Recoverable,
            Self::Ffmpeg { .. } | Self::SeekOutOfRange { .. } => ErrorSeverity::Other,
            Self::FileNotFound { .. } | Self::NoVideoStream { .. } | Self::Io(_) => {
                ErrorSeverity::Fatal
            }
        }
    }
}
