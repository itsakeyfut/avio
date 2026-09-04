//! Internal video encoder implementation.
//!
//! This module contains the internal implementation details of the video encoder,
//! including FFmpeg context management and encoding operations.

// Rust 2024: Allow unsafe operations in unsafe functions for FFmpeg C API
#![allow(unsafe_op_in_unsafe_fn)]
// FFmpeg-boundary lints: casts at the C ABI, pointer idioms, C-string
// literals, and FFI-wrapper ergonomics concentrate in this unsafe module.
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::manual_c_str_literals)]
#![allow(clippy::unused_self)]

mod color;
mod context;
mod encoding;
mod hdr;
mod options;
mod two_pass;

pub(super) use options::preset_to_string;
pub(super) use two_pass::TwoPassFrame;

use crate::{AudioCodec, EncodeError, VideoCodec};
use ff_format::{AudioFrame, VideoFrame};
use ff_sys::{
    AVAudioFifo, AVChannelLayout, AVCodecID, AVCodecID_AV_CODEC_ID_AAC, AVCodecID_AV_CODEC_ID_AC3,
    AVCodecID_AV_CODEC_ID_ALAC, AVCodecID_AV_CODEC_ID_AV1, AVCodecID_AV_CODEC_ID_DNXHD,
    AVCodecID_AV_CODEC_ID_DTS, AVCodecID_AV_CODEC_ID_EAC3, AVCodecID_AV_CODEC_ID_FFV1,
    AVCodecID_AV_CODEC_ID_FLAC, AVCodecID_AV_CODEC_ID_H264, AVCodecID_AV_CODEC_ID_HEVC,
    AVCodecID_AV_CODEC_ID_MJPEG, AVCodecID_AV_CODEC_ID_MP3, AVCodecID_AV_CODEC_ID_MPEG2VIDEO,
    AVCodecID_AV_CODEC_ID_MPEG4, AVCodecID_AV_CODEC_ID_NONE, AVCodecID_AV_CODEC_ID_OPUS,
    AVCodecID_AV_CODEC_ID_PCM_S16LE, AVCodecID_AV_CODEC_ID_PCM_S24LE, AVCodecID_AV_CODEC_ID_PNG,
    AVCodecID_AV_CODEC_ID_PRORES, AVCodecID_AV_CODEC_ID_VORBIS, AVCodecID_AV_CODEC_ID_VP8,
    AVCodecID_AV_CODEC_ID_VP9, AVMediaType_AVMEDIA_TYPE_SUBTITLE,
    AVPacketSideDataType_AV_PKT_DATA_CONTENT_LIGHT_LEVEL,
    AVPacketSideDataType_AV_PKT_DATA_MASTERING_DISPLAY_METADATA, AVPixelFormat,
    AVPixelFormat_AV_PIX_FMT_YUV420P, OutputFormatContext, swresample,
};
use std::ffi::CString;

/// Internal encoder state with FFmpeg contexts.
pub(super) struct VideoEncoderInner {
    /// Output format context
    pub(super) format_ctx: OutputFormatContext,

    /// Video codec context
    pub(super) video_codec_ctx: Option<ff_sys::CodecContext>,

    /// Audio codec context (for future use)
    pub(super) audio_codec_ctx: Option<ff_sys::CodecContext>,

    /// Video stream index
    pub(super) video_stream_index: i32,

    /// Audio stream index
    pub(super) audio_stream_index: i32,

    /// Scaling context for pixel format conversion
    pub(super) sws_ctx: Option<ff_sys::ScaleContext>,

    /// Resampling context for audio format conversion
    pub(super) swr_ctx: Option<ff_sys::ResampleContext>,

    /// Sample FIFO for fixed-frame-size codecs (AAC, FLAC, ALAC …).
    /// `None` for variable-frame-size codecs (PCM, Vorbis) where `frame_size == 0`.
    pub(super) audio_fifo: Option<*mut AVAudioFifo>,

    /// Frame counter
    pub(super) frame_count: u64,

    /// Audio sample counter
    pub(super) audio_sample_count: u64,

    /// Bytes written
    pub(super) bytes_written: u64,

    /// Actual video codec name being used
    pub(super) actual_video_codec: String,

    /// Actual audio codec name being used
    pub(super) actual_audio_codec: String,

    /// Last source frame width (for SwsContext reuse optimization)
    pub(super) last_src_width: Option<u32>,

