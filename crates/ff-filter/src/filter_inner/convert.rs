//! Format and timestamp conversion helpers.

use std::os::raw::c_int;

use ff_format::time::{Rational, Timestamp};
use ff_format::{AudioFrame, PixelFormat, PooledBuffer, SampleFormat, VideoFrame};
use ff_sys::AVFrame;

use super::{VIDEO_TIME_BASE_DEN, VIDEO_TIME_BASE_NUM};

// ── Timestamp PTS helpers ──────────────────────────────────────────────────────

/// Compute the `AVFrame.pts` ticks for pushing a video frame.
///
/// Scales the timestamp's seconds to the internal video time base (1/90000).
/// Returns [`ff_sys::AV_NOPTS_VALUE`] when the timestamp carries no valid PTS.
pub(super) fn video_pts_ticks(ts: Timestamp) -> i64 {
    if ts.pts() == ff_sys::AV_NOPTS_VALUE {
        ff_sys::AV_NOPTS_VALUE
    } else {
        (ts.as_secs_f64() * f64::from(VIDEO_TIME_BASE_DEN)) as i64
    }
}

/// Convert raw `AVFrame.pts` ticks (1/90000 time base) to a [`Timestamp`].
///
/// Returns [`Timestamp::default`] (0 at 1/90000) when `pts_raw` is
/// [`ff_sys::AV_NOPTS_VALUE`].
pub(super) fn video_ticks_to_timestamp(pts_raw: i64) -> Timestamp {
    if pts_raw == ff_sys::AV_NOPTS_VALUE {
        Timestamp::default()
    } else {
        let secs = pts_raw as f64 / f64::from(VIDEO_TIME_BASE_DEN);
        Timestamp::from_secs_f64(
            secs,
            Rational::new(VIDEO_TIME_BASE_NUM, VIDEO_TIME_BASE_DEN),
        )
    }
}

/// Compute the `AVFrame.pts` ticks for pushing an audio frame.
///
/// Scales the timestamp's seconds to the audio time base (1/`sample_rate`).
/// Returns [`ff_sys::AV_NOPTS_VALUE`] when the timestamp carries no valid PTS.
pub(super) fn audio_pts_ticks(ts: Timestamp, sample_rate: u32) -> i64 {
    if ts.pts() == ff_sys::AV_NOPTS_VALUE {
        ff_sys::AV_NOPTS_VALUE
    } else {
        (ts.as_secs_f64() * f64::from(sample_rate)) as i64
    }
}

/// Convert raw `AVFrame.pts` ticks (1/`sample_rate` time base) to a [`Timestamp`].
///
/// Returns [`Timestamp::zero`] at `1/sample_rate` when `pts_raw` is
/// [`ff_sys::AV_NOPTS_VALUE`].  Falls back to denominator 1 if `sample_rate` is 0.
pub(super) fn audio_ticks_to_timestamp(pts_raw: i64, sample_rate: u32) -> Timestamp {
    let den = if sample_rate > 0 {
        sample_rate as i32
    } else {
        1
    };
    let time_base = Rational::new(1, den);
    if pts_raw == ff_sys::AV_NOPTS_VALUE {
        Timestamp::zero(time_base)
    } else {
        let secs = if sample_rate > 0 {
            pts_raw as f64 / f64::from(sample_rate)
        } else {
            0.0
        };
        Timestamp::from_secs_f64(secs, time_base)
    }
}

// ── Format conversion helpers ─────────────────────────────────────────────────

/// Convert a [`PixelFormat`] to the corresponding `AVPixelFormat` integer.
pub(crate) fn pixel_format_to_av(fmt: PixelFormat) -> c_int {
    match fmt {
        PixelFormat::Yuv420p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P,
        PixelFormat::Rgb24 => ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24,
        PixelFormat::Bgr24 => ff_sys::AVPixelFormat_AV_PIX_FMT_BGR24,
        PixelFormat::Yuv422p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P,
        PixelFormat::Yuv444p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P,
        PixelFormat::Gray8 => ff_sys::AVPixelFormat_AV_PIX_FMT_GRAY8,
        PixelFormat::Nv12 => ff_sys::AVPixelFormat_AV_PIX_FMT_NV12,
        PixelFormat::Nv21 => ff_sys::AVPixelFormat_AV_PIX_FMT_NV21,
        PixelFormat::Rgba => ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA,
        PixelFormat::Bgra => ff_sys::AVPixelFormat_AV_PIX_FMT_BGRA,
        PixelFormat::Yuv420p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE,
        PixelFormat::P010le => ff_sys::AVPixelFormat_AV_PIX_FMT_P010LE,
        PixelFormat::Other(v) => v as c_int,
        // `PixelFormat` is `#[non_exhaustive]`; new variants default to NONE.
        _ => ff_sys::AVPixelFormat_AV_PIX_FMT_NONE,
    }
}

