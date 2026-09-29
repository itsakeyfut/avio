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

    /// A source file exists but the configured render path cannot use it.
    ///
    /// Either libavformat could not open it, or it carries no stream of the kind the
    /// track needs that this build has a decoder for. Raised by
    /// [`TimelineBuilder::build`](crate::TimelineBuilder::build) so the failure lands
    /// when the clip is added rather than at export (ADR-0023, #1850).
    ///
    /// A source that does **not** exist is not reported here: a project reopened with
    /// a moved file is a relink case, and refusing to build the document would be the
    /// wrong answer to it.
    #[error("source {path} cannot be used: {reason}")]
    SourceUnusable {
        /// The source file.
        path: String,
        /// Why the render path cannot use it.
        reason: String,
    },

    /// A clip was placed on a track of a kind its source cannot serve.
    ///
    /// A generated (`Text`/`Solid`) source synthesizes video and carries no audio, so
    /// it cannot serve an audio track. The clip is named by its position within the
    /// track, because the builder has not stamped ids yet when this is found. The
    /// same rule on the edit path is
    /// [`EditError::ClipCannotServeTrack`](crate::EditError::ClipCannotServeTrack),
    /// and [`TimelineIssue::ClipCannotServeTrack`](crate::TimelineIssue::ClipCannotServeTrack)
    /// reports it for a timeline that reached neither (ADR-0023).
    #[error("clip {clip_index} on {kind:?} track \"{track}\" has a source that cannot serve it")]
    ClipCannotServeTrack {
        /// Name of the track the clip was placed on.
        track: String,
        /// Kind of that track.
        kind: crate::ids::TrackKind,
        /// Position of the offending clip within the track's clip list.
        clip_index: usize,
    },

    /// A generated (`Text`/`Solid`) clip was placed on an active track without an
    /// out-point.
    ///
    /// A generated source synthesizes frames indefinitely, so it has no intrinsic
    /// end; the clip must set an [`out_point`](crate::Clip::out_point) (e.g. via
    /// [`Clip::trim`](crate::Clip::trim)) to bound its duration before rendering.
    #[error("generated source clip needs an out_point to bound its duration")]
    GeneratedSourceNeedsDuration,

    /// A text clip was rendered on an `FFmpeg` build that carries no `drawtext`
    /// filter.
    ///
    /// `drawtext` needs freetype, which several common packages are built without.
    /// Ask [`text_rendering_available`](ff_filter::text_rendering_available) before
    /// offering a text tool, or
    /// [`Timeline::validate`](crate::Timeline::validate) before rendering.
    #[error(
        "text clips need FFmpeg's `drawtext` filter, which this build does not have; \
         install an FFmpeg built with freetype (Windows: \
         `vcpkg install ffmpeg[core,drawtext]:x64-windows`)"
    )]
    TextRendererUnavailable,
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
