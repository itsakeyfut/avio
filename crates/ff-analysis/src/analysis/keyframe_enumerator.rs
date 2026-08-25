//! Keyframe timestamp enumeration.

#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::AnalysisError;

/// Enumerates the timestamps of all keyframes in a video stream.
///
/// Reads only packet headers — **no decoding is performed** — making this
/// significantly faster than frame-by-frame decoding.  By default the first
/// video stream is selected; call [`stream_index`](Self::stream_index) to
/// target a specific stream.
///
/// # Examples
///
/// ```ignore
/// use ff_analysis::KeyframeEnumerator;
///
/// let keyframes = KeyframeEnumerator::new("video.mp4").run()?;
/// for ts in &keyframes {
///     println!("Keyframe at {:?}", ts);
/// }
/// ```
pub struct KeyframeEnumerator {
    input: PathBuf,
    stream_index: Option<usize>,
}

impl KeyframeEnumerator {
    /// Creates a new enumerator for the given video file.
    ///
    /// The first video stream is used by default.  Call
    /// [`stream_index`](Self::stream_index) to select a different stream.
    pub fn new(input: impl AsRef<Path>) -> Self {
        Self {
            input: input.as_ref().to_path_buf(),
            stream_index: None,
        }
    }

    /// Selects a specific stream by zero-based index.
    ///
    /// When not set (the default), the first video stream in the file is used.
    #[must_use]
    pub fn stream_index(self, idx: usize) -> Self {
        Self {
            stream_index: Some(idx),
            ..self
        }
    }

    /// Enumerates keyframe timestamps and returns them in presentation order.
    ///
    /// # Errors
    ///
    /// - [`AnalysisError::Failed`] — input file not found, no video
    ///   stream exists, the requested stream index is out of range, or an
    ///   internal `FFmpeg` error occurs.
    pub fn run(self) -> Result<Vec<Duration>, AnalysisError> {
        if !self.input.exists() {
            return Err(AnalysisError::Failed {
                reason: format!("file not found: {}", self.input.display()),
            });
        }
        // SAFETY: enumerate_keyframes_unsafe manages all raw pointer lifetimes:
        // the owned InputFormatContext owns and frees the format context (closing
        // its input on drop); av_packet_alloc / av_packet_free own the packet;
        // av_packet_unref is called after every av_read_frame success.
        unsafe { super::analysis_inner::enumerate_keyframes_unsafe(&self.input, self.stream_index) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframe_enumerator_missing_file_should_return_analysis_failed() {
        let result = KeyframeEnumerator::new("does_not_exist_99999.mp4").run();
        assert!(
            matches!(result, Err(AnalysisError::Failed { .. })),
            "expected Failed for missing file, got {result:?}"
        );
    }
}
