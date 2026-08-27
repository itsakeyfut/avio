//! A safe, high-level Rust API over `FFmpeg` for building media applications: decode, encode, filter, compose, and stream.
//!
//! `avio` is the facade crate for the `ff-*` crate family, a backend-agnostic
//! multimedia toolkit. It re-exports the public APIs of all member crates behind
//! feature flags, so users can depend on a single crate and opt into only the
//! functionality they need.
//!
//! # Feature Flags
//!
//! | Feature          | Crate              | Default | Implies             |
//! |------------------|--------------------|---------|---------------------|
//! | `probe`          | `ff-probe`         | yes     |                     |
//! | `decode`         | `ff-decode`        | yes     |                     |
//! | `analysis`       | `ff-analysis`      | yes     | `decode`            |
//! | `encode`         | `ff-encode`        | yes     |                     |
//! | `hwaccel`        | `ff-encode`        | yes     |                     |
//! | `filter`         | `ff-filter`        | no      |                     |
//! | `pipeline`       | `ff-pipeline`      | no      | `decode`+`encode`+`filter` |
//! | `stream`         | `ff-stream`        | no      | `pipeline`          |
//! | `preview`        | `ff-preview`       | no      |                     |
//! | `preview-proxy`  | `ff-preview`       | no      | `preview`           |
//! | `render`         | `ff-render`        | no      | `preview`           |
//! | `render-gpu`     | `ff-render`        | no      | `render`            |
//! | `tokio`          | ff-decode/encode   | no      | `decode` + `encode` |
//! | `gpl`            | `ff-encode`        | no      |                     |
//! | `srt`            | ff-decode/stream   | no      |                     |
//! | `serde`          | `ff-filter`        | no      |                     |
//!
//! # Usage
//!
//! ```toml
//! # Default: probe + decode + analysis + encode + hwaccel
//! [dependencies]
//! avio = "0.15"
//!
//! # Add filtering
//! avio = { version = "0.15", features = ["filter"] }
//!
//! # Full stack (implies filter + pipeline)
//! avio = { version = "0.15", features = ["stream"] }
//! ```
//!
//! # Quick Start
//!
//! ## Probe
//!
//! [`open`] is a free function (not a method) that reads metadata without
//! decoding:
//!
//! ```ignore
//! use avio::open;
//!
//! let info = open("video.mp4")?;
//! println!("duration: {:?}", info.duration());
//! ```
//!
//! ## Decode
//!
//! All decoders follow the same builder pattern. Use
//! `.output_format()` / `.output_sample_rate()` to request automatic
//! format conversion inside the decoder:
//!
//! ```ignore
//! use avio::{VideoDecoder, AudioDecoder, PixelFormat, SampleFormat};
//!
//! // Video: request RGB24 output (FFmpeg converts internally)
//! let mut vdec = VideoDecoder::open("video.mp4")
//!     .output_format(PixelFormat::Rgb24)
//!     .build()?;
//! for result in &mut vdec { /* ... */ }
//!
//! // Audio: resample to 16-bit 44.1 kHz
//! let mut adec = AudioDecoder::open("video.mp4")
//!     .output_format(SampleFormat::I16)
//!     .output_sample_rate(44_100)
//!     .build()?;
//! ```
//!
//! ## Encode
//!
//! There are three encode APIs, each suited to a different situation.
//! Choosing the right one prevents unnecessary complexity.
//!
//! ### When to use `Pipeline` (feature: `pipeline`)
//!
//! Use `Pipeline` when your source is an **existing media file** and you want
//! to transcode, filter, or repackage it with minimal boilerplate.
//!
//! - You are transcoding a file to another codec or container.
//! - You want to apply filters (scale, trim, fade, tone-map, …).
//! - You want to concatenate multiple input files.
//! - You need progress reporting without managing the decode loop yourself.
//! - You are generating HLS or DASH output (`stream` feature).
//!
//! ```ignore
//! use avio::{Pipeline, EncoderConfig, VideoCodec, AudioCodec, BitrateMode};
//!
//! Pipeline::builder()
//!     .input("input.mp4")
//!     .output("output.mp4", EncoderConfig::builder()
//!         .video_codec(VideoCodec::H264)
//!         .audio_codec(AudioCodec::Aac)
//!         .bitrate_mode(BitrateMode::Crf(23))
//!         .build())
//!     .build()?
//!     .run()?;
//! ```
//!
//! **Examples:** `transcode`, `trim_and_scale`, `concat_clips`,
//! `extract_thumbnails`, `hls_output`, `abr_ladder`.
//!
//! ### When to use `VideoEncoder` / `AudioEncoder` directly (feature: `encode`)
//!
//! Use the encoder types directly when you need **frame-level control** or
//! your frames come from a source other than a media file.
//!
//! - You are generating frames programmatically (e.g., a game renderer,
//!   a signal generator, test patterns).
//! - You need to inspect or modify individual frames between decode and encode.
//! - You want per-frame metadata, custom PTS/DTS, or non-standard GOP structure.
//! - You need to react to `EncodeError::Cancelled` mid-stream.
//! - You want cancellable progress via `EncodeProgressCallback::should_cancel()`.
//!
//! ```ignore
//! use avio::{VideoDecoder, VideoEncoder, VideoCodec};
//!
//! let mut decoder = VideoDecoder::open("input.mp4").build()?;
//! let mut encoder = VideoEncoder::create("output.mp4")
//!     .video(decoder.width(), decoder.height(), decoder.frame_rate())
//!     .video_codec(VideoCodec::H264)
//!     .build()?;
//!
//! while let Ok(Some(frame)) = decoder.decode_one() {
//!     // Inspect or modify `frame` here before encoding.
//!     encoder.push_video(&frame)?;
//! }
//! encoder.finish()?;
//! ```
//!
//! **Examples:** `encode_video_direct`, `encode_audio_direct`,
//! `encode_with_progress`, `two_pass_encode`, `filter_direct`.
//!
//! ### When to use `AsyncVideoEncoder` / `AsyncAudioEncoder` (feature: `tokio`)
//!
//! Use the async encoders when your application runs on a **Tokio runtime**
//! and you need back-pressure or concurrent decode/encode.
//!
//! - You are writing an async application and cannot block the executor.
//! - Frames arrive from an async source (network, channel, microphone).
//! - You want the decoder and encoder to run concurrently on separate tasks.
//! - You rely on the bounded internal channel (capacity 8) to prevent
//!   unbounded memory growth when the encoder is slower than the producer.
//!
//! ```ignore
//! use avio::{AsyncVideoDecoder, AsyncVideoEncoder, VideoEncoder, VideoCodec};
//! use futures::StreamExt;
//!
//! let mut encoder = AsyncVideoEncoder::from_builder(
//!     VideoEncoder::create("output.mp4")
//!         .video(1920, 1080, 30.0)
//!         .video_codec(VideoCodec::H264),
//! )?;
//!
//! let stream = AsyncVideoDecoder::open("input.mp4").await?.into_stream();
//! tokio::pin!(stream);
//! while let Some(Ok(frame)) = stream.next().await {
//!     encoder.push(frame).await?;
//! }
//! encoder.finish().await?;
//! ```
//!
//! **Examples:** `async_encode_video`, `async_encode_audio`, `async_transcode`.
//!
//! # Projects using avio
//!
//! ## ascii-term: Terminal ASCII Art Video Player
//!
//! [`ascii-term`](https://github.com/itsakeyfut/ascii-term) is a terminal media player
//! that renders video as colored ASCII art with synchronized audio. It was migrated from
//! `ffmpeg-next` / `ffmpeg-sys-next` to `avio`, with no direct `unsafe` `FFmpeg` code in
//! the application. It uses:
//!
//! - [`VideoDecoder`] with `.output_format(PixelFormat::Rgb24)` for per-pixel luminance
//! - [`AudioDecoder`] with [`SampleFormat::F32`] output, converted to interleaved PCM for
//!   [`rodio`](https://crates.io/crates/rodio) playback
//! - Synchronized audio and video across two threads via `crossbeam-channel`
//!
//! ## avio-editor-demo: Non-Linear Video Editor
//!
//! [`avio-editor-demo`](https://github.com/itsakeyfut/avio-editor-demo) is a non-linear
//! video editor and the main driver of the library's API. It exercises the full decode,
//! timeline compose, preview, and export path, and is where most bugs and API changes
//! originate. It uses:
//!
//! - `Timeline` / `Clip` multi-track composition with per-clip colour correction and transitions
//! - A real-time preview that matches the exported result
//! - The `ff-preview` proxy workflow, plus scene/silence detection, waveform, and EBU R128
//!   loudness analysis
//!
//! # Extension traits
//!
//! `VideoCodecEncodeExt` adds encode-specific helpers (`.default_extension()`,
//! `.is_lgpl_compatible()`) to `VideoCodec`. Import the trait to call them:
//!
//! ```ignore
//! use avio::{VideoCodec, VideoCodecEncodeExt};
//!
//! let ext = VideoCodec::H264.default_extension(); // "mp4"
//! ```

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

