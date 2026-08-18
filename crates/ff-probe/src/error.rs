//! Error types for media probing.

use ff_format::{ErrorSeverity, MediaError};
use std::path::PathBuf;
use thiserror::Error;

/// Error type for media probing operations.
#[derive(Error, Debug)]
pub enum ProbeError {
    /// The specified file was not found.
    #[error("File not found: {path}")]
    FileNotFound {
        /// Path to the file that was not found.
        path: PathBuf,
    },

    /// The file could not be opened.
    #[error("Cannot open file: {path} - {reason}")]
    CannotOpen {
        /// Path to the file that could not be opened.
        path: PathBuf,
        /// Reason why the file could not be opened.
        reason: String,
    },

    /// The file is not a valid media file.
    #[error("Invalid media file: {path} - {reason}")]
    InvalidMedia {
        /// Path to the invalid media file.
        path: PathBuf,
        /// Reason why the file is invalid.
        reason: String,
    },

    /// No streams were found in the file.
    #[error("No streams found in file: {path}")]
    NoStreams {
        /// Path to the file with no streams.
        path: PathBuf,
    },

    /// An I/O error occurred.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// An `FFmpeg` error occurred.
    #[error("ffmpeg error: {message} (code={code})")]
    Ffmpeg {
        /// Raw `FFmpeg` error code (negative integer). `0` when no numeric code is available.
        code: i32,
        /// Human-readable error message from `av_strerror` or an internal description.
        message: String,
    },
}

impl MediaError for ProbeError {
    fn severity(&self) -> ErrorSeverity {
        match self {
            Self::Ffmpeg { .. } => ErrorSeverity::Other,
            Self::FileNotFound { .. }
            | Self::CannotOpen { .. }
            | Self::InvalidMedia { .. }
            | Self::NoStreams { .. }
            | Self::Io(_) => ErrorSeverity::Fatal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_not_found_error() {
        let err = ProbeError::FileNotFound {
            path: PathBuf::from("/path/to/missing.mp4"),
        };
        let msg = err.to_string();
        assert!(msg.contains("File not found"));
        assert!(msg.contains("missing.mp4"));
    }

    #[test]
    fn test_cannot_open_error() {
        let err = ProbeError::CannotOpen {
            path: PathBuf::from("/path/to/file.mp4"),
            reason: "permission denied".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Cannot open file"));
        assert!(msg.contains("permission denied"));
    }

    #[test]
    fn test_invalid_media_error() {
        let err = ProbeError::InvalidMedia {
            path: PathBuf::from("/path/to/bad.mp4"),
            reason: "corrupted header".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Invalid media file"));
        assert!(msg.contains("corrupted header"));
    }

    #[test]
    fn test_no_streams_error() {
        let err = ProbeError::NoStreams {
            path: PathBuf::from("/path/to/empty.mp4"),
        };
        let msg = err.to_string();
        assert!(msg.contains("No streams found"));
    }

    #[test]
    fn ffmpeg_should_display_code_and_message() {
        let err = ProbeError::Ffmpeg {
            code: -2,
            message: "codec not found".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("ffmpeg error"));
        assert!(msg.contains("codec not found"));
        assert!(msg.contains("code=-2"));
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: ProbeError = io_err.into();
        assert!(matches!(err, ProbeError::Io(_)));
    }

    #[test]
    fn probe_io_should_be_fatal() {
        let e: ProbeError = std::io::Error::other("x").into();
        assert!(e.is_fatal() && !e.is_recoverable());
    }

    #[test]
    fn probe_ffmpeg_should_be_other() {
        let e = ProbeError::Ffmpeg {
            code: -22,
            message: "x".into(),
        };
        assert!(!e.is_fatal() && !e.is_recoverable());
    }
}
