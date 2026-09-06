//! RAII owners for input (demux) and output (mux) `AVFormatContext`s.
//!
//! [`InputFormatContext`] opens a demuxing context and frees it exactly once on
//! drop, replacing the manual `avformat::open_input*` + `avformat::close_input`
//! pair. Its fallible methods return [`AvError`]. The packet argument of
//! [`read_frame`](InputFormatContext::read_frame) is the owned [`Packet`], so no
//! public signature exposes a raw pointer.
//!
//! [`OutputFormatContext`] owns the mux (output) lifecycle: it allocates a
//! muxing context, opens/closes its IO (`pb`), writes the header/trailer, and
//! frees the context exactly once on drop (closing a caller-opened `pb`),
//! replacing the manual `avformat_alloc_output_context2` + `avio_open` +
//! `avformat_free_context` teardown. The write path (stream creation, packet
//! writing, metadata / attachments / chapters) is exposed entirely through safe
//! methods.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::os::raw::c_int;
use std::path::Path;
use std::ptr::NonNull;
use std::time::Duration;

use crate::io_context::IoContext;
use crate::io_traits::{IoSink, IoSource};
use crate::{
    AV_DICT_IGNORE_SUFFIX, AV_INPUT_BUFFER_PADDING_SIZE, AV_TIME_BASE, AVChannelLayout, AVChapter,
    AVCodecID, AVCodecID_AV_CODEC_ID_BIN_DATA, AVCodecParameters, AVColorPrimaries, AVColorRange,
    AVColorSpace, AVDictionary, AVDictionaryEntry, AVFormatContext, AVMediaType,
    AVMediaType_AVMEDIA_TYPE_ATTACHMENT, AVRational, AVStream, AvError, Codec, CodecContext,
    Packet, av_dict_get as ffi_av_dict_get, av_dict_set as ffi_av_dict_set,
    av_interleaved_write_frame as ffi_av_interleaved_write_frame, av_mallocz as ffi_av_mallocz,
    av_opt_set as ffi_av_opt_set, avcodec_parameters_copy as ffi_avcodec_parameters_copy,
    avformat_alloc_context as ffi_avformat_alloc_context,
    avformat_close_input as ffi_avformat_close_input,
    avformat_new_stream as ffi_avformat_new_stream, avformat_open_input as ffi_avformat_open_input,
};

/// Collects every entry of an `AVDictionary` into a map.
///
/// Returns an empty map when `dict` is null. Keys and values are decoded lossily
/// from their C strings. Shared by the container / stream / chapter metadata
/// accessors so the raw `av_dict_get` iteration stays inside this crate.
///
/// # Safety
///
/// `dict` must be null or a valid `AVDictionary` borrowed for the duration of the
/// call.
unsafe fn read_dict(dict: *const AVDictionary) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if dict.is_null() {
        return map;
    }
    // An empty key with AV_DICT_IGNORE_SUFFIX walks every entry when each call is
    // seeded with the previously returned entry.
    let flags = AV_DICT_IGNORE_SUFFIX.cast_signed();
    let mut entry: *const AVDictionaryEntry = std::ptr::null();
    loop {
        // SAFETY: `dict` is a valid dictionary; `av_dict_get` returns the entry
        //         after `entry` (or null at the end). The empty key is a valid C
        //         string.
        entry = unsafe { ffi_av_dict_get(dict, c"".as_ptr(), entry, flags) };
        if entry.is_null() {
            break;
        }
        // SAFETY: a non-null entry exposes valid `key` / `value` pointers.
        let (key_ptr, value_ptr) = unsafe { ((*entry).key, (*entry).value) };
        if key_ptr.is_null() || value_ptr.is_null() {
            continue;
        }
        // SAFETY: both pointers are non-null, NUL-terminated C strings owned by
        //         the dictionary and valid for this borrow.
        let key = unsafe { CStr::from_ptr(key_ptr) }
            .to_string_lossy()
            .into_owned();
        let value = unsafe { CStr::from_ptr(value_ptr) }
            .to_string_lossy()
            .into_owned();
        map.insert(key, value);
    }
    map
}

/// An owned input (demux) `AVFormatContext`.
///
/// The context is freed exactly once on drop. This is guaranteed by
/// construction: the value owns a [`NonNull`] and is neither `Copy` nor `Clone`,
/// so it drops exactly once and cannot be duplicated.
#[derive(Debug)]
pub struct InputFormatContext {
    ptr: NonNull<AVFormatContext>,
    /// The custom `AVIOContext` this input reads through, when it was opened from
    /// a Rust source rather than a path or URL.
    ///
    /// Declared after `ptr` on purpose: `Drop::drop` runs before any field is
    /// dropped, so the format context is closed first and this is released after,
    /// which is the only order in which the demuxer can never call back into a
    /// freed source.
    ///
    /// Never read: holding it *is* its job. Unlike the output side, nothing has to
    /// consult it, because `AVFMT_FLAG_CUSTOM_IO` already stops
    /// `avformat_close_input` from touching the `pb`.
    #[allow(dead_code)]
    io: Option<IoContext>,
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

