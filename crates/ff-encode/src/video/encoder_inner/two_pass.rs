//! Two-pass encoding helpers.
#![allow(unsafe_op_in_unsafe_fn)]
// FFmpeg-boundary lints: casts at the C ABI, pointer idioms, C-string
// literals, and FFI-wrapper ergonomics concentrate in this unsafe module.
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::manual_c_str_literals)]
#![allow(clippy::unused_self)]

use super::color::{
    color_primaries_to_av, color_space_to_av, color_transfer_to_av, pixel_format_to_av,
};
use super::options::codec_to_id;
use super::{
    AVPixelFormat_AV_PIX_FMT_YUV420P, CString, EncodeError, VideoEncoderConfig, VideoEncoderInner,
    av_frame_alloc, av_frame_free, av_write_trailer, avcodec, avformat_write_header, ptr,
};

/// FFmpeg pass-1 encoding flag: collect two-pass statistics, discard encoded output.
pub(super) const AV_CODEC_FLAG_PASS1: i32 = 512; // 1 << 9

/// FFmpeg pass-2 encoding flag: use two-pass statistics from pass 1.
pub(super) const AV_CODEC_FLAG_PASS2: i32 = 1024; // 1 << 10

/// Buffered raw frame data for two-pass re-encoding.
///
/// Stores the already-converted YUV420P plane data from pass 1 so that
/// the same frames can be re-encoded in pass 2 without re-reading from
/// the caller.
pub struct TwoPassFrame {
    /// YUV420P plane data (Y plane at index 0, U at 1, V at 2).
    pub(super) planes: Vec<Vec<u8>>,
    /// Linesize (stride) for each plane.
    pub(super) strides: Vec<usize>,
    /// Frame width in pixels.
    pub(super) width: u32,
    /// Frame height in pixels.
    pub(super) height: u32,
    /// Presentation timestamp used when encoding this frame.
    pub(super) pts: i64,
}

