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

use std::ffi::CStr;
use std::marker::PhantomData;
use std::os::raw::c_int;
use std::path::Path;
use std::ptr::NonNull;
use std::time::Duration;

use crate::{
    AVChannelLayout, AVCodecID, AVCodecParameters, AVColorPrimaries, AVColorRange, AVColorSpace,
    AVFormatContext, AVMediaType, AVRational, AVStream, AvError, Packet,
    avformat_close_input as ffi_avformat_close_input,
};

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

    /// Returns the number of streams in the container.
    #[must_use]
    pub fn nb_streams(&self) -> u32 {
        // SAFETY: `self.ptr` is a valid owned demux context; `nb_streams` is a plain field.
        unsafe { (*self.ptr.as_ptr()).nb_streams }
    }

    /// Returns the container duration in `AV_TIME_BASE` units (microseconds).
    #[must_use]
    pub fn duration(&self) -> i64 {
        // SAFETY: `self.ptr` is a valid owned demux context; `duration` is a plain field.
        unsafe { (*self.ptr.as_ptr()).duration }
    }

    /// Returns the `AVInputFormat` flags, or `0` when the format is unset.
    ///
    /// Used to detect live/streaming sources (`AVFMT_TS_DISCONT`).
    #[must_use]
    pub fn iformat_flags(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned demux context. `iformat` is set for
        //         every successfully opened input, but we null-check it defensively
        //         and read `flags` (a plain field) only when it is present.
        unsafe {
            let iformat = (*self.ptr.as_ptr()).iformat;
            if iformat.is_null() {
                0
            } else {
                (*iformat).flags
            }
        }
    }

    /// Returns the container's overall bit rate in bits per second, or `0` when
    /// unknown.
    #[must_use]
    pub fn bit_rate(&self) -> i64 {
        // SAFETY: `self.ptr` is a valid owned demux context; `bit_rate` is a plain field.
        unsafe { (*self.ptr.as_ptr()).bit_rate }
    }

    /// Returns the input format's short name (for example `"mov,mp4,..."`), or
    /// `None` when the format or its name is unset.
    #[must_use]
    pub fn iformat_name(&self) -> Option<String> {
        // SAFETY: `self.ptr` is a valid owned demux context. `iformat` is set for
        //         every successfully opened input, but we null-check both it and
        //         its `name` pointer defensively before reading the C string.
        unsafe {
            let iformat = (*self.ptr.as_ptr()).iformat;
            if iformat.is_null() {
                return None;
            }
            let name = (*iformat).name;
            if name.is_null() {
                return None;
            }
            Some(CStr::from_ptr(name).to_string_lossy().into_owned())
        }
    }

    /// Returns a borrowed handle to stream `index`, or `None` when out of range.
    #[must_use]
    pub fn stream(&self, index: usize) -> Option<StreamRef<'_>> {
        // SAFETY: `self.ptr` is a valid owned demux context. We bound-check `index`
        //         against `nb_streams` before indexing the `streams` array, and the
        //         entries are valid stream pointers set by FFmpeg on open.
        unsafe {
            let ctx = self.ptr.as_ptr();
            if index >= (*ctx).nb_streams as usize {
                return None;
            }
            let stream_ptr = *(*ctx).streams.add(index);
            NonNull::new(stream_ptr).map(|ptr| StreamRef {
                ptr,
                _marker: PhantomData,
            })
        }
    }

    /// Iterates the container's streams as borrowed handles.
    pub fn streams(&self) -> impl Iterator<Item = StreamRef<'_>> + '_ {
        (0..self.nb_streams() as usize).filter_map(move |i| self.stream(i))
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

/// A borrowed handle to one `AVStream` of an [`InputFormatContext`].
///
/// The lifetime ties the handle to the owning format context borrow, so it
/// cannot outlive the context. It exposes only safe scalar accessors and the
/// stream's [`codecpar`](StreamRef::codecpar); the raw `*mut AVStream` is
/// private.
#[derive(Clone, Copy, Debug)]
pub struct StreamRef<'a> {
    ptr: NonNull<AVStream>,
    _marker: PhantomData<&'a InputFormatContext>,
}

impl<'a> StreamRef<'a> {
    /// Returns the stream index within its container.
    #[must_use]
    pub fn index(&self) -> c_int {
        // SAFETY: `self.ptr` borrows a valid stream from a live format context.
        unsafe { (*self.ptr.as_ptr()).index }
    }

