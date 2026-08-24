//! Internal SRT state — all `unsafe` `FFmpeg` calls live here.
//!
//! [`SrtInner`] owns a [`MuxerCore`] and the SRT URL. It is created by
//! [`crate::srt_output::SrtOutput::build`] and driven by the safe wrappers in
//! [`crate::srt_output`].
//!
//! The MPEG-TS muxer writes to the SRT URL via `avio_open2`. The connection
//! (`out_ctx->pb`) is kept open for the entire session and closed after
//! [`av_write_trailer`] in [`SrtInner::flush_and_close`].

// This module is intentionally unsafe — it drives the FFmpeg C API directly.
#![allow(unsafe_code)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::borrow_as_ptr)]
#![allow(clippy::ref_as_ptr)]
#![allow(clippy::too_many_lines)]

use std::ffi::CString;
use std::path::Path;
use std::ptr;

use ff_format::{AudioFrame, VideoFrame};
use ff_sys::{
    AVPixelFormat_AV_PIX_FMT_YUV420P, avformat_alloc_output_context2, avformat_free_context,
    avformat_new_stream, avformat_write_header,
};

use crate::codec_utils::{ffmpeg_err, ffmpeg_err_msg, open_aac_encoder};
use crate::error::StreamError;
use crate::muxer_core::MuxerCore;

// ============================================================================
// SrtInner
// ============================================================================

/// Owns the shared `FFmpeg` muxer state for the SRT output session.
///
/// Created by [`SrtInner::open`]; consumed by [`SrtInner::flush_and_close`].
/// After `flush_and_close` returns, calling any other method is undefined
/// behaviour; the safe wrapper in `srt_output.rs` prevents this via the
/// `finished` guard.
pub(crate) struct SrtInner {
    core: MuxerCore,
}

// SAFETY: SrtInner exclusively owns all FFmpeg contexts via MuxerCore.
unsafe impl Send for SrtInner {}

