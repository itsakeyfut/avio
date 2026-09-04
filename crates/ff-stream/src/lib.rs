//! # ff-stream
//!
//! HLS and DASH adaptive streaming output - the Rust way.
//!
//! This crate provides segmented HLS and DASH output along with ABR (Adaptive
//! Bitrate) ladder generation. It exposes a safe, ergonomic Builder API and
//! completely hides `FFmpeg` muxer internals.
//!
//! ## Features
//!
//! - **HLS Output**: Segmented HLS with configurable segment duration and keyframe interval
//! - **DASH Output**: Segmented DASH with configurable segment duration
//! - **ABR Ladder**: Multi-rendition HLS / DASH from a single input in one call
//! - **Builder Pattern**: Consuming builders with validation in `build()`
//!
//! ## Usage
//!
//! ### HLS Output
//!
//! ```ignore
//! use ff_stream::HlsOutput;
//! use std::time::Duration;
//!
//! HlsOutput::new("/var/www/hls")
//!     .input("source.mp4")
//!     .segment_duration(Duration::from_secs(6))
//!     .keyframe_interval(48)
//!     .build()?
//!     .write()?;
//! ```
//!
//! ### DASH Output
//!
//! ```ignore
//! use ff_stream::DashOutput;
//! use std::time::Duration;
//!
//! DashOutput::new("/var/www/dash")
//!     .input("source.mp4")
//!     .segment_duration(Duration::from_secs(4))
//!     .build()?
//!     .write()?;
//! ```
//!
//! ### ABR Ladder
//!
//! ```ignore
//! use ff_stream::{AbrLadder, Rendition};
//!
//! AbrLadder::new("source.mp4")
//!     .add_rendition(Rendition { width: 1920, height: 1080, bitrate: 6_000_000 })
//!     .add_rendition(Rendition { width: 1280, height:  720, bitrate: 3_000_000 })
//!     .add_rendition(Rendition { width:  854, height:  480, bitrate: 1_500_000 })
//!     .hls("/var/www/hls")?;
//! ```
//!
//! ## Module Structure
//!
//! - [`hls`] — [`HlsOutput`]: segmented HLS output builder
//! - [`dash`] — [`DashOutput`]: segmented DASH output builder
//! - [`abr`] — [`AbrLadder`] + [`Rendition`]: multi-rendition ABR ladder
//! - [`error`] — [`StreamError`]: unified error type for this crate
//!
//! ## Status
//!
//! The public API is stable. HLS and DASH muxing are fully implemented via the
//! `FFmpeg` HLS/DASH muxers. `write()` / `hls()` / `dash()` perform real
//! encode-and-mux operations against the filesystem.

#![warn(missing_docs)]

#[macro_use]
mod builder_macros;

pub mod abr;
pub(crate) mod codec_utils_inner;
pub mod dash;
pub(crate) mod dash_inner;
/// Unified error type for the `ff-stream` crate.
pub mod error;
pub mod fanout;
pub mod hls;
pub(crate) mod hls_inner;
pub mod live_abr;
pub mod live_dash;
pub(crate) mod live_dash_inner;
pub mod live_hls;
pub(crate) mod live_hls_inner;
pub(crate) mod muxer_inner;
pub mod output;
pub mod rtmp;
pub(crate) mod rtmp_inner;
#[cfg(feature = "srt")]
pub mod srt_output;
#[cfg(feature = "srt")]
pub(crate) mod srt_output_inner;

pub use abr::{AbrLadder, Rendition};
pub use dash::DashOutput;
pub use error::StreamError;
pub use fanout::FanoutOutput;
pub use ff_format::{ErrorSeverity, MediaError};
pub use hls::{HlsOutput, HlsSegmentFormat};
pub use live_abr::{AbrRendition, LiveAbrFormat, LiveAbrLadder};
pub use live_dash::LiveDashOutput;
pub use live_hls::LiveHlsOutput;
pub use output::StreamOutput;
pub use rtmp::RtmpOutput;
#[cfg(feature = "srt")]
pub use srt_output::SrtOutput;
