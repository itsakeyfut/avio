//! RAII owner for an `SwrContext` (libswresample audio resampling).
//!
//! [`ResampleContext`] allocates, configures, and initializes a resampling
//! context, and frees it exactly once on drop, replacing the manual
//! `swresample::alloc_set_opts2` + `swresample::init` + `swr_free`
//! sequence. The value is always initialized once constructed (there is no
//! un-initialized state), so the query methods are safe.
//! [`convert_into_planes`](ResampleContext::convert_into_planes) resamples an
//! owned [`Frame`]'s audio into caller-provided plane slices, and its mirror
//! [`convert_into_frame`](ResampleContext::convert_into_frame) resamples caller
//! plane slices into an owned output [`Frame`]; both take borrowed slices instead
//! of raw pointers. [`convert`](ResampleContext::convert) stays `unsafe` for
//! callers that still pass raw plane pointers, and [`new`](Self::new) is `unsafe`
//! because it takes raw channel-layout pointers.

use std::os::raw::c_int;
use std::ptr::NonNull;

use crate::{
    AV_NUM_DATA_POINTERS, AVChannelLayout, AVSampleFormat, AvError, Frame, SwrContext,
    swr_free as ffi_swr_free, swr_get_out_samples as ffi_swr_get_out_samples,
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

    /// Resamples / reformats an owned source [`Frame`]'s audio into the
    /// caller-provided output plane slices (safe wrapper).
    ///
    /// `dst` holds one mutable byte slice per output plane (one per channel for
    /// planar formats, a single interleaved slice for packed formats) and
    /// `dst_count` is the number of samples per channel the buffers can hold. The
    /// input samples and their count are read from `src`. Returns the number of
    /// samples produced per channel.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if conversion fails.
    ///
    /// # Safety
    ///
    /// `swr_convert` writes up to `dst_count` samples into each output plane, a
    /// bound the slice types cannot express. The caller must ensure:
    /// - each `dst[i]` is at least `dst_count * bytes_per_sample` bytes;
    /// - `dst` has one entry per output plane the configured format needs
    ///   (planar formats need one plane per channel); at most
    ///   [`AV_NUM_DATA_POINTERS`] planes are forwarded, so a planar output with
    ///   more than that many channels is not supported here;
    /// - `src` is a valid audio [`Frame`] whose planes hold `src.nb_samples`
    ///   input samples.
    /// An undersized or missing `dst` plane is an out-of-bounds / null write.
    pub unsafe fn convert_into_planes(
        &mut self,
        dst: &mut [&mut [u8]],
        dst_count: c_int,
        src: &Frame,
    ) -> Result<c_int, AvError> {
        // Build fixed-size output / input pointer arrays. Unused entries stay
        // null, matching how FFmpeg treats absent planes.
        const PLANES: usize = AV_NUM_DATA_POINTERS as usize;
        let mut out_ptrs: [*mut u8; PLANES] = [std::ptr::null_mut(); PLANES];
        for (i, plane) in dst.iter_mut().enumerate().take(PLANES) {
            out_ptrs[i] = plane.as_mut_ptr();
        }

        let src_ptr = src.as_ptr();
        // SAFETY: `self.ptr` is a valid initialized context. `out_ptrs` describes
        //         the caller's mutable output planes and `in_ptrs` a local copy of
        //         `src`'s plane pointers, both valid for the duration of the call;
        //         `swr_convert` reads/writes only within them for the sample counts
        //         given.
        unsafe {
            let in_ptrs = (*src_ptr).data;
            let in_count = (*src_ptr).nb_samples;
            crate::swresample::convert(
                self.ptr.as_ptr(),
                out_ptrs.as_mut_ptr(),
                dst_count,
                in_ptrs.as_ptr() as *const *const u8,
                in_count,
            )
        }
        .map_err(AvError::new)
    }

    /// Resamples / reformats caller-provided input plane slices into an owned
    /// output [`Frame`]'s planes (safe-borrow wrapper, mirror of
    /// [`convert_into_planes`](Self::convert_into_planes) with the roles swapped).
    ///
    /// `in_planes` holds one byte slice per input plane (one per channel for
    /// planar formats, a single interleaved slice for packed formats) and
    /// `in_count` is the number of samples per channel they hold. The output is
    /// written into `dst`'s allocated planes, up to `dst.nb_samples` samples per
    /// channel. Returns the number of samples produced per channel.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if conversion fails.
    ///
    /// # Safety
    ///
    /// `swr_convert` reads `in_count` samples from each input plane and writes up
    /// to `dst.nb_samples` samples into each output plane, bounds the slice types
    /// cannot express. The caller must ensure:
    /// - `dst` is a get_buffer'd audio [`Frame`] whose planes hold at least
    ///   `dst.nb_samples` samples per channel for the configured output format;
    /// - each `in_planes[i]` is at least `in_count * bytes_per_sample` bytes;
    /// - `in_planes` has one entry per input plane the configured format needs
    ///   (planar formats need one plane per channel); at most
    ///   [`AV_NUM_DATA_POINTERS`] planes are forwarded on each side.
    /// An undersized or missing plane is an out-of-bounds / null access.
    pub unsafe fn convert_into_frame(
        &mut self,
        dst: &mut Frame,
        in_planes: &[&[u8]],
        in_count: c_int,
    ) -> Result<c_int, AvError> {
        // Build fixed-size input pointer array from the borrowed slices; unused
        // entries stay null, matching how FFmpeg treats absent planes.
        const PLANES: usize = AV_NUM_DATA_POINTERS as usize;
        let mut in_ptrs: [*const u8; PLANES] = [std::ptr::null(); PLANES];
        for (i, plane) in in_planes.iter().enumerate().take(PLANES) {
            in_ptrs[i] = plane.as_ptr();
        }

        let dst_ptr = dst.as_mut_ptr();
        // SAFETY: `self.ptr` is a valid initialized context. `out_ptrs` is a local
        //         copy of `dst`'s (get_buffer'd) output plane pointers and `in_ptrs`
        //         a local copy of the caller's input plane pointers, both valid for
        //         the duration of the call; `swr_convert` reads/writes only within
        //         them for the sample counts given (upheld by the caller, see
        //         # Safety).
        unsafe {
            let mut out_ptrs: [*mut u8; PLANES] = (*dst_ptr).data;
            let out_count = (*dst_ptr).nb_samples;
            crate::swresample::convert(
                self.ptr.as_ptr(),
                out_ptrs.as_mut_ptr(),
                out_count,
                in_ptrs.as_ptr(),
                in_count,
            )
        }
        .map_err(AvError::new)
    }

    /// Flushes the resampler's buffered samples into `dst` (a `swr_convert` with a
    /// NULL input), returning the number of samples produced per channel.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if conversion fails.
    ///
    /// # Safety
    ///
    /// The NULL input needs no input buffer, but `swr_convert` still writes up to
    /// `dst.nb_samples` samples into each output plane, a bound the slice types
    /// cannot express. The caller must ensure `dst` is a get_buffer'd audio
    /// [`Frame`] whose planes hold at least `dst.nb_samples` samples per channel
    /// for the configured output format; otherwise the null output planes are an
    /// out-of-bounds / null write.
    pub unsafe fn flush_into_frame(&mut self, dst: &mut Frame) -> Result<c_int, AvError> {
        let dst_ptr = dst.as_mut_ptr();
        // SAFETY: `self.ptr` is a valid initialized context. `out_ptrs` is a local
        //         copy of `dst`'s (get_buffer'd) output plane pointers, valid for the
        //         duration of the call; the input is null with count 0 (the documented
        //         flush signal), so no input buffer is read. `swr_convert` writes only
        //         within the output planes for `out_count` samples (upheld by the
        //         caller, see # Safety).
        unsafe {
            let mut out_ptrs: [*mut u8; AV_NUM_DATA_POINTERS as usize] = (*dst_ptr).data;
            let out_count = (*dst_ptr).nb_samples;
            crate::swresample::convert(
                self.ptr.as_ptr(),
                out_ptrs.as_mut_ptr(),
                out_count,
                std::ptr::null(),
                0,
            )
        }
        .map_err(AvError::new)
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
        // `get_out_samples` exercises the initialized context without touching a
        // raw pointer; a non-negative estimate confirms it is live.
        let mut ctx = ctx;
        assert!(ctx.get_out_samples(1024) >= 0);
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

    #[test]
    fn convert_into_planes_should_write_resampled_samples() {
        // Identity stereo S16 resample (48k -> 48k): exercises the safe plane-slice
        // convert end to end without a rate change or buffering delay.
        let out_layout = channel_layout::stereo();
        let in_layout = channel_layout::stereo();
        // SAFETY: the two layouts are valid for the duration of the call.
        let mut ctx = unsafe {
            ResampleContext::new(
                &out_layout,
                sample_format::S16,
                48000,
                &in_layout,
                sample_format::S16,
                48000,
            )
        }
        .expect("context creation should succeed");

        let samples: c_int = 256;
        let mut src = crate::Frame::new().expect("frame allocation should succeed");
        // SAFETY: set the audio fields on a fresh frame, then allocate its buffer.
        unsafe {
            let p = src.as_mut_ptr();
            (*p).format = sample_format::S16;
            (*p).nb_samples = samples;
            (*p).sample_rate = 48000;
            channel_layout::set_default(&raw mut (*p).ch_layout, 2);
        }
        src.get_buffer(0).expect("src buffer alloc should succeed");

        // Packed S16 stereo output: one plane of `samples * 2ch * 2 bytes`.
        let mut out = vec![0u8; samples as usize * 2 * 2];
        let mut dst: [&mut [u8]; 1] = [out.as_mut_slice()];
        // SAFETY: `dst[0]` is sized for `samples` stereo S16 samples; `src` holds
        //         `samples` input samples.
        let produced = unsafe { ctx.convert_into_planes(&mut dst, samples, &src) }
            .expect("conversion should succeed");
        assert_eq!(
            produced, samples,
            "identity resample returns all input samples"
        );
    }

    #[test]
    fn convert_into_frame_should_write_resampled_samples() {
        // Identity stereo S16 resample (48k -> 48k) into a get_buffer'd output
        // frame: exercises the safe frame-output convert without a rate change.
        let out_layout = channel_layout::stereo();
        let in_layout = channel_layout::stereo();
        // SAFETY: the two layouts are valid for the duration of the call.
        let mut ctx = unsafe {
            ResampleContext::new(
                &out_layout,
                sample_format::S16,
                48000,
                &in_layout,
                sample_format::S16,
                48000,
            )
        }
        .expect("context creation should succeed");

        let samples: c_int = 256;

        // Source: packed S16 stereo input plane holding `samples` samples.
        let in_plane = vec![0u8; samples as usize * 2 * 2];
        let in_planes: [&[u8]; 1] = [in_plane.as_slice()];

        // Destination: a get_buffer'd packed S16 stereo frame sized for `samples`.
        let mut dst = crate::Frame::new().expect("frame allocation should succeed");
        // SAFETY: set the audio fields on a fresh frame, then allocate its buffer.
        unsafe {
            let p = dst.as_mut_ptr();
            (*p).format = sample_format::S16;
            (*p).nb_samples = samples;
            (*p).sample_rate = 48000;
            channel_layout::set_default(&raw mut (*p).ch_layout, 2);
        }
        dst.get_buffer(0).expect("dst buffer alloc should succeed");

        // SAFETY: `dst` is a get_buffer'd frame sized for `samples`; `in_planes[0]`
        //         holds `samples` packed stereo S16 input samples.
        let produced = unsafe { ctx.convert_into_frame(&mut dst, &in_planes, samples) }
            .expect("conversion should succeed");
        assert_eq!(
            produced, samples,
            "identity resample returns all input samples"
        );
    }
}
