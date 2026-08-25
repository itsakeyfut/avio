//! Internal live DASH state — all `unsafe` `FFmpeg` calls live here.
//!
//! [`LiveDashInner`] owns a [`MuxerCore`] and the DASH segment duration. It is
//! created by [`crate::live_dash::LiveDashOutput::build`] and driven by the
//! safe wrappers in [`crate::live_dash`].
//!
//! Public methods on `LiveDashInner` are safe; all raw `FFmpeg` calls are
//! confined to `unsafe {}` blocks inside this file.

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
use ff_sys::{AVPixelFormat_AV_PIX_FMT_YUV420P, av_opt_set, avformat_new_stream};

use crate::codec_utils::{ffmpeg_err, ffmpeg_err_msg};
use crate::error::StreamError;
use crate::muxer_core::MuxerCore;

// ============================================================================
// LiveDashInner
// ============================================================================

/// Owns the shared `FFmpeg` muxer state for a live DASH output session.
///
/// Created by [`LiveDashInner::open`]; consumed by [`LiveDashInner::flush_and_close`].
/// After `flush_and_close` returns, calling any other method is undefined behaviour;
/// the safe wrapper in `live_dash.rs` prevents this via the `finished` guard.
pub(crate) struct LiveDashInner {
    core: MuxerCore,
}

// SAFETY: LiveDashInner exclusively owns all FFmpeg contexts via MuxerCore.
unsafe impl Send for LiveDashInner {}

