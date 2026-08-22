//! RAII owner for an `AVFrame`.
//!
//! [`Frame`] allocates a frame and frees it exactly once on drop, replacing the
//! manual `av_frame_alloc` + `av_frame_free` pair. Ownership is unique;
//! [`try_clone`](Frame::try_clone) makes a ref-counted copy (`av_frame_ref`)
//! rather than a deep copy. Raw field access (data / linesize / pts / ...) is
//! still done through [`as_mut_ptr`](Frame::as_mut_ptr) for now (typed field
//! accessors are a later step).

use std::os::raw::c_int;
use std::ptr::NonNull;

use crate::{
    AVFrame, AvError, av_frame_alloc as ffi_av_frame_alloc, av_frame_free as ffi_av_frame_free,
    av_frame_get_buffer as ffi_av_frame_get_buffer, av_frame_move_ref as ffi_av_frame_move_ref,
    av_frame_ref as ffi_av_frame_ref, av_frame_unref as ffi_av_frame_unref,
};

/// An owned `AVFrame`.
///
/// The frame is freed exactly once on drop. This is guaranteed by construction:
/// the value owns a [`NonNull`] and is neither `Copy` nor `Clone`, so it drops
/// exactly once and cannot be duplicated (a ref-counted copy is made explicitly
/// via [`try_clone`](Self::try_clone)).
#[derive(Debug)]
pub struct Frame {
    ptr: NonNull<AVFrame>,
}

impl Frame {
    /// Allocates a new, empty frame.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if allocation fails.
    pub fn new() -> Result<Self, AvError> {
        // SAFETY: `av_frame_alloc` takes no arguments and returns a fresh frame or null.
        let ptr = unsafe { ffi_av_frame_alloc() };
        NonNull::new(ptr)
            .ok_or_else(|| AvError::new(crate::error_codes::ENOMEM))
            .map(|ptr| Self { ptr })
    }

    /// Returns the frame pointer for read-only use.
    #[must_use]
    pub const fn as_ptr(&self) -> *const AVFrame {
        self.ptr.as_ptr()
    }

    /// Returns the frame pointer for mutation and FFI calls.
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut AVFrame {
        self.ptr.as_ptr()
    }

    /// Unreferences the frame's buffers, returning it to a blank state.
    pub fn unref(&mut self) {
        // SAFETY: `self.ptr` is a valid owned frame.
        unsafe { ffi_av_frame_unref(self.ptr.as_ptr()) };
    }

    /// Allocates data buffers for the frame according to its already-set
    /// `format` / dimensions (video) or `nb_samples` / channel layout (audio).
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the frame's parameters are unset/invalid or
    /// allocation fails.
    pub fn get_buffer(&mut self, align: c_int) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned frame; `av_frame_get_buffer` validates
        //         the frame's parameters and returns an error code rather than
        //         faulting when they are unset.
        let ret = unsafe { ffi_av_frame_get_buffer(self.ptr.as_ptr(), align) };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(())
        }
    }

    /// Moves `src`'s buffers into `self`, leaving `src` blank (but still valid).
    pub fn move_ref(&mut self, src: &mut Frame) {
        // SAFETY: `self` and `src` are valid owned frames; `av_frame_move_ref`
        //         transfers ownership of `src`'s buffers into `self` and resets
        //         `src` to a blank frame (which remains safe to drop).
        unsafe { ffi_av_frame_move_ref(self.ptr.as_ptr(), src.ptr.as_ptr()) };
    }

    /// Makes a ref-counted copy of this frame (`av_frame_ref`), sharing the
    /// underlying buffers rather than deep-copying.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the copy cannot be allocated / referenced.
    pub fn try_clone(&self) -> Result<Self, AvError> {
        let dst = Self::new()?;
        // SAFETY: `dst` is a fresh blank frame and `self` is a valid frame;
        //         `av_frame_ref` ref-counts `self`'s buffers into `dst`.
        let ret = unsafe { ffi_av_frame_ref(dst.ptr.as_ptr(), self.ptr.as_ptr()) };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(dst)
        }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: we uniquely own the frame (NonNull, not Copy/Clone), so this runs
        //         exactly once. `av_frame_free` frees it and writes null into our
        //         local copy of the pointer, which is then discarded.
        unsafe {
            let mut raw = self.ptr.as_ptr();
            ffi_av_frame_free(&mut raw);
        }
    }
}

// SAFETY: an `AVFrame` is not safe for concurrent access, but moving ownership
//         between threads is sound because Rust's ownership model guarantees
//         exclusive access.
unsafe impl Send for Frame {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_should_allocate_and_drop_cleanly() {
        let frame = Frame::new().expect("frame allocation should succeed");
        assert!(!frame.as_ptr().is_null());
        // Dropping `frame` frees it exactly once (no panic / double free).
    }

    #[test]
    fn try_clone_should_produce_an_independent_owner() {
        let mut frame = Frame::new().expect("frame allocation should succeed");
        // `av_frame_ref` needs referenced buffers, so give the frame a small valid
        // RGBA image first (no file / codec needed).
        // SAFETY: `frame` is a valid owned frame; setting these plain scalar fields
        //         before `get_buffer` is how FFmpeg expects a video frame configured.
        unsafe {
            (*frame.as_mut_ptr()).format = crate::AVPixelFormat_AV_PIX_FMT_RGBA;
            (*frame.as_mut_ptr()).width = 16;
            (*frame.as_mut_ptr()).height = 16;
        }
        frame
            .get_buffer(0)
            .expect("buffer allocation should succeed");
        let clone = frame.try_clone().expect("ref-count clone should succeed");
        // A ref-counted clone shares the same underlying buffer (not a deep copy).
        // SAFETY: both frames are valid owned frames with an allocated buffer.
        unsafe {
            assert_eq!(
                (*clone.as_ptr()).data[0],
                (*frame.as_ptr()).data[0],
                "try_clone should share the ref-counted buffer"
            );
        }
        // Both `frame` and `clone` drop independently (ref-counted), no double free.
    }

    #[test]
    fn move_ref_should_transfer_buffer_and_blank_the_source() {
        let mut src = Frame::new().expect("frame allocation should succeed");
        // SAFETY: `src` is a valid owned frame; setting these plain scalar fields
        //         before `get_buffer` is how FFmpeg expects a video frame configured.
        unsafe {
            (*src.as_mut_ptr()).format = crate::AVPixelFormat_AV_PIX_FMT_RGBA;
            (*src.as_mut_ptr()).width = 16;
            (*src.as_mut_ptr()).height = 16;
        }
        src.get_buffer(0).expect("buffer allocation should succeed");
        let mut dst = Frame::new().expect("frame allocation should succeed");
        dst.move_ref(&mut src);
        // SAFETY: both frames are valid owned frames.
        unsafe {
            assert!(
                !(*dst.as_ptr()).data[0].is_null(),
                "dst should own the moved buffer"
            );
            assert!(
                (*src.as_ptr()).data[0].is_null(),
                "src should be blank after the move"
            );
        }
        // Both drop cleanly: `dst` frees the moved buffer, `src` is blank.
    }
}
