//! Audio encoding implementation.
//!
//! This module provides audio encoding functionality with an `FFmpeg` backend.
//! The implementation is split into public API ([`builder`]) and internal
//! implementation details ([`encoder_inner`]).

#[cfg(feature = "tokio")]
pub mod async_encoder;
pub mod builder;
pub mod codec_options;
mod encoder_inner;

#[cfg(feature = "tokio")]
pub use async_encoder::AsyncAudioEncoder;
pub use builder::{AudioEncoder, AudioEncoderBuilder};
pub use codec_options::{
    AacOptions, AacProfile, AudioCodecOptions, FlacOptions, Mp3Options, Mp3Quality,
    OpusApplication, OpusOptions,
};
