//! Shared `FFmpeg` muxer state for all streaming outputs.
//!
//! [`MuxerCore`] owns the `FFmpeg` encode/mux state: the encoder contexts,
//! resampler, scaler, and encoder frames as RAII owners, plus the raw output
//! format context (`out_ctx`). The four protocol-specific inner types
//! (`RtmpInner`, `SrtInner`, `LiveHlsInner`, `LiveDashInner`) each hold a
//! `MuxerCore` and delegate every method except `open_unsafe` to it.

// This module is intentionally unsafe — it drives the FFmpeg C API directly.
#![allow(unsafe_code)]
// Rust 2024: Allow unsafe operations in unsafe functions for FFmpeg C API
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::borrow_as_ptr)]
#![allow(clippy::ref_as_ptr)]

use std::ptr;

use ff_format::{AudioFrame, VideoFrame};
use ff_sys::{
    AVFormatContext, AVPixelFormat, AVPixelFormat_AV_PIX_FMT_YUV420P, AVRational, AVSampleFormat,
    av_rescale_q, av_write_trailer, avformat_free_context,
};

use crate::codec_utils::{
    drain_encoder, ffmpeg_err, ffmpeg_err_msg, pixel_format_to_av, sample_format_to_av,
};
use crate::error::StreamError;

// ============================================================================
// MuxerCore
// ============================================================================

/// Shared `FFmpeg` context owner for all streaming outputs.
///
/// Created by each inner type's `open_unsafe` via [`MuxerCore::new`] after the
/// format context, encoder contexts, and streams are fully configured.
/// Consumed by [`MuxerCore::flush_and_close_unsafe`].
///
/// After `flush_and_close_unsafe` returns all pointers are zeroed; subsequent
/// calls to any method are safe no-ops (Drop will be a no-op too).
pub(crate) struct MuxerCore {
    pub(crate) out_ctx: *mut AVFormatContext,
    pub(crate) vid_enc_ctx: ff_sys::CodecContext,
    /// `None` when audio is not configured (optional for HLS/DASH).
    pub(crate) aud_enc_ctx: Option<ff_sys::CodecContext>,
    /// `None` until first `push_audio_unsafe` call; recreated if input format changes.
    pub(crate) swr_ctx: Option<ff_sys::ResampleContext>,
    /// `None` until swscale is needed; recreated if source dimensions/format change.
    pub(crate) sws_ctx: Option<ff_sys::ScaleContext>,
    pub(crate) vid_enc_frame: ff_sys::Frame,
    pub(crate) aud_enc_frame: ff_sys::Frame,
    pub(crate) vid_out_stream_idx: i32,
    /// `-1` when audio is not configured.
    pub(crate) aud_out_stream_idx: i32,
    pub(crate) video_frame_count: u64,
    pub(crate) audio_pts: i64,
    pub(crate) fps_int: i32,
    pub(crate) enc_width: i32,
    pub(crate) enc_height: i32,
    /// AAC encoder `frame_size` (typically 1024); set after `avcodec_open2`.
    pub(crate) aud_frame_size: i32,
    pub(crate) aud_sample_rate: i32,
    /// Tracks the last swscale source so we can detect changes.
    pub(crate) last_sws_src_fmt: Option<AVPixelFormat>,
    pub(crate) last_sws_src_w: Option<i32>,
    pub(crate) last_sws_src_h: Option<i32>,
    /// Tracks the last swr input so we can detect format changes.
    pub(crate) last_swr_in_fmt: Option<AVSampleFormat>,
    pub(crate) last_swr_in_rate: Option<i32>,
    pub(crate) last_swr_in_channels: Option<i32>,
    /// Human-readable protocol prefix used in log messages (e.g. `"rtmp"`, `"live_hls"`).
    pub(crate) log_prefix: &'static str,
    /// When `true`, `flush_and_close_unsafe` and `free_all` close `out_ctx.pb`
    /// before freeing the format context.
    ///
    /// Set to `true` for RTMP/SRT (pb is kept open for streaming after the header
    /// write). Set to `false` for LiveHLS/LiveDASH (pb is closed immediately after
    /// `avformat_write_header` so the muxer can manage its own avio handles).
    pub(crate) close_pb_after_trailer: bool,
}

// SAFETY: MuxerCore exclusively owns its FFmpeg state (RAII owners plus the raw
// out_ctx). FFmpeg contexts are not safe for concurrent access, but transferring
// ownership between threads is safe (no shared state).
unsafe impl Send for MuxerCore {}

