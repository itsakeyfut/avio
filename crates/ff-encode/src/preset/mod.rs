//! Export preset types and predefined presets.
//!
//! [`ExportPreset`] bundles a [`VideoEncoderConfig`] and an [`AudioEncoderConfig`]
//! into a named snapshot that can be applied to a [`VideoEncoderBuilder`] before
//! calling `.build()`.
//!
//! # Examples
//!
//! ```ignore
//! use ff_encode::{ExportPreset, VideoEncoder};
//!
//! let preset = ExportPreset::youtube_1080p();
//! preset.validate()?;
//!
//! let mut encoder = preset
//!     .apply_video(VideoEncoder::create("output.mp4"))
//!     .build()?;
//! ```

mod presets;
mod validation;

use ff_format::{PixelFormat, VideoCodec};

use crate::video::codec_options::VideoCodecOptions;
use crate::{AudioCodec, BitrateMode, EncodeError, VideoEncoderBuilder};

/// Configuration for the video stream of an export preset.
///
/// Fields with `Option` type are not applied to the builder when `None`,
/// allowing the builder's existing values (or defaults) to be preserved.
#[derive(Debug, Clone)]
pub struct VideoEncoderConfig {
    /// Video codec.
    pub codec: VideoCodec,
    /// Output width in pixels. `None` = preserve source width.
    pub width: Option<u32>,
    /// Output height in pixels. `None` = preserve source height.
    pub height: Option<u32>,
    /// Output frame rate. `None` = preserve source frame rate.
    pub fps: Option<f64>,
    /// Bitrate control mode.
    pub bitrate_mode: BitrateMode,
    /// Optional pixel format override. `None` lets the encoder choose.
    pub pixel_format: Option<PixelFormat>,
    /// Optional per-codec advanced options.
    pub codec_options: Option<VideoCodecOptions>,
}

/// Configuration for the audio stream of an export preset.
#[derive(Debug, Clone)]
pub struct AudioEncoderConfig {
    /// Audio codec.
    pub codec: AudioCodec,
    /// Sample rate in Hz (e.g. 48000).
    pub sample_rate: u32,
    /// Number of audio channels (1 = mono, 2 = stereo).
    pub channels: u32,
    /// Audio bitrate in bits per second (e.g. `192_000` = 192 kbps).
    pub bitrate: u64,
}

/// A named export preset combining video and audio encoder configuration.
///
/// Create a predefined preset with [`ExportPreset::youtube_1080p()`] etc., or
/// build a custom one as a struct literal. Call [`validate()`](Self::validate)
/// before encoding to catch platform-constraint violations early.
///
/// `video: None` indicates an audio-only preset (e.g. [`podcast_mono`](Self::podcast_mono)).
///
/// # Examples
///
/// ```ignore
/// use ff_encode::{ExportPreset, VideoEncoder};
///
/// let preset = ExportPreset::youtube_1080p();
/// preset.validate()?;
///
/// let mut encoder = preset
///     .apply_video(VideoEncoder::create("output.mp4"))
///     .build()?;
/// ```
#[derive(Debug, Clone)]
pub struct ExportPreset {
    /// Human-readable name (e.g. `"youtube_1080p"`).
    pub name: String,
    /// Video encoder configuration. `None` = audio-only preset.
    pub video: Option<VideoEncoderConfig>,
    /// Audio encoder configuration.
    pub audio: AudioEncoderConfig,
}

impl ExportPreset {
    // Predefined presets

    /// `YouTube` 1080p preset: H.264, CRF 18, 1920×1080, 30 fps, AAC 192 kbps.
    #[must_use]
    pub fn youtube_1080p() -> Self {
        presets::youtube_1080p()
    }

    /// `YouTube` 4K preset: H.265, CRF 20, 3840×2160, 30 fps, AAC 256 kbps.
    #[must_use]
    pub fn youtube_4k() -> Self {
        presets::youtube_4k()
    }

    /// Twitter/X preset: H.264, CRF 23, 1280×720, 30 fps, AAC 128 kbps.
    #[must_use]
    pub fn twitter() -> Self {
        presets::twitter()
    }

    /// Instagram Square preset: H.264, CRF 23, 1080×1080, 30 fps, AAC 128 kbps.
    #[must_use]
    pub fn instagram_square() -> Self {
        presets::instagram_square()
    }

    /// Instagram Reels preset: H.264, CRF 23, 1080×1920, 30 fps, AAC 128 kbps.
    #[must_use]
    pub fn instagram_reels() -> Self {
        presets::instagram_reels()
    }

    /// Blu-ray 1080p preset: H.264, CRF 18, 1920×1080, 24 fps, AC-3 384 kbps.
    #[must_use]
    pub fn bluray_1080p() -> Self {
        presets::bluray_1080p()
    }

    /// Podcast mono preset (audio-only): AAC 128 kbps, mono, 48 kHz.
    #[must_use]
    pub fn podcast_mono() -> Self {
        presets::podcast_mono()
    }

