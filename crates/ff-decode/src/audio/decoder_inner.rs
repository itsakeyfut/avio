//! Internal audio decoder implementation using FFmpeg.
//!
//! This module contains the low-level decoder logic that directly interacts
//! with FFmpeg's C API through the ff-sys crate. It is not exposed publicly.

// Allow unsafe code in this module as it's necessary for FFmpeg FFI
#![allow(unsafe_code)]
// Allow specific clippy lints for FFmpeg FFI code
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::unnecessary_cast)]
#![allow(clippy::if_not_else)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::cast_lossless)]

use std::ffi::CStr;
use std::path::Path;
use std::ptr;
use std::time::Duration;

use ff_format::channel::ChannelLayout;
use ff_format::codec::AudioCodec;
use ff_format::container::ContainerInfo;
use ff_format::{AudioFrame, AudioStreamInfo, NetworkOptions, SampleFormat};
use ff_sys::{
    AVCodecContext, AVCodecID, AVFormatContext, AVFrame, AVMediaType_AVMEDIA_TYPE_AUDIO, AVPacket,
};

use super::resample_inner;

use crate::error::DecodeError;
use crate::shared::guards_inner::{AvFormatContextGuard, AvFrameGuard, AvPacketGuard};

/// Internal decoder state holding FFmpeg contexts.
///
/// This structure manages the lifecycle of FFmpeg objects and is responsible
/// for proper cleanup when dropped.
pub(crate) struct AudioDecoderInner {
    /// Format context for reading the media file
    format_ctx: *mut AVFormatContext,
    /// Codec context for decoding audio frames
    codec_ctx: ff_sys::CodecContext,
    /// Audio stream index in the format context
    stream_index: i32,
    /// Target output sample format (if conversion is needed)
    output_format: Option<SampleFormat>,
    /// Target output sample rate (if resampling is needed)
    output_sample_rate: Option<u32>,
    /// Target output channel count (if remixing is needed)
    output_channels: Option<u32>,
    /// Cached `SwrContext` — reused across frames to preserve FIR filter state.
    swr_ctx: Option<resample_inner::SwrContextGuard>,
    /// Key for the cached context; rebuilt when source or target parameters change.
    swr_key: Option<resample_inner::SwrKey>,
    /// Whether the source is a live/streaming input (seeking is not supported)
    is_live: bool,
    /// Whether end of file has been reached
    eof: bool,
    /// Current playback position
    position: Duration,
    /// Reusable packet for reading from file
    packet: *mut AVPacket,
    /// Reusable frame for decoding
    frame: *mut AVFrame,
    /// URL used to open this source — `None` for file-path sources.
    url: Option<String>,
    /// Network options used for the initial open (timeouts, reconnect config).
    network_opts: NetworkOptions,
    /// Number of successful reconnects so far (for logging).
    reconnect_count: u32,
}

