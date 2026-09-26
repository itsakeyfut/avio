//! Audio decoder builder for constructing audio decoders with custom configuration.
//!
//! This module provides the [`AudioDecoderBuilder`] type which enables fluent
//! configuration of audio decoders. Use [`AudioDecoder::open()`] to start building.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ff_format::{AudioFrame, AudioStreamInfo, ContainerInfo, NetworkOptions, SampleFormat};

use crate::audio::decoder_inner::AudioDecoderInner;
use crate::error::DecodeError;

/// Builder for configuring and constructing an [`AudioDecoder`].
///
/// This struct provides a fluent interface for setting up decoder options
/// before opening an audio file. It is created by calling [`AudioDecoder::open()`].
///
/// # Examples
///
/// ## Basic Usage
///
/// ```ignore
/// use ff_decode::AudioDecoder;
///
/// let decoder = AudioDecoder::open("audio.mp3")?
///     .build()?;
/// ```
///
/// ## With Custom Format and Sample Rate
///
/// ```ignore
/// use ff_decode::AudioDecoder;
/// use ff_format::SampleFormat;
///
/// let decoder = AudioDecoder::open("audio.mp3")?
///     .output_format(SampleFormat::F32)
///     .output_sample_rate(48000)
///     .build()?;
/// ```
#[derive(Debug)]
pub struct AudioDecoderBuilder {
    /// Path to the media file
    path: PathBuf,
    /// Output sample format (None = use source format)
    output_format: Option<SampleFormat>,
    /// Output sample rate (None = use source sample rate)
    output_sample_rate: Option<u32>,
    /// Output channel count (None = use source channel count)
    output_channels: Option<u32>,
    /// Network options for URL-based sources (None = use defaults)
    network_opts: Option<NetworkOptions>,
}

