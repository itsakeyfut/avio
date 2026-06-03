//! # ff-format
//!
//! Common types for video/audio processing - the Rust way.
//!
//! This crate provides shared type definitions used across the ff-* crate family.
//! It completely hides `FFmpeg` internals and provides Rust-idiomatic type safety.
//!
//! ## Module Structure
//!
//! - `pixel` - Pixel format definitions ([`PixelFormat`])
//! - `sample` - Audio sample format definitions ([`SampleFormat`])
//! - `time` - Time primitives ([`Timestamp`], [`Rational`])
//! - `frame` - Frame types ([`VideoFrame`], [`AudioFrame`])
//! - `stream` - Stream info ([`VideoStreamInfo`], [`AudioStreamInfo`])
//! - `container` - Container info ([`ContainerInfo`])
//! - `media` - Media container info ([`MediaInfo`])
//! - `color` - Color space definitions ([`ColorSpace`], [`ColorRange`], [`ColorPrimaries`])
//! - `hdr` - HDR metadata types ([`Hdr10Metadata`], [`MasteringDisplay`])
//! - `network` - Network configuration ([`NetworkOptions`])
//! - `codec` - Codec definitions ([`VideoCodec`], [`AudioCodec`])
//! - `channel` - Channel layout definitions ([`ChannelLayout`])
//! - `chapter` - Chapter information ([`ChapterInfo`])
//! - `error` - Error types ([`FormatError`])
//!
//! ## Usage
//!
//! ```
//! use ff_format::prelude::*;
//!
//! // Access pixel formats
//! let format = PixelFormat::Yuv420p;
//! assert!(format.is_planar());
//!
//! // Access sample formats
//! let audio = SampleFormat::F32;
//! assert!(audio.is_float());
//! assert_eq!(audio.bytes_per_sample(), 4);
//!
//! // Work with timestamps
//! let time_base = Rational::new(1, 90000);
//! let ts = Timestamp::new(90000, time_base);
//! assert!((ts.as_secs_f64() - 1.0).abs() < 0.001);
//!
//! // Access color and codec types
//! use ff_format::color::ColorSpace;
//! use ff_format::codec::VideoCodec;
//! let space = ColorSpace::Bt709;
//! let codec = VideoCodec::H264;
//! ```

#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]

// Module declarations
pub mod channel;
pub mod chapter;
pub mod codec;
pub mod color;
pub mod container;
pub mod error;
pub mod frame;
pub mod hdr;
pub mod media;
pub mod network;
pub mod pixel;
pub mod sample;
pub mod stream;
pub mod subtitle;
pub mod time;

pub use channel::ChannelLayout;
pub use chapter::{ChapterInfo, ChapterInfoBuilder};
pub use codec::{AudioCodec, SubtitleCodec, VideoCodec};
pub use color::{AlphaMode, ColorPrimaries, ColorRange, ColorSpace, ColorTransfer};
pub use container::{ContainerInfo, ContainerInfoBuilder};
pub use error::{FormatError, FrameError, SubtitleError};
pub use ff_common::PooledBuffer;
pub use frame::{AudioFrame, VideoFrame};
pub use hdr::{Hdr10Metadata, MasteringDisplay};
pub use media::{MediaInfo, MediaInfoBuilder};
pub use network::NetworkOptions;
pub use pixel::PixelFormat;
pub use sample::SampleFormat;
pub use stream::{
    AudioStreamInfo, AudioStreamInfoBuilder, SubtitleStreamInfo, SubtitleStreamInfoBuilder,
    VideoStreamInfo, VideoStreamInfoBuilder,
};
pub use time::{Rational, Timestamp};