// ── probe feature ─────────────────────────────────────────────────────────────
#[cfg(feature = "probe")]
pub use ff_probe::{ProbeError, open};

// ── decode feature ────────────────────────────────────────────────────────────
//
// Frame/codec types are already re-exported from ff-format above, so we omit
// them here to keep a single canonical source.
// Memory pooling: VecPool is the concrete pool implementation; FramePool is the
// trait for accepting custom pool implementations. Use VecPool directly, or
// Arc<dyn FramePool> when you need to pass a pool through an abstraction boundary.
#[cfg(feature = "decode")]
pub use ff_common::{PooledBuffer, VecPool};
// Engine surface: `TimelineError::Decode(#[from] DecodeError)` names it — unconditional.
pub use ff_decode::DecodeError;
#[cfg(feature = "decode")]
pub use ff_decode::{
    AudioDecoder, AudioDecoderBuilder, FrameExtractor, FramePool, HardwareAccel, ImageDecoder,
    ImageDecoderBuilder, SeekMode, ThumbnailSelector, VideoDecoder, VideoDecoderBuilder,
};

// ── analysis feature ──────────────────────────────────────────────────────────
// Media-analysis primitives (scene / silence / BPM / histogram / keyframe /
// black-frame detection and video scopes), extracted into `ff-analysis`.
#[cfg(feature = "analysis")]
pub use ff_analysis::{
    AnalysisError, BlackFrameDetector, BpmResult, FrameHistogram, Histogram, HistogramExtractor,
    KeyframeEnumerator, RgbParade, SceneDetector, ScopeAnalyzer, SilenceDetector, SilenceRange,
    WaveformAnalyzer, WaveformSample,
};

