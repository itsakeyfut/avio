//! RAII owner for an `AVCodecContext`.
//!
//! [`CodecContext`] allocates a codec context and frees it exactly once on drop,
//! replacing the manual `avcodec::alloc_context3` + `avcodec::free_context` pair.
//! Its fallible methods return [`AvError`]. Packet / frame arguments are the owned
//! [`Packet`] / [`Frame`] types, so `send_packet` / `receive_frame` are safe;
//! `open` (raw options dictionary), `parameters_to_context` (raw parameters), and
//! `send_eof` / `flush_buffers` (opened-context precondition) remain `unsafe`.

use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::ptr::{self, NonNull};

use crate::{
    AVCodecContext, AVCodecID, AVCodecParameters, AVColorPrimaries, AVColorRange, AVColorSpace,
    AVColorTransferCharacteristic, AVDictionary, AVFrame, AVPacket, AVPixelFormat, AVRational,
    AVSampleFormat, AvError, Codec, CodecParameters, Frame, Packet,
    avcodec_free_context as ffi_avcodec_free_context,
};

/// The outcome of a [`CodecContext::receive_frame`] call.
///
/// Encodes FFmpeg's `EAGAIN` / `EOF` drain states as named variants so callers
/// never branch on raw return codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveOutcome {
    /// An output unit is available: a decoded frame from
    /// [`receive_frame`](CodecContext::receive_frame), or an encoded packet from
    /// [`receive_packet`](CodecContext::receive_packet).
    Frame,
    /// The decoder needs more input (`EAGAIN`): send another packet, or
    /// [`send_eof`](CodecContext::send_eof) to begin draining.
    NeedInput,
    /// The decoder is fully drained (`EOF`): no more frames will be produced.
    Drained,
}

/// Maps a raw `avcodec::receive_frame` result to a [`ReceiveOutcome`].
///
/// `EAGAIN` and `EOF` are expected drain states, not errors; any other negative
/// code is a real error.
fn classify_receive(result: Result<(), c_int>) -> Result<ReceiveOutcome, AvError> {
    match result {
        Ok(()) => Ok(ReceiveOutcome::Frame),
        Err(code) if code == crate::error_codes::EAGAIN => Ok(ReceiveOutcome::NeedInput),
        Err(code) if code == crate::error_codes::EOF => Ok(ReceiveOutcome::Drained),
        Err(code) => Err(AvError::new(code)),
    }
}

/// An owned `AVCodecContext`.
///
/// The context is freed exactly once on drop. This is guaranteed by
/// construction: the value owns a [`NonNull`] and is neither `Copy` nor `Clone`,
/// so it drops exactly once and cannot be duplicated.
#[derive(Debug)]
pub struct CodecContext {
    ptr: NonNull<AVCodecContext>,
    /// Owned two-pass `stats_in` buffer, when one is set.
    ///
    /// FFmpeg's `stats_in` field must point to a NUL-terminated string that
    /// outlives the codec context but that FFmpeg must not free. Storing the
    /// [`CString`] here keeps it alive; [`Drop`] nulls the field before
    /// `avcodec_free_context` so FFmpeg never `av_free`s this Rust-owned buffer.
    stats_in: Option<CString>,
}

impl CodecContext {
    /// Allocates a codec context for `codec`, or a generic context when `None`.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if allocation fails.
    pub fn new(codec: Option<Codec>) -> Result<Self, AvError> {
        let codec_ptr = codec.map_or(std::ptr::null(), |c| c.as_ptr());
        // SAFETY: `codec_ptr` is null (yielding a generic context) or a valid static
        //         codec pointer from `Codec`; `alloc_context3` returns a non-null
        //         context or a negative error code.
        let ptr = unsafe { crate::avcodec::alloc_context3(codec_ptr) }.map_err(AvError::new)?;
        // `alloc_context3` returns `Ok` only with a non-null pointer.
        NonNull::new(ptr)
            .ok_or_else(|| AvError::new(crate::error_codes::ENOMEM))
            .map(|ptr| Self {
                ptr,
                stats_in: None,
            })
    }

