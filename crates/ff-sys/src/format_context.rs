//! RAII owners for input (demux) and output (mux) `AVFormatContext`s.
//!
//! [`InputFormatContext`] opens a demuxing context and frees it exactly once on
//! drop, replacing the manual `avformat::open_input*` + `avformat::close_input`
//! pair. Its fallible methods return [`AvError`]. The packet argument of
//! [`read_frame`](InputFormatContext::read_frame) stays a raw pointer for now
//! (an owned Packet is a later step), so that method is `unsafe`: the caller
//! upholds the usual FFmpeg preconditions.
//!
//! [`OutputFormatContext`] owns the mux (output) lifecycle: it allocates a
//! muxing context, opens/closes its IO (`pb`), writes the header/trailer, and
//! frees the context exactly once on drop (closing a caller-opened `pb`),
//! replacing the manual `avformat_alloc_output_context2` + `avio_open` +
//! `avformat_free_context` teardown. Like `InputFormatContext`, it exposes a
//! transitional `as_mut_ptr` for the write path (stream creation, packet
//! writing) not yet wrapped.

use std::ffi::{CStr, CString};
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

/// An owned output (mux) `AVFormatContext`.
///
/// Allocates a muxing context and frees it exactly once on drop, closing the IO
/// (`pb`) it opened unless the muxer manages its own IO (`AVFMT_NOFILE`). This
/// replaces the manual `avformat_alloc_output_context2` + `avio_open` +
/// `avformat_free_context` teardown scattered across every mux consumer, so no
/// early-return path can leak the context.
///
/// Exactly-once free is guaranteed by construction: the value owns a
/// [`NonNull`] and is neither `Copy` nor `Clone`.
///
/// The lifecycle (allocation, IO open/close, header/trailer) is wrapped as
/// methods; the write path (`avformat_new_stream`, per-stream field setup,
/// `av_interleaved_write_frame`) still goes through [`as_mut_ptr`] for now, so
/// this type carries a transitional raw accessor like [`InputFormatContext`]
/// (both are removed when the safe layer is sealed).
///
/// [`as_mut_ptr`]: OutputFormatContext::as_mut_ptr
#[derive(Debug)]
pub struct OutputFormatContext {
    ptr: NonNull<AVFormatContext>,
}

impl OutputFormatContext {
    /// Allocates a muxing context for `filename`, optionally forcing the muxer
    /// named `format_name` (otherwise it is guessed from the filename).
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the format cannot be resolved or the context
    /// cannot be allocated.
    pub fn new(format_name: Option<&str>, filename: &Path) -> Result<Self, AvError> {
        crate::ensure_initialized();

        let c_format = match format_name {
            Some(name) => {
                Some(CString::new(name).map_err(|_| AvError::new(crate::error_codes::EINVAL))?)
            }
            None => None,
        };
        let filename_str = filename
            .to_str()
            .ok_or_else(|| AvError::new(crate::error_codes::EINVAL))?;
        let c_filename =
            CString::new(filename_str).map_err(|_| AvError::new(crate::error_codes::EINVAL))?;

        let mut ctx: *mut AVFormatContext = std::ptr::null_mut();
        // SAFETY: `ctx` is a valid out-pointer initialised to null; the two C
        //         strings outlive the call; a null `oformat` lets FFmpeg pick the
        //         muxer from `format_name` / `filename`.
        let ret = unsafe {
            crate::avformat_alloc_output_context2(
                &mut ctx,
                std::ptr::null_mut(),
                c_format.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                c_filename.as_ptr(),
            )
        };
        if ret < 0 {
            return Err(AvError::new(ret));
        }
        NonNull::new(ctx)
            .ok_or_else(|| AvError::new(crate::error_codes::ENOMEM))
            .map(|ptr| Self { ptr })
    }

    /// Returns the context pointer for read-only use.
    #[must_use]
    pub const fn as_ptr(&self) -> *const AVFormatContext {
        self.ptr.as_ptr()
    }

    /// Returns the context pointer for mutation and FFI calls (stream creation,
    /// packet writing, and per-stream field setup during migration).
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut AVFormatContext {
        self.ptr.as_ptr()
    }

    /// Returns the number of streams registered on the context.
    #[must_use]
    pub fn nb_streams(&self) -> u32 {
        // SAFETY: `self.ptr` is a valid owned mux context; `nb_streams` is a plain field.
        unsafe { (*self.ptr.as_ptr()).nb_streams }
    }

