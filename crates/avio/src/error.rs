//! Error type for the editing model ([`Timeline`](crate::Timeline) /
//! [`Clip`](crate::Clip) derivation and rendering).

use thiserror::Error;

/// Errors from building or rendering the editing model.
///
/// Wraps the underlying primitive errors from decode / filter / encode via
/// `#[from]`, and adds the model-level failures (`ClipNotFound`, `NoInput`,
/// `Cancelled`, `TimelineRenderFailed`).
#[derive(Debug, Error)]
pub enum TimelineError {
    /// A decoding step failed (e.g. probing a clip for its duration).
    #[error("decode failed: {0}")]
    Decode(#[from] ff_decode::DecodeError),

    /// A filter graph step failed (composition or per-clip effects).
    #[error("filter failed: {0}")]
    Filter(#[from] ff_filter::FilterError),

    /// An encoding step failed while writing the rendered output.
    #[error("encode failed: {0}")]
    Encode(#[from] ff_encode::EncodeError),

    /// No input was provided to the timeline builder — both the video and audio
    /// track lists were empty.
    #[error("no input specified")]
    NoInput,

    /// The render was cancelled by the progress callback returning `false`.
    #[error("timeline render cancelled by caller")]
    Cancelled,

    /// A [`Timeline::render`](crate::Timeline::render) call failed for a
    /// structural reason not covered by a nested variant.
    #[error("timeline render failed: {reason}")]
    TimelineRenderFailed {
        /// Human-readable description of the failure.
        reason: String,
    },

    /// A clip's source file could not be found on disk.
    ///
    /// Returned by [`TimelineBuilder::build`](crate::TimelineBuilder::build) and
    /// [`Timeline::render`](crate::Timeline::render) when `Clip.source` does not
    /// exist.
    #[error("clip source not found: path={path}")]
    ClipNotFound {
        /// Absolute or relative path that could not be found.
        path: String,
    },
}

#[cfg(test)]
mod tests {
    use super::TimelineError;

    #[test]
    fn timeline_error_render_failed_should_display_correctly() {
        let err = TimelineError::TimelineRenderFailed {
            reason: "not implemented".to_string(),
        };
        assert_eq!(err.to_string(), "timeline render failed: not implemented");
    }

    #[test]
    fn timeline_error_clip_not_found_should_include_path_in_message() {
        let err = TimelineError::ClipNotFound {
            path: "/tmp/missing.mp4".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "clip source not found: path=/tmp/missing.mp4"
        );
    }
}