// ── encode feature ────────────────────────────────────────────────────────────
//
// EncodeProgress / EncodeProgressCallback carry encode-specific metrics and are
// distinct from ff-pipeline's Progress / ProgressCallback, so both sets can be
// re-exported from avio without ambiguity.
// VideoCodecEncodeExt provides encode-specific helpers (is_lgpl_compatible,
// default_extension) on the shared VideoCodec type; import it to call them.
#[cfg(feature = "encode")]
pub use ff_encode::{
    AacOptions, AacProfile, AudioCodecOptions, AudioEncoder, AudioEncoderBuilder,
    AudioEncoderConfig, Av1Options, Av1Usage, CRF_MAX, DnxhdOptions, DnxhdVariant, EncodeProgress,
    EncodeProgressCallback, ExportPreset, FlacOptions, GifPreview, H264Options, H264Preset,
    H264Profile, H264Tune, H265Options, H265Profile, H265Tier, HardwareEncoder, ImageEncoder,
    ImageEncoderBuilder, Mp3Options, Mp3Quality, OpusApplication, OpusOptions, OutputContainer,
    Preset, PreviewImageError, ProResOptions, ProResProfile, SpriteSheet, SvtAv1Options,
    VideoCodecEncodeExt, VideoCodecOptions, VideoEncoder, VideoEncoderBuilder, VideoEncoderConfig,
    Vp9Options,
};
// Engine surface: `BitrateMode` is an `EncoderConfig::builder()` setter dep and
// `EncodeError` is named by `TimelineError::Encode(#[from] EncodeError)` — unconditional.
pub use ff_encode::{BitrateMode, EncodeError};

// media-ops + trim moved to the `ff-remux` crate; re-exported here so `avio`'s
// surface is unchanged (enabled by the `encode` feature, which pulls `ff-remux`).
#[cfg(feature = "encode")]
pub use ff_remux::{
    AudioAdder, AudioExtractor, AudioReplacement, RemuxError, StreamCopyTrim, StreamCopyTrimmer,
};

