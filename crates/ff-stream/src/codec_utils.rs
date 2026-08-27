//! Shared low-level packet-writing and encoder utilities for HLS and DASH muxers.
//!
//! This module provides:
//! - [`drain_encoder`]: drains encoded packets from a codec context and writes them to a mux context
//! - [`select_h264_encoder`]: picks the best available H.264 encoder
//! - [`open_aac_encoder`]: opens an AAC encoder context
//! - [`ffmpeg_err`]: maps an `FFmpeg` error code to [`StreamError::Ffmpeg`]

// This module is intentionally unsafe — it drives the FFmpeg C API directly.
#![allow(unsafe_code)]
// Rust 2024: Allow unsafe operations in unsafe functions for FFmpeg C API
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::borrow_as_ptr)]
#![allow(clippy::ref_as_ptr)]

use ff_sys::{
    AVPixelFormat, AVPixelFormat_AV_PIX_FMT_NONE, AVRational, AVSampleFormat, ReceiveOutcome,
    av_rescale_q,
};

use ff_format::{PixelFormat, SampleFormat};

use crate::error::StreamError;

// ============================================================================
// Error helpers
// ============================================================================

/// Map an `FFmpeg` negative return code to [`StreamError::Ffmpeg`].
pub(crate) fn ffmpeg_err(code: i32) -> StreamError {
    StreamError::Ffmpeg {
        code,
        message: ff_sys::av_error_string(code),
    }
}

/// Build a [`StreamError::Ffmpeg`] from a plain message (no numeric code).
pub(crate) fn ffmpeg_err_msg(msg: &str) -> StreamError {
    StreamError::Ffmpeg {
        code: 0,
        message: msg.to_owned(),
    }
}

// ============================================================================
// Encoder selection helpers
// ============================================================================

/// Return the best available H.264 encoder.
///
/// Tries hardware encoders first (`h264_nvenc`, `h264_qsv`, `h264_amf`,
/// `h264_videotoolbox`), then software (`libx264`, `mpeg4`).
///
/// # Safety
///
/// Must be called after `ff_sys::ensure_initialized()`.
pub(crate) unsafe fn select_h264_encoder(log_prefix: &str) -> Option<ff_sys::Codec> {
    let candidates = [
        "h264_nvenc",
        "h264_qsv",
        "h264_amf",
        "h264_videotoolbox",
        "libx264",
        "mpeg4",
    ];
    for name in candidates {
        if let Some(codec) = ff_sys::Codec::find_encoder_by_name(name) {
            log::info!("{log_prefix} selected video encoder encoder={name}");
            return Some(codec);
        }
    }
    None
}

/// Open an AAC audio encoder configured for `sample_rate` Hz and `nb_channels` channels.
///
/// Tries `aac` first, then `libfdk_aac`.
///
/// # Safety
///
/// Must be called after `ff_sys::ensure_initialized()`.
pub(crate) unsafe fn open_aac_encoder(
    sample_rate: i32,
    nb_channels: i32,
    bit_rate: i64,
    log_prefix: &str,
) -> Result<ff_sys::CodecContext, StreamError> {
    let codec = ff_sys::Codec::find_encoder_by_name("aac")
        .or_else(|| ff_sys::Codec::find_encoder_by_name("libfdk_aac"))
        .ok_or_else(|| ffmpeg_err_msg("no AAC encoder available (tried aac, libfdk_aac)"))?;

    let mut enc = ff_sys::CodecContext::new(Some(codec)).map_err(|e| ffmpeg_err(e.code()))?;

    enc.set_sample_rate(sample_rate);
    enc.set_sample_fmt(ff_sys::swresample::sample_format::FLTP);
    enc.set_bit_rate(bit_rate);
    enc.set_time_base(AVRational {
        num: 1,
        den: sample_rate,
    });
    enc.set_ch_layout_default(nb_channels);

    // On open failure `enc` drops (Drop = avcodec_free_context), so no manual free.
    enc.open_codec(codec).map_err(|e| ffmpeg_err(e.code()))?;

    log::info!(
        "{log_prefix} aac encoder opened \
         sample_rate={sample_rate} channels={nb_channels} bit_rate={bit_rate}"
    );
    Ok(enc)
}

// ============================================================================
// Format conversion helpers
// ============================================================================

/// Map a [`PixelFormat`] to the corresponding `AVPixelFormat` constant.
///
/// Returns `AV_PIX_FMT_NONE` for unknown or `Other(_)` variants.
pub(crate) fn pixel_format_to_av(fmt: PixelFormat) -> AVPixelFormat {
    match fmt {
        PixelFormat::Yuv420p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P,
        PixelFormat::Yuv422p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P,
        PixelFormat::Yuv444p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P,
        PixelFormat::Rgb24 => ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24,
        PixelFormat::Bgr24 => ff_sys::AVPixelFormat_AV_PIX_FMT_BGR24,
        PixelFormat::Rgba => ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA,
        PixelFormat::Bgra => ff_sys::AVPixelFormat_AV_PIX_FMT_BGRA,
        PixelFormat::Nv12 => ff_sys::AVPixelFormat_AV_PIX_FMT_NV12,
        PixelFormat::Nv21 => ff_sys::AVPixelFormat_AV_PIX_FMT_NV21,
        PixelFormat::Yuv420p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE,
        PixelFormat::Yuv422p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P10LE,
        PixelFormat::Yuv444p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P10LE,
        PixelFormat::Yuva444p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUVA444P10LE,
        PixelFormat::P010le => ff_sys::AVPixelFormat_AV_PIX_FMT_P010LE,
        PixelFormat::Gray8 => ff_sys::AVPixelFormat_AV_PIX_FMT_GRAY8,
        PixelFormat::Gbrpf32le => ff_sys::AVPixelFormat_AV_PIX_FMT_GBRPF32LE,
        PixelFormat::Other(_) | _ => AVPixelFormat_AV_PIX_FMT_NONE,
    }
}

