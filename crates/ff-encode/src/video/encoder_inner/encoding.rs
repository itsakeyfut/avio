//! Frame conversion and packet reception helpers.
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

use super::color::{pixel_format_to_av, sample_format_to_av};
use super::{
    AVChannelLayout, AVCodecContext, AVFrame, AVPixelFormat, AudioFrame, EncodeError,
    VideoEncoderInner, VideoFrame, av_interleaved_write_frame, av_packet_alloc, av_packet_free,
    av_packet_unref, ptr, swresample, swscale,
};

/// Maximum number of planes in AVFrame data/linesize arrays.
///
/// This corresponds to FFmpeg's `AV_NUM_DATA_POINTERS` (typically 8).
/// Most pixel formats use 1-3 planes (e.g., RGB uses 1, YUV420P uses 3),
/// but this allows for future extensibility and compatibility with
/// exotic formats that may require more planes.
pub(super) const MAX_PLANES: usize = 8;

impl VideoEncoderInner {
    /// Drain and discard all pending packets from a codec context.
    ///
    /// Used during pass-1 of two-pass encoding to prevent the packet queue
    /// from filling up without writing any data to the output file.
    ///
    /// # Safety
    ///
    /// `codec_ctx` must be a valid, open `AVCodecContext`.
    pub(super) unsafe fn drain_pass1_packets(
        codec_ctx: &mut ff_sys::CodecContext,
    ) -> Result<(), EncodeError> {
        let mut packet = av_packet_alloc();
        if packet.is_null() {
            return Err(EncodeError::Ffmpeg {
                code: 0,
                message: "Cannot allocate packet".to_string(),
            });
        }

        loop {
            match codec_ctx.receive_packet(packet) {
                Ok(ff_sys::ReceiveOutcome::Frame) => {
                    // Discard — do not write to the format context.
                    av_packet_unref(packet);
                }
                Ok(ff_sys::ReceiveOutcome::NeedInput | ff_sys::ReceiveOutcome::Drained) => {
                    break;
                }
                Err(e) => {
                    av_packet_free(&raw mut packet);
                    return Err(EncodeError::Ffmpeg {
                        code: e.code(),
                        message: format!(
                            "Error receiving packet from pass-1 encoder: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    });
                }
            }
        }

        av_packet_free(&raw mut packet);
        Ok(())
    }

    /// Convert VideoFrame to AVFrame with pixel format conversion if needed.
    ///
    /// This method implements several optimizations in priority order:
    /// 1. **Fast path**: Skips conversion entirely if format/dimensions match
    /// 2. **Context reuse**: Reuses SwsContext when source properties unchanged
    /// 3. **Lazy init**: Reinitializes SwsContext only when needed
    /// 4. **Fast algorithm**: Uses BILINEAR scaling for speed/quality balance
    ///
    /// The caller supplies `codec_ctx` explicitly so this function can be used
    /// with both the regular `video_codec_ctx` and the pass-1 `pass1_codec_ctx`.
    ///
    /// # Performance Characteristics
    ///
    /// - Same format/size: ~0.1ms (direct memory copy only)
    /// - Different format/size with context reuse: ~2-5ms
    /// - Different format/size with context reinit: ~5-10ms
    ///
    /// # Safety
    ///
    /// This function is unsafe because it directly manipulates FFmpeg AVFrame pointers.
    /// The caller must ensure that `dst` is a valid, properly allocated AVFrame pointer
    /// and that `codec_ctx` is a valid, open `AVCodecContext`.
    pub(super) unsafe fn convert_video_frame(
        &mut self,
        src: &VideoFrame,
        dst: *mut AVFrame,
        codec_ctx: *mut AVCodecContext,
    ) -> Result<(), EncodeError> {
        let target_fmt = (*codec_ctx).pix_fmt;
        let target_width = (*codec_ctx).width as u32;
        let target_height = (*codec_ctx).height as u32;

        let src_fmt = pixel_format_to_av(src.format());
        let src_width = src.width();
        let src_height = src.height();

        // Optimization 1: Skip conversion if format and dimensions match
        if src_fmt == target_fmt && src_width == target_width && src_height == target_height {
            return self.copy_frame_direct(src, dst, target_fmt);
        }

        // Optimization 2 & 3: Check if we need to reinitialize SwsContext
        let needs_new_context = self.last_src_width != Some(src_width)
            || self.last_src_height != Some(src_height)
            || self.last_src_format != Some(src_fmt);

        if needs_new_context || self.sws_ctx.is_none() {
            // Free old context if exists
            if let Some(ctx) = self.sws_ctx.take() {
                swscale::free_context(ctx);
            }

            // Create new SwsContext with fast BILINEAR algorithm
            let sws = swscale::get_context(
                src_width as i32,
                src_height as i32,
                src_fmt,
                target_width as i32,
                target_height as i32,
                target_fmt,
                ff_sys::swscale::scale_flags::BILINEAR, // Fast scaling algorithm
            )
            .map_err(EncodeError::from_ffmpeg_error)?;

            self.sws_ctx = Some(sws);
            self.last_src_width = Some(src_width);
            self.last_src_height = Some(src_height);
            self.last_src_format = Some(src_fmt);
        }

        // Perform conversion using cached SwsContext
        self.scale_frame(src, dst, target_fmt, target_width, target_height)
    }

    /// Copy frame data directly without scaling (when formats match).
    pub(super) unsafe fn copy_frame_direct(
        &self,
        src: &VideoFrame,
        dst: *mut AVFrame,
        target_fmt: AVPixelFormat,
    ) -> Result<(), EncodeError> {
        // Set frame properties
        (*dst).format = target_fmt;
        (*dst).width = src.width() as i32;
        (*dst).height = src.height() as i32;

        // Allocate frame buffer
        let ret = ff_sys::av_frame_get_buffer(dst, 0);
        if ret < 0 {
            return Err(EncodeError::Ffmpeg {
                code: ret,
                message: format!(
                    "Cannot allocate frame buffer: {}",
                    ff_sys::av_error_string(ret)
                ),
            });
        }

        // Copy each plane directly
        for (i, plane) in src.planes().iter().enumerate() {
            if i >= (*dst).data.len() || (*dst).data[i].is_null() {
                break;
            }

            // Bounds check for strides array
            let src_stride = src
                .strides()
                .get(i)
                .copied()
                .ok_or_else(|| EncodeError::Ffmpeg {
                    code: 0,
                    message: format!("Missing stride for plane {}", i),
                })?;

            let dst_stride = (*dst).linesize[i] as usize;
            let plane_data = plane.data();
            let plane_height = self.get_plane_height(src.height(), i, src.format());

            // Optimization: If strides match, copy entire plane at once
            if src_stride == dst_stride {
                let total_size = src_stride * plane_height;
                if total_size <= plane_data.len() {
                    ptr::copy_nonoverlapping(plane_data.as_ptr(), (*dst).data[i], total_size);
                    continue;
                }
            }

            // Copy line by line to handle different strides
            for y in 0..plane_height {
                let src_offset = y * src_stride;
                let dst_offset = y * dst_stride;
                let line_size = src_stride.min(dst_stride);

                if src_offset + line_size <= plane_data.len() {
                    ptr::copy_nonoverlapping(
                        plane_data.as_ptr().add(src_offset),
                        (*dst).data[i].add(dst_offset),
                        line_size,
                    );
                }
            }
        }

        Ok(())
    }

    /// Scale frame using SwsContext (when formats or dimensions differ).
    pub(super) unsafe fn scale_frame(
        &self,
        src: &VideoFrame,
        dst: *mut AVFrame,
        target_fmt: AVPixelFormat,
        target_width: u32,
        target_height: u32,
    ) -> Result<(), EncodeError> {
        // Set frame properties
        (*dst).format = target_fmt;
        (*dst).width = target_width as i32;
        (*dst).height = target_height as i32;

        // Allocate frame buffer
        let ret = ff_sys::av_frame_get_buffer(dst, 0);
        if ret < 0 {
            return Err(EncodeError::Ffmpeg {
                code: ret,
                message: format!(
                    "Cannot allocate frame buffer: {}",
                    ff_sys::av_error_string(ret)
                ),
            });
        }

        // Prepare source data pointers and strides
        let mut src_data: [*const u8; MAX_PLANES] = [ptr::null(); MAX_PLANES];
        let mut src_linesize: [i32; MAX_PLANES] = [0; MAX_PLANES];

        for (i, plane) in src.planes().iter().enumerate() {
            if i < MAX_PLANES {
                src_data[i] = plane.data().as_ptr();
                src_linesize[i] = src.strides()[i] as i32;
            }
        }

        // Perform scaling/conversion
        let sws_ctx = self.sws_ctx.ok_or_else(|| EncodeError::Ffmpeg {
            code: 0,
            message: "Scaling context not initialized".to_string(),
        })?;

        swscale::scale(
            sws_ctx,
            src_data.as_ptr(),
            src_linesize.as_ptr(),
            0,
            src.height() as i32,
            (*dst).data.as_mut_ptr().cast_const(),
            (*dst).linesize.as_mut_ptr(),
        )
        .map_err(EncodeError::from_ffmpeg_error)?;

        Ok(())
    }

    /// Calculate the height of a plane for a given frame height and pixel format.
    ///
    /// Different pixel formats have different plane heights. For YUV 4:2:0 formats,
    /// the U/V planes are half the height of the Y plane.
    ///
    /// # Arguments
    ///
    /// * `frame_height` - The height of the entire frame
    /// * `plane_index` - The plane index (0: Y/RGB, 1: U/UV, 2: V)
    /// * `format` - The pixel format
    ///
    /// # Returns
    ///
    /// The height (number of rows) for the specified plane.
    #[allow(clippy::manual_div_ceil)]
    pub(super) fn get_plane_height(
        &self,
        frame_height: u32,
        plane_index: usize,
        format: ff_format::PixelFormat,
    ) -> usize {
        use ff_format::PixelFormat;

        match format {
            // YUV 4:2:0 - U and V planes are half height
            PixelFormat::Yuv420p | PixelFormat::Yuv420p10le => {
                if plane_index == 0 {
                    frame_height as usize
                } else {
                    // Safe division with ceiling: (height + 1) / 2
                    // Equivalent to div_ceil(2) but more explicit about avoiding overflow
                    // Note: div_ceil() internally uses (n + d - 1) / d which could overflow
                    ((frame_height as usize) + 1) / 2
                }
            }
            // Semi-planar NV12/NV21/P010 - UV plane is half height
            PixelFormat::Nv12 | PixelFormat::Nv21 | PixelFormat::P010le => {
                if plane_index == 0 {
                    frame_height as usize
                } else {
                    // Safe division with ceiling: (height + 1) / 2
                    ((frame_height as usize) + 1) / 2
                }
            }
            // All other formats - full height for all planes
            _ => frame_height as usize,
        }
    }

    /// Receive encoded packets from the encoder.
    pub(super) unsafe fn receive_packets(&mut self) -> Result<(), EncodeError> {
        if self.video_codec_ctx.is_none() {
            return Err(EncodeError::InvalidConfig {
                reason: "Video codec not initialized".to_string(),
            });
        }

        let mut packet = av_packet_alloc();
        if packet.is_null() {
            return Err(EncodeError::Ffmpeg {
                code: 0,
                message: "Cannot allocate packet".to_string(),
            });
        }

        loop {
            let recv = self
                .video_codec_ctx
                .as_mut()
                .ok_or_else(|| EncodeError::InvalidConfig {
                    reason: "Video codec not initialized".to_string(),
                })?
                .receive_packet(packet);
            match recv {
                Ok(ff_sys::ReceiveOutcome::Frame) => {
                    // Packet received successfully
                }
                Ok(ff_sys::ReceiveOutcome::NeedInput | ff_sys::ReceiveOutcome::Drained) => {
                    // No more packets available
                    break;
                }
                Err(e) => {
                    av_packet_free(&raw mut packet);
                    return Err(EncodeError::Ffmpeg {
                        code: e.code(),
                        message: format!(
                            "Error receiving packet: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    });
                }
            }

            // Set stream index
            (*packet).stream_index = self.video_stream_index;

            // Attach HDR10 side data to keyframe packets.
            if let Some(ref meta) = self.hdr10_metadata {
                const AV_PKT_FLAG_KEY: i32 = 1;
                if (*packet).flags & AV_PKT_FLAG_KEY != 0 {
                    self.attach_hdr10_side_data(packet, meta);
                }
            }

            // Write packet
            let write_ret = av_interleaved_write_frame(self.format_ctx, packet);
            if write_ret < 0 {
                av_packet_unref(packet);
                av_packet_free(&raw mut packet);
                return Err(EncodeError::MuxingFailed {
                    reason: ff_sys::av_error_string(write_ret),
                });
            }

            self.bytes_written += (*packet).size as u64;

            av_packet_unref(packet);
        }

        av_packet_free(&raw mut packet);
        Ok(())
    }

    /// Convert AudioFrame to AVFrame with resampling if needed.
    pub(super) unsafe fn convert_audio_frame(
        &mut self,
        src: &AudioFrame,
        dst: *mut AVFrame,
    ) -> Result<(), EncodeError> {
        let codec_ctx = self
            .audio_codec_ctx
            .as_ref()
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Audio codec not initialized".to_string(),
            })?
            .as_ptr();

        let target_sample_rate = (*codec_ctx).sample_rate;
        let target_format = (*codec_ctx).sample_fmt;
        let target_ch_layout = &(*codec_ctx).ch_layout;

        // Check if we need to resample
        let src_sample_rate = src.sample_rate() as i32;
        let src_format = sample_format_to_av(src.format());
        let src_ch_layout = {
            let mut layout = AVChannelLayout::default();
            swresample::channel_layout::set_default(&raw mut layout, src.channels() as i32);
            layout
        };

        let needs_resampling = src_sample_rate != target_sample_rate
            || src_format != target_format
            || !swresample::channel_layout::is_equal(&raw const src_ch_layout, target_ch_layout);

        if needs_resampling {
            // Initialize resampler if needed
            if self.swr_ctx.is_none() {
                let swr_ctx = swresample::alloc_set_opts2(
                    target_ch_layout,
                    target_format,
                    target_sample_rate,
                    &raw const src_ch_layout,
                    src_format,
                    src_sample_rate,
                )
                .map_err(EncodeError::from_ffmpeg_error)?;

                swresample::init(swr_ctx).map_err(EncodeError::from_ffmpeg_error)?;
                self.swr_ctx = Some(swr_ctx);
            }

            let swr_ctx = self.swr_ctx.ok_or_else(|| EncodeError::Ffmpeg {
                code: 0,
                message: "Resampling context not initialized".to_string(),
            })?;

            // Estimate output sample count
            let out_samples = swresample::estimate_output_samples(
                target_sample_rate,
                src_sample_rate,
                src.samples() as i32,
            );

            // Set frame properties
            (*dst).format = target_format;
            (*dst).sample_rate = target_sample_rate;
            (*dst).nb_samples = out_samples;

            // Copy target channel layout
            swresample::channel_layout::copy(&raw mut (*dst).ch_layout, target_ch_layout)
                .map_err(EncodeError::from_ffmpeg_error)?;

            // Allocate frame buffer
            let ret = ff_sys::av_frame_get_buffer(dst, 0);
            if ret < 0 {
                return Err(EncodeError::Ffmpeg {
                    code: ret,
                    message: format!(
                        "Cannot allocate audio frame buffer: {}",
                        ff_sys::av_error_string(ret)
                    ),
                });
            }

            // Prepare input pointers
            let in_ptrs: Vec<*const u8> = if src.format().is_planar() {
                // Planar: one pointer per channel
                src.planes().iter().map(|p| p.as_ptr()).collect()
            } else {
                // Packed: single pointer
                vec![src.planes()[0].as_ptr()]
            };

            // Convert
            let samples_out = swresample::convert(
                swr_ctx,
                (*dst).data.as_mut_ptr().cast(),
                out_samples,
                in_ptrs.as_ptr(),
                src.samples() as i32,
            )
            .map_err(EncodeError::from_ffmpeg_error)?;

            (*dst).nb_samples = samples_out;
        } else {
            // No resampling needed, direct copy
            (*dst).format = src_format;
            (*dst).sample_rate = src_sample_rate;
            (*dst).nb_samples = src.samples() as i32;

            // Copy channel layout
            swresample::channel_layout::copy(&raw mut (*dst).ch_layout, &raw const src_ch_layout)
                .map_err(EncodeError::from_ffmpeg_error)?;

            // Allocate frame buffer
            let ret = ff_sys::av_frame_get_buffer(dst, 0);
            if ret < 0 {
                return Err(EncodeError::Ffmpeg {
                    code: ret,
                    message: format!(
                        "Cannot allocate audio frame buffer: {}",
                        ff_sys::av_error_string(ret)
                    ),
                });
            }

            // Copy audio data
            if src.format().is_planar() {
                // Copy each plane
                for (i, plane) in src.planes().iter().enumerate() {
                    if i < (*dst).data.len() && !(*dst).data[i].is_null() {
                        let size = plane.len();
                        ptr::copy_nonoverlapping(plane.as_ptr(), (*dst).data[i], size);
                    }
                }
            } else {
                // Copy single packed buffer
                if !(*dst).data[0].is_null() {
                    let size = src.planes()[0].len();
                    ptr::copy_nonoverlapping(src.planes()[0].as_ptr(), (*dst).data[0], size);
                }
            }
        }

        Ok(())
    }

    /// Receive encoded audio packets from the encoder.
    pub(super) unsafe fn receive_audio_packets(&mut self) -> Result<(), EncodeError> {
        if self.audio_codec_ctx.is_none() {
            return Err(EncodeError::InvalidConfig {
                reason: "Audio codec not initialized".to_string(),
            });
        }

        let mut packet = av_packet_alloc();
        if packet.is_null() {
            return Err(EncodeError::Ffmpeg {
                code: 0,
                message: "Cannot allocate packet".to_string(),
            });
        }

        loop {
            let recv = self
                .audio_codec_ctx
                .as_mut()
                .ok_or_else(|| EncodeError::InvalidConfig {
                    reason: "Audio codec not initialized".to_string(),
                })?
                .receive_packet(packet);
            match recv {
                Ok(ff_sys::ReceiveOutcome::Frame) => {
                    // Packet received successfully
                }
                Ok(ff_sys::ReceiveOutcome::NeedInput | ff_sys::ReceiveOutcome::Drained) => {
                    // No more packets available
                    break;
                }
                Err(e) => {
                    av_packet_free(&raw mut packet);
                    return Err(EncodeError::Ffmpeg {
                        code: e.code(),
                        message: format!(
                            "Error receiving audio packet: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    });
                }
            }

            // Set stream index
            (*packet).stream_index = self.audio_stream_index;

            // Write packet
            let write_ret = av_interleaved_write_frame(self.format_ctx, packet);
            if write_ret < 0 {
                av_packet_unref(packet);
                av_packet_free(&raw mut packet);
                return Err(EncodeError::MuxingFailed {
                    reason: ff_sys::av_error_string(write_ret),
                });
            }

            self.bytes_written += (*packet).size as u64;

            av_packet_unref(packet);
        }

        av_packet_free(&raw mut packet);
        Ok(())
    }
}