    /// Opens `source` as the input, demuxing through a custom `AVIOContext`
    /// instead of a path or URL.
    ///
    /// The format is autodetected from the bytes the source yields, so the caller
    /// does not name it. `source` is moved into the context and dropped with it.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the context cannot be allocated or the source
    /// does not yield a recognised media format.
    pub fn open_custom(source: impl IoSource + 'static) -> Result<Self, AvError> {
        crate::ensure_initialized();
        let io = IoContext::reader(source)?;

        // SAFETY: allocates a fresh demux context or returns null.
        let ctx = unsafe { ffi_avformat_alloc_context() };
        let ctx = NonNull::new(ctx).ok_or_else(|| AvError::new(crate::error_codes::ENOMEM))?;

        // SAFETY: `ctx` is the context just allocated and not yet shared. `pb` and
        //         `flags` are plain fields. `avformat.h` prescribes exactly this for
        //         custom IO: "preallocate the format context and set its pb field".
        //
        //         `AVFMT_FLAG_CUSTOM_IO` is what stops `avformat_close_input` from
        //         closing a `pb` it does not own. Setting it here is belt and
        //         braces: measured on this FFmpeg, `avformat_open_input` sets the
        //         flag itself when it finds a `pb` already in place (with the line
        //         below removed the opened context still reports 0x200080). That is
        //         not something the header promises, and the flag's documented
        //         meaning is exactly this situation, so it is stated rather than
        //         assumed.
        unsafe {
            (*ctx.as_ptr()).pb = io.as_ptr();
            (*ctx.as_ptr()).flags |= crate::constants::AVFMT_FLAG_CUSTOM_IO;
        }

        let mut raw = ctx.as_ptr();
        // SAFETY: `raw` points at the context just allocated. A null url and format
        //         let FFmpeg probe the custom `pb`. Per `avformat.h`, a user-supplied
        //         context "will be freed on failure and its pointer set to NULL", so
        //         the error path below must not free it again.
        let ret = unsafe {
            ffi_avformat_open_input(
                &mut raw,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if ret < 0 {
            // FFmpeg freed the format context. `io` drops here, releasing the
            // AVIOContext, its buffer and the source exactly once.
            return Err(AvError::new(ret));
        }

        NonNull::new(raw)
            .ok_or_else(|| AvError::new(crate::error_codes::ENOMEM))
            .map(|ptr| Self { ptr, io: Some(io) })
    }

    /// Wraps a freshly opened, non-null context pointer in the owned type.
    fn from_raw(ptr: *mut AVFormatContext) -> Result<Self, AvError> {
        // The `open_*` wrappers return `Ok` only with a non-null context.
        NonNull::new(ptr)
            .ok_or_else(|| AvError::new(crate::error_codes::ENOMEM))
            .map(|ptr| Self { ptr, io: None })
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

    /// Returns the input format's long, human-readable name (for example
    /// `"QuickTime / MOV"`), or `None` when the format or its long name is unset.
    #[must_use]
    pub fn iformat_long_name(&self) -> Option<String> {
        // SAFETY: `self.ptr` is a valid owned demux context. `iformat` is set for
        //         every successfully opened input, but we null-check both it and
        //         its `long_name` pointer defensively before reading the C string.
        unsafe {
            let iformat = (*self.ptr.as_ptr()).iformat;
            if iformat.is_null() {
                return None;
            }
            let long_name = (*iformat).long_name;
            if long_name.is_null() {
                return None;
            }
            Some(CStr::from_ptr(long_name).to_string_lossy().into_owned())
        }
    }

    /// Returns the container-level metadata as a key/value map (empty when the
    /// container carries none).
    #[must_use]
    pub fn metadata(&self) -> HashMap<String, String> {
        // SAFETY: `self.ptr` is a valid owned demux context; `metadata` is a plain
        //         field (null when absent) and `read_dict` handles null.
        unsafe { read_dict((*self.ptr.as_ptr()).metadata) }
    }

    /// Returns the number of chapters in the container.
    #[must_use]
    pub fn nb_chapters(&self) -> u32 {
        // SAFETY: `self.ptr` is a valid owned demux context; `nb_chapters` is a plain field.
        unsafe { (*self.ptr.as_ptr()).nb_chapters }
    }

    /// Returns a borrowed handle to chapter `index`, or `None` when out of range.
    #[must_use]
    pub fn chapter(&self, index: usize) -> Option<ChapterRef<'_>> {
        // SAFETY: `self.ptr` is a valid owned demux context. We bound-check `index`
        //         against `nb_chapters` before indexing the `chapters` array, and
        //         the entries are valid chapter pointers set by FFmpeg on open.
        unsafe {
            let ctx = self.ptr.as_ptr();
            if index >= (*ctx).nb_chapters as usize {
                return None;
            }
            let chapter_ptr = *(*ctx).chapters.add(index);
            NonNull::new(chapter_ptr).map(|ptr| ChapterRef {
                ptr,
                _marker: PhantomData,
            })
        }
    }

    /// Iterates the container's chapters as borrowed handles.
    pub fn chapters(&self) -> impl Iterator<Item = ChapterRef<'_>> + '_ {
        (0..self.nb_chapters() as usize).filter_map(move |i| self.chapter(i))
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
        //         `avformat::close_input` wrapper. For a custom-IO input the
        //         context carries `AVFMT_FLAG_CUSTOM_IO`, so this leaves `pb`
        //         alone; the `io` field is dropped after this body and frees it.
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
/// The lifecycle (allocation, IO open/close, header/trailer) and the write path
/// (`avformat_new_stream`, per-stream field setup, `av_interleaved_write_frame`)
/// are exposed entirely through safe methods, so no public signature exposes the
/// raw context pointer.
#[derive(Debug)]
pub struct OutputFormatContext {
    ptr: NonNull<AVFormatContext>,
    /// The custom `AVIOContext` this output writes through, when one was attached
    /// with [`set_custom_io`](Self::set_custom_io).
    ///
    /// Its presence is what tells [`close_io`](Self::close_io) and `Drop` that the
    /// `pb` is **not** one `avio_open` produced and must not be `avio_closep`-ed.
    io: Option<IoContext>,
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
            .map(|ptr| Self { ptr, io: None })
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

    /// Attaches `sink` as the context's `pb`, so the muxer writes into a Rust
    /// sink instead of a file.
    ///
    /// Use instead of [`open_io`](Self::open_io). Calling it after `open_io` is
    /// not a leak -- the file's `pb` is closed first -- but it is not a useful
    /// thing to do. Callers must still skip both for
    /// [`is_nofile`](Self::is_nofile) muxers, which manage their own IO.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the `AVIOContext` cannot be allocated.
    pub fn set_custom_io(&mut self, sink: impl IoSink + 'static) -> Result<(), AvError> {
        let io = IoContext::writer(sink)?;
        // SAFETY: `self.ptr` is a valid owned mux context; `pb` and `flags` are
        //         plain fields. A `pb` already in place is closed first: if it came
        //         from `open_io` it is an `avio_open` context nothing else would
        //         ever release (`Drop` takes the custom-IO branch once `io` is set),
        //         so overwriting it would leak the context and its file handle.
        //         `close_output` null-checks and nulls `pb`, and `self.io` is `None`
        //         on that path, so nothing is freed twice.
        //         `AVFMT_FLAG_CUSTOM_IO` marks the `pb` as one the caller owns,
        //         which is also what `close_io` and `Drop` read off the `io` field
        //         below to decide not to `avio_closep` it.
        unsafe {
            if self.io.is_none() && !(*self.ptr.as_ptr()).pb.is_null() {
                crate::avformat::close_output(&mut (*self.ptr.as_ptr()).pb);
            }
            (*self.ptr.as_ptr()).pb = io.as_ptr();
            (*self.ptr.as_ptr()).flags |= crate::constants::AVFMT_FLAG_CUSTOM_IO;
        }
        self.io = Some(io);
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
    /// A custom `pb` ([`set_custom_io`](Self::set_custom_io)) is released rather
    /// than closed: `avio_closep` would free a context this type does not own that
    /// way, so the owned context is dropped instead, which frees the AVIO context,
    /// its buffer and the sink exactly once.
    pub fn close_io(&mut self) {
        if let Some(io) = self.io.take() {
            // SAFETY: `self.ptr` is a valid owned mux context; `pb` is a plain
            //         field. It is nulled before `io` is dropped so the context
            //         never holds a dangling `pb`.
            unsafe { (*self.ptr.as_ptr()).pb = std::ptr::null_mut() };
            drop(io);
            return;
        }
        // SAFETY: `self.ptr` is a valid owned mux context; `close_output`
        //         null-checks `pb` and nulls it after closing.
        unsafe { crate::avformat::close_output(&mut (*self.ptr.as_ptr()).pb) };
    }

    /// Returns the raw `*mut AVStream` at `idx`, or null when out of range.
    fn stream_ptr(&self, idx: usize) -> *mut AVStream {
        // SAFETY: `self.ptr` is a valid owned mux context. We bound-check `idx`
        //         against `nb_streams` before indexing the `streams` array.
        unsafe {
            let ctx = self.ptr.as_ptr();
            if idx >= (*ctx).nb_streams as usize {
                std::ptr::null_mut()
            } else {
                *(*ctx).streams.add(idx)
            }
        }
    }

    /// Adds a new output stream (optionally bound to `codec`) and returns its index.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the stream cannot be allocated.
    pub fn new_stream(&mut self, codec: Option<&Codec>) -> Result<usize, AvError> {
        let codec_ptr = codec.map_or(std::ptr::null(), Codec::as_ptr);
        // SAFETY: `self.ptr` is a valid owned mux context; `codec_ptr` is null or a
        //         valid `*const AVCodec` borrowed from `codec` for the call.
        let stream = unsafe { ffi_avformat_new_stream(self.ptr.as_ptr(), codec_ptr) };
        if stream.is_null() {
            return Err(AvError::new(crate::error_codes::ENOMEM));
        }
        // The stream is appended, so its index is the new `nb_streams - 1`.
        Ok(self.nb_streams() as usize - 1)
    }

    /// Returns the time base of output stream `idx`, or `0/0` when out of range.
    #[must_use]
    pub fn stream_time_base(&self, idx: usize) -> AVRational {
        let stream = self.stream_ptr(idx);
        if stream.is_null() {
            AVRational { num: 0, den: 0 }
        } else {
            // SAFETY: `stream` is a valid non-null stream from this context.
            unsafe { (*stream).time_base }
        }
    }

    /// Sets the time base of output stream `idx` (no-op when out of range).
    pub fn set_stream_time_base(&mut self, idx: usize, time_base: AVRational) {
        let stream = self.stream_ptr(idx);
        if !stream.is_null() {
            // SAFETY: `stream` is a valid non-null stream from this context.
            unsafe { (*stream).time_base = time_base };
        }
    }

    /// Copies `ctx`'s codec parameters into output stream `idx` (encoder → stream).
    ///
    /// The encoder-side counterpart of [`copy_stream_params`](Self::copy_stream_params)
    /// (which copies from another stream's parameters for stream-copy remuxing).
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if `idx` is out of range or the copy fails.
    pub fn apply_stream_params_from_context(
        &mut self,
        idx: usize,
        ctx: &CodecContext,
    ) -> Result<(), AvError> {
        let stream = self.stream_ptr(idx);
        if stream.is_null() {
            return Err(AvError::new(crate::error_codes::EINVAL));
        }
        // SAFETY: `stream` is valid; its `codecpar` is allocated with it and non-null.
        //         `parameters_from_context` copies into that codecpar.
        unsafe {
            let par = (*stream).codecpar;
            ctx.parameters_from_context(par)
        }
    }

    /// Copies `src` codec parameters into output stream `idx` (stream copy) and
    /// clears its `codec_tag` so the muxer assigns the container's value.
    ///
    /// The stream-copy counterpart of
    /// [`apply_stream_params_from_context`](Self::apply_stream_params_from_context)
    /// (which copies from an encoder context).
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if `idx` is out of range or the copy fails.
    pub fn copy_stream_params(
        &mut self,
        idx: usize,
        src: CodecParameters<'_>,
    ) -> Result<(), AvError> {
        let stream = self.stream_ptr(idx);
        if stream.is_null() {
            return Err(AvError::new(crate::error_codes::EINVAL));
        }
        // SAFETY: `stream` is valid; `dst`/`src.as_raw()` are non-null codecpar
        //         pointers (allocated with their streams); the copy is a deep copy.
        unsafe {
            let dst = (*stream).codecpar;
            let ret = ffi_avcodec_parameters_copy(dst, src.as_raw());
            if ret < 0 {
                return Err(AvError::new(ret));
            }
            (*dst).codec_tag = 0;
        }
        Ok(())
    }

    /// Sets a private muxer option (`av_opt_set` on the muxer's `priv_data`).
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the option is unrecognised or cannot be set.
    pub fn set_opt(&mut self, key: &CStr, value: &CStr) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned mux context. `priv_data` is the muxer's
        //         option object, which is null for a muxer without private options;
        //         `av_opt_set` (via `av_opt_find2`) null-checks `obj` first and then
        //         returns `AVERROR_OPTION_NOT_FOUND` without dereferencing it, so a
        //         null `priv_data` is a returned error, not a deref. `key`/`value`
        //         outlive the call.
        let ret = unsafe {
            ffi_av_opt_set(
                (*self.ptr.as_ptr()).priv_data,
                key.as_ptr(),
                value.as_ptr(),
                0,
            )
        };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(())
        }
    }

    /// Writes `pkt` to the output, interleaving it into the muxer's queue.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the write fails.
    pub fn write_interleaved(&mut self, pkt: &mut Packet) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned mux context whose header was written;
        //         `pkt` is a valid owned packet borrowed mutably for the call.
        let ret = unsafe { ffi_av_interleaved_write_frame(self.ptr.as_ptr(), pkt.as_mut_ptr()) };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(())
        }
    }

    /// Sets a container-level metadata entry (`av_dict_set` on the muxer's
    /// `metadata` dictionary). Call before writing the header.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if `key` / `value` contain a NUL byte or the set fails.
    pub fn set_metadata(&mut self, key: &str, value: &str) -> Result<(), AvError> {
        let key_c = CString::new(key).map_err(|_| AvError::new(crate::error_codes::EINVAL))?;
        let value_c = CString::new(value).map_err(|_| AvError::new(crate::error_codes::EINVAL))?;
        // SAFETY: `self.ptr` is a valid owned mux context; `av_dict_set` copies
        //         both strings into the context's `metadata` dictionary.
        let ret = unsafe {
            ffi_av_dict_set(
                &raw mut (*self.ptr.as_ptr()).metadata,
                key_c.as_ptr(),
                value_c.as_ptr(),
                0,
            )
        };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(())
        }
    }

