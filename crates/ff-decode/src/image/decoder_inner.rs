//! Internal image decoder implementation using FFmpeg.
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
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::unnecessary_cast)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_lossless)]

use std::ffi::CStr;
use std::path::Path;

use ff_format::time::{Rational, Timestamp};
use ff_format::{PixelFormat, PooledBuffer, VideoFrame};
use ff_sys::{
    AVCodecID, AVMediaType_AVMEDIA_TYPE_VIDEO, AVPixelFormat, Frame, InputFormatContext, Packet,
};

use crate::error::DecodeError;
use crate::shared::guards_inner::open_input_ctx;

// ImageDecoderInner

/// Internal state for the image decoder.
///
/// Holds raw FFmpeg pointers and is responsible for proper cleanup in `Drop`.
pub(crate) struct ImageDecoderInner {
    /// Format context for reading the image file.
    format_ctx: InputFormatContext,
    /// Codec context for decoding the image.
    codec_ctx: ff_sys::CodecContext,
    /// Video stream index in the format context.
    stream_index: usize,
    /// Reusable packet for reading from file.
    packet: Packet,
    /// Reusable frame for decoding.
    frame: Frame,
}

// SAFETY: `ImageDecoderInner` owns all FFmpeg contexts exclusively.
//         FFmpeg contexts are not safe for concurrent access (not Sync),
//         but ownership transfer between threads is safe.
unsafe impl Send for ImageDecoderInner {}

impl ImageDecoderInner {
    /// Opens an image file and prepares the decoder.
    ///
    /// Performs the full FFmpeg initialization sequence:
    /// 1. `avformat_open_input`
    /// 2. `avformat_find_stream_info`
    /// 3. `av_find_best_stream(AVMEDIA_TYPE_VIDEO)`
    /// 4. `avcodec_find_decoder`
    /// 5. `avcodec_alloc_context3`
    /// 6. `avcodec_parameters_to_context`
    /// 7. `avcodec_open2`
    pub(crate) fn new(path: &Path) -> Result<Self, DecodeError> {
        ff_sys::ensure_initialized();

        // 1. avformat_open_input
        let mut ctx = open_input_ctx(path)?;

        // 2. avformat_find_stream_info
        ctx.find_stream_info().map_err(|e| DecodeError::Ffmpeg {
            code: e.code(),
            message: format!(
                "Failed to find stream info: {}",
                ff_sys::av_error_string(e.code())
            ),
        })?;

        // 3. Find the video stream.
        let (stream_index, codec_id) =
            Self::find_video_stream(&ctx).ok_or_else(|| DecodeError::NoVideoStream {
                path: path.to_path_buf(),
            })?;

        // 4. avcodec_find_decoder
        // SAFETY: codec_id comes from FFmpeg.
        // SAFETY: avcodec_get_name is safe for any codec ID value and returns a static C string.
        let codec_name = unsafe {
            let name_ptr = ff_sys::avcodec_get_name(codec_id);
            if name_ptr.is_null() {
                String::from("unknown")
            } else {
                CStr::from_ptr(name_ptr).to_string_lossy().into_owned()
            }
        };
        let codec =
            ff_sys::Codec::find_decoder(codec_id).ok_or_else(|| DecodeError::UnsupportedCodec {
                codec: format!("{codec_name} (codec_id={codec_id:?})"),
            })?;

        // 5. avcodec_alloc_context3 (freed on drop by CodecContext).
        let mut codec_ctx =
            ff_sys::CodecContext::new(Some(codec)).map_err(|e| DecodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Failed to allocate codec context: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

        // 6. avcodec_parameters_to_context
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

        // 7. avcodec_open2
        codec_ctx
            .open_codec(codec)
            .map_err(|e| DecodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Failed to open codec: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })?;

        // Allocate packet and frame (owned; free on drop, including on an early
        // return from a later `?` — the packet frees itself if the frame alloc fails).
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

        Ok(Self {
            format_ctx: ctx,
            codec_ctx,
            stream_index,
            packet,
            frame,
        })
    }

    /// Returns the image width in pixels.
    pub(crate) fn width(&self) -> u32 {
        self.codec_ctx.width() as u32
    }

    /// Returns the image height in pixels.
    pub(crate) fn height(&self) -> u32 {
        self.codec_ctx.height() as u32
    }

