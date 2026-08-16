//! # ff-pipeline
//!
//! Unified decode → filter → encode pipeline for the ff-* crate family.
//!
//! This crate wires together `ff-decode`, `ff-filter`, and `ff-encode` into a
//! single high-level API. All `unsafe` `FFmpeg` internals remain encapsulated in
//! the underlying crates; users of `ff-pipeline` never need to write `unsafe` code.
//!
//! ## Features
//!
//! - **Single-call transcode**: open → filter → encode in one `Pipeline::run()`
//! - **Progress callbacks**: real-time frame-count and elapsed-time updates
//! - **Cancellation**: returning `false` from the progress callback aborts the pipeline
//! - **Multi-input concatenation**: pass multiple input paths to concatenate clips
//!
//! ## Usage
//!
//! ```ignore
//! use ff_pipeline::{Pipeline, EncoderConfig};
//! use ff_format::{VideoCodec, AudioCodec};
//! use ff_encode::BitrateMode;
//!
//! let config = EncoderConfig::builder()
//!     .video_codec(VideoCodec::H264)
//!     .audio_codec(AudioCodec::Aac)
//!     .bitrate_mode(BitrateMode::Cbr(4_000_000))
//!     .resolution(1280, 720)
//!     .framerate(30.0)
//!     .build();
//!
//! Pipeline::builder()
//!     .input("input.mp4")
//!     .output("output.mp4", config)
//!     .on_progress(|p| {
//!         println!("frame={} elapsed={:?}", p.frames_processed, p.elapsed);
//!         true // return false to cancel
//!     })
//!     .build()?
//!     .run()?;
//! ```
//!
//! ## Module Structure
//!
//! - [`audio_pipeline`] — [`AudioPipeline`]
//! - [`encoder_config`] — [`EncoderConfig`], [`EncoderConfigBuilder`]
//! - [`error`] — [`PipelineError`]
//! - [`pipeline`] — [`Pipeline`], [`PipelineBuilder`]
//! - [`progress`] — [`Progress`], [`ProgressCallback`]
//! - [`thumbnail`] — [`ThumbnailPipeline`]
//! - [`video_pipeline`] — [`VideoPipeline`]

#![warn(missing_docs)]

pub mod audio_pipeline;
pub mod encoder_config;
pub mod error;
pub mod pipeline;
pub mod progress;
pub mod thumbnail;
pub mod video_pipeline;

pub use audio_pipeline::AudioPipeline;
pub use encoder_config::{EncoderConfig, EncoderConfigBuilder};
pub use error::PipelineError;
pub use pipeline::{Pipeline, PipelineBuilder};
pub use progress::{Progress, ProgressCallback};
pub use thumbnail::ThumbnailPipeline;
pub use video_pipeline::VideoPipeline;
