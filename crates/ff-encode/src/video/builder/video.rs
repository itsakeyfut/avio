//! Video stream settings for [`VideoEncoderBuilder`].

use super::VideoEncoderBuilder;
use crate::{BitrateMode, HardwareEncoder, Preset, VideoCodec};

impl VideoEncoderBuilder {
    /// Configure video stream settings.
    #[must_use]
    pub fn video(mut self, width: u32, height: u32, fps: f64) -> Self {
        self.video_width = Some(width);
        self.video_height = Some(height);
        self.video_fps = Some(fps);
        self
    }

    /// Set video codec.
    #[must_use]
    pub fn video_codec(mut self, codec: VideoCodec) -> Self {
        self.video_codec = codec;
        self.video_codec_explicit = true;
        self
    }

    /// Set the bitrate control mode for video encoding.
    #[must_use]
    pub fn bitrate_mode(mut self, mode: BitrateMode) -> Self {
        self.video_bitrate_mode = Some(mode);
        self
    }

    /// Set encoding preset (speed vs quality tradeoff).
    #[must_use]
    pub fn preset(mut self, preset: Preset) -> Self {
        self.preset = preset;
        self
    }

    /// Set hardware encoder.
    #[must_use]
    pub fn hardware_encoder(mut self, hw: HardwareEncoder) -> Self {
        self.hardware_encoder = hw;
        self
    }

    /// Accept an encoder from a different codec family when the requested codec
    /// has no encoder available.
    ///
    /// By default, asking for H.264 and getting anything that is not H.264 is an
    /// error. Some codecs have an LGPL-compatible stand-in (VP9 for H.264, AV1
    /// for H.265) that can encode the frames, but the output is that other
    /// codec, so it is only used when this is set. When it is used, a warning
    /// names the encoder that was chosen, and
    /// [`actual_video_codec`](super::VideoEncoder::actual_video_codec) reports
    /// it.
    #[must_use]
    pub fn allow_codec_substitution(mut self) -> Self {
        self.allow_codec_substitution = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn builder_video_settings_should_be_stored() {
        let builder = VideoEncoderBuilder::new(PathBuf::from("output.mp4"))
            .video(1920, 1080, 30.0)
            .video_codec(VideoCodec::H264)
            .bitrate_mode(BitrateMode::Cbr(8_000_000));
        assert_eq!(builder.video_width, Some(1920));
        assert_eq!(builder.video_height, Some(1080));
        assert_eq!(builder.video_fps, Some(30.0));
        assert_eq!(builder.video_codec, VideoCodec::H264);
        assert_eq!(
            builder.video_bitrate_mode,
            Some(BitrateMode::Cbr(8_000_000))
        );
    }

    #[test]
    fn builder_preset_should_be_stored() {
        let builder = VideoEncoderBuilder::new(PathBuf::from("output.mp4"))
            .video(1920, 1080, 30.0)
            .preset(Preset::Fast);
        assert_eq!(builder.preset, Preset::Fast);
    }

    #[test]
    fn builder_hardware_encoder_should_be_stored() {
        let builder = VideoEncoderBuilder::new(PathBuf::from("output.mp4"))
            .video(1920, 1080, 30.0)
            .hardware_encoder(HardwareEncoder::Nvenc);
        assert_eq!(builder.hardware_encoder, HardwareEncoder::Nvenc);
    }
}
