//! # ff-decode
//!
//! Video and audio decoding - the Rust way.
//!
//! This crate provides frame-by-frame video/audio decoding, efficient seeking,
//! and thumbnail generation. It completely hides `FFmpeg` internals and provides
//! a safe, ergonomic Rust API.
//!
//! ## Features
//!
//! - **Video Decoding**: Frame-by-frame decoding with Iterator pattern
//! - **Audio Decoding**: Sample-level audio extraction
//! - **Seeking**: Fast keyframe and exact seeking without file re-open
//! - **Thumbnails**: Efficient thumbnail generation for timelines
//! - **Hardware Acceleration**: Optional NVDEC, QSV, AMF, `VideoToolbox`, VAAPI support
//! - **Frame Pooling**: Memory reuse for reduced allocation overhead
//!
//! ## Usage
//!
//! ### Video Decoding
//!
//! ```ignore
//! use ff_decode::{VideoDecoder, SeekMode};
//! use ff_format::PixelFormat;
//! use std::time::Duration;
//!
//! // Open a video file and create decoder
//! let mut decoder = VideoDecoder::open("video.mp4")?
//!     .output_format(PixelFormat::Rgba)
//!     .build()?;
//!
//! // Get basic info
//! println!("Duration: {:?}", decoder.duration());
//! println!("Resolution: {}x{}", decoder.width(), decoder.height());
//!
//! // Decode frames sequentially
//! for result in &mut decoder {
//!     let frame = result?;
//!     println!("Frame at {:?}", frame.timestamp().as_duration());
//! }
//!
//! // Seek to specific position
//! decoder.seek(Duration::from_secs(30), SeekMode::Keyframe)?;
//! ```
//!
//! ### Audio Decoding
//!
//! ```ignore
//! use ff_decode::AudioDecoder;
//! use ff_format::SampleFormat;
//!
//! let mut decoder = AudioDecoder::open("audio.mp3")?
//!     .output_format(SampleFormat::F32)
//!     .output_sample_rate(48000)
//!     .build()?;
//!
//! // Decode all audio samples
//! for result in &mut decoder {
//!     let frame = result?;
//!     println!("Audio frame with {} samples", frame.samples());
//! }
//! ```
//!
//! ### Hardware Acceleration (Video)
//!
//! ```ignore
//! use ff_decode::{VideoDecoder, HardwareAccel};
//!
//! let decoder = VideoDecoder::open("video.mp4")?
//!     .hardware_accel(HardwareAccel::Auto)  // Auto-detect GPU
//!     .build()?;
//! ```
//!
//! ### Frame Pooling (Video)
//!
//! ```ignore
//! use ff_decode::{VideoDecoder, FramePool};
//! use std::sync::Arc;
//!
//! // Use a frame pool for memory reuse
//! let pool: Arc<dyn FramePool> = create_frame_pool(32);
//! let decoder = VideoDecoder::open("video.mp4")?
//!     .frame_pool(pool)
//!     .build()?;
//! ```
//!
//! ## Module Structure
//!
//! - [`audio`] - Audio decoder for extracting audio frames
//! - [`video`] - Video decoder for extracting video frames
//! - [`error`] - Error types for decoding operations
//! - Frame pool types (`FramePool`, `PooledBuffer`, `VecPool`) are provided by `ff-common`
//!
//! ## Re-exports
//!
//! This crate re-exports commonly used types from `ff-format` for convenience.

#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]

// Module declarations
#[cfg(feature = "tokio")]
pub(crate) mod async_decoder;
pub mod audio;
pub mod error;
pub mod extract;
pub mod image;
mod shared;
pub mod video;

// Preserve crate::network path used throughout decoder_inner modules.
pub(crate) use shared::network;

// Re-exports for convenience
pub use audio::{AudioDecoder, AudioDecoderBuilder};
pub use error::DecodeError;
pub use extract::{FrameExtractor, ThumbnailSelector};
pub use ff_common::{FramePool, PooledBuffer};
pub use ff_format::ContainerInfo;
pub use ff_format::{ErrorSeverity, MediaError};
pub use image::{ImageDecoder, ImageDecoderBuilder};
pub use shared::{HardwareAccel, SeekMode};
pub use video::{VideoDecoder, VideoDecoderBuilder};

#[cfg(feature = "tokio")]
pub use audio::{AsyncAudioDecoder, AsyncAudioDecoderBuilder};
#[cfg(feature = "tokio")]
pub use image::AsyncImageDecoder;
#[cfg(feature = "tokio")]
pub use video::{AsyncVideoDecoder, AsyncVideoDecoderBuilder};

/// Prelude module for convenient imports.
///
/// This module re-exports all commonly used types:
///
/// ```ignore
/// use ff_decode::prelude::*;
/// ```
pub mod prelude {
    #[cfg(feature = "tokio")]
    pub use crate::{AsyncAudioDecoder, AsyncImageDecoder, AsyncVideoDecoder};
    pub use crate::{
        AudioDecoder, AudioDecoderBuilder, DecodeError, FramePool, HardwareAccel, ImageDecoder,
        ImageDecoderBuilder, PooledBuffer, SeekMode, VideoDecoder, VideoDecoderBuilder,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_error_display() {
        use std::path::PathBuf;

        let error = DecodeError::FileNotFound {
            path: PathBuf::from("/path/to/video.mp4"),
        };
        assert!(error.to_string().contains("File not found"));

        let error = DecodeError::NoVideoStream {
            path: PathBuf::from("/path/to/audio.mp3"),
        };
        assert!(error.to_string().contains("No video stream"));

        let error = DecodeError::UnsupportedCodec {
            codec: "unknown_codec".to_string(),
        };
        assert!(error.to_string().contains("Codec not supported"));
    }

    #[test]
    fn test_prelude_imports() {
        // Verify prelude exports all expected types
        use crate::prelude::*;

        let _mode: SeekMode = SeekMode::default();
        let _accel: HardwareAccel = HardwareAccel::default();

        // Video builder can be created
        let _video_builder: VideoDecoderBuilder = VideoDecoder::open("test.mp4");

        // Audio builder can be created
        let _audio_builder: AudioDecoderBuilder = AudioDecoder::open("test.mp3");
    }
}
