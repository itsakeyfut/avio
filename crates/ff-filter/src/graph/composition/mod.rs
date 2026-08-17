//! Multi-track video composition and audio mixing.
//!
//! This module provides [`MultiTrackComposer`] for compositing multiple video
//! layers onto a solid-colour canvas, and [`MultiTrackAudioMixer`] for mixing
//! multiple audio tracks into a single output stream.
//!
//! Both types produce source-only `FilterGraph` instances — call
//! `FilterGraph::pull_video` or `FilterGraph::pull_audio` in a loop to
//! extract output frames.

mod audio_concatenator;
mod clip_joiner;
pub(super) mod composition_inner;
mod multi_track_composer;
mod multi_track_mixer;
mod realtime_composer;
mod video_concatenator;

pub use audio_concatenator::AudioConcatenator;
pub use clip_joiner::ClipJoiner;
pub use multi_track_composer::{MultiTrackComposer, ProxySource, VideoLayer};
pub use multi_track_mixer::{AudioTrack, MultiTrackAudioMixer};
pub use realtime_composer::{
    LavfiSource, RealtimeComposer, RealtimeLayer, RealtimeLayerDescriptor,
};
pub use video_concatenator::VideoConcatenator;
