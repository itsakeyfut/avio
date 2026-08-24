//! RAII owner for an `AVFrame`.
//!
//! [`Frame`] allocates a frame and frees it exactly once on drop, replacing the
//! manual `av_frame_alloc` + `av_frame_free` pair. Ownership is unique;
//! [`try_clone`](Frame::try_clone) makes a ref-counted copy (`av_frame_ref`)
//! rather than a deep copy. Scalar fields (width / height / format / pts / ...)
//! are read and written through the typed accessors below. Plane data is never
//! exposed as a raw pointer: the [`video_plane`](Frame::video_plane) /
//! [`audio_plane`](Frame::audio_plane) accessors (and their `_mut` forms) return
//! self-sizing safe slices, and the swscale / swresample APIs
//! ([`ScaleContext`](crate::ScaleContext), [`ResampleContext`](crate::ResampleContext))
//! consume the frames for scaling / resampling.

use std::os::raw::c_int;
use std::ptr::NonNull;

use crate::{
    AV_NUM_DATA_POINTERS, AVFrame, AVRational, AvError, av_frame_alloc as ffi_av_frame_alloc,
    av_frame_free as ffi_av_frame_free, av_frame_get_buffer as ffi_av_frame_get_buffer,
    av_frame_move_ref as ffi_av_frame_move_ref, av_frame_ref as ffi_av_frame_ref,
    av_frame_unref as ffi_av_frame_unref, av_pix_fmt_desc_get as ffi_av_pix_fmt_desc_get,
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

    /// Sets the picture type (e.g. `AV_PICTURE_TYPE_I` to hint a keyframe).
    pub fn set_pict_type(&mut self, pict_type: crate::AVPictureType) {
        // SAFETY: `self.ptr` is a valid owned frame; `pict_type` is a plain field.
        unsafe { (*self.ptr.as_ptr()).pict_type = pict_type };
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

    /// Sets the number of audio samples per channel (audio frames).
    pub fn set_nb_samples(&mut self, nb_samples: c_int) {
        // SAFETY: `self.ptr` is a valid owned frame; `nb_samples` is a plain field.
        unsafe { (*self.ptr.as_ptr()).nb_samples = nb_samples };
    }

    /// Returns the audio sample rate in Hz (audio frames).
    #[must_use]
    pub fn sample_rate(&self) -> c_int {
        // SAFETY: `self.ptr` is a valid owned frame; `sample_rate` is a plain field.
        unsafe { (*self.ptr.as_ptr()).sample_rate }
    }

    /// Sets the audio sample rate in Hz (audio frames).
    pub fn set_sample_rate(&mut self, sample_rate: c_int) {
        // SAFETY: `self.ptr` is a valid owned frame; `sample_rate` is a plain field.
        unsafe { (*self.ptr.as_ptr()).sample_rate = sample_rate };
    }

    /// Copies `layout` into the frame's channel layout (audio frames).
    ///
    /// # Errors
    ///
    /// Returns an [`AvError`] if `av_channel_layout_copy` fails (e.g. allocation
    /// for an extended layout).
    pub fn set_ch_layout(&mut self, layout: &crate::AVChannelLayout) -> Result<(), AvError> {
        // SAFETY: `self.ptr` is a valid owned frame; `layout` is a valid channel
        //         layout; `av_channel_layout_copy` copies it into `ch_layout`.
        let ret = unsafe {
            crate::av_channel_layout_copy(
                &raw mut (*self.ptr.as_ptr()).ch_layout,
                std::ptr::from_ref(layout),
            )
        };
        if ret < 0 {
            Err(AvError::new(ret))
        } else {
            Ok(())
        }
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

    // ── Plane data accessors ──────────────────────────────────────────────────
    //
    // Typed, self-sizing views over one image / audio plane. Each length is
    // computed from the frame's own valid fields, so no raw pointer or size
    // leaks to the caller. `None` is returned for any plane that is absent or
    // whose format cannot be described (see per-method docs), which keeps these
    // methods safe.
    //
    // Invariant: the length is derived from the frame's current `format` /
    // dimensions / `nb_samples`, so these assume those were set before
    // [`get_buffer`](Self::get_buffer) and not enlarged afterwards (the normal
    // alloc -> set -> get_buffer -> use order). Growing a dimension after
    // `get_buffer` without reallocating would desync the fields from the buffer.

    /// Returns an immutable view of video plane `i`, sized to the plane's own
    /// `linesize[i] * plane_height(i)` bytes.
    ///
    /// Returns `None` when the plane is absent or cannot be sized: `i` is out of
    /// range, `data[i]` is null, the pixel format has no descriptor, or
    /// `linesize[i]` / `height` is not positive.
    #[must_use]
    pub fn video_plane(&self, i: usize) -> Option<&[u8]> {
        let len = self.video_plane_len(i)?;
        // SAFETY: `video_plane_len` returned `Some`, so `i < AV_NUM_DATA_POINTERS`,
        //         `data[i]` is non-null, and `len` is `linesize[i]` (> 0) times the
        //         plane height, i.e. the byte count FFmpeg allocated for this plane.
        //         The slice borrows `self` for its lifetime.
        unsafe {
            let data = (*self.ptr.as_ptr()).data[i];
            Some(std::slice::from_raw_parts(data, len))
        }
    }

    /// Returns a mutable view of video plane `i`, sized to the plane's own
    /// `linesize[i] * plane_height(i)` bytes.
    ///
    /// Returns `None` under the same conditions as [`video_plane`](Self::video_plane).
    pub fn video_plane_mut(&mut self, i: usize) -> Option<&mut [u8]> {
        let len = self.video_plane_len(i)?;
        // SAFETY: as in `video_plane`; `&mut self` guarantees exclusive access, so
        //         the returned mutable slice is unique for its lifetime.
        unsafe {
            let data = (*self.ptr.as_ptr()).data[i];
            Some(std::slice::from_raw_parts_mut(data, len))
        }
    }

    /// Returns an immutable view of audio plane `i`, sized to the samples it
    /// holds (one plane per channel for planar formats, a single interleaved
    /// plane 0 for packed formats).
    ///
    /// Returns `None` when the plane is absent or cannot be sized: `i` is out of
    /// range, `data[i]` is null, or the sample format is unusable.
    #[must_use]
    pub fn audio_plane(&self, i: usize) -> Option<&[u8]> {
        let len = self.audio_plane_len(i)?;
        // SAFETY: `audio_plane_len` returned `Some`, so `i` is in range, `data[i]`
        //         is non-null, and `len` is the byte count for this plane derived
        //         from `nb_samples`, the channel count, and the sample size. The
        //         slice borrows `self` for its lifetime.
        unsafe {
            let data = (*self.ptr.as_ptr()).data[i];
            Some(std::slice::from_raw_parts(data, len))
        }
    }

    /// Returns a mutable view of audio plane `i`, sized as in
    /// [`audio_plane`](Self::audio_plane).
    ///
    /// Returns `None` under the same conditions as [`audio_plane`](Self::audio_plane).
    pub fn audio_plane_mut(&mut self, i: usize) -> Option<&mut [u8]> {
        let len = self.audio_plane_len(i)?;
        // SAFETY: as in `audio_plane`; `&mut self` guarantees exclusive access, so
        //         the returned mutable slice is unique for its lifetime.
        unsafe {
            let data = (*self.ptr.as_ptr()).data[i];
            Some(std::slice::from_raw_parts_mut(data, len))
        }
    }

    /// Computes the byte length of video plane `i`, or `None` if the plane is
    /// absent / cannot be sized. Shared by the video plane accessors.
    fn video_plane_len(&self, i: usize) -> Option<usize> {
        if i >= AV_NUM_DATA_POINTERS as usize {
            return None;
        }
        // SAFETY: `self.ptr` is a valid owned frame; `data`, `linesize`, `format`,
        //         and `height` are plain fields.
        let (data, linesize, format, height) = unsafe {
            let p = self.ptr.as_ptr();
            ((*p).data[i], (*p).linesize[i], (*p).format, (*p).height)
        };
        // RK-008: a non-positive linesize would make the byte count wrap; guard it
        // (a get_buffer'd encoder frame always has a positive linesize). A
        // non-positive `height` would likewise wrap `plane_h as usize`, so guard it
        // too (symmetric with the audio accessor's `nb_samples` / channel guards).
        if data.is_null() || linesize <= 0 || height <= 0 {
            return None;
        }
        let plane_h = plane_height(format, height, i)?;
        Some(linesize as usize * plane_h as usize)
    }

    /// Computes the byte length of audio plane `i`, or `None` if the plane is
    /// absent / cannot be sized. Shared by the audio plane accessors.
    fn audio_plane_len(&self, i: usize) -> Option<usize> {
        if i >= AV_NUM_DATA_POINTERS as usize {
            return None;
        }
        // SAFETY: `self.ptr` is a valid owned frame; `data`, `format`,
        //         `nb_samples`, and `ch_layout.nb_channels` are plain fields.
        let (data, format, nb_samples, channels) = unsafe {
            let p = self.ptr.as_ptr();
            (
                (*p).data[i],
                (*p).format,
                (*p).nb_samples,
                (*p).ch_layout.nb_channels,
            )
        };
        if data.is_null() {
            return None;
        }
        let bytes = crate::swresample::sample_format::bytes_per_sample(format);
        if bytes <= 0 || nb_samples < 0 || channels <= 0 {
            return None;
        }
        let bytes = bytes as usize;
        let nb_samples = nb_samples as usize;
        if crate::swresample::sample_format::is_planar(format) {
            // Planar: one plane per channel.
            if i >= channels as usize {
                return None;
            }
            Some(nb_samples * bytes)
        } else {
            // Packed: a single interleaved plane 0.
            if i != 0 {
                return None;
            }
            Some(nb_samples * channels as usize * bytes)
        }
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

/// Returns the pixel height of video plane `plane` for `format`, or `None` if
/// the format has no pixel descriptor.
///
/// Plane 0 spans the full frame height; subsequent (chroma) planes are
/// subsampled vertically by `log2_chroma_h` from the format's descriptor.
fn plane_height(format: c_int, height: c_int, plane: usize) -> Option<c_int> {
    // SAFETY: `av_pix_fmt_desc_get` takes the format by value and returns a
    //         pointer into FFmpeg's static descriptor table (or null for an
    //         unknown format); the pointee is read only while valid here.
    let desc = unsafe { ffi_av_pix_fmt_desc_get(format) };
    if desc.is_null() {
        return None;
    }
    if plane == 0 {
        return Some(height);
    }
    // SAFETY: `desc` is non-null (checked); `log2_chroma_h` is a plain field.
    let log2_chroma_h = unsafe { (*desc).log2_chroma_h };
    let round = (1_i32 << log2_chroma_h) - 1;
    Some((height + round) >> log2_chroma_h)
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
    fn set_pict_type_should_round_trip() {
        let mut frame = Frame::new().expect("frame allocation should succeed");
        frame.set_pict_type(crate::AVPictureType_AV_PICTURE_TYPE_I);
        // SAFETY: `frame` is a valid owned frame; `pict_type` is a plain field.
        assert_eq!(
            unsafe { (*frame.as_ptr()).pict_type },
            crate::AVPictureType_AV_PICTURE_TYPE_I
        );
    }

    #[test]
    fn set_sample_rate_and_nb_samples_should_round_trip() {
        let mut frame = Frame::new().expect("frame allocation should succeed");
        frame.set_sample_rate(48_000);
        frame.set_nb_samples(1024);
        assert_eq!(frame.sample_rate(), 48_000);
        assert_eq!(frame.nb_samples(), 1024);
    }

    #[test]
    fn set_ch_layout_should_copy_the_channel_count() {
        let mut frame = Frame::new().expect("frame allocation should succeed");
        let layout = crate::swresample::channel_layout::with_channels(2);
        frame
            .set_ch_layout(&layout)
            .expect("copying a standard stereo layout should succeed");
        assert_eq!(frame.channels(), 2);
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

    #[test]
    fn video_plane_mut_should_round_trip_through_video_plane() {
        // A get_buffer'd RGB24 frame has a single packed plane whose slice length
        // is `linesize[0] * height`; a pattern written via the mut accessor reads
        // back identically through the shared accessor.
        let mut frame = Frame::new().expect("frame allocation should succeed");
        frame.set_format(crate::AVPixelFormat_AV_PIX_FMT_RGB24);
        frame.set_width(16);
        frame.set_height(16);
        frame.get_buffer(0).expect("buffer alloc should succeed");

        // SAFETY: `frame` is a valid get_buffer'd frame; `linesize[0]` is a field.
        let expected_len = unsafe { (*frame.as_ptr()).linesize[0] as usize } * 16;

        {
            let plane = frame
                .video_plane_mut(0)
                .expect("plane 0 exists on a get_buffer'd RGB24 frame");
            assert_eq!(plane.len(), expected_len, "len == linesize[0] * height");
            for (i, b) in plane.iter_mut().enumerate() {
                *b = (i % 251) as u8;
            }
        }

        let plane = frame
            .video_plane(0)
            .expect("plane 0 exists on a get_buffer'd RGB24 frame");
        assert_eq!(plane.len(), expected_len);
        assert!(
            plane.iter().enumerate().all(|(i, &b)| b == (i % 251) as u8),
            "written pattern reads back unchanged"
        );
    }

    #[test]
    fn video_plane_should_size_chroma_planes_by_subsampling() {
        // YUV420P chroma planes (1, 2) are vertically subsampled by 2, so their
        // slice length is `linesize[i] * (height + 1) / 2`. Cover an even and an
        // odd height to exercise the `(height + round) >> log2_chroma_h` rounding.
        for height in [16i32, 17i32] {
            let mut frame = Frame::new().expect("frame allocation should succeed");
            frame.set_format(crate::AVPixelFormat_AV_PIX_FMT_YUV420P);
            frame.set_width(16);
            frame.set_height(height);
            frame.get_buffer(0).expect("buffer alloc should succeed");

            // SAFETY: `frame` is a valid get_buffer'd YUV420P frame; linesize is a field.
            let (ls0, ls1) = unsafe {
                let p = frame.as_ptr();
                ((*p).linesize[0], (*p).linesize[1])
            };
            let chroma_h = (height + 1) / 2;

            assert_eq!(
                frame.video_plane(0).map(<[u8]>::len),
                Some(ls0 as usize * height as usize),
                "luma plane = linesize[0] * height (height={height})"
            );
            assert_eq!(
                frame.video_plane(1).map(<[u8]>::len),
                Some(ls1 as usize * chroma_h as usize),
                "chroma plane = linesize[1] * (height+1)/2 (height={height})"
            );
        }
    }

    #[test]
    fn video_plane_should_return_none_on_negative_linesize() {
        // RK-008 guard: a non-positive linesize cannot size a slice, so the
        // accessor must refuse it rather than compute a wrapped length.
        let mut frame = Frame::new().expect("frame allocation should succeed");
        frame.set_format(crate::AVPixelFormat_AV_PIX_FMT_RGB24);
        frame.set_width(16);
        frame.set_height(16);
        frame.get_buffer(0).expect("buffer alloc should succeed");

        // SAFETY: `frame` is a valid owned frame; force `linesize[0]` negative.
        unsafe {
            (*frame.as_mut_ptr()).linesize[0] = -1;
        }
        assert!(
            frame.video_plane(0).is_none(),
            "a negative linesize must yield None"
        );
    }

    #[test]
    fn audio_plane_len_should_match_planar_and_packed_layout() {
        use crate::swresample::{channel_layout, sample_format};

        let samples: c_int = 100;

        // Planar S16P stereo: one plane per channel, each `samples * 2` bytes.
        let mut planar = Frame::new().expect("frame allocation should succeed");
        // SAFETY: set the audio fields on a fresh frame, then allocate its buffer.
        unsafe {
            let p = planar.as_mut_ptr();
            (*p).format = sample_format::S16P;
            (*p).nb_samples = samples;
            (*p).sample_rate = 48000;
            channel_layout::set_default(&raw mut (*p).ch_layout, 2);
        }
        planar.get_buffer(0).expect("planar buffer alloc");
        assert_eq!(
            planar.audio_plane(0).map(<[u8]>::len),
            Some(samples as usize * 2)
        );
        assert_eq!(
            planar.audio_plane(1).map(<[u8]>::len),
            Some(samples as usize * 2)
        );
        assert!(
            planar.audio_plane(2).is_none(),
            "planar stereo has no third channel plane"
        );

        // Packed S16 stereo: a single interleaved plane 0 of `samples * 2ch * 2`.
        let mut packed = Frame::new().expect("frame allocation should succeed");
        // SAFETY: set the audio fields on a fresh frame, then allocate its buffer.
        unsafe {
            let p = packed.as_mut_ptr();
            (*p).format = sample_format::S16;
            (*p).nb_samples = samples;
            (*p).sample_rate = 48000;
            channel_layout::set_default(&raw mut (*p).ch_layout, 2);
        }
        packed.get_buffer(0).expect("packed buffer alloc");
        assert_eq!(
            packed.audio_plane(0).map(<[u8]>::len),
            Some(samples as usize * 2 * 2)
        );
        assert!(
            packed.audio_plane(1).is_none(),
            "packed audio exposes only plane 0"
        );
    }
}
