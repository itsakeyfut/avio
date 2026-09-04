//! Shared utility types for ff-encode.

mod bitrate;
mod codec;
pub(crate) mod codec_opts;
mod container;
mod hardware;
mod preset;
mod progress;

pub use bitrate::{BitrateMode, CRF_MAX};
pub use codec::{AudioCodec, VideoCodec, VideoCodecEncodeExt};
pub use container::OutputContainer;
pub use hardware::HardwareEncoder;
pub use preset::Preset;
pub use progress::{EncodeProgress, EncodeProgressCallback};
