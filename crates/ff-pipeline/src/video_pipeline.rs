//! Video-only transcoding pipeline.
//!
//! This module provides [`VideoPipeline`], which wraps the
//! `VideoDecoder` → `VideoEncoder` decode/encode loop behind a
//! single high-level builder API.  It mirrors the design of
//! [`AudioPipeline`](crate::AudioPipeline) but targets video-only
//! output and supports multi-input concatenation.

use std::time::Instant;

use ff_decode::VideoDecoder;
use ff_encode::{BitrateMode, VideoEncoder};
use ff_filter::HwAccel;
use ff_format::{Timestamp, VideoCodec};

use crate::error::PipelineError;
use crate::pipeline::hwaccel_to_hardware_encoder;
use crate::progress::{Progress, ProgressCallback};

/// High-level video-only transcode pipeline.
///
/// Audio is never encoded; this pipeline always produces video-only output.
/// Calling [`.mute()`](Self::mute) makes that intent explicit in code, but
/// is otherwise a no-op.
///
/// # Construction
///
/// Use the consuming builder pattern:
///
/// ```ignore
/// use ff_pipeline::VideoPipeline;
/// use ff_format::VideoCodec;
/// use ff_encode::BitrateMode;
///
/// VideoPipeline::new()
///     .input("input.mp4")
///     .output("output.mp4")
///     .video_codec(VideoCodec::H265)
///     .bitrate_mode(BitrateMode::Crf(28))
///     .mute()
///     .run()?;
/// ```
pub struct VideoPipeline {
    inputs: Vec<String>,
    output: Option<String>,
    video_codec: VideoCodec,
    resolution: Option<(u32, u32)>,
    framerate: Option<f64>,
    bitrate_mode: BitrateMode,
    /// Declarative marker — this pipeline always produces video-only output;
    /// calling `.mute()` makes the intent explicit in code.
    mute: bool,
    hardware: Option<HwAccel>,
    callback: Option<ProgressCallback>,
}

impl Default for VideoPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoPipeline {
    /// Creates a new pipeline with default settings (H.264 codec, CRF 23, software encoding).
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            output: None,
            video_codec: VideoCodec::H264,
            resolution: None,
            framerate: None,
            bitrate_mode: BitrateMode::Crf(23),
            mute: false,
            hardware: None,
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

    /// Sets the video codec for the output.
    #[must_use]
    pub fn video_codec(mut self, codec: VideoCodec) -> Self {
        self.video_codec = codec;
        self
    }

    /// Sets the output resolution in pixels.
    ///
    /// Defaults to the source resolution when not set.
    #[must_use]
    pub fn resolution(mut self, width: u32, height: u32) -> Self {
        self.resolution = Some((width, height));
        self
    }

    /// Sets the output frame rate in frames per second.
    ///
    /// Defaults to the source frame rate when not set.
    #[must_use]
    pub fn framerate(mut self, fps: f64) -> Self {
        self.framerate = Some(fps);
        self
    }

    /// Sets the bitrate control mode (CBR, VBR, or CRF).
    #[must_use]
    pub fn bitrate_mode(mut self, mode: BitrateMode) -> Self {
        self.bitrate_mode = mode;
        self
    }

    /// Declarative marker — this pipeline always produces video-only output;
    /// calling `.mute()` makes the intent explicit in code.
    #[must_use]
    pub fn mute(mut self) -> Self {
        self.mute = true;
        self
    }

    /// Sets the hardware acceleration backend.
    #[must_use]
    pub fn hardware(mut self, hw: HwAccel) -> Self {
        self.hardware = Some(hw);
        self
    }

    /// Registers a progress callback.
    ///
    /// The closure receives a [`Progress`] reference on each encoded frame
    /// and must return `true` to continue or `false` to cancel the pipeline.
    #[must_use]
    pub fn on_progress(mut self, cb: impl Fn(&Progress) -> bool + Send + 'static) -> Self {
        self.callback = Some(Box::new(cb));
        self
    }

    /// Runs the pipeline: decodes all inputs in sequence and encodes to output (video only).
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

        let num_inputs = self.inputs.len();

        // Open the first input to determine output dimensions and frame rate.
        let first_vdec = VideoDecoder::open(&self.inputs[0]).build()?;
        let (out_w, out_h) = self
            .resolution
            .unwrap_or_else(|| (first_vdec.width(), first_vdec.height()));
        let fps = self.framerate.unwrap_or_else(|| first_vdec.frame_rate());

        // total_frames is only meaningful for single-input pipelines.
        let total_frames = if num_inputs == 1 {
            first_vdec.stream_info().frame_count()
        } else {
            None
        };

