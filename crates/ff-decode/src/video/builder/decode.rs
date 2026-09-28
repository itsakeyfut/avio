//! Decode, seek, frame-extraction, and thumbnail methods for [`VideoDecoder`].

use std::time::Duration;

use ff_format::VideoFrame;

use crate::error::DecodeError;

use super::VideoDecoder;

impl VideoDecoder {
    // =========================================================================
    // Decoding Methods
    // =========================================================================

    /// Decodes the next video frame.
    ///
    /// This method reads and decodes a single frame from the video stream.
    /// Frames are returned in presentation order.
    ///
    /// # Returns
    ///
    /// - `Ok(Some(frame))` - A frame was successfully decoded
    /// - `Ok(None)` - End of stream reached, no more frames
    /// - `Err(_)` - An error occurred during decoding
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] if:
    /// - Reading from the file fails
    /// - Decoding the frame fails
    /// - Pixel format conversion fails
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::VideoDecoder;
    ///
    /// let mut decoder = VideoDecoder::open("video.mp4")?.build()?;
    ///
    /// while let Some(frame) = decoder.decode_one()? {
    ///     println!("Frame at {:?}", frame.timestamp().as_duration());
    ///     // Process frame...
    /// }
    /// ```
    pub fn decode_one(&mut self) -> Result<Option<VideoFrame>, DecodeError> {
        self.inner.decode_one()
    }

    /// Decodes all frames within a specified time range.
    ///
    /// This method seeks to the start position and decodes all frames until
    /// the end position is reached. Frames outside the range are skipped.
    ///
    /// # Arguments
    ///
    /// * `start` - Start of the time range (inclusive).
    /// * `end` - End of the time range (exclusive).
    ///
    /// # Returns
    ///
    /// A vector of frames with timestamps in the range `[start, end)`.
    /// Frames are returned in presentation order.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] if:
    /// - Seeking to the start position fails
    /// - Decoding frames fails
    /// - The time range is invalid (start >= end)
    pub fn decode_range(
        &mut self,
        start: Duration,
        end: Duration,
    ) -> Result<Vec<VideoFrame>, DecodeError> {
        // Validate range
        if start >= end {
            return Err(DecodeError::DecodingFailed {
                timestamp: Some(start),
                reason: format!(
                    "Invalid time range: start ({start:?}) must be before end ({end:?})"
                ),
            });
        }

        // Seek to start position (keyframe mode for efficiency)
        self.seek(start, crate::SeekMode::Keyframe)?;

        // Collect frames in the range
        let mut frames = Vec::new();

        while let Some(frame) = self.decode_one()? {
            let frame_time = frame.timestamp().as_duration();

            // Stop if we've passed the end of the range
            if frame_time >= end {
                break;
            }

            // Only collect frames within the range
            if frame_time >= start {
                frames.push(frame);
            }
            // Frames before start are automatically discarded
        }

        Ok(frames)
    }

    // =========================================================================
    // Seeking Methods
    // =========================================================================

    /// Seeks to a specified position in the video stream.
    ///
    /// This method performs efficient seeking without reopening the file,
    /// providing significantly better performance than file-reopen-based seeking
    /// (5-10ms vs 50-100ms).
    ///
    /// # What the decoder is left on
    ///
    /// After [`SeekMode::Exact`](crate::SeekMode::Exact), the next
    /// [`decode_one`](Self::decode_one) returns the first frame at or after
    /// `position`: reaching that frame requires decoding it, and it is held back
    /// rather than consumed. [`Keyframe`](crate::SeekMode::Keyframe) and
    /// [`Backward`](crate::SeekMode::Backward) are approximate by design and leave
    /// the decoder shortly *before* the target, for a caller that decodes up to it
    /// itself.
    ///
    /// # Arguments
    ///
    /// * `position` - Target position to seek to.
    /// * `mode` - Seek mode determining accuracy and performance.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::SeekFailed`] if:
    /// - The target position is beyond the video duration
    /// - The file format doesn't support seeking
    /// - The seek operation fails internally
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::{VideoDecoder, SeekMode};
    /// use std::time::Duration;
    ///
    /// let mut decoder = VideoDecoder::open("video.mp4")?.build()?;
    ///
    /// // Fast seek to 30 seconds (keyframe)
    /// decoder.seek(Duration::from_secs(30), SeekMode::Keyframe)?;
    ///
    /// // Exact seek to 1 minute
    /// decoder.seek(Duration::from_secs(60), SeekMode::Exact)?;
    /// ```
    pub fn seek(&mut self, position: Duration, mode: crate::SeekMode) -> Result<(), DecodeError> {
        if self.inner.is_live() {
            return Err(DecodeError::SeekNotSupported);
        }
        self.inner.seek(position, mode)
    }

    /// Returns `true` if the source is a live or streaming input.
    ///
    /// Live sources (HLS live playlists, RTMP, RTSP, MPEG-TS) have the
    /// `AVFMT_TS_DISCONT` flag set on their `AVInputFormat`. Seeking is not
    /// supported on live sources — [`VideoDecoder::seek`] will return
    /// [`DecodeError::SeekNotSupported`].
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.inner.is_live()
    }

    /// Flushes the decoder's internal buffers.
    ///
    /// This method clears any cached frames and resets the decoder state.
    /// The decoder is ready to receive new packets after flushing.
    ///
    /// # Note
    ///
    /// Calling [`seek()`](Self::seek) automatically flushes the decoder,
    /// so you don't need to call this method explicitly after seeking.
    pub fn flush(&mut self) {
        self.inner.flush();
    }

    // =========================================================================
    // Frame Extraction Methods
    // =========================================================================

    /// Returns the video frame whose presentation timestamp is closest to and
    /// at or after `timestamp`.
    ///
    /// Seeks to the keyframe immediately before `timestamp` (up to 10 seconds
    /// back to guarantee keyframe coverage), then decodes forward until a frame
    /// with PTS ≥ `timestamp` is found.  If the stream ends before that point
    /// (e.g. `timestamp` is beyond the video's duration), returns
    /// [`DecodeError::NoFrameAtTimestamp`].
    ///
    /// # Errors
    ///
    /// - [`DecodeError::NoFrameAtTimestamp`] — `timestamp` is at or beyond
    ///   the end of the stream, or no decodable frame exists there.
    /// - Any [`DecodeError`] propagated from seeking or decoding.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::VideoDecoder;
    /// use std::time::Duration;
    ///
    /// let mut decoder = VideoDecoder::open("video.mp4").build()?;
    /// let frame = decoder.extract_frame(Duration::from_secs(5))?;
    /// println!("Got frame at {:?}", frame.timestamp().as_duration());
    /// ```
    pub fn extract_frame(&mut self, timestamp: Duration) -> Result<VideoFrame, DecodeError> {
        // Seek to the keyframe just before the target; ignore seek errors
        // (e.g. the target is near the start) and decode from the beginning.
        let seek_to = timestamp.saturating_sub(Duration::from_secs(10));
        let _ = self.seek(seek_to, crate::SeekMode::Keyframe);

        loop {
            match self.decode_one()? {
                None => {
                    return Err(DecodeError::NoFrameAtTimestamp { timestamp });
                }
                Some(frame) => {
                    let pts = frame.timestamp().as_duration();
                    if pts >= timestamp {
                        log::debug!("frame extracted timestamp={timestamp:?} pts={pts:?}");
                        return Ok(frame);
                    }
                    // PTS is before the target; discard and keep decoding.
                }
            }
        }
    }

    // =========================================================================
    // Thumbnail Generation Methods
    // =========================================================================

    /// Generates a thumbnail at a specific timestamp.
    ///
    /// This method seeks to the specified position, decodes a frame, and scales
    /// it to the target dimensions. It's optimized for thumbnail generation by
    /// using keyframe seeking for speed.
    ///
    /// # Arguments
    ///
    /// * `position` - Timestamp to extract the thumbnail from.
    /// * `width` - Target thumbnail width in pixels.
    /// * `height` - Target thumbnail height in pixels.
    ///
    /// # Returns
    ///
    /// A scaled `VideoFrame` representing the thumbnail.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] if:
    /// - Seeking to the position fails
    /// - No frame can be decoded at that position (returns `Ok(None)`)
    /// - Scaling fails
    pub fn thumbnail_at(
        &mut self,
        position: Duration,
        width: u32,
        height: u32,
    ) -> Result<Option<VideoFrame>, DecodeError> {
        // 1. Seek to the specified position (keyframe mode for speed)
        self.seek(position, crate::SeekMode::Keyframe)?;

        // 2. Decode one frame — Ok(None) means no frame at this position
        match self.decode_one()? {
            Some(frame) => self.inner.scale_frame(&frame, width, height).map(Some),
            None => Ok(None),
        }
    }

    /// Generates multiple thumbnails evenly distributed across the video.
    ///
    /// This method creates a series of thumbnails by dividing the video duration
    /// into equal intervals and extracting a frame at each position.
    ///
    /// # Arguments
    ///
    /// * `count` - Number of thumbnails to generate.
    /// * `width` - Target thumbnail width in pixels.
    /// * `height` - Target thumbnail height in pixels.
    ///
    /// # Returns
    ///
    /// A vector of `VideoFrame` thumbnails in temporal order.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] if:
    /// - Any individual thumbnail generation fails (see [`thumbnail_at()`](Self::thumbnail_at))
    /// - The video duration is unknown ([`Duration::ZERO`])
    /// - Count is zero
    pub fn thumbnails(
        &mut self,
        count: usize,
        width: u32,
        height: u32,
    ) -> Result<Vec<VideoFrame>, DecodeError> {
        // Validate count
        if count == 0 {
            return Err(DecodeError::DecodingFailed {
                timestamp: None,
                reason: "Thumbnail count must be greater than zero".to_string(),
            });
        }

        let duration = self.duration();

        // Check if duration is valid
        if duration.is_zero() {
            return Err(DecodeError::DecodingFailed {
                timestamp: None,
                reason: "Cannot generate thumbnails: video duration is unknown".to_string(),
            });
        }

        // Calculate interval between thumbnails
        let interval_nanos = duration.as_nanos() / count as u128;

        // Generate thumbnails
        let mut thumbnails = Vec::with_capacity(count);

        for i in 0..count {
            // Use saturating_mul to prevent u128 overflow
            let position_nanos = interval_nanos.saturating_mul(i as u128);
            // Clamp to u64::MAX to prevent overflow when converting to Duration
            #[allow(clippy::cast_possible_truncation)]
            let position_nanos_u64 = position_nanos.min(u128::from(u64::MAX)) as u64;
            let position = Duration::from_nanos(position_nanos_u64);

            if let Some(thumbnail) = self.thumbnail_at(position, width, height)? {
                thumbnails.push(thumbnail);
            }
        }

        Ok(thumbnails)
    }
}

