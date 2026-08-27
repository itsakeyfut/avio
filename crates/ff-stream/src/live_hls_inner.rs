//! Internal live HLS state — all `unsafe` `FFmpeg` calls live here.
//!
//! [`LiveHlsInner`] owns a [`MuxerCore`] and the HLS segment duration. It is
//! created by [`crate::live_hls::LiveHlsOutput::build`] and driven by the
//! safe wrappers in [`crate::live_hls`].
//!
//! Public methods on `LiveHlsInner` are safe; all raw `FFmpeg` calls are
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

use ff_format::{AudioFrame, VideoFrame};
use ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P;

use crate::codec_utils::{ffmpeg_err, ffmpeg_err_msg};
use crate::error::StreamError;
use crate::muxer_core::MuxerCore;

// ============================================================================
// LiveHlsInner
// ============================================================================

/// Owns the shared `FFmpeg` muxer state for a live HLS output session.
///
/// Created by [`LiveHlsInner::open`]; consumed by [`LiveHlsInner::flush_and_close`].
/// After `flush_and_close` returns, calling any other method is undefined behaviour;
/// the safe wrapper in `live_hls.rs` prevents this via the `finished` guard.
pub(crate) struct LiveHlsInner {
    core: MuxerCore,
}

// SAFETY: LiveHlsInner exclusively owns all FFmpeg contexts via MuxerCore.
unsafe impl Send for LiveHlsInner {}

