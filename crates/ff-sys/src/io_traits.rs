//! The Rust side of a custom `AVIOContext`: what a caller may hand FFmpeg as a
//! byte source or sink.
//!
//! Both traits are blanket-implemented, so a caller passes a plain
//! `std::io::Cursor`, `File`, or anything else meeting the bounds; they exist
//! only so the boxed form has a name to be stored under.
//!
//! `Seek` is required rather than optional. FFmpeg accepts a null seek callback,
//! but a demuxer that cannot seek behaves differently enough (probing, and the
//! containers it can open at all) that supporting it is its own piece of work.
//!
//! `Send` is required because [`InputFormatContext`](crate::InputFormatContext)
//! and [`OutputFormatContext`](crate::OutputFormatContext) are `Send`: the source
//! travels with the context it is attached to.

use std::io::{Read, Seek, Write};

/// A byte source FFmpeg can demux from.
pub trait IoSource: Read + Seek + Send {}

impl<T: Read + Seek + Send> IoSource for T {}

/// A byte sink FFmpeg can mux into.
pub trait IoSink: Write + Seek + Send {}

impl<T: Write + Seek + Send> IoSink for T {}
