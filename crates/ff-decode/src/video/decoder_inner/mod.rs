//! Internal video decoder implementation using FFmpeg.
//!
//! This module contains the low-level decoder logic that directly interacts
//! with FFmpeg's C API through the ff-sys crate. It is not exposed publicly.

// Allow unsafe code in this module as it's necessary for FFmpeg FFI
#![allow(unsafe_code)]
// Allow specific clippy lints for FFmpeg FFI code
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::unnecessary_cast)]
#![allow(clippy::if_not_else)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::cast_lossless)]

use std::ffi::CStr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ff_format::NetworkOptions;

use ff_format::PooledBuffer;
use ff_format::codec::VideoCodec;
use ff_format::color::{ColorPrimaries, ColorRange, ColorSpace};
use ff_format::container::ContainerInfo;
use ff_format::time::{Rational, Timestamp};
use ff_format::{PixelFormat, VideoFrame, VideoStreamInfo};
use ff_sys::{
    AVCodecID, AVColorPrimaries, AVColorRange, AVColorSpace, AVHWDeviceType,
    AVMediaType_AVMEDIA_TYPE_VIDEO, AVPixelFormat, Frame, HwDeviceContext, InputFormatContext,
    Packet,
};

use crate::HardwareAccel;
use crate::error::DecodeError;
use crate::shared::guards_inner::{open_image_sequence_ctx, open_input_ctx, open_url_ctx};
use crate::video::builder::OutputScale;
use ff_common::FramePool;

/// Tolerance in seconds for keyframe/backward seek modes.
///
/// When seeking in Keyframe or Backward mode, frames are skipped until we're within
/// this tolerance of the target position. This balances accuracy with performance for
/// typical GOP sizes (1-2 seconds).
const KEYFRAME_SEEK_TOLERANCE_SECS: u64 = 1;

mod context;
mod decoding;
mod format_convert;
mod hardware;
mod seeking;

/// Internal decoder state holding FFmpeg contexts.
///
/// This structure manages the lifecycle of FFmpeg objects and is responsible
/// for proper cleanup when dropped.
pub(crate) struct VideoDecoderInner {
    /// Format context for reading the media file
    pub(super) format_ctx: InputFormatContext,
    /// Codec context for decoding video frames
    pub(super) codec_ctx: ff_sys::CodecContext,
    /// Video stream index in the format context
    pub(super) stream_index: i32,
    /// SwScale context for pixel format conversion and/or scaling (optional)
    pub(super) sws_ctx: Option<ff_sys::ScaleContext>,
    /// Cache key for the main sws_ctx: (src_w, src_h, src_fmt, dst_w, dst_h, dst_fmt)
    pub(super) sws_cache_key: Option<(u32, u32, i32, u32, u32, i32)>,
    /// Target output pixel format (if conversion is needed)
    pub(super) output_format: Option<PixelFormat>,
    /// Requested output scale (if resizing is needed)
    pub(super) output_scale: Option<OutputScale>,
    /// Whether the source is a live/streaming input (seeking is not supported)
    pub(super) is_live: bool,
    /// Whether end of file has been reached
    pub(super) eof: bool,
    /// Current playback position
    pub(super) position: Duration,
    /// Reusable packet for reading from file
    pub(super) packet: Packet,
    /// Reusable frame for decoding
    pub(super) frame: Frame,
    /// Cached SwScale context for thumbnail generation
    pub(super) thumbnail_sws_ctx: Option<ff_sys::ScaleContext>,
    /// Last thumbnail dimensions (for cache invalidation)
    pub(super) thumbnail_cache_key: Option<(u32, u32, u32, u32, AVPixelFormat)>,
    /// Owned hardware device reference kept alive for as long as the codec
    /// context uses it. Held only for its `Drop` (the codec keeps its own
    /// reference), so it is never read after construction.
    #[expect(dead_code, reason = "RAII drop-guard: released when the decoder drops")]
    pub(super) hw_device_ctx: Option<HwDeviceContext>,
    /// Active hardware acceleration mode
    pub(super) active_hw_accel: HardwareAccel,
    /// Optional frame pool for memory reuse
    pub(super) frame_pool: Option<Arc<dyn FramePool>>,
    /// URL used to open this source — `None` for file-path and image-sequence sources.
    pub(super) url: Option<String>,
    /// Network options used for the initial open (timeouts, reconnect config).
    pub(super) network_opts: NetworkOptions,
    /// Number of successful reconnects so far (for logging).
    pub(super) reconnect_count: u32,
    /// Number of consecutive `AVERROR_INVALIDDATA` packets skipped without a successful frame.
    /// Reset to 0 on each successfully decoded frame.
    pub(super) consecutive_invalid: u32,
}

