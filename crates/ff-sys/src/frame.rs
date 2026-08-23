//! RAII owner for an `AVFrame`.
//!
//! [`Frame`] allocates a frame and frees it exactly once on drop, replacing the
//! manual `av_frame_alloc` + `av_frame_free` pair. Ownership is unique;
//! [`try_clone`](Frame::try_clone) makes a ref-counted copy (`av_frame_ref`)
//! rather than a deep copy. Scalar fields (width / height / format / pts / ...)
//! are read and written through the typed accessors below; plane data
//! (`data` / `linesize`) is not exposed as a raw-pointer accessor — it is handled
//! by the swscale / swresample safe APIs ([`ScaleContext`](crate::ScaleContext),
//! [`ResampleContext`](crate::ResampleContext)).

use std::os::raw::c_int;
use std::ptr::NonNull;

use crate::{
    AVFrame, AVRational, AvError, av_frame_alloc as ffi_av_frame_alloc,
    av_frame_free as ffi_av_frame_free, av_frame_get_buffer as ffi_av_frame_get_buffer,
    av_frame_move_ref as ffi_av_frame_move_ref, av_frame_ref as ffi_av_frame_ref,
    av_frame_unref as ffi_av_frame_unref,
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

    // ── Scalar field accessors ────────────────────────────────────────────────
    //
    // Each getter reads one plain scalar field of the frame; each setter writes
    // one. They let downstream crates configure and inspect a frame without
    // dereferencing the raw `AVFrame` pointer. Plane data (`data` / `linesize`)
    // is deliberately not exposed here (that would leak a raw pointer type); the
    // swscale / swresample safe APIs handle it.

    /// Returns the frame width in pixels (video frames).
    #[must_use]
    pub fn width(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned frame; `width` is a plain field.
        unsafe { (*self.ptr.as_ptr()).width }
    }

    /// Sets the frame width in pixels (video frames).
    pub fn set_width(&mut self, width: c_int) {
        // SAFETY: `self.ptr` is a valid owned frame; `width` is a plain field.
        unsafe { (*self.ptr.as_ptr()).width = width };
    }

    /// Returns the frame height in pixels (video frames).
    #[must_use]
    pub fn height(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned frame; `height` is a plain field.
        unsafe { (*self.ptr.as_ptr()).height }
    }

    /// Sets the frame height in pixels (video frames).
    pub fn set_height(&mut self, height: c_int) {
        // SAFETY: `self.ptr` is a valid owned frame; `height` is a plain field.
        unsafe { (*self.ptr.as_ptr()).height = height };
    }

    /// Returns the frame format (an `AVPixelFormat` or `AVSampleFormat` value).
    #[must_use]
    pub fn format(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned frame; `format` is a plain field.
        unsafe { (*self.ptr.as_ptr()).format }
    }

    /// Sets the frame format (an `AVPixelFormat` or `AVSampleFormat` value).
    pub fn set_format(&mut self, format: c_int) {
        // SAFETY: `self.ptr` is a valid owned frame; `format` is a plain field.
        unsafe { (*self.ptr.as_ptr()).format = format };
    }

    /// Returns the presentation timestamp (in the frame's time base).
    #[must_use]
    pub fn pts(&self) -> i64 {
        // SAFETY: `self.ptr` is a valid owned frame; `pts` is a plain field.
        unsafe { (*self.ptr.as_ptr()).pts }
    }

    /// Sets the presentation timestamp (in the frame's time base).
    pub fn set_pts(&mut self, pts: i64) {
        // SAFETY: `self.ptr` is a valid owned frame; `pts` is a plain field.
        unsafe { (*self.ptr.as_ptr()).pts = pts };
    }

    /// Returns the DTS copied from the packet that produced this frame.
    #[must_use]
    pub fn pkt_dts(&self) -> i64 {
        // SAFETY: `self.ptr` is a valid owned frame; `pkt_dts` is a plain field.
        unsafe { (*self.ptr.as_ptr()).pkt_dts }
    }

    /// Sets the DTS field of the frame.
    pub fn set_pkt_dts(&mut self, pkt_dts: i64) {
        // SAFETY: `self.ptr` is a valid owned frame; `pkt_dts` is a plain field.
        unsafe { (*self.ptr.as_ptr()).pkt_dts = pkt_dts };
    }

    /// Returns the frame duration (in the frame's time base).
    #[must_use]
    pub fn duration(&self) -> i64 {
        // SAFETY: `self.ptr` is a valid owned frame; `duration` is a plain field.
        unsafe { (*self.ptr.as_ptr()).duration }
    }

    /// Sets the frame duration (in the frame's time base).
    pub fn set_duration(&mut self, duration: i64) {
        // SAFETY: `self.ptr` is a valid owned frame; `duration` is a plain field.
        unsafe { (*self.ptr.as_ptr()).duration = duration };
    }

    /// Returns the frame's time base.
    #[must_use]
    pub fn time_base(&self) -> AVRational {
        // SAFETY: `self.ptr` is a valid owned frame; `time_base` is a plain field.
        unsafe { (*self.ptr.as_ptr()).time_base }
    }

    /// Sets the frame's time base.
    pub fn set_time_base(&mut self, time_base: AVRational) {
        // SAFETY: `self.ptr` is a valid owned frame; `time_base` is a plain field.
        unsafe { (*self.ptr.as_ptr()).time_base = time_base };
    }

    /// Returns the number of audio samples per channel (audio frames).
    #[must_use]
    pub fn nb_samples(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned frame; `nb_samples` is a plain field.
        unsafe { (*self.ptr.as_ptr()).nb_samples }
    }

    /// Returns the audio sample rate in Hz (audio frames).
    #[must_use]
    pub fn sample_rate(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned frame; `sample_rate` is a plain field.
        unsafe { (*self.ptr.as_ptr()).sample_rate }
    }

    /// Returns the number of audio channels (audio frames).
    ///
    /// Reads `ch_layout.nb_channels`.
    #[must_use]
    pub fn channels(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned frame; `ch_layout.nb_channels` is a
        //         plain field of the embedded channel-layout struct.
        unsafe { (*self.ptr.as_ptr()).ch_layout.nb_channels }
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
    fn scalar_accessors_should_round_trip_set_and_get() {
        let mut frame = Frame::new().expect("frame allocation should succeed");
        frame.set_width(1920);
        frame.set_height(1080);
        frame.set_format(crate::AVPixelFormat_AV_PIX_FMT_RGBA);
        frame.set_pts(12_345);
        frame.set_pkt_dts(6_789);
        frame.set_duration(33);
        frame.set_time_base(AVRational { num: 1, den: 30 });

        assert_eq!(frame.width(), 1920);
        assert_eq!(frame.height(), 1080);
        assert_eq!(frame.format(), crate::AVPixelFormat_AV_PIX_FMT_RGBA);
        assert_eq!(frame.pts(), 12_345);
        assert_eq!(frame.pkt_dts(), 6_789);
        assert_eq!(frame.duration(), 33);
        let tb = frame.time_base();
        assert_eq!((tb.num, tb.den), (1, 30));
    }

    #[test]
    fn audio_accessors_should_read_sample_fields() {
        // A fresh frame reports zeroed audio fields; the getters read them without
        // touching the plane data.
        let frame = Frame::new().expect("frame allocation should succeed");
        assert_eq!(frame.nb_samples(), 0);
        assert_eq!(frame.sample_rate(), 0);
        assert_eq!(frame.channels(), 0);
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
