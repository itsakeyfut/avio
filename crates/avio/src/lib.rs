//! `avio` is a safe, high-level **video-editing engine** for Rust: assemble a
//! [`Timeline`] of [`Clip`]s, edit it through an [`Editor`], and render it to a file.
//!
//! The engine owns the editing model (timeline / clips / tracks, the model-to-scene
//! derivation, and edit history) and speaks in `ff-format` value types ([`VideoCodec`],
//! [`AudioCodec`], [`PixelFormat`], [`Color`], [`TextSpec`], ...) plus the `ff-filter`
//! authoring types ([`FilterStep`], [`BlendMode`], [`AnimationTrack`], ...). The
//! lower-level `ff-*` primitives (standalone decoders, encoders, pipelines, stream
//! outputs, the GPU compositor) live in their own crates; depend on those directly when
//! you need a primitive. See `docs/adr/0004-avio-engine-not-facade.md`.
//!
//! # Feature flags
//!
//! | Feature   | Default | Effect                                            |
//! |-----------|---------|---------------------------------------------------|
//! | `hwaccel` | yes     | hardware-accelerated export (`ff-encode/hwaccel`) |
//! | `preview` | no      | real-time `TimelinePlayer` + `Scene` types        |
//! | `serde`   | no      | `serde` (de)serialization of the model            |
//! | `gpl`     | no      | GPL-only codecs (x264 / x265)                     |
//!
//! The editing model, `render`, probe ([`open`]), and media analysis are always present.
//!
//! # Quick start
//!
//! Build a [`Timeline`] of [`Clip`]s and [`render`](Timeline::render) it with an
//! [`EncoderConfig`]; the `timeline_render` example is a complete end-to-end flow.
//!
//! ```ignore
//! use avio::{Clip, EncoderConfig, Timeline, VideoCodec, AudioCodec, BitrateMode};
//!
//! let timeline: Timeline = /* build from clips */ Timeline::default();
//! timeline.render("output.mp4", EncoderConfig::builder()
//!     .video_codec(VideoCodec::H264)
//!     .audio_codec(AudioCodec::Aac)
//!     .bitrate_mode(BitrateMode::Crf(23))
//!     .build())?;
//! ```
//!
//! Edit through an [`Editor`] with [`Command`]s (undo/redo), inspect a source with
//! [`open`] before importing it, and - with the `preview` feature - play a timeline in
//! real time via `TimelinePlayer`.
//!
//! # Projects using avio
//!
//! [`avio-editor-demo`](https://github.com/itsakeyfut/avio-editor-demo) is a non-linear
//! video editor and the primary driver of this API: multi-track composition with
//! per-clip colour correction and transitions, a real-time preview that matches the
//! exported result, and scene / silence / loudness analysis.

// ── Always-available types from ff-format ────────────────────────────────────
//
// ff-format is an unconditional dependency, so these types are always present
// regardless of which features are enabled. Re-exporting them here avoids the
// duplicate-symbol problem that would arise from re-exporting VideoCodec /
// AudioCodec separately from ff-probe *and* ff-encode (both of which pull them
// in from ff-format anyway).
pub use ff_format::subtitle::{SubtitleError, SubtitleEvent, SubtitleTrack};
pub use ff_format::{
    AlphaMode, Anchor, AudioCodec, AudioFrame, AudioStreamInfo, AudioStreamInfoBuilder,
    ChannelLayout, ChapterInfo, ChapterInfoBuilder, Color, ColorPrimaries, ColorRange, ColorSpace,
    ColorTransfer, ContainerInfo, ContainerInfoBuilder, ErrorSeverity, FormatError, FrameError,
    Hdr10Metadata, MasteringDisplay, MediaError, MediaInfo, MediaInfoBuilder, NetworkOptions,
    PixelFormat, Rational, SampleFormat, SubtitleCodec, SubtitleStreamInfo,
    SubtitleStreamInfoBuilder, TextSpec, TextStyle, Timestamp, VideoCodec, VideoFrame,
    VideoStreamInfo, VideoStreamInfoBuilder,
};