impl MuxerCore {
    /// Allocate encoder frames and initialise tracking state.
    ///
    /// Called by each inner `open_unsafe` after the format context, encoder
    /// contexts, and output streams are fully configured.
    ///
    /// # Safety
    ///
    /// - `out_ctx` must be non-null and valid.
    /// - `aud_enc_ctx` may be `None` (audio is optional for HLS/DASH outputs).
    /// - The `vid_enc_ctx` / `aud_enc_ctx` owned contexts are moved in; on `Err`
    ///   they drop here, so the caller must not free them.
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn new(
        out_ctx: *mut AVFormatContext,
        vid_enc_ctx: ff_sys::CodecContext,
        aud_enc_ctx: Option<ff_sys::CodecContext>,
        vid_out_stream_idx: i32,
        aud_out_stream_idx: i32,
        fps_int: i32,
        enc_width: i32,
        enc_height: i32,
        aud_frame_size: i32,
        aud_sample_rate: i32,
        log_prefix: &'static str,
        close_pb_after_trailer: bool,
    ) -> Result<Self, StreamError> {
        // Owned frames: each frees exactly once on drop. On the error arm the
        // successfully-allocated one (if any) drops at the end of this statement.
        let (Ok(vid_enc_frame), Ok(aud_enc_frame)) = (ff_sys::Frame::new(), ff_sys::Frame::new())
        else {
            return Err(ffmpeg_err_msg("cannot allocate encoder frames"));
        };

        Ok(Self {
            out_ctx,
            vid_enc_ctx,
            aud_enc_ctx,
            swr_ctx: None,
            sws_ctx: None,
            vid_enc_frame,
            aud_enc_frame,
            vid_out_stream_idx,
            aud_out_stream_idx,
            video_frame_count: 0,
            audio_pts: 0,
            fps_int,
            enc_width,
            enc_height,
            aud_frame_size,
            aud_sample_rate,
            last_sws_src_fmt: None,
            last_sws_src_w: None,
            last_sws_src_h: None,
            last_swr_in_fmt: None,
            last_swr_in_rate: None,
            last_swr_in_channels: None,
            log_prefix,
            close_pb_after_trailer,
        })
    }

