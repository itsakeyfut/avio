//! Internal image encoder implementation.
//!
//! All `unsafe` FFmpeg calls are isolated here. The public API in `builder.rs`
//! is fully safe.
//!
//! ## Resource management
//!
//! [`ImageEncoderInner`] owns every FFmpeg resource allocated during a single
//! still-image encode. The destination frame, the packet, the codec context,
//! the scaling context, and the output format context are owned RAII values
//! ([`ff_sys::Frame`], [`ff_sys::Packet`], [`ff_sys::CodecContext`],
//! [`ff_sys::ScaleContext`], [`ff_sys::OutputFormatContext`]) that free
//! themselves on drop; the format context also closes its IO on drop. Because
//! drop runs on every exit path — including panics and early `?` returns — no
//! manual cleanup is needed at individual error sites.

// Rust 2024: Allow unsafe operations in unsafe functions for FFmpeg C API
#![allow(unsafe_code)]
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

use std::path::Path;

use ff_format::{PixelFormat, VideoFrame};
use ff_sys::{
    AVCodecID, AVCodecID_AV_CODEC_ID_BMP, AVCodecID_AV_CODEC_ID_MJPEG, AVCodecID_AV_CODEC_ID_PNG,
    AVCodecID_AV_CODEC_ID_TIFF, AVCodecID_AV_CODEC_ID_WEBP, AVColorRange_AVCOL_RANGE_JPEG,
    AVPixelFormat, AVPixelFormat_AV_PIX_FMT_BGR24, AVPixelFormat_AV_PIX_FMT_RGB24,
    AVPixelFormat_AV_PIX_FMT_YUV420P, AVRational, OutputFormatContext, swscale,
};

use crate::EncodeError;

/// Maximum number of planes in AVFrame data/linesize arrays.
const MAX_PLANES: usize = 8;

// ── Public options struct ─────────────────────────────────────────────────────

/// Options forwarded from the builder to the encoder.
pub(super) struct ImageEncodeOptions {
    /// Override output width (pixels). `None` → use source frame width.
    pub(super) width: Option<u32>,
    /// Override output height (pixels). `None` → use source frame height.
    pub(super) height: Option<u32>,
    /// Quality 0–100 (100 = best). `None` → codec default.
    pub(super) quality: Option<u32>,
    /// Output pixel format override. `None` → codec-native default.
    pub(super) pixel_format: Option<PixelFormat>,
}

// ── RAII wrapper ──────────────────────────────────────────────────────────────

/// Owns all FFmpeg resources for a single still-image encode operation.
///
/// The destination frame, packet, codec context, scaling context, and output
/// format context are owned RAII values that free themselves on drop, so no
/// early-return path leaks them.
struct ImageEncoderInner {
    /// Output format context (owned; frees itself and closes its IO on drop).
    format_ctx: OutputFormatContext,
    codec_ctx: ff_sys::CodecContext,
    dst_frame: ff_sys::Frame,
    packet: ff_sys::Packet,
    sws_ctx: Option<ff_sys::ScaleContext>,
    dst_width: u32,
    dst_height: u32,
    pix_fmt: AVPixelFormat,
}

