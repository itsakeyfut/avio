//! Audio and video analysis tools.
//!
//! This module provides tools for extracting analytical data from media files.
//! Each tool lives in its own submodule; all `unsafe` `FFmpeg` filter-graph and
//! packet-level calls are confined to `analysis_inner`.

pub(crate) mod analysis_inner;
mod black_frame_detector;
mod bpm_detector;
mod histogram_extractor;
mod keyframe_enumerator;
mod scene_detector;
mod silence_detector;
mod waveform_analyzer;

pub use black_frame_detector::BlackFrameDetector;
pub use bpm_detector::BpmResult;
pub use histogram_extractor::{FrameHistogram, HistogramExtractor};
pub use keyframe_enumerator::KeyframeEnumerator;
pub use scene_detector::SceneDetector;
pub use silence_detector::{SilenceDetector, SilenceRange};
pub use waveform_analyzer::{WaveformAnalyzer, WaveformSample};
