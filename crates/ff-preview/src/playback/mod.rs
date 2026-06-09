//! Real-time playback types for ff-preview.
//!
//! This module exposes the primary public API for single-file video/audio
//! playback. All `unsafe` `FFmpeg` calls are isolated in `playback_inner`.
//!
//! | Sub-module         | Contents |
//! |--------------------|---------|
//! | `clock`            | [`PlaybackClock`] |
//! | `master_clock`     | Internal `MasterClock` |
//! | `sink`             | [`FrameSink`] trait, [`RgbaFrame`], [`RgbaSink`] |
//! | `decode_buffer`    | [`DecodeBuffer`], [`FrameResult`], [`SeekEvent`] |
//! | `player`           | [`PreviewPlayer`], [`PlayerCommand`] |
//! | `player_handle`    | [`PlayerHandle`] |
//! | `player_runner`    | [`PlayerRunner`] |
//! | `async_player`     | [`AsyncPreviewPlayer`] (tokio feature) |
//! | `playback_inner`   | Unsafe `FFmpeg` calls (`SwsRgbaConverter`, PCM extraction) |

mod playback_inner;

#[cfg(feature = "timeline")]
pub(crate) use playback_inner::SwsRgbaConverter;

pub(crate) mod clock;
pub(crate) mod decode_buffer;
pub(crate) mod master_clock;
pub(crate) mod player;
pub(crate) mod player_handle;
pub(crate) mod player_runner;
pub(crate) mod sink;

#[cfg(feature = "tokio")]
pub(crate) mod async_player;

pub use clock::PlaybackClock;
pub use decode_buffer::{DecodeBuffer, DecodeBufferBuilder, FrameResult, SeekEvent};
pub use player::{PlayerCommand, PreviewPlayer};
pub use player_handle::PlayerHandle;
pub use player_runner::PlayerRunner;
pub use sink::{FrameSink, RgbaFrame, RgbaSink};

#[cfg(feature = "tokio")]
pub use async_player::AsyncPreviewPlayer;
