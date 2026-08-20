//! Internal image encoder implementation.
//!
//! All `unsafe` FFmpeg calls are isolated here. The public API in `builder.rs`
//! is fully safe.
//!
//! ## Resource management
//!
//! [`ImageEncoderInner`] owns every FFmpeg pointer allocated during a single
//! still-image encode. Its [`Drop`] implementation frees them in the order
//! mandated by FFmpeg: frame → packet → sws_ctx → codec_ctx → format_ctx.
//! Because `Drop` runs on every exit path — including panics and early `?`
//! returns — no manual cleanup is needed at individual error sites.

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

use std::ffi::CString;
use std::path::Path;
use std::ptr;

use ff_format::{PixelFormat, VideoFrame};
use ff_sys::{
    AVCodecID, AVCodecID_AV_CODEC_ID_BMP, AVCodecID_AV_CODEC_ID_MJPEG, AVCodecID_AV_CODEC_ID_PNG,
    AVCodecID_AV_CODEC_ID_TIFF, AVCodecID_AV_CODEC_ID_WEBP, AVColorRange_AVCOL_RANGE_JPEG,
    AVFormatContext, AVPixelFormat, AVPixelFormat_AV_PIX_FMT_BGR24, AVPixelFormat_AV_PIX_FMT_RGB24,
    AVPixelFormat_AV_PIX_FMT_YUV420P, AVRational, av_frame_alloc, av_frame_free,
    av_interleaved_write_frame, av_packet_alloc, av_packet_free, av_packet_unref, av_write_trailer,
    avcodec, avformat, avformat_alloc_output_context2, avformat_free_context, avformat_new_stream,
    avformat_write_header, swscale,
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
/// Every field is initialised to null/`None`. Resources are set as they are
/// successfully allocated so that `Drop` only frees what actually exists.
///
/// # Drop contract
///
/// Resources are released in the following order to satisfy FFmpeg's lifetime
/// requirements:
///
/// 1. `dst_frame` — `av_frame_free`
/// 2. `packet`    — `av_packet_free`
/// 3. `sws_ctx`   — `sws_freeContext`
/// 4. `codec_ctx` — `avcodec_free_context`
/// 5. `format_ctx`— IO close (`avio_closep`) then `avformat_free_context`
struct ImageEncoderInner {
    format_ctx: *mut AVFormatContext,
    codec_ctx: *mut ff_sys::AVCodecContext,
    dst_frame: *mut ff_sys::AVFrame,
    packet: *mut ff_sys::AVPacket,
    sws_ctx: Option<*mut ff_sys::SwsContext>,
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

        // Start with everything null so Drop is safe from the very first field.
        let mut inner = Self {
            format_ctx: ptr::null_mut(),
            codec_ctx: ptr::null_mut(),
            dst_frame: ptr::null_mut(),
            packet: ptr::null_mut(),
            sws_ctx: None,
            dst_width,
            dst_height,
            pix_fmt,
        };

        // ── Step 1: Output format context ─────────────────────────────────────
        let c_path = CString::new(path.to_str().ok_or_else(|| EncodeError::CannotCreateFile {
            path: path.to_path_buf(),
        })?)
        .map_err(|_| EncodeError::CannotCreateFile {
            path: path.to_path_buf(),
        })?;

        // Prefer an explicit muxer name when one is available.
        //
        // The auto-detection path (NULL format name) resolves to the `image2`
        // muxer for most still-image formats.  `image2` expects filenames that
        // contain a `%d` sequence-number pattern and emits a cosmetic warning:
        //   "[image2 @ …] The specified filename '…' does not contain an image
        //    sequence pattern"
        // for any ordinary name like "frame.jpg".  Using a dedicated single-image
        // muxer ("mjpeg", "apng", …) avoids that warning entirely.
        //
        // If no explicit muxer is known (e.g. BMP), or if the explicit muxer
        // fails for any reason, we fall back to auto-detection.
        let explicit_fmt = codec_fallback_format(codec_id);

        let mut ret = if let Some(fmt) = explicit_fmt {
            avformat_alloc_output_context2(
                &raw mut inner.format_ctx,
                ptr::null_mut(),
                fmt,
                c_path.as_ptr(),
            )
        } else {
            avformat_alloc_output_context2(
                &raw mut inner.format_ctx,
                ptr::null_mut(),
                ptr::null(),
                c_path.as_ptr(),
            )
        };

        // Fallback to auto-detection if the explicit muxer was unavailable or
        // failed (e.g. on a minimal FFmpeg build that omits the dedicated muxer).
        if ret < 0 || inner.format_ctx.is_null() {
            ret = avformat_alloc_output_context2(
                &raw mut inner.format_ctx,
                ptr::null_mut(),
                ptr::null(),
                c_path.as_ptr(),
            );
        }

        if ret < 0 || inner.format_ctx.is_null() {
            return Err(EncodeError::Ffmpeg {
                code: ret,
                message: format!(
                    "Cannot create output context: {}",
                    ff_sys::av_error_string(ret)
                ),
            });
        }

        // ── Step 2: Video stream ──────────────────────────────────────────────
        let stream = avformat_new_stream(inner.format_ctx, ptr::null());
        if stream.is_null() {
            return Err(EncodeError::Ffmpeg {
                code: 0,
                message: "Cannot create output stream".to_string(),
            });
        }

        // ── Step 3: Find encoder ──────────────────────────────────────────────
        let codec = avcodec::find_encoder(codec_id).ok_or(EncodeError::UnsupportedCodec {
            codec: format!("codec_id={codec_id}"),
        })?;

        // ── Step 4: Allocate codec context ────────────────────────────────────
        inner.codec_ctx = avcodec::alloc_context3(codec).map_err(EncodeError::from_ffmpeg_error)?;

        // ── Step 5: Configure codec context ──────────────────────────────────
        (*inner.codec_ctx).width = dst_width as i32;
        (*inner.codec_ctx).height = dst_height as i32;
        (*inner.codec_ctx).time_base = AVRational { num: 1, den: 1 };
        (*inner.codec_ctx).pix_fmt = pix_fmt;

        // For MJPEG, declare full-range (JPEG) color so FFmpeg does not emit
        // "deprecated pixel format used" warnings that appear when using the
        // deprecated YUVJ420P format. Using YUV420P + AVCOL_RANGE_JPEG is the
        // recommended replacement since FFmpeg 5.x.
        if codec_id == AVCodecID_AV_CODEC_ID_MJPEG {
            // SAFETY: codec_ctx is non-null; color_range is a plain integer field.
            (*inner.codec_ctx).color_range = AVColorRange_AVCOL_RANGE_JPEG;
        }

        if let Some(q) = opts.quality {
            // SAFETY: codec_ctx is non-null and freshly allocated.
            apply_quality(inner.codec_ctx, codec_id, q);
        }

        // ── Step 6: Open codec ────────────────────────────────────────────────
        avcodec::open2(inner.codec_ctx, codec, ptr::null_mut())
            .map_err(EncodeError::from_ffmpeg_error)?;

        // ── Step 7: Copy parameters to stream ─────────────────────────────────
        // SAFETY: stream is non-null (checked above); codec_ctx is open.
        let par = (*stream).codecpar;
        (*par).codec_id = codec_id;
        (*par).codec_type = ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO;
        (*par).width = (*inner.codec_ctx).width;
        (*par).height = (*inner.codec_ctx).height;
        (*par).format = pix_fmt;

        // ── Step 8: Open output file ──────────────────────────────────────────
        let io_ctx = avformat::open_output(path, avformat::avio_flags::WRITE)
            .map_err(EncodeError::from_ffmpeg_error)?;
        (*inner.format_ctx).pb = io_ctx;

        // ── Step 9: Write file header ─────────────────────────────────────────
        let ret = avformat_write_header(inner.format_ctx, ptr::null_mut());
        if ret < 0 {
            return Err(EncodeError::from_ffmpeg_error(ret));
        }

        // ── Step 10: Allocate destination frame ───────────────────────────────
        inner.dst_frame = av_frame_alloc();
        if inner.dst_frame.is_null() {
            return Err(EncodeError::Ffmpeg {
                code: 0,
                message: "Cannot allocate destination frame".to_string(),
            });
        }
        (*inner.dst_frame).format = pix_fmt;
        (*inner.dst_frame).width = dst_width as i32;
        (*inner.dst_frame).height = dst_height as i32;

        let ret = ff_sys::av_frame_get_buffer(inner.dst_frame, 0);
        if ret < 0 {
            return Err(EncodeError::from_ffmpeg_error(ret));
        }

        // ── Step 11: Allocate packet ──────────────────────────────────────────
        inner.packet = av_packet_alloc();
        if inner.packet.is_null() {
            return Err(EncodeError::Ffmpeg {
                code: 0,
                message: "Cannot allocate packet".to_string(),
            });
        }

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
            let sws_ctx = swscale::get_context(
                src.width() as i32,
                src.height() as i32,
                src_fmt,
                self.dst_width as i32,
                self.dst_height as i32,
                self.pix_fmt,
                swscale::scale_flags::BILINEAR,
            )
            .map_err(EncodeError::from_ffmpeg_error)?;

            // Store so Drop frees it if scale panics.
            self.sws_ctx = Some(sws_ctx);

            let mut src_data: [*const u8; MAX_PLANES] = [ptr::null(); MAX_PLANES];
            let mut src_linesize: [i32; MAX_PLANES] = [0; MAX_PLANES];
            for (i, plane) in src.planes().iter().enumerate() {
                if i < MAX_PLANES {
                    src_data[i] = plane.data().as_ptr();
                    src_linesize[i] = src.strides()[i] as i32;
                }
            }

            let scale_result = swscale::scale(
                sws_ctx,
                src_data.as_ptr(),
                src_linesize.as_ptr(),
                0,
                src.height() as i32,
                (*self.dst_frame).data.as_mut_ptr().cast_const(),
                (*self.dst_frame).linesize.as_mut_ptr(),
            );

            // Free immediately — single use; Drop handles null case.
            if let Some(sws) = self.sws_ctx.take() {
                swscale::free_context(sws);
            }

            scale_result.map_err(EncodeError::from_ffmpeg_error)?;
        } else {
            // Direct plane copy — same format and dimensions.
            for (i, plane) in src.planes().iter().enumerate() {
                if i >= MAX_PLANES || (*self.dst_frame).data[i].is_null() {
                    break;
                }
                let src_stride = src.strides()[i];
                let dst_stride = (*self.dst_frame).linesize[i] as usize;
                let plane_data = plane.data();

                if src_stride == dst_stride {
                    std::ptr::copy_nonoverlapping(
                        plane_data.as_ptr(),
                        (*self.dst_frame).data[i],
                        plane_data.len(),
                    );
                } else {
                    let row_bytes = src_stride.min(dst_stride);
                    let num_rows = plane_data.len() / src_stride;
                    for row in 0..num_rows {
                        std::ptr::copy_nonoverlapping(
                            plane_data[row * src_stride..].as_ptr(),
                            (*self.dst_frame).data[i].add(row * dst_stride),
                            row_bytes,
                        );
                    }
                }
            }
        }

        (*self.dst_frame).pts = 0;

        // ── Send frame → encoder ──────────────────────────────────────────────
        avcodec::send_frame(self.codec_ctx, self.dst_frame)
            .map_err(EncodeError::from_ffmpeg_error)?;

        // ── Receive packets ───────────────────────────────────────────────────
        self.drain_packets(false)?;

        // ── Flush encoder ─────────────────────────────────────────────────────
        avcodec::send_frame(self.codec_ctx, ptr::null()).map_err(EncodeError::from_ffmpeg_error)?;

        // ── Drain remaining packets ───────────────────────────────────────────
        self.drain_packets(true)?;

        // ── Finalise file ─────────────────────────────────────────────────────
        av_write_trailer(self.format_ctx);
        // SAFETY: format_ctx and pb are non-null at this point.
        avformat::close_output(&raw mut (*self.format_ctx).pb);

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
            match avcodec::receive_packet(self.codec_ctx, self.packet) {
                Ok(()) => {
                    (*self.packet).stream_index = 0;
                    let ret = av_interleaved_write_frame(self.format_ctx, self.packet);
                    av_packet_unref(self.packet);
                    if ret < 0 {
                        return Err(EncodeError::from_ffmpeg_error(ret));
                    }
                }
                Err(e) if e == ff_sys::error_codes::EOF => break,
                Err(e) if !until_eof && e == ff_sys::error_codes::EAGAIN => break,
                Err(e) => return Err(EncodeError::from_ffmpeg_error(e)),
            }
        }
        Ok(())
    }
}

