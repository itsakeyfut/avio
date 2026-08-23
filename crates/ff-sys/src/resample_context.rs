//! RAII owner for an `SwrContext` (libswresample audio resampling).
//!
//! [`ResampleContext`] allocates, configures, and initializes a resampling
//! context, and frees it exactly once on drop, replacing the manual
//! `swresample::alloc_set_opts2` + `swresample::init` + `swr_free`
//! sequence. The value is always initialized once constructed (there is no
//! un-initialized state), so the query methods are safe;
//! [`convert`](ResampleContext::convert) stays `unsafe` because it takes raw
//! plane pointers (owned Frame handling is a later step), and [`new`](Self::new)
//! is `unsafe` because it takes raw channel-layout pointers.

use std::os::raw::c_int;
use std::ptr::NonNull;

use crate::{
    AVChannelLayout, AVSampleFormat, AvError, SwrContext, swr_free as ffi_swr_free,
    swr_get_out_samples as ffi_swr_get_out_samples,
};

/// An owned, initialized `SwrContext`.
///
/// The context is freed exactly once on drop. This is guaranteed by
/// construction: the value owns a [`NonNull`] and is neither `Copy` nor `Clone`,
/// so it drops exactly once and cannot be duplicated.
#[derive(Debug)]
pub struct ResampleContext {
    ptr: NonNull<SwrContext>,
}

impl ResampleContext {
    /// Allocates, configures, and initializes a resampling context for the
    /// given input / output audio parameters.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if the parameters are invalid or initialization
    /// fails.
    ///
    /// # Safety
    ///
    /// `out_ch_layout` and `in_ch_layout` must be valid `*const AVChannelLayout`
    /// pointers for the duration of the call.
    pub unsafe fn new(
        out_ch_layout: *const AVChannelLayout,
        out_sample_fmt: AVSampleFormat,
        out_sample_rate: c_int,
        in_ch_layout: *const AVChannelLayout,
        in_sample_fmt: AVSampleFormat,
        in_sample_rate: c_int,
    ) -> Result<Self, AvError> {
        // SAFETY: the caller upholds the channel-layout pointers; `alloc_set_opts2`
        //         validates the sample rates and returns a non-null context or an
        //         error code.
        let ptr = unsafe {
            crate::swresample::alloc_set_opts2(
                out_ch_layout,
                out_sample_fmt,
                out_sample_rate,
                in_ch_layout,
                in_sample_fmt,
                in_sample_rate,
            )
        }
        .map_err(AvError::new)?;
        // `alloc_set_opts2` returns `Ok` only with a non-null context. Wrap it in
        // the owned type *before* initializing, so an `init` failure drops `self`
        // (Drop = `swr_free`) rather than leaking the allocated context.
        let me = NonNull::new(ptr)
            .ok_or_else(|| AvError::new(crate::error_codes::ENOMEM))
            .map(|ptr| Self { ptr })?;
        // SAFETY: `me.ptr` is a freshly allocated and configured context.
        unsafe { crate::swresample::init(me.ptr.as_ptr()) }.map_err(AvError::new)?;
        Ok(me)
    }

    /// Returns the context pointer for read-only use.
    #[must_use]
    pub const fn as_ptr(&self) -> *const SwrContext {
        self.ptr.as_ptr()
    }

    /// Returns the context pointer for mutation and FFI calls.
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut SwrContext {
        self.ptr.as_ptr()
    }

    /// Converts (resamples / reformats) audio samples into the output buffers.
    ///
    /// Returns the number of samples produced per channel. Passing a null `in_`
    /// with `in_count` 0 flushes buffered samples.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if conversion fails.
    ///
    /// # Safety
    ///
    /// `out` / `in_` and their counts must describe buffers valid for this
    /// context's configured formats (or a null `in_` with a zero count).
    pub unsafe fn convert(
        &mut self,
        out: *mut *mut u8,
        out_count: c_int,
        in_: *const *const u8,
        in_count: c_int,
    ) -> Result<c_int, AvError> {
        // SAFETY: `self.ptr` is a valid initialized context; the caller upholds
        //         the sample buffers.
        unsafe { crate::swresample::convert(self.ptr.as_ptr(), out, out_count, in_, in_count) }
            .map_err(AvError::new)
    }

    /// Returns the number of output samples a `swr_convert` with `in_samples`
    /// input samples would currently produce (including buffered delay).
    #[must_use]
    pub fn get_out_samples(&mut self, in_samples: c_int) -> c_int {
        // SAFETY: `self.ptr` is a valid initialized context; the query takes no
        //         caller-supplied pointer.
        unsafe { ffi_swr_get_out_samples(self.ptr.as_ptr(), in_samples) }
    }
}

impl Drop for ResampleContext {
    fn drop(&mut self) {
        // SAFETY: we uniquely own the context (NonNull, not Copy/Clone), so this
        //         runs exactly once. `swr_free` frees it and writes null into our
        //         local copy of the pointer, which is then discarded. The raw
        //         binding is used directly so this type does not depend on the
        //         `swresample::free` wrapper.
        unsafe {
            let mut raw = self.ptr.as_ptr();
            ffi_swr_free(&mut raw);
        }
    }
}

// SAFETY: an `SwrContext` is not safe for concurrent access, but moving ownership
//         between threads is sound because Rust's ownership model guarantees
//         exclusive access.
unsafe impl Send for ResampleContext {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swresample::{channel_layout, sample_format};

    #[test]
    fn new_should_allocate_init_and_drop_cleanly() {
        // Standard stereo layouts need no file or codec, so this depends only on
        // the linked libswresample.
        let out_layout = channel_layout::stereo();
        let in_layout = channel_layout::stereo();
        // SAFETY: the two layouts are valid for the duration of the call.
        let ctx = unsafe {
            ResampleContext::new(
                &out_layout,
                sample_format::FLTP,
                48000,
                &in_layout,
                sample_format::S16,
                44100,
            )
        }
        .expect("context creation should succeed");
        assert!(!ctx.as_ptr().is_null());
        // Dropping `ctx` frees the context exactly once (no panic / double free).
    }

    #[test]
    fn new_should_error_on_invalid_params() {
        // A zero output sample rate is rejected by `alloc_set_opts2` (EINVAL), so
        // construction fails deterministically without allocating a context.
        let out_layout = channel_layout::stereo();
        let in_layout = channel_layout::stereo();
        // SAFETY: the two layouts are valid for the duration of the call.
        let result = unsafe {
            ResampleContext::new(
                &out_layout,
                sample_format::FLTP,
                0,
                &in_layout,
                sample_format::S16,
                44100,
            )
        };
        assert!(result.is_err());
    }
}
