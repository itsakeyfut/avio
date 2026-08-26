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
    AVChannelLayout, AVPixelFormat, AudioFrame, EncodeError, VideoEncoderInner, VideoFrame,
    swresample,
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
        let mut packet = ff_sys::Packet::new().map_err(|_| EncodeError::Ffmpeg {
            code: 0,
            message: "Cannot allocate packet".to_string(),
        })?;

        loop {
            match codec_ctx.receive_packet_into(&mut packet) {
                Ok(ff_sys::ReceiveOutcome::Frame) => {
                    // Discard — do not write to the format context.
                    packet.unref();
                }
                Ok(ff_sys::ReceiveOutcome::NeedInput | ff_sys::ReceiveOutcome::Drained) => {
                    break;
                }
                Err(e) => {
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
    /// `dst` is a safe owned frame; the target format scalars come from the
    /// caller's open codec context via the safe accessors.
    pub(super) unsafe fn convert_video_frame(
        &mut self,
        src: &VideoFrame,
        dst: &mut ff_sys::Frame,
        target_fmt: ff_sys::AVPixelFormat,
        target_width: std::os::raw::c_int,
        target_height: std::os::raw::c_int,
    ) -> Result<(), EncodeError> {
        let target_width = target_width as u32;
        let target_height = target_height as u32;

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
            // Create a new scaling context with the fast BILINEAR algorithm.
            // The old context (if any) drops on reassignment.
            self.sws_ctx = Some(
                ff_sys::ScaleContext::new(
                    src_width as i32,
                    src_height as i32,
                    src_fmt,
                    target_width as i32,
                    target_height as i32,
                    target_fmt,
                    ff_sys::swscale::scale_flags::BILINEAR, // Fast scaling algorithm
                )
                .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?,
            );
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
        dst: &mut ff_sys::Frame,
        target_fmt: AVPixelFormat,
    ) -> Result<(), EncodeError> {
        // Set frame properties
        dst.set_format(target_fmt);
        dst.set_width(src.width() as i32);
        dst.set_height(src.height() as i32);

        // Allocate frame buffer
        dst.get_buffer(0).map_err(|e| EncodeError::Ffmpeg {
            code: e.code(),
            message: format!(
                "Cannot allocate frame buffer: {}",
                ff_sys::av_error_string(e.code())
            ),
        })?;

        // Copy each plane directly
        for (i, plane) in src.planes().iter().enumerate() {
            if i >= MAX_PLANES {
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

            let plane_data = plane.data();
            // The destination stride is the frame's own linesize for plane i.
            let dst_stride = dst.linesize(i) as usize;
            // `video_plane_mut` yields `None` for an absent plane (null data),
            // matching the previous null-plane break; it self-sizes to the plane's
            // `linesize * plane_height`, superseding the removed `get_plane_height`.
            let Some(dst_plane) = dst.video_plane_mut(i) else {
                break;
            };

            // Optimization: If strides match, copy the whole plane at once.
            if src_stride == dst_stride {
                let n = plane_data.len().min(dst_plane.len());
                dst_plane[..n].copy_from_slice(&plane_data[..n]);
            } else {
                // Copy line by line to handle different strides.
                let row_bytes = src_stride.min(dst_stride);
                let num_rows = dst_plane.len() / dst_stride;
                for row in 0..num_rows {
                    let src_off = row * src_stride;
                    let dst_off = row * dst_stride;
                    if src_off + row_bytes <= plane_data.len() {
                        dst_plane[dst_off..dst_off + row_bytes]
                            .copy_from_slice(&plane_data[src_off..src_off + row_bytes]);
                    }
                }
            }
        }

        Ok(())
    }

    /// Scale frame using SwsContext (when formats or dimensions differ).
    pub(super) unsafe fn scale_frame(
        &mut self,
        src: &VideoFrame,
        dst: &mut ff_sys::Frame,
        target_fmt: AVPixelFormat,
        target_width: u32,
        target_height: u32,
    ) -> Result<(), EncodeError> {
        // Set frame properties
        dst.set_format(target_fmt);
        dst.set_width(target_width as i32);
        dst.set_height(target_height as i32);

        // Allocate frame buffer
        dst.get_buffer(0).map_err(|e| EncodeError::Ffmpeg {
            code: e.code(),
            message: format!(
                "Cannot allocate frame buffer: {}",
                ff_sys::av_error_string(e.code())
            ),
        })?;

        // Prepare source plane slices and strides.
        let src_planes: Vec<&[u8]> = src.planes().iter().map(|p| p.data()).collect();
        let src_strides: Vec<i32> = src.strides().iter().map(|&s| s as i32).collect();

        // Perform scaling/conversion using the cached scaling context (kept across
        // frames — unlike the single-use image encoder, this context is reused).
        self.sws_ctx
            .as_mut()
            .ok_or_else(|| EncodeError::Ffmpeg {
                code: 0,
                message: "Scaling context not initialized".to_string(),
            })?
            .scale_planes(&src_planes, &src_strides, src.height() as i32, dst)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        Ok(())
    }

    /// Receive encoded packets from the encoder.
    pub(super) unsafe fn receive_packets(&mut self) -> Result<(), EncodeError> {
        if self.video_codec_ctx.is_none() {
            return Err(EncodeError::InvalidConfig {
                reason: "Video codec not initialized".to_string(),
            });
        }

        let mut packet = ff_sys::Packet::new().map_err(|_| EncodeError::Ffmpeg {
            code: 0,
            message: "Cannot allocate packet".to_string(),
        })?;

        loop {
            let recv = self
                .video_codec_ctx
                .as_mut()
                .ok_or_else(|| EncodeError::InvalidConfig {
                    reason: "Video codec not initialized".to_string(),
                })?
                .receive_packet_into(&mut packet);
            match recv {
                Ok(ff_sys::ReceiveOutcome::Frame) => {
                    // Packet received successfully
                }
                Ok(ff_sys::ReceiveOutcome::NeedInput | ff_sys::ReceiveOutcome::Drained) => {
                    // No more packets available
                    break;
                }
                Err(e) => {
                    return Err(EncodeError::Ffmpeg {
                        code: e.code(),
                        message: format!(
                            "Error receiving packet: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    });
                }
            }

            // Set stream index and, for keyframes, attach HDR10 side data.
            packet.set_stream_index(self.video_stream_index);

            if let Some(ref meta) = self.hdr10_metadata {
                const AV_PKT_FLAG_KEY: i32 = 1;
                if packet.flags() & AV_PKT_FLAG_KEY != 0 {
                    self.attach_hdr10_side_data(&mut packet, meta);
                }
            }

            // Write packet
            if let Err(e) = self.format_ctx.write_interleaved(&mut packet) {
                packet.unref();
                return Err(EncodeError::MuxingFailed {
                    reason: ff_sys::av_error_string(e.code()),
                });
            }

            self.bytes_written += packet.size() as u64;

            packet.unref();
        }

        Ok(())
    }

    /// Convert AudioFrame to AVFrame with resampling if needed.
    pub(super) unsafe fn convert_audio_frame(
        &mut self,
        src: &AudioFrame,
        dst: &mut ff_sys::Frame,
    ) -> Result<(), EncodeError> {
        let codec_ctx =
            self.audio_codec_ctx
                .as_ref()
                .ok_or_else(|| EncodeError::InvalidConfig {
                    reason: "Audio codec not initialized".to_string(),
                })?;

        let target_sample_rate = codec_ctx.sample_rate();
        let target_format = codec_ctx.sample_fmt();
        let target_ch_layout = codec_ctx.ch_layout();

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
            // Initialize resampler if needed (RAII: allocates, configures, and
            // initializes internally; frees itself on drop).
            if self.swr_ctx.is_none() {
                self.swr_ctx = Some(
                    ff_sys::ResampleContext::new(
                        target_ch_layout,
                        target_format,
                        target_sample_rate,
                        &raw const src_ch_layout,
                        src_format,
                        src_sample_rate,
                    )
                    .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?,
                );
            }

            // Estimate output sample count
            let out_samples = swresample::estimate_output_samples(
                target_sample_rate,
                src_sample_rate,
                src.samples() as i32,
            );

            // Set frame properties from the encoder's target audio format.
            dst.set_format(target_format);
            dst.set_sample_rate(target_sample_rate);
            dst.set_nb_samples(out_samples);
            dst.set_ch_layout(target_ch_layout)
                .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

            // Allocate frame buffer
            dst.get_buffer(0).map_err(|e| EncodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Cannot allocate audio frame buffer: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

            // Prepare input plane slices (planar: one per channel; packed: one).
            let in_planes: Vec<&[u8]> = if src.format().is_planar() {
                src.planes().iter().map(Vec::as_slice).collect()
            } else {
                vec![src.planes()[0].as_slice()]
            };

            // Convert into the output frame's planes.
            let samples_out = self
                .swr_ctx
                .as_mut()
                .ok_or_else(|| EncodeError::Ffmpeg {
                    code: 0,
                    message: "Resampling context not initialized".to_string(),
                })?
                .convert_into_frame(dst, &in_planes, src.samples() as i32)
                .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

            dst.set_nb_samples(samples_out);
        } else {
            // No resampling needed, direct copy from the source's audio format.
            dst.set_format(src_format);
            dst.set_sample_rate(src_sample_rate);
            dst.set_nb_samples(src.samples() as i32);
            dst.set_ch_layout(&src_ch_layout)
                .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

            // Allocate frame buffer
            dst.get_buffer(0).map_err(|e| EncodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Cannot allocate audio frame buffer: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

            // Copy audio data into the destination frame's planes.
            if src.format().is_planar() {
                for (i, plane) in src.planes().iter().enumerate() {
                    if let Some(dst_plane) = dst.audio_plane_mut(i) {
                        let n = plane.len().min(dst_plane.len());
                        dst_plane[..n].copy_from_slice(&plane[..n]);
                    }
                }
            } else if let Some(dst_plane) = dst.audio_plane_mut(0) {
                let src_plane = &src.planes()[0];
                let n = src_plane.len().min(dst_plane.len());
                dst_plane[..n].copy_from_slice(&src_plane[..n]);
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

        let mut packet = ff_sys::Packet::new().map_err(|_| EncodeError::Ffmpeg {
            code: 0,
            message: "Cannot allocate packet".to_string(),
        })?;

        loop {
            let recv = self
                .audio_codec_ctx
                .as_mut()
                .ok_or_else(|| EncodeError::InvalidConfig {
                    reason: "Audio codec not initialized".to_string(),
                })?
                .receive_packet_into(&mut packet);
            match recv {
                Ok(ff_sys::ReceiveOutcome::Frame) => {
                    // Packet received successfully
                }
                Ok(ff_sys::ReceiveOutcome::NeedInput | ff_sys::ReceiveOutcome::Drained) => {
                    // No more packets available
                    break;
                }
                Err(e) => {
                    return Err(EncodeError::Ffmpeg {
                        code: e.code(),
                        message: format!(
                            "Error receiving audio packet: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    });
                }
            }

            // Set stream index.
            packet.set_stream_index(self.audio_stream_index);

            // Write packet
            if let Err(e) = self.format_ctx.write_interleaved(&mut packet) {
                packet.unref();
                return Err(EncodeError::MuxingFailed {
                    reason: ff_sys::av_error_string(e.code()),
                });
            }

            self.bytes_written += packet.size() as u64;

            packet.unref();
        }

        Ok(())
    }
}