impl Drop for ImageEncoderInner {
    fn drop(&mut self) {
        // SAFETY: Every pointer was allocated by the FFmpeg API and is either
        // null (never allocated, or already freed) or a valid owned allocation.
        // We check for null before each free to make Drop idempotent.
        //
        // Release order per the issue #154 Drop contract:
        //   1. dst_frame  — av_frame_free  (sets pointer to null)
        //   2. packet     — av_packet_free (sets pointer to null)
        //   3. sws_ctx    — sws_freeContext
        //   4. codec_ctx  — avcodec_free_context (sets pointer to null)
        //   5. format_ctx — avio_closep (if pb still open) + avformat_free_context
        unsafe {
            if !self.dst_frame.is_null() {
                // SAFETY: dst_frame is non-null and owned by this struct.
                av_frame_free(&raw mut self.dst_frame);
            }
            if !self.packet.is_null() {
                // SAFETY: packet is non-null and owned by this struct.
                av_packet_free(&raw mut self.packet);
            }
            if let Some(sws) = self.sws_ctx.take() {
                // SAFETY: sws is a valid SwsContext that hasn't been freed yet.
                swscale::free_context(sws);
            }
            if !self.codec_ctx.is_null() {
                // SAFETY: codec_ctx is non-null and owned by this struct.
                // avcodec_free_context sets the pointer to null after freeing.
                avcodec::free_context(&raw mut self.codec_ctx);
            }
            if !self.format_ctx.is_null() {
                // SAFETY: format_ctx is non-null and owned by this struct.
                // Close the IO context if it hasn't been closed yet (it is set
                // to null by avio_closep, so this check prevents a double-close
                // when encode_frame already closed it on success).
                if !(*self.format_ctx).pb.is_null() {
                    avformat::close_output(&raw mut (*self.format_ctx).pb);
                }
                avformat_free_context(self.format_ctx);
                self.format_ctx = ptr::null_mut();
            }
        }
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
fn codec_fallback_format(codec_id: AVCodecID) -> Option<*const std::os::raw::c_char> {
    // Use if/else rather than match to avoid the non_upper_case_globals lint
    // that fires when bindgen-generated constants appear in pattern position.
    if codec_id == AVCodecID_AV_CODEC_ID_MJPEG {
        Some(c"mjpeg".as_ptr())
    } else if codec_id == AVCodecID_AV_CODEC_ID_PNG {
        Some(c"apng".as_ptr())
    } else if codec_id == AVCodecID_AV_CODEC_ID_TIFF {
        Some(c"tiff".as_ptr())
    } else if codec_id == AVCodecID_AV_CODEC_ID_WEBP {
        Some(c"webp".as_ptr())
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
///
/// # Safety
///
/// `codec_ctx` must be a valid, non-null pointer to an allocated
/// `AVCodecContext` whose `priv_data` is valid (guaranteed after
/// `avcodec_alloc_context3`).
unsafe fn apply_quality(codec_ctx: *mut ff_sys::AVCodecContext, codec_id: AVCodecID, quality: u32) {
    let q = quality.min(100);

    if codec_id == AVCodecID_AV_CODEC_ID_MJPEG {
        // Map 0–100 (100 = best) → MJPEG qscale 1–31 (1 = best, 31 = worst).
        let qscale = (1 + (100 - q) * 30 / 100) as i32;
        (*codec_ctx).qmin = qscale;
        (*codec_ctx).qmax = qscale;
        log::info!("MJPEG quality applied quality={q} qscale={qscale}");
    } else if codec_id == AVCodecID_AV_CODEC_ID_PNG {
        // Map 0–100 → compression_level 0–9 (9 = maximum compression).
        let level = q * 9 / 100;
        if (*codec_ctx).priv_data.is_null() {
            log::warn!("PNG compression_level: priv_data is null, skipping quality={q}");
            return;
        }
        let Ok(key) = CString::new("compression_level") else {
            return;
        };
        let Ok(val) = CString::new(level.to_string()) else {
            return;
        };
        // SAFETY: priv_data is non-null; key/val are valid NUL-terminated strings.
        let ret = ff_sys::av_opt_set((*codec_ctx).priv_data, key.as_ptr(), val.as_ptr(), 0);
        if ret < 0 {
            log::warn!(
                "av_opt_set compression_level failed, ignoring \
                 quality={q} error={}",
                ff_sys::av_error_string(ret)
            );
        } else {
            log::info!("PNG compression_level applied quality={q} level={level}");
        }
    } else if codec_id == AVCodecID_AV_CODEC_ID_WEBP {
        // Direct 0–100 mapping for WebP quality.
        if (*codec_ctx).priv_data.is_null() {
            log::warn!("WebP quality: priv_data is null, skipping quality={q}");
            return;
        }
        let Ok(key) = CString::new("quality") else {
            return;
        };
        let Ok(val) = CString::new(q.to_string()) else {
            return;
        };
        // SAFETY: priv_data is non-null; key/val are valid NUL-terminated strings.
        let ret = ff_sys::av_opt_set((*codec_ctx).priv_data, key.as_ptr(), val.as_ptr(), 0);
        if ret < 0 {
            log::warn!(
                "av_opt_set quality failed for WebP, ignoring \
                 quality={q} error={}",
                ff_sys::av_error_string(ret)
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
/// which frees frame → packet → sws_ctx → codec_ctx → format_ctx regardless
/// of whether encoding succeeds or fails.
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

    // Verify Drop does not panic on a zero-initialised (all-null) inner struct.
    // This guards the partial-allocation cleanup path exercised when `open`
    // returns early with an error before every field is set.
    #[test]
    fn drop_on_uninitialised_inner_should_not_panic() {
        // We deliberately construct an all-null inner and drop it.
        // SAFETY: all pointers are null; Drop checks for null before freeing.
        let inner = ImageEncoderInner {
            format_ctx: ptr::null_mut(),
            codec_ctx: ptr::null_mut(),
            dst_frame: ptr::null_mut(),
            packet: ptr::null_mut(),
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
