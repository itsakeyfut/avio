//! Resampling and audio format conversion.
//!
//! This module handles `SwrContext` setup, `swr_convert` calls, and
//! conversion of FFmpeg `AVFrame` data into the `AudioFrame` type used
//! throughout the public API.
//!
//! All functions that touch FFmpeg pointers are `unsafe`. The primary
//! entry point for callers is [`convert_frame_to_audio_frame`].

#![allow(unsafe_code)]
#![allow(clippy::similar_names)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::unnecessary_cast)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::if_not_else)]

use std::ptr;

use ff_format::time::{Rational, Timestamp};
use ff_format::{AudioFrame, SampleFormat};
use ff_sys::{AVFormatContext, AVFrame, AVSampleFormat, SwrContext};

use crate::error::DecodeError;

// ── Overflow / bounds guards (issue #1175) ───────────────────────────────────

/// Rejects a negative `AVFrame::nb_samples` before casting it to `usize`.
///
/// A malformed or corrupt audio stream can make FFmpeg produce a frame with
/// `nb_samples < 0`; the bare `as usize` cast would wrap it to a near-`usize::MAX`
/// count, leading to an out-of-memory allocation or an invalid slice.
fn checked_nb_samples(nb_samples: i32) -> Result<usize, DecodeError> {
    if nb_samples < 0 {
        return Err(DecodeError::Ffmpeg {
            code: 0,
            message: format!("invalid nb_samples={nb_samples}"),
        });
    }
    Ok(nb_samples as usize)
}

/// Computes `samples * bytes_per_sample * channels` as a byte count, returning an
/// error on overflow instead of wrapping.
///
/// Without this guard a very large sample count silently overflows the product,
/// producing an undersized `vec![0u8; …]` that `swr_convert` then writes past —
/// a heap overflow. `usize::checked_mul` catches the realistic 32-bit overflow as
/// well as the (unreachable) 64-bit one.
fn checked_buffer_size(
    samples: usize,
    bytes_per_sample: usize,
    channels: usize,
) -> Result<usize, DecodeError> {
    samples
        .checked_mul(bytes_per_sample)
        .and_then(|n| n.checked_mul(channels))
        .ok_or_else(|| DecodeError::Ffmpeg {
            code: 0,
            message: format!(
                "audio buffer size overflow: samples={samples} bytes_per_sample={bytes_per_sample} channels={channels}"
            ),
        })
}

// ── SwrContext RAII guard ─────────────────────────────────────────────────────

/// RAII guard for `SwrContext` to ensure proper cleanup.
pub(crate) struct SwrContextGuard(pub(crate) *mut SwrContext);

/// Cache key that identifies a unique (src → dst) resampling configuration.
/// Stored alongside `SwrContextGuard` so the context can be reused across
/// frames without reinitialising the FIR filter state on every call.
/// (src_format, src_rate, src_channels, dst_format, dst_rate, dst_channels)
pub(crate) type SwrKey = (i32, u32, u32, i32, u32, u32);

impl Drop for SwrContextGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: self.0 is valid and owned by this guard
            unsafe {
                ff_sys::swr_free(&mut (self.0 as *mut _));
            }
        }
    }
}

// ── Format conversion helpers ─────────────────────────────────────────────────

/// Converts FFmpeg sample format to our `SampleFormat` enum.
pub(crate) fn convert_sample_format(fmt: AVSampleFormat) -> SampleFormat {
    if fmt == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_U8 {
        SampleFormat::U8
    } else if fmt == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S16 {
        SampleFormat::I16
    } else if fmt == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S32 {
        SampleFormat::I32
    } else if fmt == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLT {
        SampleFormat::F32
    } else if fmt == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_DBL {
        SampleFormat::F64
    } else if fmt == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_U8P {
        SampleFormat::U8p
    } else if fmt == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S16P {
        SampleFormat::I16p
    } else if fmt == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S32P {
        SampleFormat::I32p
    } else if fmt == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLTP {
        SampleFormat::F32p
    } else if fmt == ff_sys::AVSampleFormat_AV_SAMPLE_FMT_DBLP {
        SampleFormat::F64p
    } else {
        log::warn!("sample_format unsupported, falling back to F32 requested={fmt} fallback=F32");
        SampleFormat::F32
    }
}

