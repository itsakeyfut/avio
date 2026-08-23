//! RAII owner for an `SwsContext` (libswscale scaling / pixel-format conversion).
//!
//! [`ScaleContext`] allocates a scaling context and frees it exactly once on
//! drop, replacing the manual `swscale::get_context` + `sws_freeContext`
//! pair. Its constructor is safe (it takes only dimensions, pixel formats, and
//! flags); [`scale`](ScaleContext::scale) stays `unsafe` because it takes raw
//! plane pointers (owned Frame handling is a later step).

use std::os::raw::c_int;
use std::ptr::NonNull;

use crate::{AVPixelFormat, AvError, SwsContext, sws_freeContext as ffi_sws_freeContext};

/// An owned `SwsContext`.
///
/// The context is freed exactly once on drop. This is guaranteed by
/// construction: the value owns a [`NonNull`] and is neither `Copy` nor `Clone`,
/// so it drops exactly once and cannot be duplicated.
#[derive(Debug)]
pub struct ScaleContext {
    ptr: NonNull<SwsContext>,
}

impl ScaleContext {
    /// Allocates a scaling context for the given source / destination geometry.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the dimensions are invalid or the format
    /// combination is unsupported.
    pub fn new(
        src_w: c_int,
        src_h: c_int,
        src_fmt: AVPixelFormat,
        dst_w: c_int,
        dst_h: c_int,
        dst_fmt: AVPixelFormat,
        flags: c_int,
    ) -> Result<Self, AvError> {
        // SAFETY: `get_context` initialises FFmpeg and validates the dimensions;
        //         the returned context is owned by `self` and freed on drop.
        let ptr = unsafe {
            crate::swscale::get_context(src_w, src_h, src_fmt, dst_w, dst_h, dst_fmt, flags)
        }
        .map_err(AvError::new)?;
        // `get_context` returns `Ok` only with a non-null context.
        NonNull::new(ptr)
            .ok_or_else(|| AvError::new(crate::error_codes::ENOMEM))
            .map(|ptr| Self { ptr })
    }

    /// Returns the context pointer for read-only use.
    #[must_use]
    pub const fn as_ptr(&self) -> *const SwsContext {
        self.ptr.as_ptr()
    }

    /// Returns the context pointer for mutation and FFI calls.
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut SwsContext {
        self.ptr.as_ptr()
    }

    /// Scales / converts a slice of the source image into the destination.
    ///
    /// Returns the height of the output slice.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if scaling fails.
    ///
    /// # Safety
    ///
    /// The plane and stride arrays must be valid and sized for the formats and
    /// dimensions this context was created for.
    pub unsafe fn scale(
        &mut self,
        src: *const *const u8,
        src_stride: *const c_int,
        src_slice_y: c_int,
        src_slice_h: c_int,
        dst: *const *mut u8,
        dst_stride: *const c_int,
    ) -> Result<c_int, AvError> {
        // SAFETY: `self.ptr` is a valid owned scaling context; the caller upholds
        //         the plane / stride arrays.
        unsafe {
            crate::swscale::scale(
                self.ptr.as_ptr(),
                src,
                src_stride,
                src_slice_y,
                src_slice_h,
                dst,
                dst_stride,
            )
        }
        .map_err(AvError::new)
    }
}

impl Drop for ScaleContext {
    fn drop(&mut self) {
        // SAFETY: we uniquely own the context (NonNull, not Copy/Clone), so this
        //         runs exactly once. `sws_freeContext` takes the context pointer
        //         by value and frees it. The raw binding is used directly so this
        //         type does not depend on the `swscale::free_context` wrapper.
        unsafe {
            ffi_sws_freeContext(self.ptr.as_ptr());
        }
    }
}

// SAFETY: an `SwsContext` is not safe for concurrent access, but moving ownership
//         between threads is sound because Rust's ownership model guarantees
//         exclusive access.
unsafe impl Send for ScaleContext {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AVPixelFormat_AV_PIX_FMT_RGB24;

    #[test]
    fn new_should_allocate_and_drop_cleanly() {
        // A plain RGB downscale context needs no file or codec, so this does not
        // depend on anything beyond the linked libswscale.
        let ctx = ScaleContext::new(
            640,
            480,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            320,
            240,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            crate::swscale::scale_flags::BILINEAR,
        )
        .expect("context creation should succeed");
        assert!(!ctx.as_ptr().is_null());
        // Dropping `ctx` frees the context exactly once (no panic / double free).
    }

    #[test]
    fn new_should_error_on_zero_dimensions() {
        let result = ScaleContext::new(
            0,
            480,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            320,
            240,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            crate::swscale::scale_flags::BILINEAR,
        );
        assert!(result.is_err());
    }
}
