//! # ff-probe
//!
//! Media file metadata extraction - the Rust way.
//!
//! This crate provides functionality for extracting metadata from media files,
//! including video streams, audio streams, and container information. It serves
//! as the Rust equivalent of ffprobe with a clean, idiomatic API.
//!
//! ## Module Structure
//!
//! - `error` - Error types ([`ProbeError`])
//! - `probe` - Media info extraction ([`open`])
//!
//! ## Quick Start
//!
//! ```no_run
//! use ff_probe::open;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let info = open("video.mp4")?;
//!
//!     println!("Format: {}", info.format());
//!     println!("Duration: {:?}", info.duration());
//!
//!     // Check for video stream
//!     if let Some(video) = info.primary_video() {
//!         println!("Video: {}x{} @ {:.2} fps",
//!             video.width(),
//!             video.height(),
//!             video.fps()
//!         );
//!     }
//!
//!     // Check for audio stream
//!     if let Some(audio) = info.primary_audio() {
//!         println!("Audio: {} Hz, {} channels",
//!             audio.sample_rate(),
//!             audio.channels()
//!         );
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Extracting Detailed Information
//!
//! ```no_run
//! use ff_probe::{open, ColorSpace, ColorPrimaries};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let info = open("hdr_video.mp4")?;
//!
//!     // Enumerate all video streams
//!     for (i, stream) in info.video_streams().iter().enumerate() {
//!         println!("Video stream {}: {} {}x{}",
//!             i, stream.codec_name(), stream.width(), stream.height());
//!         println!("  Color space: {:?}", stream.color_space());
//!         println!("  Color range: {:?}", stream.color_range());
//!
//!         // Check for HDR content
//!         if stream.color_primaries() == ColorPrimaries::Bt2020 {
//!             println!("  HDR content detected!");
//!         }
//!
//!         if let Some(bitrate) = stream.bitrate() {
//!             println!("  Bitrate: {} kbps", bitrate / 1000);
//!         }
//!     }
//!
//!     // Enumerate all audio streams
//!     for (i, stream) in info.audio_streams().iter().enumerate() {
//!         println!("Audio stream {}: {} {} Hz, {} ch",
//!             i, stream.codec_name(), stream.sample_rate(), stream.channels());
//!         if let Some(lang) = stream.language() {
//!             println!("  Language: {}", lang);
//!         }
//!     }
//!
//!     // Access container metadata
//!     if let Some(title) = info.title() {
//!         println!("Title: {}", title);
//!     }
//!     if let Some(artist) = info.artist() {
//!         println!("Artist: {}", artist);
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Error Handling
//!
//! The crate provides detailed error types through [`ProbeError`]:
//!
//! ```
//! use ff_probe::{open, ProbeError};
//!
//! let result = open("/nonexistent/path.mp4");
//!
//! match result {
//!     Err(ProbeError::FileNotFound { path }) => {
//!         println!("File not found: {}", path.display());
//!     }
//!     Err(ProbeError::CannotOpen { path, reason }) => {
//!         println!("Cannot open {}: {}", path.display(), reason);
//!     }
//!     Err(ProbeError::InvalidMedia { path, reason }) => {
//!         println!("Invalid media {}: {}", path.display(), reason);
//!     }
//!     Err(e) => println!("Other error: {}", e),
//!     Ok(info) => println!("Opened: {}", info.format()),
//! }
//! ```
//!
//! ## Features
//!
//! - Extract container format information (MP4, MKV, AVI, etc.)
//! - List all video and audio streams with detailed properties
//! - Get codec parameters (codec type, pixel format, sample format)
//! - Read container and stream metadata (title, artist, etc.)
//! - Color space and HDR information (BT.709, BT.2020, etc.)
//! - Bitrate extraction and calculation
//! - Duration and frame count information

#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]

mod error;
mod probe;

// Re-export types from ff-format for convenience
pub use ff_format::{
    AudioCodec, AudioStreamInfo, ChannelLayout, ChapterInfo, ChapterInfoBuilder, ColorPrimaries,
    ColorRange, ColorSpace, MediaInfo, PixelFormat, Rational, SampleFormat, SubtitleCodec,
    SubtitleStreamInfo, SubtitleStreamInfoBuilder, Timestamp, VideoCodec, VideoStreamInfo,
};

pub use error::ProbeError;
pub use ff_format::{ErrorSeverity, MediaError};

// Re-export the open function at the crate level
pub use probe::open;

/// Prelude module for convenient imports.
///
/// This module re-exports all commonly used types for easy access:
///
/// ```no_run
/// use ff_probe::prelude::*;
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let info = open("video.mp4")?;
///
///     // Access stream information
///     if let Some(video) = info.primary_video() {
///         let _codec: VideoCodec = video.codec();
///         let _color: ColorSpace = video.color_space();
///     }
///
///     Ok(())
/// }
/// ```
pub mod prelude {
    pub use crate::{
        AudioCodec, AudioStreamInfo, ChannelLayout, ChapterInfo, ChapterInfoBuilder,
        ColorPrimaries, ColorRange, ColorSpace, MediaInfo, PixelFormat, ProbeError, Rational,
        SampleFormat, SubtitleCodec, SubtitleStreamInfo, SubtitleStreamInfoBuilder, Timestamp,
        VideoCodec, VideoStreamInfo, open,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prelude_exports() {
        // Verify prelude exports all expected types
        let _rational: Rational = Rational::default();
        let _timestamp: Timestamp = Timestamp::default();
        let _color_space: ColorSpace = ColorSpace::default();
        let _color_range: ColorRange = ColorRange::default();
        let _color_primaries: ColorPrimaries = ColorPrimaries::default();
        let _video_codec: VideoCodec = VideoCodec::default();
        let _audio_codec: AudioCodec = AudioCodec::default();
        let _channel_layout: ChannelLayout = ChannelLayout::default();
        let _video_stream: VideoStreamInfo = VideoStreamInfo::default();
        let _audio_stream: AudioStreamInfo = AudioStreamInfo::default();
        let _media_info: MediaInfo = MediaInfo::default();
        let _pixel_format: PixelFormat = PixelFormat::default();
        let _sample_format: SampleFormat = SampleFormat::default();

        // Verify open function is accessible (implicitly via the imports)
        // The function signature uses impl AsRef<Path>, not &str
        let _: Result<MediaInfo, ProbeError> = Err(ProbeError::FileNotFound {
            path: std::path::PathBuf::new(),
        });
        // Can't easily verify function pointer due to impl trait, but open is re-exported
    }

    #[test]
    fn test_probe_error_display() {
        use std::path::PathBuf;

        let err = ProbeError::FileNotFound {
            path: PathBuf::from("test.mp4"),
        };
        assert!(err.to_string().contains("test.mp4"));
    }

    #[test]
    fn test_open_nonexistent_file() {
        let result = open("/nonexistent/path/to/video.mp4");
        assert!(matches!(result, Err(ProbeError::FileNotFound { .. })));
    }
}
