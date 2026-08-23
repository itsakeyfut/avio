use super::{
    DecodeError, Duration, KEYFRAME_SEEK_TOLERANCE_SECS, VideoDecoderInner, VideoFrame,
    open_url_ctx,
};

impl VideoDecoderInner {
    /// Returns the current playback position.
    pub(crate) fn position(&self) -> Duration {
        self.position
    }

    /// Returns whether end of file has been reached.
    pub(crate) fn is_eof(&self) -> bool {
        self.eof
    }

    /// Returns whether the source is a live or streaming input.
    ///
    /// Live sources have the `AVFMT_TS_DISCONT` flag set on their `AVInputFormat`.
    /// Seeking is not meaningful on live sources.
    pub(crate) fn is_live(&self) -> bool {
        self.is_live
    }

    /// Converts a `Duration` to a presentation timestamp (PTS) in stream time_base units.
    ///
    /// # Arguments
    ///
    /// * `duration` - The duration to convert.
    ///
    /// # Returns
    ///
    /// The timestamp in stream time_base units.
    ///
    /// # Note
    ///
    /// av_seek_frame expects timestamps in stream time_base units when using a specific stream_index.
    fn duration_to_pts(&self, duration: Duration) -> i64 {
        // Convert duration to stream time_base units for seeking. The stream index
        // was validated during decoder creation; fall back to a 1/1 time base if it
        // is somehow absent (unreachable in practice).
        let time_base = self
            .format_ctx
            .stream(self.stream_index as usize)
            .map_or(ff_sys::AVRational { num: 1, den: 1 }, |s| s.time_base());

        // Convert: duration (seconds) * (time_base.den / time_base.num) = PTS
        let time_base_f64 = time_base.den as f64 / time_base.num as f64;
        (duration.as_secs_f64() * time_base_f64) as i64
    }