/// Convert an `AVPixelFormat` integer to a [`PixelFormat`].
pub(super) fn av_to_pixel_format(av_fmt: c_int) -> PixelFormat {
    match av_fmt {
        v if v == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P => PixelFormat::Yuv420p,
        v if v == ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24 => PixelFormat::Rgb24,
        v if v == ff_sys::AVPixelFormat_AV_PIX_FMT_BGR24 => PixelFormat::Bgr24,
        v if v == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P => PixelFormat::Yuv422p,
        v if v == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P => PixelFormat::Yuv444p,
        v if v == ff_sys::AVPixelFormat_AV_PIX_FMT_GRAY8 => PixelFormat::Gray8,
        v if v == ff_sys::AVPixelFormat_AV_PIX_FMT_NV12 => PixelFormat::Nv12,
        v if v == ff_sys::AVPixelFormat_AV_PIX_FMT_NV21 => PixelFormat::Nv21,
        v if v == ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA => PixelFormat::Rgba,
        v if v == ff_sys::AVPixelFormat_AV_PIX_FMT_BGRA => PixelFormat::Bgra,
        v if v == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE => PixelFormat::Yuv420p10le,
        v if v == ff_sys::AVPixelFormat_AV_PIX_FMT_P010LE => PixelFormat::P010le,
        other => PixelFormat::Other(other.max(0) as u32),
    }
}

/// Convert a [`SampleFormat`] to the corresponding `AVSampleFormat` integer.
pub(super) fn sample_format_to_av(fmt: SampleFormat) -> c_int {
    match fmt {
        SampleFormat::U8 => ff_sys::AVSampleFormat_AV_SAMPLE_FMT_U8,
        SampleFormat::I16 => ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S16,
        SampleFormat::I32 => ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S32,
        SampleFormat::F32 => ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLT,
        SampleFormat::F64 => ff_sys::AVSampleFormat_AV_SAMPLE_FMT_DBL,
        SampleFormat::U8p => ff_sys::AVSampleFormat_AV_SAMPLE_FMT_U8P,
        SampleFormat::I16p => ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S16P,
        SampleFormat::I32p => ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S32P,
        SampleFormat::F32p => ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLTP,
        SampleFormat::F64p => ff_sys::AVSampleFormat_AV_SAMPLE_FMT_DBLP,
        SampleFormat::Other(v) => v as c_int,
        // `SampleFormat` is `#[non_exhaustive]`; new variants default to FLT.
        _ => ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLT,
    }
}

/// Returns the libavfilter `sample_fmt` string for an `abuffer` args string.
pub(super) fn sample_format_to_av_name(fmt: SampleFormat) -> &'static str {
    match fmt {
        SampleFormat::U8 => "u8",
        SampleFormat::I16 => "s16",
        SampleFormat::I32 => "s32",
        SampleFormat::F32 => "flt",
        SampleFormat::F64 => "dbl",
        SampleFormat::U8p => "u8p",
        SampleFormat::I16p => "s16p",
        SampleFormat::I32p => "s32p",
        SampleFormat::F32p => "fltp",
        SampleFormat::F64p => "dblp",
        SampleFormat::Other(_) => "flt",
        // `SampleFormat` is `#[non_exhaustive]`; new variants default to flt.
        _ => "flt",
    }
}

