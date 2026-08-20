//! Audio encoder builder and public API.
//!
//! This module provides [`AudioEncoderBuilder`] for fluent configuration and
//! [`AudioEncoder`] for encoding audio frames to a file.

use std::path::PathBuf;
use std::time::Instant;

use ff_format::AudioFrame;

use super::codec_options::{AudioCodecOptions, Mp3Quality};
use super::encoder_inner::{AudioEncoderConfig, AudioEncoderInner};
use crate::{AudioCodec, EncodeError, OutputContainer};

/// Builder for constructing an [`AudioEncoder`].
///
/// Created by calling [`AudioEncoder::create()`]. Call [`build()`](Self::build)
/// to open the output file and prepare for encoding.
///
/// # Examples
///
/// ```ignore
/// use ff_encode::{AudioEncoder, AudioCodec};
///
/// let mut encoder = AudioEncoder::create("output.m4a")
///     .audio(48000, 2)
///     .audio_codec(AudioCodec::Aac)
///     .build()?;
/// ```
pub struct AudioEncoderBuilder {
    pub(crate) path: PathBuf,
    pub(crate) container: Option<OutputContainer>,
    pub(crate) audio_sample_rate: Option<u32>,
    pub(crate) audio_channels: Option<u32>,
    pub(crate) audio_codec: AudioCodec,
    pub(crate) audio_bitrate: Option<u64>,
    pub(crate) codec_options: Option<AudioCodecOptions>,
    pub(crate) audio_codec_explicit: bool,
}

impl AudioEncoderBuilder {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            container: None,
            audio_sample_rate: None,
            audio_channels: None,
            audio_codec: AudioCodec::default(),
            audio_bitrate: None,
            codec_options: None,
            audio_codec_explicit: false,
        }
    }

    /// Configure audio stream settings.
    #[must_use]
    pub fn audio(mut self, sample_rate: u32, channels: u32) -> Self {
        self.audio_sample_rate = Some(sample_rate);
        self.audio_channels = Some(channels);
        self
    }

    /// Set audio codec.
    #[must_use]
    pub fn audio_codec(mut self, codec: AudioCodec) -> Self {
        self.audio_codec = codec;
        self.audio_codec_explicit = true;
        self
    }

    /// Set audio bitrate in bits per second.
    #[must_use]
    pub fn audio_bitrate(mut self, bitrate: u64) -> Self {
        self.audio_bitrate = Some(bitrate);
        self
    }

    /// Set container format explicitly (usually auto-detected from file extension).
    #[must_use]
    pub fn container(mut self, container: OutputContainer) -> Self {
        self.container = Some(container);
        self
    }

    /// Set per-codec encoding options.
    ///
    /// The variant must match the codec set via [`audio_codec()`](Self::audio_codec).
    /// A mismatch is silently ignored.
    #[must_use]
    pub fn codec_options(mut self, opts: AudioCodecOptions) -> Self {
        self.codec_options = Some(opts);
        self
    }

    fn apply_container_defaults(&mut self) {
        let is_flac = self
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("flac"))
            || self
                .container
                .as_ref()
                .is_some_and(|c| *c == OutputContainer::Flac);
        if is_flac && !self.audio_codec_explicit {
            self.audio_codec = AudioCodec::Flac;
        }

        let is_ogg = self
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("ogg"))
            || self
                .container
                .as_ref()
                .is_some_and(|c| *c == OutputContainer::Ogg);
        if is_ogg && !self.audio_codec_explicit {
            self.audio_codec = AudioCodec::Vorbis;
        }
    }

    /// Validate builder state and open the output file.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] if configuration is invalid, the output path
    /// cannot be created, or no suitable encoder is found.
    pub fn build(self) -> Result<AudioEncoder, EncodeError> {
        AudioEncoder::from_builder(self)
    }
}

/// Encodes audio frames to a file using `FFmpeg`.
///
/// # Construction
///
/// Use [`AudioEncoder::create()`] to get an [`AudioEncoderBuilder`], then call
/// [`AudioEncoderBuilder::build()`]:
///
/// ```ignore
/// use ff_encode::{AudioEncoder, AudioCodec};
///
/// let mut encoder = AudioEncoder::create("output.m4a")
///     .audio(48000, 2)
///     .audio_codec(AudioCodec::Aac)
///     .build()?;
/// ```
pub struct AudioEncoder {
    inner: Option<AudioEncoderInner>,
    _config: AudioEncoderConfig,
    _start_time: Instant,
}

impl AudioEncoder {
    /// Creates a builder for the specified output file path.
    ///
    /// This method is infallible. Validation occurs when
    /// [`AudioEncoderBuilder::build()`] is called.
    pub fn create<P: AsRef<std::path::Path>>(path: P) -> AudioEncoderBuilder {
        AudioEncoderBuilder::new(path.as_ref().to_path_buf())
    }

