//! RAII owner for an `SwsContext` (libswscale scaling / pixel-format conversion).
//!
//! [`ScaleContext`] allocates a scaling context and frees it exactly once on
//! drop, replacing the manual `swscale::get_context` + `sws_freeContext`
//! pair. Its constructor is safe (it takes only dimensions, pixel formats, and
//! flags). The safe [`scale_frames`](ScaleContext::scale_frames) /
//! [`scale_planes`](ScaleContext::scale_planes) / [`scale_slices`](ScaleContext::scale_slices)
//! methods drive the scaler from owned [`Frame`] / borrowed plane slices, so no
//! public signature exposes a raw plane pointer.

use std::os::raw::c_int;
use std::ptr::NonNull;

use crate::{
    AV_NUM_DATA_POINTERS, AVPixelFormat, AvError, Frame, SwsContext,
    sws_freeContext as ffi_sws_freeContext,
};

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

    /// Scales / converts an owned source [`Frame`] into an owned destination
    /// [`Frame`] (safe wrapper).
    ///
    /// Reads the source's plane pointers and strides and the destination's, so
    /// callers never touch a raw `AVFrame` field. Returns the height of the
    /// output slice.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if scaling fails.
    ///
    /// # Safety
    ///
    /// `sws_scale` reads/writes the frames' planes using their own strides and
    /// heights, which the type system cannot bound. The caller must ensure:
    /// - `dst` has its buffers allocated ([`Frame::get_buffer`]) and sized for
    ///   this context's **output** geometry (a null/undersized `dst` plane is an
    ///   out-of-bounds write);
    /// - `src`'s geometry matches this context's **input** geometry.
    pub unsafe fn scale_frames(&mut self, src: &Frame, dst: &mut Frame) -> Result<c_int, AvError> {
        let src_ptr = src.as_ptr();
        let dst_ptr = dst.as_mut_ptr();
        // SAFETY: `src` / `dst` are valid owned frames; `src`'s planes are readable
        //         and `dst`'s buffers are allocated by the caller. The plane and
        //         stride arrays live in the `AVFrame`s and are sized for the formats
        //         this context was built for, so `sws_scale` stays within them.
        unsafe {
            let src_slice_h = (*src_ptr).height;
            crate::swscale::scale(
                self.ptr.as_ptr(),
                (*src_ptr).data.as_ptr() as *const *const u8,
                (*src_ptr).linesize.as_ptr(),
                0,
                src_slice_h,
                (*dst_ptr).data.as_ptr() as *const *mut u8,
                (*dst_ptr).linesize.as_ptr(),
            )
        }
        .map_err(AvError::new)
    }

    /// Scales / converts borrowed source plane slices into an owned destination
    /// [`Frame`] (safe wrapper).
    ///
    /// `src` holds one byte slice per source plane and `src_strides` the matching
    /// per-plane stride (line size in bytes); `src_h` is the number of source rows
    /// to process. The destination must already have its buffers allocated.
    ///
    /// The strides are taken as-is (top-down, positive) following the
    /// `av_image_copy` convention, so this path never reintroduces the
    /// negative-linesize row inversion.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if scaling fails.
    ///
    /// # Safety
    ///
    /// `sws_scale` reads up to `src_strides[i] * src_h` bytes from each `src[i]`,
    /// a bound the slice types cannot express. The caller must ensure:
    /// - `src.len() == src_strides.len()` and both describe this context's input
    ///   planes;
    /// - each `src[i]` is at least `src_strides[i] * src_h` bytes (per-plane
    ///   height per the pixel format's subsampling);
    /// - `dst` has its buffers allocated ([`Frame::get_buffer`]) and sized for
    ///   this context's output geometry.
    /// A shorter slice or mismatched stride is an out-of-bounds read.
    pub unsafe fn scale_planes(
        &mut self,
        src: &[&[u8]],
        src_strides: &[c_int],
        src_h: c_int,
        dst: &mut Frame,
    ) -> Result<c_int, AvError> {
        // Build fixed-size plane / stride arrays from the borrowed slices. Unused
        // entries stay null / zero, matching how FFmpeg frames leave absent planes.
        const PLANES: usize = AV_NUM_DATA_POINTERS as usize;
        let mut src_ptrs: [*const u8; PLANES] = [std::ptr::null(); PLANES];
        let mut strides: [c_int; PLANES] = [0; PLANES];
        for (i, (plane, &stride)) in src.iter().zip(src_strides).enumerate().take(PLANES) {
            src_ptrs[i] = plane.as_ptr();
            strides[i] = stride;
        }

        let dst_ptr = dst.as_mut_ptr();
        // SAFETY: `src_ptrs` / `strides` describe the borrowed source planes for
        //         the duration of the call; `dst` is a valid owned frame whose
        //         buffers the caller allocated. `sws_scale` reads/writes only within
        //         the sized planes.
        unsafe {
            crate::swscale::scale(
                self.ptr.as_ptr(),
                src_ptrs.as_ptr(),
                strides.as_ptr(),
                0,
                src_h,
                (*dst_ptr).data.as_ptr() as *const *mut u8,
                (*dst_ptr).linesize.as_ptr(),
            )
        }
        .map_err(AvError::new)
    }

    /// Scales / converts borrowed source plane slices into borrowed destination
    /// plane slices (safe wrapper).
    ///
    /// `src` holds one byte slice per source plane and `src_strides` the matching
    /// per-plane stride (line size in bytes); `src_h` is the number of source rows
    /// to process. `dst` holds one mutable byte slice per destination plane and
    /// `dst_strides` their per-plane strides. This is the fully borrowed-slice
    /// counterpart of [`scale_planes`](Self::scale_planes) for callers whose
    /// destination is a plain buffer rather than an owned [`Frame`].
    ///
    /// The strides are taken as-is (top-down, positive) following the
    /// `av_image_copy` convention, so this path never reintroduces the
    /// negative-linesize row inversion.
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if scaling fails.
    ///
    /// # Safety
    ///
    /// `sws_scale` reads up to `src_strides[i] * src_h` bytes from each `src[i]`
    /// and writes up to `dst_strides[i] * out_h` bytes into each `dst[i]`, bounds
    /// the slice types cannot express. The caller must ensure:
    /// - `src.len() == src_strides.len()` and `dst.len() == dst_strides.len()`,
    ///   both describing this context's input / output planes;
    /// - each `src[i]` is at least `src_strides[i] * src_h` bytes (per-plane
    ///   height per the pixel format's subsampling);
    /// - each `dst[i]` is at least `dst_strides[i] * out_h` bytes for this
    ///   context's output geometry;
    /// - no stride is negative.
    /// A shorter slice or mismatched stride is an out-of-bounds read / write.
    pub unsafe fn scale_slices(
        &mut self,
        src: &[&[u8]],
        src_strides: &[c_int],
        src_h: c_int,
        dst: &mut [&mut [u8]],
        dst_strides: &[c_int],
    ) -> Result<c_int, AvError> {
        // Build fixed-size plane / stride arrays from the borrowed slices. Unused
        // entries stay null / zero, matching how FFmpeg frames leave absent planes.
        const PLANES: usize = AV_NUM_DATA_POINTERS as usize;
        let mut src_ptrs: [*const u8; PLANES] = [std::ptr::null(); PLANES];
        let mut src_line: [c_int; PLANES] = [0; PLANES];
        for (i, (plane, &stride)) in src.iter().zip(src_strides).enumerate().take(PLANES) {
            src_ptrs[i] = plane.as_ptr();
            src_line[i] = stride;
        }

        let mut dst_ptrs: [*mut u8; PLANES] = [std::ptr::null_mut(); PLANES];
        let mut dst_line: [c_int; PLANES] = [0; PLANES];
        for (i, (plane, &stride)) in dst.iter_mut().zip(dst_strides).enumerate().take(PLANES) {
            dst_ptrs[i] = plane.as_mut_ptr();
            dst_line[i] = stride;
        }

        // SAFETY: `src_ptrs` / `dst_ptrs` and their stride arrays describe the
        //         borrowed planes for the duration of the call; `sws_scale`
        //         reads/writes only within the sized planes.
        unsafe {
            crate::swscale::scale(
                self.ptr.as_ptr(),
                src_ptrs.as_ptr(),
                src_line.as_ptr(),
                0,
                src_h,
                dst_ptrs.as_ptr(),
                dst_line.as_ptr(),
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
        let _ctx = ScaleContext::new(
            640,
            480,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            320,
            240,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            crate::swscale::scale_flags::BILINEAR,
        )
        .expect("context creation should succeed");
        // Dropping `_ctx` frees the context exactly once (no panic / double free).
    }

    #[test]
    fn scale_frames_should_downscale_an_rgb_frame() {
        // A plain RGB24 downscale needs no file or codec. Build a source frame
        // with allocated buffers and a destination frame, then scale.
        let mut src = Frame::new().expect("frame allocation should succeed");
        src.set_format(AVPixelFormat_AV_PIX_FMT_RGB24);
        src.set_width(64);
        src.set_height(64);
        src.get_buffer(0).expect("src buffer alloc should succeed");

        let mut dst = Frame::new().expect("frame allocation should succeed");
        dst.set_format(AVPixelFormat_AV_PIX_FMT_RGB24);
        dst.set_width(32);
        dst.set_height(32);
        dst.get_buffer(0).expect("dst buffer alloc should succeed");

        let mut ctx = ScaleContext::new(
            64,
            64,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            32,
            32,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            crate::swscale::scale_flags::BILINEAR,
        )
        .expect("context creation should succeed");

        // SAFETY: both frames are get_buffer'd RGB24 at the context geometry.
        let out_h = unsafe { ctx.scale_frames(&src, &mut dst) }.expect("scaling should succeed");
        assert_eq!(out_h, 32, "should process all 32 output rows");
    }

    #[test]
    fn scale_frames_should_preserve_vertical_row_order() {
        // RK-008 guard: a vertical row inversion would swap top and bottom. Paint
        // the source's top half bright and bottom half dark; after an identity-size
        // scale the destination must keep the top bright and the bottom dark.
        let (w, h) = (16i32, 16i32);
        let mut src = Frame::new().expect("frame allocation should succeed");
        src.set_format(AVPixelFormat_AV_PIX_FMT_RGB24);
        src.set_width(w);
        src.set_height(h);
        src.get_buffer(0).expect("src buffer alloc should succeed");
        // SAFETY: `src` has a get_buffer'd RGB24 plane of `linesize * h` bytes.
        unsafe {
            let p = src.as_mut_ptr();
            let data = (*p).data[0];
            let stride = (*p).linesize[0] as usize;
            for y in 0..h as usize {
                let val = if y < h as usize / 2 { 200u8 } else { 40u8 };
                std::ptr::write_bytes(data.add(y * stride), val, w as usize * 3);
            }
        }

        let mut dst = Frame::new().expect("frame allocation should succeed");
        dst.set_format(AVPixelFormat_AV_PIX_FMT_RGB24);
        dst.set_width(w);
        dst.set_height(h);
        dst.get_buffer(0).expect("dst buffer alloc should succeed");

        let mut ctx = ScaleContext::new(
            w,
            h,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            w,
            h,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            crate::swscale::scale_flags::BILINEAR,
        )
        .expect("context creation should succeed");
        // SAFETY: both frames are get_buffer'd RGB24 at the context geometry.
        unsafe { ctx.scale_frames(&src, &mut dst) }.expect("scaling should succeed");

        // SAFETY: `dst` is a get_buffer'd RGB24 plane.
        let (top, bottom) = unsafe {
            let p = dst.as_ptr();
            let data = (*p).data[0];
            let stride = (*p).linesize[0] as usize;
            (*data, *data.add((h as usize - 1) * stride))
        };
        assert!(
            top > bottom,
            "top row ({top}) must stay brighter than bottom ({bottom}); a row inversion would flip this"
        );
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

    #[test]
    fn scale_planes_should_downscale_from_borrowed_planes() {
        // Feed a single RGB24 plane (borrowed Vec) through the plane-slice path.
        let src_w = 64usize;
        let src_h = 64usize;
        let src_stride = src_w * 3;
        let src_buf = vec![0u8; src_stride * src_h];
        let src_planes: [&[u8]; 1] = [src_buf.as_slice()];
        let src_strides: [c_int; 1] = [src_stride as c_int];

        let mut dst = Frame::new().expect("frame allocation should succeed");
        dst.set_format(AVPixelFormat_AV_PIX_FMT_RGB24);
        dst.set_width(32);
        dst.set_height(32);
        dst.get_buffer(0).expect("dst buffer alloc should succeed");

        let mut ctx = ScaleContext::new(
            src_w as c_int,
            src_h as c_int,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            32,
            32,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            crate::swscale::scale_flags::BILINEAR,
        )
        .expect("context creation should succeed");

        // SAFETY: the single plane is sized `src_stride * src_h`; `dst` is
        // get_buffer'd at the context's output geometry.
        let out_h =
            unsafe { ctx.scale_planes(&src_planes, &src_strides, src_h as c_int, &mut dst) }
                .expect("plane scaling should succeed");
        assert_eq!(out_h, 32, "should process all 32 output rows");
    }

    #[test]
    fn scale_slices_should_downscale_from_borrowed_slices() {
        // Feed a single RGB24 plane (borrowed Vec) in and a borrowed Vec out
        // through the fully-slice path.
        let src_w = 64usize;
        let src_h = 64usize;
        let src_stride = src_w * 3;
        let src_buf = vec![0u8; src_stride * src_h];
        let src_planes: [&[u8]; 1] = [src_buf.as_slice()];
        let src_strides: [c_int; 1] = [src_stride as c_int];

        let dst_w = 32usize;
        let dst_h = 32usize;
        let dst_stride = dst_w * 3;
        let mut dst_buf = vec![0u8; dst_stride * dst_h];
        let mut dst_planes: [&mut [u8]; 1] = [dst_buf.as_mut_slice()];
        let dst_strides: [c_int; 1] = [dst_stride as c_int];

        let mut ctx = ScaleContext::new(
            src_w as c_int,
            src_h as c_int,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            dst_w as c_int,
            dst_h as c_int,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            crate::swscale::scale_flags::BILINEAR,
        )
        .expect("context creation should succeed");

        // SAFETY: the single src plane is sized `src_stride * src_h`; the single
        // dst plane is sized `dst_stride * dst_h`.
        let out_h = unsafe {
            ctx.scale_slices(
                &src_planes,
                &src_strides,
                src_h as c_int,
                &mut dst_planes,
                &dst_strides,
            )
        }
        .expect("slice scaling should succeed");
        assert_eq!(out_h, dst_h as c_int, "should process all 32 output rows");
    }

    #[test]
    fn scale_slices_should_preserve_vertical_row_order() {
        // RK-008 guard: strides are taken as-is (positive), so an identity-size
        // scale must keep the source's top-half brightness on top.
        let (w, h) = (16usize, 16usize);
        let stride = w * 3;
        let mut src_buf = vec![0u8; stride * h];
        for y in 0..h {
            let val = if y < h / 2 { 200u8 } else { 40u8 };
            for b in &mut src_buf[y * stride..y * stride + w * 3] {
                *b = val;
            }
        }
        let src_planes: [&[u8]; 1] = [src_buf.as_slice()];
        let src_strides: [c_int; 1] = [stride as c_int];

        let mut dst_buf = vec![0u8; stride * h];
        let mut dst_planes: [&mut [u8]; 1] = [dst_buf.as_mut_slice()];
        let dst_strides: [c_int; 1] = [stride as c_int];

        let mut ctx = ScaleContext::new(
            w as c_int,
            h as c_int,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            w as c_int,
            h as c_int,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            crate::swscale::scale_flags::BILINEAR,
        )
        .expect("context creation should succeed");

        // SAFETY: both single planes are sized `stride * h`.
        unsafe {
            ctx.scale_slices(
                &src_planes,
                &src_strides,
                h as c_int,
                &mut dst_planes,
                &dst_strides,
            )
        }
        .expect("slice scaling should succeed");

        let top = dst_buf[0];
        let bottom = dst_buf[(h - 1) * stride];
        assert!(
            top > bottom,
            "top row ({top}) must stay brighter than bottom ({bottom}); a row inversion would flip this"
        );
    }
}