/// Convert an `AVSampleFormat` integer to a [`SampleFormat`].
pub(super) fn av_to_sample_format(av_fmt: c_int) -> SampleFormat {
    match av_fmt {
        v if v == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_U8 => SampleFormat::U8,
        v if v == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S16 => SampleFormat::I16,
        v if v == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S32 => SampleFormat::I32,
        v if v == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLT => SampleFormat::F32,
        v if v == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_DBL => SampleFormat::F64,
        v if v == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_U8P => SampleFormat::U8p,
        v if v == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S16P => SampleFormat::I16p,
        v if v == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S32P => SampleFormat::I32p,
        v if v == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLTP => SampleFormat::F32p,
        v if v == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_DBLP => SampleFormat::F64p,
        other => SampleFormat::Other(other.max(0) as u32),
    }
}

// ── AVFrame ↔ frame data helpers ──────────────────────────────────────────────

/// Number of pixel rows in the given plane of a video frame.
pub(super) fn plane_height(fmt: PixelFormat, plane: usize, frame_height: usize) -> usize {
    match fmt {
        // YUV 4:2:0 — Y full height, U/V halved.
        PixelFormat::Yuv420p | PixelFormat::Yuv420p10le => {
            if plane == 0 {
                frame_height
            } else {
                frame_height.div_ceil(2)
            }
        }
        // Semi-planar NV12/NV21 / P010le — Y full, UV halved.
        PixelFormat::Nv12 | PixelFormat::Nv21 | PixelFormat::P010le => {
            if plane == 0 {
                frame_height
            } else {
                frame_height.div_ceil(2)
            }
        }
        // Everything else: all planes span the full height.
        _ => frame_height,
    }
}

/// Copy [`VideoFrame`] plane data row-by-row into a pre-allocated `AVFrame`.
///
/// # Safety
///
/// `raw_frame` must point to a valid `AVFrame` whose `data` / `linesize`
/// arrays have been populated by `av_frame_get_buffer`.
pub(super) unsafe fn copy_video_planes_to_av(src: &VideoFrame, dst: *mut AVFrame) {
    for i in 0..src.num_planes().min(8) {
        let Some(plane_data) = src.plane(i) else {
            continue;
        };
        let dst_ptr = (*dst).data[i];
        if dst_ptr.is_null() {
            continue;
        }
        let src_stride = src.strides()[i];
        let dst_stride = (*dst).linesize[i] as usize;
        let rows = plane_height(src.format(), i, src.height() as usize);

        for row in 0..rows {
            let src_off = row * src_stride;
            let dst_off = row * dst_stride;
            let copy_len = src_stride.min(dst_stride);
            if src_off + copy_len <= plane_data.len() {
                // SAFETY: `src_off + copy_len` is within `plane_data`; the dst
                // slice is within the `FFmpeg`-allocated buffer which is at
                // least `linesize[i] * height` bytes per plane.
                std::ptr::copy_nonoverlapping(
                    plane_data.as_ptr().add(src_off),
                    dst_ptr.add(dst_off),
                    copy_len,
                );
            }
        }
    }
}