impl VideoEncoderInner {
    /// Run the second pass of two-pass encoding.
    ///
    /// 1. Flushes the pass-1 encoder and collects `stats_out`.
    /// 2. Initialises a pass-2 codec context with `AV_CODEC_FLAG_PASS2` and
    ///    the collected statistics.
    /// 3. Opens the real output file and writes the container header.
    /// 4. Re-encodes all buffered frames through the pass-2 context.
    /// 5. Flushes the pass-2 encoder and writes the container trailer.
    ///
    /// # Safety
    ///
    /// Must only be called from `finish` when `self.two_pass` is `true`.
    /// All FFmpeg resources must be valid at the point of the call.
    pub(super) unsafe fn run_pass2(&mut self) -> Result<(), EncodeError> {
        // ── Step 1: Flush pass-1 encoder ────────────────────────────────────
        let mut pass1_ctx = self
            .pass1_codec_ctx
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Pass-1 codec context not available".to_string(),
            })?;

        // SAFETY: pass1_ctx is a valid open codec context.
        if let Err(e) = avcodec::send_frame(pass1_ctx, ptr::null())
            && e != ff_sys::error_codes::EOF
        {
            return Err(EncodeError::Ffmpeg {
                code: e,
                message: format!("pass1 flush send_frame: {}", ff_sys::av_error_string(e)),
            });
        }
        self.drain_pass1_packets(pass1_ctx)?;

        // ── Step 2: Collect stats_out ────────────────────────────────────────
        // SAFETY: stats_out is either null or a valid C string owned by the
        // codec context; it remains valid until avcodec_free_context is called.
        let stats_str = if !(*pass1_ctx).stats_out.is_null() {
            std::ffi::CStr::from_ptr((*pass1_ctx).stats_out)
                .to_string_lossy()
                .into_owned()
        } else {
            log::warn!(
                "two-pass pass-1 produced no stats_out; pass-2 quality may not improve \
                 codec={}",
                self.actual_video_codec
            );
            String::new()
        };
        log::info!("two-pass pass-1 complete stats_len={}", stats_str.len());

        // ── Step 3: Free pass-1 codec context ───────────────────────────────
        // SAFETY: pass1_ctx is no longer needed; we own it exclusively.
        avcodec::free_context(&raw mut pass1_ctx);
        self.pass1_codec_ctx = None;

        // ── Step 4: Set up pass-2 codec context ─────────────────────────────
        let config = self
            .two_pass_config
            .take()
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Two-pass config not available for pass-2 initialisation".to_string(),
            })?;

        let output_path = config.path.clone();
        self.init_pass2_codec_ctx(&config, &stats_str)?;

        // ── Step 5: Open output file and write header ────────────────────────
        match ff_sys::avformat::open_output(&output_path, ff_sys::avformat::avio_flags::WRITE) {
            Ok(pb) => (*self.format_ctx).pb = pb,
            Err(_) => {
                return Err(EncodeError::CannotCreateFile { path: output_path });
            }
        }

        Self::apply_movflags(self.format_ctx, config.container);
        Self::apply_metadata(self.format_ctx, &config.metadata);
        Self::apply_chapters(self.format_ctx, &config.chapters);
        let ret = avformat_write_header(self.format_ctx, ptr::null_mut());
        if ret < 0 {
            return Err(EncodeError::Ffmpeg {
                code: ret,
                message: format!(
                    "Cannot write header in pass 2: {}",
                    ff_sys::av_error_string(ret)
                ),
            });
        }

        // ── Step 6: Re-encode all buffered frames ────────────────────────────
        let frames = std::mem::take(&mut self.buffered_frames);
        self.frame_count = 0;
        for tf in &frames {
            self.push_two_pass_frame(tf)?;
        }

        // ── Step 7: Flush pass-2 encoder and write trailer ───────────────────
        if let Some(codec_ctx) = self.video_codec_ctx {
            // SAFETY: codec_ctx is a valid open pass-2 codec context.
            if let Err(e) = avcodec::send_frame(codec_ctx, ptr::null())
                && e != ff_sys::error_codes::EOF
            {
                return Err(EncodeError::Ffmpeg {
                    code: e,
                    message: format!("pass2 flush send_frame: {}", ff_sys::av_error_string(e)),
                });
            }
            self.receive_packets()?;
        }

        // Write subtitle passthrough packets before trailer.
        self.write_subtitle_packets()?;

        let ret = av_write_trailer(self.format_ctx);
        if ret < 0 {
            return Err(EncodeError::Ffmpeg {
                code: ret,
                message: format!("Cannot write trailer: {}", ff_sys::av_error_string(ret)),
            });
        }

        Ok(())
    }

    /// Initialise the pass-2 video codec context.
    ///
    /// Mirrors the configuration performed in `init_video_encoder` but sets
    /// `AV_CODEC_FLAG_PASS2` and assigns `stats_in` from the pass-1 statistics
    /// string. Does **not** create a new AVStream — the stream was already
    /// registered during `init_video_encoder` (pass 1).
    ///
    /// # Safety
    ///
    /// Must only be called from `run_pass2`. `self.format_ctx` must be valid.
    unsafe fn init_pass2_codec_ctx(
        &mut self,
        config: &VideoEncoderConfig,
        stats: &str,
    ) -> Result<(), EncodeError> {
        use crate::BitrateMode;
        let width = config.video_width.unwrap_or(0);
        let height = config.video_height.unwrap_or(0);
        let fps = config.video_fps.unwrap_or(30.0);
        let encoder_name = self.actual_video_codec.clone();

        let c_encoder_name =
            CString::new(encoder_name.as_str()).map_err(|_| EncodeError::Ffmpeg {
                code: 0,
                message: "Invalid encoder name for pass 2".to_string(),
            })?;

        let codec_ptr =
            avcodec::find_encoder_by_name(c_encoder_name.as_ptr()).ok_or_else(|| {
                EncodeError::NoSuitableEncoder {
                    codec: encoder_name.clone(),
                    tried: vec![encoder_name.clone()],
                }
            })?;

        let codec_ctx =
            avcodec::alloc_context3(codec_ptr).map_err(EncodeError::from_ffmpeg_error)?;

        // Mirror the same codec configuration as pass 1.
        (*codec_ctx).codec_id = codec_to_id(config.video_codec);
        (*codec_ctx).width = width as i32;
        (*codec_ctx).height = height as i32;
        (*codec_ctx).time_base.num = 1;
        (*codec_ctx).time_base.den = (fps * 1000.0) as i32;
        (*codec_ctx).framerate.num = fps as i32;
        (*codec_ctx).framerate.den = 1;
        (*codec_ctx).pix_fmt = AVPixelFormat_AV_PIX_FMT_YUV420P;

        match config.video_bitrate_mode.as_ref() {
            Some(BitrateMode::Cbr(bps)) => {
                (*codec_ctx).bit_rate = *bps as i64;
            }
            Some(BitrateMode::Vbr { target, max }) => {
                (*codec_ctx).bit_rate = *target as i64;
                (*codec_ctx).rc_max_rate = *max as i64;
                (*codec_ctx).rc_buffer_size = (*max * 2) as i32;
            }
            Some(BitrateMode::Crf(q)) => {
                let crf_str = CString::new(q.to_string()).map_err(|_| EncodeError::Ffmpeg {
                    code: 0,
                    message: "Invalid CRF value".to_string(),
                })?;
                // SAFETY: priv_data, option name, and value are all valid pointers.
                let ret = ff_sys::av_opt_set(
                    (*codec_ctx).priv_data,
                    b"crf\0".as_ptr() as *const i8,
                    crf_str.as_ptr(),
                    0,
                );
                if ret < 0 {
                    log::warn!(
                        "crf option not supported by pass-2 encoder, falling back to default \
                         encoder={encoder_name} crf={q}"
                    );
                    (*codec_ctx).bit_rate = 2_000_000;
                }
            }
            None => {
                (*codec_ctx).bit_rate = 2_000_000;
            }
        }

        if encoder_name.contains("264") || encoder_name.contains("265") {
            let preset_cstr =
                CString::new(config.preset.as_str()).map_err(|_| EncodeError::Ffmpeg {
                    code: 0,
                    message: "Invalid preset value".to_string(),
                })?;
            // SAFETY: priv_data, option name, and value are all valid pointers.
            let ret = ff_sys::av_opt_set(
                (*codec_ctx).priv_data,
                b"preset\0".as_ptr() as *const i8,
                preset_cstr.as_ptr(),
                0,
            );
            if ret < 0 {
                log::warn!(
                    "preset option not supported by pass-2 encoder, ignoring \
                     encoder={encoder_name} preset={}",
                    config.preset
                );
            }
        }

        // Apply per-codec options before opening the pass-2 codec context.
        if let Some(opts) = config.codec_options.as_ref() {
            // SAFETY: codec_ctx is valid and allocated; priv_data is set by
            // avcodec_alloc_context3. Options are applied before avcodec_open2
            // so they take effect during codec initialisation.
            Self::apply_codec_options(codec_ctx, opts, &encoder_name);
        }

        // Apply explicit pixel format override for pass 2 (mirrors pass 1).
        if let Some(fmt) = config.pixel_format.as_ref() {
            // SAFETY: codec_ctx is valid and allocated; direct field write is safe.
            (*codec_ctx).pix_fmt = pixel_format_to_av(*fmt);
        }

        // Apply HDR10 color context for pass 2 (mirrors pass 1).
        if config.hdr10_metadata.is_some() {
            // SAFETY: codec_ctx is valid and allocated; direct field writes are safe.
            (*codec_ctx).color_primaries = ff_sys::AVColorPrimaries_AVCOL_PRI_BT2020;
            (*codec_ctx).color_trc = ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_SMPTEST2084;
            (*codec_ctx).colorspace = ff_sys::AVColorSpace_AVCOL_SPC_BT2020_NCL;
        }

        // Apply explicit color overrides for pass 2 (mirrors pass 1; take priority over HDR10).
        if let Some(cs) = config.color_space {
            // SAFETY: codec_ctx is valid and allocated; direct field write is safe.
            (*codec_ctx).colorspace = color_space_to_av(cs);
        }
        if let Some(trc) = config.color_transfer {
            // SAFETY: codec_ctx is valid and allocated; direct field write is safe.
            (*codec_ctx).color_trc = color_transfer_to_av(trc);
        }
        if let Some(cp) = config.color_primaries {
            // SAFETY: codec_ctx is valid and allocated; direct field write is safe.
            (*codec_ctx).color_primaries = color_primaries_to_av(cp);
        }

        // Set the pass-2 flag and provide stats_in.
        // SAFETY: codec_ctx is a valid allocated (but not yet opened) context.
        (*codec_ctx).flags |= AV_CODEC_FLAG_PASS2;

        // Point stats_in to our owned CString (kept alive in self.stats_in_cstr
        // until cleanup() nulls the pointer and drops it).
        if !stats.is_empty() {
            let stats_cstr = CString::new(stats).map_err(|_| EncodeError::Ffmpeg {
                code: 0,
                message: "Invalid stats string from pass 1".to_string(),
            })?;
            // SAFETY: stats_cstr.as_ptr() is valid for the lifetime of stats_cstr,
            // which is stored in self.stats_in_cstr and dropped only after the codec
            // context is freed in cleanup().
            (*codec_ctx).stats_in = stats_cstr.as_ptr().cast_mut();
            self.stats_in_cstr = Some(stats_cstr);
        }

        // Try to open the pass-2 codec with PASS2 flag. Some encoders (e.g. the
        // native mpeg4 encoder without meaningful stats) do not support PASS2 and
        // return AVERROR(EPERM). In that case, fall back to opening without the
        // flag so the caller still gets a valid encoder and usable output.
        if avcodec::open2(codec_ctx, codec_ptr, ptr::null_mut()).is_err() {
            log::warn!(
                "two-pass pass-2 codec rejected AV_CODEC_FLAG_PASS2, \
                 falling back to single-pass mode codec={encoder_name}"
            );
            (*codec_ctx).flags &= !AV_CODEC_FLAG_PASS2;
            (*codec_ctx).stats_in = ptr::null_mut();
            self.stats_in_cstr = None;
            avcodec::open2(codec_ctx, codec_ptr, ptr::null_mut()).map_err(|e| {
                EncodeError::Ffmpeg {
                    code: e,
                    message: format!(
                        "pass2 avcodec_open2 fallback: {}",
                        ff_sys::av_error_string(e)
                    ),
                }
            })?;
        }
        log::info!(
            "two-pass pass-2 codec opened codec={encoder_name} width={width} height={height}"
        );

        self.video_codec_ctx = Some(codec_ctx);
        Ok(())
    }

    /// Encode a single buffered YUV420P frame through the pass-2 codec context.
    ///
    /// The frame data was captured during pass 1 (already converted to YUV420P)
    /// and is re-encoded here with the optimised pass-2 settings.
    ///
    /// # Safety
    ///
    /// Must only be called from `run_pass2`. `self.video_codec_ctx` and
    /// `self.format_ctx` must be valid and the output file must be open.
    unsafe fn push_two_pass_frame(&mut self, tf: &TwoPassFrame) -> Result<(), EncodeError> {
        let codec_ctx = self
            .video_codec_ctx
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Pass-2 codec context not initialized".to_string(),
            })?;

        let mut av_frame = av_frame_alloc();
        if av_frame.is_null() {
            return Err(EncodeError::Ffmpeg {
                code: 0,
                message: "Cannot allocate frame for pass 2".to_string(),
            });
        }

        // Set frame format — always YUV420P (converted during pass 1).
        (*av_frame).format = AVPixelFormat_AV_PIX_FMT_YUV420P;
        (*av_frame).width = tf.width as i32;
        (*av_frame).height = tf.height as i32;

        // Allocate the frame buffer.
        let ret = ff_sys::av_frame_get_buffer(av_frame, 0);
        if ret < 0 {
            av_frame_free(&raw mut av_frame);
            return Err(EncodeError::Ffmpeg {
                code: ret,
                message: format!(
                    "Cannot allocate pass-2 frame buffer: {}",
                    ff_sys::av_error_string(ret)
                ),
            });
        }

        // Copy the buffered YUV420P data into the AVFrame.
        let uv_height = (tf.height as usize).div_ceil(2);
        for (plane_idx, (plane_data, src_stride)) in
            tf.planes.iter().zip(tf.strides.iter()).enumerate()
        {
            if plane_idx >= 3 || (*av_frame).data[plane_idx].is_null() || plane_data.is_empty() {
                break;
            }
            let dst_stride = (*av_frame).linesize[plane_idx] as usize;
            let plane_height = if plane_idx == 0 {
                tf.height as usize
            } else {
                uv_height
            };

            for row in 0..plane_height {
                let src_off = row * src_stride;
                let dst_off = row * dst_stride;
                let copy_len = (*src_stride).min(dst_stride);

                if src_off + copy_len <= plane_data.len() {
                    // SAFETY: bounds checked above; both pointers are valid and
                    // the regions do not overlap.
                    ptr::copy_nonoverlapping(
                        plane_data.as_ptr().add(src_off),
                        (*av_frame).data[plane_idx].add(dst_off),
                        copy_len,
                    );
                }
            }
        }

        (*av_frame).pts = tf.pts;

        // Send to pass-2 encoder.
        let send_result = avcodec::send_frame(codec_ctx, av_frame);
        if let Err(e) = send_result {
            av_frame_free(&raw mut av_frame);
            return Err(EncodeError::Ffmpeg {
                code: e,
                message: format!(
                    "Failed to send frame to pass-2 encoder: {}",
                    ff_sys::av_error_string(e)
                ),
            });
        }

        let receive_result = self.receive_packets();
        av_frame_free(&raw mut av_frame);
        receive_result?;

        self.frame_count += 1;
        Ok(())
    }
}