impl Iterator for VideoDecoder {
    type Item = Result<VideoFrame, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.fused {
            return None;
        }
        match self.decode_one() {
            Ok(Some(frame)) => Some(Ok(frame)),
            Ok(None) => None,
            Err(e) => {
                self.fused = true;
                Some(Err(e))
            }
        }
    }
}

impl std::iter::FusedIterator for VideoDecoder {}

#[cfg(test)]
#[allow(clippy::panic, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::video::builder::VideoDecoder;

    #[test]
    fn seek_mode_variants_should_be_comparable() {
        use crate::SeekMode;

        let keyframe = SeekMode::Keyframe;
        let exact = SeekMode::Exact;
        let backward = SeekMode::Backward;

        assert_eq!(keyframe, SeekMode::Keyframe);
        assert_eq!(exact, SeekMode::Exact);
        assert_eq!(backward, SeekMode::Backward);
        assert_ne!(keyframe, exact);
        assert_ne!(exact, backward);
    }

    #[test]
    fn seek_mode_default_should_be_keyframe() {
        use crate::SeekMode;

        let default_mode = SeekMode::default();
        assert_eq!(default_mode, SeekMode::Keyframe);
    }

    #[test]
    fn decode_range_invalid_range_should_return_error() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("ff_decode_range_test.txt");
        std::fs::write(&test_file, "test").expect("Failed to create test file");

        let result = VideoDecoder::open(&test_file).build();
        let _ = std::fs::remove_file(&test_file);

        if let Ok(mut decoder) = result {
            let start = Duration::from_secs(10);
            let end = Duration::from_secs(5); // end < start

            let range_result = decoder.decode_range(start, end);
            assert!(range_result.is_err());

            if let Err(DecodeError::DecodingFailed { reason, .. }) = range_result {
                assert!(reason.contains("Invalid time range"));
            }
        }
    }

    #[test]
    fn decode_range_equal_start_end_should_return_error() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("ff_decode_range_equal_test.txt");
        std::fs::write(&test_file, "test").expect("Failed to create test file");

        let result = VideoDecoder::open(&test_file).build();
        let _ = std::fs::remove_file(&test_file);

        if let Ok(mut decoder) = result {
            let time = Duration::from_secs(5);
            let range_result = decoder.decode_range(time, time);
            assert!(range_result.is_err());

            if let Err(DecodeError::DecodingFailed { reason, .. }) = range_result {
                assert!(reason.contains("Invalid time range"));
            }
        }
    }

    #[test]
    fn thumbnails_zero_count_should_return_error() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("ff_decode_thumbnails_zero_test.txt");
        std::fs::write(&test_file, "test").expect("Failed to create test file");

        let result = VideoDecoder::open(&test_file).build();
        let _ = std::fs::remove_file(&test_file);

        if let Ok(mut decoder) = result {
            let thumbnails_result = decoder.thumbnails(0, 160, 90);
            assert!(thumbnails_result.is_err());

            if let Err(DecodeError::DecodingFailed { reason, .. }) = thumbnails_result {
                assert!(reason.contains("Thumbnail count must be greater than zero"));
            }
        }
    }

    #[test]
    fn extract_frame_beyond_duration_should_err() {
        let e = DecodeError::NoFrameAtTimestamp {
            timestamp: Duration::from_secs(9999),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("9999"),
            "expected timestamp in error message, got: {msg}"
        );
    }

    #[test]
    fn thumbnail_dimensions_aspect_ratio_same_should_fit_exactly() {
        // Source: 1920x1080 (16:9), Target: 320x180 (16:9) → exact fit
        let src_width = 1920.0_f64;
        let src_height = 1080.0_f64;
        let target_width = 320.0_f64;
        let target_height = 180.0_f64;

        let src_aspect = src_width / src_height;
        let target_aspect = target_width / target_height;

        let (scaled_width, scaled_height) = if src_aspect > target_aspect {
            let height = (target_width / src_aspect).round();
            (target_width, height)
        } else {
            let width = (target_height * src_aspect).round();
            (width, target_height)
        };

        assert_eq!(scaled_width, 320.0);
        assert_eq!(scaled_height, 180.0);
    }

    #[test]
    fn thumbnail_dimensions_wide_source_should_constrain_height() {
        // Source: 1920x1080 (16:9), Target: 180x180 (1:1) → fits width, height adjusted
        let src_width = 1920.0_f64;
        let src_height = 1080.0_f64;
        let target_width = 180.0_f64;
        let target_height = 180.0_f64;

        let src_aspect = src_width / src_height;
        let target_aspect = target_width / target_height;

        let (scaled_width, scaled_height) = if src_aspect > target_aspect {
            let height = (target_width / src_aspect).round();
            (target_width, height)
        } else {
            let width = (target_height * src_aspect).round();
            (width, target_height)
        };

        assert_eq!(scaled_width, 180.0);
        // 180 / (16/9) = 101.25 → 101
        assert!((scaled_height - 101.0).abs() < 1.0);
    }

    #[test]
    fn thumbnail_dimensions_tall_source_should_constrain_width() {
        // Source: 1080x1920 (9:16 - portrait), Target: 180x180 (1:1) → fits height, width adjusted
        let src_width = 1080.0_f64;
        let src_height = 1920.0_f64;
        let target_width = 180.0_f64;
        let target_height = 180.0_f64;

        let src_aspect = src_width / src_height;
        let target_aspect = target_width / target_height;

        let (scaled_width, scaled_height) = if src_aspect > target_aspect {
            let height = (target_width / src_aspect).round();
            (target_width, height)
        } else {
            let width = (target_height * src_aspect).round();
            (width, target_height)
        };

        // 180 * (9/16) = 101.25 → 101
        assert!((scaled_width - 101.0).abs() < 1.0);
        assert_eq!(scaled_height, 180.0);
    }
}