/// Build a [`VideoFrame`] by copying data out of an `AVFrame`.
///
/// # Safety
///
/// `raw_frame` must point to a valid, populated `AVFrame`.
pub(super) unsafe fn av_frame_to_video_frame(raw_frame: *const AVFrame) -> Result<VideoFrame, ()> {
    let width = (*raw_frame).width as u32;
    let height = (*raw_frame).height as u32;
    let format = av_to_pixel_format((*raw_frame).format);
    let pts_raw = (*raw_frame).pts;
    if pts_raw == ff_sys::AV_NOPTS_VALUE {
        log::warn!("pts invalid in output video frame from filter graph");
    }
    let timestamp = video_ticks_to_timestamp(pts_raw);
    // AV_PICTURE_TYPE_I = 1: I-frame (key frame).  `key_frame` was removed
    // from AVFrame in FFmpeg 6; use `pict_type` instead.
    let key_frame = (*raw_frame).pict_type == 1;

    let num_planes = format.num_planes();
    let mut planes: Vec<PooledBuffer> = Vec::with_capacity(num_planes);
    let mut strides: Vec<usize> = Vec::with_capacity(num_planes);

    for i in 0..num_planes {
        let src_ptr = (*raw_frame).data[i];
        if src_ptr.is_null() {
            return Err(());
        }
        let linesize_raw = (*raw_frame).linesize[i];
        let stride = linesize_raw.unsigned_abs() as usize;
        let rows = plane_height(format, i, height as usize);

        let data = if linesize_raw < 0 {
            // Some filters (e.g. `vflip`) produce frames with a negative linesize to
            // indicate a bottom-up scan order. `data[i]` then points to the last row,
            // and each successive row is at a lower address. Seek back to the first
            // row and copy the whole contiguous block (preserved behaviour).
            //
            // SAFETY: with a negative linesize `data[i]` sits at the start of the
            // *last* row; offsetting by `linesize_raw * (rows - 1)` steps back to the
            // first row so the `stride * rows` slice covers the full plane.
            let data_ptr = src_ptr.offset(linesize_raw as isize * (rows as isize - 1));
            std::slice::from_raw_parts(data_ptr, stride * rows).to_vec()
        } else {
            // Positive linesize: copy only the *valid* bytes of each row, not the
            // full padded `stride`. A `crop` filter returns a view whose `data[i]`
            // is offset into a larger parent allocation while `linesize` stays the
            // parent's; reading `stride * rows` from the offset pointer would run
            // past the parent buffer by the offset and segfault (issue: large crop
            // offset after an up-scale). Reading `row_bytes ≤ stride` per row keeps
            // the last row within bounds. `stride` is preserved so downstream stride
            // math is unchanged; inter-row padding is left zeroed (never read).
            let row_bytes = {
                let n = ff_sys::av_image_get_linesize((*raw_frame).format, width as i32, i as i32);
                if n > 0 {
                    (n as usize).min(stride)
                } else {
                    stride
                }
            };
            let mut buf = vec![0u8; stride * rows];
            for row in 0..rows {
                // SAFETY: source row `row` starts at `src_ptr + row * stride` and has
                // at least `row_bytes` valid bytes; the dst slice is inside `buf`.
                std::ptr::copy_nonoverlapping(
                    src_ptr.add(row * stride),
                    buf.as_mut_ptr().add(row * stride),
                    row_bytes,
                );
            }
            buf
        };

        planes.push(PooledBuffer::standalone(data));
        strides.push(stride);
    }

    VideoFrame::new(planes, strides, width, height, format, timestamp, key_frame).map_err(|_| ())
}

/// Copy [`AudioFrame`] plane data into a pre-allocated `AVFrame`.
///
/// # Safety
///
/// `raw_frame` must point to a valid `AVFrame` whose `data` arrays have been
/// populated by `av_frame_get_buffer`.
pub(super) unsafe fn copy_audio_planes_to_av(src: &AudioFrame, dst: *mut AVFrame) {
    for i in 0..src.num_planes().min(8) {
        let Some(plane_data) = src.plane(i) else {
            continue;
        };
        let dst_ptr = (*dst).data[i];
        if dst_ptr.is_null() {
            continue;
        }
        // SAFETY: `FFmpeg` allocated `dst_ptr` with `av_frame_get_buffer`; it
        // is at least `plane_data.len()` bytes.
        std::ptr::copy_nonoverlapping(plane_data.as_ptr(), dst_ptr, plane_data.len());
    }
}