/// Converts our `SampleFormat` to FFmpeg `AVSampleFormat`.
pub(crate) fn sample_format_to_av(format: SampleFormat) -> AVSampleFormat {
    match format {
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
        _ => {
            log::warn!(
                "sample_format has no AV mapping, falling back to F32 \
                 format={format:?} fallback=AV_SAMPLE_FMT_FLT"
            );
            ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLT
        }
    }
}

// ── Channel layout helper ─────────────────────────────────────────────────────

/// Creates a default `AVChannelLayout` for the given channel count.
///
/// # Safety
///
/// The returned layout must be freed with `av_channel_layout_uninit`.
unsafe fn create_channel_layout(channels: u32) -> ff_sys::AVChannelLayout {
    // SAFETY: Zeroing AVChannelLayout is safe as a starting state
    let mut layout = unsafe { std::mem::zeroed::<ff_sys::AVChannelLayout>() };
    // SAFETY: Caller is responsible for freeing with av_channel_layout_uninit
    unsafe {
        ff_sys::av_channel_layout_default(&raw mut layout, channels as i32);
    }
    layout
}

// ── Frame-to-AudioFrame conversion ───────────────────────────────────────────

/// Extracts raw sample bytes from an `AVFrame` into per-channel plane buffers.
///
/// # Safety
///
/// Caller must ensure `frame` is valid and `format` matches the actual frame format.
pub(crate) unsafe fn extract_planes(
    frame: *const AVFrame,
    nb_samples: usize,
    channels: u32,
    format: SampleFormat,
) -> Result<Vec<Vec<u8>>, DecodeError> {
    // SAFETY: Caller ensures frame is valid and format matches actual frame format
    unsafe {
        let mut planes = Vec::new();
        let bytes_per_sample = format.bytes_per_sample();

        if format.is_planar() {
            // Planar: one plane per channel
            for ch in 0..channels as usize {
                let plane_size = checked_buffer_size(nb_samples, bytes_per_sample, 1)?;
                let mut plane_data = vec![0u8; plane_size];

                let src_ptr = (*frame).data[ch];
                std::ptr::copy_nonoverlapping(src_ptr, plane_data.as_mut_ptr(), plane_size);

                planes.push(plane_data);
            }
        } else {
            // Packed: single plane with interleaved samples
            let plane_size = checked_buffer_size(nb_samples, bytes_per_sample, channels as usize)?;
            let mut plane_data = vec![0u8; plane_size];

            let src_ptr = (*frame).data[0];
            std::ptr::copy_nonoverlapping(src_ptr, plane_data.as_mut_ptr(), plane_size);

            planes.push(plane_data);
        }

        Ok(planes)
    }
}

/// Converts an `AVFrame` to an `AudioFrame` without any resampling or format
/// conversion.
///
/// # Safety
///
/// Caller must ensure `frame` and `format_ctx` are valid, and `stream_index`
/// is a valid index into `format_ctx`'s stream list.
pub(crate) unsafe fn av_frame_to_audio_frame(
    frame: *const AVFrame,
    format_ctx: *mut AVFormatContext,
    stream_index: i32,
) -> Result<AudioFrame, DecodeError> {
    // SAFETY: Caller ensures frame and format_ctx are valid
    unsafe {
        let nb_samples = checked_nb_samples((*frame).nb_samples)?;
        let channels = (*frame).ch_layout.nb_channels as u32;
        let sample_rate = (*frame).sample_rate as u32;
        let format = convert_sample_format((*frame).format);

        // Extract timestamp
        let pts = (*frame).pts;
        let timestamp = if pts != ff_sys::AV_NOPTS_VALUE {
            let stream = (*format_ctx).streams.add(stream_index as usize);
            let time_base = (*(*stream)).time_base;
            Timestamp::new(
                pts as i64,
                Rational::new(time_base.num as i32, time_base.den as i32),
            )
        } else {
            Timestamp::invalid()
        };

        // Convert frame to planes
        let planes = extract_planes(frame, nb_samples, channels, format)?;

        AudioFrame::new(planes, nb_samples, channels, sample_rate, format, timestamp).map_err(|e| {
            DecodeError::Ffmpeg {
                code: 0,
                message: format!("Failed to create AudioFrame: {e}"),
            }
        })
    }
}

