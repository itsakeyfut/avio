//! # ff-encode
//!
//! Video and audio encoding - the Rust way.

// FFmpeg's C API is called through `unsafe`; the FFI is isolated in the
// `*_inner` modules, which carry their own scoped clippy allows for the
// FFmpeg-boundary lints (casts, pointer idioms).
#![allow(unsafe_code)]
//!
//! This crate provides video and audio encoding functionality for timeline export.
//! It supports automatic codec selection with LGPL compliance, hardware acceleration,
//! and provides a clean Builder pattern API.
//!
//! ## Features
//!
//! - **Video Encoding**: H.264, H.265, VP9, AV1, `ProRes`, `DNxHD`
//! - **Audio Encoding**: AAC, Opus, MP3, FLAC, PCM, Vorbis
//! - **Hardware Acceleration**: NVENC, QSV, AMF, `VideoToolbox`, VA-API
//! - **LGPL Compliance**: Automatic codec fallback when GPL features disabled
//! - **Progress Callbacks**: Real-time encoding progress updates
//! - **Builder Pattern**: Ergonomic encoder configuration
//!
//! ## Usage
//!
//! ### Basic Encoding
//!
//! ```ignore
//! use ff_encode::{VideoEncoder, VideoCodec, AudioCodec, BitrateMode, Preset};
//! use ff_format::VideoFrame;
//!
//! // Create encoder with Builder pattern
//! let mut encoder = VideoEncoder::create("output.mp4")?
//!     .video(1920, 1080, 30.0)          // resolution, FPS
//!     .video_codec(VideoCodec::H264)     // codec
//!     .bitrate_mode(BitrateMode::Cbr(8_000_000))  // 8 Mbps
//!     .preset(Preset::Medium)            // speed/quality balance
//!     .audio(48000, 2)                   // sample rate, channels
//!     .audio_codec(AudioCodec::Aac)
//!     .audio_bitrate(192_000)            // 192 kbps
//!     .build()?;
//!
//! // Check actual codec used
//! println!("Video codec: {}", encoder.actual_video_codec());
//! println!("Audio codec: {}", encoder.actual_audio_codec());
//!
//! // Push frames
//! for frame in frames {
//!     encoder.push_video(&frame)?;
//! }
//!
//! // Push audio
//! encoder.push_audio(&audio_samples)?;
//!
//! // Finish encoding
//! encoder.finish()?;
//! ```
//!
//! ### Progress Callbacks
//!
//! ```ignore
//! use ff_encode::{VideoEncoder, EncodeProgress};
//!
//! // Simple closure-based callback
//! let mut encoder = VideoEncoder::create("output.mp4")?
//!     .video(1920, 1080, 30.0)
//!     .on_progress(|progress| {
//!         println!("Encoded {} frames ({:.1}%) at {:.1} fps",
//!             progress.frames_encoded,
//!             progress.percent(),
//!             progress.current_fps
//!         );
//!     })
//!     .build()?;
//! ```
//!
//! ### Progress Callbacks with Cancellation
//!
//! ```ignore
//! use ff_encode::{VideoEncoder, EncodeProgressCallback, EncodeProgress};
//! use std::sync::Arc;
//! use std::sync::atomic::{AtomicBool, Ordering};
//!
//! struct CancellableProgress {
//!     cancelled: Arc<AtomicBool>,
//! }
//!
//! impl EncodeProgressCallback for CancellableProgress {
//!     fn on_progress(&mut self, progress: &EncodeProgress) {
//!         println!("Progress: {:.1}%", progress.percent());
//!     }
//!
//!     fn should_cancel(&self) -> bool {
//!         self.cancelled.load(Ordering::Relaxed)
//!     }
//! }
//!
//! let cancelled = Arc::new(AtomicBool::new(false));
//! let mut encoder = VideoEncoder::create("output.mp4")?
//!     .video(1920, 1080, 30.0)
//!     .progress_callback(CancellableProgress {
//!         cancelled: cancelled.clone()
//!     })
//!     .build()?;
//!
//! // Later, to cancel encoding:
//! cancelled.store(true, Ordering::Relaxed);
//! ```
//!
//! ### Hardware Encoding
//!
//! ```ignore
//! use ff_encode::{VideoEncoder, HardwareEncoder};
//!
//! // Check available hardware encoders
//! for hw in HardwareEncoder::available() {
//!     println!("Available: {:?}", hw);
//! }
//!
//! // Create encoder with auto hardware detection
//! let mut encoder = VideoEncoder::create("output.mp4")?
//!     .video(1920, 1080, 60.0)
//!     .hardware_encoder(HardwareEncoder::Auto)  // auto-detect
//!     .build()?;
//!
//! // Check what encoder was actually used
//! println!("Using: {}", encoder.actual_video_codec());
//! println!("Hardware encoder: {:?}", encoder.hardware_encoder());
//! println!("Is hardware encoding: {}", encoder.is_hardware_encoding());
//! ```
//!
//! ## LGPL Compliance & Commercial Use
//!
//! **By default, this crate is LGPL-compliant and safe for commercial use without licensing fees.**
//!
//! ### Default Behavior (LGPL-Compatible)
//!
//! When H.264/H.265 encoding is requested, the encoder automatically selects codecs in this priority:
//!
//! 1. **Hardware encoders** (LGPL-compatible, no licensing fees):
//!    - NVIDIA NVENC (`h264_nvenc`, `hevc_nvenc`)
//!    - Intel Quick Sync Video (`h264_qsv`, `hevc_qsv`)
//!    - AMD AMF/VCE (`h264_amf`, `hevc_amf`)
//!    - Apple `VideoToolbox` (`h264_videotoolbox`, `hevc_videotoolbox`)
//!    - VA-API (`h264_vaapi`, `hevc_vaapi`) - Linux
//!
//! 2. **Fallback to royalty-free codecs**:
//!    - For H.264 request → VP9 (libvpx-vp9)
//!    - For H.265 request → AV1 (libaom-av1)
//!
//! ### GPL Feature (Commercial Licensing Required)
//!
//! Enable GPL codecs (libx264, libx265) only if:
//! - You have appropriate licenses from MPEG LA, or
//! - Your software is GPL-licensed (open source), or
//! - For non-commercial/educational use only
//!
//! ```toml
//! # WARNING: Requires GPL compliance and licensing fees for commercial use
//! ff-encode = { version = "0.1", features = ["gpl"] }
//! ```
//!
//! ### Checking Compliance at Runtime
//!
//! You can verify which encoder was selected:
//!
//! ```ignore
//! let encoder = VideoEncoder::create("output.mp4")?
//!     .video(1920, 1080, 30.0)
//!     .video_codec(VideoCodec::H264)
//!     .build()?;
//!
//! println!("Using: {}", encoder.actual_video_codec());
//! println!("LGPL compliant: {}", encoder.is_lgpl_compliant());
//! ```