/// Build an [`AudioFrame`] by copying data out of an `AVFrame`.
///
/// # Safety
///
/// `raw_frame` must point to a valid, populated `AVFrame`.
pub(super) unsafe fn av_frame_to_audio_frame(raw_frame: *const AVFrame) -> Result<AudioFrame, ()> {
    let samples = (*raw_frame).nb_samples as usize;
    let channels = (*raw_frame).ch_layout.nb_channels as u32;
    let sample_rate = (*raw_frame).sample_rate as u32;
    let format = av_to_sample_format((*raw_frame).format);
    let pts_raw = (*raw_frame).pts;
    if pts_raw == ff_sys::AV_NOPTS_VALUE {
        log::warn!("pts invalid in output audio frame from filter graph sample_rate={sample_rate}");
    }
    let timestamp = audio_ticks_to_timestamp(pts_raw, sample_rate);

    let num_planes = if format.is_planar() {
        channels as usize
    } else {
        1
    };
    let bytes_per_sample = format.bytes_per_sample();
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(num_planes);

    for i in 0..num_planes {
        let src_ptr = (*raw_frame).data[i];
        if src_ptr.is_null() {
            return Err(());
        }
        let byte_count = samples * bytes_per_sample;
        // SAFETY: `av_buffersink_get_frame` guarantees at least
        // `nb_samples * bytes_per_sample` bytes per plane pointer.
        let data = std::slice::from_raw_parts(src_ptr, byte_count).to_vec();
        planes.push(data);
    }

    AudioFrame::new(planes, samples, channels, sample_rate, format, timestamp).map_err(|_| ())
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ff_format::time::Rational;

    // ── PTS helpers ───────────────────────────────────────────────────────────

    /// A valid 1-second video timestamp must scale to exactly 90 000 ticks
    /// in the 1/90000 time base used by the video buffersrc.
    #[test]
    fn video_pts_ticks_should_scale_timestamp_to_90000_time_base() {
        let ts = Timestamp::new(90000, Rational::new(1, 90000));
        assert_eq!(video_pts_ticks(ts), 90000);
    }

    /// A timestamp whose raw PTS equals `AV_NOPTS_VALUE` must pass through
    /// unchanged so FFmpeg knows the frame has no valid presentation time.
    #[test]
    fn video_pts_ticks_with_nopts_value_should_return_av_nopts_value() {
        let ts = Timestamp::new(ff_sys::AV_NOPTS_VALUE, Rational::new(1, 90000));
        assert_eq!(video_pts_ticks(ts), ff_sys::AV_NOPTS_VALUE);
    }

    /// 90 000 ticks at 1/90000 must convert back to ~1.0 second.
    #[test]
    fn video_ticks_to_timestamp_should_convert_ticks_to_secs() {
        let ts = video_ticks_to_timestamp(90000);
        assert!(
            (ts.as_secs_f64() - 1.0).abs() < 1e-6,
            "expected ~1.0 s, got {}",
            ts.as_secs_f64()
        );
    }

    /// `AV_NOPTS_VALUE` ticks must yield `Timestamp::default()` (0 at 1/90000).
    #[test]
    fn video_ticks_to_timestamp_with_nopts_should_return_default_timestamp() {
        let ts = video_ticks_to_timestamp(ff_sys::AV_NOPTS_VALUE);
        assert_eq!(ts, Timestamp::default());
    }

    /// A valid 1-second audio timestamp at 48 kHz must scale to exactly 48 000 ticks.
    #[test]
    fn audio_pts_ticks_should_scale_timestamp_to_sample_rate_time_base() {
        let ts = Timestamp::new(48000, Rational::new(1, 48000));
        assert_eq!(audio_pts_ticks(ts, 48000), 48000);
    }

    /// A timestamp whose raw PTS equals `AV_NOPTS_VALUE` must pass through
    /// unchanged on the audio push path.
    #[test]
    fn audio_pts_ticks_with_nopts_value_should_return_av_nopts_value() {
        let ts = Timestamp::new(ff_sys::AV_NOPTS_VALUE, Rational::new(1, 48000));
        assert_eq!(audio_pts_ticks(ts, 48000), ff_sys::AV_NOPTS_VALUE);
    }

    /// 48 000 ticks at 48 kHz must convert back to ~1.0 second.
    #[test]
    fn audio_ticks_to_timestamp_should_convert_ticks_to_secs() {
        let ts = audio_ticks_to_timestamp(48000, 48000);
        assert!(
            (ts.as_secs_f64() - 1.0).abs() < 1e-6,
            "expected ~1.0 s, got {}",
            ts.as_secs_f64()
        );
    }

    /// `AV_NOPTS_VALUE` ticks must yield a zero timestamp with the correct
    /// audio time base (1/sample_rate).
    #[test]
    fn audio_ticks_to_timestamp_with_nopts_should_return_zero_timestamp() {
        let ts = audio_ticks_to_timestamp(ff_sys::AV_NOPTS_VALUE, 48000);
        assert_eq!(ts.pts(), 0);
        assert_eq!(ts.time_base(), Rational::new(1, 48000));
    }
}