// ── tokio feature ─────────────────────────────────────────────────────────────
//
// Enabling `tokio` also enables `decode` and `encode` (see Cargo.toml), so the
// underlying crate dependencies are guaranteed to be present. Each async wrapper
// is a thin Send + async shell around its synchronous counterpart, backed by
// spawn_blocking and a bounded tokio::sync::mpsc channel (encoders, cap=8).
#[cfg(feature = "tokio")]
pub use ff_decode::{
    AsyncAudioDecoder, AsyncAudioDecoderBuilder, AsyncImageDecoder, AsyncVideoDecoder,
    AsyncVideoDecoderBuilder,
};
#[cfg(feature = "tokio")]
pub use ff_encode::{AsyncAudioEncoder, AsyncVideoEncoder};

// ── filter feature ────────────────────────────────────────────────────────────
#[cfg(feature = "filter")]
pub use ff_filter::{
    AnalyzeOptions, AnimationEntry, AudioConcatenator, AudioTrack, CrossfadeJoiner, FilterGraph,
    FilterGraphBuilder, Interpolation, LavfiSource, LayerSource, LensProfile, Lerp, LoudnessMeter,
    LoudnessResult, MultiTrackAudioMixer, MultiTrackComposer, NoiseType, ProxySource,
    QualityMetrics, RealtimeComposer, SolidSource, StabilizeOptions, Stabilizer, TextSource,
    VideoConcatenator, VideoLayer,
};
// Engine surface: the ff-filter authoring set the model names via Clip / FilterStep /
// animation (Clip fields, FilterStep variant payloads, animation authoring,
// Clip::realtime_layer[_descriptor] returns, FilterError, and the EncoderConfig HwAccel
// setter dep) — unconditional.
pub use ff_filter::{
    AnimatedValue, AnimationTrack, BlendMode, CompositeOp, DrawTextOptions, Easing, EqBand,
    FilterError, FilterStep, HwAccel, Keyframe, RealtimeLayer, RealtimeLayerDescriptor, Rgb,
    ScaleAlgorithm, ToneMap, XfadeTransition, YadifMode,
};

// ── editing model (unconditional) ─────────────────────────────────────────────
//
// The editing model (`Timeline` / `Clip` / `Editor` / `render` / `TimelineError`) is
// defined in `avio` itself — the engine owns the model, and it is always compiled
// (`ff-decode` / `ff-encode` / `ff-filter` / `ff-pipeline` are non-optional). The
// standalone execution pipelines stay in `ff-pipeline`, re-exported below and still
// gated behind the `pipeline` feature (removed in #1484).
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

#[cfg(feature = "pipeline")]
pub use ff_pipeline::{
    AudioPipeline, Pipeline, PipelineBuilder, PipelineError, ProgressCallback, ThumbnailPipeline,
    VideoPipeline,
};
// Engine surface: `Timeline::render(config: EncoderConfig)` and
// `render_with_progress(_, _, impl Fn(&Progress) -> bool)` name these; the builder is
// `EncoderConfig::builder()`'s constructor — unconditional.
pub use ff_pipeline::{EncoderConfig, EncoderConfigBuilder, Progress};

// ── stream feature ────────────────────────────────────────────────────────────
//
// Enabling `stream` also enables `pipeline` (and transitively `filter`).
#[cfg(feature = "stream")]
pub use ff_stream::{
    AbrLadder, AbrRendition, DashOutput, FanoutOutput, HlsOutput, HlsSegmentFormat, LiveAbrFormat,
    LiveAbrLadder, LiveDashOutput, LiveHlsOutput, Rendition, RtmpOutput, StreamError, StreamOutput,
};

#[cfg(all(feature = "stream", feature = "srt"))]
pub use ff_stream::SrtOutput;