    /// Returns the context pointer for read-only use.
    #[must_use]
    pub const fn as_ptr(&self) -> *const AVCodecContext {
        self.ptr.as_ptr()
    }

    /// Returns the context pointer for mutation and FFI calls.
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut AVCodecContext {
        self.ptr.as_ptr()
    }

    /// Sets the number of decoding/encoding threads.
    pub fn set_thread_count(&mut self, thread_count: c_int) {
        // SAFETY: `self.ptr` is a valid owned context; `thread_count` is a plain field.
        unsafe { (*self.ptr.as_ptr()).thread_count = thread_count };
    }

    /// Sets the coded picture width.
    pub fn set_width(&mut self, width: c_int) {
        // SAFETY: `self.ptr` is a valid owned context; `width` is a plain field.
        unsafe { (*self.ptr.as_ptr()).width = width };
    }

    /// Sets the coded picture height.
    pub fn set_height(&mut self, height: c_int) {
        // SAFETY: `self.ptr` is a valid owned context; `height` is a plain field.
        unsafe { (*self.ptr.as_ptr()).height = height };
    }

    /// Sets the pixel format.
    pub fn set_pix_fmt(&mut self, pix_fmt: AVPixelFormat) {
        // SAFETY: `self.ptr` is a valid owned context; `pix_fmt` is a plain field.
        unsafe { (*self.ptr.as_ptr()).pix_fmt = pix_fmt };
    }

    /// Sets the time base.
    pub fn set_time_base(&mut self, time_base: AVRational) {
        // SAFETY: `self.ptr` is a valid owned context; `time_base` is a plain field.
        unsafe { (*self.ptr.as_ptr()).time_base = time_base };
    }

    /// Returns the pixel format.
    #[must_use]
    pub fn pix_fmt(&self) -> AVPixelFormat {
        // SAFETY: `self.ptr` is a valid owned context; `pix_fmt` is a plain field.
        unsafe { (*self.ptr.as_ptr()).pix_fmt }
    }

    /// Returns the sample format.
    #[must_use]
    pub fn sample_fmt(&self) -> AVSampleFormat {
        // SAFETY: `self.ptr` is a valid owned context; `sample_fmt` is a plain field.
        unsafe { (*self.ptr.as_ptr()).sample_fmt }
    }

    /// Returns the coded picture width.
    #[must_use]
    pub fn width(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned context; `width` is a plain field.
        unsafe { (*self.ptr.as_ptr()).width }
    }

    /// Returns the coded picture height.
    #[must_use]
    pub fn height(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned context; `height` is a plain field.
        unsafe { (*self.ptr.as_ptr()).height }
    }

    /// Sets the codec id.
    pub fn set_codec_id(&mut self, codec_id: AVCodecID) {
        // SAFETY: `self.ptr` is a valid owned context; `codec_id` is a plain field.
        unsafe { (*self.ptr.as_ptr()).codec_id = codec_id };
    }

    /// Sets the frame rate.
    pub fn set_framerate(&mut self, framerate: AVRational) {
        // SAFETY: `self.ptr` is a valid owned context; `framerate` is a plain field.
        unsafe { (*self.ptr.as_ptr()).framerate = framerate };
    }

    /// Sets the target bit rate in bits per second.
    pub fn set_bit_rate(&mut self, bit_rate: i64) {
        // SAFETY: `self.ptr` is a valid owned context; `bit_rate` is a plain field.
        unsafe { (*self.ptr.as_ptr()).bit_rate = bit_rate };
    }

    /// Sets the rate-control maximum bit rate.
    pub fn set_rc_max_rate(&mut self, rc_max_rate: i64) {
        // SAFETY: `self.ptr` is a valid owned context; `rc_max_rate` is a plain field.
        unsafe { (*self.ptr.as_ptr()).rc_max_rate = rc_max_rate };
    }

