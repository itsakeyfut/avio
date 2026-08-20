//! # ff-remux
//!
//! Stream-copy remuxing over `FFmpeg`: trim a clip, and replace / extract / add an
//! audio stream, all **without re-encoding** (a bitstream copy through
//! libavformat's demuxer and muxer).
//!
//! This is a distinct `FFmpeg` area from encoding (`ff-encode` wraps libavcodec);
//! `ff-remux` wraps libavformat stream copy. Use it on its own, or combine it with
//! the other `ff-*` crates. Errors are typed and contextual ([`RemuxError`]).
//!
//! Part of the [`avio`](https://github.com/itsakeyfut/avio) crate family; each
//! crate is versioned independently.

// FFmpeg's C API is called through `unsafe`; the FFI is isolated in the
// `*_inner` modules, which carry their own scoped clippy allows for the
// FFmpeg-boundary lints (casts, pointer idioms).
#![allow(unsafe_code)]

mod error;
mod media_ops;
mod trim;

pub use error::RemuxError;
pub use ff_format::{ErrorSeverity, MediaError};
pub use media_ops::{AudioAdder, AudioExtractor, AudioReplacement};
pub use trim::{StreamCopyTrim, StreamCopyTrimmer};
