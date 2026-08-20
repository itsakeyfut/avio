//! Predefined export presets.

use super::{AudioEncoderConfig, ExportPreset, VideoEncoderConfig};
use crate::{AudioCodec, BitrateMode, VideoCodec};

/// `YouTube` 1080p preset: H.264 CRF 18, 1920×1080, 30 fps, AAC 192 kbps.
pub(super) fn youtube_1080p() -> ExportPreset {
    ExportPreset {
        name: "youtube_1080p".to_string(),
        video: Some(VideoEncoderConfig {
            codec: VideoCodec::H264,
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            bitrate_mode: BitrateMode::Crf(18),
            pixel_format: None,
            codec_options: None,
        }),
        audio: AudioEncoderConfig {
            codec: AudioCodec::Aac,
            sample_rate: 48000,
            channels: 2,
            bitrate: 192_000,
        },
    }
}

/// `YouTube` 4K preset: H.265 CRF 20, 3840×2160, 30 fps, AAC 256 kbps.
pub(super) fn youtube_4k() -> ExportPreset {
    ExportPreset {
        name: "youtube_4k".to_string(),
        video: Some(VideoEncoderConfig {
            codec: VideoCodec::H265,
            width: Some(3840),
            height: Some(2160),
            fps: Some(30.0),
            bitrate_mode: BitrateMode::Crf(20),
            pixel_format: None,
            codec_options: None,
        }),
        audio: AudioEncoderConfig {
            codec: AudioCodec::Aac,
            sample_rate: 48000,
            channels: 2,
            bitrate: 256_000,
        },
    }
}

/// Twitter/X preset: H.264 CRF 23, 1280×720, 30 fps, AAC 128 kbps.
pub(super) fn twitter() -> ExportPreset {
    ExportPreset {
        name: "twitter".to_string(),
        video: Some(VideoEncoderConfig {
            codec: VideoCodec::H264,
            width: Some(1280),
            height: Some(720),
            fps: Some(30.0),
            bitrate_mode: BitrateMode::Crf(23),
            pixel_format: None,
            codec_options: None,
        }),
        audio: AudioEncoderConfig {
            codec: AudioCodec::Aac,
            sample_rate: 48000,
            channels: 2,
            bitrate: 128_000,
        },
    }
}

/// Instagram Square preset: H.264 CRF 23, 1080×1080, 30 fps, AAC 128 kbps.
pub(super) fn instagram_square() -> ExportPreset {
    ExportPreset {
        name: "instagram_square".to_string(),
        video: Some(VideoEncoderConfig {
            codec: VideoCodec::H264,
            width: Some(1080),
            height: Some(1080),
            fps: Some(30.0),
            bitrate_mode: BitrateMode::Crf(23),
            pixel_format: None,
            codec_options: None,
        }),
        audio: AudioEncoderConfig {
            codec: AudioCodec::Aac,
            sample_rate: 48000,
            channels: 2,
            bitrate: 128_000,
        },
    }
}

/// Instagram Reels preset: H.264 CRF 23, 1080×1920 (9:16), 30 fps, AAC 128 kbps.
pub(super) fn instagram_reels() -> ExportPreset {
    ExportPreset {
        name: "instagram_reels".to_string(),
        video: Some(VideoEncoderConfig {
            codec: VideoCodec::H264,
            width: Some(1080),
            height: Some(1920),
            fps: Some(30.0),
            bitrate_mode: BitrateMode::Crf(23),
            pixel_format: None,
            codec_options: None,
        }),
        audio: AudioEncoderConfig {
            codec: AudioCodec::Aac,
            sample_rate: 48000,
            channels: 2,
            bitrate: 128_000,
        },
    }
}

/// Blu-ray 1080p preset: H.264 CRF 18, 1920×1080, 24 fps, AC-3 384 kbps.
pub(super) fn bluray_1080p() -> ExportPreset {
    ExportPreset {
        name: "bluray_1080p".to_string(),
        video: Some(VideoEncoderConfig {
            codec: VideoCodec::H264,
            width: Some(1920),
            height: Some(1080),
            fps: Some(24.0),
            bitrate_mode: BitrateMode::Crf(18),
            pixel_format: None,
            codec_options: None,
        }),
        audio: AudioEncoderConfig {
            codec: AudioCodec::Ac3,
            sample_rate: 48000,
            channels: 2,
            bitrate: 384_000,
        },
    }
}

/// Podcast mono preset (audio-only): AAC 128 kbps, mono, 48 kHz.
pub(super) fn podcast_mono() -> ExportPreset {
    ExportPreset {
        name: "podcast_mono".to_string(),
        video: None,
        audio: AudioEncoderConfig {
            codec: AudioCodec::Aac,
            sample_rate: 48000,
            channels: 1,
            bitrate: 128_000,
        },
    }
}

/// Lossless archive preset: FFV1 video (source dimensions), FLAC audio.
pub(super) fn lossless_rgb() -> ExportPreset {
    ExportPreset {
        name: "lossless_rgb".to_string(),
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
    }
}

/// Web H.264 preset (VP9): VP9 CRF 33, 1280×720, 30 fps, Opus 128 kbps.
pub(super) fn web_h264() -> ExportPreset {
    ExportPreset {
        name: "web_h264".to_string(),
        video: Some(VideoEncoderConfig {
            codec: VideoCodec::Vp9,
            width: Some(1280),
            height: Some(720),
            fps: Some(30.0),
            bitrate_mode: BitrateMode::Crf(33),
            pixel_format: None,
            codec_options: None,
        }),
        audio: AudioEncoderConfig {
            codec: AudioCodec::Opus,
            sample_rate: 48000,
            channels: 2,
            bitrate: 128_000,
        },
    }
}
