//! Stream-copy trimming — cut a media file to a time range without re-encoding.

mod trim_inner;
mod trimmer;

pub use trimmer::{StreamCopyTrim, StreamCopyTrimmer};
