use super::{
    AVFrame, Arc, DecodeError, Duration, OutputScale, PixelFormat, PooledBuffer, Rational,
    Timestamp, VideoDecoderInner, VideoFrame,
};
use crate::shared::plane_inner::plane_row_ptr;

impl VideoDecoderInner {
    /// Decodes the next video frame.
    ///
    /// Transparently reconnects on `StreamInterrupted` when
    /// `NetworkOptions::reconnect_on_error` is enabled.
    ///
    /// # Returns
    ///
    /// - `Ok(Some(frame))` - Successfully decoded a frame
    /// - `Ok(None)` - End of stream reached
    /// - `Err(_)` - Decoding error occurred
    pub(crate) fn decode_one(&mut self) -> Result<Option<VideoFrame>, DecodeError> {
        loop {
            match self.decode_one_inner() {
                Ok(frame) => return Ok(frame),
                Err(DecodeError::StreamInterrupted { .. })
                    if self.url.is_some() && self.network_opts.reconnect_on_error =>
                {
                    self.attempt_reconnect()?;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn decode_one_inner(&mut self) -> Result<Option<VideoFrame>, DecodeError> {
        if self.eof {
            return Ok(None);
        }

        unsafe {
            loop {
                // Try to receive a frame from the decoder
                match self.codec_ctx.receive_frame(self.frame).map_err(|e| {
                    DecodeError::DecodingFailed {
                        timestamp: Some(self.position),
                        reason: ff_sys::av_error_string(e.code()),
                    }
                })? {
                    ff_sys::ReceiveOutcome::Frame => {
                        // Successfully received a frame — reset corrupt-stream counter.
                        self.consecutive_invalid = 0;

                        // Check if this is a hardware frame and transfer to CPU memory if needed
                        self.transfer_hardware_frame_if_needed()?;

                        // SAFETY: self.frame is valid and non-null after receive_frame succeeded.
                        let w = (*self.frame).width as u32;
                        let h = (*self.frame).height as u32;
                        if w > 32_768 || h > 32_768 {
                            log::warn!(
                                "frame rejected reason=unsupported_resolution width={w} height={h}"
                            );
                            return Err(DecodeError::UnsupportedResolution {
                                width: w,
                                height: h,
                            });
                        }

                        let video_frame = self.convert_frame_to_video_frame()?;

                        // Update position based on frame timestamp
                        let pts = (*self.frame).pts;
                        if pts != ff_sys::AV_NOPTS_VALUE {
                            let stream = (*self.format_ctx).streams.add(self.stream_index as usize);
                            let time_base = (*(*stream)).time_base;
                            let timestamp_secs =
                                pts as f64 * time_base.num as f64 / time_base.den as f64;
                            self.position = Duration::from_secs_f64(timestamp_secs);
                        }

                        return Ok(Some(video_frame));
                    }
                    ff_sys::ReceiveOutcome::NeedInput => {
                        // Need to send more packets to the decoder
                        // Read a packet from the file
                        let read_ret = ff_sys::av_read_frame(self.format_ctx, self.packet);

                        if read_ret == ff_sys::error_codes::EOF {
                            // End of file - flush the decoder
                            let _ = self.codec_ctx.send_eof();
                            self.eof = true;
                            continue;
                        } else if read_ret < 0 {
                            return Err(if let Some(url) = &self.url {
                                // Network source: map to typed variant so reconnect can detect it.
                                crate::network::map_network_error(
                                    read_ret,
                                    crate::network::sanitize_url(url),
                                )
                            } else {
                                DecodeError::Ffmpeg {
                                    code: read_ret,
                                    message: format!(
                                        "Failed to read frame: {}",
                                        ff_sys::av_error_string(read_ret)
                                    ),
                                }
                            });
                        }

                        // Check if this packet belongs to the video stream
                        if (*self.packet).stream_index == self.stream_index {
                            // Send the packet to the decoder
                            let send_result = self.codec_ctx.send_packet(self.packet);
                            // SAFETY: self.packet is valid and non-null; pts is a plain i64 field.
                            let pkt_pts = (*self.packet).pts;
                            ff_sys::av_packet_unref(self.packet);

                            if let Err(se) = send_result {
                                if se.code() == ff_sys::error_codes::AVERROR_INVALIDDATA {
                                    log::warn!("packet skipped reason=invalid_data pts={pkt_pts}");
                                    self.consecutive_invalid += 1;
                                    if self.consecutive_invalid >= 32 {
                                        log::warn!(
                                            "stream corrupted consecutive_invalid_packets={}",
                                            self.consecutive_invalid
                                        );
                                        return Err(DecodeError::StreamCorrupted {
                                            consecutive_invalid_packets: self.consecutive_invalid,
                                        });
                                    }
                                    // Do not return error; fall through to read the next packet.
                                } else if !se.is_eagain() {
                                    return Err(DecodeError::Ffmpeg {
                                        code: se.code(),
                                        message: format!(
                                            "Failed to send packet: {}",
                                            ff_sys::av_error_string(se.code())
                                        ),
                                    });
                                }
                            }
                        } else {
                            // Not our stream, unref and continue
                            ff_sys::av_packet_unref(self.packet);
                        }
                    }
                    ff_sys::ReceiveOutcome::Drained => {
                        // Decoder has been fully flushed
                        self.eof = true;
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// Converts an AVFrame to a VideoFrame, applying pixel format conversion if needed.
    unsafe fn convert_frame_to_video_frame(&mut self) -> Result<VideoFrame, DecodeError> {
        // SAFETY: Caller ensures self.frame is valid
        unsafe {
            let src_width = (*self.frame).width as u32;
            let src_height = (*self.frame).height as u32;
            let src_format = (*self.frame).format;

            // Determine output format
            let dst_format = if let Some(fmt) = self.output_format {
                Self::pixel_format_to_av(fmt)
            } else {
                src_format
            };

            // Determine output dimensions
            let (dst_width, dst_height) = self.resolve_output_dims(src_width, src_height);

            // Check if conversion or scaling is needed
            let needs_conversion =
                src_format != dst_format || dst_width != src_width || dst_height != src_height;

            if needs_conversion {
                self.convert_with_sws(
                    src_width, src_height, src_format, dst_width, dst_height, dst_format,
                )
            } else {
                self.av_frame_to_video_frame(self.frame)
            }
        }
    }

    /// Computes the destination (width, height) from `output_scale` and source dimensions.
    ///
    /// Returns `(src_width, src_height)` when no scale is set.
    /// All returned dimensions are rounded up to the nearest even number.
    fn resolve_output_dims(&self, src_width: u32, src_height: u32) -> (u32, u32) {
        let round_even = |n: u32| (n + 1) & !1;

        match self.output_scale {
            None => (src_width, src_height),
            Some(OutputScale::Exact { width, height }) => (round_even(width), round_even(height)),
            Some(OutputScale::FitWidth(target_w)) => {
                let target_w = round_even(target_w);
                if src_width == 0 {
                    return (target_w, target_w);
                }
                let h = (target_w as u64 * src_height as u64 / src_width as u64) as u32;
                (target_w, round_even(h.max(2)))
            }
            Some(OutputScale::FitHeight(target_h)) => {
                let target_h = round_even(target_h);
                if src_height == 0 {
                    return (target_h, target_h);
                }
                let w = (target_h as u64 * src_width as u64 / src_height as u64) as u32;
                (round_even(w.max(2)), target_h)
            }
        }
    }

    /// Converts an AVFrame to a VideoFrame.
    pub(super) unsafe fn av_frame_to_video_frame(
        &self,
        frame: *const AVFrame,
    ) -> Result<VideoFrame, DecodeError> {
        // SAFETY: Caller ensures frame and format_ctx are valid
        unsafe {
            let width = (*frame).width as u32;
            let height = (*frame).height as u32;
            let format = Self::convert_pixel_format((*frame).format);

            // Extract timestamp
            let pts = (*frame).pts;
            let timestamp = if pts != ff_sys::AV_NOPTS_VALUE {
                let stream = (*self.format_ctx).streams.add(self.stream_index as usize);
                let time_base = (*(*stream)).time_base;
                Timestamp::new(
                    pts as i64,
                    Rational::new(time_base.num as i32, time_base.den as i32),
                )
            } else {
                Timestamp::default()
            };

            // Convert frame to planes and strides
            let (planes, strides) =
                self.extract_planes_and_strides(frame, width, height, format)?;

            VideoFrame::new(planes, strides, width, height, format, timestamp, false).map_err(|e| {
                DecodeError::Ffmpeg {
                    code: 0,
                    message: format!("Failed to create VideoFrame: {e}"),
                }
            })
        }
    }

    /// Allocates a buffer, optionally using the frame pool.
    ///
    /// If a frame pool is configured and has available buffers, uses the pool.
    /// Otherwise, allocates a new Vec<u8>.
    ///
    /// Allocates a buffer for decoded frame data.
    ///
    /// If a frame pool is configured, attempts to acquire a buffer from the pool.
    /// The returned PooledBuffer will automatically be returned to the pool when dropped.
    fn allocate_buffer(&self, size: usize) -> PooledBuffer {
        if let Some(ref pool) = self.frame_pool {
            if let Some(pooled_buffer) = pool.acquire(size) {
                return pooled_buffer;
            }
            // Pool is configured but currently empty (or has no buffer large
            // enough). Allocate fresh memory and attach it to the pool so
            // that when the VideoFrame is dropped the buffer is returned via
            // pool.release() and becomes available for the next frame.
            return PooledBuffer::new(vec![0u8; size], Arc::downgrade(pool));
        }
        PooledBuffer::standalone(vec![0u8; size])
    }

    /// Extracts planes and strides from an AVFrame.
    unsafe fn extract_planes_and_strides(
        &self,
        frame: *const AVFrame,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<(Vec<PooledBuffer>, Vec<usize>), DecodeError> {
        // Bytes per pixel constants for different pixel formats
        const BYTES_PER_PIXEL_RGBA: usize = 4;
        const BYTES_PER_PIXEL_RGB24: usize = 3;

        // SAFETY: Caller ensures frame is valid and format matches actual frame format
        unsafe {
            let mut planes = Vec::new();
            let mut strides = Vec::new();

            #[allow(clippy::match_same_arms)]
            match format {
                PixelFormat::Rgba | PixelFormat::Bgra | PixelFormat::Rgb24 | PixelFormat::Bgr24 => {
                    // Packed formats - single plane
                    let src = (*frame).data[0];
                    let src_linesize = (*frame).linesize[0];
                    let bytes_per_pixel = if matches!(format, PixelFormat::Rgba | PixelFormat::Bgra)
                    {
                        BYTES_PER_PIXEL_RGBA
                    } else {
                        BYTES_PER_PIXEL_RGB24
                    };
                    let row_size = (width as usize) * bytes_per_pixel;
                    let buffer_size = row_size * height as usize;
                    let mut plane_data = self.allocate_buffer(buffer_size);

                    for y in 0..height as usize {
                        let dst_offset = y * row_size;
                        let src_ptr = plane_row_ptr(src, src_linesize, y);
                        let plane_slice = plane_data.as_mut();
                        // SAFETY: We copy exactly `row_size` bytes per row. The source pointer
                        // is valid (from FFmpeg frame data), destination has sufficient capacity
                        // (allocated with height * row_size), and ranges don't overlap.
                        std::ptr::copy_nonoverlapping(
                            src_ptr,
                            plane_slice[dst_offset..].as_mut_ptr(),
                            row_size,
                        );
                    }

                    planes.push(plane_data);
                    strides.push(row_size);
                }
                PixelFormat::Yuv420p | PixelFormat::Yuv422p | PixelFormat::Yuv444p => {
                    // Planar YUV formats
                    let (chroma_width, chroma_height) = match format {
                        PixelFormat::Yuv420p => (width / 2, height / 2),
                        PixelFormat::Yuv422p => (width / 2, height),
                        PixelFormat::Yuv444p => (width, height),
                        _ => unreachable!(),
                    };

                    // Y plane
                    let y_stride = width as usize;
                    let y_size = y_stride * height as usize;
                    let mut y_data = self.allocate_buffer(y_size);
                    let y_src = (*frame).data[0];
                    let y_src_linesize = (*frame).linesize[0];
                    for y in 0..height as usize {
                        let dst_offset = y * y_stride;
                        let src_ptr = plane_row_ptr(y_src, y_src_linesize, y);
                        let y_slice = y_data.as_mut();
                        // SAFETY: Copying Y plane row-by-row. Source is valid FFmpeg data,
                        // destination has sufficient capacity, no overlap.
                        std::ptr::copy_nonoverlapping(
                            src_ptr,
                            y_slice[dst_offset..].as_mut_ptr(),
                            width as usize,
                        );
                    }
                    planes.push(y_data);
                    strides.push(y_stride);

                    // U plane
                    let u_stride = chroma_width as usize;
                    let u_size = u_stride * chroma_height as usize;
                    let mut u_data = self.allocate_buffer(u_size);
                    let u_src = (*frame).data[1];
                    let u_src_linesize = (*frame).linesize[1];
                    for y in 0..chroma_height as usize {
                        let dst_offset = y * u_stride;
                        let src_ptr = plane_row_ptr(u_src, u_src_linesize, y);
                        let u_slice = u_data.as_mut();
                        // SAFETY: Copying U (chroma) plane row-by-row. Valid source,
                        // sufficient destination capacity, no overlap.
                        std::ptr::copy_nonoverlapping(
                            src_ptr,
                            u_slice[dst_offset..].as_mut_ptr(),
                            chroma_width as usize,
                        );
                    }
                    planes.push(u_data);
                    strides.push(u_stride);

                    // V plane
                    let v_stride = chroma_width as usize;
                    let v_size = v_stride * chroma_height as usize;
                    let mut v_data = self.allocate_buffer(v_size);
                    let v_src = (*frame).data[2];
                    let v_src_linesize = (*frame).linesize[2];
                    for y in 0..chroma_height as usize {
                        let dst_offset = y * v_stride;
                        let src_ptr = plane_row_ptr(v_src, v_src_linesize, y);
                        let v_slice = v_data.as_mut();
                        // SAFETY: Copying V (chroma) plane row-by-row. Valid source,
                        // sufficient destination capacity, no overlap.
                        std::ptr::copy_nonoverlapping(
                            src_ptr,
                            v_slice[dst_offset..].as_mut_ptr(),
                            chroma_width as usize,
                        );
                    }
                    planes.push(v_data);
                    strides.push(v_stride);
                }
                PixelFormat::Gray8 => {
                    // Single plane grayscale
                    let stride = width as usize;
                    let mut plane_data = self.allocate_buffer(stride * height as usize);
                    let src = (*frame).data[0];
                    let src_linesize = (*frame).linesize[0];

                    for y in 0..height as usize {
                        let dst_offset = y * stride;
                        let src_ptr = plane_row_ptr(src, src_linesize, y);
                        let plane_slice = plane_data.as_mut();
                        // SAFETY: Copying grayscale plane row-by-row. Valid source,
                        // sufficient destination capacity, no overlap.
                        std::ptr::copy_nonoverlapping(
                            src_ptr,
                            plane_slice[dst_offset..].as_mut_ptr(),
                            width as usize,
                        );
                    }

                    planes.push(plane_data);
                    strides.push(stride);
                }
                PixelFormat::Nv12 | PixelFormat::Nv21 => {
                    // Semi-planar formats
                    let uv_height = height / 2;

                    // Y plane
                    let y_stride = width as usize;
                    let mut y_data = self.allocate_buffer(y_stride * height as usize);
                    let y_src = (*frame).data[0];
                    let y_src_linesize = (*frame).linesize[0];
                    for y in 0..height as usize {
                        let dst_offset = y * y_stride;
                        let src_ptr = plane_row_ptr(y_src, y_src_linesize, y);
                        let y_slice = y_data.as_mut();
                        // SAFETY: Copying Y plane (semi-planar) row-by-row. Valid source,
                        // sufficient destination capacity, no overlap.
                        std::ptr::copy_nonoverlapping(
                            src_ptr,
                            y_slice[dst_offset..].as_mut_ptr(),
                            width as usize,
                        );
                    }
                    planes.push(y_data);
                    strides.push(y_stride);

                    // UV plane
                    let uv_stride = width as usize;
                    let mut uv_data = self.allocate_buffer(uv_stride * uv_height as usize);
                    let uv_src = (*frame).data[1];
                    let uv_src_linesize = (*frame).linesize[1];
                    for y in 0..uv_height as usize {
                        let dst_offset = y * uv_stride;
                        let src_ptr = plane_row_ptr(uv_src, uv_src_linesize, y);
                        let uv_slice = uv_data.as_mut();
                        // SAFETY: Copying interleaved UV plane (semi-planar) row-by-row.
                        // Valid source, sufficient destination capacity, no overlap.
                        std::ptr::copy_nonoverlapping(
                            src_ptr,
                            uv_slice[dst_offset..].as_mut_ptr(),
                            width as usize,
                        );
                    }
                    planes.push(uv_data);
                    strides.push(uv_stride);
                }
                PixelFormat::Gbrpf32le => {
                    // Planar GBR float: 3 full-resolution planes, 4 bytes per sample (f32)
                    const BYTES_PER_SAMPLE: usize = 4;
                    let row_size = width as usize * BYTES_PER_SAMPLE;
                    let size = row_size * height as usize;

                    for plane_idx in 0..3usize {
                        let src = (*frame).data[plane_idx];
                        let src_linesize = (*frame).linesize[plane_idx];
                        let mut plane_data = self.allocate_buffer(size);
                        for y in 0..height as usize {
                            let dst_offset = y * row_size;
                            let src_ptr = plane_row_ptr(src, src_linesize, y);
                            let dst_slice = plane_data.as_mut();
                            // SAFETY: Copying one row of a planar float plane. Source is valid
                            // FFmpeg frame data, destination has sufficient capacity, no overlap.
                            std::ptr::copy_nonoverlapping(
                                src_ptr,
                                dst_slice[dst_offset..].as_mut_ptr(),
                                row_size,
                            );
                        }
                        planes.push(plane_data);
                        strides.push(row_size);
                    }
                }
                _ => {
                    return Err(DecodeError::Ffmpeg {
                        code: 0,
                        message: format!("Unsupported pixel format: {format:?}"),
                    });
                }
            }

            Ok((planes, strides))
        }
    }
}
