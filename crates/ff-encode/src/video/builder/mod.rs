//! Video encoder builder and public API.
//!
//! This module provides [`VideoEncoderBuilder`] for fluent configuration and
//! [`VideoEncoder`] for encoding video (and optionally audio) frames.

use std::path::PathBuf;
use std::time::Instant;

use ff_format::{AudioFrame, VideoFrame};

use super::codec_options::VideoCodecOptions;
use super::encoder_inner::{VideoEncoderConfig, VideoEncoderInner, preset_to_string};
use crate::{
    AudioCodec, EncodeError, EncodeProgressCallback, HardwareEncoder, OutputContainer, Preset,
    VideoCodec,
};

mod audio;
mod color;
mod meta;
mod video;

/// Builder for constructing a [`VideoEncoder`].
///
/// Created by calling [`VideoEncoder::create()`]. Call [`build()`](Self::build)
/// to open the output file and prepare for encoding.
///
/// # Examples
///
/// ```ignore
/// use ff_encode::{VideoEncoder, VideoCodec, Preset};
///
/// let mut encoder = VideoEncoder::create(test_out("output.mp4"))
///     .video(1920, 1080, 30.0)
///     .video_codec(VideoCodec::H264)
///     .preset(Preset::Medium)
///     .build()?;
/// ```
// A builder aggregates many independent output toggles; the bool count is inherent.
#[allow(clippy::struct_excessive_bools)]
pub struct VideoEncoderBuilder {
    pub(crate) path: PathBuf,
    pub(crate) container: Option<OutputContainer>,
    pub(crate) video_width: Option<u32>,
    pub(crate) video_height: Option<u32>,
    pub(crate) video_fps: Option<f64>,
    pub(crate) video_codec: VideoCodec,
    pub(crate) video_bitrate_mode: Option<crate::BitrateMode>,
    pub(crate) preset: Preset,
    pub(crate) hardware_encoder: HardwareEncoder,
    pub(crate) audio_sample_rate: Option<u32>,
    pub(crate) audio_channels: Option<u32>,
    pub(crate) audio_codec: AudioCodec,
    pub(crate) audio_bitrate: Option<u64>,
    pub(crate) progress_callback: Option<Box<dyn EncodeProgressCallback>>,
    /// A caller-supplied byte sink to mux into instead of `path`.
    ///
    /// Set by [`VideoEncoderBuilder::output_sink`]; when present the `path` is
    /// only what the muxer is guessed from, and nothing is written to disk.
    pub(crate) sink: Option<Box<dyn ff_sys::IoSink>>,
    pub(crate) two_pass: bool,
    pub(crate) faststart: bool,
    pub(crate) metadata: Vec<(String, String)>,
    pub(crate) chapters: Vec<ff_format::chapter::ChapterInfo>,
    pub(crate) subtitle_passthrough: Option<(String, usize)>,
    pub(crate) codec_options: Option<VideoCodecOptions>,
    /// Codec-private options set by name, applied after `codec_options`.
    pub(crate) codec_opts: Vec<(String, String)>,
    pub(crate) video_codec_explicit: bool,
    pub(crate) audio_codec_explicit: bool,
    pub(crate) pixel_format: Option<ff_format::PixelFormat>,
    pub(crate) hdr10_metadata: Option<ff_format::Hdr10Metadata>,
    pub(crate) color_space: Option<ff_format::ColorSpace>,
    pub(crate) color_transfer: Option<ff_format::ColorTransfer>,
    pub(crate) color_primaries: Option<ff_format::ColorPrimaries>,
    /// Binary attachments: (raw data, MIME type, filename).
    pub(crate) attachments: Vec<(Vec<u8>, String, String)>,
}

impl std::fmt::Debug for VideoEncoderBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoEncoderBuilder")
            .field("path", &self.path)
            .field("container", &self.container)
            .field("video_width", &self.video_width)
            .field("video_height", &self.video_height)
            .field("video_fps", &self.video_fps)
            .field("video_codec", &self.video_codec)
            .field("video_bitrate_mode", &self.video_bitrate_mode)
            .field("preset", &self.preset)
            .field("hardware_encoder", &self.hardware_encoder)
            .field("audio_sample_rate", &self.audio_sample_rate)
            .field("audio_channels", &self.audio_channels)
            .field("audio_codec", &self.audio_codec)
            .field("audio_bitrate", &self.audio_bitrate)
            .field(
                "progress_callback",
                &self.progress_callback.as_ref().map(|_| "<callback>"),
            )
            .field("sink", &self.sink.as_ref().map(|_| "<sink>"))
            .field("two_pass", &self.two_pass)
            .field("faststart", &self.faststart)
            .field("metadata", &self.metadata)
            .field("chapters", &self.chapters)
            .field("subtitle_passthrough", &self.subtitle_passthrough)
            .field("codec_options", &self.codec_options)
            .field("codec_opts", &self.codec_opts)
            .field("video_codec_explicit", &self.video_codec_explicit)
            .field("audio_codec_explicit", &self.audio_codec_explicit)
            .field("pixel_format", &self.pixel_format)
            .field("hdr10_metadata", &self.hdr10_metadata)
            .field("color_space", &self.color_space)
            .field("color_transfer", &self.color_transfer)
            .field("color_primaries", &self.color_primaries)
            .field("attachments_count", &self.attachments.len())
            .finish()
    }
}