    /// Sets the rate-control buffer size.
    pub fn set_rc_buffer_size(&mut self, rc_buffer_size: c_int) {
        // SAFETY: `self.ptr` is a valid owned context; `rc_buffer_size` is a plain field.
        unsafe { (*self.ptr.as_ptr()).rc_buffer_size = rc_buffer_size };
    }

    /// Sets the color primaries.
    pub fn set_color_primaries(&mut self, color_primaries: AVColorPrimaries) {
        // SAFETY: `self.ptr` is a valid owned context; `color_primaries` is a plain field.
        unsafe { (*self.ptr.as_ptr()).color_primaries = color_primaries };
    }

    /// Sets the color transfer characteristic.
    pub fn set_color_trc(&mut self, color_trc: AVColorTransferCharacteristic) {
        // SAFETY: `self.ptr` is a valid owned context; `color_trc` is a plain field.
        unsafe { (*self.ptr.as_ptr()).color_trc = color_trc };
    }

    /// Sets the colorspace.
    pub fn set_colorspace(&mut self, colorspace: AVColorSpace) {
        // SAFETY: `self.ptr` is a valid owned context; `colorspace` is a plain field.
        unsafe { (*self.ptr.as_ptr()).colorspace = colorspace };
    }

    /// Sets the color range.
    pub fn set_color_range(&mut self, color_range: AVColorRange) {
        // SAFETY: `self.ptr` is a valid owned context; `color_range` is a plain field.
        unsafe { (*self.ptr.as_ptr()).color_range = color_range };
    }

    /// Sets the audio sample rate in Hz.
    pub fn set_sample_rate(&mut self, sample_rate: c_int) {
        // SAFETY: `self.ptr` is a valid owned context; `sample_rate` is a plain field.
        unsafe { (*self.ptr.as_ptr()).sample_rate = sample_rate };
    }

    /// Sets the audio sample format.
    pub fn set_sample_fmt(&mut self, sample_fmt: AVSampleFormat) {
        // SAFETY: `self.ptr` is a valid owned context; `sample_fmt` is a plain field.
        unsafe { (*self.ptr.as_ptr()).sample_fmt = sample_fmt };
    }

    /// Sets the maximum number of B-frames.
    pub fn set_max_b_frames(&mut self, max_b_frames: c_int) {
        // SAFETY: `self.ptr` is a valid owned context; `max_b_frames` is a plain field.
        unsafe { (*self.ptr.as_ptr()).max_b_frames = max_b_frames };
    }

    /// Sets the group-of-pictures size.
    pub fn set_gop_size(&mut self, gop_size: c_int) {
        // SAFETY: `self.ptr` is a valid owned context; `gop_size` is a plain field.
        unsafe { (*self.ptr.as_ptr()).gop_size = gop_size };
    }

    /// Sets the number of reference frames.
    pub fn set_refs(&mut self, refs: c_int) {
        // SAFETY: `self.ptr` is a valid owned context; `refs` is a plain field.
        unsafe { (*self.ptr.as_ptr()).refs = refs };
    }

    /// Sets the minimum quantizer.
    pub fn set_qmin(&mut self, qmin: c_int) {
        // SAFETY: `self.ptr` is a valid owned context; `qmin` is a plain field.
        unsafe { (*self.ptr.as_ptr()).qmin = qmin };
    }

    /// Sets the maximum quantizer.
    pub fn set_qmax(&mut self, qmax: c_int) {
        // SAFETY: `self.ptr` is a valid owned context; `qmax` is a plain field.
        unsafe { (*self.ptr.as_ptr()).qmax = qmax };
    }

    /// Initialises `ch_layout` to the default native layout for `nb_channels`.
    pub fn set_ch_layout_default(&mut self, nb_channels: c_int) {
        // SAFETY: `self.ptr` is a valid owned context; `av_channel_layout_default`
        //         writes the default native layout into the `ch_layout` field.
        unsafe {
            crate::av_channel_layout_default(&raw mut (*self.ptr.as_ptr()).ch_layout, nb_channels);
        }
    }