impl VideoDecoderInner {
    /// Opens a media file and initializes the decoder.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the media file
    /// * `output_format` - Optional target pixel format for conversion
    /// * `hardware_accel` - Hardware acceleration mode
    /// * `thread_count` - Number of decoding threads (0 = auto)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The file cannot be opened
    /// - No video stream is found
    /// - The codec is not supported
    /// - Decoder initialization fails
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        path: &Path,
        output_format: Option<PixelFormat>,
        output_scale: Option<OutputScale>,
        hardware_accel: HardwareAccel,
        thread_count: usize,
        frame_rate: Option<u32>,
        frame_pool: Option<Arc<dyn FramePool>>,
        network_opts: Option<NetworkOptions>,
    ) -> Result<(Self, VideoStreamInfo, ContainerInfo), DecodeError> {
        // Ensure FFmpeg is initialized (thread-safe and idempotent)
        ff_sys::ensure_initialized();

        let path_str = path.to_str().unwrap_or("");
        let is_image_sequence = path_str.contains('%');
        let is_network_url = crate::network::is_url(path_str);

        let url = if is_network_url {
            Some(path_str.to_owned())
        } else {
            None
        };
        let stored_network_opts = network_opts.clone().unwrap_or_default();

        // Verify SRT availability before attempting to open (feature + runtime check).
        if is_network_url {
            crate::network::check_srt_url(path_str)?;
        }

        // Open the input (owned demux context).
        let mut ctx = if is_network_url {
            let network = network_opts.unwrap_or_default();
            log::info!(
                "opening network source url={} connect_timeout_ms={} read_timeout_ms={}",
                crate::network::sanitize_url(path_str),
                network.connect_timeout.as_millis(),
                network.read_timeout.as_millis(),
            );
            open_url_ctx(path_str, &network)?
        } else if is_image_sequence {
            let fps = frame_rate.unwrap_or(25);
            open_image_sequence_ctx(path, fps)?
        } else {
            open_input_ctx(path)?
        };

        // Read stream information
        ctx.find_stream_info().map_err(|e| DecodeError::Ffmpeg {
            code: e.code(),
            message: format!(
                "Failed to find stream info: {}",
                ff_sys::av_error_string(e.code())
            ),
        })?;

        // Detect live/streaming source via the AVFMT_TS_DISCONT flag on AVInputFormat.
        let is_live = (ctx.iformat_flags() & ff_sys::AVFMT_TS_DISCONT) != 0;

        // Find the video stream
        let (stream_index, codec_id) =
            Self::find_video_stream(&ctx).ok_or_else(|| DecodeError::NoVideoStream {
                path: path.to_path_buf(),
            })?;

        // Find the decoder for this codec
        // SAFETY: codec_id is valid from FFmpeg
        let codec_name = unsafe { Self::extract_codec_name(codec_id) };
        let codec = ff_sys::Codec::find_decoder(codec_id).ok_or_else(|| {
            // Distinguish between a totally unknown codec ID and a known codec
            // whose decoder was not compiled into this FFmpeg build.
            if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_EXR {
                DecodeError::DecoderUnavailable {
                    codec: "exr".to_string(),
                    hint: "Requires FFmpeg built with EXR support \
                           (--enable-decoder=exr)"
                        .to_string(),
                }
            } else {
                DecodeError::UnsupportedCodec {
                    codec: format!("{codec_name} (codec_id={codec_id:?})"),
                }
            }
        })?;

        // Allocate codec context (freed on drop by CodecContext).
        let mut codec_ctx =
            ff_sys::CodecContext::new(Some(codec)).map_err(|e| DecodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Failed to allocate codec context: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

        // Copy codec parameters from stream to context
        let codecpar = ctx
            .stream(stream_index)
            .ok_or_else(|| DecodeError::NoVideoStream {
                path: path.to_path_buf(),
            })?
            .codecpar();
        codec_ctx
            .apply_parameters(&codecpar)
            .map_err(|e| DecodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Failed to copy codec parameters: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

        // Set thread count
        if thread_count > 0 {
            codec_ctx.set_thread_count(thread_count as i32);
        }

        // Initialize hardware acceleration if requested. The owned
        // `HwDeviceContext` (when Some) holds our reference to the hw device; the
        // codec context takes its own via `set_hw_device_ctx`, so both are freed
        // independently on drop.
        let (hw_device_ctx, active_hw_accel) =
            Self::init_hardware_accel(&mut codec_ctx, hardware_accel)?;

        // Open the codec. On failure, `hw_device_ctx` (owned) drops with the rest
        // of this constructor's locals, releasing our reference; `codec_ctx` drops
        // too, releasing its own — no manual cleanup needed.
        codec_ctx
            .open_codec(codec)
            .map_err(|e| DecodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Failed to open codec: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

        // Extract stream and container information through the borrowed
        // stream / codec-context accessors.
        let duration_val = ctx.duration();
        let stream = ctx
            .stream(stream_index)
            .ok_or_else(|| DecodeError::NoVideoStream {
                path: path.to_path_buf(),
            })?;
        let stream_info = Self::extract_stream_info(stream, &codec_ctx, duration_val)?;

        // Extract container information
        let container_info = Self::extract_container_info(&ctx);

        // Allocate packet and frame (owned; free on drop, including on an early
        // return from a later `?` in this constructor).
        let packet = Packet::new().map_err(|e| DecodeError::Ffmpeg {
            code: e.code(),
            message: format!(
                "Failed to allocate packet: {}",
                ff_sys::av_error_string(e.code())
            ),
        })?;
        let frame = Frame::new().map_err(|e| DecodeError::Ffmpeg {
            code: e.code(),
            message: format!(
                "Failed to allocate frame: {}",
                ff_sys::av_error_string(e.code())
            ),
        })?;

        // All initialization successful - transfer ownership to VideoDecoderInner
        Ok((
            Self {
                format_ctx: ctx,
                codec_ctx,
                stream_index: stream_index as i32,
                sws_ctx: None,
                sws_cache_key: None,
                output_format,
                output_scale,
                is_live,
                eof: false,
                position: Duration::ZERO,
                packet,
                frame,
                thumbnail_sws_ctx: None,
                thumbnail_cache_key: None,
                hw_device_ctx,
                active_hw_accel,
                frame_pool,
                url,
                network_opts: stored_network_opts,
                reconnect_count: 0,
                consecutive_invalid: 0,
            },
            stream_info,
            container_info,
        ))
    }
}

// No manual `Drop` is needed: every field owns its FFmpeg resource and frees
// itself when the struct drops (fields drop in declaration order). The hw device
// reference (`HwDeviceContext`) is reference-counted and the `CodecContext` holds
// its own reference, so the order in which the two release relative to each other
// does not matter.

// SAFETY: VideoDecoderInner manages FFmpeg contexts which are thread-safe when not shared.
// We don't expose mutable access across threads, so Send is safe.
unsafe impl Send for VideoDecoderInner {}

#[cfg(test)]
mod tests {
    use ff_format::PixelFormat;
    use ff_format::codec::VideoCodec;
    use ff_format::color::{ColorPrimaries, ColorRange, ColorSpace};

    use crate::HardwareAccel;

    use super::VideoDecoderInner;

    // -------------------------------------------------------------------------
    // convert_pixel_format
    // -------------------------------------------------------------------------

    #[test]
    fn pixel_format_yuv420p() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P),
            PixelFormat::Yuv420p
        );
    }

    #[test]
    fn pixel_format_yuv422p() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P),
            PixelFormat::Yuv422p
        );
    }

    #[test]
    fn pixel_format_yuv444p() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P),
            PixelFormat::Yuv444p
        );
    }

    #[test]
    fn pixel_format_rgb24() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24),
            PixelFormat::Rgb24
        );
    }

    #[test]
    fn pixel_format_bgr24() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_BGR24),
            PixelFormat::Bgr24
        );
    }

    #[test]
    fn pixel_format_rgba() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA),
            PixelFormat::Rgba
        );
    }

    #[test]
    fn pixel_format_bgra() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_BGRA),
            PixelFormat::Bgra
        );
    }

    #[test]
    fn pixel_format_gray8() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_GRAY8),
            PixelFormat::Gray8
        );
    }

    #[test]
    fn pixel_format_nv12() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_NV12),
            PixelFormat::Nv12
        );
    }

    #[test]
    fn pixel_format_nv21() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_NV21),
            PixelFormat::Nv21
        );
    }

    #[test]
    fn pixel_format_yuv420p10le_should_return_yuv420p10le() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE),
            PixelFormat::Yuv420p10le
        );
    }

    #[test]
    fn pixel_format_yuv422p10le_should_return_yuv422p10le() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P10LE),
            PixelFormat::Yuv422p10le
        );
    }

    #[test]
    fn pixel_format_yuv444p10le_should_return_yuv444p10le() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P10LE),
            PixelFormat::Yuv444p10le
        );
    }

    #[test]
    fn pixel_format_p010le_should_return_p010le() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_P010LE),
            PixelFormat::P010le
        );
    }

    #[test]
    fn pixel_format_unknown_falls_back_to_yuv420p() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_NONE),
            PixelFormat::Yuv420p
        );
    }

    // -------------------------------------------------------------------------
    // convert_color_space
    // -------------------------------------------------------------------------

    #[test]
    fn color_space_bt709() {
        assert_eq!(
            VideoDecoderInner::convert_color_space(ff_sys::AVColorSpace_AVCOL_SPC_BT709),
            ColorSpace::Bt709
        );
    }

    #[test]
    fn color_space_bt470bg_yields_bt470bg() {
        assert_eq!(
            VideoDecoderInner::convert_color_space(ff_sys::AVColorSpace_AVCOL_SPC_BT470BG),
            ColorSpace::Bt470bg
        );
    }

    #[test]
    fn color_space_smpte170m_yields_smpte170m() {
        assert_eq!(
            VideoDecoderInner::convert_color_space(ff_sys::AVColorSpace_AVCOL_SPC_SMPTE170M),
            ColorSpace::Smpte170m
        );
    }

    #[test]
    fn color_space_bt2020_ncl() {
        assert_eq!(
            VideoDecoderInner::convert_color_space(ff_sys::AVColorSpace_AVCOL_SPC_BT2020_NCL),
            ColorSpace::Bt2020Ncl
        );
    }

    #[test]
    fn color_space_unknown_falls_back_to_bt709() {
        assert_eq!(
            VideoDecoderInner::convert_color_space(ff_sys::AVColorSpace_AVCOL_SPC_UNSPECIFIED),
            ColorSpace::Bt709
        );
    }

    // -------------------------------------------------------------------------
    // convert_color_range
    // -------------------------------------------------------------------------

    #[test]
    fn color_range_jpeg_yields_full() {
        assert_eq!(
            VideoDecoderInner::convert_color_range(ff_sys::AVColorRange_AVCOL_RANGE_JPEG),
            ColorRange::Full
        );
    }

    #[test]
    fn color_range_mpeg_yields_limited() {
        assert_eq!(
            VideoDecoderInner::convert_color_range(ff_sys::AVColorRange_AVCOL_RANGE_MPEG),
            ColorRange::Limited
        );
    }

    #[test]
    fn color_range_unknown_falls_back_to_limited() {
        assert_eq!(
            VideoDecoderInner::convert_color_range(ff_sys::AVColorRange_AVCOL_RANGE_UNSPECIFIED),
            ColorRange::Limited
        );
    }

    // -------------------------------------------------------------------------
    // convert_color_primaries
    // -------------------------------------------------------------------------

    #[test]
    fn color_primaries_bt709() {
        assert_eq!(
            VideoDecoderInner::convert_color_primaries(ff_sys::AVColorPrimaries_AVCOL_PRI_BT709),
            ColorPrimaries::Bt709
        );
    }

    #[test]
    fn color_primaries_bt470bg_yields_bt470bg() {
        assert_eq!(
            VideoDecoderInner::convert_color_primaries(ff_sys::AVColorPrimaries_AVCOL_PRI_BT470BG),
            ColorPrimaries::Bt470bg
        );
    }

    #[test]
    fn color_primaries_smpte170m_yields_smpte170m() {
        assert_eq!(
            VideoDecoderInner::convert_color_primaries(
                ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE170M
            ),
            ColorPrimaries::Smpte170m
        );
    }

    #[test]
    fn color_primaries_bt2020() {
        assert_eq!(
            VideoDecoderInner::convert_color_primaries(ff_sys::AVColorPrimaries_AVCOL_PRI_BT2020),
            ColorPrimaries::Bt2020
        );
    }

    #[test]
    fn color_primaries_unknown_falls_back_to_bt709() {
        assert_eq!(
            VideoDecoderInner::convert_color_primaries(
                ff_sys::AVColorPrimaries_AVCOL_PRI_UNSPECIFIED
            ),
            ColorPrimaries::Bt709
        );
    }

    // -------------------------------------------------------------------------
    // convert_codec
    // -------------------------------------------------------------------------

    #[test]
    fn codec_h264() {
        assert_eq!(
            VideoDecoderInner::convert_codec(ff_sys::AVCodecID_AV_CODEC_ID_H264),
            VideoCodec::H264
        );
    }

    #[test]
    fn codec_hevc_yields_h265() {
        assert_eq!(
            VideoDecoderInner::convert_codec(ff_sys::AVCodecID_AV_CODEC_ID_HEVC),
            VideoCodec::H265
        );
    }

    #[test]
    fn codec_vp8() {
        assert_eq!(
            VideoDecoderInner::convert_codec(ff_sys::AVCodecID_AV_CODEC_ID_VP8),
            VideoCodec::Vp8
        );
    }

    #[test]
    fn codec_vp9() {
        assert_eq!(
            VideoDecoderInner::convert_codec(ff_sys::AVCodecID_AV_CODEC_ID_VP9),
            VideoCodec::Vp9
        );
    }

    #[test]
    fn codec_av1() {
        assert_eq!(
            VideoDecoderInner::convert_codec(ff_sys::AVCodecID_AV_CODEC_ID_AV1),
            VideoCodec::Av1
        );
    }

    #[test]
    fn codec_mpeg4() {
        assert_eq!(
            VideoDecoderInner::convert_codec(ff_sys::AVCodecID_AV_CODEC_ID_MPEG4),
            VideoCodec::Mpeg4
        );
    }

    #[test]
    fn codec_prores() {
        assert_eq!(
            VideoDecoderInner::convert_codec(ff_sys::AVCodecID_AV_CODEC_ID_PRORES),
            VideoCodec::ProRes
        );
    }

    #[test]
    fn codec_unknown_falls_back_to_h264() {
        assert_eq!(
            VideoDecoderInner::convert_codec(ff_sys::AVCodecID_AV_CODEC_ID_NONE),
            VideoCodec::H264
        );
    }

    // -------------------------------------------------------------------------
    // hw_accel_to_device_type
    // -------------------------------------------------------------------------

    #[test]
    fn hw_accel_auto_yields_none() {
        assert_eq!(
            VideoDecoderInner::hw_accel_to_device_type(HardwareAccel::Auto),
            None
        );
    }

    #[test]
    fn hw_accel_none_yields_none() {
        assert_eq!(
            VideoDecoderInner::hw_accel_to_device_type(HardwareAccel::None),
            None
        );
    }

    #[test]
    fn hw_accel_nvdec_yields_cuda() {
        assert_eq!(
            VideoDecoderInner::hw_accel_to_device_type(HardwareAccel::Nvdec),
            Some(ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_CUDA)
        );
    }

    #[test]
    fn hw_accel_qsv_yields_qsv() {
        assert_eq!(
            VideoDecoderInner::hw_accel_to_device_type(HardwareAccel::Qsv),
            Some(ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_QSV)
        );
    }

    #[test]
    fn hw_accel_amf_yields_d3d11va() {
        assert_eq!(
            VideoDecoderInner::hw_accel_to_device_type(HardwareAccel::Amf),
            Some(ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_D3D11VA)
        );
    }

    #[test]
    fn hw_accel_videotoolbox() {
        assert_eq!(
            VideoDecoderInner::hw_accel_to_device_type(HardwareAccel::VideoToolbox),
            Some(ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_VIDEOTOOLBOX)
        );
    }

    #[test]
    fn hw_accel_vaapi() {
        assert_eq!(
            VideoDecoderInner::hw_accel_to_device_type(HardwareAccel::Vaapi),
            Some(ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_VAAPI)
        );
    }

    // -------------------------------------------------------------------------
    // pixel_format_to_av — round-trip
    // -------------------------------------------------------------------------

    #[test]
    fn pixel_format_to_av_round_trip_yuv420p() {
        let av = VideoDecoderInner::pixel_format_to_av(PixelFormat::Yuv420p);
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(av),
            PixelFormat::Yuv420p
        );
    }

    #[test]
    fn pixel_format_to_av_round_trip_yuv422p() {
        let av = VideoDecoderInner::pixel_format_to_av(PixelFormat::Yuv422p);
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(av),
            PixelFormat::Yuv422p
        );
    }

    #[test]
    fn pixel_format_to_av_round_trip_yuv444p() {
        let av = VideoDecoderInner::pixel_format_to_av(PixelFormat::Yuv444p);
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(av),
            PixelFormat::Yuv444p
        );
    }

    #[test]
    fn pixel_format_to_av_round_trip_rgb24() {
        let av = VideoDecoderInner::pixel_format_to_av(PixelFormat::Rgb24);
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(av),
            PixelFormat::Rgb24
        );
    }

    #[test]
    fn pixel_format_to_av_round_trip_bgr24() {
        let av = VideoDecoderInner::pixel_format_to_av(PixelFormat::Bgr24);
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(av),
            PixelFormat::Bgr24
        );
    }

    #[test]
    fn pixel_format_to_av_round_trip_rgba() {
        let av = VideoDecoderInner::pixel_format_to_av(PixelFormat::Rgba);
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(av),
            PixelFormat::Rgba
        );
    }

    #[test]
    fn pixel_format_to_av_round_trip_bgra() {
        let av = VideoDecoderInner::pixel_format_to_av(PixelFormat::Bgra);
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(av),
            PixelFormat::Bgra
        );
    }

    #[test]
    fn pixel_format_to_av_round_trip_gray8() {
        let av = VideoDecoderInner::pixel_format_to_av(PixelFormat::Gray8);
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(av),
            PixelFormat::Gray8
        );
    }

    #[test]
    fn pixel_format_to_av_round_trip_nv12() {
        let av = VideoDecoderInner::pixel_format_to_av(PixelFormat::Nv12);
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(av),
            PixelFormat::Nv12
        );
    }

    #[test]
    fn pixel_format_to_av_round_trip_nv21() {
        let av = VideoDecoderInner::pixel_format_to_av(PixelFormat::Nv21);
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(av),
            PixelFormat::Nv21
        );
    }

    #[test]
    fn pixel_format_to_av_unknown_falls_back_to_yuv420p_av() {
        // Other(999) has no explicit mapping and hits the _ fallback arm.
        assert_eq!(
            VideoDecoderInner::pixel_format_to_av(PixelFormat::Other(999)),
            ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P
        );
    }

    // -------------------------------------------------------------------------
    // extract_codec_name
    // -------------------------------------------------------------------------

    #[test]
    fn codec_name_should_return_h264_for_h264_codec_id() {
        let name =
            unsafe { VideoDecoderInner::extract_codec_name(ff_sys::AVCodecID_AV_CODEC_ID_H264) };
        assert_eq!(name, "h264");
    }

    #[test]
    fn codec_name_should_return_none_for_none_codec_id() {
        let name =
            unsafe { VideoDecoderInner::extract_codec_name(ff_sys::AVCodecID_AV_CODEC_ID_NONE) };
        assert_eq!(name, "none");
    }

    #[test]
    fn convert_pixel_format_should_map_gbrpf32le() {
        assert_eq!(
            VideoDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_GBRPF32LE),
            PixelFormat::Gbrpf32le
        );
    }

    #[test]
    fn unsupported_codec_error_should_include_codec_name() {
        let codec_id = ff_sys::AVCodecID_AV_CODEC_ID_H264;
        let codec_name = unsafe { VideoDecoderInner::extract_codec_name(codec_id) };
        let error = crate::error::DecodeError::UnsupportedCodec {
            codec: format!("{codec_name} (codec_id={codec_id:?})"),
        };
        let msg = error.to_string();
        assert!(msg.contains("h264"), "expected codec name in error: {msg}");
        assert!(
            msg.contains("codec_id="),
            "expected codec_id in error: {msg}"
        );
    }
}