    /// Returns the stream's time base.
    #[must_use]
    pub fn time_base(&self) -> AVRational {
        // SAFETY: `self.ptr` borrows a valid stream from a live format context.
        unsafe { (*self.ptr.as_ptr()).time_base }
    }

    /// Returns the stream's average frame rate (may be `0/0` when unknown).
    #[must_use]
    pub fn avg_frame_rate(&self) -> AVRational {
        // SAFETY: `self.ptr` borrows a valid stream from a live format context.
        unsafe { (*self.ptr.as_ptr()).avg_frame_rate }
    }

    /// Returns a borrowed handle to the stream's codec parameters.
    #[must_use]
    pub fn codecpar(&self) -> CodecParameters<'a> {
        // SAFETY: `self.ptr` borrows a valid stream. FFmpeg allocates `codecpar`
        //         together with the stream, so it is non-null for a demuxed stream.
        unsafe {
            let par = (*self.ptr.as_ptr()).codecpar;
            CodecParameters {
                ptr: NonNull::new_unchecked(par),
                _marker: PhantomData,
            }
        }
    }
}

/// A borrowed handle to an `AVCodecParameters` owned by a stream.
///
/// Exposes the scalar fields ff-decode reads and hands its raw pointer to the
/// safe [`CodecContext::apply_parameters`](crate::CodecContext::apply_parameters)
/// via a crate-private accessor; the raw pointer is not part of the public API.
#[derive(Clone, Copy, Debug)]
pub struct CodecParameters<'a> {
    ptr: NonNull<AVCodecParameters>,
    _marker: PhantomData<&'a InputFormatContext>,
}

impl CodecParameters<'_> {
    /// Returns the media type (video / audio / ...).
    #[must_use]
    pub fn codec_type(&self) -> AVMediaType {
        // SAFETY: `self.ptr` borrows valid codec parameters from a live stream.
        unsafe { (*self.ptr.as_ptr()).codec_type }
    }

    /// Returns the codec id.
    #[must_use]
    pub fn codec_id(&self) -> AVCodecID {
        // SAFETY: `self.ptr` borrows valid codec parameters from a live stream.
        unsafe { (*self.ptr.as_ptr()).codec_id }
    }

    /// Returns the coded width in pixels (0 for non-video streams).
    #[must_use]
    pub fn width(&self) -> c_int {
        // SAFETY: `self.ptr` borrows valid codec parameters from a live stream.
        unsafe { (*self.ptr.as_ptr()).width }
    }

    /// Returns the coded height in pixels (0 for non-video streams).
    #[must_use]
    pub fn height(&self) -> c_int {
        // SAFETY: `self.ptr` borrows valid codec parameters from a live stream.
        unsafe { (*self.ptr.as_ptr()).height }
    }

    /// Returns the audio sample rate in Hz (0 for non-audio streams).
    #[must_use]
    pub fn sample_rate(&self) -> c_int {
        // SAFETY: `self.ptr` borrows valid codec parameters from a live stream.
        unsafe { (*self.ptr.as_ptr()).sample_rate }
    }

    /// Returns the color space.
    #[must_use]
    pub fn color_space(&self) -> AVColorSpace {
        // SAFETY: `self.ptr` borrows valid codec parameters from a live stream.
        unsafe { (*self.ptr.as_ptr()).color_space }
    }

    /// Returns the color range.
    #[must_use]
    pub fn color_range(&self) -> AVColorRange {
        // SAFETY: `self.ptr` borrows valid codec parameters from a live stream.
        unsafe { (*self.ptr.as_ptr()).color_range }
    }

    /// Returns the color primaries.
    #[must_use]
    pub fn color_primaries(&self) -> AVColorPrimaries {
        // SAFETY: `self.ptr` borrows valid codec parameters from a live stream.
        unsafe { (*self.ptr.as_ptr()).color_primaries }
    }

    /// Returns a copy of the channel layout (order / channel count / mask).
    #[must_use]
    pub fn ch_layout(&self) -> AVChannelLayout {
        // SAFETY: `self.ptr` borrows valid codec parameters from a live stream;
        //         `ch_layout` is a plain POD field copied out by value.
        unsafe { (*self.ptr.as_ptr()).ch_layout }
    }

    /// Returns the underlying raw pointer for FFI calls within the crate.
    pub(crate) fn as_raw(&self) -> *const AVCodecParameters {
        self.ptr.as_ptr()
    }
}

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
