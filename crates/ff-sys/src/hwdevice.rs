//! RAII owner for a hardware device context (`AVBufferRef` from
//! `av_hwdevice_ctx_create`).
//!
//! [`HwDeviceContext`] creates a hardware device (CUDA / QSV / VAAPI / ...) and
//! unreferences it exactly once on drop, replacing the manual
//! `av_hwdevice_ctx_create` + `av_buffer_unref` pair. Its constructor is safe (it
//! takes only an [`AVHWDeviceType`] and creates the default device). Hand it to a
//! codec with [`CodecContext::set_hw_device_ctx`](crate::CodecContext::set_hw_device_ctx),
//! which keeps its own reference; the two references are released independently.

use std::ptr::NonNull;

use crate::{
    AVBufferRef, AVHWDeviceType, AvError, av_buffer_unref as ffi_av_buffer_unref,
    av_hwdevice_ctx_create as ffi_av_hwdevice_ctx_create,
};

/// An owned hardware device context.
///
/// The reference is released exactly once on drop. This is guaranteed by
/// construction: the value owns a [`NonNull`] and is neither `Copy` nor `Clone`,
/// so it drops exactly once and cannot be duplicated.
#[derive(Debug)]
pub struct HwDeviceContext {
    ptr: NonNull<AVBufferRef>,
}

impl HwDeviceContext {
    /// Creates the default hardware device of the given type (e.g.
    /// `AV_HWDEVICE_TYPE_CUDA`).
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] when the backend is unavailable on this system
    /// (no driver / device), so callers can fall back to software decoding.
    pub fn new(device_type: AVHWDeviceType) -> Result<Self, AvError> {
        let mut ptr: *mut AVBufferRef = std::ptr::null_mut();
        // SAFETY: `av_hwdevice_ctx_create` writes a new device reference into
        //         `ptr` (or leaves it null and returns < 0). The default device is
        //         requested with null `device` / `opts` and `flags = 0`.
        let ret = unsafe {
            ffi_av_hwdevice_ctx_create(
                std::ptr::addr_of_mut!(ptr),
                device_type,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            )
        };
        if ret < 0 {
            return Err(AvError::new(ret));
        }
        NonNull::new(ptr)
            .ok_or_else(|| AvError::new(crate::error_codes::ENOMEM))
            .map(|ptr| Self { ptr })
    }

    /// Returns the underlying device reference for FFI calls within the crate.
    ///
    /// Not part of the public API: callers hand a `&HwDeviceContext` to
    /// [`CodecContext::set_hw_device_ctx`](crate::CodecContext::set_hw_device_ctx),
    /// which takes its own reference.
    pub(crate) fn as_raw(&self) -> *mut AVBufferRef {
        self.ptr.as_ptr()
    }
}

impl Drop for HwDeviceContext {
    fn drop(&mut self) {
        // SAFETY: we uniquely own this reference (NonNull, not Copy/Clone), so
        //         this runs exactly once. `av_buffer_unref` drops our reference
        //         (freeing the device when the last reference goes) and writes
        //         null into our local copy of the pointer, which is discarded.
        unsafe {
            let mut raw = self.ptr.as_ptr();
            ffi_av_buffer_unref(std::ptr::addr_of_mut!(raw));
        }
    }
}

// SAFETY: an `AVBufferRef` device context is reference-counted and safe to move
//         between threads; Rust's ownership model guarantees exclusive access to
//         this owner, and we expose no shared mutation.
unsafe impl Send for HwDeviceContext {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_should_return_a_result_without_panicking() {
        // Creating a CUDA device either succeeds (a GPU / driver is present) or
        // returns an error (CI has no backend). Either way the constructor must
        // not panic, and a successful context must drop cleanly (no double free).
        match HwDeviceContext::new(crate::AVHWDeviceType_AV_HWDEVICE_TYPE_CUDA) {
            Ok(ctx) => {
                assert!(!ctx.as_raw().is_null());
                // `ctx` drops here, unreferencing exactly once.
            }
            Err(e) => {
                // A well-formed error code, not a panic.
                assert!(e.code() < 0);
            }
        }
    }
}