impl VideoEncoderBuilder {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            container: None,
            video_width: None,
            video_height: None,
            video_fps: None,
            video_codec: VideoCodec::default(),
            video_bitrate_mode: None,
            preset: Preset::default(),
            hardware_encoder: HardwareEncoder::default(),
            audio_sample_rate: None,
            audio_channels: None,
            audio_codec: AudioCodec::default(),
            audio_bitrate: None,
            progress_callback: None,
            sink: None,
            two_pass: false,
            faststart: false,
            metadata: Vec::new(),
            chapters: Vec::new(),
            subtitle_passthrough: None,
            codec_options: None,
            codec_opts: Vec::new(),
            video_codec_explicit: false,
            audio_codec_explicit: false,
            pixel_format: None,
            hdr10_metadata: None,
            color_space: None,
            color_transfer: None,
            color_primaries: None,
            attachments: Vec::new(),
        }
    }

    /// Validate builder state and open the output file.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] if configuration is invalid, the output path
    /// cannot be created, or no suitable encoder is found.
    pub fn build(self) -> Result<VideoEncoder, EncodeError> {
        let this = self.apply_container_defaults();
        this.validate()?;
        VideoEncoder::from_builder(this)
    }

    /// Apply container-specific codec defaults before validation.
    ///
    /// For `WebM` paths/containers, default to VP9 + Opus when the caller has
    /// not explicitly chosen a codec.
    fn apply_container_defaults(mut self) -> Self {
        let is_webm = self
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("webm"))
            || self
                .container
                .as_ref()
                .is_some_and(|c| *c == OutputContainer::WebM);

        if is_webm {
            if !self.video_codec_explicit {
                self.video_codec = VideoCodec::Vp9;
            }
            if !self.audio_codec_explicit {
                self.audio_codec = AudioCodec::Opus;
            }
        }

        let is_avi = self
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("avi"))
            || self
                .container
                .as_ref()
                .is_some_and(|c| *c == OutputContainer::Avi);

        if is_avi {
            if !self.video_codec_explicit {
                self.video_codec = VideoCodec::H264;
            }
            if !self.audio_codec_explicit {
                self.audio_codec = AudioCodec::Mp3;
            }
        }

        let is_mov = self
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("mov"))
            || self
                .container
                .as_ref()
                .is_some_and(|c| *c == OutputContainer::Mov);

        if is_mov {
            if !self.video_codec_explicit {
                self.video_codec = VideoCodec::H264;
            }
            if !self.audio_codec_explicit {
                self.audio_codec = AudioCodec::Aac;
            }
        }

        // Image-sequence paths contain '%' (e.g. "frames/frame%04d.png").
        // Auto-select codec from the extension that follows the pattern.
        let is_image_sequence = self.path.to_str().is_some_and(|s| s.contains('%'));
        if is_image_sequence && !self.video_codec_explicit {
            let ext = self
                .path
                .to_str()
                .and_then(|s| s.rfind('.').map(|i| &s[i + 1..]))
                .unwrap_or("");
            if ext.eq_ignore_ascii_case("png") {
                self.video_codec = VideoCodec::Png;
            } else if ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg") {
                self.video_codec = VideoCodec::Mjpeg;
            }
        }

        self
    }

    fn validate(&self) -> Result<(), EncodeError> {
        let has_video =
            self.video_width.is_some() && self.video_height.is_some() && self.video_fps.is_some();
        let has_audio = self.audio_sample_rate.is_some() && self.audio_channels.is_some();

        if !has_video && !has_audio {
            return Err(EncodeError::InvalidConfig {
                reason: "At least one video or audio stream must be configured".to_string(),
            });
        }

        if self.faststart && self.sink.is_some() {
            // `movflags=+faststart` finalises by *reopening the output for reading*
            // (`mov_write_trailer` -> `shift_data` -> `ff_format_shift_data`, which
            // does `io_open(s, &read_pb, s->url, AVIO_FLAG_READ, ...)`). With a sink
            // the bytes never reached `s->url`, so that read hits whatever is at the
            // path -- nothing, or an unrelated file. Measured: with no such file
            // `finish()` fails and leaves a moov-less stream in the sink; with a
            // stale file of that name `finish()` returns `Ok` and that file's
            // contents are copied into the caller's sink. Any muxer that relocates
            // data by reopening `s->url` is incompatible with a custom `pb`.
            return Err(EncodeError::InvalidConfig {
                reason: "faststart cannot write to a caller-supplied sink".to_string(),
            });
        }

        if self.two_pass {
            if self.sink.is_some() {
                // Pass 2 opens the output after pass 1 has run, so it needs an
                // output it can open twice. A sink is moved in once and cannot be
                // reopened, and half-supporting that would mean silently writing
                // only the second pass.
                return Err(EncodeError::InvalidConfig {
                    reason: "Two-pass encoding cannot write to a caller-supplied sink".to_string(),
                });
            }
            if !has_video {
                return Err(EncodeError::InvalidConfig {
                    reason: "Two-pass encoding requires a video stream".to_string(),
                });
            }
            if has_audio {
                return Err(EncodeError::InvalidConfig {
                    reason:
                        "Two-pass encoding is video-only and is incompatible with audio streams"
                            .to_string(),
                });
            }
        }

        // Image-sequence paths (containing '%') do not support audio streams.
        let is_image_sequence = self.path.to_str().is_some_and(|s| s.contains('%'));
        if is_image_sequence && has_audio {
            return Err(EncodeError::InvalidConfig {
                reason: "Image sequence output does not support audio streams".to_string(),
            });
        }

        // PNG supports odd dimensions; all other codecs require even width/height.
        let requires_even_dims = !matches!(self.video_codec, VideoCodec::Png);

        if has_video {
            // Dimension range check (2–32768 inclusive).
            let w = self.video_width.unwrap_or(0);
            let h = self.video_height.unwrap_or(0);
            if (self.video_width.is_some() || self.video_height.is_some())
                && (!(2..=32_768).contains(&w) || !(2..=32_768).contains(&h))
            {
                log::warn!(
                    "video dimensions out of range width={w} height={h} \
                     (valid range 2–32768 per axis)"
                );
                return Err(EncodeError::InvalidDimensions {
                    width: w,
                    height: h,
                });
            }

            if let Some(width) = self.video_width
                && (requires_even_dims && width % 2 != 0)
            {
                return Err(EncodeError::InvalidConfig {
                    reason: format!("Video width must be even, got {width}"),
                });
            }
            if let Some(height) = self.video_height
                && (requires_even_dims && height % 2 != 0)
            {
                return Err(EncodeError::InvalidConfig {
                    reason: format!("Video height must be even, got {height}"),
                });
            }
            if let Some(fps) = self.video_fps
                && fps <= 0.0
            {
                return Err(EncodeError::InvalidConfig {
                    reason: format!("Video FPS must be positive, got {fps}"),
                });
            }
            if let Some(fps) = self.video_fps
                && fps > 1000.0
            {
                log::warn!("video fps exceeds maximum fps={fps} (maximum 1000)");
                return Err(EncodeError::InvalidConfig {
                    reason: format!("fps {fps} exceeds maximum 1000"),
                });
            }
            if let Some(crate::BitrateMode::Crf(q)) = self.video_bitrate_mode
                && q > crate::CRF_MAX
            {
                return Err(EncodeError::InvalidConfig {
                    reason: format!(
                        "BitrateMode::Crf value must be 0-{}, got {q}",
                        crate::CRF_MAX
                    ),
                });
            }
            if let Some(crate::BitrateMode::Vbr { target, max }) = self.video_bitrate_mode
                && max < target
            {
                return Err(EncodeError::InvalidConfig {
                    reason: format!("BitrateMode::Vbr max ({max}) must be >= target ({target})"),
                });
            }

            // Bitrate ceiling: 800 Mbps (800_000_000 bps).
            let effective_bitrate: Option<u64> = match self.video_bitrate_mode {
                Some(crate::BitrateMode::Cbr(bps)) => Some(bps),
                Some(crate::BitrateMode::Vbr { max, .. }) => Some(max),
                _ => None,
            };
            if let Some(bps) = effective_bitrate
                && bps > 800_000_000
            {
                log::warn!("video bitrate exceeds maximum bitrate={bps} maximum=800000000");
                return Err(EncodeError::InvalidBitrate { bitrate: bps });
            }
        }

        if let Some(VideoCodecOptions::Av1(ref opts)) = self.codec_options
            && opts.cpu_used > 8
        {
            return Err(EncodeError::InvalidOption {
                name: "cpu_used".to_string(),
                reason: "must be 0–8".to_string(),
            });
        }

        if let Some(VideoCodecOptions::Av1Svt(ref opts)) = self.codec_options
            && opts.preset > 13
        {
            return Err(EncodeError::InvalidOption {
                name: "preset".to_string(),
                reason: "must be 0–13".to_string(),
            });
        }

        if let Some(VideoCodecOptions::Vp9(ref opts)) = self.codec_options {
            if opts.cpu_used < -8 || opts.cpu_used > 8 {
                return Err(EncodeError::InvalidOption {
                    name: "cpu_used".to_string(),
                    reason: "must be -8–8".to_string(),
                });
            }
            if let Some(cq) = opts.cq_level
                && cq > 63
            {
                return Err(EncodeError::InvalidOption {
                    name: "cq_level".to_string(),
                    reason: "must be 0–63".to_string(),
                });
            }
        }

        if let Some(VideoCodecOptions::Dnxhd(ref opts)) = self.codec_options
            && opts.variant.is_dnxhd()
        {
            let valid = matches!(
                (self.video_width, self.video_height),
                (Some(1920), Some(1080)) | (Some(1280), Some(720))
            );
            if !valid {
                return Err(EncodeError::InvalidOption {
                    name: "variant".to_string(),
                    reason: "DNxHD variants require 1920×1080 or 1280×720 resolution".to_string(),
                });
            }
        }

        // WebM container codec enforcement.
        let is_webm = self
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("webm"))
            || self
                .container
                .as_ref()
                .is_some_and(|c| *c == OutputContainer::WebM);

        if is_webm {
            let webm_video_ok = matches!(
                self.video_codec,
                VideoCodec::Vp9 | VideoCodec::Av1 | VideoCodec::Av1Svt
            );
            if !webm_video_ok {
                return Err(EncodeError::UnsupportedContainerCodecCombination {
                    container: "webm".to_string(),
                    codec: self.video_codec.name().to_string(),
                    hint: "WebM supports VP9, AV1 (video) and Vorbis, Opus (audio)".to_string(),
                });
            }

            let webm_audio_ok = matches!(self.audio_codec, AudioCodec::Opus | AudioCodec::Vorbis);
            if !webm_audio_ok {
                return Err(EncodeError::UnsupportedContainerCodecCombination {
                    container: "webm".to_string(),
                    codec: self.audio_codec.name().to_string(),
                    hint: "WebM supports VP9, AV1 (video) and Vorbis, Opus (audio)".to_string(),
                });
            }
        }

        // AVI container codec enforcement.
        let is_avi = self
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("avi"))
            || self
                .container
                .as_ref()
                .is_some_and(|c| *c == OutputContainer::Avi);

        if is_avi {
            let avi_video_ok = matches!(self.video_codec, VideoCodec::H264 | VideoCodec::Mpeg4);
            if !avi_video_ok {
                return Err(EncodeError::UnsupportedContainerCodecCombination {
                    container: "avi".to_string(),
                    codec: self.video_codec.name().to_string(),
                    hint: "AVI supports H264 and MPEG-4 (video); MP3, AAC, and PCM 16-bit (audio)"
                        .to_string(),
                });
            }

            let avi_audio_ok = matches!(
                self.audio_codec,
                AudioCodec::Mp3 | AudioCodec::Aac | AudioCodec::Pcm | AudioCodec::Pcm16
            );
            if !avi_audio_ok {
                return Err(EncodeError::UnsupportedContainerCodecCombination {
                    container: "avi".to_string(),
                    codec: self.audio_codec.name().to_string(),
                    hint: "AVI supports H264 and MPEG-4 (video); MP3, AAC, and PCM 16-bit (audio)"
                        .to_string(),
                });
            }
        }

        // MOV container codec enforcement.
        let is_mov = self
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("mov"))
            || self
                .container
                .as_ref()
                .is_some_and(|c| *c == OutputContainer::Mov);

        if is_mov {
            let mov_video_ok = matches!(
                self.video_codec,
                VideoCodec::H264 | VideoCodec::H265 | VideoCodec::ProRes
            );
            if !mov_video_ok {
                return Err(EncodeError::UnsupportedContainerCodecCombination {
                    container: "mov".to_string(),
                    codec: self.video_codec.name().to_string(),
                    hint: "MOV supports H264, H265, and ProRes (video); AAC and PCM (audio)"
                        .to_string(),
                });
            }

            let mov_audio_ok = matches!(
                self.audio_codec,
                AudioCodec::Aac | AudioCodec::Pcm | AudioCodec::Pcm16 | AudioCodec::Pcm24
            );
            if !mov_audio_ok {
                return Err(EncodeError::UnsupportedContainerCodecCombination {
                    container: "mov".to_string(),
                    codec: self.audio_codec.name().to_string(),
                    hint: "MOV supports H264, H265, and ProRes (video); AAC and PCM (audio)"
                        .to_string(),
                });
            }
        }

        // fMP4 container codec enforcement.
        let is_fmp4 = self
            .container
            .as_ref()
            .is_some_and(|c| *c == OutputContainer::FMp4);

        if is_fmp4 {
            let fmp4_video_ok = !matches!(
                self.video_codec,
                VideoCodec::Mpeg2 | VideoCodec::Mpeg4 | VideoCodec::Mjpeg
            );
            if !fmp4_video_ok {
                return Err(EncodeError::UnsupportedContainerCodecCombination {
                    container: "fMP4".to_string(),
                    codec: self.video_codec.name().to_string(),
                    hint: "fMP4 supports H.264, H.265, VP9, AV1".to_string(),
                });
            }
        }

        if has_audio {
            if let Some(rate) = self.audio_sample_rate
                && rate == 0
            {
                return Err(EncodeError::InvalidConfig {
                    reason: "Audio sample rate must be non-zero".to_string(),
                });
            }
            if let Some(ch) = self.audio_channels
                && ch == 0
            {
                return Err(EncodeError::InvalidConfig {
                    reason: "Audio channels must be non-zero".to_string(),
                });
            }
        }

        Ok(())
    }
}