/// Prelude module for convenient imports.
///
/// This module re-exports all commonly used types for easy access:
///
/// ```ignore
/// use ff_format::prelude::*;
/// ```
pub mod prelude {
    pub use crate::{
        AudioCodec, AudioFrame, AudioStreamInfo, ChannelLayout, ChapterInfo, ColorPrimaries,
        ColorRange, ColorSpace, FormatError, FrameError, MediaInfo, NetworkOptions, PixelFormat,
        PooledBuffer, Rational, SampleFormat, Timestamp, VideoCodec, VideoFrame, VideoStreamInfo,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prelude_exports() {
        // Verify prelude exports all expected types
        let _pixel: PixelFormat = PixelFormat::default();
        let _sample: SampleFormat = SampleFormat::default();
        let _rational: Rational = Rational::default();
        let _timestamp: Timestamp = Timestamp::default();
        let _video_frame: VideoFrame = VideoFrame::default();
        let _audio_frame: AudioFrame = AudioFrame::default();

        // New types
        let _color_space: ColorSpace = ColorSpace::default();
        let _color_range: ColorRange = ColorRange::default();
        let _color_primaries: ColorPrimaries = ColorPrimaries::default();
        let _video_codec: VideoCodec = VideoCodec::default();
        let _audio_codec: AudioCodec = AudioCodec::default();
        let _channel_layout: ChannelLayout = ChannelLayout::default();
        let _video_stream: VideoStreamInfo = VideoStreamInfo::default();
        let _audio_stream: AudioStreamInfo = AudioStreamInfo::default();
        let _media_info: MediaInfo = MediaInfo::default();
        let _network_opts: NetworkOptions = NetworkOptions::default();
    }

    #[test]
    fn test_stream_info_builder() {
        // Test VideoStreamInfo builder
        let video = VideoStreamInfo::builder()
            .index(0)
            .codec(VideoCodec::H264)
            .width(1920)
            .height(1080)
            .frame_rate(Rational::new(30, 1))
            .pixel_format(PixelFormat::Yuv420p)
            .color_space(ColorSpace::Bt709)
            .build();

        assert_eq!(video.width(), 1920);
        assert_eq!(video.height(), 1080);
        assert_eq!(video.codec(), VideoCodec::H264);
        assert_eq!(video.color_space(), ColorSpace::Bt709);

        // Test AudioStreamInfo builder
        let audio = AudioStreamInfo::builder()
            .index(1)
            .codec(AudioCodec::Aac)
            .sample_rate(48000)
            .channels(2)
            .sample_format(SampleFormat::F32)
            .build();

        assert_eq!(audio.sample_rate(), 48000);
        assert_eq!(audio.channels(), 2);
        assert_eq!(audio.codec(), AudioCodec::Aac);
        assert_eq!(audio.channel_layout(), ChannelLayout::Stereo);
    }

    #[test]
    fn test_media_info_builder() {
        use std::time::Duration;

        // Create streams
        let video = VideoStreamInfo::builder()
            .index(0)
            .codec(VideoCodec::H264)
            .width(1920)
            .height(1080)
            .frame_rate(Rational::new(30, 1))
            .build();

        let audio = AudioStreamInfo::builder()
            .index(1)
            .codec(AudioCodec::Aac)
            .sample_rate(48000)
            .channels(2)
            .build();

        // Create media info
        let media = MediaInfo::builder()
            .path("/path/to/video.mp4")
            .format("mp4")
            .format_long_name("QuickTime / MOV")
            .duration(Duration::from_secs(120))
            .file_size(100_000_000)
            .bitrate(8_000_000)
            .video_stream(video)
            .audio_stream(audio)
            .metadata("title", "Test Video")
            .build();

        assert!(media.has_video());
        assert!(media.has_audio());
        assert_eq!(media.resolution(), Some((1920, 1080)));
        assert!((media.frame_rate().unwrap() - 30.0).abs() < 0.001);
        assert_eq!(media.sample_rate(), Some(48000));
        assert_eq!(media.channels(), Some(2));
        assert_eq!(media.format(), "mp4");
        assert_eq!(media.format_long_name(), Some("QuickTime / MOV"));
        assert_eq!(media.metadata_value("title"), Some("Test Video"));
    }
}