impl AudioDecoderBuilder {
    /// Creates a new builder for the specified file path.
    ///
    /// This is an internal constructor; use [`AudioDecoder::open()`] instead.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            output_format: None,
            output_sample_rate: None,
            output_channels: None,
            network_opts: None,
        }
    }

    /// Sets the output sample format for decoded frames.
    ///
    /// If not set, frames are returned in the source format. Setting an
    /// output format enables automatic conversion during decoding.
    ///
    /// # Common Formats
    ///
    /// - [`SampleFormat::F32`] - 32-bit float, most common for editing
    /// - [`SampleFormat::I16`] - 16-bit integer, CD quality
    /// - [`SampleFormat::F32p`] - Planar 32-bit float, efficient for processing
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::AudioDecoder;
    /// use ff_format::SampleFormat;
    ///
    /// let decoder = AudioDecoder::open("audio.mp3")?
    ///     .output_format(SampleFormat::F32)
    ///     .build()?;
    /// ```
    #[must_use]
    pub fn output_format(mut self, format: SampleFormat) -> Self {
        self.output_format = Some(format);
        self
    }

    /// Sets the output sample rate in Hz.
    ///
    /// If not set, frames are returned at the source sample rate. Setting an
    /// output sample rate enables automatic resampling during decoding.
    ///
    /// # Common Sample Rates
    ///
    /// - 44100 Hz - CD quality audio
    /// - 48000 Hz - Professional audio, most common in video
    /// - 96000 Hz - High-resolution audio
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::AudioDecoder;
    ///
    /// // Resample to 48kHz
    /// let decoder = AudioDecoder::open("audio.mp3")?
    ///     .output_sample_rate(48000)
    ///     .build()?;
    /// ```
    #[must_use]
    pub fn output_sample_rate(mut self, sample_rate: u32) -> Self {
        self.output_sample_rate = Some(sample_rate);
        self
    }

    /// Sets the output channel count.
    ///
    /// If not set, frames are returned with the source channel count. Setting an
    /// output channel count enables automatic channel remixing during decoding.
    ///
    /// # Common Channel Counts
    ///
    /// - 1 - Mono
    /// - 2 - Stereo
    /// - 6 - 5.1 surround sound
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::AudioDecoder;
    ///
    /// // Convert to stereo
    /// let decoder = AudioDecoder::open("audio.mp3")?
    ///     .output_channels(2)
    ///     .build()?;
    /// ```
    #[must_use]
    pub fn output_channels(mut self, channels: u32) -> Self {
        self.output_channels = Some(channels);
        self
    }

    /// Sets network options for URL-based audio sources (HTTP, RTSP, RTMP, etc.).
    ///
    /// When set, the builder skips the file-existence check and passes connect
    /// and read timeouts to `avformat_open_input` via an `AVDictionary`.
    /// Call this before `.build()` when opening `rtmp://`, `rtsp://`, `http://`,
    /// `https://`, `udp://`, `srt://`, or `rtp://` URLs.
    ///
    /// # HLS / M3U8 Playlists
    ///
    /// Audio-only HLS playlists (`.m3u8` pointing to AAC or MP3 segments) are
    /// detected automatically by `FFmpeg`. Pass the full HTTP(S) URL:
    ///
    /// ```ignore
    /// use ff_decode::AudioDecoder;
    /// use ff_format::NetworkOptions;
    ///
    /// let decoder = AudioDecoder::open("https://example.com/audio/index.m3u8")
    ///     .network(NetworkOptions::default())
    ///     .build()?;
    /// ```
    ///
    /// # UDP / MPEG-TS
    ///
    /// `udp://` URLs are always live — `is_live()` returns `true` and seeking
    /// is not supported. Two extra `AVDictionary` options are set automatically
    /// to reduce packet loss on high-bitrate streams:
    ///
    /// | Option | Value | Reason |
    /// |---|---|---|
    /// | `buffer_size` | `65536` | Enlarges the UDP receive buffer |
    /// | `overrun_nonfatal` | `1` | Discards excess data instead of erroring |
    ///
    /// # SRT (Secure Reliable Transport)
    ///
    /// SRT URLs (`srt://host:port`) require the `srt` feature flag **and** a
    /// libsrt-enabled `FFmpeg` build.  Enable the feature in `Cargo.toml`:
    ///
    /// ```toml
    /// [dependencies]
    /// ff-decode = { version = "*", features = ["srt"] }
    /// ```
    ///
    /// Without the `srt` feature, opening an `srt://` URL returns
    /// [`DecodeError::ConnectionFailed`]. If the feature is enabled but the
    /// linked `FFmpeg` was not built with `--enable-libsrt`, the same error is
    /// returned with a message directing you to rebuild `FFmpeg`.
    ///
    /// ```ignore
    /// use ff_decode::AudioDecoder;
    /// use ff_format::NetworkOptions;
    ///
    /// let decoder = AudioDecoder::open("srt://ingest.example.com:4200")
    ///     .network(NetworkOptions::default())
    ///     .build()?;
    /// ```
    ///
    /// # Credentials
    ///
    /// HTTP basic-auth credentials must be embedded directly in the URL:
    /// `https://user:password@cdn.example.com/audio/index.m3u8`.
    /// The password is redacted in log output.
    ///
    /// # DRM Limitation
    ///
    /// DRM-protected streams are **not** supported:
    /// - HLS: `FairPlay`, Widevine, AES-128 with external key servers
    /// - DASH: CENC, `PlayReady`, Widevine
    ///
    /// `FFmpeg` can parse the manifest and fetch segments, but key delivery
    /// to a DRM license server is outside the scope of this API.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::AudioDecoder;
    /// use ff_format::NetworkOptions;
    /// use std::time::Duration;
    ///
    /// let decoder = AudioDecoder::open("http://stream.example.com/audio.aac")
    ///     .network(NetworkOptions {
    ///         connect_timeout: Duration::from_secs(5),
    ///         ..Default::default()
    ///     })
    ///     .build()?;
    /// ```
    #[must_use]
    pub fn network(mut self, opts: NetworkOptions) -> Self {
        self.network_opts = Some(opts);
        self
    }

    /// Returns the configured file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the configured output format, if any.
    #[must_use]
    pub fn get_output_format(&self) -> Option<SampleFormat> {
        self.output_format
    }

    /// Returns the configured output sample rate, if any.
    #[must_use]
    pub fn get_output_sample_rate(&self) -> Option<u32> {
        self.output_sample_rate
    }

    /// Returns the configured output channel count, if any.
    #[must_use]
    pub fn get_output_channels(&self) -> Option<u32> {
        self.output_channels
    }

    /// Builds the audio decoder with the configured options.
    ///
    /// This method opens the media file, initializes the decoder context,
    /// and prepares for frame decoding.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The file cannot be found ([`DecodeError::FileNotFound`])
    /// - The file contains no audio stream ([`DecodeError::NoAudioStream`])
    /// - The codec is not supported ([`DecodeError::UnsupportedCodec`])
    /// - Other `FFmpeg` errors occur ([`DecodeError::Ffmpeg`])
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::AudioDecoder;
    ///
    /// let decoder = AudioDecoder::open("audio.mp3")?
    ///     .build()?;
    ///
    /// // Start decoding
    /// for result in &mut decoder {
    ///     let frame = result?;
    ///     // Process frame...
    /// }
    /// ```
    pub fn build(self) -> Result<AudioDecoder, DecodeError> {
        // Network URLs skip the file-existence check (literal path does not exist).
        let is_network_url = self.path.to_str().is_some_and(crate::network::is_url);
        if !is_network_url && !self.path.exists() {
            return Err(DecodeError::FileNotFound {
                path: self.path.clone(),
            });
        }

        // Create the decoder inner
        let (inner, stream_info, container_info) = AudioDecoderInner::new(
            &self.path,
            self.output_format,
            self.output_sample_rate,
            self.output_channels,
            self.network_opts,
        )?;

        Ok(AudioDecoder {
            path: self.path,
            inner,
            stream_info,
            container_info,
            fused: false,
        })
    }
}

