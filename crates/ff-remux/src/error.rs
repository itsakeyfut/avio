//! Error type for remuxing operations.

use ff_format::{ErrorSeverity, MediaError};
use thiserror::Error;

/// Errors that can occur during stream-copy remuxing (trim, audio replace /
/// extract / add).
#[derive(Error, Debug)]
pub enum RemuxError {
    /// A configuration value is missing or invalid.
    #[error("invalid configuration: {reason}")]
    InvalidConfig {
        /// Human-readable description of the configuration problem.
        reason: String,
    },

    /// A remux operation (trim, extract, replace, add) failed for a structural
    /// reason (e.g. the input has no matching stream, or a mux/remux call failed).
    #[error("remux operation failed: {reason}")]
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

impl RemuxError {
    /// Create an error from a raw `FFmpeg` error code, resolving the message via
    /// `av_strerror`.
    pub(crate) fn from_ffmpeg_error(errnum: i32) -> Self {
        RemuxError::Ffmpeg {
            code: errnum,
            message: ff_sys::av_error_string(errnum),
        }
    }
}

impl MediaError for RemuxError {
    fn severity(&self) -> ErrorSeverity {
        match self {
            Self::Ffmpeg { .. } => ErrorSeverity::Other,
            Self::InvalidConfig { .. } | Self::OperationFailed { .. } | Self::Io(_) => {
                ErrorSeverity::Fatal
            }
        }
    }
}