/// Encodes video (and optionally audio) frames to a file using `FFmpeg`.
///
/// # Construction
///
/// Use [`VideoEncoder::create()`] to get a [`VideoEncoderBuilder`], then call
/// [`VideoEncoderBuilder::build()`]:
///
/// ```ignore
/// use ff_encode::{VideoEncoder, VideoCodec};
///
/// let mut encoder = VideoEncoder::create(test_out("output.mp4"))
///     .video(1920, 1080, 30.0)
///     .video_codec(VideoCodec::H264)
///     .build()?;
/// ```
pub struct VideoEncoder {
    inner: Option<VideoEncoderInner>,
    _config: VideoEncoderConfig,
    start_time: Instant,
    progress_callback: Option<Box<dyn crate::EncodeProgressCallback>>,
}

impl VideoEncoder {
    /// Creates a builder for the specified output file path.
    ///
    /// This method is infallible. Validation occurs when
    /// [`VideoEncoderBuilder::build()`] is called.
    pub fn create<P: AsRef<std::path::Path>>(path: P) -> VideoEncoderBuilder {
        VideoEncoderBuilder::new(path.as_ref().to_path_buf())
    }

    pub(crate) fn from_builder(builder: VideoEncoderBuilder) -> Result<Self, EncodeError> {
        let config = VideoEncoderConfig {
            path: builder.path.clone(),
            video_width: builder.video_width,
            video_height: builder.video_height,
            video_fps: builder.video_fps,
            video_codec: builder.video_codec,
            video_bitrate_mode: builder.video_bitrate_mode,
            preset: preset_to_string(builder.preset),
            hardware_encoder: builder.hardware_encoder,
            audio_sample_rate: builder.audio_sample_rate,
            audio_channels: builder.audio_channels,
            audio_codec: builder.audio_codec,
            audio_bitrate: builder.audio_bitrate,
            _progress_callback: builder.progress_callback.is_some(),
            two_pass: builder.two_pass,
            metadata: builder.metadata,
            chapters: builder.chapters,
            subtitle_passthrough: builder.subtitle_passthrough,
            codec_options: builder.codec_options,
            codec_opts: builder.codec_opts,
            pixel_format: builder.pixel_format,
            hdr10_metadata: builder.hdr10_metadata,
            color_space: builder.color_space,
            color_transfer: builder.color_transfer,
            color_primaries: builder.color_primaries,
            attachments: builder.attachments,
            container: builder.container,
            faststart: builder.faststart,
        };

        // Create the inner encoder when at least one of video or audio is
        // configured.  `video_width.is_some()` alone is not sufficient:
        // audio-only presets (e.g. podcast_mono) set audio fields but no video
        // dimensions, so we must also check for audio configuration.
        let has_audio = config.audio_sample_rate.is_some() && config.audio_channels.is_some();
        let inner = if config.video_width.is_some() || has_audio {
            Some(VideoEncoderInner::new(&config, builder.sink)?)
        } else {
            None
        };

        Ok(Self {
            inner,
            _config: config,
            start_time: Instant::now(),
            progress_callback: builder.progress_callback,
        })
    }