/// Converts an `AVFrame` to an `AudioFrame`, applying sample format / sample
/// rate / channel count conversion via SwResample when the output parameters
/// differ from the decoded source.
///
/// `swr_cache` and `swr_key` are owned by the caller (`AudioDecoderInner`) and
/// persist across frames.  The `SwrContext` is only rebuilt when the source or
/// target parameters change, which preserves the FIR filter delay buffer across
/// frame boundaries and prevents inter-frame discontinuities.
///
/// # Safety
///
/// Caller must ensure `frame` and `format_ctx` are valid.
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn convert_frame_to_audio_frame(
    frame: *mut AVFrame,
    format_ctx: *mut AVFormatContext,
    stream_index: i32,
    output_format: Option<SampleFormat>,
    output_sample_rate: Option<u32>,
    output_channels: Option<u32>,
    swr_cache: &mut Option<SwrContextGuard>,
    swr_key: &mut Option<SwrKey>,
) -> Result<AudioFrame, DecodeError> {
    // SAFETY: Caller ensures frame is valid
    unsafe {
        let nb_samples = checked_nb_samples((*frame).nb_samples)?;
        let channels = (*frame).ch_layout.nb_channels as u32;
        let sample_rate = (*frame).sample_rate as u32;
        let src_format = (*frame).format;

        let needs_conversion =
            output_format.is_some() || output_sample_rate.is_some() || output_channels.is_some();

        if needs_conversion {
            convert_with_swr(
                frame,
                nb_samples,
                channels,
                sample_rate,
                src_format,
                output_format,
                output_sample_rate,
                output_channels,
                format_ctx,
                stream_index,
                swr_cache,
                swr_key,
            )
        } else {
            av_frame_to_audio_frame(frame, format_ctx, stream_index)
        }
    }
}

// ── SwResample pipeline ───────────────────────────────────────────────────────