    /// Encode and mux one video frame.
    ///
    /// # Safety
    ///
    /// `self` must have been initialised by the enclosing inner type's
    /// `open_unsafe` and must not yet be finished.
    pub(crate) unsafe fn push_video_unsafe(
        &mut self,
        frame: &VideoFrame,
    ) -> Result<(), StreamError> {
        let src_fmt = pixel_format_to_av(frame.format());
        let src_w = frame.width() as i32;
        let src_h = frame.height() as i32;
        let needs_conversion = src_fmt != AVPixelFormat_AV_PIX_FMT_YUV420P
            || src_w != self.enc_width
            || src_h != self.enc_height
            || src_fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_NONE;

        // (Re)create SwsContext when source properties change.
        if needs_conversion
            && (self.last_sws_src_fmt != Some(src_fmt)
                || self.last_sws_src_w != Some(src_w)
                || self.last_sws_src_h != Some(src_h))
        {
            // Move-assign the new context; the old one (if any) drops here.
            self.sws_ctx = Some(
                ff_sys::ScaleContext::new(
                    src_w,
                    src_h,
                    src_fmt,
                    self.enc_width,
                    self.enc_height,
                    AVPixelFormat_AV_PIX_FMT_YUV420P,
                    ff_sys::swscale::scale_flags::BILINEAR,
                )
                .map_err(|_| {
                    ffmpeg_err_msg(&format!(
                        "{} swscale context creation failed for video frame",
                        self.log_prefix
                    ))
                })?,
            );
            self.last_sws_src_fmt = Some(src_fmt);
            self.last_sws_src_w = Some(src_w);
            self.last_sws_src_h = Some(src_h);
        }

        // Configure and allocate the encoder frame (common to both paths).
        self.vid_enc_frame
            .set_format(AVPixelFormat_AV_PIX_FMT_YUV420P);
        self.vid_enc_frame.set_width(self.enc_width);
        self.vid_enc_frame.set_height(self.enc_height);
        self.set_vid_enc_pts();

        if let Err(e) = self.vid_enc_frame.get_buffer(0) {
            self.vid_enc_frame.unref();
            return Err(ffmpeg_err(e.code()));
        }

        if needs_conversion {
            // Scale/convert the source into the encoder frame.
            let src_planes: Vec<&[u8]> = frame.planes().iter().map(AsRef::as_ref).collect();
            let src_strides: Vec<i32> = frame.strides().iter().map(|&s| s as i32).collect();

            self.sws_ctx
                .as_mut()
                .ok_or_else(|| {
                    ffmpeg_err_msg(&format!("{} swscale context missing", self.log_prefix))
                })?
                .scale_planes(&src_planes, &src_strides, src_h, &mut self.vid_enc_frame)
                .map_err(|_| {
                    ffmpeg_err_msg(&format!("{} swscale conversion failed", self.log_prefix))
                })?;
        } else {
            // Same format and dimensions — copy Y/U/V planes via self-sizing
            // accessors (each plane's height comes from the frame descriptor).
            let planes = frame.planes();
            let strides = frame.strides();
            for (plane_idx, (src_plane, &src_stride)) in
                planes.iter().zip(strides.iter()).enumerate().take(3)
            {
                // SAFETY: vid_enc_frame is a get_buffer'd frame; linesize is a plain field.
                let dst_stride = (*self.vid_enc_frame.as_ptr()).linesize[plane_idx] as usize;
                let Some(dst_plane) = self.vid_enc_frame.video_plane_mut(plane_idx) else {
                    continue;
                };
                let src_bytes: &[u8] = src_plane.as_ref();
                let copy_width = dst_stride.min(src_stride);
                let num_rows = dst_plane.len() / dst_stride;
                for row in 0..num_rows {
                    let src_off = row * src_stride;
                    let dst_off = row * dst_stride;
                    if src_off + copy_width > src_bytes.len() {
                        break;
                    }
                    dst_plane[dst_off..dst_off + copy_width]
                        .copy_from_slice(&src_bytes[src_off..src_off + copy_width]);
                }
            }
        }

        if self
            .vid_enc_ctx
            .send_frame(self.vid_enc_frame.as_ptr())
            .is_ok()
        {
            // SAFETY: out_ctx is valid; vid_out_stream_idx is valid.
            drain_encoder(
                self.vid_enc_ctx.as_mut_ptr(),
                self.out_ctx,
                self.vid_out_stream_idx,
                self.log_prefix,
                AVRational {
                    num: 1,
                    den: self.fps_int,
                },
            );
        }

        self.vid_enc_frame.unref();
        self.video_frame_count += 1;
        Ok(())
    }

    /// Encode and mux one audio frame.
    ///
    /// Silently returns when audio is not configured (`aud_enc_ctx` is null or
    /// `aud_out_stream_idx < 0`), making this safe to call for all output types.
    ///
    /// # Safety
    ///
    /// `self` must have been initialised by the enclosing inner type's
    /// `open_unsafe` and must not yet be finished.
    pub(crate) unsafe fn push_audio_unsafe(&mut self, frame: &AudioFrame) {
        if self.aud_enc_ctx.is_none() || self.aud_out_stream_idx < 0 {
            return; // audio not configured
        }

        let in_fmt = sample_format_to_av(frame.format());
        let in_rate = frame.sample_rate() as i32;
        let in_channels = frame.channels() as i32;

        // (Re)create SwrContext when input parameters change.
        if self.last_swr_in_fmt != Some(in_fmt)
            || self.last_swr_in_rate != Some(in_rate)
            || self.last_swr_in_channels != Some(in_channels)
        {
            let in_layout = ff_sys::swresample::channel_layout::with_channels(in_channels);
            // Borrow the encoder's channel layout only for the resampler build.
            let new_ctx = {
                let Some(aud_ctx) = self.aud_enc_ctx.as_ref() else {
                    return;
                };
                ff_sys::ResampleContext::new(
                    aud_ctx.ch_layout(),
                    ff_sys::swresample::sample_format::FLTP,
                    self.aud_sample_rate,
                    &in_layout,
                    in_fmt,
                    in_rate,
                )
            };
            let Ok(ctx) = new_ctx else {
                log::warn!("{} swr alloc failed, dropping audio frame", self.log_prefix);
                return;
            };
            // Move-assign the new resampler; the old one (if any) drops here.
            self.swr_ctx = Some(ctx);
            self.last_swr_in_fmt = Some(in_fmt);
            self.last_swr_in_rate = Some(in_rate);
            self.last_swr_in_channels = Some(in_channels);
        }

        // Prepare the encoder frame.
        self.aud_enc_frame
            .set_format(ff_sys::swresample::sample_format::FLTP);
        self.aud_enc_frame.set_sample_rate(self.aud_sample_rate);
        self.aud_enc_frame.set_nb_samples(self.aud_frame_size);
        if let Some(aud_ctx) = self.aud_enc_ctx.as_ref() {
            let _ = self.aud_enc_frame.set_ch_layout(aud_ctx.ch_layout());
        }

        if self.aud_enc_frame.get_buffer(0).is_err() {
            self.aud_enc_frame.unref();
            return;
        }

        // Resample the input into the encoder frame's planes.
        let in_planes: Vec<&[u8]> = frame.planes().iter().map(Vec::as_slice).collect();
        let in_count = frame.samples() as i32;
        let samples_out = if let Some(swr) = self.swr_ctx.as_mut() {
            swr.convert_into_frame(&mut self.aud_enc_frame, &in_planes, in_count)
                .ok()
        } else {
            None
        };

        if let Some(n) = samples_out
            && n > 0
        {
            self.aud_enc_frame.set_nb_samples(n);
            self.aud_enc_frame.set_pts(self.audio_pts);
            if let Some(aud_ctx) = self.aud_enc_ctx.as_mut()
                && aud_ctx.send_frame(self.aud_enc_frame.as_ptr()).is_ok()
            {
                let aud_frame_period = AVRational {
                    num: aud_ctx.frame_size(),
                    den: aud_ctx.sample_rate(),
                };
                // SAFETY: out_ctx is valid; aud_out_stream_idx is valid.
                drain_encoder(
                    aud_ctx.as_mut_ptr(),
                    self.out_ctx,
                    self.aud_out_stream_idx,
                    self.log_prefix,
                    aud_frame_period,
                );
            }
            self.audio_pts += i64::from(n);
        }

        self.aud_enc_frame.unref();
    }