// ── preview feature ───────────────────────────────────────────────────────────
//
// Single-file real-time playback with frame-accurate seek and A/V sync.
// Enable the `preview` feature to access `PreviewPlayer`, `PlaybackClock`,
// and the `RgbaSink` / `RgbaFrame` helpers.
// Enable `preview-proxy` to additionally access `ProxyGenerator`.
// Enable both `preview` and `pipeline` to access `TimelinePlayer` /
// `SceneRunner` for multi-clip real-time preview.
#[cfg(feature = "preview")]
pub use ff_preview::{
    AudioMixer, AudioTrackHandle, DecodeBuffer, DecodeBufferBuilder, FrameResult, FrameSink,
    PlaybackClock, PlayerCommand, PlayerEvent, PlayerHandle, PlayerRunner, PreviewError,
    PreviewPlayer, RgbaFrame, RgbaSink, SeekEvent,
};

// `HardwareAccel` is re-exported under the `decode` feature (from `ff_decode`).
// When `preview` is enabled without `decode`, expose it here so that callers can
// still call `PlayerRunner::set_hardware_accel()` without a direct `ff-preview`
// dependency.  The `not(feature = "decode")` guard prevents a duplicate-name
// error when both features are active.
#[cfg(all(feature = "preview", not(feature = "decode")))]
pub use ff_preview::HardwareAccel;

// The editing-model preview entry `TimelinePlayer` is defined in `avio` (the
// `player` module) — it derives a `Scene` from a `Timeline` and hands it to
// `ff-preview`'s `ScenePlayer`. `ScenePlayer` / `SceneRunner` / the `Scene` types
// stay in `ff-preview` (model-agnostic). The model is unconditional now, so these
// require only `preview` (which supplies `ff-preview/timeline`).
#[cfg(feature = "preview")]
mod player;
#[cfg(feature = "preview")]
pub use ff_preview::{
    Scene, SceneAudioPlacement, SceneAudioTrack, ScenePlacement, ScenePlayer, SceneRunner,
    SceneVideoTrack,
};
#[cfg(feature = "preview")]
pub use player::TimelinePlayer;

#[cfg(all(feature = "preview", feature = "tokio"))]
pub use ff_preview::AsyncPreviewPlayer;

#[cfg(feature = "preview-proxy")]
pub use ff_preview::{ProxyGenerator, ProxyJob, ProxyResolution};

// ── render feature ────────────────────────────────────────────────────────────
//
// GPU compositing (`ff-render`). Namespaced under `avio::render` rather than
// re-exported flat because several node types share names with the `filter`
// feature (`BlendMode`, `ScaleAlgorithm`); a flat re-export would collide.
// Enabling `render` also enables `preview` (see Cargo.toml).
#[cfg(feature = "render")]
pub mod render {
    //! GPU compositing pipeline (`ff-render`).
    //!
    //! These types are namespaced (`avio::render::*`) because `BlendMode` and
    //! `ScaleAlgorithm` would otherwise collide with the `filter` feature's
    //! re-exports of the same names.
    pub use ff_render::{
        AlphaMatteNode, BlendMode, BlendModeNode, ChromaKeyNode, ColorGradeNode, CrossfadeNode,
        GpuFrameSink, LumaMaskNode, OverlayNode, RenderError, RenderGraph, RenderNodeCpu,
        ScaleAlgorithm, ScaleNode, ShapeMaskNode, TransformNode, YuvFormat, YuvUploadNode,
    };

