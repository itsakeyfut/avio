//! Error types for pipeline operations.
//!
//! This module provides [`PipelineError`], which covers all failure modes that
//! can arise when building or running a [`Pipeline`](crate::Pipeline).

/// Errors that can occur while building or running a pipeline.
///
/// # Error Categories
///
/// - **Downstream errors**: [`Decode`](Self::Decode), [`Filter`](Self::Filter),
///   [`Encode`](Self::Encode) — propagated from the underlying crates via `#[from]`
/// - **Configuration errors**: [`NoInput`](Self::NoInput), [`NoOutput`](Self::NoOutput),
///   [`SecondaryInputWithoutFilter`](Self::SecondaryInputWithoutFilter)
///   — returned by [`PipelineBuilder::build`](crate::PipelineBuilder::build)
/// - **Runtime control**: [`Cancelled`](Self::Cancelled) — returned by
///   [`Pipeline::run`](crate::Pipeline::run) when the progress callback returns `false`
/// - **Availability**: [`FrameNotAvailable`](Self::FrameNotAvailable) — no frame at position
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// A decoding step failed.
    ///
    /// Wraps [`ff_decode::DecodeError`] and is produced automatically via `#[from]`
    /// when a decode operation inside the pipeline returns an error.
    #[error("decode failed: {0}")]
    Decode(#[from] ff_decode::DecodeError),

    /// A filter graph step failed.
    ///
    /// Wraps [`ff_filter::FilterError`] and is produced automatically via `#[from]`
    /// when the filter graph inside the pipeline returns an error.
    #[error("filter failed: {0}")]
    Filter(#[from] ff_filter::FilterError),

    /// An encoding step failed.
    ///
    /// Wraps [`ff_encode::EncodeError`] and is produced automatically via `#[from]`
    /// when an encode operation inside the pipeline returns an error.
    #[error("encode failed: {0}")]
    Encode(#[from] ff_encode::EncodeError),

    /// No input path was provided to the builder.
    ///
    /// At least one call to [`PipelineBuilder::input`](crate::PipelineBuilder::input)
    /// is required before [`PipelineBuilder::build`](crate::PipelineBuilder::build).
    #[error("no input specified")]
    NoInput,

    /// No output path and config were provided to the builder.
    ///
    /// A call to [`PipelineBuilder::output`](crate::PipelineBuilder::output) is
    /// required before [`PipelineBuilder::build`](crate::PipelineBuilder::build).
    #[error("no output specified")]
    NoOutput,

    /// `secondary_input()` was called but no filter graph was provided.
    ///
    /// A secondary input only makes sense when a multi-slot filter is set via
    /// [`PipelineBuilder::filter`](crate::PipelineBuilder::filter).
    #[error("secondary input provided without a filter graph")]
    SecondaryInputWithoutFilter,

    /// The pipeline was cancelled by the progress callback.
    ///
    /// Returned by [`Pipeline::run`](crate::Pipeline::run) when the
    /// [`ProgressCallback`](crate::ProgressCallback) returns `false`.
    #[error("pipeline cancelled by caller")]
    Cancelled,

    /// An I/O error (e.g. creating the output directory for thumbnails).
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// No frame was available at the requested position.
    ///
    /// Returned by thumbnail and seek-and-decode operations when the decoder
    /// reports `Ok(None)` — the position is past the end of the stream or no
    /// decodable frame exists at that point.
    #[error("no frame available at the requested position")]
    FrameNotAvailable,
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::PipelineError;

    // --- Display messages: unit variants ---

    #[test]
    fn no_input_should_display_correct_message() {
        let err = PipelineError::NoInput;
        assert_eq!(err.to_string(), "no input specified");
    }

    #[test]
    fn no_output_should_display_correct_message() {
        let err = PipelineError::NoOutput;
        assert_eq!(err.to_string(), "no output specified");
    }

    #[test]
    fn cancelled_should_display_correct_message() {
        let err = PipelineError::Cancelled;
        assert_eq!(err.to_string(), "pipeline cancelled by caller");
    }

    // --- Display messages: wrapping variants ---

    #[test]
    fn decode_should_prefix_inner_message() {
        let err = PipelineError::Decode(ff_decode::DecodeError::decoding_failed("test error"));
        assert!(err.to_string().starts_with("decode failed:"));
    }

    #[test]
    fn filter_should_prefix_inner_message() {
        let err = PipelineError::Filter(ff_filter::FilterError::BuildFailed);
        assert_eq!(
            err.to_string(),
            "filter failed: failed to build filter graph"
        );
    }

    #[test]
    fn encode_should_prefix_inner_message() {
        let err = PipelineError::Encode(ff_encode::EncodeError::Cancelled);
        assert_eq!(err.to_string(), "encode failed: Encoding cancelled by user");
    }

    // --- From conversions ---

    #[test]
    fn decode_error_should_convert_into_pipeline_error() {
        let inner = ff_decode::DecodeError::decoding_failed("test error");
        let err: PipelineError = inner.into();
        assert!(matches!(err, PipelineError::Decode(_)));
    }

    #[test]
    fn filter_error_should_convert_into_pipeline_error() {
        let inner = ff_filter::FilterError::BuildFailed;
        let err: PipelineError = inner.into();
        assert!(matches!(err, PipelineError::Filter(_)));
    }

    #[test]
    fn encode_error_should_convert_into_pipeline_error() {
        let inner = ff_encode::EncodeError::Cancelled;
        let err: PipelineError = inner.into();
        assert!(matches!(err, PipelineError::Encode(_)));
    }

    // --- std::error::Error::source() ---

    #[test]
    fn decode_should_expose_source() {
        let err = PipelineError::Decode(ff_decode::DecodeError::decoding_failed("test error"));
        assert!(err.source().is_some());
    }

    #[test]
    fn filter_should_expose_source() {
        let err = PipelineError::Filter(ff_filter::FilterError::BuildFailed);
        assert!(err.source().is_some());
    }

    #[test]
    fn encode_should_expose_source() {
        let err = PipelineError::Encode(ff_encode::EncodeError::Cancelled);
        assert!(err.source().is_some());
    }

    #[test]
    fn unit_variants_should_have_no_source() {
        assert!(PipelineError::NoInput.source().is_none());
        assert!(PipelineError::NoOutput.source().is_none());
        assert!(PipelineError::Cancelled.source().is_none());
    }

    // --- Io variant ---

    #[test]
    fn io_error_should_convert_into_pipeline_error() {
        let inner = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: PipelineError = inner.into();
        assert!(matches!(err, PipelineError::Io(_)));
    }

    #[test]
    fn io_error_should_display_correct_message() {
        let inner = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let err: PipelineError = inner.into();
        assert_eq!(err.to_string(), "i/o error: access denied");
    }

    #[test]
    fn io_error_should_expose_source() {
        let inner = std::io::Error::new(std::io::ErrorKind::Other, "some error");
        let err: PipelineError = inner.into();
        assert!(err.source().is_some());
    }
}
