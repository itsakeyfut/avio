use super::{
    Arc, DecodeError, Duration, Frame, OutputScale, PixelFormat, PooledBuffer, Rational, Timestamp,
    VideoDecoderInner, VideoFrame,
};

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
                match self.codec_ctx.receive_frame(&mut self.frame).map_err(|e| {
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

                        let w = self.frame.width() as u32;
                        let h = self.frame.height() as u32;
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
                        let pts = self.frame.pts();
                        if pts != ff_sys::AV_NOPTS_VALUE
                            && let Some(stream) = self.format_ctx.stream(self.stream_index as usize)
                        {
                            let time_base = stream.time_base();
                            let timestamp_secs =
                                pts as f64 * time_base.num as f64 / time_base.den as f64;
                            self.position = Duration::from_secs_f64(timestamp_secs);
                        }

                        return Ok(Some(video_frame));
                    }
                    ff_sys::ReceiveOutcome::NeedInput => {
                        // Need to send more packets to the decoder
                        // Read a packet from the file
                        match self.format_ctx.read_frame(&mut self.packet) {
                            Ok(()) => {}
                            Err(e) if e.is_eof() => {
                                // End of file - flush the decoder
                                let _ = self.codec_ctx.send_eof();
                                self.eof = true;
                                continue;
                            }
                            Err(e) => {
                                let read_ret = e.code();
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
                        }

                        // Check if this packet belongs to the video stream
                        if self.packet.stream_index() == self.stream_index {
                            // Send the packet to the decoder
                            let send_result = self.codec_ctx.send_packet(&self.packet);
                            let pkt_pts = self.packet.pts();
                            self.packet.unref();

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
                            self.packet.unref();
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

    /// Converts the decoder's current frame to a VideoFrame, applying pixel format
    /// conversion if needed.
    unsafe fn convert_frame_to_video_frame(&mut self) -> Result<VideoFrame, DecodeError> {
        let src_width = self.frame.width() as u32;
        let src_height = self.frame.height() as u32;
        let src_format = self.frame.format();

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

        // SAFETY: `self.frame` holds a valid decoded frame in `src_format`.
        unsafe {
            if needs_conversion {
                self.convert_with_sws(
                    src_width, src_height, src_format, dst_width, dst_height, dst_format,
                )
            } else {
                self.av_frame_to_video_frame(&self.frame)
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

    /// Converts an owned [`Frame`] to a [`VideoFrame`].
    ///
    /// Scalar fields are read through the frame's accessors; the plane data copy
    /// in [`extract_planes_and_strides`](Self::extract_planes_and_strides) reads
    /// each plane through [`Frame::copy_plane_rows`].
    ///
    /// # Safety
    ///
    /// The caller must ensure `frame`'s pixel format is one the decoder produced,
    /// so the plane copy reads the correct plane sizes.
    pub(super) unsafe fn av_frame_to_video_frame(
        &self,
        frame: &Frame,
    ) -> Result<VideoFrame, DecodeError> {
        let width = frame.width() as u32;
        let height = frame.height() as u32;
        let format = Self::convert_pixel_format(frame.format());

        // Extract timestamp
        let pts = frame.pts();
        let timestamp = if pts != ff_sys::AV_NOPTS_VALUE {
            match self.format_ctx.stream(self.stream_index as usize) {
                Some(stream) => {
                    let time_base = stream.time_base();
                    Timestamp::new(
                        pts,
                        Rational::new(time_base.num as i32, time_base.den as i32),
                    )
                }
                None => Timestamp::default(),
            }
        } else {
            Timestamp::default()
        };

        // Convert frame to planes and strides.
        // SAFETY: `format` is derived from `frame.format()`, so it matches the frame.
        let (planes, strides) =
            unsafe { self.extract_planes_and_strides(frame, width, height, format)? };

        VideoFrame::new(planes, strides, width, height, format, timestamp, false).map_err(|e| {
            DecodeError::Ffmpeg {
                code: 0,
                message: format!("Failed to create VideoFrame: {e}"),
            }
        })
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

    /// Extracts planes and strides from a decoded frame.
    ///
    /// # Safety
    ///
    /// The caller must ensure `format` and `width` / `height` match the frame's
    /// real pixel format and dimensions; the per-plane [`Frame::copy_plane_rows`]
    /// copies below trust that geometry.
    unsafe fn extract_planes_and_strides(
        &self,
        frame: &Frame,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<(Vec<PooledBuffer>, Vec<usize>), DecodeError> {
        // Bytes per pixel constants for different pixel formats
        const BYTES_PER_PIXEL_RGBA: usize = 4;
        const BYTES_PER_PIXEL_RGB24: usize = 3;

        let missing_plane = |plane: usize| DecodeError::Ffmpeg {
            code: 0,
            message: format!("decoded frame plane {plane} is missing or null"),
        };

        let mut planes = Vec::new();
        let mut strides = Vec::new();

        #[allow(clippy::match_same_arms)]
        match format {
            PixelFormat::Rgba | PixelFormat::Bgra | PixelFormat::Rgb24 | PixelFormat::Bgr24 => {
                // Packed formats - single plane
                let bytes_per_pixel = if matches!(format, PixelFormat::Rgba | PixelFormat::Bgra) {
                    BYTES_PER_PIXEL_RGBA
                } else {
                    BYTES_PER_PIXEL_RGB24
                };
                let row_size = (width as usize) * bytes_per_pixel;
                let mut plane_data = self.allocate_buffer(row_size * height as usize);
                // SAFETY: `row_size` / `height` match the frame's packed format, and
                //         `plane_data` holds `row_size * height` bytes.
                unsafe {
                    frame.copy_plane_rows(
                        0,
                        plane_data.as_mut(),
                        row_size,
                        height as usize,
                        row_size,
                    )
                }
                .ok_or_else(|| missing_plane(0))?;
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
                let mut y_data = self.allocate_buffer(y_stride * height as usize);
                // SAFETY: `y_stride` / `height` match the luma plane; `y_data` fits it.
                unsafe {
                    frame.copy_plane_rows(0, y_data.as_mut(), y_stride, height as usize, y_stride)
                }
                .ok_or_else(|| missing_plane(0))?;
                planes.push(y_data);
                strides.push(y_stride);

                // U / V chroma planes
                let chroma_stride = chroma_width as usize;
                for plane_idx in 1..=2 {
                    let mut chroma_data =
                        self.allocate_buffer(chroma_stride * chroma_height as usize);
                    // SAFETY: `chroma_stride` / `chroma_height` match plane `plane_idx`;
                    //         `chroma_data` fits it.
                    unsafe {
                        frame.copy_plane_rows(
                            plane_idx,
                            chroma_data.as_mut(),
                            chroma_stride,
                            chroma_height as usize,
                            chroma_stride,
                        )
                    }
                    .ok_or_else(|| missing_plane(plane_idx))?;
                    planes.push(chroma_data);
                    strides.push(chroma_stride);
                }
            }
            PixelFormat::Gray8 => {
                // Single plane grayscale
                let stride = width as usize;
                let mut plane_data = self.allocate_buffer(stride * height as usize);
                // SAFETY: `stride` / `height` match the grayscale plane; `plane_data` fits it.
                unsafe {
                    frame.copy_plane_rows(0, plane_data.as_mut(), stride, height as usize, stride)
                }
                .ok_or_else(|| missing_plane(0))?;
                planes.push(plane_data);
                strides.push(stride);
            }
            PixelFormat::Nv12 | PixelFormat::Nv21 => {
                // Semi-planar formats
                let uv_height = height / 2;

                // Y plane
                let y_stride = width as usize;
                let mut y_data = self.allocate_buffer(y_stride * height as usize);
                // SAFETY: `y_stride` / `height` match the luma plane; `y_data` fits it.
                unsafe {
                    frame.copy_plane_rows(0, y_data.as_mut(), y_stride, height as usize, y_stride)
                }
                .ok_or_else(|| missing_plane(0))?;
                planes.push(y_data);
                strides.push(y_stride);

                // Interleaved UV plane
                let uv_stride = width as usize;
                let mut uv_data = self.allocate_buffer(uv_stride * uv_height as usize);
                // SAFETY: `uv_stride` / `uv_height` match the interleaved chroma plane;
                //         `uv_data` fits it.
                unsafe {
                    frame.copy_plane_rows(
                        1,
                        uv_data.as_mut(),
                        uv_stride,
                        uv_height as usize,
                        uv_stride,
                    )
                }
                .ok_or_else(|| missing_plane(1))?;
                planes.push(uv_data);
                strides.push(uv_stride);
            }
            PixelFormat::Gbrpf32le => {
                // Planar GBR float: 3 full-resolution planes, 4 bytes per sample (f32)
                const BYTES_PER_SAMPLE: usize = 4;
                let row_size = width as usize * BYTES_PER_SAMPLE;

                for plane_idx in 0..3usize {
                    let mut plane_data = self.allocate_buffer(row_size * height as usize);
                    // SAFETY: `row_size` / `height` match this full-resolution float
                    //         plane; `plane_data` fits it.
                    unsafe {
                        frame.copy_plane_rows(
                            plane_idx,
                            plane_data.as_mut(),
                            row_size,
                            height as usize,
                            row_size,
                        )
                    }
                    .ok_or_else(|| missing_plane(plane_idx))?;
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
