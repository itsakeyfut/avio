//! # ff-analysis
//!
//! Media-analysis primitives over `FFmpeg`: scene, silence, black-frame and
//! keyframe detection, histogram and waveform extraction, tempo (BPM) results,
//! and video scopes (waveform / vectorscope / RGB parade / histogram).
//!
//! These tools read decoded media and report analytical data; they do not edit
//! or transform it. The analyzers build on [`ff_decode`] (for frame access) and
//! drive their own libavfilter graphs where needed. Errors are typed and
//! contextual ([`AnalysisError`]).
//!
//! Part of the [`avio`](https://github.com/itsakeyfut/avio) crate family; each
//! crate is versioned independently.

#![warn(missing_docs)]

mod error;

pub mod analysis;
pub mod scope;

pub use error::AnalysisError;

pub use analysis::{
    BlackFrameDetector, BpmResult, FrameHistogram, HistogramExtractor, KeyframeEnumerator,
    SceneDetector, SilenceDetector, SilenceRange, WaveformAnalyzer, WaveformSample,
};
pub use scope::{Histogram, RgbParade, ScopeAnalyzer};