// ── probe (media metadata) ────────────────────────────────────────────────────
// `open` inspects a file before building a Timeline; kept as an engine convenience.
pub use ff_probe::{ProbeError, open};

// Errors the engine's `TimelineError` wraps by `#[from]`.
pub use ff_decode::DecodeError;

// ── analysis (scene / silence / loudness / scopes) ────────────────────────────
// Media analysis feeds editing decisions; kept as an engine convenience.
pub use ff_analysis::{
    AnalysisError, BlackFrameDetector, BpmResult, FrameHistogram, Histogram, HistogramExtractor,
    KeyframeEnumerator, RgbParade, SceneDetector, ScopeAnalyzer, SilenceDetector, SilenceRange,
    WaveformAnalyzer, WaveformSample,
};

// `BitrateMode` configures `EncoderConfig`; `EncodeError` is wrapped by `TimelineError`.
pub use ff_encode::{BitrateMode, EncodeError};

// ── the ff-filter authoring set the model speaks ──────────────────────────────
// Clip fields (BlendMode / CompositeOp / XfadeTransition), FilterStep variant
// payloads (ScaleAlgorithm / ToneMap / EqBand / DrawTextOptions / YadifMode / Rgb /
// AnimatedValue), animation authoring (AnimationTrack / Keyframe / Easing),
// Clip::realtime_layer[_descriptor] returns, FilterError, and the EncoderConfig HwAccel setter.
pub use ff_filter::{
    AnimatedValue, AnimationTrack, BlendMode, CompositeOp, DrawTextOptions, Easing, EqBand,
    FilterError, FilterStep, HwAccel, Keyframe, RealtimeLayer, RealtimeLayerDescriptor, Rgb,
    ScaleAlgorithm, ToneMap, XfadeTransition, YadifMode,
};

// ── editing model (unconditional) ─────────────────────────────────────────────
//
// The editing model (`Timeline` / `Clip` / `Editor` / `render` / `TimelineError`) is
// defined in `avio` itself — the engine owns the model, always compiled
// (`ff-decode` / `ff-encode` / `ff-filter` / `ff-pipeline` are non-optional).
mod clip;
mod derive;
mod edit;
mod editor;
mod error;
mod ids;
mod marker;
mod timeline;
mod track;
mod validate;

pub use clip::{Clip, ClipSource, FitMode, VideoEffectRenderer};
pub use edit::{ClipProperty, Command, EditError, apply};
pub use editor::Editor;
pub use error::TimelineError;
pub use ids::{ClipId, GroupId, MarkerId, TrackId, TrackKind};
pub use marker::Marker;
pub use timeline::{Timeline, TimelineBuilder};
pub use track::Track;
pub use validate::TimelineIssue;

// `Timeline::render` takes an `EncoderConfig` and reports `Progress`.
pub use ff_pipeline::{EncoderConfig, EncoderConfigBuilder, Progress};

// ── real-time preview (opt-in `preview`) ──────────────────────────────────────
//
// `TimelinePlayer` is avio's engine preview entry: it derives a `Scene` from a
// `Timeline` and hands it to `ff-preview`'s runner. The `Scene` value types and the
// `SceneRunner` / `PlayerHandle` handles + `PreviewError` are named by
// `TimelinePlayer::open`. All are gated on `preview` (`ff-preview` is optional).
#[cfg(feature = "preview")]
mod player;
#[cfg(feature = "preview")]
pub use ff_preview::{
    PlayerHandle, PreviewError, Scene, SceneAudioPlacement, SceneAudioTrack, ScenePlacement,
    SceneRunner, SceneVideoTrack,
};
#[cfg(feature = "preview")]
pub use player::TimelinePlayer;
#[cfg(test)]
mod tests {
    use super::*;