    /// Sets the codec flags bitmask.
    pub fn set_flags(&mut self, flags: c_int) {
        // SAFETY: `self.ptr` is a valid owned context; `flags` is a plain field.
        unsafe { (*self.ptr.as_ptr()).flags = flags };
    }

    /// Returns the codec flags bitmask.
    #[must_use]
    pub fn flags(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned context; `flags` is a plain field.
        unsafe { (*self.ptr.as_ptr()).flags }
    }

    /// Returns the time base.
    #[must_use]
    pub fn time_base(&self) -> AVRational {
        // SAFETY: `self.ptr` is a valid owned context; `time_base` is a plain field.
        unsafe { (*self.ptr.as_ptr()).time_base }
    }

    /// Returns the codec id.
    #[must_use]
    pub fn codec_id(&self) -> AVCodecID {
        // SAFETY: `self.ptr` is a valid owned context; `codec_id` is a plain field.
        unsafe { (*self.ptr.as_ptr()).codec_id }
    }

    /// Returns the encoder frame size (samples per audio frame).
    #[must_use]
    pub fn frame_size(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned context; `frame_size` is a plain field.
        unsafe { (*self.ptr.as_ptr()).frame_size }
    }

    /// Returns the audio sample rate in Hz.
    #[must_use]
    pub fn sample_rate(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned context; `sample_rate` is a plain field.
        unsafe { (*self.ptr.as_ptr()).sample_rate }
    }

    /// Returns the number of audio channels.
    #[must_use]
    pub fn channels(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned context; `ch_layout.nb_channels` is a plain field.
        unsafe { (*self.ptr.as_ptr()).ch_layout.nb_channels }
    }

