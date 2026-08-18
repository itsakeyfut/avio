//! Error type for media-analysis operations.

use ff_decode::DecodeError;
use ff_format::{ErrorSeverity, MediaError};
use thiserror::Error;

/// Errors that can occur during media analysis (scene / silence / BPM /
/// histogram / keyframe / black-frame / waveform).
#[derive(Error, Debug)]
pub enum AnalysisError {
    /// An analysis operation failed for a structural reason (e.g. a zero
    /// interval, a missing stream, or an unsupported format).
    #[error("analysis failed: {reason}")]
    Failed {
        /// Human-readable description of why the analysis failed.
        reason: String,
    },

    /// BPM detection failed for a structural reason (e.g. no audio stream, or a
    /// clip too short to analyse).
    ///
    /// Reserved for the planned tempo detector (spectral flux + autocorrelation).
    #[error("BPM detection failed: {reason}")]
    BpmDetectionFailed {
        /// Human-readable description of why BPM detection failed.
        reason: String,
    },

    /// An error propagated from the underlying decoder.
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

impl MediaError for AnalysisError {
    /// Decoder-propagated errors keep the decoder's own classification; the
    /// analysis-specific failures are fatal (the operation cannot proceed).
    fn severity(&self) -> ErrorSeverity {
        match self {
            Self::Decode(e) => e.severity(),
            Self::Failed { .. } | Self::BpmDetectionFailed { .. } => ErrorSeverity::Fatal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_should_display_reason() {
        let e = AnalysisError::Failed {
            reason: "interval must be non-zero".to_string(),
        };
        let msg = e.to_string();
        assert!(msg.contains("analysis failed"), "unexpected message: {msg}");
        assert!(
            msg.contains("interval must be non-zero"),
            "expected reason in message: {msg}"
        );
    }

    #[test]
    fn failed_should_be_fatal_and_not_recoverable() {
        let e = AnalysisError::Failed {
            reason: "zero interval".to_string(),
        };
        assert!(e.is_fatal());
        assert!(!e.is_recoverable());
    }

    #[test]
    fn bpm_detection_failed_should_display_reason() {
        let e = AnalysisError::BpmDetectionFailed {
            reason: "no audio stream".to_string(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("BPM detection failed"),
            "unexpected message: {msg}"
        );
        assert!(
            msg.contains("no audio stream"),
            "expected reason in message: {msg}"
        );
    }

    #[test]
    fn bpm_detection_failed_should_be_fatal_and_not_recoverable() {
        let e = AnalysisError::BpmDetectionFailed {
            reason: "clip too short".to_string(),
        };
        assert!(e.is_fatal());
        assert!(!e.is_recoverable());
    }

    #[test]
    fn decode_variant_should_delegate_severity_to_inner() {
        // A recoverable decoder error must remain recoverable through the wrapper.
        let inner = DecodeError::decoding_failed("corrupt frame");
        let recoverable = inner.is_recoverable();
        let e = AnalysisError::Decode(inner);
        assert_eq!(e.is_recoverable(), recoverable);
        assert!(e.is_recoverable());
    }

    #[test]
    fn decode_variant_should_convert_via_from() {
        let e: AnalysisError = DecodeError::FileNotFound {
            path: std::path::PathBuf::from("missing.mp4"),
        }
        .into();
        assert!(matches!(e, AnalysisError::Decode(_)));
        assert!(e.is_fatal());
    }
}