    /// Adds a binary attachment stream (`AVMEDIA_TYPE_ATTACHMENT` /
    /// `AV_CODEC_ID_BIN_DATA`), storing `data` in the stream's `extradata` and
    /// recording `filename` / `mime_type` in its metadata. Returns the new
    /// stream's index. Call before writing the header.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the stream or its `extradata` cannot be
    /// allocated, or `filename` / `mime_type` contain a NUL byte.
    pub fn add_attachment_stream(
        &mut self,
        data: &[u8],
        mime_type: &str,
        filename: &str,
    ) -> Result<usize, AvError> {
        // Reject attachments too large for `extradata_size` (a `c_int`) before
        // allocating anything, so the field never wraps to a negative value.
        let extradata_size =
            c_int::try_from(data.len()).map_err(|_| AvError::new(crate::error_codes::EINVAL))?;
        let filename_c =
            CString::new(filename).map_err(|_| AvError::new(crate::error_codes::EINVAL))?;
        let mime_c =
            CString::new(mime_type).map_err(|_| AvError::new(crate::error_codes::EINVAL))?;
        // SAFETY: `self.ptr` is a valid owned mux context. A null codec lets the
        //         muxer pick a default; the returned stream and its `codecpar`
        //         are owned by the context. `extradata` is `av_mallocz`'d (owned
        //         by the codecpar, freed with it) with the required trailing
        //         padding, and `data` is copied into its leading bytes.
        unsafe {
            let stream = ffi_avformat_new_stream(self.ptr.as_ptr(), std::ptr::null());
            if stream.is_null() {
                return Err(AvError::new(crate::error_codes::ENOMEM));
            }
            let codecpar = (*stream).codecpar;
            (*codecpar).codec_type = AVMediaType_AVMEDIA_TYPE_ATTACHMENT;
            (*codecpar).codec_id = AVCodecID_AV_CODEC_ID_BIN_DATA;

            let alloc_size = data.len() + AV_INPUT_BUFFER_PADDING_SIZE as usize;
            let extradata = ffi_av_mallocz(alloc_size).cast::<u8>();
            if extradata.is_null() {
                return Err(AvError::new(crate::error_codes::ENOMEM));
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), extradata, data.len());
            (*codecpar).extradata = extradata;
            (*codecpar).extradata_size = extradata_size;

            ffi_av_dict_set(
                &raw mut (*stream).metadata,
                c"filename".as_ptr(),
                filename_c.as_ptr(),
                0,
            );
            ffi_av_dict_set(
                &raw mut (*stream).metadata,
                c"mimetype".as_ptr(),
                mime_c.as_ptr(),
                0,
            );
        }
        Ok(self.nb_streams() as usize - 1)
    }

    /// Sets the container's chapters, allocating the `AVChapter` array with
    /// FFmpeg's allocator (owned by the context, freed on drop). Chapter times
    /// are in microseconds (`AV_TIME_BASE` units). Call before writing the header.
    ///
    /// Per-chapter allocation / NUL-byte-title failures are logged and skipped;
    /// the array is compacted so it holds exactly `nb_chapters` entries.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the top-level chapter array cannot be allocated.
    pub fn set_chapters(&mut self, chapters: &[ChapterSpec<'_>]) -> Result<(), AvError> {
        if chapters.is_empty() {
            return Ok(());
        }
        // SAFETY: `self.ptr` is a valid owned mux context. The chapter pointer
        //         array and each `AVChapter` are `av_mallocz`'d (owned by the
        //         context, freed by `avformat_free_context` on drop). We write
        //         each successful chapter at the running `nb_chapters` index so
        //         the array is compact (no null gaps).
        unsafe {
            let ctx = self.ptr.as_ptr();
            let arr = ffi_av_mallocz(std::mem::size_of::<*mut AVChapter>() * chapters.len())
                .cast::<*mut AVChapter>();
            if arr.is_null() {
                return Err(AvError::new(crate::error_codes::ENOMEM));
            }
            (*ctx).chapters = arr;
            (*ctx).nb_chapters = 0;

            for spec in chapters {
                let chap = ffi_av_mallocz(std::mem::size_of::<AVChapter>()).cast::<AVChapter>();
                if chap.is_null() {
                    log::warn!(
                        "av_mallocz failed for AVChapter, skipping chapter id={}",
                        spec.id
                    );
                    continue;
                }
                (*chap).id = spec.id;
                (*chap).time_base = AVRational {
                    num: 1,
                    den: AV_TIME_BASE as c_int,
                };
                (*chap).start = spec.start_us;
                (*chap).end = spec.end_us;
                (*chap).metadata = std::ptr::null_mut::<AVDictionary>();

                if let Some(title) = spec.title {
                    if let Ok(title_c) = CString::new(title) {
                        ffi_av_dict_set(
                            &raw mut (*chap).metadata,
                            c"title".as_ptr(),
                            title_c.as_ptr(),
                            0,
                        );
                    } else {
                        log::warn!(
                            "chapter title contains a NUL byte, skipping title id={}",
                            spec.id
                        );
                    }
                }
                *arr.add((*ctx).nb_chapters as usize) = chap;
                (*ctx).nb_chapters += 1;
            }
        }
        Ok(())
    }
}

