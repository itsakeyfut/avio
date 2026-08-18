//! # ff-remux
//!
//! Stream-copy remuxing over FFmpeg: trim a clip, and replace / extract / add an
//! audio stream, all **without re-encoding** (a bitstream copy through
//! libavformat's demuxer and muxer).
//!
//! This is a distinct FFmpeg area from encoding (`ff-encode` wraps libavcodec);
//! `ff-remux` wraps libavformat stream copy. Use it on its own, or combine it with
//! the other `ff-*` crates. Errors are typed and contextual ([`RemuxError`]).
//!
//! Part of the [`avio`](https://github.com/itsakeyfut/avio) crate family; each
//! crate is versioned independently.

#![allow(unsafe_code)]
#![allow(clippy::borrow_as_ptr)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::ref_as_ptr)]
#![allow(clippy::unnecessary_safety_doc)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::manual_c_str_literals)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::single_match_else)]
#![allow(clippy::inefficient_to_string)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::assigning_clones)]
#![allow(clippy::unused_self)]
#![allow(clippy::unnecessary_safety_comment)]

mod error;
mod media_ops;
mod trim;

pub use error::RemuxError;
pub use ff_format::{ErrorSeverity, MediaError};
pub use media_ops::{AudioAdder, AudioExtractor, AudioReplacement};
pub use trim::{StreamCopyTrim, StreamCopyTrimmer};