    /// Sets a private codec option to a string value.
    ///
    /// Targets the context's `priv_data`, matching a direct
    /// `av_opt_set(ctx->priv_data, key, value, 0)`. The caller decides how to
    /// react to a failure; this never logs or silently skips.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if `key` or `value` contains an interior NUL, or if
    /// FFmpeg rejects the option.
    pub fn set_opt(&mut self, key: &str, value: &str) -> Result<(), AvError> {
        let key_c = CString::new(key).map_err(|_| AvError::new(crate::error_codes::EINVAL))?;
        let val_c = CString::new(value).map_err(|_| AvError::new(crate::error_codes::EINVAL))?;
        // SAFETY: `self.ptr` is a valid owned context whose `priv_data` is set by
        //         `avcodec_alloc_context3`; `key_c`/`val_c` are valid NUL-terminated
        //         C strings kept alive across the call, which copies both.
        let ret = unsafe {
            crate::av_opt_set(
                (*self.ptr.as_ptr()).priv_data,
                key_c.as_ptr(),
                val_c.as_ptr(),
                0,
            )
        };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(())
        }
    }

    /// Sets an option on the context itself, searching child objects.
    ///
    /// Targets the context (cast to the AVOptions object) with
    /// `AV_OPT_SEARCH_CHILDREN`, matching a direct
    /// `av_opt_set(ctx, key, value, AV_OPT_SEARCH_CHILDREN)`.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if `key` or `value` contains an interior NUL, or if
    /// FFmpeg rejects the option.
    /// Sets a codec-private option whose value is arbitrary bytes (not
    /// necessarily UTF-8), preserving the raw bytes exactly (e.g. a 4-byte
    /// FourCC tag). Targets `priv_data`.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the key has an interior NUL or FFmpeg rejects
    /// the option.
    pub fn set_opt_cstr(&mut self, key: &str, value: &CStr) -> Result<(), AvError> {
        let key_c = CString::new(key).map_err(|_| AvError::new(crate::error_codes::EINVAL))?;
        // SAFETY: `self.ptr` is a valid owned context whose `priv_data` is set by
        //         `avcodec_alloc_context3`; `key_c` and `value` are valid
        //         NUL-terminated C strings kept alive across the call, which copies
        //         both.
        let ret = unsafe {
            crate::av_opt_set(
                (*self.ptr.as_ptr()).priv_data,
                key_c.as_ptr(),
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

    /// Sets an option on the context (searching child objects), for options that
    /// live on the encoder context itself rather than `priv_data`.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the key/value has an interior NUL or FFmpeg
    /// rejects the option.
    pub fn set_opt_search_children(&mut self, key: &str, value: &str) -> Result<(), AvError> {
        let key_c = CString::new(key).map_err(|_| AvError::new(crate::error_codes::EINVAL))?;
        let val_c = CString::new(value).map_err(|_| AvError::new(crate::error_codes::EINVAL))?;
        // SAFETY: `self.ptr` is a valid owned context usable as an AVOptions object;
        //         `key_c`/`val_c` are valid NUL-terminated C strings kept alive
        //         across the call, which copies both.
        let ret = unsafe {
            crate::av_opt_set(
                self.ptr.as_ptr().cast(),
                key_c.as_ptr(),
                val_c.as_ptr(),
                crate::AV_OPT_SEARCH_CHILDREN as c_int,
            )
        };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(())
        }
    }

    /// Stores a copy of the two-pass statistics and points `stats_in` at it.
    ///
    /// The copy is owned by this context; [`Drop`] nulls the raw `stats_in` field
    /// before `avcodec_free_context`, so FFmpeg never frees the Rust-owned buffer.
    pub fn set_stats_in(&mut self, stats: &CStr) {
        // Store first, then take the pointer of the stored copy so the pointer we
        // hand FFmpeg refers to a buffer this context keeps alive.
        self.stats_in = Some(stats.to_owned());
        if let Some(owned) = self.stats_in.as_ref() {
            let stats_ptr = owned.as_ptr().cast_mut();
            // SAFETY: `self.ptr` is a valid owned context; `stats_ptr` points into a
            //         CString owned by `self.stats_in`, kept alive until Drop nulls
            //         this field.
            unsafe { (*self.ptr.as_ptr()).stats_in = stats_ptr };
        }
    }

    /// Clears `stats_in` and drops the owned statistics buffer.
    pub fn clear_stats_in(&mut self) {
        // SAFETY: `self.ptr` is a valid owned context; nulling `stats_in` is sound.
        unsafe { (*self.ptr.as_ptr()).stats_in = ptr::null_mut() };
        self.stats_in = None;
    }

    /// Returns a copy of the encoder's two-pass statistics output, if any.
    #[must_use]
    pub fn stats_out(&self) -> Option<CString> {
        // SAFETY: `self.ptr` is a valid owned context; `stats_out` is null or a
        //         valid NUL-terminated C string owned by the context.
        unsafe {
            let p = (*self.ptr.as_ptr()).stats_out;
            if p.is_null() {
                None
            } else {
                Some(CStr::from_ptr(p).to_owned())
            }
        }
    }

    /// Copies the parameters of `params` into the context (safe wrapper).
    ///
    /// This is the borrowed-handle counterpart of the raw
    /// [`parameters_to_context`](Self::parameters_to_context); it takes a
    /// [`CodecParameters`] borrowed from an [`InputFormatContext`](crate::InputFormatContext)
    /// stream, so no raw pointer is exposed.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the parameters cannot be copied.
    pub fn apply_parameters(&mut self, params: &CodecParameters<'_>) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned context; `params` wraps a valid
        //         `AVCodecParameters` borrowed from a live format context.
        unsafe { crate::avcodec::parameters_to_context(self.ptr.as_ptr(), params.as_raw()) }
            .map_err(AvError::new)
    }

    /// Copies stream parameters into the context.
    ///
    /// # Safety
    ///
    /// `par` must be a valid `*const AVCodecParameters`.
    pub unsafe fn parameters_to_context(
        &mut self,
        par: *const AVCodecParameters,
    ) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned context; the caller upholds `par`.
        unsafe { crate::avcodec::parameters_to_context(self.ptr.as_ptr(), par) }
            .map_err(AvError::new)
    }

    /// Opens the context with `codec` using default options (safe wrapper).
    ///
    /// This is the safe counterpart of the raw [`open`](Self::open); it always
    /// passes a null options dictionary, which every ff-decode call site uses.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the codec cannot be opened.
    pub fn open_codec(&mut self, codec: Codec) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned context and `codec` is a valid static
        //         codec; a null options pointer is accepted by `avcodec_open2`.
        unsafe { crate::avcodec::open2(self.ptr.as_ptr(), codec.as_ptr(), std::ptr::null_mut()) }
            .map_err(AvError::new)
    }

    /// Opens the context with `codec` and optional dictionary `options`.
    ///
    /// # Safety
    ///
    /// `options` must be null or a valid `*mut *mut AVDictionary`.
    pub unsafe fn open(
        &mut self,
        codec: Codec,
        options: *mut *mut AVDictionary,
    ) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned context and `codec` is a valid static
        //         codec; the caller upholds `options`.
        unsafe { crate::avcodec::open2(self.ptr.as_ptr(), codec.as_ptr(), options) }
            .map_err(AvError::new)
    }

    /// Sends a packet to the decoder.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the decoder cannot accept the packet.
    pub fn send_packet(&mut self, pkt: &Packet) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid open context; `pkt` is a valid owned packet.
        unsafe { crate::avcodec::send_packet(self.ptr.as_ptr(), pkt.as_ptr()) }
            .map_err(AvError::new)
    }

    /// Signals end-of-stream by sending a null packet, so the decoder enters
    /// draining and all buffered frames can be pulled.
    ///
    /// After this, loop [`receive_frame`](Self::receive_frame) until it returns
    /// [`ReceiveOutcome::Drained`]. This is the one supported way to drain, so a
    /// caller cannot forget the flush.
    ///
    /// # Safety
    ///
    /// The context must have been opened via [`open`](Self::open) first.
    pub unsafe fn send_eof(&mut self) -> Result<(), AvError> {
        // SAFETY: the caller guarantees the context is opened; a null packet is
        //         the documented end-of-stream signal for `avcodec_send_packet`.
        unsafe { crate::avcodec::send_packet(self.ptr.as_ptr(), std::ptr::null()) }
            .map_err(AvError::new)
    }

    /// Receives a decoded frame, returning a typed [`ReceiveOutcome`].
    ///
    /// `EAGAIN` (need input) and `EOF` (drained) are returned as
    /// [`ReceiveOutcome::NeedInput`] / [`ReceiveOutcome::Drained`] rather than
    /// errors; only other negative codes are `Err`.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] on a real decode error (`EAGAIN` / `EOF` are typed
    /// outcomes, not errors).
    pub fn receive_frame(&mut self, frame: &mut Frame) -> Result<ReceiveOutcome, AvError> {
        // SAFETY: `self.ptr` is a valid open context; `frame` is a valid owned frame.
        let result =
            unsafe { crate::avcodec::receive_frame(self.ptr.as_ptr(), frame.as_mut_ptr()) };
        classify_receive(result)
    }

    /// Resets the codec's internal buffers (for example after a seek).
    ///
    /// # Safety
    ///
    /// The context must have been opened via [`open`](Self::open) first:
    /// `avcodec_flush_buffers` reads codec-internal state that `avcodec_open2`
    /// allocates, so calling it on an unopened context is undefined behaviour.
    pub unsafe fn flush_buffers(&mut self) {
        // SAFETY: the caller guarantees the context is opened; `flush_buffers`
        //         reads no caller-supplied pointer.
        unsafe { crate::avcodec::flush_buffers(self.ptr.as_ptr()) };
    }

    /// Sends a frame to the encoder (a null frame flushes it, entering draining).
    ///
    /// After sending a null frame, loop [`receive_packet`](Self::receive_packet)
    /// until it returns [`ReceiveOutcome::Drained`] to collect the buffered packets.
    ///
    /// # Safety
    ///
    /// `frame` must be null or a valid `*const AVFrame`.
    pub unsafe fn send_frame(&mut self, frame: *const AVFrame) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid open encoder context; the caller upholds `frame`.
        unsafe { crate::avcodec::send_frame(self.ptr.as_ptr(), frame) }.map_err(AvError::new)
    }

    /// Receives an encoded packet, returning a typed [`ReceiveOutcome`].
    ///
    /// `EAGAIN` (need input) and `EOF` (drained) are returned as
    /// [`ReceiveOutcome::NeedInput`] / [`ReceiveOutcome::Drained`] rather than
    /// errors; only other negative codes are `Err`.
    ///
    /// # Safety
    ///
    /// `pkt` must be a valid `*mut AVPacket`.
    pub unsafe fn receive_packet(&mut self, pkt: *mut AVPacket) -> Result<ReceiveOutcome, AvError> {
        // SAFETY: `self.ptr` is a valid open encoder context; the caller upholds `pkt`.
        let result = unsafe { crate::avcodec::receive_packet(self.ptr.as_ptr(), pkt) };
        classify_receive(result)
    }

    /// Copies this context's parameters into `par` (encoder → stream codecpar).
    ///
    /// # Safety
    ///
    /// `par` must be a valid `*mut AVCodecParameters`.
    pub unsafe fn parameters_from_context(
        &self,
        par: *mut AVCodecParameters,
    ) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid context; the caller upholds `par`.
        unsafe { crate::avcodec::parameters_from_context(par, self.ptr.as_ptr()) }
            .map_err(AvError::new)
    }
}