impl ImageEncoderInner {
    /// Allocate all FFmpeg resources and open the encoder.
    ///
    /// On error the partially-initialised struct is dropped, which frees
    /// whatever was successfully allocated via the `Drop` impl.
    ///
    /// # Safety
    ///
    /// `path` must be a valid UTF-8 file path. `src` is used only to derive
    /// fallback dimensions when `opts` does not override them.
    unsafe fn open(
        path: &Path,
        opts: &ImageEncodeOptions,
        src: &VideoFrame,
    ) -> Result<Self, EncodeError> {
        let codec_id = codec_from_extension(path)?;
        let dst_width = opts.width.unwrap_or_else(|| src.width());
        let dst_height = opts.height.unwrap_or_else(|| src.height());
        let pix_fmt = opts
            .pixel_format
            .map_or_else(|| preferred_pix_fmt(codec_id), pixel_format_to_av);

        // Find the encoder and allocate the owned codec context, frame, packet,
        // and (below) format context up front so the struct holds them by value.
        let codec = ff_sys::Codec::find_encoder(codec_id).ok_or(EncodeError::UnsupportedCodec {
            codec: format!("codec_id={codec_id}"),
        })?;
        let codec_ctx = ff_sys::CodecContext::new(Some(codec))
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        let dst_frame =
            ff_sys::Frame::new().map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;
        let packet = ff_sys::Packet::new().map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;
        // ── Step 1: Output format context (owned) ─────────────────────────────
        // Prefer an explicit single-image muxer when one is known. Auto-detection
        // (NULL format name) resolves to the `image2` muxer for most still-image
        // formats, which expects a `%d` sequence pattern in the filename and emits
        // a cosmetic warning for ordinary names like "frame.jpg"; a dedicated
        // single-image muxer ("mjpeg", "apng", …) avoids that. If no explicit
        // muxer is known (e.g. BMP) or it is unavailable on a minimal FFmpeg
        // build, fall back to auto-detection. The owned context frees itself and
        // closes its IO on drop, so any early return below cannot leak it.
        let format_ctx = match codec_fallback_format(codec_id) {
            Some(fmt) => OutputFormatContext::new(Some(fmt), path)
                .or_else(|_| OutputFormatContext::new(None, path)),
            None => OutputFormatContext::new(None, path),
        }
        .map_err(|e| EncodeError::Ffmpeg {
            code: e.code(),
            message: format!(
                "Cannot create output context: {}",
                ff_sys::av_error_string(e.code())
            ),
        })?;

        let mut inner = Self {
            format_ctx,
            codec_ctx,
            dst_frame,
            packet,
            sws_ctx: None,
            dst_width,
            dst_height,
            pix_fmt,
        };

        // ── Step 2: Video stream ──────────────────────────────────────────────
        let stream_idx = inner
            .format_ctx
            .new_stream(None)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // ── Step 3: Configure codec context ──────────────────────────────────
        inner.codec_ctx.set_width(dst_width as i32);
        inner.codec_ctx.set_height(dst_height as i32);
        inner.codec_ctx.set_time_base(AVRational { num: 1, den: 1 });
        inner.codec_ctx.set_pix_fmt(pix_fmt);

        // For MJPEG, declare full-range (JPEG) color so FFmpeg does not emit
        // "deprecated pixel format used" warnings that appear when using the
        // deprecated YUVJ420P format. Using YUV420P + AVCOL_RANGE_JPEG is the
        // recommended replacement since FFmpeg 5.x.
        if codec_id == AVCodecID_AV_CODEC_ID_MJPEG {
            inner
                .codec_ctx
                .set_color_range(AVColorRange_AVCOL_RANGE_JPEG);
        }

        if let Some(q) = opts.quality {
            apply_quality(&mut inner.codec_ctx, codec_id, q);
        }

        // ── Step 4: Open codec ────────────────────────────────────────────────
        inner
            .codec_ctx
            .open_codec(codec)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // ── Step 5: Copy parameters to stream ─────────────────────────────────
        inner
            .format_ctx
            .apply_stream_params_from_context(stream_idx, &inner.codec_ctx)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // ── Step 8: Open output file ──────────────────────────────────────────
        inner
            .format_ctx
            .open_io(path)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // ── Step 9: Write file header ─────────────────────────────────────────
        inner
            .format_ctx
            .write_header()
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // ── Step 10: Configure destination frame and allocate its buffer ──────
        inner.dst_frame.set_format(pix_fmt);
        inner.dst_frame.set_width(dst_width as i32);
        inner.dst_frame.set_height(dst_height as i32);

        inner
            .dst_frame
            .get_buffer(0)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        Ok(inner)
    }

