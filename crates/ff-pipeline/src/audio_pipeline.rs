//! Audio-only transcoding pipeline.
//!
//! This module provides [`AudioPipeline`], which wraps the
//! `AudioDecoder` → `AudioEncoder` decode/encode loop behind a
//! single high-level builder API.  It mirrors the design of
//! [`ThumbnailPipeline`](crate::ThumbnailPipeline) but targets
//! audio-only files and supports multi-input concatenation.

use std::time::Instant;

use ff_decode::AudioDecoder;
use ff_encode::AudioEncoder;
use ff_format::{AudioCodec, Timestamp};

use crate::error::PipelineError;
use crate::progress::{Progress, ProgressCallback};

/// High-level audio-only transcode pipeline.
///
/// # Construction
///
/// Use the consuming builder pattern:
///
/// ```ignore
/// use ff_pipeline::AudioPipeline;
/// use ff_format::AudioCodec;
///
/// AudioPipeline::new()
///     .input("input.mp3")
///     .output("output.aac")
///     .audio_codec(AudioCodec::Aac)
///     .bitrate(128_000)
///     .run()?;
/// ```
pub struct AudioPipeline {
    inputs: Vec<String>,
    output: Option<String>,
    audio_codec: AudioCodec,
    bitrate: Option<u64>,
    callback: Option<ProgressCallback>,
}

impl Default for AudioPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioPipeline {
    /// Creates a new pipeline with default settings (AAC codec, no bitrate override).
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            output: None,
            audio_codec: AudioCodec::default(),
            bitrate: None,
            callback: None,
        }
    }

    /// Appends an input file path.
    ///
    /// Multiple inputs are concatenated in order.
    #[must_use]
    pub fn input(mut self, path: &str) -> Self {
        self.inputs.push(path.to_owned());
        self
    }

    /// Sets the output file path.
    #[must_use]
    pub fn output(mut self, path: &str) -> Self {
        self.output = Some(path.to_owned());
        self
    }

    /// Sets the audio codec for the output.
    #[must_use]
    pub fn audio_codec(mut self, codec: AudioCodec) -> Self {
        self.audio_codec = codec;
        self
    }

    /// Sets the target bitrate in bits per second.
    #[must_use]
    pub fn bitrate(mut self, bps: u64) -> Self {
        self.bitrate = Some(bps);
        self
    }

    /// Registers a progress callback.
    ///
    /// The closure receives a [`Progress`] reference on each decoded frame
    /// and must return `true` to continue or `false` to cancel the pipeline.
    #[must_use]
    pub fn on_progress(mut self, cb: impl Fn(&Progress) -> bool + Send + 'static) -> Self {
        self.callback = Some(Box::new(cb));
        self
    }

    /// Runs the pipeline: decodes all inputs in sequence and encodes to output.
    ///
    /// # Errors
    ///
    /// - [`PipelineError::NoOutput`]  — no output path was set
    /// - [`PipelineError::NoInput`]   — no input paths were provided
    /// - [`PipelineError::Decode`]    — a decoding error occurred
    /// - [`PipelineError::Encode`]    — an encoding error occurred
    /// - [`PipelineError::Cancelled`] — the progress callback returned `false`
    pub fn run(mut self) -> Result<(), PipelineError> {
        let out_path = self.output.take().ok_or(PipelineError::NoOutput)?;

        if self.inputs.is_empty() {
            return Err(PipelineError::NoInput);
        }

        // Probe the first input for sample rate and channel count.
        let first_dec = AudioDecoder::open(&self.inputs[0]).build()?;
        let sample_rate = first_dec.sample_rate();
        let channels = first_dec.channels();
        drop(first_dec);

        // Build the encoder.
        let mut enc_builder = AudioEncoder::create(&out_path)
            .audio(sample_rate, channels)
            .audio_codec(self.audio_codec);
        if let Some(bps) = self.bitrate {
            enc_builder = enc_builder.audio_bitrate(bps);
        }
        let mut encoder = enc_builder.build()?;

        log::info!(
            "audio pipeline starting inputs={} output={out_path} \
             sample_rate={sample_rate} channels={channels}",
            self.inputs.len()
        );

        let start = Instant::now();
        let mut frames_processed: u64 = 0;
        let mut cancelled = false;
        let mut pts_offset_secs: f64 = 0.0;

        'inputs: for input in &self.inputs {
            let mut adec = AudioDecoder::open(input).build()?;
            let mut last_end_secs = pts_offset_secs;

            while let Some(mut aframe) = adec.decode_one()? {
                let ts = aframe.timestamp();
                let new_pts = pts_offset_secs + ts.as_secs_f64();
                let frame_dur = frame_duration_secs(aframe.samples(), sample_rate);
                last_end_secs = new_pts + frame_dur;

                aframe.set_timestamp(Timestamp::from_secs_f64(new_pts, ts.time_base()));
                encoder.push(&aframe)?;

                frames_processed += 1;

                if let Some(ref cb) = self.callback {
                    let progress = Progress {
                        frames_processed,
                        total_frames: None,
                        elapsed: start.elapsed(),
                    };
                    if !cb(&progress) {
                        cancelled = true;
                        break 'inputs;
                    }
                }
            }

            pts_offset_secs = last_end_secs;
            log::debug!(
                "audio input complete path={input} pts_offset_secs={:.3}",
                pts_offset_secs
            );
        }

        encoder.finish()?;

        log::info!(
            "audio pipeline finished frames_processed={frames_processed} elapsed={:?}",
            start.elapsed()
        );

        if cancelled {
            return Err(PipelineError::Cancelled);
        }

        Ok(())
    }
}

#[allow(clippy::cast_precision_loss)]
fn frame_duration_secs(samples: usize, sample_rate: u32) -> f64 {
    if sample_rate > 0 {
        samples as f64 / f64::from(sample_rate)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_should_have_default_aac_codec() {
        let p = AudioPipeline::new();
        assert_eq!(p.audio_codec, AudioCodec::Aac);
    }

    #[test]
    fn input_should_append_to_inputs() {
        let p = AudioPipeline::new().input("a.mp3").input("b.mp3");
        assert_eq!(p.inputs, vec!["a.mp3", "b.mp3"]);
    }

    #[test]
    fn output_should_store_path() {
        let p = AudioPipeline::new().output("out.aac");
        assert_eq!(p.output.as_deref(), Some("out.aac"));
    }

    #[test]
    fn audio_codec_should_store_value() {
        let p = AudioPipeline::new().audio_codec(AudioCodec::Mp3);
        assert_eq!(p.audio_codec, AudioCodec::Mp3);
    }

    #[test]
    fn bitrate_should_store_value() {
        let p = AudioPipeline::new().bitrate(192_000);
        assert_eq!(p.bitrate, Some(192_000));
    }

    #[test]
    fn run_with_no_output_should_return_no_output_error() {
        let result = AudioPipeline::new().input("x.mp3").run();
        assert!(matches!(result, Err(PipelineError::NoOutput)));
    }

    #[test]
    fn run_with_no_inputs_should_return_no_input_error() {
        let result = AudioPipeline::new().output("out.aac").run();
        assert!(matches!(result, Err(PipelineError::NoInput)));
    }
}