    /// Decodes the image, consuming `self` and returning a [`VideoFrame`].
    ///
    /// Follows the sequence:
    /// 1. `av_read_frame`
    /// 2. `avcodec_send_packet`
    /// 3. `avcodec_receive_frame`
    /// 4. Convert to [`VideoFrame`]
    pub(crate) fn decode(mut self) -> Result<VideoFrame, DecodeError> {
        // 1. av_read_frame
        if let Err(e) = self.format_ctx.read_frame(&mut self.packet) {
            let ret = e.code();
            return Err(DecodeError::Ffmpeg {
                code: ret,
                message: format!("Failed to read frame: {}", ff_sys::av_error_string(ret)),
            });
        }

        // 2. avcodec_send_packet
        let send_result = self.codec_ctx.send_packet(&self.packet);
        self.packet.unref();
        if let Err(e) = send_result {
            return Err(DecodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Failed to send packet to decoder: {}",
                    ff_sys::av_error_string(e.code())
                ),
            });
        }

        // 3. avcodec_receive_frame
        match self
            .codec_ctx
            .receive_frame(&mut self.frame)
            .map_err(|e| DecodeError::Ffmpeg {
                code: e.code(),
                message: format!(
                    "Failed to receive decoded frame: {}",
                    ff_sys::av_error_string(e.code())
                ),
            })? {
            ff_sys::ReceiveOutcome::Frame => {}
            // Preserve the pre-migration behaviour: a bare EAGAIN/EOF from this
            // single receive was surfaced as a `Ffmpeg` error with that raw code.
            ff_sys::ReceiveOutcome::NeedInput => {
                return Err(DecodeError::Ffmpeg {
                    code: ff_sys::error_codes::EAGAIN,
                    message: format!(
                        "Failed to receive decoded frame: {}",
                        ff_sys::av_error_string(ff_sys::error_codes::EAGAIN)
                    ),
                });
            }
            ff_sys::ReceiveOutcome::Drained => {
                return Err(DecodeError::Ffmpeg {
                    code: ff_sys::error_codes::EOF,
                    message: format!(
                        "Failed to receive decoded frame: {}",
                        ff_sys::av_error_string(ff_sys::error_codes::EOF)
                    ),
                });
            }
        }

        // 4. Convert to VideoFrame.
        // SAFETY: frame is valid and contains decoded image data.
        let video_frame = unsafe { self.av_frame_to_video_frame(&self.frame)? };
        Ok(video_frame)
    }

    /// Finds the first video stream in the format context.
    fn find_video_stream(format_ctx: &InputFormatContext) -> Option<(usize, AVCodecID)> {
        for stream in format_ctx.streams() {
            let codecpar = stream.codecpar();
            if codecpar.codec_type() == AVMediaType_AVMEDIA_TYPE_VIDEO {
                return Some((stream.index() as usize, codecpar.codec_id()));
            }
        }
        None
    }

    /// Maps an `AVPixelFormat` value to our [`PixelFormat`] enum.
    ///
    /// Image decoders commonly produce YUVJ formats (full-range YUV), which
    /// have the same plane layout as the corresponding YUV formats but with a
    /// different color range flag.  We map them to their YUV equivalents here
    /// and rely on the colour-range metadata to distinguish them if needed.
    fn convert_pixel_format(fmt: AVPixelFormat) -> PixelFormat {
        if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P
            || fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUVJ420P
        {
            PixelFormat::Yuv420p
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P
            || fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUVJ422P
        {
            PixelFormat::Yuv422p
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P
            || fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUVJ444P
        {
            PixelFormat::Yuv444p
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24 {
            PixelFormat::Rgb24
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_BGR24 {
            PixelFormat::Bgr24
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA {
            PixelFormat::Rgba
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_BGRA {
            PixelFormat::Bgra
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_GRAY8 {
            PixelFormat::Gray8
        } else {
            log::warn!(
                "pixel_format unsupported, falling back to Rgb24 requested={fmt} fallback=Rgb24"
            );
            PixelFormat::Rgb24
        }
    }

    /// Converts a decoded owned [`Frame`] to a [`VideoFrame`].
    ///
    /// Scalar fields are read through accessors; the plane data copy in
    /// [`extract_planes_and_strides`](Self::extract_planes_and_strides) reads each
    /// plane through [`Frame::copy_plane_rows`].
    ///
    /// # Safety
    ///
    /// `frame` must hold a fully decoded image whose pixel format the copy expects.
    unsafe fn av_frame_to_video_frame(&self, frame: &Frame) -> Result<VideoFrame, DecodeError> {
        let width = frame.width() as u32;
        let height = frame.height() as u32;
        let format = Self::convert_pixel_format(frame.format());

        // Extract timestamp (images often have no meaningful PTS).
        let pts = frame.pts();
        let timestamp = if pts == ff_sys::AV_NOPTS_VALUE {
            Timestamp::default()
        } else {
            match self.format_ctx.stream(self.stream_index) {
                Some(stream) => {
                    let time_base = stream.time_base();
                    Timestamp::new(
                        pts,
                        Rational::new(time_base.num as i32, time_base.den as i32),
                    )
                }
                None => Timestamp::default(),
            }
        };

        // SAFETY: `format` is derived from `frame.format()`, so it matches the frame.
        let (planes, strides) =
            unsafe { Self::extract_planes_and_strides(frame, width, height, format)? };

        // Images are always key frames.
        VideoFrame::new(planes, strides, width, height, format, timestamp, true).map_err(|e| {
            DecodeError::Ffmpeg {
                code: 0,
                message: format!("Failed to create VideoFrame: {e}"),
            }
        })
    }

    /// Extracts pixel data from a decoded [`Frame`] into [`PooledBuffer`] planes.
    ///
    /// Copies data row-by-row to strip any FFmpeg padding from line strides.
    ///
    /// # Safety
    ///
    /// `frame` must be a valid, fully decoded frame with `format` matching the
    /// actual pixel format of the frame.
    unsafe fn extract_planes_and_strides(
        frame: &Frame,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<(Vec<PooledBuffer>, Vec<usize>), DecodeError> {
        let w = width as usize;
        let h = height as usize;
        let mut planes: Vec<PooledBuffer> = Vec::new();
        let mut strides: Vec<usize> = Vec::new();

        // Copies plane `i` (`rows` x `row_bytes`, packed into `buf` at `row_bytes`
        // stride) and returns `true` when the plane was present. A `false` result
        // (null / absent plane) leaves `buf` zero-filled — the caller decides
        // whether that is an error (a required plane) or acceptable (a chroma
        // plane).
        // SAFETY: the caller (this `unsafe fn`) guarantees `format` and the
        //         per-plane geometry match the frame, so `copy_plane_rows` reads
        //         within each plane and writes within `buf`.
        let copy_plane = |i: usize, buf: &mut [u8], rows: usize, row_bytes: usize| unsafe {
            frame
                .copy_plane_rows(i, buf, row_bytes, rows, row_bytes)
                .is_some()
        };

        match format {
            PixelFormat::Rgba | PixelFormat::Bgra => {
                let row_w = w * 4;
                let mut buf = vec![0u8; row_w * h];
                if !copy_plane(0, &mut buf, h, row_w) {
                    return Err(DecodeError::Ffmpeg {
                        code: 0,
                        message: "Null plane data for packed format".to_string(),
                    });
                }
                planes.push(PooledBuffer::standalone(buf));
                strides.push(row_w);
            }
            PixelFormat::Rgb24 | PixelFormat::Bgr24 => {
                let row_w = w * 3;
                let mut buf = vec![0u8; row_w * h];
                if !copy_plane(0, &mut buf, h, row_w) {
                    return Err(DecodeError::Ffmpeg {
                        code: 0,
                        message: "Null plane data for packed format".to_string(),
                    });
                }
                planes.push(PooledBuffer::standalone(buf));
                strides.push(row_w);
            }
            PixelFormat::Gray8 => {
                let mut buf = vec![0u8; w * h];
                if !copy_plane(0, &mut buf, h, w) {
                    return Err(DecodeError::Ffmpeg {
                        code: 0,
                        message: "Null plane data for Gray8".to_string(),
                    });
                }
                planes.push(PooledBuffer::standalone(buf));
                strides.push(w);
            }
            PixelFormat::Yuv420p | PixelFormat::Nv12 | PixelFormat::Nv21 => {
                // Y plane (full size).
                let mut y_buf = vec![0u8; w * h];
                if !copy_plane(0, &mut y_buf, h, w) {
                    return Err(DecodeError::Ffmpeg {
                        code: 0,
                        message: "Null Y plane".to_string(),
                    });
                }
                planes.push(PooledBuffer::standalone(y_buf));
                strides.push(w);

                if matches!(format, PixelFormat::Nv12 | PixelFormat::Nv21) {
                    // Interleaved UV plane (half height); a null plane stays zeroed.
                    let uv_h = h / 2;
                    let mut uv_buf = vec![0u8; w * uv_h];
                    copy_plane(1, &mut uv_buf, uv_h, w);
                    planes.push(PooledBuffer::standalone(uv_buf));
                    strides.push(w);
                } else {
                    // YUV 4:2:0 — separate U and V planes (half width, half height).
                    let uv_w = w / 2;
                    let uv_h = h / 2;
                    for plane_idx in 1..=2usize {
                        let mut uv_buf = vec![0u8; uv_w * uv_h];
                        copy_plane(plane_idx, &mut uv_buf, uv_h, uv_w);
                        planes.push(PooledBuffer::standalone(uv_buf));
                        strides.push(uv_w);
                    }
                }
            }
            PixelFormat::Yuv422p => {
                // Y plane (full size), U and V planes (half width, full height).
                let uv_w = w / 2;
                let plane_dims = [(w, h), (uv_w, h), (uv_w, h)];
                for (plane_idx, (pw, ph)) in plane_dims.iter().enumerate() {
                    let mut buf = vec![0u8; pw * ph];
                    copy_plane(plane_idx, &mut buf, *ph, *pw);
                    planes.push(PooledBuffer::standalone(buf));
                    strides.push(*pw);
                }
            }
            PixelFormat::Yuv444p => {
                // All three planes are full size.
                for plane_idx in 0..3usize {
                    let mut buf = vec![0u8; w * h];
                    copy_plane(plane_idx, &mut buf, h, w);
                    planes.push(PooledBuffer::standalone(buf));
                    strides.push(w);
                }
            }
            _ => {
                return Err(DecodeError::Ffmpeg {
                    code: 0,
                    message: format!("Unsupported pixel format for image decoding: {format:?}"),
                });
            }
        }

        Ok((planes, strides))
    }
}

// All fields own their FFmpeg resources (`Frame`, `Packet`, `CodecContext`,
// `InputFormatContext`) and free themselves on drop, so no manual `Drop` impl is
// required.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_pixel_format_yuv420p_should_map_to_yuv420p() {
        assert_eq!(
            ImageDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P),
            PixelFormat::Yuv420p
        );
    }

    #[test]
    fn convert_pixel_format_yuvj420p_should_map_to_yuv420p() {
        assert_eq!(
            ImageDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_YUVJ420P),
            PixelFormat::Yuv420p
        );
    }

    #[test]
    fn convert_pixel_format_rgb24_should_map_to_rgb24() {
        assert_eq!(
            ImageDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24),
            PixelFormat::Rgb24
        );
    }

    #[test]
    fn convert_pixel_format_rgba_should_map_to_rgba() {
        assert_eq!(
            ImageDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA),
            PixelFormat::Rgba
        );
    }

    #[test]
    fn convert_pixel_format_gray8_should_map_to_gray8() {
        assert_eq!(
            ImageDecoderInner::convert_pixel_format(ff_sys::AVPixelFormat_AV_PIX_FMT_GRAY8),
            PixelFormat::Gray8
        );
    }

    #[test]
    fn unsupported_codec_error_should_include_codec_name() {
        let codec_id = ff_sys::AVCodecID_AV_CODEC_ID_PNG;
        // SAFETY: avcodec_get_name is safe for any codec ID value and returns a static C string.
        let codec_name = unsafe {
            let name_ptr = ff_sys::avcodec_get_name(codec_id);
            if name_ptr.is_null() {
                String::from("unknown")
            } else {
                std::ffi::CStr::from_ptr(name_ptr)
                    .to_string_lossy()
                    .into_owned()
            }
        };
        let error = crate::error::DecodeError::UnsupportedCodec {
            codec: format!("{codec_name} (codec_id={codec_id:?})"),
        };
        let msg = error.to_string();
        assert!(msg.contains("png"), "expected codec name in error: {msg}");
        assert!(
            msg.contains("codec_id="),
            "expected codec_id in error: {msg}"
        );
    }
}
