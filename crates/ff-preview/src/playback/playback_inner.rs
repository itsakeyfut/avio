//! Unsafe `FFmpeg` calls for the playback subsystem.
//!
//! This module is the only place in `ff-preview` where `unsafe` code is
//! permitted. All `unsafe` blocks must carry a `// SAFETY:` comment explaining
//! why the invariants hold.

#![allow(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]

use ff_format::{AudioFrame, PixelFormat, VideoFrame};

/// Extract interleaved `f32` PCM samples from a decoded [`AudioFrame`].
///
/// The caller must have configured the decoder for [`ff_format::SampleFormat::F32`]
/// (packed interleaved). Resampling to the target sample rate and channel count
/// is handled by [`ff_decode::AudioDecoder`] via `swr_convert` internally; this
/// function only copies the already-converted sample bytes into a `Vec<f32>`.
///
/// Returns an empty `Vec` when the frame is not in packed `F32` format (should
/// not occur when the decoder is configured with `SampleFormat::F32`).
pub(crate) fn audio_frame_to_f32(frame: &AudioFrame) -> Vec<f32> {
    frame.as_f32().map(<[f32]>::to_vec).unwrap_or_default()
}

// SwsRgbaConverter

/// Lazy `sws_scale` converter that outputs packed RGBA (4 bytes/pixel, alpha = 255).
///
/// The `SwsContext` is created on the first call to [`convert`](Self::convert) and
/// reused for subsequent frames with the same dimensions and source pixel format.
/// A new context is allocated automatically when the frame geometry changes
/// (uncommon in practice for a single file).
///
/// [`convert_to`](Self::convert_to) accepts explicit output dimensions so that overlay
/// layers can be scaled to match the primary video track's canvas size before compositing.
pub(crate) struct SwsRgbaConverter {
    /// `None` before the first `convert` call or after a geometry change.
    /// The owned [`ff_sys::ScaleContext`] frees the underlying `SwsContext` on drop.
    ctx: Option<ff_sys::ScaleContext>,
    /// Cached `(src_w, src_h, format, dst_w, dst_h)` so geometry changes can be detected.
    cache_key: Option<(u32, u32, PixelFormat, u32, u32)>,
}

impl SwsRgbaConverter {
    pub(crate) fn new() -> Self {
        Self {
            ctx: None,
            cache_key: None,
        }
    }

    /// Convert `frame` to packed RGBA at its native resolution and write into `dst`.
    ///
    /// Returns `true` on success; `false` when the frame dimensions are zero or
    /// when `sws_getContext` / `sws_scale` fails (failures are logged as `warn`).
    ///
    /// `dst` is resized to `width * height * 4` bytes before writing.
    pub(crate) fn convert(&mut self, frame: &VideoFrame, dst: &mut Vec<u8>) -> bool {
        self.convert_to(frame, dst, frame.width(), frame.height())
    }

    /// Convert `frame` to packed RGBA scaled to `(dst_w, dst_h)` and write into `dst`.
    ///
    /// When `dst_w == frame.width()` and `dst_h == frame.height()` this is identical to
    /// [`convert`](Self::convert). When the dimensions differ, `libswscale` performs
    /// bilinear rescaling so the output always matches the requested canvas size.
    ///
    /// Rescales a frame to an explicit target size (used where a fixed canvas
    /// resolution is required).
    pub(crate) fn convert_to(
        &mut self,
        frame: &VideoFrame,
        dst: &mut Vec<u8>,
        dst_w: u32,
        dst_h: u32,
    ) -> bool {
        let src_w = frame.width();
        let src_h = frame.height();
        if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
            return false;
        }
        let fmt = frame.format();
        let key = (src_w, src_h, fmt, dst_w, dst_h);