    // The re-export surface after ADR-0004: the editing engine + the value types the
    // model speaks + the convenience keeps (probe / analysis). Standalone primitives
    // (decoders, encoders, pipelines, stream outputs, the render module) are no longer
    // reachable through `avio` (docs/adr/0004-avio-engine-not-facade.md).

    #[test]
    fn format_value_types_should_be_accessible() {
        let _: VideoCodec = VideoCodec::default();
        let _: AudioCodec = AudioCodec::default();
        let _: PixelFormat = PixelFormat::default();
        let _: SampleFormat = SampleFormat::default();
        let _: ChannelLayout = ChannelLayout::default();
        let _: ColorSpace = ColorSpace::default();
        let _: Rational = Rational::default();
        let _: Timestamp = Timestamp::default();
        let _: MediaInfo = MediaInfo::default();
        let _: NetworkOptions = NetworkOptions::default();
    }

    #[test]
    fn editing_model_should_be_accessible() {
        // avio owns the model; its types must resolve by name.
        let _ = std::mem::size_of::<Timeline>();
        let _ = std::mem::size_of::<TimelineBuilder>();
        let _ = std::mem::size_of::<Clip>();
        let _ = std::mem::size_of::<Track>();
        let _ = std::mem::size_of::<Editor>();
        let _ = std::mem::size_of::<Command>();
        let _ = std::mem::size_of::<Marker>();
        let _ = std::mem::size_of::<TimelineError>();
        let _ = std::mem::size_of::<ClipId>();
        let _ = std::mem::size_of::<TrackId>();
    }

    #[test]
    fn filter_authoring_types_should_be_accessible() {
        // The ff-filter value types the model speaks (Clip fields / FilterStep payloads / animation).
        let _ = std::mem::size_of::<FilterStep>();
        let _ = std::mem::size_of::<BlendMode>();
        let _ = std::mem::size_of::<CompositeOp>();
        let _ = std::mem::size_of::<XfadeTransition>();
        let _ = std::mem::size_of::<AnimationTrack<f64>>();
        let _ = std::mem::size_of::<Keyframe<f64>>();
        let _: ToneMap = ToneMap::Hable;
        let _: ScaleAlgorithm = ScaleAlgorithm::Bilinear;
    }

    #[test]
    fn export_config_should_be_accessible() {
        let _: BitrateMode = BitrateMode::Crf(23);
        let _config = EncoderConfig::builder()
            .video_codec(VideoCodec::H264)
            .audio_codec(AudioCodec::Aac)
            .bitrate_mode(BitrateMode::Cbr(4_000_000))
            .build();
        let _ = std::mem::size_of::<Progress>();
    }

    #[test]
    fn error_types_should_be_accessible() {
        // TimelineError wraps these primitive errors by `#[from]`.
        let _ = DecodeError::decoding_failed("test");
        let _ = EncodeError::Cancelled;
        let _ = std::mem::size_of::<FilterError>();
    }

    #[test]
    fn probe_convenience_should_be_accessible() {
        let result = open("/no/such/file.mp4");
        assert!(matches!(result, Err(ProbeError::FileNotFound { .. })));
    }

    #[test]
    fn analysis_convenience_should_be_accessible() {
        let _ = std::mem::size_of::<SceneDetector>();
        let _ = std::mem::size_of::<SilenceDetector>();
        let _ = std::mem::size_of::<WaveformAnalyzer>();
        let _ = std::mem::size_of::<ScopeAnalyzer>();
    }

    #[cfg(feature = "preview")]
    #[test]
    fn preview_engine_surface_should_be_accessible() {
        // TimelinePlayer (avio-defined) plus the Scene types / handles it names.
        let _ = std::mem::size_of::<TimelinePlayer>();
        let _ = std::mem::size_of::<SceneRunner>();
        let _ = std::mem::size_of::<Scene>();
        let _ = std::mem::size_of::<PlayerHandle>();
    }
}
