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
    ptr,
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
        if self.pass1_codec_ctx.is_none() {
            return Err(EncodeError::InvalidConfig {
                reason: "Pass-1 codec context not available".to_string(),
            });
        }

        // SAFETY: the pass-1 context is valid and open. The flush send tolerates EOF.
        if let Err(e) = self
            .pass1_codec_ctx
            .as_mut()
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Pass-1 codec context not initialized".to_string(),
            })?
            .send_frame(ptr::null())
            && e.code() != ff_sys::error_codes::EOF
        {
            return Err(EncodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "pass1 flush send_frame: {}",
                    ff_sys::av_error_string(e.code())
                ),
            });
        }
        Self::drain_pass1_packets(self.pass1_codec_ctx.as_mut().ok_or_else(|| {
            EncodeError::InvalidConfig {
                reason: "Pass-1 codec context not initialized".to_string(),
            }
        })?)?;

        // ── Step 2: Collect stats_out (before freeing pass 1) ────────────────
        let stats_out = self
            .pass1_codec_ctx
            .as_ref()
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Pass-1 codec context not initialized".to_string(),
            })?
            .stats_out();
        let stats_str = if let Some(stats) = stats_out {
            stats.to_string_lossy().into_owned()
        } else {
            log::warn!(
                "two-pass pass-1 produced no stats_out; pass-2 quality may not improve \
                 codec={}",
                self.actual_video_codec
            );
            String::new()
        };
        log::info!("two-pass pass-1 complete stats_len={}", stats_str.len());

        // ── Step 3: Free pass-1 codec context (owned context drops → frees) ──
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
        self.format_ctx
            .open_io(&output_path)
            .map_err(|_| EncodeError::CannotCreateFile { path: output_path })?;

        let fmt = self.format_ctx.as_mut_ptr();
        Self::apply_movflags(fmt, config.container);
        Self::apply_metadata(fmt, &config.metadata);
        Self::apply_chapters(fmt, &config.chapters);
        self.format_ctx
            .write_header()
            .map_err(|e| EncodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Cannot write header in pass 2: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

        // ── Step 6: Re-encode all buffered frames ────────────────────────────
        let frames = std::mem::take(&mut self.buffered_frames);
        self.frame_count = 0;
        for tf in &frames {
            self.push_two_pass_frame(tf)?;
        }

        // ── Step 7: Flush pass-2 encoder and write trailer ───────────────────
        if self.video_codec_ctx.is_some() {
            // SAFETY: the pass-2 codec context is valid and open. Flush tolerates EOF.
            if let Err(e) = self
                .video_codec_ctx
                .as_mut()
                .ok_or_else(|| EncodeError::InvalidConfig {
                    reason: "Video codec not initialized".to_string(),
                })?
                .send_frame(ptr::null())
                && e.code() != ff_sys::error_codes::EOF
            {
                return Err(EncodeError::Ffmpeg {
                    code: e.code(),
                    message: format!(
                        "pass2 flush send_frame: {}",
                        ff_sys::av_error_string(e.code())
                    ),
                });
            }
            self.receive_packets()?;
        }

        // Write subtitle passthrough packets before trailer.
        self.write_subtitle_packets()?;

        self.format_ctx
            .write_trailer()
            .map_err(|e| EncodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Cannot write trailer: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

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

        let selected_codec =
            ff_sys::Codec::find_encoder_by_name(&encoder_name).ok_or_else(|| {
                EncodeError::NoSuitableEncoder {
                    codec: encoder_name.clone(),
                    tried: vec![encoder_name.clone()],
                }
            })?;

        let mut codec_ctx = ff_sys::CodecContext::new(Some(selected_codec))
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // Mirror the same codec configuration as pass 1.
        codec_ctx.set_codec_id(codec_to_id(config.video_codec));
        codec_ctx.set_width(width as i32);
        codec_ctx.set_height(height as i32);
        codec_ctx.set_time_base(ff_sys::AVRational {
            num: 1,
            den: (fps * 1000.0) as i32,
        });
        codec_ctx.set_framerate(ff_sys::AVRational {
            num: fps as i32,
            den: 1,
        });
        codec_ctx.set_pix_fmt(AVPixelFormat_AV_PIX_FMT_YUV420P);

        match config.video_bitrate_mode.as_ref() {
            Some(BitrateMode::Cbr(bps)) => {
                codec_ctx.set_bit_rate(*bps as i64);
            }
            Some(BitrateMode::Vbr { target, max }) => {
                codec_ctx.set_bit_rate(*target as i64);
                codec_ctx.set_rc_max_rate(*max as i64);
                codec_ctx.set_rc_buffer_size((*max * 2) as i32);
            }
            Some(BitrateMode::Crf(q)) => {
                if codec_ctx.set_opt("crf", &q.to_string()).is_err() {
                    log::warn!(
                        "crf option not supported by pass-2 encoder, falling back to default \
                         encoder={encoder_name} crf={q}"
                    );
                    codec_ctx.set_bit_rate(2_000_000);
                }
            }
            None => {
                codec_ctx.set_bit_rate(2_000_000);
            }
        }

        if (encoder_name.contains("264") || encoder_name.contains("265"))
            && codec_ctx.set_opt("preset", config.preset.as_str()).is_err()
        {
            log::warn!(
                "preset option not supported by pass-2 encoder, ignoring \
                 encoder={encoder_name} preset={}",
                config.preset
            );
        }

        // Apply per-codec options before opening the pass-2 codec context.
        if let Some(opts) = config.codec_options.as_ref() {
            // Options are applied before avcodec_open2 so they take effect during
            // codec initialisation.
            Self::apply_codec_options(&mut codec_ctx, opts, &encoder_name);
        }

        // Apply explicit pixel format override for pass 2 (mirrors pass 1).
        if let Some(fmt) = config.pixel_format.as_ref() {
            codec_ctx.set_pix_fmt(pixel_format_to_av(*fmt));
        }

        // Apply HDR10 color context for pass 2 (mirrors pass 1).
        if config.hdr10_metadata.is_some() {
            codec_ctx.set_color_primaries(ff_sys::AVColorPrimaries_AVCOL_PRI_BT2020);
            codec_ctx.set_color_trc(ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_SMPTEST2084);
            codec_ctx.set_colorspace(ff_sys::AVColorSpace_AVCOL_SPC_BT2020_NCL);
        }

        // Apply explicit color overrides for pass 2 (mirrors pass 1; take priority over HDR10).
        if let Some(cs) = config.color_space {
            codec_ctx.set_colorspace(color_space_to_av(cs));
        }
        if let Some(trc) = config.color_transfer {
            codec_ctx.set_color_trc(color_transfer_to_av(trc));
        }
        if let Some(cp) = config.color_primaries {
            codec_ctx.set_color_primaries(color_primaries_to_av(cp));
        }

        // Set the pass-2 flag and provide stats_in.
        codec_ctx.set_flags(codec_ctx.flags() | AV_CODEC_FLAG_PASS2);

        // Hand the pass-1 statistics to the encoder. The CodecContext takes an
        // owned copy and keeps it alive; its Drop nulls the raw field before the
        // context is freed.
        if !stats.is_empty() {
            let stats_cstr = CString::new(stats).map_err(|_| EncodeError::Ffmpeg {
                code: 0,
                message: "Invalid stats string from pass 1".to_string(),
            })?;
            codec_ctx.set_stats_in(&stats_cstr);
        }

        // Try to open the pass-2 codec with PASS2 flag. Some encoders (e.g. the
        // native mpeg4 encoder without meaningful stats) do not support PASS2 and
        // return AVERROR(EPERM). In that case, fall back to opening without the
        // flag so the caller still gets a valid encoder and usable output.
        if codec_ctx.open(selected_codec, ptr::null_mut()).is_err() {
            log::warn!(
                "two-pass pass-2 codec rejected AV_CODEC_FLAG_PASS2, \
                 falling back to single-pass mode codec={encoder_name}"
            );
            // Null stats_in and drop the owned CString BEFORE re-opening so the
            // discarded pass-2 attempt never references the Rust-owned pointer.
            codec_ctx.set_flags(codec_ctx.flags() & !AV_CODEC_FLAG_PASS2);
            codec_ctx.clear_stats_in();
            codec_ctx
                .open(selected_codec, ptr::null_mut())
                .map_err(|e| EncodeError::Ffmpeg {
                    code: e.code(),
                    message: format!(
                        "pass2 avcodec_open2 fallback: {}",
                        ff_sys::av_error_string(e.code())
                    ),
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
        if self.video_codec_ctx.is_none() {
            return Err(EncodeError::InvalidConfig {
                reason: "Pass-2 codec context not initialized".to_string(),
            });
        }

        let mut av_frame = ff_sys::Frame::new().map_err(|_| EncodeError::Ffmpeg {
            code: 0,
            message: "Cannot allocate frame for pass 2".to_string(),
        })?;

        // Set frame format — always YUV420P (converted during pass 1).
        av_frame.set_format(AVPixelFormat_AV_PIX_FMT_YUV420P);
        av_frame.set_width(tf.width as i32);
        av_frame.set_height(tf.height as i32);

        // Allocate the frame buffer.
        av_frame.get_buffer(0).map_err(|e| EncodeError::Ffmpeg {
            code: e.code(),
            message: format!(
                "Cannot allocate pass-2 frame buffer: {}",
                ff_sys::av_error_string(e.code())
            ),
        })?;

        // Copy the buffered YUV420P data into the AVFrame. `video_plane_mut`
        // self-sizes each destination plane (Y full height, U/V by subsampling)
        // and yields `None` for an absent plane.
        for (plane_idx, (plane_data, src_stride)) in
            tf.planes.iter().zip(tf.strides.iter()).enumerate()
        {
            if plane_idx >= 3 || plane_data.is_empty() {
                break;
            }
            // SAFETY: `av_frame` is a valid get_buffer'd frame; `linesize` is a plain field.
            let dst_stride = (*av_frame.as_ptr()).linesize[plane_idx] as usize;
            let Some(dst_plane) = av_frame.video_plane_mut(plane_idx) else {
                break;
            };

            let num_rows = dst_plane.len() / dst_stride;
            for row in 0..num_rows {
                let src_off = row * src_stride;
                let dst_off = row * dst_stride;
                let copy_len = (*src_stride).min(dst_stride);

                if src_off + copy_len <= plane_data.len() {
                    dst_plane[dst_off..dst_off + copy_len]
                        .copy_from_slice(&plane_data[src_off..src_off + copy_len]);
                }
            }
        }

        av_frame.set_pts(tf.pts);

        // Send to pass-2 encoder.
        self.video_codec_ctx
            .as_mut()
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Video codec not initialized".to_string(),
            })?
            .send_frame(av_frame.as_ptr())
            .map_err(|e| EncodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Failed to send frame to pass-2 encoder: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

        // Receive packets (the owned `av_frame` drops at end of scope).
        self.receive_packets()?;

        self.frame_count += 1;
        Ok(())
    }
}