impl LiveHlsInner {
    /// Open all `FFmpeg` contexts and write the HLS header.
    ///
    /// # Parameters
    ///
    /// - `output_dir`: directory where `index.m3u8` and `.ts` segments are written.
    /// - `segment_secs`: target HLS segment duration in seconds.
    /// - `playlist_size`: maximum number of segments kept in the sliding playlist.
    /// - `enc_width`, `enc_height`, `fps_int`: video encoder dimensions and frame rate.
    /// - `video_bitrate`: video encoder bit rate in bits/s.
    /// - `audio`: optional `(sample_rate, nb_channels, bit_rate)` tuple; `None` skips audio.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open(
        output_dir: &str,
        segment_secs: u32,
        playlist_size: u32,
        enc_width: i32,
        enc_height: i32,
        fps_int: i32,
        video_bitrate: u64,
        audio: Option<(i32, i32, i64)>,
        segment_format: crate::hls::HlsSegmentFormat,
    ) -> Result<Self, StreamError> {
        // SAFETY: All FFmpeg resources are managed within this function; the
        // returned LiveHlsInner takes exclusive ownership of every pointer.
        unsafe {
            Self::open_unsafe(
                output_dir,
                segment_secs,
                playlist_size,
                enc_width,
                enc_height,
                fps_int,
                video_bitrate,
                audio,
                segment_format,
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

    /// Flush both encoders and write the HLS trailer. Consumes `self`.
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
        playlist_size: u32,
        enc_width: i32,
        enc_height: i32,
        fps_int: i32,
        video_bitrate: u64,
        audio: Option<(i32, i32, i64)>,
        segment_format: crate::hls::HlsSegmentFormat,
    ) -> Result<Self, StreamError> {
        ff_sys::ensure_initialized();

        // ── 1. Allocate HLS output context ────────────────────────────────────
        let playlist_path = format!("{output_dir}/index.m3u8");

        // The owned context frees itself on every early return below, so no manual
        // teardown is needed on error paths.
        let mut out_ctx = ff_sys::OutputFormatContext::new(Some("hls"), Path::new(&playlist_path))
            .map_err(|e| ffmpeg_err(e.code()))?;

        // ── 2. Set HLS muxer options ──────────────────────────────────────────
        let seg_time_str = format!("{segment_secs}");
        let list_size_str = format!("{playlist_size}");
        let use_fmp4 = segment_format == crate::hls::HlsSegmentFormat::Fmp4;
        let seg_ext = if use_fmp4 { "m4s" } else { "ts" };
        let seg_filename = format!("{output_dir}/segment%03d.{seg_ext}");

        if let (Ok(c_seg_time), Ok(c_list_size), Ok(c_seg_file)) = (
            CString::new(seg_time_str.as_str()),
            CString::new(list_size_str.as_str()),
            CString::new(seg_filename.as_str()),
        ) {
            if let Err(e) = out_ctx.set_opt(c"hls_time", &c_seg_time) {
                log::warn!(
                    "live_hls hls_time option not supported, using default \
                     requested={seg_time_str} error={}",
                    ff_sys::av_error_string(e.code())
                );
            }
            if let Err(e) = out_ctx.set_opt(c"hls_list_size", &c_list_size) {
                log::warn!(
                    "live_hls hls_list_size option not supported, using default \
                     requested={list_size_str} error={}",
                    ff_sys::av_error_string(e.code())
                );
            }
            if let Err(e) = out_ctx.set_opt(c"hls_flags", c"delete_segments") {
                log::warn!(
                    "live_hls hls_flags option not supported error={}",
                    ff_sys::av_error_string(e.code())
                );
            }
            if let Err(e) = out_ctx.set_opt(c"hls_segment_filename", &c_seg_file) {
                log::warn!(
                    "live_hls hls_segment_filename option not supported, using default \
                     requested={seg_filename} error={}",
                    ff_sys::av_error_string(e.code())
                );
            }
            if use_fmp4 && let Err(e) = out_ctx.set_opt(c"hls_segment_type", c"fmp4") {
                log::warn!(
                    "live_hls hls_segment_type fmp4 option not supported error={}",
                    ff_sys::av_error_string(e.code())
                );
            }
        }

        // ── 3. Open H.264 video encoder ───────────────────────────────────────
        let vid_enc_codec =
            crate::codec_utils::select_h264_encoder("live_hls").ok_or_else(|| {
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
            .open_codec(vid_enc_codec)
            .map_err(|e| ffmpeg_err(e.code()))?;

        // ── 4. Add video output stream ────────────────────────────────────────
        let vid_out_stream_idx = out_ctx
            .new_stream(Some(&vid_enc_codec))
            .map_err(|e| ffmpeg_err(e.code()))? as i32;
        out_ctx.set_stream_time_base(vid_out_stream_idx as usize, vid_enc_ctx.time_base());
        out_ctx
            .apply_stream_params_from_context(vid_out_stream_idx as usize, &vid_enc_ctx)
            .map_err(|e| ffmpeg_err(e.code()))?;

        // ── 5. Open AAC audio encoder and add audio stream (optional) ─────────
        let mut aud_enc_ctx: Option<ff_sys::CodecContext> = None;
        let mut aud_out_stream_idx: i32 = -1;
        let mut aud_sample_rate = 44100i32;
        let mut aud_frame_size = 1024i32;

        if let Some((sr, nc, abr)) = audio {
            aud_sample_rate = sr;

            match crate::codec_utils::open_aac_encoder(sr, nc, abr, "live_hls") {
                Ok(ctx) => {
                    aud_frame_size = if ctx.frame_size() > 0 {
                        ctx.frame_size()
                    } else {
                        1024
                    };

                    match out_ctx.new_stream(None) {
                        Ok(idx) => {
                            let idx = idx as i32;
                            aud_out_stream_idx = idx;
                            out_ctx.set_stream_time_base(
                                idx as usize,
                                ff_sys::AVRational { num: 1, den: sr },
                            );
                            if out_ctx
                                .apply_stream_params_from_context(idx as usize, &ctx)
                                .is_err()
                            {
                                log::warn!("live_hls audio stream codecpar copy failed");
                            }
                            aud_enc_ctx = Some(ctx);
                        }
                        // `ctx` drops here (frees the codec context).
                        Err(_) => {
                            log::warn!(
                                "live_hls cannot create audio output stream, skipping audio"
                            );
                        }
                    }
                }
                Err(e) => {
                    log::warn!("live_hls aac encoder unavailable: {e}, skipping audio");
                }
            }
        }

        // ── 6. Open output file and write header ──────────────────────────────
        out_ctx
            .open_io(Path::new(&playlist_path))
            .map_err(|e| ffmpeg_err(e.code()))?;

        // On header-write failure the owned `out_ctx` drops here, closing `pb` and
        // freeing the context.
        out_ctx.write_header().map_err(|e| ffmpeg_err(e.code()))?;

        // Close pb so the HLS muxer can rename its .tmp playlist files without
        // hitting a locked-file error on Windows. `close_io` nulls `pb`, so the
        // later drop only frees the context.
        out_ctx.close_io();

        log::info!(
            "live_hls output opened \
             output_dir={output_dir} segment_duration={segment_secs}s \
             playlist_size={playlist_size} width={enc_width} height={enc_height} \
             fps={fps_int} audio={}",
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
            "live_hls",
        )?;

        Ok(Self { core })
    }
}
