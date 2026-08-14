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
use std::ptr;

use ff_format::time::{Rational, Timestamp};
use ff_format::{PixelFormat, PooledBuffer, VideoFrame};
use ff_sys::{
    AVCodecContext, AVCodecID, AVFormatContext, AVFrame, AVMediaType_AVMEDIA_TYPE_VIDEO, AVPacket,
    AVPixelFormat,
};

use crate::error::DecodeError;
use crate::shared::guards_inner::{AvCodecContextGuard, AvFormatContextGuard};
use crate::shared::plane_inner::plane_row_ptr;

// ── ImageDecoderInner ─────────────────────────────────────────────────────────

/// Internal state for the image decoder.
///
/// Holds raw FFmpeg pointers and is responsible for proper cleanup in `Drop`.
pub(crate) struct ImageDecoderInner {
    /// Format context for reading the image file.
    format_ctx: *mut AVFormatContext,
    /// Codec context for decoding the image.
    codec_ctx: *mut AVCodecContext,
    /// Video stream index in the format context.
    stream_index: usize,
    /// Reusable packet for reading from file.
    packet: *mut AVPacket,
    /// Reusable frame for decoding.
    frame: *mut AVFrame,
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
        // SAFETY: Path is valid; AvFormatContextGuard ensures cleanup on error.
        let format_ctx_guard = unsafe { AvFormatContextGuard::new(path)? };
        let format_ctx = format_ctx_guard.as_ptr();

        // 2. avformat_find_stream_info
        // SAFETY: format_ctx is valid and owned by the guard.
        unsafe {
            ff_sys::avformat::find_stream_info(format_ctx).map_err(|e| DecodeError::Ffmpeg {
                code: e,
                message: format!("Failed to find stream info: {}", ff_sys::av_error_string(e)),
            })?;
        }