impl Drop for CodecContext {
    fn drop(&mut self) {
        // SAFETY: we uniquely own the context (NonNull, not Copy/Clone), so this
        //         runs exactly once. When we own the `stats_in` buffer we null the
        //         field first so `avcodec_free_context` does not `av_free` the
        //         Rust-owned CString (a double-free); `self.stats_in` then drops
        //         normally, freeing the Rust buffer. `avcodec_free_context` frees
        //         the context and writes null into our local copy of the pointer,
        //         which is then discarded. The raw binding is used directly so this
        //         type does not depend on the `avcodec::free_context` wrapper
        //         (retired in #1490).
        unsafe {
            if self.stats_in.is_some() {
                (*self.ptr.as_ptr()).stats_in = ptr::null_mut();
            }
            let mut raw = self.ptr.as_ptr();
            ffi_avcodec_free_context(&mut raw);
        }
    }
}

// SAFETY: an `AVCodecContext` is not safe for concurrent access, but moving
//         ownership between threads is sound because Rust's ownership model
//         guarantees exclusive access.
unsafe impl Send for CodecContext {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_context_new_should_allocate_and_drop_cleanly() {
        // A `None` codec yields a generic context, so this does not depend on any
        // specific decoder being present in the linked FFmpeg build.
        let ctx = CodecContext::new(None).expect("alloc should succeed");
        assert!(!ctx.as_ptr().is_null());
        // Dropping `ctx` here frees the context exactly once (no panic / double free).
    }