/// A chapter to write into an [`OutputFormatContext`] via
/// [`set_chapters`](OutputFormatContext::set_chapters). Times are in
/// microseconds (`AV_TIME_BASE` units).
#[derive(Clone, Copy, Debug)]
pub struct ChapterSpec<'a> {
    /// Chapter id.
    pub id: i64,
    /// Start time in microseconds.
    pub start_us: i64,
    /// End time in microseconds.
    pub end_us: i64,
    /// Optional chapter title.
    pub title: Option<&'a str>,
}

impl Drop for OutputFormatContext {
    fn drop(&mut self) {
        // SAFETY: we uniquely own the context (NonNull, not Copy/Clone), so this
        //         runs exactly once. A non-null `pb` is one of two things, and they
        //         are released differently:
        //
        //         - one `open_io` opened with `avio_open`, which must be
        //           `avio_closep`-ed. It is null for `AVFMT_NOFILE` muxers, where
        //           the caller opened none, and after `close_io`.
        //         - one `set_custom_io` attached, owned by the `io` field. Closing
        //           that with `avio_closep` would free a context `IoContext` also
        //           frees, so it is nulled here and released when `io` drops right
        //           after this body.
        //
        //         `close_output` null-checks and nulls `pb`; `avformat_free_context`
        //         does not touch `pb` either way.
        unsafe {
            let ctx = self.ptr.as_ptr();
            if self.io.is_some() {
                (*ctx).pb = std::ptr::null_mut();
            } else if !(*ctx).pb.is_null() {
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

    /// Returns the stream's real base frame rate (may be `0/0` when unknown).
    ///
    /// This is the lowest frame rate with which all timestamps can be
    /// represented accurately (the container's guessed constant frame rate),
    /// distinct from [`avg_frame_rate`](Self::avg_frame_rate).
    #[must_use]
    pub fn r_frame_rate(&self) -> AVRational {
        // SAFETY: `self.ptr` borrows a valid stream from a live format context.
        unsafe { (*self.ptr.as_ptr()).r_frame_rate }
    }

    /// Returns the number of frames in the stream (0 when unknown).
    #[must_use]
    pub fn nb_frames(&self) -> i64 {
        // SAFETY: `self.ptr` borrows a valid stream from a live format context.
        unsafe { (*self.ptr.as_ptr()).nb_frames }
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

    /// Returns the stream's duration in its own time base, or a non-positive
    /// value (`AV_NOPTS_VALUE` or 0) when unknown.
    #[must_use]
    pub fn duration(&self) -> i64 {
        // SAFETY: `self.ptr` borrows a valid stream from a live format context.
        unsafe { (*self.ptr.as_ptr()).duration }
    }

    /// Returns the stream's disposition flags (a bitmask of `AV_DISPOSITION_*`).
    #[must_use]
    pub fn disposition(&self) -> c_int {
        // SAFETY: `self.ptr` borrows a valid stream from a live format context.
        unsafe { (*self.ptr.as_ptr()).disposition }
    }

    /// Returns the stream-level metadata as a key/value map (empty when the
    /// stream carries none). Used to read tags such as `language` and `title`.
    #[must_use]
    pub fn metadata(&self) -> HashMap<String, String> {
        // SAFETY: `self.ptr` borrows a valid stream; `metadata` is a plain field
        //         (null when absent) and `read_dict` handles null.
        unsafe { read_dict((*self.ptr.as_ptr()).metadata) }
    }
}

/// A borrowed handle to an `AVCodecParameters` owned by something else.
///
/// The owner is a demuxed stream ([`StreamRef::codecpar`]) or a bitstream-filter
/// context ([`BsfContext::output_params`](crate::BsfContext::output_params)); the
/// lifetime parameter is a borrow token and deliberately names neither, so one type
/// serves both. Exposes the scalar fields ff-decode reads and hands its raw pointer
/// to the safe [`CodecContext::apply_parameters`](crate::CodecContext::apply_parameters)
/// via a crate-private accessor; the raw pointer is not part of the public API.
#[derive(Clone, Copy, Debug)]
pub struct CodecParameters<'a> {
    ptr: NonNull<AVCodecParameters>,
    _marker: PhantomData<&'a ()>,
}

impl<'a> CodecParameters<'a> {
    /// Borrows an `AVCodecParameters` block owned elsewhere in the crate.
    ///
    /// The caller ties `'a` to the owner, so the block outlives this handle.
    pub(crate) const fn from_raw(ptr: NonNull<AVCodecParameters>) -> Self {
        Self {
            ptr,
            _marker: PhantomData,
        }
    }
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

    /// Returns the raw pixel/sample format value (`AVPixelFormat` for video,
    /// `AVSampleFormat` for audio), or `-1` when unset.
    #[must_use]
    pub fn format(&self) -> c_int {
        // SAFETY: `self.ptr` borrows valid codec parameters from a live stream.
        unsafe { (*self.ptr.as_ptr()).format }
    }

    /// Returns the stream's bit rate in bits per second (0 when unknown).
    #[must_use]
    pub fn bit_rate(&self) -> i64 {
        // SAFETY: `self.ptr` borrows valid codec parameters from a live stream.
        unsafe { (*self.ptr.as_ptr()).bit_rate }
    }

    /// Returns the underlying raw pointer for FFI calls within the crate.
    pub(crate) fn as_raw(&self) -> *const AVCodecParameters {
        self.ptr.as_ptr()
    }
}

/// A borrowed handle to an `AVChapter` owned by an [`InputFormatContext`].
///
/// Mirrors [`StreamRef`] / [`CodecParameters`]: the raw chapter pointer is not
/// part of the public API; the scalar fields ff-probe reads are exposed as safe
/// accessors, and per-chapter tags are read through [`metadata`](Self::metadata).
#[derive(Clone, Copy, Debug)]
pub struct ChapterRef<'a> {
    ptr: NonNull<AVChapter>,
    _marker: PhantomData<&'a InputFormatContext>,
}

impl ChapterRef<'_> {
    /// Returns the chapter's unique id.
    #[must_use]
    pub fn id(&self) -> i64 {
        // SAFETY: `self.ptr` borrows a valid chapter from a live format context.
        unsafe { (*self.ptr.as_ptr()).id }
    }

    /// Returns the chapter's time base (used to interpret `start` / `end`).
    #[must_use]
    pub fn time_base(&self) -> AVRational {
        // SAFETY: `self.ptr` borrows a valid chapter from a live format context.
        unsafe { (*self.ptr.as_ptr()).time_base }
    }

    /// Returns the chapter's start time in its own [`time_base`](Self::time_base).
    #[must_use]
    pub fn start(&self) -> i64 {
        // SAFETY: `self.ptr` borrows a valid chapter from a live format context.
        unsafe { (*self.ptr.as_ptr()).start }
    }

    /// Returns the chapter's end time in its own [`time_base`](Self::time_base).
    #[must_use]
    pub fn end(&self) -> i64 {
        // SAFETY: `self.ptr` borrows a valid chapter from a live format context.
        unsafe { (*self.ptr.as_ptr()).end }
    }

    /// Returns the chapter-level metadata as a key/value map (empty when the
    /// chapter carries none). Used to read tags such as `title`.
    #[must_use]
    pub fn metadata(&self) -> HashMap<String, String> {
        // SAFETY: `self.ptr` borrows a valid chapter; `metadata` is a plain field
        //         (null when absent) and `read_dict` handles null.
        unsafe { read_dict((*self.ptr.as_ptr()).metadata) }
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

    /// The asset used by the custom-IO tests, or `None` when it is unavailable.
    fn custom_io_fixture() -> Option<Vec<u8>> {
        let path = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/audio/konekonoosanpo.mp3"
        ));
        std::fs::read(path).ok()
    }