        // Re-create the context when geometry, format, or output size changes.
        if self.cache_key.as_ref() != Some(&key) {
            let src_fmt = pixel_format_to_av(fmt);
            let dst_fmt = ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA;
            // The old context (if any) drops on reassignment.
            // Dimensions are > 0 (checked above); formats are valid AV constants.
            match ff_sys::ScaleContext::new(
                src_w as i32,
                src_h as i32,
                src_fmt,
                dst_w as i32,
                dst_h as i32,
                dst_fmt,
                ff_sys::swscale::scale_flags::FAST_BILINEAR,
            ) {
                Ok(ctx) => self.ctx = Some(ctx),
                Err(e) => {
                    log::warn!(
                        "sws_getContext failed format={fmt:?} src={src_w}x{src_h} \
                         dst={dst_w}x{dst_h} code={code}",
                        code = e.code()
                    );
                    return false;
                }
            }
            self.cache_key = Some(key);
        }

        let rgba_stride = (dst_w * 4) as usize;
        let total = rgba_stride * dst_h as usize;
        dst.resize(total, 0u8);

        // Collect per-plane borrowed slices and strides from the VideoFrame.
        // VideoFrame stores at most 4 planes.
        let n = frame.num_planes().min(4);
        let mut src_planes: Vec<&[u8]> = Vec::with_capacity(n);
        let mut src_strides: Vec<i32> = Vec::with_capacity(n);
        for i in 0..n {
            if let (Some(plane), Some(stride)) = (frame.plane(i), frame.stride(i)) {
                src_planes.push(plane);
                src_strides.push(stride as i32);
            }
        }

        let dst_strides: [i32; 1] = [rgba_stride as i32];
        let mut dst_slices: [&mut [u8]; 1] = [dst.as_mut_slice()];

        let Some(ctx) = self.ctx.as_mut() else {
            log::warn!("sws_scale skipped: scaling context not initialized");
            return false;
        };
        // SAFETY: ctx is initialized (created above); each src plane is sized for
        // `src_h` rows at its stride, and the single RGBA dst plane is sized
        // `dst_w * dst_h * 4` bytes (resized above) at `rgba_stride`.
        let result = unsafe {
            ctx.scale_slices(
                &src_planes,
                &src_strides,
                src_h as i32,
                &mut dst_slices,
                &dst_strides,
            )
        };
        match result {
            Ok(_) => true,
            Err(e) => {
                log::warn!(
                    "sws_scale failed src={src_w}x{src_h} dst={dst_w}x{dst_h} code={}",
                    e.code()
                );
                false
            }
        }
    }
}

