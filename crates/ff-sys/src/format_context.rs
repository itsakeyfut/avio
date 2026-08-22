//! RAII owner for an input (demux) `AVFormatContext`.
//!
//! [`InputFormatContext`] opens a demuxing context and frees it exactly once on
//! drop, replacing the manual `avformat::open_input*` + `avformat::close_input`
//! pair. Its fallible methods return [`AvError`]. The packet argument of
//! [`read_frame`](InputFormatContext::read_frame) stays a raw pointer for now
//! (an owned Packet is a later step), so that method is `unsafe`: the caller
//! upholds the usual FFmpeg preconditions.
//!
//! The mux (output) lifecycle is a separate owner (`OutputFormatContext`,
//! tracked in #1493); this type covers demuxing only.

use std::os::raw::c_int;
use std::path::Path;
use std::ptr::NonNull;
use std::time::Duration;

use crate::{AVFormatContext, AvError, Packet, avformat_close_input as ffi_avformat_close_input};

/// An owned input (demux) `AVFormatContext`.
///
/// The context is freed exactly once on drop. This is guaranteed by
/// construction: the value owns a [`NonNull`] and is neither `Copy` nor `Clone`,
/// so it drops exactly once and cannot be duplicated.
#[derive(Debug)]
pub struct InputFormatContext {
    ptr: NonNull<AVFormatContext>,
}

impl InputFormatContext {
    /// Opens a media file and reads its header.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the path is invalid or the file cannot be
    /// opened as a recognised media format.
    pub fn open(path: &Path) -> Result<Self, AvError> {
        // SAFETY: `open_input` initialises FFmpeg and validates the path; the
        //         returned context is owned by `self` and freed on drop.
        let ptr = unsafe { crate::avformat::open_input(path) }.map_err(AvError::new)?;
        Self::from_raw(ptr)
    }

    /// Opens a network URL with connect/read timeouts.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the URL is invalid, the host is unreachable,
    /// the connection times out, or the format is not recognised.
    pub fn open_url(
        url: &str,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<Self, AvError> {
        // SAFETY: `open_input_url` initialises FFmpeg and validates the URL; the
        //         returned context is owned by `self` and freed on drop.
        let ptr = unsafe { crate::avformat::open_input_url(url, connect_timeout, read_timeout) }
            .map_err(AvError::new)?;
        Self::from_raw(ptr)
    }

    /// Opens an image sequence using the `image2` demuxer at `framerate`.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the path is invalid, the sequence cannot be
    /// opened, or the `image2` demuxer is unavailable.
    pub fn open_image_sequence(path: &Path, framerate: u32) -> Result<Self, AvError> {
        // SAFETY: `open_input_image_sequence` initialises FFmpeg and validates the
        //         path; the returned context is owned by `self` and freed on drop.
        let ptr = unsafe { crate::avformat::open_input_image_sequence(path, framerate) }
            .map_err(AvError::new)?;
        Self::from_raw(ptr)
    }

    /// Wraps a freshly opened, non-null context pointer in the owned type.
    fn from_raw(ptr: *mut AVFormatContext) -> Result<Self, AvError> {
        // The `open_*` wrappers return `Ok` only with a non-null context.
        NonNull::new(ptr)
            .ok_or_else(|| AvError::new(crate::error_codes::ENOMEM))
            .map(|ptr| Self { ptr })
    }

    /// Returns the context pointer for read-only use.
    #[must_use]
    pub const fn as_ptr(&self) -> *const AVFormatContext {
        self.ptr.as_ptr()
    }

    /// Returns the context pointer for mutation and FFI calls.
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut AVFormatContext {
        self.ptr.as_ptr()
    }

    /// Reads stream information, populating per-stream codec parameters.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if stream information cannot be read.
    pub fn find_stream_info(&mut self) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned demux context.
        unsafe { crate::avformat::find_stream_info(self.ptr.as_ptr()) }.map_err(AvError::new)
    }

    /// Seeks to `timestamp` (in `stream_index`'s time base) using `flags`.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if seeking fails.
    pub fn seek_frame(
        &mut self,
        stream_index: c_int,
        timestamp: i64,
        flags: c_int,
    ) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned demux context.
        unsafe { crate::avformat::seek_frame(self.ptr.as_ptr(), stream_index, timestamp, flags) }
            .map_err(AvError::new)
    }

    /// Seeks to `ts` within the `[min_ts, max_ts]` window using `flags`.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if seeking fails.
    pub fn seek_file(
        &mut self,
        stream_index: c_int,
        min_ts: i64,
        ts: i64,
        max_ts: i64,
        flags: c_int,
    ) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned demux context.
        unsafe {
            crate::avformat::seek_file(self.ptr.as_ptr(), stream_index, min_ts, ts, max_ts, flags)
        }
        .map_err(AvError::new)
    }

    /// Reads the next packet into `pkt`.
    ///
    /// End-of-stream surfaces as an [`AvError`] for which [`AvError::is_eof`]
    /// returns `true`, rather than a distinct outcome.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] on read failure or at end-of-stream (`EOF`).
    pub fn read_frame(&mut self, pkt: &mut Packet) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned demux context; `pkt` is a valid owned packet.
        unsafe { crate::avformat::read_frame(self.ptr.as_ptr(), pkt.as_mut_ptr()) }
            .map_err(AvError::new)
    }
}

impl Drop for InputFormatContext {
    fn drop(&mut self) {
        // SAFETY: we uniquely own the context (NonNull, not Copy/Clone), so this
        //         runs exactly once. `avformat_close_input` closes the input and
        //         frees the context, writing null into our local copy of the
        //         pointer, which is then discarded. The raw binding is used
        //         directly so this type does not depend on the
        //         `avformat::close_input` wrapper.
        unsafe {
            let mut raw = self.ptr.as_ptr();
            ffi_avformat_close_input(&mut raw);
        }
    }
}

// SAFETY: an `AVFormatContext` is not safe for concurrent access, but moving
//         ownership between threads is sound because Rust's ownership model
//         guarantees exclusive access.
unsafe impl Send for InputFormatContext {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_should_error_on_missing_path() {
        // Opening a guaranteed-nonexistent path fails without allocating a
        // context (the error path returns before any owned value is built).
        let result = InputFormatContext::open(Path::new("/nonexistent/path/to/file.mp4"));
        assert!(result.is_err());
    }
}