impl SrtInner {
    /// Open the `FFmpeg` MPEG-TS context and establish the SRT connection.
    ///
    /// # Parameters
    ///
    /// - `url`: SRT destination URL (e.g. `srt://192.168.1.100:9000`).
    /// - `enc_width`, `enc_height`, `fps_int`: video encoder dimensions and frame rate.
    /// - `video_bitrate`: video encoder bit rate in bits/s.
    /// - `aud_sample_rate`, `aud_channels`, `aud_bitrate`: audio encoder parameters.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open(
        url: &str,
        enc_width: i32,
        enc_height: i32,
        fps_int: i32,
        video_bitrate: u64,
        aud_sample_rate: i32,
        aud_channels: i32,
        aud_bitrate: i64,
    ) -> Result<Self, StreamError> {
        // SAFETY: All FFmpeg resources are managed within this function; the
        // returned SrtInner takes exclusive ownership of every pointer.
        unsafe {
            Self::open_unsafe(
                url,
                enc_width,
                enc_height,
                fps_int,
                video_bitrate,
                aud_sample_rate,
                aud_channels,
                aud_bitrate,
            )
        }
    }

    /// Encode and mux one video frame.
    pub(crate) fn push_video(&mut self, frame: &VideoFrame) -> Result<(), StreamError> {
        // SAFETY: self was initialised by open() and is not yet finished.
        unsafe { self.core.push_video_unsafe(frame) }
    }

    /// Encode and mux one audio frame.
    pub(crate) fn push_audio(&mut self, frame: &AudioFrame) {
        // SAFETY: self was initialised by open() and is not yet finished.
        unsafe {
            self.core.push_audio_unsafe(frame);
        }
    }

    /// Flush both encoders, write the MPEG-TS trailer, and close the SRT connection.
    /// Consumes `self`.
    pub(crate) fn flush_and_close(mut self) {
        // SAFETY: self was initialised by open(); flush_and_close is called once.
        unsafe {
            self.core.flush_and_close_unsafe();
        }
    }

    // ── Private unsafe implementations ───────────────────────────────────────

    #[allow(unsafe_op_in_unsafe_fn)]
    #[allow(clippy::too_many_arguments)]
    unsafe fn open_unsafe(
        url: &str,
        enc_width: i32,
        enc_height: i32,
        fps_int: i32,
        video_bitrate: u64,
        aud_sample_rate: i32,
        aud_channels: i32,
        aud_bitrate: i64,
    ) -> Result<Self, StreamError> {
        ff_sys::ensure_initialized();

        // ── 1. Allocate MPEG-TS output context with SRT URL ────────────────
        let c_url = CString::new(url).map_err(|_| ffmpeg_err_msg("SRT URL contains null byte"))?;

        let mut out_ctx: *mut ff_sys::AVFormatContext = ptr::null_mut();
        let ret = avformat_alloc_output_context2(
            &mut out_ctx,
            ptr::null_mut(),
            c"mpegts".as_ptr(),
            c_url.as_ptr(),
        );
        if ret < 0 || out_ctx.is_null() {
            return Err(ffmpeg_err(ret));
        }

        // ── 2. Open H.264 video encoder ────────────────────────────────────
        let vid_enc_codec = crate::codec_utils::select_h264_encoder("srt").ok_or_else(|| {
            avformat_free_context(out_ctx);
            ffmpeg_err_msg(
                "no H.264 encoder available \
                 (tried h264_nvenc, h264_qsv, h264_amf, h264_videotoolbox, libx264, mpeg4)",
            )
        })?;

        let mut vid_enc_ctx = ff_sys::CodecContext::new(Some(vid_enc_codec)).map_err(|e| {
            avformat_free_context(out_ctx);
            ffmpeg_err(e.code())
        })?;
        vid_enc_ctx.set_width(enc_width);
        vid_enc_ctx.set_height(enc_height);
        vid_enc_ctx.set_pix_fmt(AVPixelFormat_AV_PIX_FMT_YUV420P);
        vid_enc_ctx.set_time_base(ff_sys::AVRational {
            num: 1,
            den: fps_int,
        });
        vid_enc_ctx.set_framerate(ff_sys::AVRational {
            num: fps_int,
            den: 1,
        });
        // GOP size of 2 s gives a reasonable keyframe interval for MPEG-TS/SRT.
        vid_enc_ctx.set_gop_size(fps_int * 2);
        vid_enc_ctx.set_bit_rate(video_bitrate as i64);

        // On open failure `vid_enc_ctx` drops (frees the codec context); only the
        // raw format context needs an explicit free.
        vid_enc_ctx
            .open(vid_enc_codec, ptr::null_mut())
            .map_err(|e| {
                avformat_free_context(out_ctx);
                ffmpeg_err(e.code())
            })?;

        // ── 3. Add video output stream ─────────────────────────────────────
        let vid_out_stream = avformat_new_stream(out_ctx, vid_enc_codec.as_ptr());
        if vid_out_stream.is_null() {
            avformat_free_context(out_ctx);
            return Err(ffmpeg_err_msg("cannot create video output stream"));
        }
        (*vid_out_stream).time_base = vid_enc_ctx.time_base();
        let vid_out_stream_idx = ((*out_ctx).nb_streams - 1) as i32;

        // SAFETY: vid_out_stream and vid_enc_ctx are valid; avcodec_open2 has been called.
        vid_enc_ctx
            .parameters_from_context((*vid_out_stream).codecpar)
            .map_err(|e| {
                avformat_free_context(out_ctx);
                ffmpeg_err(e.code())
            })?;

        // ── 4. Open AAC audio encoder ──────────────────────────────────────
        let aud_enc_ctx = open_aac_encoder(aud_sample_rate, aud_channels, aud_bitrate, "srt")
            .inspect_err(|_| {
                avformat_free_context(out_ctx);
            })?;

        let aud_frame_size = if aud_enc_ctx.frame_size() > 0 {
            aud_enc_ctx.frame_size()
        } else {
            1024
        };

        // ── 5. Add audio output stream ─────────────────────────────────────
        let aud_out_stream = avformat_new_stream(out_ctx, ptr::null());
        if aud_out_stream.is_null() {
            avformat_free_context(out_ctx);
            return Err(ffmpeg_err_msg("cannot create audio output stream"));
        }
        (*aud_out_stream).time_base.num = 1;
        (*aud_out_stream).time_base.den = aud_sample_rate;
        let aud_out_stream_idx = ((*out_ctx).nb_streams - 1) as i32;

        // SAFETY: aud_out_stream and aud_enc_ctx are valid.
        if aud_enc_ctx
            .parameters_from_context((*aud_out_stream).codecpar)
            .is_err()
        {
            log::warn!("srt audio stream codecpar copy failed");
        }

        // ── 6. Open SRT connection and write MPEG-TS header ────────────────
        // SRT uses a persistent network connection like RTMP.
        // We use avio_open via avformat::open_output with the URL as the path.
        // SAFETY: url is a valid null-terminated C string (validated above).
        let pb = ff_sys::avformat::open_output(Path::new(url), ff_sys::avformat::avio_flags::WRITE)
            .map_err(|e| {
                avformat_free_context(out_ctx);
                ffmpeg_err(e)
            })?;
        (*out_ctx).pb = pb;

        let ret = avformat_write_header(out_ctx, ptr::null_mut());
        if ret < 0 {
            ff_sys::avformat::close_output(&mut (*out_ctx).pb);
            avformat_free_context(out_ctx);
            return Err(ffmpeg_err(ret));
        }

        // NOTE: pb is intentionally kept open. SRT is a persistent connection;
        // closing pb here would terminate the stream.

        log::info!(
            "srt output opened url={url} video={enc_width}x{enc_height}@{fps_int}fps \
             bitrate={video_bitrate}bps"
        );

        // ── 7. Build MuxerCore ─────────────────────────────────────────────
        let core = MuxerCore::new(
            out_ctx,
            vid_enc_ctx,
            Some(aud_enc_ctx),
            vid_out_stream_idx,
            aud_out_stream_idx,
            fps_int,
            enc_width,
            enc_height,
            aud_frame_size,
            aud_sample_rate,
            "srt",
            true, // close_pb_after_trailer: pb stays open for streaming
        )
        .inspect_err(|_| {
            // `vid_enc_ctx` / `aud_enc_ctx` were moved in and drop inside `new` on
            // error; only the raw format context needs an explicit free.
            avformat_free_context(out_ctx);
        })?;

        Ok(Self { core })
    }
}
