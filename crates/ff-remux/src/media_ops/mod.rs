//! Media stream operations — audio replacement and extraction via stream-copy remux.

mod media_inner;

use std::path::PathBuf;

use crate::error::RemuxError;

/// Replace a video file's audio track with audio from a separate source file.
///
/// The video bitstream is copied bit-for-bit (no decode/encode cycle).  The
/// audio track from `audio_input` replaces any existing audio in
/// `video_input`.
///
/// Returns [`RemuxError::OperationFailed`] when no audio stream is found
/// in `audio_input`, or no video stream is found in `video_input`.
///
/// # Example
///
/// ```ignore
/// use ff_remux::AudioReplacement;
///
/// AudioReplacement::new("source.mp4", "new_audio.aac", "output.mp4").run()?;
/// ```
pub struct AudioReplacement {
    video_input: PathBuf,
    audio_input: PathBuf,
    output: PathBuf,
}

impl AudioReplacement {
    /// Create a new `AudioReplacement`.
    ///
    /// - `video_input` — source file whose video stream is kept.
    /// - `audio_input` — source file whose first audio stream is used.
    /// - `output`      — path for the combined output file.
    pub fn new(
        video_input: impl Into<PathBuf>,
        audio_input: impl Into<PathBuf>,
        output: impl Into<PathBuf>,
    ) -> Self {
        Self {
            video_input: video_input.into(),
            audio_input: audio_input.into(),
            output: output.into(),
        }
    }

    /// Execute the audio replacement operation.
    ///
    /// # Errors
    ///
    /// - [`RemuxError::OperationFailed`] if `video_input` has no video
    ///   stream or `audio_input` has no audio stream.
    /// - [`RemuxError::Ffmpeg`] if any `FFmpeg` API call fails.
    pub fn run(self) -> Result<(), RemuxError> {
        log::debug!(
            "audio replacement start video_input={} audio_input={} output={}",
            self.video_input.display(),
            self.audio_input.display(),
            self.output.display(),
        );
        media_inner::run_audio_replacement(&self.video_input, &self.audio_input, &self.output)
    }
}

// ── AudioExtractor ────────────────────────────────────────────────────────────

/// Demux an audio track from a media file and write it to a standalone audio file.
///
/// The audio bitstream is stream-copied (no decode/encode cycle).  By default
/// the first audio stream is selected; call [`stream_index`](Self::stream_index)
/// to pick a specific one.
///
/// Returns [`RemuxError::OperationFailed`] when:
/// - no audio stream is found (or `stream_index` points to a non-audio stream), or
/// - the audio codec is incompatible with the output container.
///
/// # Example
///
/// ```ignore
/// use ff_remux::AudioExtractor;
///
/// AudioExtractor::new("source.mp4", "audio.mp3").run()?;
/// ```
pub struct AudioExtractor {
    input: PathBuf,
    output: PathBuf,
    stream_index: Option<usize>,
}

impl AudioExtractor {
    /// Create a new `AudioExtractor`.
    ///
    /// - `input`  — source media file.
    /// - `output` — destination audio file (format auto-detected from extension).
    pub fn new(input: impl Into<PathBuf>, output: impl Into<PathBuf>) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
            stream_index: None,
        }
    }

    /// Select a specific audio stream by index (0-based over all streams in
    /// the container).  Defaults to the first audio stream when not set.
    #[must_use]
    pub fn stream_index(mut self, idx: usize) -> Self {
        self.stream_index = Some(idx);
        self
    }

    /// Execute the audio extraction operation.
    ///
    /// # Errors
    ///
    /// - [`RemuxError::OperationFailed`] if no audio stream is found,
    ///   the requested stream index is invalid or not audio, or the codec is
    ///   incompatible with the output container.
    /// - [`RemuxError::Ffmpeg`] if any `FFmpeg` API call fails.
    pub fn run(self) -> Result<(), RemuxError> {
        log::debug!(
            "audio extraction start input={} output={} stream_index={:?}",
            self.input.display(),
            self.output.display(),
            self.stream_index,
        );
        media_inner::run_audio_extraction(&self.input, &self.output, self.stream_index)
    }
}

// ── AudioAdder ────────────────────────────────────────────────────────────────

/// Mux an audio track into a silent (or existing) video file.
///
/// The video bitstream is stream-copied (no decode/encode cycle).  When the
/// audio source is shorter than the video and [`loop_audio`](Self::loop_audio)
/// has been called, the audio is looped by re-seeking and advancing the PTS
/// offset until the video is exhausted.
///
/// Returns [`RemuxError::OperationFailed`] when no video stream is found
/// in `video_input` or no audio stream is found in `audio_input`.
///
/// # Example
///
/// ```ignore
/// use ff_remux::AudioAdder;
///
/// AudioAdder::new("silent.mp4", "soundtrack.mp3", "output.mp4")
///     .loop_audio()
///     .run()?;
/// ```
pub struct AudioAdder {
    video_input: PathBuf,
    audio_input: PathBuf,
    output: PathBuf,
    loop_audio: bool,
}

impl AudioAdder {
    /// Create a new `AudioAdder`.
    ///
    /// - `video_input` — source file whose video stream is kept.
    /// - `audio_input` — source file whose first audio stream is used.
    /// - `output`      — path for the combined output file.
    pub fn new(
        video_input: impl Into<PathBuf>,
        audio_input: impl Into<PathBuf>,
        output: impl Into<PathBuf>,
    ) -> Self {
        Self {
            video_input: video_input.into(),
            audio_input: audio_input.into(),
            output: output.into(),
            loop_audio: false,
        }
    }

    /// Loop the audio when it is shorter than the video.
    ///
    /// The audio is re-seeked to the start and the PTS offset is advanced each
    /// time the audio stream is exhausted, until the video ends.
    #[must_use]
    pub fn loop_audio(mut self) -> Self {
        self.loop_audio = true;
        self
    }

    /// Execute the audio addition operation.
    ///
    /// # Errors
    ///
    /// - [`RemuxError::OperationFailed`] if `video_input` has no video
    ///   stream or `audio_input` has no audio stream.
    /// - [`RemuxError::Ffmpeg`] if any `FFmpeg` API call fails.
    pub fn run(self) -> Result<(), RemuxError> {
        log::debug!(
            "audio addition start video_input={} audio_input={} output={} loop_audio={}",
            self.video_input.display(),
            self.audio_input.display(),
            self.output.display(),
            self.loop_audio,
        );
        media_inner::run_audio_addition(
            &self.video_input,
            &self.audio_input,
            &self.output,
            self.loop_audio,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_replacement_run_with_nonexistent_video_input_should_fail() {
        let result =
            AudioReplacement::new("nonexistent_video.mp4", "nonexistent_audio.mp3", "out.mp4")
                .run();
        assert!(
            result.is_err(),
            "expected error for nonexistent video input, got Ok(())"
        );
    }

    #[test]
    fn audio_extractor_run_with_nonexistent_input_should_fail() {
        let result = AudioExtractor::new("nonexistent_input.mp4", "out.mp3").run();
        assert!(
            result.is_err(),
            "expected error for nonexistent input, got Ok(())"
        );
    }

    #[test]
    fn audio_adder_run_with_nonexistent_video_input_should_fail() {
        let result =
            AudioAdder::new("nonexistent_video.mp4", "nonexistent_audio.mp3", "out.mp4").run();
        assert!(
            result.is_err(),
            "expected error for nonexistent video input, got Ok(())"
        );
    }
}