    pub(crate) fn from_builder(mut builder: AudioEncoderBuilder) -> Result<Self, EncodeError> {
        builder.apply_container_defaults();

        // Enforce FLAC container codec allowlist.
        let is_flac = builder
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("flac"))
            || builder
                .container
                .as_ref()
                .is_some_and(|c| *c == OutputContainer::Flac);
        if is_flac && !matches!(builder.audio_codec, AudioCodec::Flac) {
            return Err(EncodeError::UnsupportedContainerCodecCombination {
                container: "flac".to_string(),
                codec: builder.audio_codec.name().to_string(),
                hint: "FLAC container only supports the FLAC codec".to_string(),
            });
        }

        // Enforce OGG container codec allowlist.
        let is_ogg = builder
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("ogg"))
            || builder
                .container
                .as_ref()
                .is_some_and(|c| *c == OutputContainer::Ogg);
        if is_ogg && !matches!(builder.audio_codec, AudioCodec::Vorbis | AudioCodec::Opus) {
            return Err(EncodeError::UnsupportedContainerCodecCombination {
                container: "ogg".to_string(),
                codec: builder.audio_codec.name().to_string(),
                hint: "OGG container supports Vorbis and Opus".to_string(),
            });
        }

        // Validate per-codec options before constructing the inner encoder.
        if let Some(AudioCodecOptions::Opus(ref opts)) = builder.codec_options
            && let Some(dur) = opts.frame_duration_ms
            && ![2u32, 5, 10, 20, 40, 60].contains(&dur)
        {
            return Err(EncodeError::InvalidOption {
                name: "frame_duration_ms".to_string(),
                reason: "must be one of: 2, 5, 10, 20, 40, 60".to_string(),
            });
        }
        if let Some(AudioCodecOptions::Aac(ref opts)) = builder.codec_options
            && let Some(q) = opts.vbr_quality
            && !(1..=5).contains(&q)
        {
            return Err(EncodeError::InvalidOption {
                name: "vbr_quality".to_string(),
                reason: "must be 1–5".to_string(),
            });
        }
        if let Some(AudioCodecOptions::Mp3(ref opts)) = builder.codec_options
            && let Mp3Quality::Vbr(q) = opts.quality
            && q > 9
        {
            return Err(EncodeError::InvalidOption {
                name: "vbr_quality".to_string(),
                reason: "must be 0–9 (0=best)".to_string(),
            });
        }
        if let Some(AudioCodecOptions::Flac(ref opts)) = builder.codec_options
            && opts.compression_level > 12
        {
            return Err(EncodeError::InvalidOption {
                name: "compression_level".to_string(),
                reason: "must be 0–12".to_string(),
            });
        }

        // Validate channel count and sample rate before constructing inner.
        if let Some(ch) = builder.audio_channels
            && ch > 8
        {
            log::warn!("audio channel count out of range count={ch} maximum=8");
            return Err(EncodeError::InvalidChannelCount { count: ch });
        }
        if let Some(sr) = builder.audio_sample_rate
            && !(8_000..=384_000).contains(&sr)
        {
            log::warn!("audio sample rate out of range rate={sr} minimum=8000 maximum=384000");
            return Err(EncodeError::InvalidSampleRate { rate: sr });
        }

        let config = AudioEncoderConfig {
            path: builder.path.clone(),
            sample_rate: builder
                .audio_sample_rate
                .ok_or_else(|| EncodeError::InvalidConfig {
                    reason: "Audio sample rate not configured".to_string(),
                })?,
            channels: builder
                .audio_channels
                .ok_or_else(|| EncodeError::InvalidConfig {
                    reason: "Audio channels not configured".to_string(),
                })?,
            codec: builder.audio_codec,
            bitrate: builder.audio_bitrate,
            codec_options: builder.codec_options,
            _progress_callback: false,
        };

        let inner = Some(AudioEncoderInner::new(&config)?);

        Ok(Self {
            inner,
            _config: config,
            _start_time: Instant::now(),
        })
    }

    /// Returns the name of the `FFmpeg` encoder actually used (e.g. `"aac"`, `"libopus"`).
    #[must_use]
    pub fn actual_codec(&self) -> &str {
        self.inner
            .as_ref()
            .map_or("", |inner| inner.actual_codec.as_str())
    }

    /// Pushes an audio frame for encoding.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] if encoding fails or the encoder is not initialised.
    pub fn push(&mut self, frame: &AudioFrame) -> Result<(), EncodeError> {
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Audio encoder not initialized".to_string(),
            })?;
        inner.push_frame(frame)?;
        Ok(())
    }

    /// Flushes remaining frames and writes the file trailer.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] if finalising fails.
    pub fn finish(mut self) -> Result<(), EncodeError> {
        if let Some(mut inner) = self.inner.take() {
            inner.finish()?;
        }
        Ok(())
    }
}

