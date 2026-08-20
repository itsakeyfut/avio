//! Video encoding implementation.
//!
//! This module provides video encoding functionality with an `FFmpeg` backend.
//! The implementation is split into public API ([`builder`]) and internal
//! implementation details ([`encoder_inner`]).

#[cfg(feature = "tokio")]
pub mod async_encoder;
pub mod builder;
pub mod codec_options;
mod encoder_inner;

#[cfg(feature = "tokio")]
pub use async_encoder::AsyncVideoEncoder;
pub use builder::{VideoEncoder, VideoEncoderBuilder};
pub use codec_options::{
    Av1Options, Av1Usage, DnxhdOptions, DnxhdVariant, H264Options, H264Preset, H264Profile,
    H264Tune, H265Options, H265Profile, H265Tier, ProResOptions, ProResProfile, SvtAv1Options,
    VideoCodecOptions, Vp9Options,
};