    /// Lossless archive preset: FFV1 video (source resolution), FLAC audio.
    #[must_use]
    pub fn lossless_rgb() -> Self {
        presets::lossless_rgb()
    }

    /// Web H.264 preset (VP9): VP9, CRF 33, 1280×720, 30 fps, Opus 128 kbps.
    #[must_use]
    pub fn web_h264() -> Self {
        presets::web_h264()
    }

    // Validation

    /// Validates this preset against platform-specific constraints.
    ///
    /// Call this before passing the preset to [`apply_video`](Self::apply_video)
    /// or [`apply_audio`](Self::apply_audio) to surface constraint violations
    /// before encoding begins.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError::PresetConstraintViolation`] when a platform rule
    /// is violated (e.g. fps > 60 on a `YouTube` preset, or wrong aspect ratio
    /// on an Instagram preset).
    pub fn validate(&self) -> Result<(), EncodeError> {
        validation::validate_preset(self)
    }

    // Builder helpers

    /// Applies the video configuration to a [`VideoEncoderBuilder`].
    ///
    /// Sets the codec and bitrate mode unconditionally. Resolution and frame
    /// rate are only applied when all three (`width`, `height`, `fps`) are
    /// `Some`. Pixel format and per-codec options are applied when present.
    ///
    /// Returns `builder` unchanged when [`video`](Self::video) is `None`
    /// (audio-only preset).
    #[must_use]
    pub fn apply_video(&self, builder: VideoEncoderBuilder) -> VideoEncoderBuilder {
        let Some(ref v) = self.video else {
            return builder;
        };
        let mut b = builder
            .video_codec(v.codec)
            .bitrate_mode(v.bitrate_mode.clone());
        if let (Some(w), Some(h), Some(fps)) = (v.width, v.height, v.fps) {
            b = b.video(w, h, fps);
        }
        if let Some(pf) = v.pixel_format {
            b = b.pixel_format(pf);
        }
        if let Some(opts) = v.codec_options.clone() {
            b = b.codec_options(opts);
        }
        b
    }