impl Drop for AudioEncoder {
    fn drop(&mut self) {
        // AudioEncoderInner handles cleanup in its own Drop.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_should_return_builder_without_error() {
        let _builder: AudioEncoderBuilder = AudioEncoder::create("output.m4a");
    }

    #[test]
    fn builder_audio_settings_should_be_stored() {
        let builder = AudioEncoder::create("output.m4a")
            .audio(48000, 2)
            .audio_codec(AudioCodec::Aac)
            .audio_bitrate(192_000);
        assert_eq!(builder.audio_sample_rate, Some(48000));
        assert_eq!(builder.audio_channels, Some(2));
        assert_eq!(builder.audio_codec, AudioCodec::Aac);
        assert_eq!(builder.audio_bitrate, Some(192_000));
    }

    #[test]
    fn build_without_sample_rate_should_return_error() {
        let result = AudioEncoder::create("output.m4a").build();
        assert!(result.is_err());
    }

    #[test]
    fn flac_extension_without_explicit_codec_should_default_to_flac() {
        let builder = AudioEncoder::create("output.flac").audio(44100, 2);
        let mut b = builder;
        b.apply_container_defaults();
        assert_eq!(b.audio_codec, AudioCodec::Flac);
    }

    #[test]
    fn ogg_extension_without_explicit_codec_should_default_to_vorbis() {
        let builder = AudioEncoder::create("output.ogg").audio(44100, 2);
        let mut b = builder;
        b.apply_container_defaults();
        assert_eq!(b.audio_codec, AudioCodec::Vorbis);
    }

    #[test]
    fn flac_extension_with_explicit_codec_should_not_override() {
        let builder = AudioEncoder::create("output.flac")
            .audio(44100, 2)
            .audio_codec(AudioCodec::Flac);
        let mut b = builder;
        b.apply_container_defaults();
        assert_eq!(b.audio_codec, AudioCodec::Flac);
    }

    #[test]
    fn flac_container_enum_without_explicit_codec_should_default_to_flac() {
        let builder = AudioEncoder::create("output.audio")
            .audio(44100, 2)
            .container(OutputContainer::Flac);
        let mut b = builder;
        b.apply_container_defaults();
        assert_eq!(b.audio_codec, AudioCodec::Flac);
    }

    #[test]
    fn ogg_container_enum_without_explicit_codec_should_default_to_vorbis() {
        let builder = AudioEncoder::create("output.audio")
            .audio(44100, 2)
            .container(OutputContainer::Ogg);
        let mut b = builder;
        b.apply_container_defaults();
        assert_eq!(b.audio_codec, AudioCodec::Vorbis);
    }

    #[test]
    fn flac_extension_with_incompatible_codec_should_return_error() {
        let result = AudioEncoder::create("output.flac")
            .audio(44100, 2)
            .audio_codec(AudioCodec::Mp3)
            .build();
        assert!(
            matches!(
                result,
                Err(EncodeError::UnsupportedContainerCodecCombination {
                    ref container,
                    ..
                }) if container == "flac"
            ),
            "expected UnsupportedContainerCodecCombination for flac"
        );
    }

    #[test]
    fn ogg_extension_with_incompatible_codec_should_return_error() {
        let result = AudioEncoder::create("output.ogg")
            .audio(44100, 2)
            .audio_codec(AudioCodec::Mp3)
            .build();
        assert!(
            matches!(
                result,
                Err(EncodeError::UnsupportedContainerCodecCombination {
                    ref container,
                    ..
                }) if container == "ogg"
            ),
            "expected UnsupportedContainerCodecCombination for ogg"
        );
    }

    #[test]
    fn ogg_with_opus_should_pass_validation() {
        // Opus is a valid OGG codec — validation should not reject it.
        // (build() will fail due to missing sample-rate check, but not with
        // UnsupportedContainerCodecCombination.)
        let result = AudioEncoder::create("output.ogg")
            .audio_codec(AudioCodec::Opus)
            .build();
        assert!(!matches!(
            result,
            Err(EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn non_flac_ogg_extension_should_not_enforce_container_codecs() {
        // A plain .mp3 path should not trigger FLAC/OGG enforcement.
        let result = AudioEncoder::create("output.mp3")
            .audio_codec(AudioCodec::Flac)
            .build();
        assert!(!matches!(
            result,
            Err(EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }
}