    /// Returns the name of the `FFmpeg` encoder actually used (e.g. `"h264_nvenc"`, `"libx264"`).
    #[must_use]
    pub fn actual_video_codec(&self) -> &str {
        self.inner
            .as_ref()
            .map_or("", |inner| inner.actual_video_codec.as_str())
    }

    /// Returns the name of the `FFmpeg` audio encoder actually used.
    #[must_use]
    pub fn actual_audio_codec(&self) -> &str {
        self.inner
            .as_ref()
            .map_or("", |inner| inner.actual_audio_codec.as_str())
    }

    /// Returns the hardware encoder actually in use.
    #[must_use]
    pub fn hardware_encoder(&self) -> crate::HardwareEncoder {
        let codec_name = self.actual_video_codec();
        if codec_name.contains("nvenc") {
            crate::HardwareEncoder::Nvenc
        } else if codec_name.contains("qsv") {
            crate::HardwareEncoder::Qsv
        } else if codec_name.contains("amf") {
            crate::HardwareEncoder::Amf
        } else if codec_name.contains("videotoolbox") {
            crate::HardwareEncoder::VideoToolbox
        } else if codec_name.contains("vaapi") {
            crate::HardwareEncoder::Vaapi
        } else {
            crate::HardwareEncoder::None
        }
    }