    #[test]
    fn open_custom_should_mark_the_context_as_custom_io() {
        // Pins the end state rather than the line that produces it: measured,
        // `avformat_open_input` also sets this flag when it finds a `pb` already in
        // place, so removing the explicit set does not change the result here. What
        // the assertion is worth is catching a future where neither does it, since
        // the consequence -- `avformat_close_input` closing a `pb` this type also
        // frees -- is a double free that does not reliably crash.
        let Some(bytes) = custom_io_fixture() else {
            return; // fixture missing
        };
        let Ok(ctx) = InputFormatContext::open_custom(std::io::Cursor::new(bytes)) else {
            return; // demuxer absent (CI's minimal FFmpeg) -- nothing to exercise
        };
        // SAFETY: `ctx` owns a valid demux context; `flags` is a plain field.
        let flags = unsafe { (*ctx.ptr.as_ptr()).flags };
        assert_ne!(
            flags & crate::constants::AVFMT_FLAG_CUSTOM_IO,
            0,
            "a custom-IO input must carry AVFMT_FLAG_CUSTOM_IO"
        );
    }

    #[test]
    fn open_custom_should_reject_bytes_that_are_not_a_container() {
        // The error path has to release the AVIO context and the source without a
        // format context to hang them on; this drives it.
        let result = InputFormatContext::open_custom(std::io::Cursor::new(vec![0u8; 512]));
        assert!(result.is_err(), "512 zero bytes are not a media container");
    }