/// An audio decoder for extracting audio frames from media files.
///
/// The decoder provides frame-by-frame access to audio content with support
/// for resampling and format conversion.
///
/// # Construction
///
/// Use [`AudioDecoder::open()`] to create a builder, then call [`AudioDecoderBuilder::build()`]:
///
/// ```ignore
/// use ff_decode::AudioDecoder;
/// use ff_format::SampleFormat;
///
/// let decoder = AudioDecoder::open("audio.mp3")?
///     .output_format(SampleFormat::F32)
///     .output_sample_rate(48000)
///     .build()?;
/// ```
///
/// # Frame Decoding
///
/// Frames can be decoded one at a time or using an iterator:
///
/// ```ignore
/// // Decode one frame
/// if let Some(frame) = decoder.decode_one()? {
///     println!("Frame with {} samples", frame.samples());
/// }
///
/// // Iterator form — AudioDecoder implements Iterator directly
/// for result in &mut decoder {
///     let frame = result?;
///     // Process frame...
/// }
/// ```
///
/// # Seeking
///
/// The decoder supports seeking to specific positions:
///
/// ```ignore
/// use std::time::Duration;
///
/// // Seek to 30 seconds
/// decoder.seek(Duration::from_secs(30))?;
/// ```
pub struct AudioDecoder {
    /// Path to the media file
    path: PathBuf,
    /// Internal decoder state
    inner: AudioDecoderInner,
    /// Audio stream information
    stream_info: AudioStreamInfo,
    /// Container-level metadata
    container_info: ContainerInfo,
    /// Set to `true` after a decoding error; causes [`Iterator::next`] to return `None`.
    fused: bool,
}

impl AudioDecoder {
    /// Opens a media file and returns a builder for configuring the decoder.
    ///
    /// This is the entry point for creating a decoder. The returned builder
    /// allows setting options before the decoder is fully initialized.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the media file to decode.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::AudioDecoder;
    ///
    /// // Simple usage
    /// let decoder = AudioDecoder::open("audio.mp3")?
    ///     .build()?;
    ///
    /// // With options
    /// let decoder = AudioDecoder::open("audio.mp3")?
    ///     .output_format(SampleFormat::F32)
    ///     .output_sample_rate(48000)
    ///     .build()?;
    /// ```
    ///
    /// # Note
    ///
    /// This method does not validate that the file exists or is a valid
    /// media file. Validation occurs when [`AudioDecoderBuilder::build()`] is called.
    pub fn open(path: impl AsRef<Path>) -> AudioDecoderBuilder {
        AudioDecoderBuilder::new(path.as_ref().to_path_buf())
    }

    // =========================================================================
    // Information Methods
    // =========================================================================

    /// Returns the audio stream information.
    ///
    /// This contains metadata about the audio stream including sample rate,
    /// channel count, codec, and format characteristics.
    #[must_use]
    pub fn stream_info(&self) -> &AudioStreamInfo {
        &self.stream_info
    }