impl LiveDashInner {
    /// Open all `FFmpeg` contexts and write the DASH header.
    ///
    /// # Parameters
    ///
    /// - `output_dir`: directory where `manifest.mpd` and `.m4s` segments are written.
    /// - `segment_secs`: target DASH segment duration in seconds.
    /// - `enc_width`, `enc_height`, `fps_int`: video encoder dimensions and frame rate.
    /// - `video_bitrate`: video encoder bit rate in bits/s.
    /// - `audio`: optional `(sample_rate, nb_channels, bit_rate)` tuple; `None` skips audio.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open(
        output_dir: &str,
        segment_secs: u32,
        enc_width: i32,
        enc_height: i32,
        fps_int: i32,
        video_bitrate: u64,
        audio: Option<(i32, i32, i64)>,
    ) -> Result<Self, StreamError> {
        // SAFETY: All FFmpeg resources are managed within this function; the
        // returned LiveDashInner takes exclusive ownership of every pointer.
        unsafe {
            Self::open_unsafe(
                output_dir,
                segment_secs,
                enc_width,
                enc_height,
                fps_int,
                video_bitrate,
                audio,
            )
        }
    }

    /// Encode and mux one video frame.
    pub(crate) fn push_video(&mut self, frame: &VideoFrame) -> Result<(), StreamError> {
        // SAFETY: self was initialised by open() and is not yet finished.
        unsafe { self.core.push_video_unsafe(frame) }
    }

    /// Encode and mux one audio frame.
    ///
    /// If audio was not configured at `open` time, this is a silent no-op.
    pub(crate) fn push_audio(&mut self, frame: &AudioFrame) {
        // SAFETY: self was initialised by open() and is not yet finished.
        unsafe {
            self.core.push_audio_unsafe(frame);
        }
    }

    /// Flush both encoders and write the DASH trailer. Consumes `self`.
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
        output_dir: &str,
        segment_secs: u32,
        enc_width: i32,
        enc_height: i32,
        fps_int: i32,
        video_bitrate: u64,
        audio: Option<(i32, i32, i64)>,
    ) -> Result<Self, StreamError> {
        ff_sys::ensure_initialized();

        // ── 1. Allocate DASH output context ───────────────────────────────────
        let manifest_path = format!("{output_dir}/manifest.mpd");

        // The owned context frees itself on every early return below, so no manual
        // teardown is needed on error paths.
        let mut out_ctx = ff_sys::OutputFormatContext::new(Some("dash"), Path::new(&manifest_path))
            .map_err(|e| ffmpeg_err(e.code()))?;

        // ── 2. Set DASH muxer options ─────────────────────────────────────────
        let seg_time_str = format!("{segment_secs}");

        if let Ok(c_seg_time) = CString::new(seg_time_str.as_str()) {
            let ret = av_opt_set(
                (*out_ctx.as_mut_ptr()).priv_data,
                c"seg_duration".as_ptr(),
                c_seg_time.as_ptr(),
                0,
            );
            if ret < 0 {
                log::warn!(
                    "live_dash seg_duration option not supported, using default \
                     requested={seg_time_str} error={}",
                    ff_sys::av_error_string(ret)
                );
            }
        }

        let ret = av_opt_set(
            (*out_ctx.as_mut_ptr()).priv_data,
            c"use_template".as_ptr(),
            c"1".as_ptr(),
            0,
        );
        if ret < 0 {
            log::warn!(
                "live_dash use_template option not supported error={}",
                ff_sys::av_error_string(ret)
            );
        }

        let ret = av_opt_set(
            (*out_ctx.as_mut_ptr()).priv_data,
            c"use_timeline".as_ptr(),
            c"1".as_ptr(),
            0,
        );
        if ret < 0 {
            log::warn!(
                "live_dash use_timeline option not supported error={}",
                ff_sys::av_error_string(ret)
            );
        }

        let ret = av_opt_set(
            (*out_ctx.as_mut_ptr()).priv_data,
            c"remove_at_exit".as_ptr(),
            c"0".as_ptr(),
            0,
        );
        if ret < 0 {
            log::warn!(
                "live_dash remove_at_exit option not supported error={}",
                ff_sys::av_error_string(ret)
            );
        }

        // ── 3. Open H.264 video encoder ───────────────────────────────────────
        let vid_enc_codec =
            crate::codec_utils::select_h264_encoder("live_dash").ok_or_else(|| {
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
        vid_enc_ctx.set_gop_size(fps_int * segment_secs as i32);
        vid_enc_ctx.set_bit_rate(video_bitrate as i64);

        // On open failure `vid_enc_ctx` drops (frees the codec context); the owned
        // `out_ctx` frees on the `?` early return.
        vid_enc_ctx
            .open(vid_enc_codec, ptr::null_mut())
            .map_err(|e| ffmpeg_err(e.code()))?;

        // ── 4. Add video output stream ────────────────────────────────────────
        let vid_out_stream = avformat_new_stream(out_ctx.as_mut_ptr(), vid_enc_codec.as_ptr());
        if vid_out_stream.is_null() {
            return Err(ffmpeg_err_msg("cannot create video output stream"));
        }
        (*vid_out_stream).time_base = vid_enc_ctx.time_base();
        let vid_out_stream_idx = (out_ctx.nb_streams() - 1) as i32;

        // SAFETY: vid_out_stream and vid_enc_ctx are valid; avcodec_open2 has been called.
        vid_enc_ctx
            .parameters_from_context((*vid_out_stream).codecpar)
            .map_err(|e| ffmpeg_err(e.code()))?;

        // ── 5. Open AAC audio encoder and add audio stream (optional) ─────────
        let mut aud_enc_ctx: Option<ff_sys::CodecContext> = None;
        let mut aud_out_stream_idx: i32 = -1;
        let mut aud_sample_rate = 44100i32;
        let mut aud_frame_size = 1024i32;

        if let Some((sr, nc, abr)) = audio {
            aud_sample_rate = sr;

            match crate::codec_utils::open_aac_encoder(sr, nc, abr, "live_dash") {
                Ok(ctx) => {
                    aud_frame_size = if ctx.frame_size() > 0 {
                        ctx.frame_size()
                    } else {
                        1024
                    };

                    let aud_out_stream = avformat_new_stream(out_ctx.as_mut_ptr(), ptr::null());
                    if aud_out_stream.is_null() {
                        // `ctx` drops here (frees the codec context).
                        log::warn!("live_dash cannot create audio output stream, skipping audio");
                    } else {
                        (*aud_out_stream).time_base.num = 1;
                        (*aud_out_stream).time_base.den = sr;
                        aud_out_stream_idx = (out_ctx.nb_streams() - 1) as i32;

                        // SAFETY: aud_out_stream and ctx are valid.
                        if ctx
                            .parameters_from_context((*aud_out_stream).codecpar)
                            .is_err()
                        {
                            log::warn!("live_dash audio stream codecpar copy failed");
                        }
                        aud_enc_ctx = Some(ctx);
                    }
                }
                Err(e) => {
                    log::warn!("live_dash aac encoder unavailable: {e}, skipping audio");
                }
            }
        }

        // ── 6. Open output file and write header ──────────────────────────────
        out_ctx
            .open_io(Path::new(&manifest_path))
            .map_err(|e| ffmpeg_err(e.code()))?;

        // On header-write failure the owned `out_ctx` drops here, closing `pb` and
        // freeing the context.
        out_ctx.write_header().map_err(|e| ffmpeg_err(e.code()))?;

        // Close pb so the DASH muxer can manage its own avio handles for segment
        // files without hitting a locked-file error on Windows. `close_io` nulls
        // `pb`, so the later drop only frees the context.
        out_ctx.close_io();

        log::info!(
            "live_dash output opened \
             output_dir={output_dir} segment_duration={segment_secs}s \
             width={enc_width} height={enc_height} fps={fps_int} audio={}",
            aud_out_stream_idx >= 0
        );

        // ── 7. Build MuxerCore ────────────────────────────────────────────────
        let core = MuxerCore::new(
            out_ctx,
            vid_enc_ctx,
            aud_enc_ctx,
            vid_out_stream_idx,
            aud_out_stream_idx,
            fps_int,
            enc_width,
            enc_height,
            aud_frame_size,
            aud_sample_rate,
            "live_dash",
        )?;

        Ok(Self { core })
    }
}
