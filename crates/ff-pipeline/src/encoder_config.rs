//! Codec and quality configuration for the pipeline output.

use ff_encode::BitrateMode;
use ff_filter::HwAccel;
use ff_format::{AudioCodec, VideoCodec};

/// Codec and quality configuration for the pipeline output.
///
/// Passed to [`PipelineBuilder::output`](crate::PipelineBuilder::output) alongside the output path.
///
/// Construct via [`EncoderConfig::builder`].
#[non_exhaustive]
pub struct EncoderConfig {
    /// Video codec to use for the output stream.
    pub video_codec: VideoCodec,

    /// Audio codec to use for the output stream.
    pub audio_codec: AudioCodec,

    /// Bitrate control mode (CBR, VBR, or CRF).
    pub bitrate_mode: BitrateMode,

    /// Output resolution as `(width, height)` in pixels.
    ///
    /// Resolution precedence in [`Pipeline::run`](crate::Pipeline::run):
    /// 1. This field when `Some` — explicit value always wins.
    /// 2. The output dimensions of a `scale` filter, inferred automatically.
    /// 3. The source video's native resolution.
    ///
    /// When a `scale` filter is used via [`PipelineBuilder::filter`](crate::PipelineBuilder::filter) you
    /// typically do **not** need to set this field; the pipeline infers the
    /// encoder dimensions from the filter. Set it explicitly only to override
    /// the filter's output size or to resize without a filter.
    pub resolution: Option<(u32, u32)>,

    /// Output frame rate in frames per second.
    ///
    /// `None` preserves the source frame rate.
    pub framerate: Option<f64>,

    /// Hardware acceleration device to use during encoding.
    ///
    /// `None` uses software (CPU) encoding.
    pub hardware: Option<HwAccel>,

    /// Accept an encoder from a different codec family when `video_codec` has no
    /// encoder available.
    ///
    /// Defaults to `false`, which makes an encode that cannot honour
    /// `video_codec` fail rather than quietly produce another codec.
    pub allow_codec_substitution: bool,
}

impl EncoderConfig {
    /// Returns an [`EncoderConfigBuilder`] with sensible defaults:
    /// H.264 video, AAC audio, CRF 23, no resolution/framerate override, software encoding.
    #[must_use]
    pub fn builder() -> EncoderConfigBuilder {
        EncoderConfigBuilder::new()
    }
}

/// Consuming builder for [`EncoderConfig`].
///
/// Obtain via [`EncoderConfig::builder`].
pub struct EncoderConfigBuilder {
    video_codec: VideoCodec,
    audio_codec: AudioCodec,
    bitrate_mode: BitrateMode,
    resolution: Option<(u32, u32)>,
    framerate: Option<f64>,
    hardware: Option<HwAccel>,
    allow_codec_substitution: bool,
}

impl EncoderConfigBuilder {
    fn new() -> Self {
        Self {
            video_codec: VideoCodec::H264,
            audio_codec: AudioCodec::Aac,
            bitrate_mode: BitrateMode::Crf(23),
            resolution: None,
            framerate: None,
            hardware: None,
            allow_codec_substitution: false,
        }
    }

    /// Sets the video codec.
    #[must_use]
    pub fn video_codec(mut self, codec: VideoCodec) -> Self {
        self.video_codec = codec;
        self
    }

    /// Sets the audio codec.
    #[must_use]
    pub fn audio_codec(mut self, codec: AudioCodec) -> Self {
        self.audio_codec = codec;
        self
    }

    /// Sets the bitrate control mode.
    #[must_use]
    pub fn bitrate_mode(mut self, mode: BitrateMode) -> Self {
        self.bitrate_mode = mode;
        self
    }

    /// Convenience: sets `BitrateMode::Crf(crf)`.
    #[must_use]
    pub fn crf(mut self, crf: u32) -> Self {
        self.bitrate_mode = BitrateMode::Crf(crf);
        self
    }

    /// Sets the output resolution in pixels.
    #[must_use]
    pub fn resolution(mut self, width: u32, height: u32) -> Self {
        self.resolution = Some((width, height));
        self
    }

    /// Sets the output frame rate in frames per second.
    #[must_use]
    pub fn framerate(mut self, fps: f64) -> Self {
        self.framerate = Some(fps);
        self
    }

    /// Sets the hardware acceleration backend.
    #[must_use]
    pub fn hardware(mut self, hw: HwAccel) -> Self {
        self.hardware = Some(hw);
        self
    }

    /// Accepts an encoder from a different codec family when the requested video
    /// codec has no encoder available.
    ///
    /// Without this, an encode that cannot honour the requested codec fails
    /// instead of silently writing a different one.
    #[must_use]
    pub fn allow_codec_substitution(mut self) -> Self {
        self.allow_codec_substitution = true;
        self
    }

    /// Builds the [`EncoderConfig`]. Never fails; returns the config directly.
    #[must_use]
    pub fn build(self) -> EncoderConfig {
        EncoderConfig {
            video_codec: self.video_codec,
            audio_codec: self.audio_codec,
            bitrate_mode: self.bitrate_mode,
            resolution: self.resolution,
            framerate: self.framerate,
            hardware: self.hardware,
            allow_codec_substitution: self.allow_codec_substitution,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_should_use_h264_aac_crf23_as_defaults() {
        let config = EncoderConfig::builder().build();
        assert!(matches!(config.video_codec, VideoCodec::H264));
        assert!(matches!(config.audio_codec, AudioCodec::Aac));
        assert!(matches!(config.bitrate_mode, BitrateMode::Crf(23)));
        assert!(config.resolution.is_none());
        assert!(config.framerate.is_none());
        assert!(config.hardware.is_none());
    }

    #[test]
    fn builder_should_store_all_fields() {
        let config = EncoderConfig::builder()
            .video_codec(VideoCodec::H265)
            .audio_codec(AudioCodec::Opus)
            .bitrate_mode(BitrateMode::Cbr(4_000_000))
            .resolution(1280, 720)
            .framerate(30.0)
            .build();

        assert!(matches!(config.video_codec, VideoCodec::H265));
        assert!(matches!(config.audio_codec, AudioCodec::Opus));
        assert!(matches!(config.bitrate_mode, BitrateMode::Cbr(4_000_000)));
        assert_eq!(config.resolution, Some((1280, 720)));
        assert_eq!(config.framerate, Some(30.0));
    }

    #[test]
    fn crf_convenience_should_set_crf_mode() {
        let config = EncoderConfig::builder().crf(28).build();
        assert!(matches!(config.bitrate_mode, BitrateMode::Crf(28)));
    }
}