    /// Applies the audio configuration to a [`VideoEncoderBuilder`].
    ///
    /// Sets sample rate, channel count, codec, and bitrate.
    #[must_use]
    pub fn apply_audio(&self, builder: VideoEncoderBuilder) -> VideoEncoderBuilder {
        builder
            .audio(self.audio.sample_rate, self.audio.channels)
            .audio_codec(self.audio.codec)
            .audio_bitrate(self.audio.bitrate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // youtube_1080p

    #[test]
    fn youtube_1080p_preset_should_have_correct_codec() {
        let preset = ExportPreset::youtube_1080p();
        let video = preset
            .video
            .as_ref()
            .expect("youtube_1080p must have video");
        assert_eq!(video.codec, VideoCodec::H264);
    }

    #[test]
    fn youtube_1080p_preset_should_have_correct_resolution() {
        let preset = ExportPreset::youtube_1080p();
        let video = preset
            .video
            .as_ref()
            .expect("youtube_1080p must have video");
        assert_eq!(video.width, Some(1920));
        assert_eq!(video.height, Some(1080));
    }

    #[test]
    fn youtube_1080p_preset_audio_should_be_aac_192kbps() {
        let preset = ExportPreset::youtube_1080p();
        assert_eq!(preset.audio.codec, AudioCodec::Aac);
        assert_eq!(preset.audio.bitrate, 192_000);
    }

    // youtube_4k

    #[test]
    fn youtube_4k_preset_should_have_h265_codec() {
        let preset = ExportPreset::youtube_4k();
        let video = preset.video.as_ref().expect("youtube_4k must have video");
        assert_eq!(video.codec, VideoCodec::H265);
    }

    #[test]
    fn youtube_4k_preset_should_have_correct_resolution() {
        let preset = ExportPreset::youtube_4k();
        let video = preset.video.as_ref().expect("youtube_4k must have video");
        assert_eq!(video.width, Some(3840));
        assert_eq!(video.height, Some(2160));
    }

    #[test]
    fn youtube_4k_preset_audio_should_be_aac_256kbps() {
        let preset = ExportPreset::youtube_4k();
        assert_eq!(preset.audio.codec, AudioCodec::Aac);
        assert_eq!(preset.audio.bitrate, 256_000);
    }

    // lossless_rgb

    #[test]
    fn lossless_rgb_preset_should_have_ffv1_codec() {
        let preset = ExportPreset::lossless_rgb();
        let video = preset.video.as_ref().expect("lossless_rgb must have video");
        assert_eq!(video.codec, VideoCodec::Ffv1);
    }

    #[test]
    fn lossless_rgb_preset_should_preserve_source_resolution() {
        let preset = ExportPreset::lossless_rgb();
        let video = preset.video.as_ref().expect("lossless_rgb must have video");
        assert!(video.width.is_none(), "lossless_rgb should not fix width");
        assert!(video.height.is_none(), "lossless_rgb should not fix height");
        assert!(video.fps.is_none(), "lossless_rgb should not fix fps");
    }

    // podcast_mono

    #[test]
    fn podcast_mono_preset_should_have_no_video() {
        let preset = ExportPreset::podcast_mono();
        assert!(
            preset.video.is_none(),
            "podcast_mono must be audio-only (video=None)"
        );
    }

    #[test]
    fn podcast_mono_preset_should_have_mono_audio() {
        let preset = ExportPreset::podcast_mono();
        assert_eq!(preset.audio.channels, 1);
    }

    // web_h264

    #[test]
    fn web_h264_preset_should_use_vp9_codec() {
        let preset = ExportPreset::web_h264();
        let video = preset.video.as_ref().expect("web_h264 must have video");
        assert_eq!(video.codec, VideoCodec::Vp9);
    }

    // human-readable names

    #[test]
    fn all_presets_should_have_non_empty_names() {
        let presets = [
            ExportPreset::youtube_1080p(),
            ExportPreset::youtube_4k(),
            ExportPreset::twitter(),
            ExportPreset::instagram_square(),
            ExportPreset::instagram_reels(),
            ExportPreset::bluray_1080p(),
            ExportPreset::podcast_mono(),
            ExportPreset::lossless_rgb(),
            ExportPreset::web_h264(),
        ];
        for preset in &presets {
            assert!(!preset.name.is_empty(), "preset name must not be empty");
        }
    }

    // apply_video / apply_audio

    #[test]
    fn custom_preset_should_apply_codec_to_builder() {
        use crate::VideoEncoder;

        let preset = ExportPreset {
            name: "custom".to_string(),
            video: Some(VideoEncoderConfig {
                codec: VideoCodec::H264,
                width: Some(1280),
                height: Some(720),
                fps: Some(24.0),
                bitrate_mode: BitrateMode::Crf(23),
                pixel_format: None,
                codec_options: None,
            }),
            audio: AudioEncoderConfig {
                codec: AudioCodec::Aac,
                sample_rate: 44100,
                channels: 2,
                bitrate: 128_000,
            },
        };

        let builder = preset.apply_video(VideoEncoder::create("out.mp4"));
        assert_eq!(builder.video_codec, VideoCodec::H264);
        assert_eq!(builder.video_width, Some(1280));
        assert_eq!(builder.video_height, Some(720));
    }

    #[test]
    fn preset_with_no_resolution_should_not_override_builder_resolution() {
        use crate::VideoEncoder;

        let preset = ExportPreset {
            name: "no-res".to_string(),
            video: Some(VideoEncoderConfig {
                codec: VideoCodec::Ffv1,
                width: None,
                height: None,
                fps: None,
                bitrate_mode: BitrateMode::Crf(0),
                pixel_format: None,
                codec_options: None,
            }),
            audio: AudioEncoderConfig {
                codec: AudioCodec::Flac,
                sample_rate: 48000,
                channels: 2,
                bitrate: 0,
            },
        };

        // The builder starts with no video dimensions set.
        let builder = preset.apply_video(VideoEncoder::create("out.mkv"));
        assert!(
            builder.video_width.is_none(),
            "apply_video should not set width when VideoEncoderConfig.width is None"
        );
        assert!(
            builder.video_height.is_none(),
            "apply_video should not set height when VideoEncoderConfig.height is None"
        );
    }

    #[test]
    fn audio_only_preset_apply_video_should_return_builder_unchanged() {
        use crate::VideoEncoder;

        let preset = ExportPreset::podcast_mono();
        let builder_before = VideoEncoder::create("out.m4a");
        let builder_after = preset.apply_video(VideoEncoder::create("out.m4a"));
        // Both start from the same state; codec should still be default.
        assert_eq!(
            builder_before.video_codec, builder_after.video_codec,
            "apply_video on audio-only preset must leave builder unchanged"
        );
    }

    #[test]
    fn apply_audio_should_set_sample_rate_and_bitrate() {
        use crate::VideoEncoder;

        let preset = ExportPreset::youtube_1080p();
        let builder = preset.apply_audio(VideoEncoder::create("out.mp4"));
        assert_eq!(builder.audio_sample_rate, Some(48000));
        assert_eq!(builder.audio_bitrate, Some(192_000));
    }

    // validation

    #[test]
    fn youtube_1080p_preset_should_pass_validation() {
        assert!(ExportPreset::youtube_1080p().validate().is_ok());
    }

    #[test]
    fn youtube_4k_preset_should_pass_validation() {
        assert!(ExportPreset::youtube_4k().validate().is_ok());
    }

    #[test]
    fn instagram_square_preset_should_pass_validation() {
        assert!(ExportPreset::instagram_square().validate().is_ok());
    }

    #[test]
    fn instagram_reels_preset_should_pass_validation() {
        assert!(ExportPreset::instagram_reels().validate().is_ok());
    }
}
