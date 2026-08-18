//! Error type for preview-image generation (sprite sheet / GIF preview).

use ff_format::{ErrorSeverity, MediaError};
use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur while generating a sprite sheet or GIF preview.
#[derive(Error, Debug)]
pub enum PreviewImageError {
    /// The output file could not be created.
    #[error("Cannot create output file: {path}")]
    CannotCreateFile {
        /// File path that failed.
        path: PathBuf,
    },

    /// The requested codec is not available in this `FFmpeg` build.
    #[error("Unsupported codec: {codec}")]
    UnsupportedCodec {
        /// Codec name.
        codec: String,
    },

    /// A preview-generation operation failed for a structural reason (e.g. zero
    /// rows/cols, or an internal filter-graph/encode step failed).
    #[error("preview operation failed: {reason}")]
    OperationFailed {
        /// Human-readable description of the failure.
        reason: String,
    },

    /// An underlying `FFmpeg` function returned an error code.
    #[error("ffmpeg error: {message} (code={code})")]
    Ffmpeg {
        /// Raw `FFmpeg` error code (negative integer). `0` when no numeric code is available.
        code: i32,
        /// Human-readable error message from `av_strerror` or an internal description.
        message: String,
    },

    /// An I/O error occurred.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl PreviewImageError {
    /// Create an error from a raw `FFmpeg` error code, resolving the message via
    /// `av_strerror`.
    pub(crate) fn from_ffmpeg_error(errnum: i32) -> Self {
        PreviewImageError::Ffmpeg {
            code: errnum,
            message: ff_sys::av_error_string(errnum),
        }
    }
}

impl MediaError for PreviewImageError {
    fn severity(&self) -> ErrorSeverity {
        match self {
            Self::Ffmpeg { .. } => ErrorSeverity::Other,
            Self::CannotCreateFile { .. }
            | Self::UnsupportedCodec { .. }
            | Self::OperationFailed { .. }
            | Self::Io(_) => ErrorSeverity::Fatal,
        }
    }
}
