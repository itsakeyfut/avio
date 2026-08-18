//! Error types for filter graph operations.

use ff_format::{ErrorSeverity, MediaError};
use thiserror::Error;

/// Errors that can occur during filter graph construction and processing.
#[derive(Debug, Error)]
pub enum FilterError {
    /// Failed to build the filter graph (invalid filter chain or `FFmpeg` error
    /// during graph creation).
    #[error("failed to build filter graph")]
    BuildFailed,

    /// A frame processing operation (push or pull) failed.
    #[error("failed to process frame")]
    ProcessFailed,

    /// An invalid configuration was detected during graph construction.
    #[error("invalid filter configuration: {reason}")]
    InvalidConfig {
        /// Human-readable reason for the failure.
        reason: String,
    },

    /// A frame was pushed to an invalid input slot.
    #[error("invalid input: slot={slot} reason={reason}")]
    InvalidInput {
        /// The slot index that was out of range or otherwise invalid.
        slot: usize,
        /// Human-readable reason for the failure.
        reason: String,
    },

    /// An underlying `FFmpeg` function returned an error code.
    #[error("ffmpeg error: {message} (code={code})")]
    Ffmpeg {
        /// The raw `FFmpeg` error code.
        code: i32,
        /// Human-readable description of the error.
        message: String,
    },

    /// A multi-track composition or mixing operation failed.
    ///
    /// Returned by [`MultiTrackComposer::build`](crate::MultiTrackComposer::build) and
    /// [`MultiTrackAudioMixer::build`](crate::MultiTrackAudioMixer::build) when the
    /// `FFmpeg` filter graph cannot be constructed.
    #[error("composition failed: {reason}")]
    CompositionFailed {
        /// Human-readable reason for the failure.
        reason: String,
    },

    /// An analysis operation failed for a structural reason.
    ///
    /// Returned by [`LoudnessMeter::measure`](crate::analysis::LoudnessMeter::measure)
    /// when the input file is not found, the format is unsupported, or the
    /// `FFmpeg` filter graph cannot be constructed.
    #[error("analysis failed: {reason}")]
    AnalysisFailed {
        /// Human-readable reason for the failure.
        reason: String,
    },

    /// A function requires a GPL-licensed `FFmpeg` filter but the `gpl` feature
    /// flag is not enabled.
    ///
    /// Enable the `gpl` feature in `Cargo.toml` and ensure your distribution
    /// complies with the GPL licence before using this function.
    #[error("GPL-licensed feature required: {feature} (enable the `gpl` feature flag)")]
    GplRequired {
        /// Name of the filter or capability that requires GPL.
        feature: &'static str,
    },
}

impl MediaError for FilterError {
    fn severity(&self) -> ErrorSeverity {
        match self {
            Self::Ffmpeg { .. } | Self::ProcessFailed | Self::InvalidInput { .. } => {
                ErrorSeverity::Other
            }
            Self::BuildFailed
            | Self::InvalidConfig { .. }
            | Self::CompositionFailed { .. }
            | Self::AnalysisFailed { .. }
            | Self::GplRequired { .. } => ErrorSeverity::Fatal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FilterError;
    use std::error::Error;

    #[test]
    fn build_failed_should_display_correct_message() {
        let err = FilterError::BuildFailed;
        assert_eq!(err.to_string(), "failed to build filter graph");
    }

    #[test]
    fn process_failed_should_display_correct_message() {
        let err = FilterError::ProcessFailed;
        assert_eq!(err.to_string(), "failed to process frame");
    }

    #[test]
    fn invalid_input_should_display_slot_and_reason() {
        let err = FilterError::InvalidInput {
            slot: 2,
            reason: "slot out of range".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "invalid input: slot=2 reason=slot out of range"
        );
    }

    #[test]
    fn ffmpeg_should_display_code_and_message() {
        let err = FilterError::Ffmpeg {
            code: -22,
            message: "Invalid argument".to_string(),
        };
        assert_eq!(err.to_string(), "ffmpeg error: Invalid argument (code=-22)");
    }

    #[test]
    fn composition_failed_should_display_reason() {
        let err = FilterError::CompositionFailed {
            reason: "no layers".to_string(),
        };
        assert_eq!(err.to_string(), "composition failed: no layers");
    }

    #[test]
    fn analysis_failed_should_display_reason() {
        let err = FilterError::AnalysisFailed {
            reason: "file not found".to_string(),
        };
        assert_eq!(err.to_string(), "analysis failed: file not found");
    }

    #[test]
    fn gpl_required_should_display_feature_name() {
        let err = FilterError::GplRequired {
            feature: "rubberband",
        };
        assert_eq!(
            err.to_string(),
            "GPL-licensed feature required: rubberband (enable the `gpl` feature flag)"
        );
    }

    #[test]
    fn filter_error_should_implement_std_error() {
        fn assert_error<E: Error>(_: &E) {}
        assert_error(&FilterError::BuildFailed);
        assert_error(&FilterError::ProcessFailed);
        assert_error(&FilterError::InvalidInput {
            slot: 0,
            reason: String::new(),
        });
        assert_error(&FilterError::Ffmpeg {
            code: 0,
            message: String::new(),
        });
    }
}