    /// Returns the sample rate in Hz.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.stream_info.sample_rate()
    }

    /// Returns the number of audio channels.
    ///
    /// The type is `u32` to match `FFmpeg` and professional audio APIs. When
    /// integrating with `rodio` or `cpal` (which require `u16`), cast with
    /// `decoder.channels() as u16` — channel counts never exceed `u16::MAX`
    /// in practice.
    #[must_use]
    pub fn channels(&self) -> u32 {
        self.stream_info.channels()
    }

    /// Returns the duration of the **audio stream** this decoder opened.
    ///
    /// Not the container's duration, which follows whichever stream is longest: a file
    /// whose video outruns its audio says more than the audio holds (#1861). For the
    /// duration of the *file*, probe it: `MediaInfo::duration` is the container's by
    /// contract.
    ///
    /// The container's duration is the **fallback** for a stream that does not carry
    /// its own, which some formats leave unset.
    ///
    /// Returns [`Duration::ZERO`] if neither is known.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.stream_info.duration().unwrap_or(Duration::ZERO)
    }

    /// Returns the duration of the audio stream as [`duration`](Self::duration) reads
    /// it, or `None` for live streams and formats that carry no duration at all.
    #[must_use]
    pub fn duration_opt(&self) -> Option<Duration> {
        self.stream_info.duration()
    }

    /// Returns container-level metadata (format name, bitrate, stream count).
    #[must_use]
    pub fn container_info(&self) -> &ContainerInfo {
        &self.container_info
    }

    /// Returns the current playback position.
    #[must_use]
    pub fn position(&self) -> Duration {
        self.inner.position()
    }

    /// Returns `true` once the decoder has been fully drained.
    ///
    /// This becomes `true` only when no further frame will be returned. Reaching
    /// the end of the file is not enough on its own: a decoder still holds
    /// buffered frames at that point, and those are returned first.
    #[must_use]
    pub fn is_eof(&self) -> bool {
        self.inner.is_eof()
    }

    /// Returns the file path being decoded.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    // =========================================================================
    // Decoding Methods
    // =========================================================================

    /// Decodes the next audio frame.
    ///
    /// This method reads and decodes a single frame from the audio stream.
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
    /// - Sample format conversion fails
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::AudioDecoder;
    ///
    /// let mut decoder = AudioDecoder::open("audio.mp3")?.build()?;
    ///
    /// while let Some(frame) = decoder.decode_one()? {
    ///     println!("Frame with {} samples", frame.samples());
    ///     // Process frame...
    /// }
    /// ```
    pub fn decode_one(&mut self) -> Result<Option<AudioFrame>, DecodeError> {
        self.inner.decode_one()
    }

    /// Decodes all frames and returns their raw PCM data.
    ///
    /// This method decodes the entire audio file and returns all samples
    /// as a contiguous byte buffer.
    ///
    /// # Performance
    ///
    /// - Memory scales with audio duration and quality
    /// - For 10 minutes of stereo 48kHz F32 audio: ~110 MB
    ///
    /// # Returns
    ///
    /// A byte vector containing all audio samples in the configured output format.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] if:
    /// - Decoding any frame fails
    /// - The file cannot be read
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::AudioDecoder;
    /// use ff_format::SampleFormat;
    ///
    /// let mut decoder = AudioDecoder::open("audio.mp3")?
    ///     .output_format(SampleFormat::F32)
    ///     .build()?;
    ///
    /// let samples = decoder.decode_all()?;
    /// println!("Decoded {} bytes", samples.len());
    /// ```
    ///
    /// # Memory Usage
    ///
    /// Stereo 48kHz F32 audio:
    /// - 1 minute: ~11 MB
    /// - 5 minutes: ~55 MB
    /// - 10 minutes: ~110 MB
    pub fn decode_all(&mut self) -> Result<Vec<u8>, DecodeError> {
        let mut buffer = Vec::new();

        while let Some(frame) = self.decode_one()? {
            // Collect samples from all planes
            for plane in frame.planes() {
                buffer.extend_from_slice(plane);
            }
        }

        Ok(buffer)
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
    /// A byte vector containing audio samples within `[start, end)`.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] if:
    /// - Seeking to the start position fails
    /// - Decoding frames fails
    /// - The time range is invalid (start >= end)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::AudioDecoder;
    /// use std::time::Duration;
    ///
    /// let mut decoder = AudioDecoder::open("audio.mp3")?.build()?;
    ///
    /// // Decode audio from 5s to 10s
    /// let samples = decoder.decode_range(
    ///     Duration::from_secs(5),
    ///     Duration::from_secs(10),
    /// )?;
    ///
    /// println!("Decoded {} bytes", samples.len());
    /// ```
    pub fn decode_range(&mut self, start: Duration, end: Duration) -> Result<Vec<u8>, DecodeError> {
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
        let mut buffer = Vec::new();

        while let Some(frame) = self.decode_one()? {
            let frame_time = frame.timestamp().as_duration();

            // Stop if we've passed the end of the range
            if frame_time >= end {
                break;
            }

            // Only collect frames within the range
            if frame_time >= start {
                for plane in frame.planes() {
                    buffer.extend_from_slice(plane);
                }
            }
        }

        Ok(buffer)
    }

    // =========================================================================
    // Seeking Methods
    // =========================================================================

    /// Seeks to a specified position in the audio stream.
    ///
    /// This method performs efficient seeking without reopening the file.
    ///
    /// # Arguments
    ///
    /// * `position` - Target position to seek to.
    /// * `mode` - Seek mode (Keyframe, Exact, or Backward).
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::SeekFailed`] if:
    /// - The target position is beyond the audio duration
    /// - The file format doesn't support seeking
    /// - The seek operation fails internally
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use ff_decode::{AudioDecoder, SeekMode};
    /// use std::time::Duration;
    ///
    /// let mut decoder = AudioDecoder::open("audio.mp3")?.build()?;
    ///
    /// // Seek to 30 seconds with keyframe mode (fast)
    /// decoder.seek(Duration::from_secs(30), SeekMode::Keyframe)?;
    ///
    /// // Seek to exact position (slower but precise)
    /// decoder.seek(Duration::from_secs(45), SeekMode::Exact)?;
    ///
    /// // Decode next frame
    /// if let Some(frame) = decoder.decode_one()? {
    ///     println!("Frame at {:?}", frame.timestamp().as_duration());
    /// }
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
    /// supported on live sources — [`AudioDecoder::seek`] will return
    /// [`DecodeError::SeekNotSupported`].
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.inner.is_live()
    }

    /// Flushes the decoder's internal buffers.
    ///
    /// This method clears any cached frames and resets the decoder state.
    /// The decoder is ready to receive new packets after flushing.
    pub fn flush(&mut self) {
        self.inner.flush();
    }
}

impl Iterator for AudioDecoder {
    type Item = Result<AudioFrame, DecodeError>;

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

impl std::iter::FusedIterator for AudioDecoder {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_builder_default_values() {
        let builder = AudioDecoderBuilder::new(PathBuf::from("test.mp3"));

        assert_eq!(builder.path(), Path::new("test.mp3"));
        assert!(builder.get_output_format().is_none());
        assert!(builder.get_output_sample_rate().is_none());
        assert!(builder.get_output_channels().is_none());
    }

    #[test]
    fn test_builder_output_format() {
        let builder =
            AudioDecoderBuilder::new(PathBuf::from("test.mp3")).output_format(SampleFormat::F32);

        assert_eq!(builder.get_output_format(), Some(SampleFormat::F32));
    }

    #[test]
    fn test_builder_output_sample_rate() {
        let builder = AudioDecoderBuilder::new(PathBuf::from("test.mp3")).output_sample_rate(48000);

        assert_eq!(builder.get_output_sample_rate(), Some(48000));
    }

    #[test]
    fn test_builder_output_channels() {
        let builder = AudioDecoderBuilder::new(PathBuf::from("test.mp3")).output_channels(2);

        assert_eq!(builder.get_output_channels(), Some(2));
    }

    #[test]
    fn test_builder_chaining() {
        let builder = AudioDecoderBuilder::new(PathBuf::from("test.mp3"))
            .output_format(SampleFormat::F32)
            .output_sample_rate(48000)
            .output_channels(2);

        assert_eq!(builder.get_output_format(), Some(SampleFormat::F32));
        assert_eq!(builder.get_output_sample_rate(), Some(48000));
        assert_eq!(builder.get_output_channels(), Some(2));
    }

    #[test]
    fn test_decoder_open() {
        let builder = AudioDecoder::open("audio.mp3");
        assert_eq!(builder.path(), Path::new("audio.mp3"));
    }

    #[test]
    fn test_build_file_not_found() {
        let result = AudioDecoder::open("nonexistent_file_12345.mp3").build();

        assert!(result.is_err());
        match result {
            Err(DecodeError::FileNotFound { path }) => {
                assert!(
                    path.to_string_lossy()
                        .contains("nonexistent_file_12345.mp3")
                );
            }
            Err(e) => panic!("Expected FileNotFound error, got: {e:?}"),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn network_setter_should_store_options() {
        let opts = NetworkOptions::default();
        let builder = AudioDecoderBuilder::new(PathBuf::from("test.mp3")).network(opts.clone());
        assert_eq!(builder.network_opts, Some(opts));
    }

    #[test]
    fn build_should_bypass_file_existence_check_for_network_url() {
        // A network URL that clearly does not exist locally should not return
        // FileNotFound — it will return a different error (or succeed) from
        // FFmpeg's network layer. The important thing is that FileNotFound is
        // NOT returned.
        let result = AudioDecoder::open("http://192.0.2.1/nonexistent.aac").build();
        assert!(
            !matches!(result, Err(DecodeError::FileNotFound { .. })),
            "FileNotFound must not be returned for network URLs"
        );
    }
}