/// Map a [`PixelFormat`] to its `AVPixelFormat` counterpart.
///
/// Mirrors the mapping in `ff-decode`'s `pixel_format_to_av`; duplicated here
/// because that function is `pub(super)` and inaccessible from this crate.
fn pixel_format_to_av(format: PixelFormat) -> ff_sys::AVPixelFormat {
    match format {
        PixelFormat::Yuv420p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P,
        PixelFormat::Yuv422p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P,
        PixelFormat::Yuv444p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P,
        PixelFormat::Rgb24 => ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24,
        PixelFormat::Bgr24 => ff_sys::AVPixelFormat_AV_PIX_FMT_BGR24,
        PixelFormat::Rgba => ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA,
        PixelFormat::Bgra => ff_sys::AVPixelFormat_AV_PIX_FMT_BGRA,
        PixelFormat::Gray8 => ff_sys::AVPixelFormat_AV_PIX_FMT_GRAY8,
        PixelFormat::Nv12 => ff_sys::AVPixelFormat_AV_PIX_FMT_NV12,
        PixelFormat::Nv21 => ff_sys::AVPixelFormat_AV_PIX_FMT_NV21,
        PixelFormat::Yuv420p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE,
        PixelFormat::Yuv422p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P10LE,
        PixelFormat::Yuv444p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P10LE,
        PixelFormat::Yuva444p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUVA444P10LE,
        PixelFormat::P010le => ff_sys::AVPixelFormat_AV_PIX_FMT_P010LE,
        PixelFormat::Gbrpf32le => ff_sys::AVPixelFormat_AV_PIX_FMT_GBRPF32LE,
        _ => {
            log::warn!(
                "pixel_format has no AV mapping, falling back to Yuv420p \
                 format={format:?} fallback=AV_PIX_FMT_YUV420P"
            );
            ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff_format::{AudioFrame, SampleFormat, Timestamp};

    #[test]
    fn audio_frame_to_f32_should_extract_packed_f32_samples() {
        // Build a 2-sample stereo F32 frame (4 values: L0, R0, L1, R1).
        let values: Vec<f32> = vec![1.0, -1.0, 0.5, -0.5];
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_ne_bytes()).collect();
        let frame = AudioFrame::new(
            vec![bytes],
            2, // 2 samples per channel
            2, // stereo
            48_000,
            SampleFormat::F32,
            Timestamp::default(),
        )
        .unwrap();

        let out = audio_frame_to_f32(&frame);

        assert_eq!(out.len(), 4);
        assert!(
            (out[0] - 1.0).abs() < f32::EPSILON,
            "first sample mismatch: expected 1.0 got {}",
            out[0]
        );
        assert!(
            (out[1] - (-1.0)).abs() < f32::EPSILON,
            "second sample mismatch: expected -1.0 got {}",
            out[1]
        );
        assert!(
            (out[2] - 0.5).abs() < f32::EPSILON,
            "third sample mismatch: expected 0.5 got {}",
            out[2]
        );
        assert!(
            (out[3] - (-0.5)).abs() < f32::EPSILON,
            "fourth sample mismatch: expected -0.5 got {}",
            out[3]
        );
    }

    #[test]
    fn audio_frame_to_f32_should_return_empty_for_non_f32_format() {
        // I16 format: 2 samples × 2 channels × 2 bytes/sample = 8 bytes in one packed plane.
        let bytes = vec![0u8; 8];
        let frame = AudioFrame::new(
            vec![bytes],
            2,
            2,
            48_000,
            SampleFormat::I16,
            Timestamp::default(),
        )
        .unwrap();

        let out = audio_frame_to_f32(&frame);
        assert!(
            out.is_empty(),
            "non-F32 frame should return an empty Vec, got {} samples",
            out.len()
        );
    }

    #[test]
    fn convert_to_should_rebuild_scaler_when_source_geometry_changes() {
        // Drive the cache-rebuild move-assign path: a second convert with a
        // different source geometry replaces the cached `ScaleContext`, dropping
        // the old one exactly once. Pure libswscale (no filters), so this runs in
        // CI. RGBA in / RGBA out keeps the conversion format-agnostic.
        let mut converter = SwsRgbaConverter::new();
        let mut dst = Vec::new();

        // First geometry: 8x8 -> 16x16 builds the initial context.
        let frame_a = VideoFrame::from_rgba(8, 8, vec![120u8; 8 * 8 * 4]).unwrap();
        assert!(converter.convert_to(&frame_a, &mut dst, 16, 16));
        assert_eq!(dst.len(), 16 * 16 * 4);

        // Second geometry: 4x4 -> 16x16 changes the cache key, so the context is
        // rebuilt (old ScaleContext drops on reassignment).
        let frame_b = VideoFrame::from_rgba(4, 4, vec![200u8; 4 * 4 * 4]).unwrap();
        assert!(converter.convert_to(&frame_b, &mut dst, 16, 16));
        assert_eq!(dst.len(), 16 * 16 * 4);

        // Third convert reuses the cached context (same geometry as the second).
        let frame_c = VideoFrame::from_rgba(4, 4, vec![50u8; 4 * 4 * 4]).unwrap();
        assert!(converter.convert_to(&frame_c, &mut dst, 16, 16));
        assert_eq!(dst.len(), 16 * 16 * 4);
    }
}
