//! # ff-preview
//!
//! Real-time video/audio preview and proxy workflow for the `avio` crate family.
//!
//! This crate provides single-file playback (`PreviewPlayer`) with frame-accurate
//! seek, A/V sync, and an optional proxy generation workflow.
//!
//! ## Feature Flags
//!
//! | Feature | Description | Default |
//! |---------|-------------|---------|
//! | `tokio` | Async `AsyncPreviewPlayer` backed by `spawn_blocking` | no |
//! | `proxy` | `ProxyGenerator` for lower-resolution proxy files | no |
//!
//! ## Usage
//!
//! ```ignore
//! use ff_preview::{PreviewPlayer, RgbaSink};
//!
//! let mut player = PreviewPlayer::open("clip.mp4")?;
//! player.set_sink(Box::new(RgbaSink::new()));
//! player.play();
//! player.run()?;
//! ```

#![warn(clippy::all)]
#![warn(clippy::pedantic)]

pub mod audio;
pub(crate) mod cache;
pub mod error;
pub mod event;
pub mod playback;

#[cfg(feature = "proxy")]
pub mod proxy;

#[cfg(feature = "timeline")]
pub mod scene;

pub use audio::{AudioMixer, AudioTrackHandle};
pub use error::PreviewError;
pub use event::PlayerEvent;
pub use ff_decode::HardwareAccel;
pub use playback::{
    DecodeBuffer, DecodeBufferBuilder, FrameResult, FrameSink, PlaybackClock, PlayerCommand,
    PlayerHandle, PlayerRunner, PreviewPlayer, RgbaFrame, RgbaSink, SeekEvent,
};

#[cfg(feature = "tokio")]
pub use playback::AsyncPreviewPlayer;

#[cfg(feature = "proxy")]
pub use proxy::{ProxyGenerator, ProxyJob, ProxyResolution};

#[cfg(feature = "timeline")]
pub use scene::{
    Scene, SceneAudioPlacement, SceneAudioTrack, ScenePlacement, ScenePlayer, SceneRunner,
    SceneVideoTrack,
};