impl AudioDecoderInner {
    /// Opens a media file and initializes the audio decoder.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the media file
    /// * `output_format` - Optional target sample format for conversion
    /// * `output_sample_rate` - Optional target sample rate for resampling
    /// * `output_channels` - Optional target channel count for remixing
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The file cannot be opened
    /// - No audio stream is found
    /// - The codec is not supported
    /// - Decoder initialization fails
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        path: &Path,
        output_format: Option<SampleFormat>,
        output_sample_rate: Option<u32>,
        output_channels: Option<u32>,
        network_opts: Option<NetworkOptions>,
    ) -> Result<(Self, AudioStreamInfo, ContainerInfo), DecodeError> {
        // Ensure FFmpeg is initialized (thread-safe and idempotent)
        ff_sys::ensure_initialized();

        let path_str = path.to_str().unwrap_or("");
        let is_network_url = crate::network::is_url(path_str);

        let url = if is_network_url {
            Some(path_str.to_owned())
        } else {
            None
        };
        let stored_network_opts = network_opts.clone().unwrap_or_default();

        // Verify SRT availability before attempting to open (feature + runtime check).
        if is_network_url {
            crate::network::check_srt_url(path_str)?;
        }

        // Open the input source (with RAII guard)
        // SAFETY: Path is valid, AvFormatContextGuard ensures cleanup
        let format_ctx_guard = unsafe {
            if is_network_url {
                let network = network_opts.unwrap_or_default();
                log::info!(
                    "opening network audio source url={} connect_timeout_ms={} read_timeout_ms={}",
                    crate::network::sanitize_url(path_str),
                    network.connect_timeout.as_millis(),
                    network.read_timeout.as_millis()
                );
                AvFormatContextGuard::new_url(path_str, &network)?
            } else {
                AvFormatContextGuard::new(path)?
            }
        };
        let format_ctx = format_ctx_guard.as_ptr();

        // Read stream information
        // SAFETY: format_ctx is valid and owned by guard
        unsafe {
            ff_sys::avformat::find_stream_info(format_ctx).map_err(|e| DecodeError::Ffmpeg {
                code: e,
                message: format!("Failed to find stream info: {}", ff_sys::av_error_string(e)),
            })?;
        }

        // Detect live/streaming source via the AVFMT_TS_DISCONT flag on AVInputFormat.
        // SAFETY: format_ctx is valid and non-null; iformat is set by avformat_open_input
        //         and is non-null for all successfully opened formats.
        let is_live = unsafe {
            let iformat = (*format_ctx).iformat;
            !iformat.is_null() && ((*iformat).flags & ff_sys::AVFMT_TS_DISCONT) != 0
        };

        // Find the audio stream
        // SAFETY: format_ctx is valid
        let (stream_index, codec_id) =
            unsafe { Self::find_audio_stream(format_ctx) }.ok_or_else(|| {
                DecodeError::NoAudioStream {
                    path: path.to_path_buf(),
                }
            })?;

        // Find the decoder for this codec
        // SAFETY: codec_id is valid from FFmpeg
        let codec_name = unsafe { Self::extract_codec_name(codec_id) };
        let codec = unsafe {
            ff_sys::avcodec::find_decoder(codec_id).ok_or_else(|| {
                DecodeError::UnsupportedCodec {
                    codec: format!("{codec_name} (codec_id={codec_id:?})"),
                }
            })?
        };

        // Allocate codec context (freed on drop by CodecContext).
        // SAFETY: codec pointer is valid.
        let mut codec_ctx =
            unsafe { ff_sys::CodecContext::new(codec) }.map_err(|e| DecodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Failed to allocate codec context: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

        // Copy codec parameters from stream to context
        // SAFETY: format_ctx is valid, stream_index is valid; codec_ctx is owned
        unsafe {
            let stream = (*format_ctx).streams.add(stream_index as usize);
            let codecpar = (*(*stream)).codecpar;
            codec_ctx
                .parameters_to_context(codecpar)
                .map_err(|e| DecodeError::Ffmpeg {
                    code: e.code(),
                    message: format!(
                        "Failed to copy codec parameters: {}",
                        ff_sys::av_error_string(e.code())
                    ),
                })?;
        }

        // Open the codec
        // SAFETY: codec is valid; codec_ctx is owned
        unsafe {
            codec_ctx
                .open(codec, ptr::null_mut())
                .map_err(|e| DecodeError::Ffmpeg {
                    code: e.code(),
                    message: format!(
                        "Failed to open codec: {}",
                        ff_sys::av_error_string(e.code())
                    ),
                })?;
        }

        // Extract stream information
        // SAFETY: All pointers are valid
        let stream_info = unsafe {
            Self::extract_stream_info(format_ctx, stream_index as i32, codec_ctx.as_mut_ptr())?
        };

        // Extract container information
        // SAFETY: format_ctx is valid and avformat_find_stream_info has been called
        let container_info = unsafe { Self::extract_container_info(format_ctx) };

        // Allocate packet and frame (with RAII guards)
        // SAFETY: FFmpeg is initialized, guards ensure cleanup
        let packet_guard = unsafe { AvPacketGuard::new()? };
        let frame_guard = unsafe { AvFrameGuard::new()? };

        // All initialization successful - transfer ownership to AudioDecoderInner
        Ok((
            Self {
                format_ctx: format_ctx_guard.into_raw(),
                codec_ctx,
                stream_index: stream_index as i32,
                output_format,
                output_sample_rate,
                output_channels,
                swr_ctx: None,
                swr_key: None,
                is_live,
                eof: false,
                position: Duration::ZERO,
                packet: packet_guard.into_raw(),
                frame: frame_guard.into_raw(),
                url,
                network_opts: stored_network_opts,
                reconnect_count: 0,
            },
            stream_info,
            container_info,
        ))
    }

    /// Finds the first audio stream in the format context.
    ///
    /// # Returns
    ///
    /// Returns `Some((index, codec_id))` if an audio stream is found, `None` otherwise.
    ///
    /// # Safety
    ///
    /// Caller must ensure `format_ctx` is valid and initialized.
    unsafe fn find_audio_stream(format_ctx: *mut AVFormatContext) -> Option<(usize, AVCodecID)> {
        // SAFETY: Caller ensures format_ctx is valid
        unsafe {
            let nb_streams = (*format_ctx).nb_streams as usize;

            for i in 0..nb_streams {
                let stream = (*format_ctx).streams.add(i);
                let codecpar = (*(*stream)).codecpar;

                if (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_AUDIO {
                    return Some((i, (*codecpar).codec_id));
                }
            }

            None
        }
    }

    /// Returns the human-readable codec name for a given `AVCodecID`.
    unsafe fn extract_codec_name(codec_id: ff_sys::AVCodecID) -> String {
        // SAFETY: avcodec_get_name is safe for any codec ID value
        let name_ptr = unsafe { ff_sys::avcodec_get_name(codec_id) };
        if name_ptr.is_null() {
            return String::from("unknown");
        }
        // SAFETY: avcodec_get_name returns a valid C string with static lifetime
        unsafe { CStr::from_ptr(name_ptr).to_string_lossy().into_owned() }
    }

    /// Extracts audio stream information from FFmpeg structures.
    unsafe fn extract_stream_info(
        format_ctx: *mut AVFormatContext,
        stream_index: i32,
        codec_ctx: *mut AVCodecContext,
    ) -> Result<AudioStreamInfo, DecodeError> {
        // SAFETY: Caller ensures all pointers are valid
        let (sample_rate, channels, sample_fmt, duration_val, channel_layout, codec_id) = unsafe {
            let stream = (*format_ctx).streams.add(stream_index as usize);
            let codecpar = (*(*stream)).codecpar;

            (
                (*codecpar).sample_rate as u32,
                (*codecpar).ch_layout.nb_channels as u32,
                (*codec_ctx).sample_fmt,
                (*format_ctx).duration,
                (*codecpar).ch_layout,
                (*codecpar).codec_id,
            )
        };

        // Extract duration
        let duration = if duration_val > 0 {
            let duration_secs = duration_val as f64 / 1_000_000.0;
            Some(Duration::from_secs_f64(duration_secs))
        } else {
            None
        };

        // Extract sample format
        let sample_format = resample_inner::convert_sample_format(sample_fmt);

        // Extract channel layout
        let channel_layout_enum = Self::convert_channel_layout(&channel_layout, channels);

        // Extract codec
        let codec = Self::convert_codec(codec_id);
        let codec_name = unsafe { Self::extract_codec_name(codec_id) };

        // Build stream info
        let mut builder = AudioStreamInfo::builder()
            .index(stream_index as u32)
            .codec(codec)
            .codec_name(codec_name)
            .sample_rate(sample_rate)
            .channels(channels)
            .sample_format(sample_format)
            .channel_layout(channel_layout_enum);

        if let Some(d) = duration {
            builder = builder.duration(d);
        }

        Ok(builder.build())
    }

    /// Extracts container-level information from the `AVFormatContext`.
    ///
    /// # Safety
    ///
    /// Caller must ensure `format_ctx` is valid and `avformat_find_stream_info` has been called.
    unsafe fn extract_container_info(format_ctx: *mut AVFormatContext) -> ContainerInfo {
        // SAFETY: Caller ensures format_ctx is valid
        unsafe {
            let format_name = if (*format_ctx).iformat.is_null() {
                String::new()
            } else {
                let ptr = (*(*format_ctx).iformat).name;
                if ptr.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                }
            };

            let bit_rate = {
                let br = (*format_ctx).bit_rate;
                if br > 0 { Some(br as u64) } else { None }
            };

            let nb_streams = (*format_ctx).nb_streams as u32;

            let mut builder = ContainerInfo::builder()
                .format_name(format_name)
                .nb_streams(nb_streams);
            if let Some(br) = bit_rate {
                builder = builder.bit_rate(br);
            }
            builder.build()
        }
    }

    /// Converts FFmpeg channel layout to our `ChannelLayout` enum.
    fn convert_channel_layout(layout: &ff_sys::AVChannelLayout, channels: u32) -> ChannelLayout {
        if layout.order == ff_sys::AVChannelOrder_AV_CHANNEL_ORDER_NATIVE {
            // SAFETY: When order is AV_CHANNEL_ORDER_NATIVE, the mask field is valid
            let mask = unsafe { layout.u.mask };
            match mask {
                0x4 => ChannelLayout::Mono,
                0x3 => ChannelLayout::Stereo,
                0x103 => ChannelLayout::Stereo2_1,
                0x7 => ChannelLayout::Surround3_0,
                0x33 => ChannelLayout::Quad,
                0x37 => ChannelLayout::Surround5_0,
                0x3F => ChannelLayout::Surround5_1,
                0x13F => ChannelLayout::Surround6_1,
                0x63F => ChannelLayout::Surround7_1,
                _ => {
                    log::warn!(
                        "channel_layout mask has no mapping, deriving from channel count \
                         mask={mask} channels={channels}"
                    );
                    ChannelLayout::from_channels(channels)
                }
            }
        } else {
            log::warn!(
                "channel_layout order is not NATIVE, deriving from channel count \
                 order={order} channels={channels}",
                order = layout.order
            );
            ChannelLayout::from_channels(channels)
        }
    }

    /// Converts FFmpeg codec ID to our `AudioCodec` enum.
    fn convert_codec(codec_id: AVCodecID) -> AudioCodec {
        if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_AAC {
            AudioCodec::Aac
        } else if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_MP3 {
            AudioCodec::Mp3
        } else if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_OPUS {
            AudioCodec::Opus
        } else if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_VORBIS {
            AudioCodec::Vorbis
        } else if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_FLAC {
            AudioCodec::Flac
        } else if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_PCM_S16LE {
            AudioCodec::Pcm
        } else {
            log::warn!(
                "audio codec unsupported, falling back to Aac codec_id={codec_id} fallback=Aac"
            );
            AudioCodec::Aac
        }
    }

    /// Decodes the next audio frame.
    ///
    /// Transparently reconnects on `StreamInterrupted` when
    /// `NetworkOptions::reconnect_on_error` is enabled.
    ///
    /// # Returns
    ///
    /// - `Ok(Some(frame))` - Successfully decoded a frame
    /// - `Ok(None)` - End of stream reached
    /// - `Err(_)` - Decoding error occurred
    pub(crate) fn decode_one(&mut self) -> Result<Option<AudioFrame>, DecodeError> {
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

    fn decode_one_inner(&mut self) -> Result<Option<AudioFrame>, DecodeError> {
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
                        // Successfully received a frame
                        let audio_frame = resample_inner::convert_frame_to_audio_frame(
                            self.frame,
                            self.format_ctx,
                            self.stream_index,
                            self.output_format,
                            self.output_sample_rate,
                            self.output_channels,
                            &mut self.swr_ctx,
                            &mut self.swr_key,
                        )?;

                        // Update position based on frame timestamp
                        let pts = (*self.frame).pts;
                        if pts != ff_sys::AV_NOPTS_VALUE {
                            let stream = (*self.format_ctx).streams.add(self.stream_index as usize);
                            let time_base = (*(*stream)).time_base;
                            let timestamp_secs =
                                pts as f64 * time_base.num as f64 / time_base.den as f64;
                            self.position = Duration::from_secs_f64(timestamp_secs);
                        }

                        return Ok(Some(audio_frame));
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

                        // Check if this packet belongs to the audio stream
                        if (*self.packet).stream_index == self.stream_index {
                            // Send the packet to the decoder
                            let send_result = self.codec_ctx.send_packet(self.packet);
                            ff_sys::av_packet_unref(self.packet);

                            if let Err(se) = send_result
                                && !se.is_eagain()
                            {
                                return Err(DecodeError::Ffmpeg {
                                    code: se.code(),
                                    message: format!(
                                        "Failed to send packet: {}",
                                        ff_sys::av_error_string(se.code())
                                    ),
                                });
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
    fn duration_to_pts(&self, duration: Duration) -> i64 {
        // SAFETY: format_ctx and stream_index are valid (owned by AudioDecoderInner)
        let time_base = unsafe {
            let stream = (*self.format_ctx).streams.add(self.stream_index as usize);
            (*(*stream)).time_base
        };

        // Convert: duration (seconds) * (time_base.den / time_base.num) = PTS
        let time_base_f64 = time_base.den as f64 / time_base.num as f64;
        (duration.as_secs_f64() * time_base_f64) as i64
    }

    /// Seeks to a specified position in the audio stream.
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
        let flags = ff_sys::avformat::seek_flags::BACKWARD;

        // 1. Clear any pending packet and frame
        // SAFETY: packet and frame are valid (owned by AudioDecoderInner)
        unsafe {
            ff_sys::av_packet_unref(self.packet);
            ff_sys::av_frame_unref(self.frame);
        }

        // 2. Seek in the format context
        // SAFETY: format_ctx and stream_index are valid
        unsafe {
            ff_sys::avformat::seek_frame(self.format_ctx, self.stream_index, timestamp, flags)
                .map_err(|e| DecodeError::SeekFailed {
                    target: position,
                    reason: ff_sys::av_error_string(e),
                })?;
        }

        // 3. Flush decoder buffers and reset the cached SwrContext so the
        //    resampler does not carry stale delay samples across the seek point.
        // SAFETY: the codec context was opened during construction.
        unsafe { self.codec_ctx.flush_buffers() };
        self.swr_ctx = None;
        self.swr_key = None;

        // 4. Drain any remaining frames from the decoder after flush
        // SAFETY: codec_ctx and frame are valid
        unsafe {
            // Drain while frames are produced. NeedInput / Drained / real errors all
            // end draining (preserves the pre-migration `Err(_) => break` behaviour that
            // swallowed errors here).
            while let Ok(ff_sys::ReceiveOutcome::Frame) = self.codec_ctx.receive_frame(self.frame) {
                ff_sys::av_frame_unref(self.frame);
            }
        }

        // 5. Reset internal state
        self.eof = false;

        // 6. For exact mode, skip frames to reach exact position
        if mode == SeekMode::Exact {
            self.skip_to_exact(position)?;
        }
        // For Keyframe/Backward modes, we're already at the keyframe after av_seek_frame

        Ok(())
    }

    /// Skips frames until reaching the exact target position.
    ///
    /// This is used by [`Self::seek`] when `SeekMode::Exact` is specified.
    ///
    /// # Arguments
    ///
    /// * `target` - The exact target position.
    fn skip_to_exact(&mut self, target: Duration) -> Result<(), DecodeError> {
        // Decode frames until we reach or pass the target
        while let Some(frame) = self.decode_one()? {
            let frame_time = frame.timestamp().as_duration();
            if frame_time >= target {
                // We've reached the target position
                break;
            }
            // Continue decoding to get closer (frames are automatically dropped)
        }
        Ok(())
    }

    /// Flushes the decoder's internal buffers.
    pub(crate) fn flush(&mut self) {
        // SAFETY: the codec context was opened during construction.
        unsafe { self.codec_ctx.flush_buffers() };
        self.eof = false;
    }

    // ── Reconnect helpers ─────────────────────────────────────────────────────

    /// Attempts to reconnect to the stream URL using exponential backoff.
    ///
    /// Called from `decode_one()` when `StreamInterrupted` is received and
    /// `NetworkOptions::reconnect_on_error` is `true`. After all attempts fail,
    /// returns a `StreamInterrupted` error.
    fn attempt_reconnect(&mut self) -> Result<(), DecodeError> {
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
    /// re-finds the audio stream, and flushes the codec.
    fn reopen(&mut self, url: &str) -> Result<(), DecodeError> {
        // Close the current format context. `avformat_close_input` sets the pointer
        // to null — this matches the null check in Drop so no double-free occurs.
        // SAFETY: self.format_ctx is valid and owned exclusively by self.
        unsafe {
            ff_sys::avformat::close_input(std::ptr::addr_of_mut!(self.format_ctx));
        }

        // Re-open the URL with the stored network timeouts.
        // SAFETY: url is a valid UTF-8 network URL string.
        self.format_ctx = unsafe {
            ff_sys::avformat::open_input_url(
                url,
                self.network_opts.connect_timeout,
                self.network_opts.read_timeout,
            )
            .map_err(|e| crate::network::map_network_error(e, crate::network::sanitize_url(url)))?
        };

        // Re-read stream information.
        // SAFETY: self.format_ctx is valid and freshly opened.
        unsafe {
            ff_sys::avformat::find_stream_info(self.format_ctx).map_err(|e| {
                DecodeError::Ffmpeg {
                    code: e,
                    message: format!(
                        "reconnect find_stream_info failed: {}",
                        ff_sys::av_error_string(e)
                    ),
                }
            })?;
        }

        // Re-find the audio stream (index may differ in theory after reconnect).
        // SAFETY: self.format_ctx is valid.
        let (stream_index, _) = unsafe { Self::find_audio_stream(self.format_ctx) }
            .ok_or_else(|| DecodeError::NoAudioStream { path: url.into() })?;
        self.stream_index = stream_index as i32;

        // Flush codec buffers to discard stale decoded state from before the drop.
        // SAFETY: the codec context was opened during construction.
        unsafe { self.codec_ctx.flush_buffers() };

        self.eof = false;
        Ok(())
    }
}

impl Drop for AudioDecoderInner {
    fn drop(&mut self) {
        // Free frame and packet
        if !self.frame.is_null() {
            // SAFETY: self.frame is valid and owned by this instance
            unsafe {
                ff_sys::av_frame_free(&mut (self.frame as *mut _));
            }
        }

        if !self.packet.is_null() {
            // SAFETY: self.packet is valid and owned by this instance
            unsafe {
                ff_sys::av_packet_free(&mut (self.packet as *mut _));
            }
        }

        // Close format context
        if !self.format_ctx.is_null() {
            // SAFETY: self.format_ctx is valid and owned by this instance
            unsafe {
                ff_sys::avformat::close_input(&mut (self.format_ctx as *mut _));
            }
        }
    }
}

// SAFETY: AudioDecoderInner manages FFmpeg contexts which are thread-safe when not shared.
// We don't expose mutable access across threads, so Send is safe.
unsafe impl Send for AudioDecoderInner {}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use ff_format::channel::ChannelLayout;

    use super::AudioDecoderInner;

    /// Constructs an `AVChannelLayout` with `AV_CHANNEL_ORDER_NATIVE` and the given mask.
    fn native_layout(mask: u64, nb_channels: i32) -> ff_sys::AVChannelLayout {
        ff_sys::AVChannelLayout {
            order: ff_sys::AVChannelOrder_AV_CHANNEL_ORDER_NATIVE,
            nb_channels,
            u: ff_sys::AVChannelLayout__bindgen_ty_1 { mask },
            opaque: std::ptr::null_mut(),
        }
    }

    /// Constructs an `AVChannelLayout` with `AV_CHANNEL_ORDER_UNSPEC`.
    fn unspec_layout(nb_channels: i32) -> ff_sys::AVChannelLayout {
        ff_sys::AVChannelLayout {
            order: ff_sys::AVChannelOrder_AV_CHANNEL_ORDER_UNSPEC,
            nb_channels,
            u: ff_sys::AVChannelLayout__bindgen_ty_1 { mask: 0 },
            opaque: std::ptr::null_mut(),
        }
    }

    #[test]
    fn native_mask_mono() {
        let layout = native_layout(0x4, 1);
        assert_eq!(
            AudioDecoderInner::convert_channel_layout(&layout, 1),
            ChannelLayout::Mono
        );
    }

    #[test]
    fn native_mask_stereo() {
        let layout = native_layout(0x3, 2);
        assert_eq!(
            AudioDecoderInner::convert_channel_layout(&layout, 2),
            ChannelLayout::Stereo
        );
    }

    #[test]
    fn native_mask_stereo2_1() {
        let layout = native_layout(0x103, 3);
        assert_eq!(
            AudioDecoderInner::convert_channel_layout(&layout, 3),
            ChannelLayout::Stereo2_1
        );
    }

    #[test]
    fn native_mask_surround3_0() {
        let layout = native_layout(0x7, 3);
        assert_eq!(
            AudioDecoderInner::convert_channel_layout(&layout, 3),
            ChannelLayout::Surround3_0
        );
    }

    #[test]
    fn native_mask_quad() {
        let layout = native_layout(0x33, 4);
        assert_eq!(
            AudioDecoderInner::convert_channel_layout(&layout, 4),
            ChannelLayout::Quad
        );
    }

    #[test]
    fn native_mask_surround5_0() {
        let layout = native_layout(0x37, 5);
        assert_eq!(
            AudioDecoderInner::convert_channel_layout(&layout, 5),
            ChannelLayout::Surround5_0
        );
    }

    #[test]
    fn native_mask_surround5_1() {
        let layout = native_layout(0x3F, 6);
        assert_eq!(
            AudioDecoderInner::convert_channel_layout(&layout, 6),
            ChannelLayout::Surround5_1
        );
    }

    #[test]
    fn native_mask_surround6_1() {
        let layout = native_layout(0x13F, 7);
        assert_eq!(
            AudioDecoderInner::convert_channel_layout(&layout, 7),
            ChannelLayout::Surround6_1
        );
    }

    #[test]
    fn native_mask_surround7_1() {
        let layout = native_layout(0x63F, 8);
        assert_eq!(
            AudioDecoderInner::convert_channel_layout(&layout, 8),
            ChannelLayout::Surround7_1
        );
    }

    #[test]
    fn native_mask_unknown_falls_back_to_from_channels() {
        // mask=0x1 is not a standard layout; should fall back to from_channels(2)
        let layout = native_layout(0x1, 2);
        assert_eq!(
            AudioDecoderInner::convert_channel_layout(&layout, 2),
            ChannelLayout::from_channels(2)
        );
    }

    #[test]
    fn non_native_order_falls_back_to_from_channels() {
        let layout = unspec_layout(6);
        assert_eq!(
            AudioDecoderInner::convert_channel_layout(&layout, 6),
            ChannelLayout::from_channels(6)
        );
    }

    // -------------------------------------------------------------------------
    // extract_codec_name
    // -------------------------------------------------------------------------

    #[test]
    fn codec_name_should_return_h264_for_h264_codec_id() {
        let name =
            unsafe { AudioDecoderInner::extract_codec_name(ff_sys::AVCodecID_AV_CODEC_ID_H264) };
        assert_eq!(name, "h264");
    }

    #[test]
    fn codec_name_should_return_none_for_none_codec_id() {
        let name =
            unsafe { AudioDecoderInner::extract_codec_name(ff_sys::AVCodecID_AV_CODEC_ID_NONE) };
        assert_eq!(name, "none");
    }

    #[test]
    fn unsupported_codec_error_should_include_codec_name() {
        let codec_id = ff_sys::AVCodecID_AV_CODEC_ID_MP3;
        let codec_name = unsafe { AudioDecoderInner::extract_codec_name(codec_id) };
        let error = crate::error::DecodeError::UnsupportedCodec {
            codec: format!("{codec_name} (codec_id={codec_id:?})"),
        };
        let msg = error.to_string();
        assert!(msg.contains("mp3"), "expected codec name in error: {msg}");
        assert!(
            msg.contains("codec_id="),
            "expected codec_id in error: {msg}"
        );
    }
}
