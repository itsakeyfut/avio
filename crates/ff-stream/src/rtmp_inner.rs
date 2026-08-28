//! Internal RTMP state — all `unsafe` `FFmpeg` calls live here.
//!
//! [`RtmpInner`] owns a [`MuxerCore`] and the RTMP URL. It is created by
//! [`crate::rtmp::RtmpOutput::build`] and driven by the safe wrappers in
//! [`crate::rtmp`].
//!
//! Unlike the HLS/DASH muxers, the RTMP connection (`out_ctx->pb`) is kept
//! open for the entire session; the owned `OutputFormatContext` closes it on
//! drop, after the trailer is written in [`RtmpInner::flush_and_close`].

// This module is intentionally unsafe — it drives the FFmpeg C API directly.
#![allow(unsafe_code)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::borrow_as_ptr)]
#![allow(clippy::ref_as_ptr)]
#![allow(clippy::too_many_lines)]

use std::path::Path;

use ff_format::{AudioFrame, VideoFrame};
use ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P;

use crate::codec_utils::{ffmpeg_err, ffmpeg_err_msg, open_aac_encoder};
use crate::error::StreamError;
use crate::muxer_core::MuxerCore;

// ============================================================================
// RtmpInner
// ============================================================================

/// Owns the shared `FFmpeg` muxer state for the RTMP output session.
///
/// Created by [`RtmpInner::open`]; consumed by [`RtmpInner::flush_and_close`].
/// After `flush_and_close` returns, calling any other method is undefined behaviour;
/// the safe wrapper in `rtmp.rs` prevents this via the `finished` guard.
pub(crate) struct RtmpInner {
    core: MuxerCore,
}

// SAFETY: RtmpInner exclusively owns all FFmpeg contexts via MuxerCore.
unsafe impl Send for RtmpInner {}

impl RtmpInner {
    /// Open the `FFmpeg` context and establish the RTMP connection.
    ///
    /// # Parameters
    ///
    /// - `url`: RTMP ingest URL (e.g. `rtmp://ingest.example.com/live/key`).
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
        // returned RtmpInner takes exclusive ownership of every pointer.
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

    /// Flush both encoders, write the FLV trailer, and close the RTMP connection.
    /// Consumes `self`.
    pub(crate) fn flush_and_close(mut self) {
        // SAFETY: self was initialised by open(); flush_and_close is called once.
        unsafe {
            self.core.flush_and_close_unsafe();
        }
    }

    // Private unsafe implementations

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

        // 1. Allocate FLV output context with RTMP URL
        // The owned context frees itself on every early return below (closing its
        // `pb` if one was opened), so no manual teardown is needed on error paths.
        let mut out_ctx = ff_sys::OutputFormatContext::new(Some("flv"), Path::new(url))
            .map_err(|e| ffmpeg_err(e.code()))?;

        // 2. Open H.264 video encoder
        let vid_enc_codec = crate::codec_utils::select_h264_encoder("rtmp").ok_or_else(|| {
            ffmpeg_err_msg(
                "no H.264 encoder available \
                     (tried h264_nvenc, h264_qsv, h264_amf, h264_videotoolbox, libx264, mpeg4)",
            )
        })?;

        let mut vid_enc_ctx =
            ff_sys::CodecContext::new(Some(vid_enc_codec)).map_err(|e| ffmpeg_err(e.code()))?;
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
        // GOP size of 2 s gives a reasonable keyframe interval for RTMP.
        vid_enc_ctx.set_gop_size(fps_int * 2);
        vid_enc_ctx.set_bit_rate(video_bitrate as i64);

        // On open failure `vid_enc_ctx` drops (frees the codec context); the owned
        // `out_ctx` frees on the `?` early return.
        vid_enc_ctx
            .open_codec(vid_enc_codec)
            .map_err(|e| ffmpeg_err(e.code()))?;

        // 3. Add video output stream
        let vid_out_stream_idx = out_ctx
            .new_stream(Some(&vid_enc_codec))
            .map_err(|e| ffmpeg_err(e.code()))? as i32;
        out_ctx.set_stream_time_base(vid_out_stream_idx as usize, vid_enc_ctx.time_base());
        out_ctx
            .apply_stream_params_from_context(vid_out_stream_idx as usize, &vid_enc_ctx)
            .map_err(|e| ffmpeg_err(e.code()))?;

        // 4. Open AAC audio encoder
        let aud_enc_ctx = open_aac_encoder(aud_sample_rate, aud_channels, aud_bitrate, "rtmp")?;

        let aud_frame_size = if aud_enc_ctx.frame_size() > 0 {
            aud_enc_ctx.frame_size()
        } else {
            1024
        };

        // 5. Add audio output stream
        let aud_out_stream_idx = out_ctx.new_stream(None).map_err(|e| ffmpeg_err(e.code()))? as i32;
        out_ctx.set_stream_time_base(
            aud_out_stream_idx as usize,
            ff_sys::AVRational {
                num: 1,
                den: aud_sample_rate,
            },
        );
        if out_ctx
            .apply_stream_params_from_context(aud_out_stream_idx as usize, &aud_enc_ctx)
            .is_err()
        {
            log::warn!("rtmp audio stream codecpar copy failed");
        }

        // 6. Open RTMP connection and write FLV header
        // Unlike HLS/DASH, RTMP uses a persistent network connection. `open_io`
        // opens the avio handle for the URL and attaches it as the context's `pb`.
        out_ctx
            .open_io(Path::new(url))
            .map_err(|e| ffmpeg_err(e.code()))?;

        // On header-write failure the owned `out_ctx` drops here, closing `pb` and
        // freeing the context.
        out_ctx.write_header().map_err(|e| ffmpeg_err(e.code()))?;

        // NOTE: pb is intentionally kept open. RTMP is a persistent TCP connection;
        // closing pb here would terminate the stream. The owned `out_ctx` closes it
        // on drop, after the trailer in `flush_and_close_unsafe`.

        log::info!(
            "rtmp output opened url={url} video={enc_width}x{enc_height}@{fps_int}fps \
             bitrate={video_bitrate}bps"
        );

        // 7. Build MuxerCore
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
            "rtmp",
        )?;

        Ok(Self { core })
    }
}