/// Performs sample format / rate / channel conversion using `libswresample`.
///
/// The `SwrContext` is cached in `swr_cache` / `swr_key` and reused across
/// frames so the FIR filter's internal delay buffer is preserved at each frame
/// boundary.  A fresh context is only allocated when the source or target
/// parameters change.
///
/// # Safety
///
/// Caller must ensure `frame` and `format_ctx` are valid.
#[allow(clippy::too_many_arguments)]
unsafe fn convert_with_swr(
    frame: *mut AVFrame,
    nb_samples: usize,
    src_channels: u32,
    src_sample_rate: u32,
    src_format: i32,
    output_format: Option<SampleFormat>,
    output_sample_rate: Option<u32>,
    output_channels: Option<u32>,
    format_ctx: *mut AVFormatContext,
    stream_index: i32,
    swr_cache: &mut Option<SwrContextGuard>,
    swr_key: &mut Option<SwrKey>,
) -> Result<AudioFrame, DecodeError> {
    // Determine target parameters
    let dst_format = output_format.map_or(src_format, sample_format_to_av);
    let dst_sample_rate = output_sample_rate.unwrap_or(src_sample_rate);
    let dst_channels = output_channels.unwrap_or(src_channels);

    // If no conversion is needed, return the frame directly
    if src_format == dst_format
        && src_sample_rate == dst_sample_rate
        && src_channels == dst_channels
    {
        return unsafe { av_frame_to_audio_frame(frame, format_ctx, stream_index) };
    }

    let key: SwrKey = (
        src_format,
        src_sample_rate,
        src_channels,
        dst_format,
        dst_sample_rate,
        dst_channels,
    );

    // Rebuild the SwrContext only when the resampling parameters change.
    // Reusing the context across frames preserves the FIR filter delay buffer,
    // preventing inter-frame discontinuities that cause audio crackling.
    if swr_key.as_ref() != Some(&key) {
        *swr_cache = None; // drop old context (calls swr_free via Drop)

        // Create channel layouts for source and destination
        // SAFETY: We'll properly clean up these layouts via av_channel_layout_uninit
        let mut src_ch_layout = unsafe { create_channel_layout(src_channels) };
        let mut dst_ch_layout = unsafe { create_channel_layout(dst_channels) };

        // Allocate and configure SwrContext
        let mut new_ctx: *mut SwrContext = ptr::null_mut();

        // SAFETY: FFmpeg API call with valid parameters; new_ctx is initialised to null
        let ret = unsafe {
            ff_sys::swr_alloc_set_opts2(
                &raw mut new_ctx,
                &raw const dst_ch_layout,
                dst_format,
                dst_sample_rate as i32,
                &raw const src_ch_layout,
                src_format,
                src_sample_rate as i32,
                0,
                ptr::null_mut(),
            )
        };

        if ret < 0 {
            unsafe {
                ff_sys::av_channel_layout_uninit(&raw mut src_ch_layout);
                ff_sys::av_channel_layout_uninit(&raw mut dst_ch_layout);
            }
            return Err(DecodeError::Ffmpeg {
                code: ret,
                message: format!(
                    "Failed to allocate SwrContext: {}",
                    ff_sys::av_error_string(ret)
                ),
            });
        }

        // Initialize the resampler
        // SAFETY: new_ctx is valid after swr_alloc_set_opts2 succeeded
        let ret = unsafe { ff_sys::swr_init(new_ctx) };
        if ret < 0 {
            unsafe {
                ff_sys::av_channel_layout_uninit(&raw mut src_ch_layout);
                ff_sys::av_channel_layout_uninit(&raw mut dst_ch_layout);
                ff_sys::swr_free(&mut (new_ctx as *mut _));
            }
            return Err(DecodeError::Ffmpeg {
                code: ret,
                message: format!(
                    "Failed to initialize SwrContext: {}",
                    ff_sys::av_error_string(ret)
                ),
            });
        }

        unsafe {
            ff_sys::av_channel_layout_uninit(&raw mut src_ch_layout);
            ff_sys::av_channel_layout_uninit(&raw mut dst_ch_layout);
        }

        *swr_cache = Some(SwrContextGuard(new_ctx));
        *swr_key = Some(key);
    }

    // SAFETY: swr_cache is always Some after the rebuild block above
    let swr_ctx = match swr_cache.as_ref() {
        Some(guard) => guard.0,
        None => {
            return Err(DecodeError::Ffmpeg {
                code: 0,
                message: "SwrContext missing after initialisation".to_string(),
            });
        }
    };

    // Calculate output sample count (includes buffered delay from previous frames)
    // SAFETY: swr_ctx is valid and initialized
    let out_samples = unsafe { ff_sys::swr_get_out_samples(swr_ctx, nb_samples as i32) };

    if out_samples < 0 {
        return Err(DecodeError::Ffmpeg {
            code: 0,
            message: "Failed to calculate output sample count".to_string(),
        });
    }

    let out_samples = out_samples as usize;

    // Allocate output buffer
    let dst_sample_fmt = convert_sample_format(dst_format);
    let bytes_per_sample = dst_sample_fmt.bytes_per_sample();
    let is_planar = dst_sample_fmt.is_planar();

    // Total bytes are identical for planar (dst_channels planes × out_samples ×
    // bytes_per_sample) and packed (out_samples × dst_channels × bytes_per_sample).
    let buffer_size = checked_buffer_size(out_samples, bytes_per_sample, dst_channels as usize)?;

    let mut out_buffer = vec![0u8; buffer_size];

    // Prepare output pointers for swr_convert
    let mut out_ptrs = if is_planar {
        let plane_size = checked_buffer_size(out_samples, bytes_per_sample, 1)?;
        (0..dst_channels)
            .map(|i| {
                let offset = i as usize * plane_size;
                out_buffer[offset..].as_mut_ptr()
            })
            .collect::<Vec<_>>()
    } else {
        vec![out_buffer.as_mut_ptr()]
    };

    // Get input data pointers from frame
    // SAFETY: frame is valid
    let in_ptrs = unsafe { (*frame).data };

    // Convert samples using SwResample
    // SAFETY: All pointers are valid and buffers are properly sized
    let converted_samples = unsafe {
        ff_sys::swr_convert(
            swr_ctx,
            out_ptrs.as_mut_ptr(),
            out_samples as i32,
            in_ptrs.as_ptr() as *mut *const u8,
            nb_samples as i32,
        )
    };

    if converted_samples < 0 {
        return Err(DecodeError::Ffmpeg {
            code: converted_samples,
            message: format!(
                "Failed to convert samples: {}",
                ff_sys::av_error_string(converted_samples)
            ),
        });
    }

    // Extract timestamp from original frame
    // SAFETY: frame is valid
    let timestamp = unsafe {
        let pts = (*frame).pts;
        if pts != ff_sys::AV_NOPTS_VALUE {
            let stream = (*format_ctx).streams.add(stream_index as usize);
            let time_base = (*(*stream)).time_base;
            Timestamp::new(pts, Rational::new(time_base.num, time_base.den))
        } else {
            Timestamp::invalid()
        }
    };

    // Create planes for AudioFrame
    let planes = if is_planar {
        let plane_size = checked_buffer_size(converted_samples as usize, bytes_per_sample, 1)?;
        (0..dst_channels)
            .map(|i| {
                let offset = i as usize * plane_size;
                out_buffer[offset..offset + plane_size].to_vec()
            })
            .collect()
    } else {
        let end = checked_buffer_size(
            converted_samples as usize,
            bytes_per_sample,
            dst_channels as usize,
        )?;
        vec![out_buffer[..end].to_vec()]
    };

    AudioFrame::new(
        planes,
        converted_samples as usize,
        dst_channels,
        dst_sample_rate,
        dst_sample_fmt,
        timestamp,
    )
    .map_err(|e| DecodeError::Ffmpeg {
        code: 0,
        message: format!("Failed to create AudioFrame: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Overflow / bounds guards (issue #1175) ───────────────────────────────

    #[test]
    fn checked_nb_samples_negative_should_return_error() {
        assert!(checked_nb_samples(-1).is_err());
        assert!(checked_nb_samples(i32::MIN).is_err());
    }

    #[test]
    fn checked_nb_samples_valid_should_return_count() {
        assert!(matches!(checked_nb_samples(0), Ok(0)));
        assert!(matches!(checked_nb_samples(1024), Ok(1024)));
    }

    #[test]
    fn checked_buffer_size_normal_should_return_byte_count() {
        // 1024 samples * 8 bytes (f64) * 8 channels = 65_536
        assert!(matches!(checked_buffer_size(1024, 8, 8), Ok(65_536)));
    }

    #[test]
    fn checked_buffer_size_overflow_should_return_error() {
        // Overflows usize on both 64-bit and 32-bit targets.
        assert!(checked_buffer_size(usize::MAX, 2, 1).is_err());
    }

    #[test]
    fn convert_sample_format_should_map_all_packed_formats() {
        assert_eq!(
            convert_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_U8),
            SampleFormat::U8
        );
        assert_eq!(
            convert_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S16),
            SampleFormat::I16
        );
        assert_eq!(
            convert_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S32),
            SampleFormat::I32
        );
        assert_eq!(
            convert_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLT),
            SampleFormat::F32
        );
        assert_eq!(
            convert_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_DBL),
            SampleFormat::F64
        );
    }

    #[test]
    fn convert_sample_format_should_map_all_planar_formats() {
        assert_eq!(
            convert_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_U8P),
            SampleFormat::U8p
        );
        assert_eq!(
            convert_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S16P),
            SampleFormat::I16p
        );
        assert_eq!(
            convert_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_S32P),
            SampleFormat::I32p
        );
        assert_eq!(
            convert_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_FLTP),
            SampleFormat::F32p
        );
        assert_eq!(
            convert_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_DBLP),
            SampleFormat::F64p
        );
    }

    #[test]
    fn convert_sample_format_should_fall_back_to_f32_for_unknown_format() {
        // AV_SAMPLE_FMT_NB is not a real format — should fall back to F32
        assert_eq!(
            convert_sample_format(ff_sys::AVSampleFormat_AV_SAMPLE_FMT_NB),
            SampleFormat::F32
        );
    }

    #[test]
    fn sample_format_to_av_should_round_trip_all_formats() {
        let formats = [
            SampleFormat::U8,
            SampleFormat::I16,
            SampleFormat::I32,
            SampleFormat::F32,
            SampleFormat::F64,
            SampleFormat::U8p,
            SampleFormat::I16p,
            SampleFormat::I32p,
            SampleFormat::F32p,
            SampleFormat::F64p,
        ];
        for fmt in formats {
            let av = sample_format_to_av(fmt);
            let back = convert_sample_format(av);
            assert_eq!(back, fmt, "round-trip failed for {fmt:?}");
        }
    }
}