    /// Last source frame height (for SwsContext reuse optimization)
    pub(super) last_src_height: Option<u32>,

    /// Last source frame format (for SwsContext reuse optimization)
    pub(super) last_src_format: Option<AVPixelFormat>,

    /// Whether two-pass encoding is active.
    pub(super) two_pass: bool,

    /// Pass-1 codec context (two-pass mode only; None in single-pass and after pass 1 completes).
    pub(super) pass1_codec_ctx: Option<ff_sys::CodecContext>,

    /// Buffered YUV420P frame data for pass-2 re-encoding (two-pass mode only).
    pub(super) buffered_frames: Vec<TwoPassFrame>,

    /// Stored configuration for reconstructing the pass-2 codec context.
    pub(super) two_pass_config: Option<VideoEncoderConfig>,

    /// Subtitle passthrough info: (source_path, source_stream_index, output_stream_index).
    ///
    /// Set by `init_subtitle_passthrough`; read by `write_subtitle_packets`.
    /// `None` if no subtitle passthrough was requested.
    pub(super) subtitle_passthrough: Option<(String, usize, i32)>,

    /// HDR10 static metadata to embed in keyframe packets.
    ///
    /// `None` if no HDR metadata was requested.
    pub(super) hdr10_metadata: Option<ff_format::Hdr10Metadata>,
}

/// VideoEncoder configuration (stored from builder).
#[derive(Debug, Clone)]
pub(super) struct VideoEncoderConfig {
    pub(super) path: std::path::PathBuf,
    pub(super) video_width: Option<u32>,
    pub(super) video_height: Option<u32>,
    pub(super) video_fps: Option<f64>,
    pub(super) video_codec: VideoCodec,
    pub(super) video_bitrate_mode: Option<crate::BitrateMode>,
    pub(super) preset: String,
    pub(super) hardware_encoder: crate::HardwareEncoder,
    pub(super) audio_sample_rate: Option<u32>,
    pub(super) audio_channels: Option<u32>,
    pub(super) audio_codec: AudioCodec,
    pub(super) audio_bitrate: Option<u64>,
    pub(super) _progress_callback: bool,
    pub(super) two_pass: bool,
    pub(super) metadata: Vec<(String, String)>,
    pub(super) chapters: Vec<ff_format::chapter::ChapterInfo>,
    pub(super) subtitle_passthrough: Option<(String, usize)>,
    pub(super) codec_options: Option<crate::video::codec_options::VideoCodecOptions>,
    /// Codec-private options set by name; applied after `codec_options`.
    pub(super) codec_opts: Vec<(String, String)>,
    pub(super) pixel_format: Option<ff_format::PixelFormat>,
    pub(super) hdr10_metadata: Option<ff_format::Hdr10Metadata>,
    pub(super) color_space: Option<ff_format::ColorSpace>,
    pub(super) color_transfer: Option<ff_format::ColorTransfer>,
    pub(super) color_primaries: Option<ff_format::ColorPrimaries>,
    /// Binary attachments: (raw data, MIME type, filename).
    pub(super) attachments: Vec<(Vec<u8>, String, String)>,
    pub(super) container: Option<crate::OutputContainer>,
    /// Relocate the `moov` atom to the front for progressive MP4/MOV playback.
    pub(super) faststart: bool,
}