    #[test]
    fn set_then_get_should_round_trip_dimensions_and_formats() {
        // A `None` codec yields a generic context whose scalar fields can be set
        // and read back without opening any codec.
        let mut ctx = CodecContext::new(None).expect("alloc should succeed");
        ctx.set_width(1920);
        ctx.set_height(1080);
        ctx.set_pix_fmt(crate::AVPixelFormat_AV_PIX_FMT_YUV420P);
        ctx.set_time_base(AVRational { num: 1, den: 30 });

        assert_eq!(ctx.width(), 1920);
        assert_eq!(ctx.height(), 1080);
        assert_eq!(ctx.pix_fmt(), crate::AVPixelFormat_AV_PIX_FMT_YUV420P);
    }

    #[test]
    fn set_then_get_should_round_trip_scalar_fields() {
        let mut ctx = CodecContext::new(None).expect("alloc should succeed");
        ctx.set_codec_id(crate::AVCodecID_AV_CODEC_ID_H264);
        ctx.set_bit_rate(2_000_000);
        ctx.set_sample_rate(48_000);
        ctx.set_gop_size(12);
        ctx.set_max_b_frames(2);
        ctx.set_refs(3);
        ctx.set_qmin(10);
        ctx.set_qmax(40);
        ctx.set_framerate(AVRational { num: 30, den: 1 });
        ctx.set_flags(0);
        ctx.set_flags(ctx.flags() | 0x0400);

        assert_eq!(ctx.codec_id(), crate::AVCodecID_AV_CODEC_ID_H264);
        assert_eq!(ctx.sample_rate(), 48_000);
        assert_eq!(ctx.flags() & 0x0400, 0x0400);
    }