    /// Flush both encoders, write the container trailer, and release all resources.
    ///
    /// For RTMP/SRT (`close_pb_after_trailer = true`) the persistent network
    /// connection (`out_ctx.pb`) is closed after `av_write_trailer`.
    /// For HLS/DASH (`close_pb_after_trailer = false`) `pb` was already closed
    /// after the header write, so this is skipped.
    ///
    /// All pointers are zeroed after this call so that [`Drop`] is a no-op.
    ///
    /// # Safety
    ///
    /// `self` must have been initialised by the enclosing inner type's
    /// `open_unsafe`. This method must be called at most once.
    pub(crate) unsafe fn flush_and_close_unsafe(&mut self) {
        // ── Flush video encoder ───────────────────────────────────────────────
        let _ = self.vid_enc_ctx.send_frame(ptr::null());
        drain_encoder(
            self.vid_enc_ctx.as_mut_ptr(),
            self.out_ctx,
            self.vid_out_stream_idx,
            self.log_prefix,
            AVRational {
                num: 1,
                den: self.fps_int,
            },
        );

        // ── Flush audio encoder ───────────────────────────────────────────────
        if self.aud_out_stream_idx >= 0 && self.aud_enc_ctx.is_some() {
            // Drain any remaining resampler buffered samples.
            if self.swr_ctx.is_some() {
                self.aud_enc_frame
                    .set_format(ff_sys::swresample::sample_format::FLTP);
                self.aud_enc_frame.set_sample_rate(self.aud_sample_rate);
                self.aud_enc_frame.set_nb_samples(self.aud_frame_size);
                if let Some(aud_ctx) = self.aud_enc_ctx.as_ref() {
                    let _ = self.aud_enc_frame.set_ch_layout(aud_ctx.ch_layout());
                }

                if self.aud_enc_frame.get_buffer(0).is_ok() {
                    let out_count = self.aud_frame_size;
                    // Flush the resampler with a NULL input. `convert_into_frame`
                    // models the normal-input case; the flush needs a NULL input
                    // pointer, so the raw `convert` is used here.
                    let flushed = if let Some(swr) = self.swr_ctx.as_mut() {
                        swr.convert(
                            (*self.aud_enc_frame.as_mut_ptr()).data.as_mut_ptr(),
                            out_count,
                            ptr::null(),
                            0,
                        )
                        .ok()
                    } else {
                        None
                    };
                    if let Some(n) = flushed
                        && n > 0
                    {
                        self.aud_enc_frame.set_nb_samples(n);
                        self.aud_enc_frame.set_pts(self.audio_pts);
                        if let Some(aud_ctx) = self.aud_enc_ctx.as_mut()
                            && aud_ctx.send_frame(self.aud_enc_frame.as_ptr()).is_ok()
                        {
                            let aud_frame_period = AVRational {
                                num: aud_ctx.frame_size(),
                                den: aud_ctx.sample_rate(),
                            };
                            drain_encoder(
                                aud_ctx.as_mut_ptr(),
                                self.out_ctx,
                                self.aud_out_stream_idx,
                                self.log_prefix,
                                aud_frame_period,
                            );
                        }
                    }
                    self.aud_enc_frame.unref();
                }
            }

            // Flush the AAC encoder itself.
            if let Some(aud_ctx) = self.aud_enc_ctx.as_mut() {
                let _ = aud_ctx.send_frame(ptr::null());
                let aud_frame_period = AVRational {
                    num: aud_ctx.frame_size(),
                    den: aud_ctx.sample_rate(),
                };
                drain_encoder(
                    aud_ctx.as_mut_ptr(),
                    self.out_ctx,
                    self.aud_out_stream_idx,
                    self.log_prefix,
                    aud_frame_period,
                );
            }
        }

        // ── Write trailer ─────────────────────────────────────────────────────
        av_write_trailer(self.out_ctx);

        // For RTMP/SRT the connection (pb) was kept open throughout the session
        // and must be closed now. For HLS/DASH pb was already closed after the
        // header write, so closing it here would be a double-close.
        if self.close_pb_after_trailer && !(*self.out_ctx).pb.is_null() {
            ff_sys::avformat::close_output(&mut (*self.out_ctx).pb);
        }

        log::info!("{} output finished", self.log_prefix);

        // Free the raw out_ctx and null it so Drop does not double-free; the owned
        // frames/contexts drop with the struct.
        self.free_all();
    }