#[cfg(feature = "tokio")]
mod async_encoder;
mod audio;
mod error;
mod image;
mod preset;
#[cfg(feature = "preview-image")]
mod preview;
mod shared;
mod video;

pub use audio::{
    AacOptions, AacProfile, AudioCodecOptions, AudioEncoder, AudioEncoderBuilder, FlacOptions,
    Mp3Options, Mp3Quality, OpusApplication, OpusOptions,
};
pub use error::EncodeError;
pub use ff_format::{ErrorSeverity, MediaError};
pub use image::{ImageEncoder, ImageEncoderBuilder};
pub use preset::{AudioEncoderConfig, ExportPreset, VideoEncoderConfig};
#[cfg(feature = "preview-image")]
pub use preview::{GifPreview, PreviewImageError, SpriteSheet};
pub use shared::{
    AudioCodec, BitrateMode, CRF_MAX, EncodeProgress, EncodeProgressCallback, HardwareEncoder,
    OutputContainer, Preset, VideoCodec, VideoCodecEncodeExt,
};
pub use video::{
    Av1Options, Av1Usage, DnxhdOptions, DnxhdVariant, H264Options, H264Preset, H264Profile,
    H264Tune, H265Options, H265Profile, H265Tier, ProResOptions, ProResProfile, SvtAv1Options,
    VideoCodecOptions, VideoEncoder, VideoEncoderBuilder, Vp9Options,
};

#[cfg(feature = "tokio")]
pub use audio::AsyncAudioEncoder;
#[cfg(feature = "tokio")]
pub use video::AsyncVideoEncoder;