/// Map a [`SampleFormat`] to the corresponding `AVSampleFormat` constant.
///
/// Returns `AV_SAMPLE_FMT_NONE` for unknown or `Other(_)` variants.
pub(crate) fn sample_format_to_av(fmt: SampleFormat) -> AVSampleFormat {
    match fmt {
        SampleFormat::U8 => ff_sys::swresample::sample_format::U8,
        SampleFormat::I16 => ff_sys::swresample::sample_format::S16,
        SampleFormat::I32 => ff_sys::swresample::sample_format::S32,
        SampleFormat::F32 => ff_sys::swresample::sample_format::FLT,
        SampleFormat::F64 => ff_sys::swresample::sample_format::DBL,
        SampleFormat::U8p => ff_sys::swresample::sample_format::U8P,
        SampleFormat::I16p => ff_sys::swresample::sample_format::S16P,
        SampleFormat::I32p => ff_sys::swresample::sample_format::S32P,
        SampleFormat::F32p => ff_sys::swresample::sample_format::FLTP,
        SampleFormat::F64p => ff_sys::swresample::sample_format::DBLP,
        SampleFormat::Other(_) | _ => ff_sys::swresample::sample_format::NONE,
    }
}

/// Drain all available encoded packets from `enc_ctx` into `out_ctx`.
///
/// For each packet received from the encoder:
/// 1. Overrides `pkt->duration` (before rescaling) with one frame's worth of
///    time expressed in `enc_ctx->time_base` units — computed from `frame_period`
///    at drain time.  Some encoders (e.g. mpeg4) lazily mutate their `time_base`
///    on the first `avcodec_send_frame` call, so `enc_ctx->time_base` must be
///    read here, not in the calling code.  The HLS/DASH muxers accumulate
///    `pkt->duration` to determine segment boundaries and `TARGETDURATION`; a
///    near-zero duration produces `#EXT-X-TARGETDURATION:0`.
/// 2. Rescales `pts`, `dts`, and `duration` from `enc_ctx->time_base` to the
///    output stream's `time_base` using `av_packet_rescale_ts`.
/// 3. Writes the packet with `av_interleaved_write_frame`.
///
/// # Parameters
///
/// - `frame_period`: rational duration of one encoder frame, expressed as a
///   fraction of a second (e.g. `{1, fps}` for video, `{frame_size, sample_rate}`
///   for audio).  Converted to `enc_ctx->time_base` units inside this function so
///   it is immune to lazy time-base mutations.
///
/// # Preconditions
///
/// - `enc_ctx` must be fully opened with at least one `send_frame` preceding this
///   call, and `out_ctx`'s header must already have been written. Both are
///   borrowed mutably, so their lifecycle stays owned by the caller.
/// - `stream_idx` must be a valid index into `out_ctx`'s stream array (an
///   out-of-range index yields a `0/0` time base and the write fails).
pub(crate) fn drain_encoder(
    enc_ctx: &mut ff_sys::CodecContext,
    out_ctx: &mut ff_sys::OutputFormatContext,
    stream_idx: usize,
    log_prefix: &str,
    frame_period: AVRational,
) {
    let Ok(mut pkt) = ff_sys::Packet::new() else {
        return;
    };

    let stream_tb = out_ctx.stream_time_base(stream_idx);
    // Read enc_tb HERE — some encoders (mpeg4) mutate time_base lazily on first
    // send_frame, so the value may differ from what the caller observed earlier.
    let enc_tb = enc_ctx.time_base();

    // Compute the correct per-frame duration in enc_tb units using the live enc_tb.
    // av_rescale_q converts 1 unit of `frame_period` (e.g. 1/fps second) into enc_tb ticks.
    // SAFETY: `av_rescale_q` is a pure integer rescale with no pointer arguments.
    let frame_dur_enc_tb = unsafe { av_rescale_q(1, frame_period, enc_tb) };

    // NeedInput (EAGAIN) / Drained (EOF) / real error all end the drain.
    while let Ok(ReceiveOutcome::Frame) = enc_ctx.receive_packet(&mut pkt) {
        // Always override duration with the correct per-frame value BEFORE rescaling.
        if frame_dur_enc_tb > 0 {
            pkt.set_duration(frame_dur_enc_tb);
        }

        // Rescale pts/dts/duration from encoder time_base to stream time_base.
        pkt.rescale_ts(enc_tb, stream_tb);

        pkt.set_stream_index(stream_idx as i32);
        let ret = out_ctx.write_interleaved(&mut pkt);
        pkt.unref();
        if let Err(e) = ret {
            log::warn!(
                "{log_prefix} av_interleaved_write_frame failed \
                 stream_index={stream_idx} error={}",
                ff_sys::av_error_string(e.code())
            );
            break;
        }
    }
}
