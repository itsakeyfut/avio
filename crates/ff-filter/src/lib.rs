//! Video and audio filter graph operations — the Rust way.
//!
//! This crate provides a safe, ergonomic API for constructing and running
//! `FFmpeg` libavfilter filter graphs.  All `unsafe` `FFmpeg` internals are
//! encapsulated in the `filter_inner` module; users never need to write `unsafe` code.
//!
//! ## Quick start
//!
//! ```ignore
//! use ff_filter::{FilterGraph, ToneMap};
//! use std::time::Duration;
//!
//! // Build a filter graph: scale to 1280×720, then apply tone mapping.
//! let mut graph = FilterGraph::builder()
//!     .scale(1280, 720)
//!     .tone_map(ToneMap::Hable)
//!     .build()?;
//!
//! // Push decoded VideoFrames in …
//! graph.push_video(0, &decoded_frame)?;
//!
//! // … and pull filtered frames out.
//! while let Some(frame) = graph.pull_video()? {
//!     // encode or display `frame`
//! }
//! ```
//!
//! ## Module structure
//!
//! - [`graph`] — public types: [`FilterGraph`], [`FilterGraphBuilder`], [`ToneMap`], [`HwAccel`]
//! - [`error`] — [`FilterError`]
//! - `filter_inner` — `pub(crate)` unsafe `FFmpeg` calls (not part of the public API)

pub mod analysis;
pub mod animation;
pub mod blend;
pub mod composite;
pub mod effects;
pub mod error;
mod filter_inner;
pub mod graph;

pub use analysis::{LoudnessMeter, LoudnessResult, QualityMetrics};
pub use animation::{AnimatedValue, AnimationEntry, AnimationTrack, Easing, Keyframe, Lerp};
pub use blend::BlendMode;
pub use composite::CompositeOp;
pub use effects::{
    AnalyzeOptions, Interpolation, LensProfile, NoiseType, StabilizeOptions, Stabilizer,
};
pub use error::FilterError;
pub use ff_format::{ErrorSeverity, MediaError};
pub use graph::{
    AudioConcatenator, AudioTrack, CrossfadeJoiner, DrawTextOptions, EqBand, FilterGraph,
    FilterGraphBuilder, FilterStep, HwAccel, LavfiSource, MultiTrackAudioMixer, MultiTrackComposer,
    ProxySource, RealtimeComposer, RealtimeLayer, RealtimeLayerDescriptor, Rgb, ScaleAlgorithm,
    ToneMap, VideoConcatenator, VideoLayer, XfadeTransition, YadifMode,
};
