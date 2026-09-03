//! Filter graph public API: [`FilterGraph`] and [`FilterGraphBuilder`].

pub mod builder;
pub mod composition;
mod ffmpeg_token;
pub(crate) mod filter_step;
#[allow(clippy::module_inception)]
mod graph;
pub mod types;

pub use builder::FilterGraphBuilder;
pub use composition::{
    AudioConcatenator, AudioTrack, CrossfadeJoiner, LavfiSource, LayerSource, MultiTrackAudioMixer,
    MultiTrackComposer, ProxySource, RealtimeComposer, RealtimeLayer, RealtimeLayerDescriptor,
    SolidSource, TextSource, VideoConcatenator, VideoLayer,
};
pub use ffmpeg_token::FfmpegToken;
pub use filter_step::FilterStep;
pub use graph::FilterGraph;
pub use types::{
    DrawTextOptions, EqBand, HwAccel, PitchAlgo, Rgb, ScaleAlgorithm, ToneMap, XfadeTransition,
    YadifMode, dissolve_mask, xfade_frand, xfade_frand_field,
};