    /// Converts a presentation timestamp (PTS) to a `Duration`.
    ///
    /// # Arguments
    ///
    /// * `pts` - The presentation timestamp in stream time base units.
    ///
    /// # Returns
    ///
    /// The duration corresponding to the PTS.
    ///
    /// # Safety
    ///
    /// Caller must ensure that `format_ctx` and `stream_index` are valid.
    /// Seeks to a specified position in the video stream.
    ///
    /// This method performs efficient seeking without reopening the file.
    /// It uses `av_seek_frame` internally and flushes the decoder buffers.
    ///
    /// # Performance Characteristics
    ///
    /// - **Keyframe seek**: 5-10ms for typical GOP sizes (1-2 seconds)
    /// - **Exact seek**: Proportional to distance from nearest keyframe
    /// - **Large GOP videos**: May require sequential decoding from distant keyframe
    ///
    /// For videos with sparse keyframes (GOP > 2 seconds), the method will
    /// decode frames sequentially from the nearest keyframe to reach the target.
    /// This ensures correct frame data but may take longer (10-50ms for very large GOPs).
    ///
    /// # Arguments
    ///
    /// * `position` - Target position to seek to.
    /// * `mode` - Seek mode (Keyframe, Exact, or Backward).
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::SeekFailed`] if the seek operation fails.
    pub(crate) fn seek(
        &mut self,
        position: Duration,
        mode: crate::SeekMode,
    ) -> Result<(), DecodeError> {
        use crate::SeekMode;

        let timestamp = self.duration_to_pts(position);

        // All seek modes use BACKWARD flag to find the nearest keyframe at or before target.
        // The difference between modes is in the post-seek processing below.
        let flags = ff_sys::avformat::seek_flags::BACKWARD;

        // 1. Clear any pending packet and frame to avoid reading stale data after seek
        // SAFETY:
        // - packet is valid: allocated in constructor, owned by VideoDecoderInner
        // - frame is valid: allocated in constructor, owned by VideoDecoderInner
        self.packet.unref();
        self.frame.unref();

        // 2. Seek in the format context (file is NOT reopened)
        // Use av_seek_frame with the stream index and timestamp in stream time_base units
        self.format_ctx
            .seek_frame(self.stream_index as i32, timestamp, flags)
            .map_err(|e| DecodeError::SeekFailed {
                target: position,
                reason: ff_sys::av_error_string(e.code()),
            })?;

        // 3. Flush decoder buffers to clear any cached frames
        // SAFETY: the codec context was opened during construction.
        unsafe { self.codec_ctx.flush_buffers() };

        // 4. Drain any remaining frames from the decoder after flush
        // This ensures no stale frames are returned after the seek
        // SAFETY: frame is valid: allocated in constructor, owned by VideoDecoderInner
        // Drain while frames are produced. NeedInput / Drained / any error all stop
        // draining (preserves the pre-migration `Err(_) => break` behaviour).
        while let Ok(ff_sys::ReceiveOutcome::Frame) = self.codec_ctx.receive_frame(&mut self.frame)
        {
            self.frame.unref();
        }

        // 5. Reset internal state
        self.eof = false;
        // Note: We don't update self.position here because it will be updated
        // when the next frame is decoded. This ensures position reflects actual decoded position.

        // 6. Skip forward to the target position
        //
        // Context: av_seek_frame with BACKWARD flag seeks to the nearest keyframe *at or before*
        // the target timestamp. For videos with sparse keyframes (large GOP size), this may
        // land far from the target (e.g., at the first keyframe for GOP=entire video).
        //
        // Solution: Decode frames sequentially from the keyframe until reaching the target.
        // This is necessary because H.264/H.265 P-frames and B-frames depend on previous
        // frames for reconstruction, so we must decode all intermediate frames.
        //
        // Performance Impact:
        // - Typical GOP (1-2s): 30-60 frames to skip, ~5-10ms overhead
        // - Large GOP (5-10s): 150-300 frames to skip, ~20-50ms overhead
        // - Worst case (single keyframe): May decode entire video, ~100ms-1s
        if mode == SeekMode::Exact {
            // For exact mode, decode until we reach or pass the exact target
            self.skip_to_exact(position)?;
        } else {
            // For keyframe/backward modes, decode until we're reasonably close to the target
            // Rationale: Balances accuracy with performance for common use cases
            let tolerance = Duration::from_secs(KEYFRAME_SEEK_TOLERANCE_SECS);
            let min_position = position.saturating_sub(tolerance);

            while let Some(frame) = self.decode_one()? {
                let frame_time = frame.timestamp().as_duration();
                if frame_time >= min_position {
                    // We're close enough to the target
                    break;
                }
                // Continue decoding to get closer (frames are automatically dropped)
            }
        }

        Ok(())
    }

    /// Skips frames until reaching the exact target position.
    ///
    /// This is used by [`Self::seek`] when `SeekMode::Exact` is specified.
    /// It decodes and discards frames from the nearest keyframe until
    /// reaching the target position.
    ///
    /// # Performance
    ///
    /// Time complexity is O(n) where n is the number of frames between the
    /// keyframe and target. For a 30fps video with 2-second GOP:
    /// - Worst case: ~60 frames to decode, ~10-20ms
    /// - Average case: ~30 frames to decode, ~5-10ms
    ///
    /// # Arguments
    ///
    /// * `target` - The exact target position.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::SeekFailed`] if EOF is reached before the target position.
    fn skip_to_exact(&mut self, target: Duration) -> Result<(), DecodeError> {
        loop {
            match self.decode_one()? {
                Some(frame) => {
                    let frame_time = frame.timestamp().as_duration();
                    if frame_time >= target {
                        // Reached or passed the target frame
                        // Position will be updated by decode_one() which was just called
                        break;
                    }
                    // Continue decoding (frame is automatically dropped)
                }
                None => {
                    // Reached EOF before finding target frame
                    return Err(DecodeError::SeekFailed {
                        target,
                        reason: "Reached end of stream before target position".to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Flushes the decoder's internal buffers.
    ///
    /// This clears any cached frames and resets the decoder state.
    /// The decoder is ready to receive new packets after flushing.
    pub(crate) fn flush(&mut self) {
        // SAFETY: the codec context was opened during construction.
        unsafe { self.codec_ctx.flush_buffers() };
        self.eof = false;
    }

    /// Scales a video frame to the specified dimensions while preserving aspect ratio.
    ///
    /// This method uses SwScale to resize frames efficiently using a "fit-within"
    /// strategy that preserves the original aspect ratio.
    ///
    /// # Aspect Ratio Preservation
    ///
    /// The frame is scaled to fit within `(target_width, target_height)` while
    /// maintaining its original aspect ratio. The output dimensions will be at most
    /// the target size, with at least one dimension matching the target. No letterboxing
    /// or pillarboxing is applied - the frame is simply scaled down to fit.
    ///
    /// # Arguments
    ///
    /// * `frame` - The source frame to scale.
    /// * `target_width` - Desired width in pixels.
    /// * `target_height` - Desired height in pixels.
    ///
    /// # Returns
    ///
    /// A new `VideoFrame` scaled to fit within the target dimensions.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] if SwScale context creation or scaling fails.
    ///
    /// # Performance
    ///
    /// - Caches SwScale context for repeated calls with same dimensions
    /// - Context creation: ~0.1-0.5ms (only on first call or dimension change)
    /// - Typical scaling time: 1-3ms for 1080p → 320x180
    /// - Uses bilinear interpolation for quality/performance balance
    ///
    /// # Cache Behavior
    ///
    /// The SwScale context is cached based on source/target dimensions and format.
    /// When generating multiple thumbnails with the same size (e.g., via `thumbnails()`),
    /// the context is reused, eliminating the ~0.1-0.5ms creation overhead per thumbnail.
    pub(crate) fn scale_frame(
        &mut self,
        frame: &VideoFrame,
        target_width: u32,
        target_height: u32,
    ) -> Result<VideoFrame, DecodeError> {
        let src_width = frame.width();
        let src_height = frame.height();
        let src_format = frame.format();

        // Calculate scaled dimensions to preserve aspect ratio (fit within target)
        let src_aspect = src_width as f64 / src_height as f64;
        let target_aspect = target_width as f64 / target_height as f64;

        let (scaled_width, scaled_height) = if src_aspect > target_aspect {
            // Source is wider - fit to width
            let height = (target_width as f64 / src_aspect).round() as u32;
            (target_width, height)
        } else {
            // Source is taller or equal - fit to height
            let width = (target_height as f64 * src_aspect).round() as u32;
            (width, target_height)
        };

        // Convert pixel format to FFmpeg format
        let av_format = Self::pixel_format_to_av(src_format);

        // Cache key: (src_width, src_height, scaled_width, scaled_height, format)
        let cache_key = (
            src_width,
            src_height,
            scaled_width,
            scaled_height,
            av_format,
        );

        // Reuse the cached SwScale context when the key matches; otherwise build a
        // fresh owned context into the cache slot. The cache key is only set after
        // scaling succeeds, so an error after a rebuild leaves the freshly built
        // context keyless — it is dropped/replaced on the next call rather than
        // being reused with a stale key.
        let is_cached =
            self.thumbnail_cache_key == Some(cache_key) && self.thumbnail_sws_ctx.is_some();

        if !is_cached {
            // Drop any stale cached context, then build the replacement.
            self.thumbnail_sws_ctx = None;
            self.thumbnail_cache_key = None;

            self.thumbnail_sws_ctx = Some(
                ff_sys::ScaleContext::new(
                    src_width as i32,
                    src_height as i32,
                    av_format,
                    scaled_width as i32,
                    scaled_height as i32,
                    av_format,
                    ff_sys::swscale::scale_flags::BILINEAR,
                )
                .map_err(|e| DecodeError::Ffmpeg {
                    code: 0,
                    message: format!("Failed to create scaling context: {e}"),
                })?,
            );
        }

        let Some(sws_ctx) = self.thumbnail_sws_ctx.as_mut() else {
            return Err(DecodeError::Ffmpeg {
                code: 0,
                message: "SwsContext not initialized".to_string(),
            });
        };

        // Borrow the source planes and strides directly from the VideoFrame; the
        // scaler reads them in place, so no scratch source frame is needed.
        let src_planes: Vec<&[u8]> = frame.planes().iter().map(AsRef::as_ref).collect();
        let src_strides: Vec<i32> = frame.strides().iter().map(|&s| s as i32).collect();

        // Allocate the destination frame (owned; freed on scope exit).
        let mut dst_frame = ff_sys::Frame::new().map_err(|e| DecodeError::Ffmpeg {
            code: e.code(),
            message: format!(
                "Failed to allocate frame: {}",
                ff_sys::av_error_string(e.code())
            ),
        })?;

        dst_frame.set_width(scaled_width as i32);
        dst_frame.set_height(scaled_height as i32);
        dst_frame.set_format(av_format);

        // Allocate buffer for destination frame
        if let Err(e) = dst_frame.get_buffer(0) {
            // On a rebuild the freshly built context stays in the cache slot but
            // keyless (thumbnail_cache_key was cleared above), so it is
            // dropped/replaced on the next call rather than reused.
            return Err(DecodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Failed to allocate destination frame buffer: {}",
                    ff_sys::av_error_string(e.code())
                ),
            });
        }

        // Perform scaling from the borrowed source planes into the owned frame.
        // SAFETY: `src_planes`/`src_strides` are the source frame's own planes and
        // positive strides sized for `src_height`; `dst_frame` was allocated via
        // `get_buffer` with the context's output geometry.
        if let Err(e) = unsafe {
            sws_ctx.scale_planes(&src_planes, &src_strides, src_height as i32, &mut dst_frame)
        } {
            // On a rebuild the freshly built context stays in the cache slot but
            // keyless (thumbnail_cache_key was cleared above), so it is
            // dropped/replaced on the next call rather than reused.
            return Err(DecodeError::Ffmpeg {
                code: 0,
                message: format!("Failed to scale frame: {e}"),
            });
        }

        // Scaling successful - record the cache key for the (re)built context.
        if !is_cached {
            self.thumbnail_cache_key = Some(cache_key);
        }

        // Copy timestamp
        dst_frame.set_pts(frame.timestamp().pts());

        // Convert destination frame to VideoFrame.
        // SAFETY: `dst_frame` holds valid scaled image data in `av_format`.
        let video_frame = unsafe { self.av_frame_to_video_frame(&dst_frame)? };

        Ok(video_frame)
    }

    // ── Reconnect helpers ─────────────────────────────────────────────────────

    /// Attempts to reconnect to the stream URL using exponential backoff.
    ///
    /// Called from `decode_one()` when `StreamInterrupted` is received and
    /// `NetworkOptions::reconnect_on_error` is `true`. After all attempts fail,
    /// returns a `StreamInterrupted` error.
    pub(super) fn attempt_reconnect(&mut self) -> Result<(), DecodeError> {
        let url = match self.url.as_deref() {
            Some(u) => u.to_owned(),
            None => return Ok(()), // file-path source: no reconnect
        };
        let max = self.network_opts.max_reconnect_attempts;

        for attempt in 1..=max {
            let backoff_ms = 100u64 * (1u64 << (attempt - 1).min(10));
            log::warn!(
                "reconnecting attempt={attempt} url={} backoff_ms={backoff_ms}",
                crate::network::sanitize_url(&url)
            );
            std::thread::sleep(Duration::from_millis(backoff_ms));
            match self.reopen(&url) {
                Ok(()) => {
                    self.reconnect_count += 1;
                    log::info!(
                        "reconnected attempt={attempt} url={} total_reconnects={}",
                        crate::network::sanitize_url(&url),
                        self.reconnect_count
                    );
                    return Ok(());
                }
                Err(e) => log::warn!("reconnect attempt={attempt} failed err={e}"),
            }
        }

        Err(DecodeError::StreamInterrupted {
            code: 0,
            endpoint: crate::network::sanitize_url(&url),
            message: format!("stream did not recover after {max} attempts"),
        })
    }

    /// Closes the current `AVFormatContext`, re-opens the URL, re-reads stream info,
    /// re-finds the video stream, and flushes the codec.
    fn reopen(&mut self, url: &str) -> Result<(), DecodeError> {
        // Re-open the URL with the stored network timeouts. Assigning the fresh
        // context drops the previous one, which closes and frees it.
        self.format_ctx = open_url_ctx(url, &self.network_opts)?;

        // Re-read stream information.
        self.format_ctx
            .find_stream_info()
            .map_err(|e| DecodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "reconnect find_stream_info failed: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

        // Re-find the video stream (index may differ in theory after reconnect).
        let (stream_index, _) = Self::find_video_stream(&self.format_ctx)
            .ok_or_else(|| DecodeError::NoVideoStream { path: url.into() })?;
        self.stream_index = stream_index as i32;

        // Flush codec buffers to discard stale decoded state from before the drop.
        // SAFETY: the codec context was opened during construction.
        unsafe { self.codec_ctx.flush_buffers() };

        self.eof = false;
        Ok(())
    }
}
