//! RAII owner for an `AVCodecContext`.
//!
//! [`CodecContext`] allocates a codec context and frees it exactly once on drop,
//! replacing the manual `avcodec::alloc_context3` + `avcodec::free_context` pair.
//! Its fallible methods return [`AvError`]. Packet / frame arguments are the owned
//! [`Packet`] / [`Frame`] types, so `send_packet` / `receive_frame` are safe;
//! `open` (raw options dictionary), `parameters_to_context` (raw parameters), and
//! `send_eof` / `flush_buffers` (opened-context precondition) remain `unsafe`.

use std::os::raw::c_int;
use std::ptr::NonNull;

use crate::{
    AVCodecContext, AVCodecParameters, AVDictionary, AVFrame, AVPacket, AvError, Codec,
    CodecParameters, Frame, Packet, avcodec_free_context as ffi_avcodec_free_context,
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
            .map(|ptr| Self { ptr })
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
        //         runs exactly once. `avcodec_free_context` frees it and writes
        //         null into our local copy of the pointer, which is then discarded.
        //         The raw binding is used directly so this type does not depend on
        //         the `avcodec::free_context` wrapper (retired in #1490).
        unsafe {
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