    /// Returns `true` if a hardware encoder is active.
    #[must_use]
    pub fn is_hardware_encoding(&self) -> bool {
        !matches!(self.hardware_encoder(), crate::HardwareEncoder::None)
    }

    /// Returns `true` if the selected encoder is LGPL-compatible (safe for commercial use).
    #[must_use]
    pub fn is_lgpl_compliant(&self) -> bool {
        let codec_name = self.actual_video_codec();
        if codec_name.contains("nvenc")
            || codec_name.contains("qsv")
            || codec_name.contains("amf")
            || codec_name.contains("videotoolbox")
            || codec_name.contains("vaapi")
        {
            return true;
        }
        if codec_name.contains("vp9")
            || codec_name.contains("av1")
            || codec_name.contains("aom")
            || codec_name.contains("svt")
            || codec_name.contains("prores")
            || codec_name == "mpeg4"
            || codec_name == "dnxhd"
        {
            return true;
        }
        if codec_name == "libx264" || codec_name == "libx265" {
            return false;
        }
        true
    }

    /// Pushes a video frame for encoding.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] if encoding fails or the encoder is not initialised.
    /// Returns [`EncodeError::Cancelled`] if the progress callback requested cancellation.
    pub fn push_video(&mut self, frame: &VideoFrame) -> Result<(), EncodeError> {
        if let Some(ref callback) = self.progress_callback
            && callback.should_cancel()
        {
            return Err(EncodeError::Cancelled);
        }
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Video encoder not initialized".to_string(),
            })?;
        inner.push_video_frame(frame)?;
        let progress = self.create_progress_info();
        if let Some(ref mut callback) = self.progress_callback {
            callback.on_progress(&progress);
        }
        Ok(())
    }

    /// Pushes an audio frame for encoding.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] if encoding fails or the encoder is not initialised.
    pub fn push_audio(&mut self, frame: &AudioFrame) -> Result<(), EncodeError> {
        if let Some(ref callback) = self.progress_callback
            && callback.should_cancel()
        {
            return Err(EncodeError::Cancelled);
        }
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| EncodeError::InvalidConfig {
                reason: "Audio encoder not initialized".to_string(),
            })?;
        inner.push_audio_frame(frame)?;
        let progress = self.create_progress_info();
        if let Some(ref mut callback) = self.progress_callback {
            callback.on_progress(&progress);
        }
        Ok(())
    }

    /// Flushes remaining frames and writes the file trailer.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] if finalising fails.
    pub fn finish(mut self) -> Result<(), EncodeError> {
        if let Some(mut inner) = self.inner.take() {
            inner.finish()?;
        }
        Ok(())
    }

    fn create_progress_info(&self) -> crate::EncodeProgress {
        let elapsed = self.start_time.elapsed();
        let (frames_encoded, bytes_written) = self
            .inner
            .as_ref()
            .map_or((0, 0), |inner| (inner.frame_count, inner.bytes_written));
        #[allow(clippy::cast_precision_loss)]
        let current_fps = if !elapsed.is_zero() {
            frames_encoded as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };
        #[allow(clippy::cast_precision_loss)]
        // Bitrate is a non-negative bits-per-second rate; the f64→u64 fallback
        // (used only when the integer division would divide by zero) cannot wrap
        // or lose a sign in practice.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let current_bitrate = if !elapsed.is_zero() {
            let elapsed_secs = elapsed.as_secs();
            match (bytes_written * 8).checked_div(elapsed_secs) {
                Some(bitrate) => bitrate,
                None => ((bytes_written * 8) as f64 / elapsed.as_secs_f64()) as u64,
            }
        } else {
            0
        };
        crate::EncodeProgress {
            frames_encoded,
            total_frames: None,
            bytes_written,
            current_bitrate,
            elapsed,
            remaining: None,
            current_fps,
        }
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        // VideoEncoderInner handles cleanup in its own Drop.
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn codec_opt_should_collect_pairs_in_order() {
        // Order is preserved because it decides the outcome for a repeated key,
        // and because comma-joined values like `x264-params` are position
        // sensitive.
        let builder = VideoEncoder::create("out.mp4")
            .codec_opt("x264-params", "keyint=48")
            .codec_opt("aq-mode", "2")
            .codec_opt("x264-params", "keyint=24");
        assert_eq!(
            builder.codec_opts,
            vec![
                ("x264-params".to_string(), "keyint=48".to_string()),
                ("aq-mode".to_string(), "2".to_string()),
                ("x264-params".to_string(), "keyint=24".to_string()),
            ],
            "pairs must be kept in call order, duplicates included"
        );
    }

    #[test]
    fn codec_opt_should_appear_in_the_builder_debug() {
        // `VideoEncoderBuilder` writes its `Debug` by hand, so a new field is
        // silently missing from debug output unless it is added there too.
        let builder = VideoEncoder::create("out.mp4").codec_opt("aq-mode", "2");
        let rendered = format!("{builder:?}");
        assert!(
            rendered.contains("codec_opts"),
            "the field must be listed in the manual Debug impl; got {rendered}"
        );
        assert!(
            rendered.contains("aq-mode"),
            "the value must be rendered, not just the field name; got {rendered}"
        );
    }

    use super::super::encoder_inner::{VideoEncoderConfig, VideoEncoderInner};
    use super::*;
    use crate::HardwareEncoder;

    /// Returns a path inside `target/test-output/` so that any files created
    /// by builder unit tests do not litter the crate root.
    fn test_out(name: &str) -> String {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-output");
        std::fs::create_dir_all(&dir).ok();
        dir.join(name).to_string_lossy().into_owned()
    }

    fn create_mock_encoder(video_codec_name: &str, audio_codec_name: &str) -> VideoEncoder {
        VideoEncoder {
            inner: Some(VideoEncoderInner {
                // A real (unopened) mux context; the mock never muxes and drops it untouched.
                format_ctx: ff_sys::OutputFormatContext::new(
                    None,
                    std::path::Path::new("mock.mp4"),
                )
                .expect("mock output context allocation should succeed"),
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
                actual_video_codec: video_codec_name.to_string(),
                actual_audio_codec: audio_codec_name.to_string(),
                last_src_width: None,
                last_src_height: None,
                last_src_format: None,
                two_pass: false,
                pass1_codec_ctx: None,
                buffered_frames: Vec::new(),
                two_pass_config: None,
                subtitle_passthrough: None,
                hdr10_metadata: None,
            }),
            _config: VideoEncoderConfig {
                path: "test.mp4".into(),
                video_width: Some(1920),
                video_height: Some(1080),
                video_fps: Some(30.0),
                video_codec: crate::VideoCodec::H264,
                video_bitrate_mode: None,
                preset: "medium".to_string(),
                hardware_encoder: HardwareEncoder::Auto,
                audio_sample_rate: None,
                audio_channels: None,
                audio_codec: crate::AudioCodec::Aac,
                audio_bitrate: None,
                _progress_callback: false,
                two_pass: false,
                metadata: Vec::new(),
                chapters: Vec::new(),
                subtitle_passthrough: None,
                codec_options: None,
                codec_opts: Vec::new(),
                pixel_format: None,
                hdr10_metadata: None,
                color_space: None,
                color_transfer: None,
                color_primaries: None,
                attachments: Vec::new(),
                container: None,
                faststart: false,
            },
            start_time: std::time::Instant::now(),
            progress_callback: None,
        }
    }

    #[test]
    fn create_should_return_builder_without_error() {
        let _builder: VideoEncoderBuilder = VideoEncoder::create(test_out("output.mp4"));
    }

    #[test]
    fn build_without_streams_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mp4")).build();
        assert!(result.is_err());
    }

    #[test]
    fn build_with_odd_width_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mp4"))
            .video(1921, 1080, 30.0)
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_with_odd_height_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mp4"))
            .video(1920, 1081, 30.0)
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_with_invalid_fps_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mp4"))
            .video(1920, 1080, -1.0)
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn two_pass_with_audio_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mp4"))
            .video(640, 480, 30.0)
            .audio(48000, 2)
            .two_pass()
            .build();
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(
                matches!(e, crate::EncodeError::InvalidConfig { .. }),
                "expected InvalidConfig, got {e:?}"
            );
        }
    }

    #[test]
    fn two_pass_without_video_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mp4"))
            .two_pass()
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_with_crf_above_51_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mp4"))
            .video(1920, 1080, 30.0)
            .bitrate_mode(crate::BitrateMode::Crf(100))
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn bitrate_mode_vbr_with_max_less_than_target_should_return_error() {
        let result = VideoEncoder::create(test_out("test_vbr.mp4"))
            .video(640, 480, 30.0)
            .bitrate_mode(crate::BitrateMode::Vbr {
                target: 4_000_000,
                max: 2_000_000,
            })
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn is_lgpl_compliant_should_be_true_for_hardware_encoders() {
        for codec_name in &[
            "h264_nvenc",
            "h264_qsv",
            "h264_amf",
            "h264_videotoolbox",
            "hevc_vaapi",
        ] {
            let encoder = create_mock_encoder(codec_name, "");
            assert!(
                encoder.is_lgpl_compliant(),
                "expected LGPL-compliant for {codec_name}"
            );
        }
    }

    #[test]
    fn is_lgpl_compliant_should_be_false_for_gpl_encoders() {
        for codec_name in &["libx264", "libx265"] {
            let encoder = create_mock_encoder(codec_name, "");
            assert!(
                !encoder.is_lgpl_compliant(),
                "expected non-LGPL for {codec_name}"
            );
        }
    }

    #[test]
    fn hardware_encoder_detection_should_match_codec_name() {
        let cases: &[(&str, HardwareEncoder, bool)] = &[
            ("h264_nvenc", HardwareEncoder::Nvenc, true),
            ("h264_qsv", HardwareEncoder::Qsv, true),
            ("h264_amf", HardwareEncoder::Amf, true),
            ("h264_videotoolbox", HardwareEncoder::VideoToolbox, true),
            ("h264_vaapi", HardwareEncoder::Vaapi, true),
            ("libx264", HardwareEncoder::None, false),
            ("libvpx-vp9", HardwareEncoder::None, false),
        ];
        for (codec_name, expected_hw, expected_is_hw) in cases {
            let encoder = create_mock_encoder(codec_name, "");
            assert_eq!(
                encoder.hardware_encoder(),
                *expected_hw,
                "hw for {codec_name}"
            );
            assert_eq!(
                encoder.is_hardware_encoding(),
                *expected_is_hw,
                "is_hw for {codec_name}"
            );
        }
    }

    #[test]
    fn webm_extension_without_explicit_codec_should_default_to_vp9_opus() {
        let builder = VideoEncoder::create(test_out("output.webm")).video(640, 480, 30.0);
        let normalized = builder.apply_container_defaults();
        assert_eq!(normalized.video_codec, VideoCodec::Vp9);
        assert_eq!(normalized.audio_codec, AudioCodec::Opus);
    }

    #[test]
    fn webm_extension_with_explicit_vp9_should_preserve_codec() {
        let builder = VideoEncoder::create(test_out("output.webm"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::Vp9);
        assert!(builder.video_codec_explicit);
        let normalized = builder.apply_container_defaults();
        assert_eq!(normalized.video_codec, VideoCodec::Vp9);
    }

    #[test]
    fn avi_extension_without_explicit_codec_should_default_to_h264_mp3() {
        let builder = VideoEncoder::create(test_out("output.avi")).video(640, 480, 30.0);
        let normalized = builder.apply_container_defaults();
        assert_eq!(normalized.video_codec, VideoCodec::H264);
        assert_eq!(normalized.audio_codec, AudioCodec::Mp3);
    }

    #[test]
    fn mov_extension_without_explicit_codec_should_default_to_h264_aac() {
        let builder = VideoEncoder::create(test_out("output.mov")).video(640, 480, 30.0);
        let normalized = builder.apply_container_defaults();
        assert_eq!(normalized.video_codec, VideoCodec::H264);
        assert_eq!(normalized.audio_codec, AudioCodec::Aac);
    }

    #[test]
    fn webm_extension_with_h264_video_codec_should_return_error() {
        let result = VideoEncoder::create(test_out("output.webm"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::H264)
            .build();
        assert!(matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn webm_extension_with_h265_video_codec_should_return_error() {
        let result = VideoEncoder::create(test_out("output.webm"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::H265)
            .build();
        assert!(matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn webm_extension_with_incompatible_audio_codec_should_return_error() {
        let result = VideoEncoder::create(test_out("output.webm"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::Vp9)
            .audio(48000, 2)
            .audio_codec(AudioCodec::Aac)
            .build();
        assert!(matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn webm_container_enum_with_incompatible_codec_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mkv"))
            .video(640, 480, 30.0)
            .container(OutputContainer::WebM)
            .video_codec(VideoCodec::H264)
            .build();
        assert!(matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn non_webm_extension_should_not_enforce_webm_codecs() {
        // H264 + AAC on .mp4 should not trigger WebM validation
        let result = VideoEncoder::create(test_out("output.mp4"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::H264)
            .build();
        // Should not fail with UnsupportedContainerCodecCombination
        assert!(!matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn avi_with_incompatible_video_codec_should_return_error() {
        let result = VideoEncoder::create(test_out("output.avi"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::Vp9)
            .build();
        assert!(matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn avi_with_incompatible_audio_codec_should_return_error() {
        let result = VideoEncoder::create(test_out("output.avi"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::H264)
            .audio(48000, 2)
            .audio_codec(AudioCodec::Opus)
            .build();
        assert!(matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn mov_with_incompatible_video_codec_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mov"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::Vp9)
            .build();
        assert!(matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn mov_with_incompatible_audio_codec_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mov"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::H264)
            .audio(48000, 2)
            .audio_codec(AudioCodec::Opus)
            .build();
        assert!(matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn avi_container_enum_with_incompatible_codec_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mp4"))
            .video(640, 480, 30.0)
            .container(OutputContainer::Avi)
            .video_codec(VideoCodec::Vp9)
            .build();
        assert!(matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn mov_container_enum_with_incompatible_codec_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mp4"))
            .video(640, 480, 30.0)
            .container(OutputContainer::Mov)
            .video_codec(VideoCodec::Vp9)
            .build();
        assert!(matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn avi_with_pcm_audio_should_pass_validation() {
        // AudioCodec::Pcm (backward-compat alias for 16-bit PCM) must be accepted in AVI.
        let result = VideoEncoder::create(test_out("output.avi"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::H264)
            .audio(48000, 2)
            .audio_codec(AudioCodec::Pcm)
            .build();
        assert!(!matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn mov_with_pcm24_audio_should_pass_validation() {
        let result = VideoEncoder::create(test_out("output.mov"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::H264)
            .audio(48000, 2)
            .audio_codec(AudioCodec::Pcm24)
            .build();
        assert!(!matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn non_avi_mov_extension_should_not_enforce_avi_mov_codecs() {
        // Vp9 on .webm should not trigger AVI/MOV validation
        let result = VideoEncoder::create(test_out("output.webm"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::Vp9)
            .build();
        assert!(!matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination {
                ref container, ..
            }) if container == "avi" || container == "mov"
        ));
    }

    #[test]
    fn fmp4_container_with_h264_should_pass_validation() {
        let result = VideoEncoder::create(test_out("output.mp4"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::H264)
            .container(OutputContainer::FMp4)
            .build();
        assert!(!matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination { .. })
        ));
    }

    #[test]
    fn fmp4_container_with_mpeg4_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mp4"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::Mpeg4)
            .container(OutputContainer::FMp4)
            .build();
        assert!(matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination {
                ref container, ..
            }) if container == "fMP4"
        ));
    }

    #[test]
    fn fmp4_container_with_mjpeg_should_return_error() {
        let result = VideoEncoder::create(test_out("output.mp4"))
            .video(640, 480, 30.0)
            .video_codec(VideoCodec::Mjpeg)
            .container(OutputContainer::FMp4)
            .build();
        assert!(matches!(
            result,
            Err(crate::EncodeError::UnsupportedContainerCodecCombination {
                ref container, ..
            }) if container == "fMP4"
        ));
    }
}