        // 3. Find the video stream.
        // SAFETY: format_ctx is valid.
        let (stream_index, codec_id) =
            unsafe { Self::find_video_stream(format_ctx) }.ok_or_else(|| {
                DecodeError::NoVideoStream {
                    path: path.to_path_buf(),
                }
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
        let codec = unsafe {
            ff_sys::avcodec::find_decoder(codec_id).ok_or_else(|| {
                DecodeError::UnsupportedCodec {
                    codec: format!("{codec_name} (codec_id={codec_id:?})"),
                }
            })?
        };

        // 5. avcodec_alloc_context3
        // SAFETY: codec pointer is valid; AvCodecContextGuard ensures cleanup.
        let codec_ctx_guard = unsafe { AvCodecContextGuard::new(codec)? };
        let codec_ctx = codec_ctx_guard.as_ptr();

        // 6. avcodec_parameters_to_context
        // SAFETY: All pointers are valid; stream_index was validated above.
        unsafe {
            let stream = (*format_ctx).streams.add(stream_index);
            let codecpar = (*(*stream)).codecpar;
            ff_sys::avcodec::parameters_to_context(codec_ctx, codecpar).map_err(|e| {
                DecodeError::Ffmpeg {
                    code: e,
                    message: format!(
                        "Failed to copy codec parameters: {}",
                        ff_sys::av_error_string(e)
                    ),
                }
            })?;
        }

        // 7. avcodec_open2
        // SAFETY: codec_ctx and codec are valid; no hardware acceleration for images.
        unsafe {
            ff_sys::avcodec::open2(codec_ctx, codec, ptr::null_mut()).map_err(|e| {
                DecodeError::Ffmpeg {
                    code: e,
                    message: format!("Failed to open codec: {}", ff_sys::av_error_string(e)),
                }
            })?;
        }

        // Allocate packet and frame.
        // SAFETY: FFmpeg is initialized.
        let packet = unsafe { ff_sys::av_packet_alloc() };
        if packet.is_null() {
            return Err(DecodeError::Ffmpeg {
                code: 0,
                message: "Failed to allocate packet".to_string(),
            });
        }
        let frame = unsafe { ff_sys::av_frame_alloc() };
        if frame.is_null() {
            unsafe { ff_sys::av_packet_free(&mut (packet as *mut _)) };
            return Err(DecodeError::Ffmpeg {
                code: 0,
                message: "Failed to allocate frame".to_string(),
            });
        }

        Ok(Self {
            format_ctx: format_ctx_guard.into_raw(),
            codec_ctx: codec_ctx_guard.into_raw(),
            stream_index,
            packet,
            frame,
        })
    }

    /// Returns the image width in pixels.
    pub(crate) fn width(&self) -> u32 {
        // SAFETY: codec_ctx is valid for the lifetime of `self`.
        unsafe { (*self.codec_ctx).width as u32 }
    }

    /// Returns the image height in pixels.
    pub(crate) fn height(&self) -> u32 {
        // SAFETY: codec_ctx is valid for the lifetime of `self`.
        unsafe { (*self.codec_ctx).height as u32 }
    }

    /// Decodes the image, consuming `self` and returning a [`VideoFrame`].
    ///
    /// Follows the sequence:
    /// 1. `av_read_frame`
    /// 2. `avcodec_send_packet`
    /// 3. `avcodec_receive_frame`
    /// 4. Convert to [`VideoFrame`]
    pub(crate) fn decode(self) -> Result<VideoFrame, DecodeError> {
        // 1. av_read_frame
        // SAFETY: format_ctx and packet are valid.
        let ret = unsafe { ff_sys::av_read_frame(self.format_ctx, self.packet) };
        if ret < 0 {
            return Err(DecodeError::Ffmpeg {
                code: ret,
                message: format!("Failed to read frame: {}", ff_sys::av_error_string(ret)),
            });
        }

        // 2. avcodec_send_packet
        // SAFETY: codec_ctx and packet are valid; packet contains image data.
        let ret = unsafe { ff_sys::avcodec_send_packet(self.codec_ctx, self.packet) };
        unsafe { ff_sys::av_packet_unref(self.packet) };
        if ret < 0 {
            return Err(DecodeError::Ffmpeg {
                code: ret,
                message: format!(
                    "Failed to send packet to decoder: {}",
                    ff_sys::av_error_string(ret)
                ),
            });
        }

        // 3. avcodec_receive_frame
        // SAFETY: codec_ctx and frame are valid.
        let ret = unsafe { ff_sys::avcodec_receive_frame(self.codec_ctx, self.frame) };
        if ret < 0 {
            return Err(DecodeError::Ffmpeg {
                code: ret,
                message: format!(
                    "Failed to receive decoded frame: {}",
                    ff_sys::av_error_string(ret)
                ),
            });
        }

        // 4. Convert to VideoFrame.
        // SAFETY: frame is valid and contains decoded image data.
        let video_frame = unsafe { self.av_frame_to_video_frame(self.frame)? };
        Ok(video_frame)
    }

    /// Finds the first video stream in the format context.
    ///
    /// # Safety
    ///
    /// `format_ctx` must be a valid, fully initialized `AVFormatContext`.
    unsafe fn find_video_stream(format_ctx: *mut AVFormatContext) -> Option<(usize, AVCodecID)> {
        // SAFETY: Caller ensures format_ctx is valid.
        unsafe {
            let nb_streams = (*format_ctx).nb_streams as usize;
            for i in 0..nb_streams {
                let stream = (*format_ctx).streams.add(i);
                let codecpar = (*(*stream)).codecpar;
                if (*codecpar).codec_type == AVMediaType_AVMEDIA_TYPE_VIDEO {
                    return Some((i, (*codecpar).codec_id));
                }
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

    /// Converts a decoded `AVFrame` to a [`VideoFrame`].
    ///
    /// # Safety
    ///
    /// `frame` must be a valid, fully decoded `AVFrame` owned by `self`.
    unsafe fn av_frame_to_video_frame(
        &self,
        frame: *const AVFrame,
    ) -> Result<VideoFrame, DecodeError> {
        // SAFETY: Caller ensures frame is valid.
        unsafe {
            let width = (*frame).width as u32;
            let height = (*frame).height as u32;
            let format = Self::convert_pixel_format((*frame).format);

            // Extract timestamp (images often have no meaningful PTS).
            let pts = (*frame).pts;
            let timestamp = if pts == ff_sys::AV_NOPTS_VALUE {
                Timestamp::default()
            } else {
                let stream = (*self.format_ctx).streams.add(self.stream_index);
                let time_base = (*(*stream)).time_base;
                Timestamp::new(
                    pts as i64,
                    Rational::new(time_base.num as i32, time_base.den as i32),
                )
            };

            let (planes, strides) = Self::extract_planes_and_strides(frame, width, height, format)?;

            // Images are always key frames.
            VideoFrame::new(planes, strides, width, height, format, timestamp, true).map_err(|e| {
                DecodeError::Ffmpeg {
                    code: 0,
                    message: format!("Failed to create VideoFrame: {e}"),
                }
            })
        }
    }

    /// Extracts pixel data from an `AVFrame` into [`PooledBuffer`] planes.
    ///
    /// Copies data row-by-row to strip any FFmpeg padding from line strides.
    ///
    /// # Safety
    ///
    /// `frame` must be a valid, fully decoded `AVFrame` with `format` matching
    /// the actual pixel format of the frame.
    unsafe fn extract_planes_and_strides(
        frame: *const AVFrame,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<(Vec<PooledBuffer>, Vec<usize>), DecodeError> {
        // SAFETY: Caller ensures frame is valid and format matches.
        unsafe {
            let w = width as usize;
            let h = height as usize;
            let mut planes: Vec<PooledBuffer> = Vec::new();
            let mut strides: Vec<usize> = Vec::new();

            match format {
                PixelFormat::Rgba | PixelFormat::Bgra => {
                    let bytes_per_pixel = 4_usize;
                    let row_w = w * bytes_per_pixel;
                    let mut buf = vec![0u8; row_w * h];
                    let src = (*frame).data[0];
                    if src.is_null() {
                        return Err(DecodeError::Ffmpeg {
                            code: 0,
                            message: "Null plane data for packed format".to_string(),
                        });
                    }
                    let src_linesize = (*frame).linesize[0];
                    for row in 0..h {
                        ptr::copy_nonoverlapping(
                            plane_row_ptr(src, src_linesize, row),
                            buf[row * row_w..].as_mut_ptr(),
                            row_w,
                        );
                    }
                    planes.push(PooledBuffer::standalone(buf));
                    strides.push(row_w);
                }
                PixelFormat::Rgb24 | PixelFormat::Bgr24 => {
                    let bytes_per_pixel = 3_usize;
                    let row_w = w * bytes_per_pixel;
                    let mut buf = vec![0u8; row_w * h];
                    let src = (*frame).data[0];
                    if src.is_null() {
                        return Err(DecodeError::Ffmpeg {
                            code: 0,
                            message: "Null plane data for packed format".to_string(),
                        });
                    }
                    let src_linesize = (*frame).linesize[0];
                    for row in 0..h {
                        ptr::copy_nonoverlapping(
                            plane_row_ptr(src, src_linesize, row),
                            buf[row * row_w..].as_mut_ptr(),
                            row_w,
                        );
                    }
                    planes.push(PooledBuffer::standalone(buf));
                    strides.push(row_w);
                }
                PixelFormat::Gray8 => {
                    let mut buf = vec![0u8; w * h];
                    let src = (*frame).data[0];
                    if src.is_null() {
                        return Err(DecodeError::Ffmpeg {
                            code: 0,
                            message: "Null plane data for Gray8".to_string(),
                        });
                    }
                    let src_linesize = (*frame).linesize[0];
                    for row in 0..h {
                        ptr::copy_nonoverlapping(
                            plane_row_ptr(src, src_linesize, row),
                            buf[row * w..].as_mut_ptr(),
                            w,
                        );
                    }
                    planes.push(PooledBuffer::standalone(buf));
                    strides.push(w);
                }
                PixelFormat::Yuv420p | PixelFormat::Nv12 | PixelFormat::Nv21 => {
                    // Y plane (full size).
                    let mut y_buf = vec![0u8; w * h];
                    let y_src = (*frame).data[0];
                    if y_src.is_null() {
                        return Err(DecodeError::Ffmpeg {
                            code: 0,
                            message: "Null Y plane".to_string(),
                        });
                    }
                    let y_src_linesize = (*frame).linesize[0];
                    for row in 0..h {
                        ptr::copy_nonoverlapping(
                            plane_row_ptr(y_src, y_src_linesize, row),
                            y_buf[row * w..].as_mut_ptr(),
                            w,
                        );
                    }
                    planes.push(PooledBuffer::standalone(y_buf));
                    strides.push(w);

                    if matches!(format, PixelFormat::Nv12 | PixelFormat::Nv21) {
                        // Interleaved UV plane (half height).
                        let uv_h = h / 2;
                        let mut uv_buf = vec![0u8; w * uv_h];
                        let uv_src = (*frame).data[1];
                        if !uv_src.is_null() {
                            let uv_src_linesize = (*frame).linesize[1];
                            for row in 0..uv_h {
                                ptr::copy_nonoverlapping(
                                    plane_row_ptr(uv_src, uv_src_linesize, row),
                                    uv_buf[row * w..].as_mut_ptr(),
                                    w,
                                );
                            }
                        }
                        planes.push(PooledBuffer::standalone(uv_buf));
                        strides.push(w);
                    } else {
                        // YUV 4:2:0 — separate U and V planes (half width, half height).
                        let uv_w = w / 2;
                        let uv_h = h / 2;
                        for plane_idx in 1..=2usize {
                            let mut uv_buf = vec![0u8; uv_w * uv_h];
                            let uv_src = (*frame).data[plane_idx];
                            if !uv_src.is_null() {
                                let uv_src_linesize = (*frame).linesize[plane_idx];
                                for row in 0..uv_h {
                                    ptr::copy_nonoverlapping(
                                        plane_row_ptr(uv_src, uv_src_linesize, row),
                                        uv_buf[row * uv_w..].as_mut_ptr(),
                                        uv_w,
                                    );
                                }
                            }
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
                        let src = (*frame).data[plane_idx];
                        if !src.is_null() {
                            let src_linesize = (*frame).linesize[plane_idx];
                            for row in 0..*ph {
                                ptr::copy_nonoverlapping(
                                    plane_row_ptr(src, src_linesize, row),
                                    buf[row * pw..].as_mut_ptr(),
                                    *pw,
                                );
                            }
                        }
                        planes.push(PooledBuffer::standalone(buf));
                        strides.push(*pw);
                    }
                }
                PixelFormat::Yuv444p => {
                    // All three planes are full size.
                    for plane_idx in 0..3usize {
                        let mut buf = vec![0u8; w * h];
                        let src = (*frame).data[plane_idx];
                        if !src.is_null() {
                            let src_linesize = (*frame).linesize[plane_idx];
                            for row in 0..h {
                                ptr::copy_nonoverlapping(
                                    plane_row_ptr(src, src_linesize, row),
                                    buf[row * w..].as_mut_ptr(),
                                    w,
                                );
                            }
                        }
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
}

impl Drop for ImageDecoderInner {
    fn drop(&mut self) {
        // SAFETY: All pointers are exclusively owned by this struct and were
        // allocated by the corresponding FFmpeg alloc functions.
        unsafe {
            if !self.frame.is_null() {
                ff_sys::av_frame_free(&mut (self.frame as *mut _));
            }
            if !self.packet.is_null() {
                ff_sys::av_packet_free(&mut (self.packet as *mut _));
            }
            if !self.codec_ctx.is_null() {
                ff_sys::avcodec::free_context(&mut (self.codec_ctx as *mut _));
            }
            if !self.format_ctx.is_null() {
                ff_sys::avformat::close_input(&mut (self.format_ctx as *mut _));
            }
        }
    }
}

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