        log::info!(
            "video pipeline starting inputs={num_inputs} output={out_path} \
             width={out_w} height={out_h} fps={fps} total_frames={total_frames:?}"
        );

        let hw = hwaccel_to_hardware_encoder(self.hardware);
        let mut encoder = VideoEncoder::create(&out_path)
            .video(out_w, out_h, fps)
            .video_codec(self.video_codec)
            .bitrate_mode(self.bitrate_mode)
            .hardware_encoder(hw)
            .build()?;

        let start = Instant::now();
        let mut frames_processed: u64 = 0;
        let mut cancelled = false;
        let frame_period_secs = if fps > 0.0 { 1.0 / fps } else { 0.0 };

        // PTS offset in seconds: accumulates the duration of all processed inputs.
        let mut pts_offset_secs: f64 = 0.0;

        // Reuse the already-opened first decoder; open fresh decoders for subsequent inputs.
        let mut maybe_first_vdec = Some(first_vdec);

        'inputs: for input in &self.inputs {
            let mut vdec = if let Some(vd) = maybe_first_vdec.take() {
                vd
            } else {
                VideoDecoder::open(input).build()?
            };

            let mut last_frame_end_secs: f64 = pts_offset_secs;

            while let Some(mut raw_frame) = vdec.decode_one()? {
                // Rebase timestamp so this clip follows the previous one.
                let ts = raw_frame.timestamp();
                let new_pts_secs = pts_offset_secs + ts.as_secs_f64();
                last_frame_end_secs = new_pts_secs + frame_period_secs;
                raw_frame.set_timestamp(Timestamp::from_secs_f64(new_pts_secs, ts.time_base()));

                encoder.push_video(&raw_frame)?;
                frames_processed += 1;

                if let Some(ref cb) = self.callback {
                    let progress = Progress {
                        frames_processed,
                        total_frames,
                        elapsed: start.elapsed(),
                    };
                    if !cb(&progress) {
                        log::info!(
                            "video pipeline cancelled by callback \
                             frames_processed={frames_processed}"
                        );
                        cancelled = true;
                        break 'inputs;
                    }
                }
            }

            // Advance PTS offset to the end of the last frame of this input.
            pts_offset_secs = last_frame_end_secs;
            log::debug!("input complete path={input} pts_offset_secs={pts_offset_secs:.3}");
        }

        encoder.finish()?;

        let elapsed = start.elapsed();
        log::info!(
            "video pipeline finished frames_processed={frames_processed} elapsed={elapsed:?}"
        );

        if cancelled {
            return Err(PipelineError::Cancelled);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff_encode::BitrateMode;
    use ff_format::VideoCodec;

    #[test]
    fn new_should_have_default_h264_codec() {
        let p = VideoPipeline::new();
        assert_eq!(p.video_codec, VideoCodec::H264);
    }

    #[test]
    fn new_should_have_default_crf_23_bitrate_mode() {
        let p = VideoPipeline::new();
        assert!(matches!(p.bitrate_mode, BitrateMode::Crf(23)));
    }

    #[test]
    fn input_should_append_to_inputs() {
        let p = VideoPipeline::new().input("a.mp4").input("b.mp4");
        assert_eq!(p.inputs, vec!["a.mp4", "b.mp4"]);
    }

    #[test]
    fn output_should_store_path() {
        let p = VideoPipeline::new().output("out.mp4");
        assert_eq!(p.output.as_deref(), Some("out.mp4"));
    }

    #[test]
    fn video_codec_should_store_value() {
        let p = VideoPipeline::new().video_codec(VideoCodec::H265);
        assert_eq!(p.video_codec, VideoCodec::H265);
    }

    #[test]
    fn resolution_should_store_value() {
        let p = VideoPipeline::new().resolution(1920, 1080);
        assert_eq!(p.resolution, Some((1920, 1080)));
    }

    #[test]
    fn framerate_should_store_value() {
        let p = VideoPipeline::new().framerate(60.0);
        assert_eq!(p.framerate, Some(60.0));
    }

    #[test]
    fn mute_should_set_flag() {
        let p = VideoPipeline::new().mute();
        assert!(p.mute);
    }

    #[test]
    fn hardware_should_store_value() {
        let p = VideoPipeline::new().hardware(HwAccel::Cuda);
        assert_eq!(p.hardware, Some(HwAccel::Cuda));
    }

    #[test]
    fn run_with_no_output_should_return_no_output_error() {
        let result = VideoPipeline::new().input("x.mp4").run();
        assert!(matches!(result, Err(PipelineError::NoOutput)));
    }

    #[test]
    fn run_with_no_inputs_should_return_no_input_error() {
        let result = VideoPipeline::new().output("out.mp4").run();
        assert!(matches!(result, Err(PipelineError::NoInput)));
    }
}
