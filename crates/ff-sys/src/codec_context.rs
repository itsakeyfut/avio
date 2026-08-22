//! RAII owner for an `AVCodecContext`.
//!
//! [`CodecContext`] allocates a codec context and frees it exactly once on drop,
//! replacing the manual `avcodec::alloc_context3` + `avcodec::free_context` pair.
//! Its fallible methods return [`AvError`]. Packet / frame arguments stay raw
//! pointers for now (owned Frame / Packet are a later step), so the
//! pointer-taking methods are `unsafe`: the caller upholds the usual FFmpeg
//! preconditions.

use std::ptr::NonNull;

use crate::{
    AVCodec, AVCodecContext, AVCodecParameters, AVDictionary, AVFrame, AVPacket, AvError,
    avcodec_free_context as ffi_avcodec_free_context,
};

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
    /// Allocates a codec context for `codec`.
    ///
    /// # Safety
    ///
    /// `codec` must be null (yielding a generic context) or a valid
    /// `*const AVCodec` (for example from `avcodec::find_decoder`).
    pub unsafe fn new(codec: *const AVCodec) -> Result<Self, AvError> {
        // SAFETY: the caller upholds the `codec` precondition; `alloc_context3`
        //         returns a non-null context or a negative error code.
        let ptr = unsafe { crate::avcodec::alloc_context3(codec) }.map_err(AvError::new)?;
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

    /// Opens the context with `codec` and optional dictionary `options`.
    ///
    /// # Safety
    ///
    /// `codec` must be a valid `*const AVCodec`, and `options` must be null or a
    /// valid `*mut *mut AVDictionary`.
    pub unsafe fn open(
        &mut self,
        codec: *const AVCodec,
        options: *mut *mut AVDictionary,
    ) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned context; the caller upholds
        //         `codec` / `options`.
        unsafe { crate::avcodec::open2(self.ptr.as_ptr(), codec, options) }.map_err(AvError::new)
    }

    /// Sends a packet to the decoder (a null packet flushes it).
    ///
    /// # Safety
    ///
    /// `pkt` must be null or a valid `*const AVPacket`.
    pub unsafe fn send_packet(&mut self, pkt: *const AVPacket) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid open context; the caller upholds `pkt`.
        unsafe { crate::avcodec::send_packet(self.ptr.as_ptr(), pkt) }.map_err(AvError::new)
    }

    /// Receives a decoded frame from the decoder.
    ///
    /// # Safety
    ///
    /// `frame` must be a valid `*mut AVFrame`.
    pub unsafe fn receive_frame(&mut self, frame: *mut AVFrame) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid open context; the caller upholds `frame`.
        unsafe { crate::avcodec::receive_frame(self.ptr.as_ptr(), frame) }.map_err(AvError::new)
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
        // A null codec yields a generic context, so this does not depend on any
        // specific decoder being present in the linked FFmpeg build.
        // SAFETY: a null codec pointer is valid for `alloc_context3`.
        let ctx = unsafe { CodecContext::new(std::ptr::null()) }.expect("alloc should succeed");
        assert!(!ctx.as_ptr().is_null());
        // Dropping `ctx` here frees the context exactly once (no panic / double free).
    }
}