    /// Returns the `AVOutputFormat` flags, or `0` when the format is unset.
    #[must_use]
    pub fn oformat_flags(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned mux context. `oformat` is set for
        //         every successfully allocated output, but we null-check it
        //         defensively before reading `flags` (a plain field).
        unsafe {
            let oformat = (*self.ptr.as_ptr()).oformat;
            if oformat.is_null() {
                0
            } else {
                (*oformat).flags
            }
        }
    }

    /// Returns `true` when the muxer manages its own IO (`AVFMT_NOFILE`), so the
    /// caller must not open or close a `pb`.
    #[must_use]
    pub fn is_nofile(&self) -> bool {
        self.oformat_flags() & crate::constants::AVFMT_NOFILE != 0
    }

    /// Opens the output IO for `path` (write mode) and attaches it as the
    /// context's `pb`.
    ///
    /// Callers must skip this for [`is_nofile`](Self::is_nofile) muxers, which
    /// manage their own IO.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the output cannot be opened for writing.
    pub fn open_io(&mut self, path: &Path) -> Result<(), AvError> {
        // SAFETY: `open_output` validates the path and returns a freshly opened
        //         AVIO context; we take ownership of it as this context's `pb`.
        let pb = unsafe { crate::avformat::open_output(path, crate::avformat::avio_flags::WRITE) }
            .map_err(AvError::new)?;
        // SAFETY: `self.ptr` is a valid owned mux context; `pb` is a plain field.
        unsafe { (*self.ptr.as_ptr()).pb = pb };
        Ok(())
    }

    /// Writes the container header.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the header cannot be written.
    pub fn write_header(&mut self) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned mux context; no muxer options.
        let ret = unsafe { crate::avformat_write_header(self.ptr.as_ptr(), std::ptr::null_mut()) };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(())
        }
    }

    /// Writes the container trailer, finalising the output.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the trailer cannot be written.
    pub fn write_trailer(&mut self) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned mux context whose header was written.
        let ret = unsafe { crate::av_write_trailer(self.ptr.as_ptr()) };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(())
        }
    }

    /// Closes the output IO (`pb`) early, before drop.
    ///
    /// Used by segment muxers (HLS/DASH) that close the caller-opened `pb` right
    /// after the header write so the muxer can manage its own segment files. The
    /// close nulls `pb`, so a later drop does not double-close. This is a no-op
    /// when `pb` is already null.
    pub fn close_io(&mut self) {
        // SAFETY: `self.ptr` is a valid owned mux context; `close_output`
        //         null-checks `pb` and nulls it after closing.
        unsafe { crate::avformat::close_output(&mut (*self.ptr.as_ptr()).pb) };
    }
}

impl Drop for OutputFormatContext {
    fn drop(&mut self) {
        // SAFETY: we uniquely own the context (NonNull, not Copy/Clone), so this
        //         runs exactly once. Close the caller-opened `pb` if one is still
        //         open (it is null for `AVFMT_NOFILE` muxers, where the caller
        //         opened none, and after `close_io`), then free the context. A
        //         non-null `pb` is always one the caller opened and owns, so it
        //         must be closed regardless of the muxer flags — mirroring the
        //         manual `if pb != null { avio_closep }` teardown this replaces.
        //         `close_output` null-checks and nulls `pb`; `avformat_free_context`
        //         does not touch `pb`.
        unsafe {
            let ctx = self.ptr.as_ptr();
            if !(*ctx).pb.is_null() {
                crate::avformat::close_output(&mut (*ctx).pb);
            }
            crate::avformat_free_context(ctx);
        }
    }
}

// SAFETY: an `AVFormatContext` is not safe for concurrent access, but moving
//         ownership between threads is sound because Rust's ownership model
//         guarantees exclusive access.
unsafe impl Send for OutputFormatContext {}

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

    #[test]
    fn output_new_should_error_on_bogus_format() {
        // A format name that matches no muxer makes allocation fail, returning
        // before any owned context is built (nothing to leak).
        let result =
            OutputFormatContext::new(Some("definitely_not_a_real_muxer"), Path::new("out.bin"));
        assert!(result.is_err());
    }

    #[test]
    fn output_new_should_allocate_and_drop() {
        // Allocate a normal file muxer (guessed from the `.mp4` extension) and
        // drop it immediately. This exercises the success path, the
        // `oformat`-flag read, and free-on-drop with a never-opened `pb`. Skip
        // gracefully if the mp4 muxer is absent from a minimal FFmpeg build.
        let Ok(ctx) = OutputFormatContext::new(None, Path::new("out.mp4")) else {
            return;
        };
        // mp4 is a normal file muxer, not one that manages its own IO.
        assert!(!ctx.is_nofile());
        // `ctx` drops here: frees the context (its `pb` was never opened).
    }
}