    #[test]
    fn set_ch_layout_default_should_set_channel_count() {
        let mut ctx = CodecContext::new(None).expect("alloc should succeed");
        ctx.set_ch_layout_default(2);
        assert_eq!(ctx.channels(), 2);
    }

    #[test]
    fn set_opt_unknown_key_should_return_err() {
        // A generic context has null `priv_data`; `av_opt_set` reports the unknown
        // option as an error rather than applying it. This holds for any FFmpeg
        // build, so the assertion is build-independent.
        let mut ctx = CodecContext::new(None).expect("alloc should succeed");
        assert!(ctx.set_opt("no_such_option_xyz", "0").is_err());
    }

    #[test]
    fn set_opt_search_children_unknown_key_should_return_err() {
        // The search-children path targets the context object; an unknown option
        // is rejected on any FFmpeg build, so the assertion is build-independent.
        let mut ctx = CodecContext::new(None).expect("alloc should succeed");
        assert!(
            ctx.set_opt_search_children("no_such_option_xyz", "0")
                .is_err()
        );
    }

    #[test]
    fn stats_in_round_trip_should_drop_once() {
        // Setting `stats_in` stores a Rust-owned CString and points the raw field
        // at it. Drop must null the field before `avcodec_free_context` so neither
        // FFmpeg nor Rust double-frees the buffer. Constructing, setting, and
        // dropping cleanly (no panic / double free) exercises that invariant.
        let stats = CString::new("frame stats data").expect("no interior NUL");
        let mut ctx = CodecContext::new(None).expect("alloc should succeed");
        ctx.set_stats_in(&stats);
        // stats_out is null on a fresh generic context.
        assert!(ctx.stats_out().is_none());
        ctx.clear_stats_in();
        ctx.set_stats_in(&stats);
        // Dropping `ctx` here must not double-free the owned stats buffer.
    }

    #[test]
    fn sample_fmt_should_read_back_default() {
        // A fresh generic context reports its (default) sample format without a
        // codec being opened; reading it must not panic.
        let ctx = CodecContext::new(None).expect("alloc should succeed");
        let _ = ctx.sample_fmt();
    }

    #[test]
    fn receive_outcome_should_classify_ok_as_frame() {
        assert_eq!(classify_receive(Ok(())), Ok(ReceiveOutcome::Frame));
    }

    #[test]
    fn receive_outcome_should_classify_eagain_as_need_input() {
        assert_eq!(
            classify_receive(Err(crate::error_codes::EAGAIN)),
            Ok(ReceiveOutcome::NeedInput)
        );
    }

    #[test]
    fn receive_outcome_should_classify_eof_as_drained() {
        assert_eq!(
            classify_receive(Err(crate::error_codes::EOF)),
            Ok(ReceiveOutcome::Drained)
        );
    }

    #[test]
    fn receive_outcome_should_classify_other_code_as_error() {
        assert_eq!(classify_receive(Err(-22)), Err(AvError::new(-22)));
    }
}