    /// Set the PTS on `vid_enc_frame` from `video_frame_count` and `fps_int`.
    ///
    /// # Safety
    ///
    /// Calls the `av_rescale_q` FFI; `vid_enc_ctx` must be a valid codec context.
    unsafe fn set_vid_enc_pts(&mut self) {
        let pts = av_rescale_q(
            self.video_frame_count as i64,
            AVRational {
                num: 1,
                den: self.fps_int,
            },
            self.vid_enc_ctx.time_base(),
        );
        self.vid_enc_frame.set_pts(pts);
    }

    /// Release the raw output format context and drop the optional owned contexts.
    ///
    /// The owned encoder frames / codec / resampler / scaler contexts free on their
    /// own `Drop`; this only needs to close `out_ctx.pb` (RTMP/SRT) and free the raw
    /// `out_ctx`, nulling it so a second call (from `Drop`) is a no-op.
    ///
    /// For RTMP/SRT (`close_pb_after_trailer = true`) closes `out_ctx.pb`
    /// before freeing `out_ctx` — handles the abnormal Drop path where
    /// `flush_and_close_unsafe` was never called.
    ///
    /// # Safety
    ///
    /// Must only be called when no other reference to `out_ctx` exists.
    unsafe fn free_all(&mut self) {
        // Owned scale / resample / audio-codec contexts drop when set to `None`;
        // a second call re-`None`s them, so this is idempotent. The `vid_enc_ctx`
        // field is not an `Option` — it drops exactly once when the struct drops.
        self.sws_ctx = None;
        self.swr_ctx = None;
        // `vid_enc_frame` / `aud_enc_frame` are owned `Frame`s: they free exactly
        // once when the struct drops, so there is nothing to free here.
        self.aud_enc_ctx = None;
        if !self.out_ctx.is_null() {
            // For RTMP/SRT: close pb if still open (Drop path where
            // flush_and_close_unsafe was not called).
            if self.close_pb_after_trailer && !(*self.out_ctx).pb.is_null() {
                ff_sys::avformat::close_output(&mut (*self.out_ctx).pb);
            }
            avformat_free_context(self.out_ctx);
            self.out_ctx = ptr::null_mut();
        }
    }
}

impl Drop for MuxerCore {
    fn drop(&mut self) {
        // SAFETY: free_all checks each pointer for null before freeing.
        // flush_and_close_unsafe already zeroed all pointers on the success path,
        // so this is a safe no-op in the normal case.
        unsafe {
            self.free_all();
        }
    }
}
