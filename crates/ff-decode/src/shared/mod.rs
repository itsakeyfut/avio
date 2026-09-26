//! Shared utility types for ff-decode.

pub(crate) mod guards_inner;
mod hardware;
pub(crate) mod network;
mod seek;
pub(crate) mod stream_duration;

pub use hardware::HardwareAccel;
pub use seek::SeekMode;
