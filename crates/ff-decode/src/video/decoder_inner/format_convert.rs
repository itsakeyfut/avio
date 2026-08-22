use super::{
    AVCodecID, AVColorPrimaries, AVColorRange, AVColorSpace, AVPixelFormat, AvFrameGuard,
    ColorPrimaries, ColorRange, ColorSpace, DecodeError, PixelFormat, VideoCodec,
    VideoDecoderInner, VideoFrame,
};

impl VideoDecoderInner {
    /// Converts FFmpeg pixel format to our PixelFormat enum.
    pub(super) fn convert_pixel_format(fmt: AVPixelFormat) -> PixelFormat {
        if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P {
            PixelFormat::Yuv420p
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P {
            PixelFormat::Yuv422p
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P {
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
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_NV12 {
            PixelFormat::Nv12
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_NV21 {
            PixelFormat::Nv21
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE {
            PixelFormat::Yuv420p10le
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P10LE {
            PixelFormat::Yuv422p10le
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P10LE {
            PixelFormat::Yuv444p10le
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_P010LE {
            PixelFormat::P010le
        } else if fmt == ff_sys::AVPixelFormat_AV_PIX_FMT_GBRPF32LE {
            PixelFormat::Gbrpf32le
        } else {
            log::warn!(
                "pixel_format unsupported, falling back to Yuv420p requested={fmt} fallback=Yuv420p"
            );
            PixelFormat::Yuv420p
        }
    }

    /// Converts FFmpeg color space to our ColorSpace enum.
    pub(super) fn convert_color_space(space: AVColorSpace) -> ColorSpace {
        if space == ff_sys::AVColorSpace_AVCOL_SPC_BT709 {
            ColorSpace::Bt709
        } else if space == ff_sys::AVColorSpace_AVCOL_SPC_BT470BG {
            ColorSpace::Bt470bg
        } else if space == ff_sys::AVColorSpace_AVCOL_SPC_SMPTE170M {
            ColorSpace::Smpte170m
        } else if space == ff_sys::AVColorSpace_AVCOL_SPC_SMPTE240M {
            ColorSpace::Smpte240m
        } else if space == ff_sys::AVColorSpace_AVCOL_SPC_FCC {
            ColorSpace::Fcc
        } else if space == ff_sys::AVColorSpace_AVCOL_SPC_YCGCO {
            ColorSpace::Ycgco
        } else if space == ff_sys::AVColorSpace_AVCOL_SPC_BT2020_NCL {
            ColorSpace::Bt2020Ncl
        } else if space == ff_sys::AVColorSpace_AVCOL_SPC_BT2020_CL {
            ColorSpace::Bt2020Cl
        } else if space == ff_sys::AVColorSpace_AVCOL_SPC_RGB {
            ColorSpace::Rgb
        } else {
            log::warn!(
                "color_space unsupported, falling back to Bt709 requested={space} fallback=Bt709"
            );
            ColorSpace::Bt709
        }
    }

    /// Converts FFmpeg color range to our ColorRange enum.
    pub(super) fn convert_color_range(range: AVColorRange) -> ColorRange {
        if range == ff_sys::AVColorRange_AVCOL_RANGE_JPEG {
            ColorRange::Full
        } else if range == ff_sys::AVColorRange_AVCOL_RANGE_MPEG {
            ColorRange::Limited
        } else {
            log::warn!(
                "color_range unsupported, falling back to Limited requested={range} fallback=Limited"
            );
            ColorRange::Limited
        }
    }

    /// Converts FFmpeg color primaries to our ColorPrimaries enum.
    pub(super) fn convert_color_primaries(primaries: AVColorPrimaries) -> ColorPrimaries {
        if primaries == ff_sys::AVColorPrimaries_AVCOL_PRI_BT709 {
            ColorPrimaries::Bt709
        } else if primaries == ff_sys::AVColorPrimaries_AVCOL_PRI_BT470BG {
            ColorPrimaries::Bt470bg
        } else if primaries == ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE170M {
            ColorPrimaries::Smpte170m
        } else if primaries == ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE240M {
            ColorPrimaries::Smpte240m
        } else if primaries == ff_sys::AVColorPrimaries_AVCOL_PRI_FILM {
            ColorPrimaries::Film
        } else if primaries == ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE431 {
            ColorPrimaries::DciP3
        } else if primaries == ff_sys::AVColorPrimaries_AVCOL_PRI_SMPTE432 {
            ColorPrimaries::DisplayP3
        } else if primaries == ff_sys::AVColorPrimaries_AVCOL_PRI_BT2020 {
            ColorPrimaries::Bt2020
        } else {
            log::warn!(
                "color_primaries unsupported, falling back to Bt709 requested={primaries} fallback=Bt709"
            );
            ColorPrimaries::Bt709
        }
    }

    /// Converts FFmpeg codec ID to our VideoCodec enum.
    pub(super) fn convert_codec(codec_id: AVCodecID) -> VideoCodec {
        if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_H264 {
            VideoCodec::H264
        } else if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_HEVC {
            VideoCodec::H265
        } else if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_VP8 {
            VideoCodec::Vp8
        } else if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_VP9 {
            VideoCodec::Vp9
        } else if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_AV1 {
            VideoCodec::Av1
        } else if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_MPEG4 {
            VideoCodec::Mpeg4
        } else if codec_id == ff_sys::AVCodecID_AV_CODEC_ID_PRORES {
            VideoCodec::ProRes
        } else {
            log::warn!(
                "video codec unsupported, falling back to H264 codec_id={codec_id} fallback=H264"
            );
            VideoCodec::H264
        }
    }

    /// Converts our `PixelFormat` to FFmpeg `AVPixelFormat`.
    pub(super) fn pixel_format_to_av(format: PixelFormat) -> AVPixelFormat {
        match format {
            PixelFormat::Yuv420p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P,
            PixelFormat::Yuv422p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P,
            PixelFormat::Yuv444p => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P,
            PixelFormat::Rgb24 => ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24,
            PixelFormat::Bgr24 => ff_sys::AVPixelFormat_AV_PIX_FMT_BGR24,
            PixelFormat::Rgba => ff_sys::AVPixelFormat_AV_PIX_FMT_RGBA,
            PixelFormat::Bgra => ff_sys::AVPixelFormat_AV_PIX_FMT_BGRA,
            PixelFormat::Gray8 => ff_sys::AVPixelFormat_AV_PIX_FMT_GRAY8,
            PixelFormat::Nv12 => ff_sys::AVPixelFormat_AV_PIX_FMT_NV12,
            PixelFormat::Nv21 => ff_sys::AVPixelFormat_AV_PIX_FMT_NV21,
            PixelFormat::Yuv420p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P10LE,
            PixelFormat::Yuv422p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV422P10LE,
            PixelFormat::Yuv444p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUV444P10LE,
            PixelFormat::Yuva444p10le => ff_sys::AVPixelFormat_AV_PIX_FMT_YUVA444P10LE,
            PixelFormat::P010le => ff_sys::AVPixelFormat_AV_PIX_FMT_P010LE,
            PixelFormat::Gbrpf32le => ff_sys::AVPixelFormat_AV_PIX_FMT_GBRPF32LE,
            _ => {
                log::warn!(
                    "pixel_format has no AV mapping, falling back to Yuv420p format={format:?} fallback=AV_PIX_FMT_YUV420P"
                );
                ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P
            }
        }
    }

    /// Converts pixel format and/or scales a frame using `libswscale`.
    ///
    /// The `sws_ctx` is cached and recreated only when the source/destination
    /// parameters change (cache key: `(src_w, src_h, src_fmt, dst_w, dst_h, dst_fmt)`).
    pub(super) unsafe fn convert_with_sws(
        &mut self,
        src_width: u32,
        src_height: u32,
        src_format: i32,
        dst_width: u32,
        dst_height: u32,
        dst_format: i32,
    ) -> Result<VideoFrame, DecodeError> {
        // SAFETY: Caller ensures frame and context pointers are valid
        unsafe {
            // Get or create SwScale context, invalidating cache when parameters change.
            let cache_key = (
                src_width, src_height, src_format, dst_width, dst_height, dst_format,
            );
            if self.sws_cache_key != Some(cache_key) {
                // The old context (if any) drops on reassignment.
                self.sws_ctx = Some(
                    ff_sys::ScaleContext::new(
                        src_width as i32,
                        src_height as i32,
                        src_format,
                        dst_width as i32,
                        dst_height as i32,
                        dst_format,
                        ff_sys::swscale::scale_flags::BILINEAR,
                    )
                    .map_err(|e| DecodeError::Ffmpeg {
                        code: 0,
                        message: format!("Failed to create sws context: {e}"),
                    })?,
                );
                self.sws_cache_key = Some(cache_key);
            }

            let Some(sws_ctx) = self.sws_ctx.as_mut() else {
                return Err(DecodeError::Ffmpeg {
                    code: 0,
                    message: "SwsContext not initialized".to_string(),
                });
            };

            // Allocate destination frame (with RAII guard)
            let dst_frame_guard = AvFrameGuard::new()?;
            let dst_frame = dst_frame_guard.as_ptr();

            (*dst_frame).width = dst_width as i32;
            (*dst_frame).height = dst_height as i32;
            (*dst_frame).format = dst_format;

            // Allocate buffer for destination frame
            let buffer_ret = ff_sys::av_frame_get_buffer(dst_frame, 0);
            if buffer_ret < 0 {
                return Err(DecodeError::Ffmpeg {
                    code: buffer_ret,
                    message: format!(
                        "Failed to allocate frame buffer: {}",
                        ff_sys::av_error_string(buffer_ret)
                    ),
                });
            }

            // Perform conversion/scaling (src_height is the number of input rows to process)
            sws_ctx
                .scale(
                    (*self.frame).data.as_ptr() as *const *const u8,
                    (*self.frame).linesize.as_ptr(),
                    0,
                    src_height as i32,
                    (*dst_frame).data.as_ptr() as *const *mut u8,
                    (*dst_frame).linesize.as_ptr(),
                )
                .map_err(|e| DecodeError::Ffmpeg {
                    code: 0,
                    message: format!("Failed to scale frame: {e}"),
                })?;

            // Copy timestamp
            (*dst_frame).pts = (*self.frame).pts;

            // Convert to VideoFrame
            let video_frame = self.av_frame_to_video_frame(dst_frame)?;

            // dst_frame is automatically freed when guard drops

            Ok(video_frame)
        }
    }
}