    // wgpu-gated in ff-render → reachable only with avio's `render-gpu` feature.
    #[cfg(feature = "render-gpu")]
    pub use ff_render::{
        Compositor, FrameLayer, LayerTransform, RenderContext, RenderNode, TextureHandle,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ff-format (always-on) ─────────────────────────────────────────────────

    #[test]
    fn format_reexports_should_be_accessible() {
        let _: VideoCodec = VideoCodec::default();
        let _: AudioCodec = AudioCodec::default();
        let _: PixelFormat = PixelFormat::default();
        let _: SampleFormat = SampleFormat::default();
        let _: ChannelLayout = ChannelLayout::default();
        let _: ColorSpace = ColorSpace::default();
        let _: ColorRange = ColorRange::default();
        let _: ColorPrimaries = ColorPrimaries::default();
        let _: Rational = Rational::default();
        let _: Timestamp = Timestamp::default();
        let _: MediaInfo = MediaInfo::default();
        let _: NetworkOptions = NetworkOptions::default();
    }

    // ── probe feature ─────────────────────────────────────────────────────────

    #[cfg(feature = "probe")]
    #[test]
    fn probe_open_should_be_accessible() {
        // open is a function; a non-existent path yields ProbeError
        let result = open("/no/such/file.mp4");
        assert!(matches!(result, Err(ProbeError::FileNotFound { .. })));
    }

    #[cfg(feature = "probe")]
    #[test]
    fn probe_error_should_be_accessible() {
        let err = ProbeError::FileNotFound {
            path: std::path::PathBuf::from("missing.mp4"),
        };
        assert!(err.to_string().contains("missing.mp4"));
    }

    // ── decode feature ────────────────────────────────────────────────────────

    #[cfg(feature = "decode")]
    #[test]
    fn decode_builder_types_should_be_accessible() {
        // Builder entry points are static methods on the decoder types.
        // Calling them with a dummy path exercises name resolution without
        // touching FFmpeg.
        let _ = VideoDecoder::open("/no/such/file.mp4");
        let _ = AudioDecoder::open("/no/such/file.mp4");
        let _ = ImageDecoder::open("/no/such/file.mp4");
    }

    #[cfg(feature = "decode")]
    #[test]
    fn decode_error_should_be_accessible() {
        let _: DecodeError = DecodeError::decoding_failed("test");
    }

    #[cfg(feature = "decode")]
    #[test]
    fn decode_seek_mode_should_be_accessible() {
        let _: SeekMode = SeekMode::Keyframe;
        let _: SeekMode = SeekMode::Exact;
        let _: SeekMode = SeekMode::Backward;
    }

    #[cfg(feature = "decode")]
    #[test]
    fn decode_hardware_accel_should_be_accessible() {
        let _: HardwareAccel = HardwareAccel::Auto;
        let _: HardwareAccel = HardwareAccel::None;
    }

    #[cfg(feature = "decode")]
    #[test]
    fn decode_vec_pool_should_be_accessible() {
        let pool: std::sync::Arc<VecPool> = VecPool::new(8);
        assert_eq!(pool.capacity(), 8);
        assert_eq!(pool.available(), 0);
    }

    // ── encode feature ────────────────────────────────────────────────────────

    #[cfg(feature = "encode")]
    #[test]
    fn encode_builder_types_should_be_accessible() {
        // VideoEncoder::create / AudioEncoder::create are the public entry
        // points that return their respective builder types.
        let _ = VideoEncoder::create("/tmp/out.mp4");
        let _ = AudioEncoder::create("/tmp/out.mp3");
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encode_bitrate_mode_should_be_accessible() {
        let _: BitrateMode = BitrateMode::Cbr(1_000_000);
        let _: BitrateMode = BitrateMode::Crf(23);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encode_error_should_be_accessible() {
        let _: EncodeError = EncodeError::Cancelled;
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encode_progress_should_be_accessible() {
        assert!(std::mem::size_of::<EncodeProgress>() > 0);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encode_progress_callback_should_be_accessible() {
        // EncodeProgressCallback is a trait; verify it is in scope by creating
        // a minimal no-op implementation.
        struct NoOp;
        impl EncodeProgressCallback for NoOp {
            fn on_progress(&mut self, _: &EncodeProgress) {}
        }
        let _ = NoOp;
    }

    // ── tokio feature ─────────────────────────────────────────────────────────

    #[cfg(feature = "tokio")]
    #[test]
    fn tokio_async_decoders_should_be_accessible() {
        // Verify name resolution: constructing the builder/future without
        // opening a file is enough to confirm the types are in scope.
        let _ = AsyncVideoDecoder::open("/no/such/file.mp4");
        let _ = AsyncAudioDecoder::open("/no/such/file.mp4");
        let _ = AsyncImageDecoder::open("/no/such/file.mp4");
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn tokio_async_encoders_should_be_accessible() {
        // from_builder consumes a builder; constructing the builder (which is
        // a sync operation) confirms the types are in scope without touching FFmpeg.
        use ff_encode::{AudioEncoderBuilder, VideoEncoderBuilder};
        fn _accepts_video_builder(_: VideoEncoderBuilder) {}
        fn _accepts_audio_builder(_: AudioEncoderBuilder) {}
        // The types compile; that is the assertion.
        let _ = std::mem::size_of::<AsyncVideoEncoder>();
        let _ = std::mem::size_of::<AsyncAudioEncoder>();
    }

    // ── filter feature ────────────────────────────────────────────────────────

    #[cfg(feature = "filter")]
    #[test]
    fn filter_graph_builder_should_be_accessible() {
        // FilterGraphBuilder::new() is the public entry point.
        let _builder: FilterGraphBuilder = FilterGraphBuilder::new();
    }

    #[cfg(feature = "filter")]
    #[test]
    fn filter_tone_map_should_be_accessible() {
        let _: ToneMap = ToneMap::Hable;
        let _: ToneMap = ToneMap::Reinhard;
        let _: ToneMap = ToneMap::Mobius;
    }

    #[cfg(feature = "filter")]
    #[test]
    fn filter_hw_accel_should_be_accessible() {
        let _: HwAccel = HwAccel::Cuda;
        let _: HwAccel = HwAccel::VideoToolbox;
    }

    #[cfg(feature = "filter")]
    #[test]
    fn filter_error_should_be_accessible() {
        let _: FilterError = FilterError::BuildFailed;
        let _: FilterError = FilterError::ProcessFailed;
    }

    // ── pipeline feature ──────────────────────────────────────────────────────

    #[cfg(feature = "pipeline")]
    #[test]
    fn pipeline_builder_should_be_accessible() {
        // Pipeline::builder() is the public entry point; verify name resolution.
        let _builder: PipelineBuilder = Pipeline::builder();
    }

    #[cfg(feature = "pipeline")]
    #[test]
    fn pipeline_error_should_be_accessible() {
        let _: PipelineError = PipelineError::NoInput;
        let _: PipelineError = PipelineError::NoOutput;
        let _: PipelineError = PipelineError::Cancelled;
    }

    #[cfg(feature = "pipeline")]
    #[test]
    fn pipeline_progress_should_be_accessible() {
        let p = Progress {
            frames_processed: 10,
            total_frames: Some(100),
            elapsed: std::time::Duration::from_secs(1),
        };
        assert_eq!(p.percent(), Some(10.0));
    }

    #[cfg(feature = "pipeline")]
    #[test]
    fn pipeline_progress_callback_should_be_accessible() {
        // ProgressCallback is Box<dyn Fn(&Progress) -> bool + Send>.
        let _cb: ProgressCallback = Box::new(|_: &Progress| true);
    }

    #[cfg(feature = "pipeline")]
    #[test]
    fn pipeline_thumbnail_pipeline_should_be_accessible() {
        // ThumbnailPipeline::new constructs without opening a file.
        let _t: ThumbnailPipeline = ThumbnailPipeline::new("/no/such/file.mp4");
    }

    #[cfg(feature = "pipeline")]
    #[test]
    fn pipeline_audio_pipeline_should_be_accessible() {
        let _: AudioPipeline = AudioPipeline::new();
    }

    #[cfg(all(feature = "pipeline", feature = "encode"))]
    #[test]
    fn pipeline_encoder_config_should_be_accessible() {
        let _config = EncoderConfig::builder()
            .video_codec(VideoCodec::H264)
            .audio_codec(AudioCodec::Aac)
            .bitrate_mode(BitrateMode::Cbr(4_000_000))
            .build();
    }

    // ── stream feature ────────────────────────────────────────────────────────

    #[cfg(feature = "stream")]
    #[test]
    fn stream_hls_output_should_be_accessible() {
        // HlsOutput::new() is the public entry point; verify name resolution.
        let _hls: HlsOutput = HlsOutput::new("/tmp/hls");
    }

    #[cfg(feature = "stream")]
    #[test]
    fn stream_dash_output_should_be_accessible() {
        // DashOutput::new() is the public entry point; verify name resolution.
        let _dash: DashOutput = DashOutput::new("/tmp/dash");
    }

    #[cfg(feature = "stream")]
    #[test]
    fn stream_abr_ladder_should_be_accessible() {
        // AbrLadder::new() is the public entry point; verify name resolution.
        let _ladder: AbrLadder = AbrLadder::new("/no/such/file.mp4");
    }

    #[cfg(feature = "stream")]
    #[test]
    fn stream_rendition_should_be_accessible() {
        let _r: Rendition = Rendition {
            width: 1280,
            height: 720,
            bitrate: 3_000_000,
        };
    }

    #[cfg(feature = "stream")]
    #[test]
    fn stream_error_should_be_accessible() {
        let _err: StreamError = StreamError::InvalidConfig {
            reason: "test".into(),
        };
    }

    // ── preview feature ───────────────────────────────────────────────────────

    #[cfg(feature = "preview")]
    #[test]
    fn preview_rgba_types_should_be_accessible() {
        // RgbaSink and RgbaFrame are the concrete sink/frame types added in v0.14.0.
        let _: Option<RgbaSink> = None;
        let _: Option<RgbaFrame> = None;
    }

    #[cfg(feature = "preview")]
    #[test]
    fn preview_player_command_should_be_accessible() {
        // PlayerCommand variants must be reachable via avio for callers that do
        // not take a direct ff-preview dependency.
        let _ = PlayerCommand::Play;
        let _ = PlayerCommand::Pause;
        let _ = PlayerCommand::Stop;
    }

    #[cfg(feature = "preview")]
    #[test]
    fn preview_audio_types_should_be_accessible() {
        // AudioMixer and AudioTrackHandle must be in scope under the preview feature.
        let _ = std::mem::size_of::<AudioMixer>();
        let _ = std::mem::size_of::<AudioTrackHandle>();
    }

    // When `decode` is not enabled, HardwareAccel must still be reachable through
    // the `preview` feature.  This test is compiled only when `decode` is absent
    // so it does not collide with the `decode`-feature re-export.
    #[cfg(all(feature = "preview", not(feature = "decode")))]
    #[test]
    fn preview_hardware_accel_without_decode_should_be_accessible() {
        let _ = HardwareAccel::Auto;
        let _ = HardwareAccel::None;
    }

    #[cfg(all(feature = "preview", feature = "pipeline"))]
    #[test]
    fn preview_timeline_types_should_be_accessible() {
        // TimelinePlayer and SceneRunner are available when both `preview` and
        // `pipeline` features are enabled (avio wires ff-preview/timeline via
        // `ff-preview?/timeline` in the `pipeline` feature definition).
        let _ = std::mem::size_of::<TimelinePlayer>();
        let _ = std::mem::size_of::<SceneRunner>();
    }

    #[cfg(all(feature = "preview", feature = "tokio"))]
    #[test]
    fn preview_async_player_should_be_accessible() {
        // Confirm AsyncPreviewPlayer is in scope under the combined feature gate.
        let _ = std::mem::size_of::<AsyncPreviewPlayer>();
    }

    #[cfg(feature = "preview-proxy")]
    #[test]
    fn preview_proxy_types_should_be_accessible() {
        // ProxyResolution variants cover Quarter / Half / Eighth.
        let _: ProxyResolution = ProxyResolution::Half;
        let _: ProxyResolution = ProxyResolution::Quarter;
        let _: ProxyResolution = ProxyResolution::Eighth;
    }

    // ── render feature ────────────────────────────────────────────────────────

    #[cfg(feature = "render")]
    #[test]
    fn render_core_types_should_be_accessible() {
        // The ungated ff-render surface is namespaced under `avio::render`.
        let _: render::RenderError = render::RenderError::Composite {
            message: "test".into(),
        };
        let _: render::RenderGraph = render::RenderGraph::new_cpu();
        let _: render::BlendMode = render::BlendMode::Normal;
        let _: render::ScaleAlgorithm = render::ScaleAlgorithm::Bilinear;
    }

    // BlendMode / ScaleAlgorithm exist under both `filter` and `render`; with both
    // features on, the namespacing must keep them distinct (no collision).
    #[cfg(all(feature = "render", feature = "filter"))]
    #[test]
    fn render_and_filter_blend_mode_should_not_collide() {
        let _: BlendMode = BlendMode::Normal;
        let _: render::BlendMode = render::BlendMode::Normal;
    }

    #[cfg(feature = "render-gpu")]
    #[test]
    fn render_gpu_types_should_be_accessible() {
        // wgpu-gated types are reachable only under `render-gpu`; verify name
        // resolution without constructing a GPU device.
        let _ = std::mem::size_of::<render::Compositor>();
        let _ = std::mem::size_of::<render::RenderContext>();
    }
}
