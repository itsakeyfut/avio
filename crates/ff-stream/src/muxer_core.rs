//! Shared `FFmpeg` muxer state for all streaming outputs.
//!
//! [`MuxerCore`] owns all raw `FFmpeg` contexts (video/audio encoder, resampler,
//! scaler, output format context). The four protocol-specific inner types
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
    AVFormatContext, AVFrame, AVPixelFormat, AVPixelFormat_AV_PIX_FMT_YUV420P, AVRational,
    AVSampleFormat, av_frame_alloc, av_frame_free, av_frame_get_buffer, av_frame_unref,
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
    pub(crate) vid_enc_frame: *mut AVFrame,
    pub(crate) aud_enc_frame: *mut AVFrame,
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

// SAFETY: MuxerCore exclusively owns all raw FFmpeg pointers. FFmpeg contexts
// are not safe for concurrent access, but transferring ownership between threads
// is safe (no shared state).
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
        let vid_enc_frame = av_frame_alloc();
        let aud_enc_frame = av_frame_alloc();

        if vid_enc_frame.is_null() || aud_enc_frame.is_null() {
            if !vid_enc_frame.is_null() {
                av_frame_free(&mut (vid_enc_frame as *mut _));
            }
            if !aud_enc_frame.is_null() {
                av_frame_free(&mut (aud_enc_frame as *mut _));
            }
            return Err(ffmpeg_err_msg("cannot allocate encoder frames"));
        }

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

        if needs_conversion {
            // (Re)create SwsContext when source properties change.
            if self.last_sws_src_fmt != Some(src_fmt)
                || self.last_sws_src_w != Some(src_w)
                || self.last_sws_src_h != Some(src_h)
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

            (*self.vid_enc_frame).format = AVPixelFormat_AV_PIX_FMT_YUV420P;
            (*self.vid_enc_frame).width = self.enc_width;
            (*self.vid_enc_frame).height = self.enc_height;
            self.set_vid_enc_pts();

            let buf_ret = av_frame_get_buffer(self.vid_enc_frame, 0);
            if buf_ret < 0 {
                av_frame_unref(self.vid_enc_frame);
                return Err(ffmpeg_err(buf_ret));
            }

            // Build source plane/linesize arrays from the VideoFrame.
            let planes = frame.planes();
            let strides = frame.strides();
            let mut src_data = [ptr::null::<u8>(); 8];
            let mut src_linesize = [0i32; 8];
            for (i, (plane, &stride)) in planes.iter().zip(strides.iter()).enumerate().take(8) {
                src_data[i] = plane.as_ref().as_ptr();
                src_linesize[i] = stride as i32;
            }

            self.sws_ctx
                .as_mut()
                .ok_or_else(|| {
                    ffmpeg_err_msg(&format!("{} swscale context missing", self.log_prefix))
                })?
                .scale(
                    src_data.as_ptr(),
                    src_linesize.as_ptr(),
                    0,
                    src_h,
                    (*self.vid_enc_frame).data.as_mut_ptr().cast_const(),
                    (*self.vid_enc_frame).linesize.as_mut_ptr(),
                )
                .map_err(|_| {
                    ffmpeg_err_msg(&format!("{} swscale conversion failed", self.log_prefix))
                })?;
        } else {
            // Same format and dimensions — copy planes directly.
            (*self.vid_enc_frame).format = AVPixelFormat_AV_PIX_FMT_YUV420P;
            (*self.vid_enc_frame).width = self.enc_width;
            (*self.vid_enc_frame).height = self.enc_height;
            self.set_vid_enc_pts();

            let buf_ret = av_frame_get_buffer(self.vid_enc_frame, 0);
            if buf_ret < 0 {
                av_frame_unref(self.vid_enc_frame);
                return Err(ffmpeg_err(buf_ret));
            }

            // Copy Y/U/V planes line-by-line.
            let planes = frame.planes();
            let strides = frame.strides();
            for (plane_idx, (src_plane, &src_stride)) in
                planes.iter().zip(strides.iter()).enumerate().take(3)
            {
                let dst_stride = (*self.vid_enc_frame).linesize[plane_idx] as usize;
                let dst_ptr = (*self.vid_enc_frame).data[plane_idx];
                if dst_ptr.is_null() {
                    continue;
                }
                let rows = if plane_idx == 0 {
                    self.enc_height as usize
                } else {
                    (self.enc_height as usize).div_ceil(2)
                };
                let copy_width = dst_stride.min(src_stride);
                let src_bytes: &[u8] = src_plane.as_ref();
                for row in 0..rows {
                    let src_off = row * src_stride;
                    let dst_off = row * dst_stride;
                    if src_off + copy_width > src_bytes.len() {
                        break;
                    }
                    // SAFETY: dst_ptr + dst_off is within the allocated frame buffer.
                    ptr::copy_nonoverlapping(
                        src_bytes.as_ptr().add(src_off),
                        dst_ptr.add(dst_off),
                        copy_width,
                    );
                }
            }
        }

        if self.vid_enc_ctx.send_frame(self.vid_enc_frame).is_ok() {
            // SAFETY: vid_enc_ctx and out_ctx are valid; vid_out_stream_idx is valid.
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

        av_frame_unref(self.vid_enc_frame);
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
        let aud_ptr = match self.aud_enc_ctx.as_mut() {
            Some(c) => c.as_mut_ptr(),
            None => return,
        };

        let in_fmt = sample_format_to_av(frame.format());
        let in_rate = frame.sample_rate() as i32;
        let in_channels = frame.channels() as i32;

        // (Re)create SwrContext when input parameters change.
        if self.last_swr_in_fmt != Some(in_fmt)
            || self.last_swr_in_rate != Some(in_rate)
            || self.last_swr_in_channels != Some(in_channels)
        {
            let in_layout = ff_sys::swresample::channel_layout::with_channels(in_channels);
            let enc_ch_layout = &(*aud_ptr).ch_layout;

            // Move-assign the new resampler; the old one (if any) drops here.
            if let Ok(ctx) = ff_sys::ResampleContext::new(
                enc_ch_layout,
                ff_sys::swresample::sample_format::FLTP,
                self.aud_sample_rate,
                &in_layout,
                in_fmt,
                in_rate,
            ) {
                self.swr_ctx = Some(ctx);
                self.last_swr_in_fmt = Some(in_fmt);
                self.last_swr_in_rate = Some(in_rate);
                self.last_swr_in_channels = Some(in_channels);
            } else {
                log::warn!("{} swr alloc failed, dropping audio frame", self.log_prefix);
                return;
            }
        }

        // Prepare the encoder frame.
        (*self.aud_enc_frame).format = ff_sys::swresample::sample_format::FLTP;
        (*self.aud_enc_frame).sample_rate = self.aud_sample_rate;
        (*self.aud_enc_frame).nb_samples = self.aud_frame_size;
        let _ = ff_sys::swresample::channel_layout::copy(
            &mut (*self.aud_enc_frame).ch_layout,
            &(*aud_ptr).ch_layout,
        );

        let buf_ret = av_frame_get_buffer(self.aud_enc_frame, 0);
        if buf_ret < 0 {
            av_frame_unref(self.aud_enc_frame);
            return;
        }

        // Build input plane pointers from the AudioFrame.
        let planes = frame.planes();
        let mut in_data = [ptr::null::<u8>(); 8];
        for (i, plane) in planes.iter().enumerate().take(8) {
            in_data[i] = plane.as_ptr();
        }

        let dst_data = (*self.aud_enc_frame).data.as_mut_ptr();
        let out_count = self.aud_frame_size;
        let in_ptr = in_data.as_ptr();
        let in_count = frame.samples() as i32;
        let samples_out = if let Some(swr) = self.swr_ctx.as_mut() {
            swr.convert(dst_data, out_count, in_ptr, in_count).ok()
        } else {
            None
        };

        if let Some(n) = samples_out
            && n > 0
        {
            (*self.aud_enc_frame).nb_samples = n;
            (*self.aud_enc_frame).pts = self.audio_pts;
            if ff_sys::avcodec::send_frame(aud_ptr, self.aud_enc_frame).is_ok() {
                let aud_frame_period = AVRational {
                    num: (*aud_ptr).frame_size,
                    den: (*aud_ptr).sample_rate,
                };
                // SAFETY: aud_enc_ctx and out_ctx are valid; aud_out_stream_idx is valid.
                drain_encoder(
                    aud_ptr,
                    self.out_ctx,
                    self.aud_out_stream_idx,
                    self.log_prefix,
                    aud_frame_period,
                );
            }
            self.audio_pts += i64::from(n);
        }

        av_frame_unref(self.aud_enc_frame);
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
        if self.aud_out_stream_idx >= 0
            && let Some(aud_ptr) = self
                .aud_enc_ctx
                .as_mut()
                .map(ff_sys::CodecContext::as_mut_ptr)
        {
            // Drain any remaining resampler buffered samples.
            if self.swr_ctx.is_some() {
                (*self.aud_enc_frame).format = ff_sys::swresample::sample_format::FLTP;
                (*self.aud_enc_frame).sample_rate = self.aud_sample_rate;
                (*self.aud_enc_frame).nb_samples = self.aud_frame_size;
                let _ = ff_sys::swresample::channel_layout::copy(
                    &mut (*self.aud_enc_frame).ch_layout,
                    &(*aud_ptr).ch_layout,
                );

                if av_frame_get_buffer(self.aud_enc_frame, 0) == 0 {
                    let dst_data = (*self.aud_enc_frame).data.as_mut_ptr();
                    let out_count = self.aud_frame_size;
                    let flushed = if let Some(swr) = self.swr_ctx.as_mut() {
                        swr.convert(dst_data, out_count, ptr::null(), 0).ok()
                    } else {
                        None
                    };
                    if let Some(n) = flushed
                        && n > 0
                    {
                        (*self.aud_enc_frame).nb_samples = n;
                        (*self.aud_enc_frame).pts = self.audio_pts;
                        if ff_sys::avcodec::send_frame(aud_ptr, self.aud_enc_frame).is_ok() {
                            let aud_frame_period = AVRational {
                                num: (*aud_ptr).frame_size,
                                den: (*aud_ptr).sample_rate,
                            };
                            drain_encoder(
                                aud_ptr,
                                self.out_ctx,
                                self.aud_out_stream_idx,
                                self.log_prefix,
                                aud_frame_period,
                            );
                        }
                    }
                    av_frame_unref(self.aud_enc_frame);
                }
            }

            // Flush the AAC encoder itself.
            let _ = ff_sys::avcodec::send_frame(aud_ptr, ptr::null());
            let aud_frame_period = AVRational {
                num: (*aud_ptr).frame_size,
                den: (*aud_ptr).sample_rate,
            };
            drain_encoder(
                aud_ptr,
                self.out_ctx,
                self.aud_out_stream_idx,
                self.log_prefix,
                aud_frame_period,
            );
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

        // Zero all pointers so Drop does not double-free.
        self.free_all();
    }

    /// Set the PTS on `vid_enc_frame` from `video_frame_count` and `fps_int`.
    ///
    /// # Safety
    ///
    /// `vid_enc_frame` and `vid_enc_ctx` must be valid non-null pointers.
    unsafe fn set_vid_enc_pts(&mut self) {
        (*self.vid_enc_frame).pts = av_rescale_q(
            self.video_frame_count as i64,
            AVRational {
                num: 1,
                den: self.fps_int,
            },
            (*self.vid_enc_ctx.as_ptr()).time_base,
        );
    }

    /// Free all owned `FFmpeg` contexts and zero the pointers.
    ///
    /// Each pointer is checked for null before freeing. After this call all
    /// pointers are null, so a second call is a no-op.
    ///
    /// For RTMP/SRT (`close_pb_after_trailer = true`) closes `out_ctx.pb`
    /// before freeing `out_ctx` — handles the abnormal Drop path where
    /// `flush_and_close_unsafe` was never called.
    ///
    /// # Safety
    ///
    /// Must only be called when no other reference to the contained pointers exists.
    unsafe fn free_all(&mut self) {
        // Owned scale / resample / audio-codec contexts drop when set to `None`;
        // a second call re-`None`s them, so this is idempotent. The `vid_enc_ctx`
        // field is not an `Option` — it drops exactly once when the struct drops.
        self.sws_ctx = None;
        self.swr_ctx = None;
        if !self.vid_enc_frame.is_null() {
            av_frame_free(&mut (self.vid_enc_frame as *mut _));
            self.vid_enc_frame = ptr::null_mut();
        }
        if !self.aud_enc_frame.is_null() {
            av_frame_free(&mut (self.aud_enc_frame as *mut _));
            self.aud_enc_frame = ptr::null_mut();
        }
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