impl VideoEncoderInner {
    /// Create a new encoder with the given configuration.
    pub(super) fn new(config: &VideoEncoderConfig) -> Result<Self, EncodeError> {
        unsafe {
            ff_sys::ensure_initialized();

            // For image-sequence outputs (path contains '%'), use the `image2`
            // muxer explicitly. The `image2` muxer manages I/O internally
            // (AVFMT_NOFILE), so we must not call avio_open on it.
            let is_image_sequence = config.path.to_str().is_some_and(|s| s.contains('%'));

            // Allocate output format context (owned; frees itself / closes its IO
            // on drop, so any early return below cannot leak it).
            let format_ctx =
                OutputFormatContext::new(is_image_sequence.then_some("image2"), &config.path)
                    .map_err(|e| EncodeError::Ffmpeg {
                        code: e.code(),
                        message: format!(
                            "Cannot create output context: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    })?;

            let mut encoder = Self {
                format_ctx,
                video_codec_ctx: None,
                audio_codec_ctx: None,
                video_stream_index: -1,
                audio_stream_index: -1,
                sws_ctx: None,
                swr_ctx: None,
                audio_fifo: None,
                frame_count: 0,
                audio_sample_count: 0,
                bytes_written: 0,
                actual_video_codec: String::new(),
                actual_audio_codec: String::new(),
                last_src_width: None,
                last_src_height: None,
                last_src_format: None,
                two_pass: config.two_pass,
                pass1_codec_ctx: None,
                buffered_frames: Vec::new(),
                two_pass_config: None,
                subtitle_passthrough: None,
                hdr10_metadata: config.hdr10_metadata.clone(),
            };

            // Initialize video encoder if configured
            if let (Some(width), Some(height), Some(fps)) =
                (config.video_width, config.video_height, config.video_fps)
            {
                encoder.init_video_encoder(
                    width,
                    height,
                    fps,
                    config.video_codec,
                    config.video_bitrate_mode.as_ref(),
                    &config.preset,
                    config.hardware_encoder,
                    config.two_pass,
                    config.codec_options.as_ref(),
                    &config.codec_opts,
                    config.pixel_format.as_ref(),
                    config.color_space,
                    config.color_transfer,
                    config.color_primaries,
                )?;
            }

            // Store config for pass-2 reconstruction (two-pass mode only).
            if config.two_pass {
                encoder.two_pass_config = Some(config.clone());
            }

            // Initialize audio encoder if configured
            if let (Some(sample_rate), Some(channels)) =
                (config.audio_sample_rate, config.audio_channels)
            {
                encoder.init_audio_encoder(
                    sample_rate,
                    channels,
                    config.audio_codec,
                    config.audio_bitrate,
                )?;
            }

            // Register subtitle passthrough stream (must happen before avformat_write_header).
            if let Some((ref path, stream_index)) = config.subtitle_passthrough {
                encoder.init_subtitle_passthrough(path, stream_index);
            }

            // Register attachment streams (must happen before avformat_write_header).
            if !config.attachments.is_empty() {
                encoder.init_attachments(&config.attachments);
            }

            // For two-pass encoding the output file is opened in run_pass2() after
            // pass-1 statistics have been collected.  Single-pass opens it now.
            // Image-sequence output (path contains '%') uses the `image2` muxer which
            // manages I/O internally (AVFMT_NOFILE) — skip avio_open in that case.
            if !config.two_pass {
                // Muxers that manage their own IO (image2 sequences, AVFMT_NOFILE)
                // must not have a `pb` opened for them.
                if !encoder.format_ctx.is_nofile() {
                    encoder.format_ctx.open_io(&config.path).map_err(|_| {
                        EncodeError::CannotCreateFile {
                            path: config.path.clone(),
                        }
                    })?;
                }

                Self::apply_movflags(
                    &mut encoder.format_ctx,
                    config.container,
                    &config.path,
                    config.faststart,
                );
                Self::apply_metadata(&mut encoder.format_ctx, &config.metadata);
                Self::apply_chapters(&mut encoder.format_ctx, &config.chapters);
                encoder
                    .format_ctx
                    .write_header()
                    .map_err(|e| EncodeError::Ffmpeg {
                        code: e.code(),
                        message: format!(
                            "Cannot write header: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    })?;
            }

            Ok(encoder)
        }
    }

    /// Push a video frame for encoding.
    ///
    /// In two-pass mode the frame is converted to YUV420P via the pass-1 codec
    /// context, the converted data is buffered for pass-2 replay, and the frame
    /// is then sent through the pass-1 encoder (whose output is discarded).
    pub(super) fn push_video_frame(&mut self, frame: &VideoFrame) -> Result<(), EncodeError> {
        // SAFETY: self is properly initialised; all raw FFmpeg pointers are valid and exclusively owned.
        unsafe {
            // Two-pass path
            if self.two_pass {
                // Read the pass-1 codec's target format up front (Copy scalars) so
                // the `&mut self` convert call does not alias the codec context.
                let (pass1_fmt, pass1_width, pass1_height) = {
                    let cc = self.pass1_codec_ctx.as_ref().ok_or_else(|| {
                        EncodeError::InvalidConfig {
                            reason: "Pass-1 codec context not initialized".to_string(),
                        }
                    })?;
                    (cc.pix_fmt(), cc.width(), cc.height())
                };

                // Convert the incoming frame to YUV420P (the pass-1 codec's format).
                let mut av_frame = ff_sys::Frame::new().map_err(|_| EncodeError::Ffmpeg {
                    code: 0,
                    message: "Cannot allocate frame".to_string(),
                })?;

                self.convert_video_frame(
                    frame,
                    &mut av_frame,
                    pass1_fmt,
                    pass1_width,
                    pass1_height,
                )?;

                // Buffer the converted YUV420P data for pass-2 replay. `video_plane`
                // self-sizes each plane (Y full height, U/V by chroma subsampling)
                // and yields `None` (→ empty Vec) for an absent plane.
                let width = pass1_width as u32;
                let height = pass1_height as u32;

                let planes: Vec<Vec<u8>> = (0..3)
                    .map(|i| {
                        av_frame
                            .video_plane(i)
                            .map(<[u8]>::to_vec)
                            .unwrap_or_default()
                    })
                    .collect();

                let strides: Vec<usize> = (0..3).map(|i| av_frame.linesize(i) as usize).collect();

                self.buffered_frames.push(TwoPassFrame {
                    planes,
                    strides,
                    width,
                    height,
                    // time_base.den = fps * 1000, so one frame duration = 1000 ticks.
                    pts: self.frame_count as i64 * 1000,
                });

                // Send to pass-1 encoder and discard the encoded output.
                // time_base.den = fps * 1000, so one frame duration = 1000 ticks.
                av_frame.set_pts(self.frame_count as i64 * 1000);
                self.pass1_codec_ctx
                    .as_mut()
                    .ok_or_else(|| EncodeError::InvalidConfig {
                        reason: "Pass-1 codec context not initialized".to_string(),
                    })?
                    .send_frame(Some(&av_frame))
                    .map_err(|e| EncodeError::Ffmpeg {
                        code: e.code(),
                        message: format!(
                            "Failed to send frame to pass-1 encoder: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    })?;

                Self::drain_pass1_packets(self.pass1_codec_ctx.as_mut().ok_or_else(|| {
                    EncodeError::InvalidConfig {
                        reason: "Pass-1 codec context not initialized".to_string(),
                    }
                })?)?;

                self.frame_count += 1;
                return Ok(());
            }

            // Single-pass path
            // Read the codec's target format up front (Copy scalars) so the
            // `&mut self` convert call does not alias the codec context.
            let (target_fmt, target_width, target_height) = {
                let cc =
                    self.video_codec_ctx
                        .as_ref()
                        .ok_or_else(|| EncodeError::InvalidConfig {
                            reason: "Video codec not initialized".to_string(),
                        })?;
                (cc.pix_fmt(), cc.width(), cc.height())
            };

            // Allocate AVFrame
            let mut av_frame = ff_sys::Frame::new().map_err(|_| EncodeError::Ffmpeg {
                code: 0,
                message: "Cannot allocate frame".to_string(),
            })?;

            // Convert VideoFrame to AVFrame
            self.convert_video_frame(
                frame,
                &mut av_frame,
                target_fmt,
                target_width,
                target_height,
            )?;

            // Set frame properties.
            // time_base.den = fps * 1000, so one frame duration = 1000 ticks.
            av_frame.set_pts(self.frame_count as i64 * 1000);

            // Send frame to encoder
            self.video_codec_ctx
                .as_mut()
                .ok_or_else(|| EncodeError::InvalidConfig {
                    reason: "Video codec not initialized".to_string(),
                })?
                .send_frame(Some(&av_frame))
                .map_err(|e| EncodeError::Ffmpeg {
                    code: e.code(),
                    message: format!(
                        "Failed to send frame: {}",
                        ff_sys::av_error_string(e.code())
                    ),
                })?;

            // Receive packets (the owned `av_frame` drops at end of scope).
            self.receive_packets()?;

            self.frame_count += 1;

            Ok(())
        } // unsafe
    }

    /// Push an audio frame for encoding.
    ///
    /// For fixed-frame-size codecs (AAC, FLAC, ALAC …) samples are buffered in
    /// `audio_fifo` and drained in exact `frame_size`-sample chunks so that
    /// `avcodec_send_frame` never receives a frame whose `nb_samples` differs from
    /// what the encoder requires.  Variable-frame-size codecs (`frame_size == 0`,
    /// e.g. PCM) bypass the FIFO and send converted frames directly.
    pub(super) fn push_audio_frame(&mut self, frame: &AudioFrame) -> Result<(), EncodeError> {
        // SAFETY: self is properly initialised; all raw FFmpeg pointers are valid and exclusively owned.
        unsafe {
            let (frame_size, sample_fmt, sample_rate) = {
                let cc =
                    self.audio_codec_ctx
                        .as_ref()
                        .ok_or_else(|| EncodeError::InvalidConfig {
                            reason: "Audio codec not initialized".to_string(),
                        })?;
                (cc.frame_size(), cc.sample_fmt(), cc.sample_rate())
            };

            // Allocate and convert incoming frame.
            let mut av_frame = ff_sys::Frame::new().map_err(|_| EncodeError::Ffmpeg {
                code: 0,
                message: "Cannot allocate frame".to_string(),
            })?;
            self.convert_audio_frame(frame, &mut av_frame)?;

            // Variable frame-size path (PCM, Vorbis …)
            if frame_size <= 0 || self.audio_fifo.is_none() {
                av_frame.set_pts(self.audio_sample_count as i64);
                self.audio_codec_ctx
                    .as_mut()
                    .ok_or_else(|| EncodeError::InvalidConfig {
                        reason: "Audio codec not initialized".to_string(),
                    })?
                    .send_frame(Some(&av_frame))
                    .map_err(|e| EncodeError::Ffmpeg {
                        code: e.code(),
                        message: format!(
                            "Failed to send audio frame: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    })?;
                self.receive_audio_packets()?;
                self.audio_sample_count += frame.samples() as u64;
                return Ok(());
            }

            // Fixed frame-size path (AAC, FLAC, ALAC …)
            let fifo = self.audio_fifo.ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Audio FIFO not initialized for fixed-frame-size codec".to_string(),
            })?;

            // Write converted samples into the FIFO, then let `av_frame` drop.
            {
                let nb_samples = av_frame.nb_samples();
                ff_sys::swresample::audio_fifo::write_frame(fifo, &av_frame, nb_samples)
                    .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;
            }
            drop(av_frame);

            // Drain full frame_size chunks.
            while ff_sys::swresample::audio_fifo::size(fifo) >= frame_size {
                let mut out_frame = ff_sys::Frame::new().map_err(|_| EncodeError::Ffmpeg {
                    code: 0,
                    message: "Cannot allocate audio frame".to_string(),
                })?;
                out_frame.set_nb_samples(frame_size);
                out_frame.set_format(sample_fmt);
                out_frame.set_sample_rate(sample_rate);
                {
                    let cc = self.audio_codec_ctx.as_ref().ok_or_else(|| {
                        EncodeError::InvalidConfig {
                            reason: "Audio codec not initialized".to_string(),
                        }
                    })?;
                    out_frame
                        .set_ch_layout(cc.ch_layout())
                        .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;
                }

                out_frame.get_buffer(0).map_err(|e| EncodeError::Ffmpeg {
                    code: e.code(),
                    message: format!(
                        "Cannot allocate audio frame buffer: {}",
                        ff_sys::av_error_string(e.code())
                    ),
                })?;

                ff_sys::swresample::audio_fifo::read_frame(fifo, &mut out_frame, frame_size)
                    .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

                out_frame.set_pts(self.audio_sample_count as i64);
                self.audio_codec_ctx
                    .as_mut()
                    .ok_or_else(|| EncodeError::InvalidConfig {
                        reason: "Audio codec not initialized".to_string(),
                    })?
                    .send_frame(Some(&out_frame))
                    .map_err(|e| EncodeError::Ffmpeg {
                        code: e.code(),
                        message: format!(
                            "Failed to send audio frame: {}",
                            ff_sys::av_error_string(e.code())
                        ),
                    })?;
                self.receive_audio_packets()?;
                self.audio_sample_count += frame_size as u64;
            }

            Ok(())
        } // unsafe
    }

    /// Finish encoding and write trailer.
    pub(super) fn finish(&mut self) -> Result<(), EncodeError> {
        // SAFETY: self is properly initialised; all raw FFmpeg pointers are valid and exclusively owned.
        unsafe {
            // For two-pass, run the second pass now (handles flushing + trailer).
            if self.two_pass {
                return self.run_pass2();
            }

            // Single-pass: flush video encoder
            if self.video_codec_ctx.is_some() {
                // Send NULL frame to flush
                self.video_codec_ctx
                    .as_mut()
                    .ok_or_else(|| EncodeError::InvalidConfig {
                        reason: "Video codec not initialized".to_string(),
                    })?
                    .send_frame(None)
                    .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;
                self.receive_packets()?;
            }

            // Flush remaining FIFO samples (fixed-frame-size codecs only).
            // The last chunk may be smaller than frame_size; send it as-is so the
            // encoder can write a short final frame before the NULL-frame flush.
            if let Some(fifo) = self.audio_fifo
                && self.audio_codec_ctx.is_some()
            {
                let (aud_sample_fmt, aud_sample_rate) = {
                    let cc = self.audio_codec_ctx.as_ref().ok_or_else(|| {
                        EncodeError::InvalidConfig {
                            reason: "Audio codec not initialized".to_string(),
                        }
                    })?;
                    (cc.sample_fmt(), cc.sample_rate())
                };
                let remaining = ff_sys::swresample::audio_fifo::size(fifo);
                if remaining > 0
                    && let Ok(mut out_frame) = ff_sys::Frame::new()
                {
                    out_frame.set_nb_samples(remaining);
                    out_frame.set_format(aud_sample_fmt);
                    out_frame.set_sample_rate(aud_sample_rate);
                    if let Some(cc) = self.audio_codec_ctx.as_ref() {
                        let _ = out_frame.set_ch_layout(cc.ch_layout());
                    }
                    if out_frame.get_buffer(0).is_ok() {
                        let _ = ff_sys::swresample::audio_fifo::read_frame(
                            fifo,
                            &mut out_frame,
                            remaining,
                        );
                        out_frame.set_pts(self.audio_sample_count as i64);
                        let _ = self
                            .audio_codec_ctx
                            .as_mut()
                            .ok_or_else(|| EncodeError::InvalidConfig {
                                reason: "Audio codec not initialized".to_string(),
                            })?
                            .send_frame(Some(&out_frame));
                        let _ = self.receive_audio_packets();
                        self.audio_sample_count += remaining as u64;
                    }
                    // `out_frame` drops here.
                }
            }

            // Flush audio encoder
            if self.audio_codec_ctx.is_some() {
                // Send NULL frame to flush
                self.audio_codec_ctx
                    .as_mut()
                    .ok_or_else(|| EncodeError::InvalidConfig {
                        reason: "Audio codec not initialized".to_string(),
                    })?
                    .send_frame(None)
                    .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;
                self.receive_audio_packets()?;
            }

            // Write subtitle passthrough packets before trailer.
            self.write_subtitle_packets()?;

            // Write trailer
            self.format_ctx
                .write_trailer()
                .map_err(|e| EncodeError::Ffmpeg {
                    code: e.code(),
                    message: format!(
                        "Cannot write trailer: {}",
                        ff_sys::av_error_string(e.code())
                    ),
                })?;

            Ok(())
        } // unsafe
    }
}

impl Drop for VideoEncoderInner {
    fn drop(&mut self) {
        // SAFETY: We own all the FFmpeg resources and need to free them
        unsafe {
            self.cleanup();
        }
    }
}

// SAFETY: VideoEncoderInner owns all FFmpeg contexts exclusively.
//         These contexts are not accessed from multiple threads simultaneously;
//         all access is serialized by whichever thread holds the VideoEncoder.
//         Ownership transfer between threads is safe because FFmpeg contexts
//         are created and destroyed on the same thread (via std::thread::spawn).
unsafe impl Send for VideoEncoderInner {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_h264_encoder_candidates_auto() {
        let inner = create_dummy_encoder_inner();
        let candidates = inner.select_h264_encoder_candidates(crate::HardwareEncoder::Auto);

        // Should include hardware encoders
        #[cfg(feature = "hwaccel")]
        {
            assert!(candidates.contains(&"h264_nvenc"));
            assert!(candidates.contains(&"h264_qsv"));
        }

        // Should include libx264 if GPL feature is enabled
        #[cfg(feature = "gpl")]
        {
            assert!(candidates.contains(&"libx264"));
        }

        // Should always include VP9 fallback
        assert!(candidates.contains(&"libvpx-vp9"));
    }

    #[test]
    fn test_h264_encoder_candidates_nvenc() {
        let inner = create_dummy_encoder_inner();
        let candidates = inner.select_h264_encoder_candidates(crate::HardwareEncoder::Nvenc);

        #[cfg(feature = "hwaccel")]
        {
            // NVENC should be first priority
            assert_eq!(candidates[0], "h264_nvenc");
        }

        // Should include VP9 fallback
        assert!(candidates.contains(&"libvpx-vp9"));
    }

    #[test]
    fn test_h264_encoder_candidates_none() {
        let inner = create_dummy_encoder_inner();
        let candidates = inner.select_h264_encoder_candidates(crate::HardwareEncoder::None);

        #[cfg(feature = "hwaccel")]
        {
            // Should not include hardware encoders
            assert!(!candidates.contains(&"h264_nvenc"));
            assert!(!candidates.contains(&"h264_qsv"));
        }

        // Should include VP9 fallback
        assert!(candidates.contains(&"libvpx-vp9"));
    }

    #[test]
    fn test_h265_encoder_candidates_auto() {
        let inner = create_dummy_encoder_inner();
        let candidates = inner.select_h265_encoder_candidates(crate::HardwareEncoder::Auto);

        // Should include hardware encoders
        #[cfg(feature = "hwaccel")]
        {
            assert!(candidates.contains(&"hevc_nvenc"));
            assert!(candidates.contains(&"hevc_qsv"));
        }

        // Should include libx265 if GPL feature is enabled
        #[cfg(feature = "gpl")]
        {
            assert!(candidates.contains(&"libx265"));
        }

        // Should always include AV1 fallback
        assert!(candidates.contains(&"libaom-av1") || candidates.contains(&"libsvtav1"));
    }

    #[test]
    fn test_lgpl_fallback_priority() {
        let inner = create_dummy_encoder_inner();

        // Test H264 candidates
        let h264_candidates = inner.select_h264_encoder_candidates(crate::HardwareEncoder::None);

        #[cfg(not(feature = "gpl"))]
        {
            // Without GPL feature, should only have VP9
            assert_eq!(h264_candidates, vec!["libvpx-vp9"]);
        }

        // Test H265 candidates
        let h265_candidates = inner.select_h265_encoder_candidates(crate::HardwareEncoder::None);

        #[cfg(not(feature = "gpl"))]
        {
            // Without GPL feature, should only have AV1 options
            assert!(h265_candidates.contains(&"libaom-av1"));
            assert!(!h265_candidates.contains(&"libx265"));
        }
    }

    #[test]
    fn test_pixel_format_to_av() {
        // Test common pixel formats
        assert_eq!(
            color::pixel_format_to_av(ff_format::PixelFormat::Yuv420p),
            ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P
        );
        assert_eq!(
            color::pixel_format_to_av(ff_format::PixelFormat::Rgba),
            ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA
        );
        assert_eq!(
            color::pixel_format_to_av(ff_format::PixelFormat::Nv12),
            ff_sys::AVPixelFormat_AV_PIX_FMT_NV12
        );

        // Other(v) passes through the raw integer unchanged.
        assert_eq!(
            color::pixel_format_to_av(ff_format::PixelFormat::Other(999)),
            999 as AVPixelFormat
        );

        // 10-bit formats
        assert_eq!(
            color::pixel_format_to_av(ff_format::PixelFormat::Yuv420p10le),
            ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE
        );
        assert_eq!(
            color::pixel_format_to_av(ff_format::PixelFormat::Yuv422p10le),
            ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P10LE
        );
        assert_eq!(
            color::pixel_format_to_av(ff_format::PixelFormat::Yuv444p10le),
            ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P10LE
        );
    }

    #[test]
    fn from_av_pixel_format_yuv420p10le_should_return_yuv420p10le() {
        assert_eq!(
            color::from_av_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE),
            ff_format::PixelFormat::Yuv420p10le
        );
    }

    #[test]
    fn from_av_pixel_format_yuv422p10le_should_return_yuv422p10le() {
        assert_eq!(
            color::from_av_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P10LE),
            ff_format::PixelFormat::Yuv422p10le
        );
    }

    #[test]
    fn from_av_pixel_format_yuv444p10le_should_return_yuv444p10le() {
        assert_eq!(
            color::from_av_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P10LE),
            ff_format::PixelFormat::Yuv444p10le
        );
    }

    #[test]
    fn from_av_pixel_format_none_should_return_other() {
        // AV_PIX_FMT_NONE (-1) is an unrecognised value; must round-trip as Other.
        let fmt = ff_sys::AVPixelFormat_AV_PIX_FMT_NONE;
        assert_eq!(
            color::from_av_pixel_format(fmt),
            ff_format::PixelFormat::Other(fmt as u32)
        );
    }

    #[test]
    fn color_space_to_av_bt709_should_return_bt709() {
        assert_eq!(
            color::color_space_to_av(ff_format::ColorSpace::Bt709),
            ff_sys::AVColorSpace_AVCOL_SPC_BT709
        );
    }

    #[test]
    fn color_space_to_av_bt2020_ncl_should_return_bt2020_ncl() {
        assert_eq!(
            color::color_space_to_av(ff_format::ColorSpace::Bt2020Ncl),
            ff_sys::AVColorSpace_AVCOL_SPC_BT2020_NCL
        );
    }

    #[test]
    fn color_space_to_av_rgb_should_return_rgb() {
        assert_eq!(
            color::color_space_to_av(ff_format::ColorSpace::Rgb),
            ff_sys::AVColorSpace_AVCOL_SPC_RGB
        );
    }

    #[test]
    fn color_transfer_to_av_hlg_should_return_arib_std_b67() {
        assert_eq!(
            color::color_transfer_to_av(ff_format::ColorTransfer::Hlg),
            ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_ARIB_STD_B67
        );
    }

    #[test]
    fn color_transfer_to_av_pq_should_return_smptest2084() {
        assert_eq!(
            color::color_transfer_to_av(ff_format::ColorTransfer::Pq),
            ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_SMPTEST2084
        );
    }

    #[test]
    fn color_transfer_to_av_bt709_should_return_bt709() {
        assert_eq!(
            color::color_transfer_to_av(ff_format::ColorTransfer::Bt709),
            ff_sys::AVColorTransferCharacteristic_AVCOL_TRC_BT709
        );
    }

    #[test]
    fn color_primaries_to_av_bt2020_should_return_bt2020() {
        assert_eq!(
            color::color_primaries_to_av(ff_format::ColorPrimaries::Bt2020),
            ff_sys::AVColorPrimaries_AVCOL_PRI_BT2020
        );
    }

    #[test]
    fn color_primaries_to_av_bt709_should_return_bt709() {
        assert_eq!(
            color::color_primaries_to_av(ff_format::ColorPrimaries::Bt709),
            ff_sys::AVColorPrimaries_AVCOL_PRI_BT709
        );
    }

    #[test]
    fn test_sws_context_tracking() {
        let mut inner = create_dummy_encoder_inner();

        // Initially no context
        assert_eq!(inner.last_src_width, None);
        assert_eq!(inner.last_src_height, None);
        assert_eq!(inner.last_src_format, None);

        // After setting (simulating what convert_video_frame does)
        inner.last_src_width = Some(1920);
        inner.last_src_height = Some(1080);
        inner.last_src_format = Some(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P);

        // Verify tracking
        assert_eq!(inner.last_src_width, Some(1920));
        assert_eq!(inner.last_src_height, Some(1080));
        assert_eq!(
            inner.last_src_format,
            Some(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P)
        );
    }

    /// Helper function to create a dummy encoder inner for testing.
    fn create_dummy_encoder_inner() -> VideoEncoderInner {
        VideoEncoderInner {
            // A real (unopened) mux context; the test only exercises the sws
            // tracking fields and drops the context untouched.
            format_ctx: OutputFormatContext::new(None, std::path::Path::new("dummy.mp4"))
                .expect("dummy output context allocation should succeed"),
            video_codec_ctx: None,
            audio_codec_ctx: None,
            video_stream_index: -1,
            audio_stream_index: -1,
            sws_ctx: None,
            swr_ctx: None,
            audio_fifo: None,
            frame_count: 0,
            audio_sample_count: 0,
            bytes_written: 0,
            actual_video_codec: String::new(),
            actual_audio_codec: String::new(),
            last_src_width: None,
            last_src_height: None,
            last_src_format: None,
            two_pass: false,
            pass1_codec_ctx: None,
            buffered_frames: Vec::new(),
            two_pass_config: None,
            subtitle_passthrough: None,
            hdr10_metadata: None,
        }
    }
}