    /// Fill `dst_frame`, encode it, write all packets, and finalise the file.
    ///
    /// Writes the trailer and closes the IO context on success. On failure the
    /// `Drop` impl handles releasing the remaining FFmpeg resources.
    ///
    /// # Safety
    ///
    /// `self` must have been successfully opened via [`open`].
    unsafe fn encode_frame(&mut self, src: &VideoFrame) -> Result<(), EncodeError> {
        // ── Fill dst_frame ────────────────────────────────────────────────────
        let src_fmt = pixel_format_to_av(src.format());
        let needs_conversion = src_fmt != self.pix_fmt
            || src.width() != self.dst_width
            || src.height() != self.dst_height;

        if needs_conversion {
            // Store so Drop frees it if scale panics (RAII: frees on drop).
            self.sws_ctx = Some(
                ff_sys::ScaleContext::new(
                    src.width() as i32,
                    src.height() as i32,
                    src_fmt,
                    self.dst_width as i32,
                    self.dst_height as i32,
                    self.pix_fmt,
                    swscale::scale_flags::BILINEAR,
                )
                .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?,
            );

            let src_planes: Vec<&[u8]> = src.planes().iter().map(|p| p.data()).collect();
            let src_strides: Vec<i32> = src.strides().iter().map(|&s| s as i32).collect();

            let scale_result = self
                .sws_ctx
                .as_mut()
                .ok_or_else(|| EncodeError::Ffmpeg {
                    code: 0,
                    message: "Scaling context not initialized".to_string(),
                })?
                .scale_planes(
                    &src_planes,
                    &src_strides,
                    src.height() as i32,
                    &mut self.dst_frame,
                );

            // Drop the context now — single use; the owned field frees it.
            self.sws_ctx = None;

            scale_result.map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;
        } else {
            // Direct plane copy — same format and dimensions.
            for (i, plane) in src.planes().iter().enumerate() {
                if i >= MAX_PLANES {
                    break;
                }
                let src_stride = src.strides()[i];
                let plane_data = plane.data();
                // The destination stride is the frame's own linesize for plane i.
                let dst_stride = self.dst_frame.linesize(i) as usize;
                // `video_plane_mut` yields `None` for an absent plane (null data),
                // matching the previous null-plane break.
                let Some(dst_plane) = self.dst_frame.video_plane_mut(i) else {
                    break;
                };

                if src_stride == dst_stride {
                    let n = plane_data.len().min(dst_plane.len());
                    dst_plane[..n].copy_from_slice(&plane_data[..n]);
                } else {
                    let row_bytes = src_stride.min(dst_stride);
                    let num_rows = plane_data.len() / src_stride;
                    for row in 0..num_rows {
                        let src_off = row * src_stride;
                        let dst_off = row * dst_stride;
                        dst_plane[dst_off..dst_off + row_bytes]
                            .copy_from_slice(&plane_data[src_off..src_off + row_bytes]);
                    }
                }
            }
        }

        self.dst_frame.set_pts(0);

        // ── Send frame → encoder ──────────────────────────────────────────────
        self.codec_ctx
            .send_frame(Some(&self.dst_frame))
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // ── Receive packets ───────────────────────────────────────────────────
        self.drain_packets(false)?;

        // ── Flush encoder ─────────────────────────────────────────────────────
        self.codec_ctx
            .send_frame(None)
            .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?;

        // ── Drain remaining packets ───────────────────────────────────────────
        self.drain_packets(true)?;

        // ── Finalise file ─────────────────────────────────────────────────────
        // Preserve the prior behaviour of not surfacing a trailer error here.
        let _ = self.format_ctx.write_trailer();
        // Close the IO now (nulls `pb`) so the later drop does not double-close.
        self.format_ctx.close_io();

        Ok(())
    }