    #[test]
    fn set_custom_io_should_mark_the_context_as_custom_io() {
        // Mirror of the input case: the output `Drop` reads the `io` field rather
        // than the flag, but the flag is what libavformat itself keys on, so both
        // have to be set.
        let Ok(mut ctx) = OutputFormatContext::new(None, Path::new("out.mp4")) else {
            return; // mp4 muxer absent
        };
        if ctx.set_custom_io(std::io::Cursor::new(Vec::new())).is_err() {
            return;
        }
        // SAFETY: `ctx` owns a valid mux context; `flags` is a plain field.
        let flags = unsafe { (*ctx.ptr.as_ptr()).flags };
        assert_ne!(
            flags & crate::constants::AVFMT_FLAG_CUSTOM_IO,
            0,
            "a custom-IO output must carry AVFMT_FLAG_CUSTOM_IO"
        );
        assert!(ctx.io.is_some(), "the sink must be owned by the context");
    }

    #[test]
    fn input_open_valid_file_should_allocate_and_drop_cleanly() {
        // ADR-0003 Confirmation (drop-once): a successfully opened container is an
        // owned `InputFormatContext` that frees exactly once on drop via
        // `avformat_close_input` — no manual close, no double free. Skip gracefully
        // when the fixture or its demuxer is unavailable (e.g. CI's minimal FFmpeg
        // build) so the test never flakes (RK-002).
        let path = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/audio/konekonoosanpo.mp3"
        ));
        let Ok(mut ctx) = InputFormatContext::open(&path) else {
            return; // fixture missing or mp3 demuxer absent — nothing to exercise
        };
        // Populate stream info (best effort), then skip if the demuxer reported no
        // streams, so a hyper-minimal FFmpeg build never turns this into a failure.
        let _ = ctx.find_stream_info();
        if ctx.nb_streams() == 0 {
            return;
        }
        // A live context was built; it drops at end of scope, freeing the context
        // exactly once (avformat_close_input) with no panic / double free.
        assert!(
            ctx.nb_streams() >= 1,
            "an opened container should expose at least one stream"
        );
    }

    #[test]
    fn read_dict_should_collect_all_entries() {
        // Build a small dictionary with av_dict_set, read it back through the
        // shared helper, then free it. Deterministic and file-independent.
        let mut dict: *mut AVDictionary = std::ptr::null_mut();
        // SAFETY: `dict` starts null; `av_dict_set` allocates/extends it. The keys
        //         and values are valid NUL-terminated C strings.
        unsafe {
            ffi_av_dict_set(&mut dict, c"title".as_ptr(), c"Example".as_ptr(), 0);
            ffi_av_dict_set(&mut dict, c"language".as_ptr(), c"eng".as_ptr(), 0);
        }
        // SAFETY: `dict` is a valid dictionary built just above.
        let map = unsafe { read_dict(dict) };
        // SAFETY: frees the dictionary allocated above and nulls our pointer.
        unsafe { crate::av_dict_free(&mut dict) };

        assert_eq!(map.get("title").map(String::as_str), Some("Example"));
        assert_eq!(map.get("language").map(String::as_str), Some("eng"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn read_dict_should_return_empty_for_null() {
        // A null dictionary (a container / stream / chapter with no tags) yields
        // an empty map rather than dereferencing null.
        // SAFETY: a null dictionary pointer is explicitly allowed by `read_dict`.
        let map = unsafe { read_dict(std::ptr::null()) };
        assert!(map.is_empty());
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

    #[test]
    fn output_stream_api_should_round_trip() {
        // Exercises new_stream / stream_time_base / set_stream_time_base / set_opt.
        // Needs a real muxer; skip gracefully if mp4 is absent (minimal FFmpeg build).
        let Ok(mut ctx) = OutputFormatContext::new(None, Path::new("out.mp4")) else {
            return;
        };
        let idx = ctx
            .new_stream(None)
            .expect("new_stream should allocate a stream");
        assert_eq!(idx, 0);
        assert_eq!(ctx.nb_streams(), 1);

        ctx.set_stream_time_base(idx, AVRational { num: 1, den: 30 });
        let tb = ctx.stream_time_base(idx);
        assert_eq!((tb.num, tb.den), (1, 30));

        // An out-of-range index is a safe no-op / zero, not a panic.
        let oob = ctx.stream_time_base(99);
        assert_eq!((oob.num, oob.den), (0, 0));

        // An unrecognised muxer option surfaces as an error (no panic).
        let key = std::ffi::CString::new("definitely_not_a_real_option").unwrap();
        let value = std::ffi::CString::new("1").unwrap();
        assert!(ctx.set_opt(&key, &value).is_err());

        // The codec-parameter copy's out-of-range index guard returns Err, not a panic.
        let cc = CodecContext::new(None).expect("codec context alloc should succeed");
        assert!(ctx.apply_stream_params_from_context(99, &cc).is_err());
    }

    #[test]
    fn metadata_attachment_and_chapters_should_apply() {
        // Exercises set_metadata / add_attachment_stream / set_chapters. These set
        // context/stream fields (no header write), so any file muxer works; skip
        // gracefully if mp4 is absent from a minimal FFmpeg build.
        let Ok(mut ctx) = OutputFormatContext::new(None, Path::new("out.mp4")) else {
            return;
        };

        ctx.set_metadata("title", "test")
            .expect("set_metadata should succeed");
        // A NUL byte in the key is rejected without panicking.
        assert!(ctx.set_metadata("k\0", "v").is_err());

        let idx = ctx
            .add_attachment_stream(b"font-bytes", "application/x-font", "font.ttf")
            .expect("add_attachment_stream should allocate a stream");
        assert_eq!(ctx.nb_streams(), idx as u32 + 1);
        // A NUL byte in the filename is rejected without panicking.
        assert!(ctx.add_attachment_stream(b"x", "m", "f\0").is_err());

        // Empty chapters is a no-op Ok; a non-empty set allocates and compacts.
        ctx.set_chapters(&[]).expect("empty chapters is Ok");
        ctx.set_chapters(&[
            ChapterSpec {
                id: 0,
                start_us: 0,
                end_us: 1_000_000,
                title: Some("Intro"),
            },
            ChapterSpec {
                id: 1,
                start_us: 1_000_000,
                end_us: 2_000_000,
                title: None,
            },
        ])
        .expect("set_chapters should allocate");
    }
}