    /// Drain encoded packets from the codec and write them to the container.
    ///
    /// When `until_eof` is `true` the loop continues until `AVERROR_EOF`;
    /// when `false` it also stops on `AVERROR(EAGAIN)` (no more packets yet).
    ///
    /// # Safety
    ///
    /// `self.codec_ctx`, `self.packet`, and `self.format_ctx` must all be valid.
    unsafe fn drain_packets(&mut self, until_eof: bool) -> Result<(), EncodeError> {
        loop {
            match self
                .codec_ctx
                .receive_packet(&mut self.packet)
                .map_err(|e| EncodeError::from_ffmpeg_error(e.code()))?
            {
                ff_sys::ReceiveOutcome::Frame => {
                    self.packet.set_stream_index(0);
                    let ret = self.format_ctx.write_interleaved(&mut self.packet);
                    self.packet.unref();
                    if let Err(e) = ret {
                        return Err(EncodeError::from_ffmpeg_error(e.code()));
                    }
                }
                ff_sys::ReceiveOutcome::Drained => break,
                ff_sys::ReceiveOutcome::NeedInput => {
                    if !until_eof {
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

// ── Extension / format helpers ────────────────────────────────────────────────

/// Return the `AVCodecID` for the given file extension.
///
/// This is `pub(super)` so `builder.rs` can call it for early validation.
pub(super) fn codec_from_extension(path: &Path) -> Result<AVCodecID, EncodeError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "jpg" | "jpeg" => Ok(AVCodecID_AV_CODEC_ID_MJPEG),
        "png" => Ok(AVCodecID_AV_CODEC_ID_PNG),
        "bmp" => Ok(AVCodecID_AV_CODEC_ID_BMP),
        "tif" | "tiff" => Ok(AVCodecID_AV_CODEC_ID_TIFF),
        "webp" => Ok(AVCodecID_AV_CODEC_ID_WEBP),
        "" => Err(EncodeError::InvalidConfig {
            reason: "no file extension".to_string(),
        }),
        e => Err(EncodeError::UnsupportedCodec {
            codec: e.to_string(),
        }),
    }
}

/// Return a codec-specific fallback muxer name for use when filename-based
/// format detection fails (e.g. for numeric filenames like `thumb_0000.jpg`).
///
/// These short names refer to dedicated single-image muxers that do not
/// perform image-sequence pattern validation and are present in all standard
/// FFmpeg builds.  Returns `None` for codecs whose primary muxer is `image2`
/// and for which no dedicated alternative is commonly available.
fn codec_fallback_format(codec_id: AVCodecID) -> Option<&'static str> {
    // Use if/else rather than match to avoid the non_upper_case_globals lint
    // that fires when bindgen-generated constants appear in pattern position.
    if codec_id == AVCodecID_AV_CODEC_ID_MJPEG {
        Some("mjpeg")
    } else if codec_id == AVCodecID_AV_CODEC_ID_PNG {
        Some("apng")
    } else if codec_id == AVCodecID_AV_CODEC_ID_TIFF {
        Some("tiff")
    } else if codec_id == AVCodecID_AV_CODEC_ID_WEBP {
        Some("webp")
    } else {
        None
    }
}

/// Return the preferred `AVPixelFormat` for the given codec.
fn preferred_pix_fmt(codec_id: AVCodecID) -> AVPixelFormat {
    match codec_id {
        // Use YUV420P + AVCOL_RANGE_JPEG (set in open()) instead of the
        // deprecated YUVJ420P alias to avoid "deprecated pixel format" warnings.
        x if x == AVCodecID_AV_CODEC_ID_MJPEG => AVPixelFormat_AV_PIX_FMT_YUV420P,
        x if x == AVCodecID_AV_CODEC_ID_PNG => AVPixelFormat_AV_PIX_FMT_RGB24,
        x if x == AVCodecID_AV_CODEC_ID_BMP => AVPixelFormat_AV_PIX_FMT_BGR24,
        x if x == AVCodecID_AV_CODEC_ID_TIFF => AVPixelFormat_AV_PIX_FMT_RGB24,
        x if x == AVCodecID_AV_CODEC_ID_WEBP => AVPixelFormat_AV_PIX_FMT_YUV420P,
        _ => AVPixelFormat_AV_PIX_FMT_RGB24,
    }
}

/// Map a `PixelFormat` enum value to the corresponding `AVPixelFormat` constant.
fn pixel_format_to_av(fmt: PixelFormat) -> AVPixelFormat {
    match fmt {
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
        PixelFormat::P010le => ff_sys::AVPixelFormat_AV_PIX_FMT_P010LE,
        _ => ff_sys::AVPixelFormat_AV_PIX_FMT_RGB24,
    }
}

// ── Quality helper ────────────────────────────────────────────────────────────

/// Apply a quality value (0–100, 100 = best) to the codec context.
///
/// Must be called after the codec context fields are set but before
/// `avcodec_open2`.
fn apply_quality(codec_ctx: &mut ff_sys::CodecContext, codec_id: AVCodecID, quality: u32) {
    let q = quality.min(100);

    if codec_id == AVCodecID_AV_CODEC_ID_MJPEG {
        // Map 0–100 (100 = best) → MJPEG qscale 1–31 (1 = best, 31 = worst).
        let qscale = (1 + (100 - q) * 30 / 100) as i32;
        codec_ctx.set_qmin(qscale);
        codec_ctx.set_qmax(qscale);
        log::info!("MJPEG quality applied quality={q} qscale={qscale}");
    } else if codec_id == AVCodecID_AV_CODEC_ID_PNG {
        // Map 0–100 → compression_level 0–9 (9 = maximum compression).
        let level = q * 9 / 100;
        if let Err(e) = codec_ctx.set_opt("compression_level", &level.to_string()) {
            log::warn!(
                "av_opt_set compression_level failed, ignoring \
                 quality={q} error={}",
                ff_sys::av_error_string(e.code())
            );
        } else {
            log::info!("PNG compression_level applied quality={q} level={level}");
        }
    } else if codec_id == AVCodecID_AV_CODEC_ID_WEBP {
        // Direct 0–100 mapping for WebP quality.
        if let Err(e) = codec_ctx.set_opt("quality", &q.to_string()) {
            log::warn!(
                "av_opt_set quality failed for WebP, ignoring \
                 quality={q} error={}",
                ff_sys::av_error_string(e.code())
            );
        } else {
            log::info!("WebP quality applied quality={q}");
        }
    } else {
        // BMP and TIFF have no quality concept; any other codec is unrecognised.
        let fmt_name = if codec_id == AVCodecID_AV_CODEC_ID_BMP {
            "bmp"
        } else if codec_id == AVCodecID_AV_CODEC_ID_TIFF {
            "tiff"
        } else {
            "this format"
        };
        log::warn!("quality option has no effect for {fmt_name} images, ignoring quality={q}");
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Encode a single `VideoFrame` and write it to `path`.
///
/// Resources are managed via [`ImageEncoderInner`]'s [`Drop`] implementation,
/// which frees frame → packet → sws_ctx → format_ctx (the owned `codec_ctx`
/// drops itself last) regardless of whether encoding succeeds or fails.
///
pub(super) fn encode_image(
    path: &Path,
    frame: &VideoFrame,
    opts: &ImageEncodeOptions,
) -> Result<(), EncodeError> {
    // SAFETY: ImageEncoderInner::open and encode_frame exclusively own all
    // FFmpeg resources; Drop frees them on every exit path.
    unsafe {
        ff_sys::ensure_initialized();

        // Open the encoder; any error here drops `inner` (partially initialised),
        // which frees whatever was allocated so far.
        let mut inner = ImageEncoderInner::open(path, opts, frame)?;

        // Encode and finalise the file; on error `inner` is dropped here via `?`,
        // releasing all remaining FFmpeg resources.
        inner.encode_frame(frame)?;

        log::info!(
            "Image encoded successfully path={} src={}x{} dst={}x{}",
            path.display(),
            frame.width(),
            frame.height(),
            inner.dst_width,
            inner.dst_height,
        );

        Ok(())
    } // unsafe
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn codec_from_extension_jpeg_should_return_mjpeg() {
        let id = codec_from_extension(Path::new("img.jpg")).unwrap();
        assert_eq!(id, AVCodecID_AV_CODEC_ID_MJPEG);
    }

    #[test]
    fn codec_from_extension_jpeg_alias_should_return_mjpeg() {
        let id = codec_from_extension(Path::new("img.jpeg")).unwrap();
        assert_eq!(id, AVCodecID_AV_CODEC_ID_MJPEG);
    }

    #[test]
    fn codec_from_extension_png_should_return_png() {
        let id = codec_from_extension(Path::new("img.PNG")).unwrap(); // upper-case
        assert_eq!(id, AVCodecID_AV_CODEC_ID_PNG);
    }

    #[test]
    fn codec_from_extension_bmp_should_return_bmp() {
        let id = codec_from_extension(Path::new("img.bmp")).unwrap();
        assert_eq!(id, AVCodecID_AV_CODEC_ID_BMP);
    }

    #[test]
    fn codec_from_extension_tif_should_return_tiff() {
        let id = codec_from_extension(Path::new("img.tif")).unwrap();
        assert_eq!(id, AVCodecID_AV_CODEC_ID_TIFF);
    }

    #[test]
    fn codec_from_extension_tiff_should_return_tiff() {
        let id = codec_from_extension(Path::new("img.tiff")).unwrap();
        assert_eq!(id, AVCodecID_AV_CODEC_ID_TIFF);
    }

    #[test]
    fn codec_from_extension_webp_should_return_webp() {
        let id = codec_from_extension(Path::new("img.webp")).unwrap();
        assert_eq!(id, AVCodecID_AV_CODEC_ID_WEBP);
    }

    #[test]
    fn codec_from_extension_no_ext_should_return_invalid_config() {
        let result = codec_from_extension(Path::new("no_extension"));
        assert!(matches!(result, Err(EncodeError::InvalidConfig { .. })));
    }

    #[test]
    fn codec_from_extension_unknown_should_return_unsupported_codec() {
        let result = codec_from_extension(Path::new("img.avi"));
        assert!(matches!(result, Err(EncodeError::UnsupportedCodec { .. })));
    }

    #[test]
    fn preferred_pix_fmt_mjpeg_should_return_yuv420p() {
        // Uses YUV420P (not the deprecated YUVJ420P); color range is set
        // separately via color_range = AVCOL_RANGE_JPEG in open().
        assert_eq!(
            preferred_pix_fmt(AVCodecID_AV_CODEC_ID_MJPEG),
            AVPixelFormat_AV_PIX_FMT_YUV420P
        );
    }

    #[test]
    fn preferred_pix_fmt_png_should_return_rgb24() {
        assert_eq!(
            preferred_pix_fmt(AVCodecID_AV_CODEC_ID_PNG),
            AVPixelFormat_AV_PIX_FMT_RGB24
        );
    }

    #[test]
    fn preferred_pix_fmt_bmp_should_return_bgr24() {
        assert_eq!(
            preferred_pix_fmt(AVCodecID_AV_CODEC_ID_BMP),
            AVPixelFormat_AV_PIX_FMT_BGR24
        );
    }

    #[test]
    fn preferred_pix_fmt_webp_should_return_yuv420p() {
        assert_eq!(
            preferred_pix_fmt(AVCodecID_AV_CODEC_ID_WEBP),
            AVPixelFormat_AV_PIX_FMT_YUV420P
        );
    }

    #[test]
    fn pixel_format_to_av_yuv420p_should_match() {
        assert_eq!(
            pixel_format_to_av(PixelFormat::Yuv420p),
            AVPixelFormat_AV_PIX_FMT_YUV420P
        );
    }

    #[test]
    fn pixel_format_to_av_rgb24_should_match() {
        assert_eq!(
            pixel_format_to_av(PixelFormat::Rgb24),
            AVPixelFormat_AV_PIX_FMT_RGB24
        );
    }

    // Verify Drop does not panic on an inner struct whose output context was
    // allocated but never opened (`open_io`) — the early-return path when `open`
    // fails after allocating the context but before writing the header. The owned
    // context must free itself with a null `pb` without closing anything.
    #[test]
    fn drop_with_unopened_context_should_not_panic() {
        // Allocate the output context first; skip gracefully when no still-image
        // muxer is available (CI's minimal FFmpeg is built without image2/png).
        let Ok(format_ctx) = OutputFormatContext::new(None, std::path::Path::new("dummy.png"))
        else {
            return;
        };
        // All owned fields free themselves on drop; `format_ctx` has no `pb`
        // opened, so its drop frees the context without closing an IO.
        let inner = ImageEncoderInner {
            format_ctx,
            codec_ctx: ff_sys::CodecContext::new(None).expect("generic context alloc"),
            dst_frame: ff_sys::Frame::new().expect("frame alloc"),
            packet: ff_sys::Packet::new().expect("packet alloc"),
            sws_ctx: None,
            dst_width: 0,
            dst_height: 0,
            pix_fmt: ff_sys::AVPixelFormat_AV_PIX_FMT_NONE,
        };
        drop(inner); // must not panic
    }

    // Verify codec_from_extension is case-insensitive (uses .to_lowercase()).
    #[test]
    fn codec_from_extension_case_insensitive_should_work() {
        let _ = codec_from_extension(&PathBuf::from("IMG.JPG")).unwrap();
        let _ = codec_from_extension(&PathBuf::from("IMG.BMP")).unwrap();
        let _ = codec_from_extension(&PathBuf::from("IMG.WEBP")).unwrap();
    }
}
