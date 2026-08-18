//! Unsafe FFmpeg filter graph calls for preview generation.
//!
//! All `unsafe` code is isolated here; [`super`] exposes safe wrappers.
//!
//! Entry points:
//! - [`generate_sprite_sheet_unsafe`] — filter graph + PNG encode for sprite sheets
//! - [`generate_gif_preview_unsafe`]  — two-pass palettegen + GIF encode

#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::CString;
use std::path::Path;
use std::ptr;
use std::time::Duration;

use ff_sys::{
    AVCodecID_AV_CODEC_ID_GIF, AVCodecID_AV_CODEC_ID_PNG, AVPixelFormat_AV_PIX_FMT_RGB24,
    AVRational, av_buffersink_get_frame, av_frame_alloc, av_frame_free, av_frame_get_buffer,
    av_interleaved_write_frame, av_opt_set, av_packet_alloc, av_packet_free, av_packet_unref,
    av_write_trailer, avcodec, avfilter_get_by_name, avfilter_graph_alloc, avfilter_graph_config,
    avfilter_graph_create_filter, avfilter_graph_free, avfilter_link, avformat,
    avformat_alloc_output_context2, avformat_free_context, avformat_new_stream,
    avformat_write_header, swscale,
};

use crate::PreviewImageError;

/// Probes the video at `path` and returns its duration in seconds.
///
/// # Safety
///
/// All FFmpeg pointers are null-checked and freed on every exit path.
unsafe fn probe_video_duration_secs(path: &Path) -> Result<f64, PreviewImageError> {
    let fmt_ctx = avformat::open_input(path).map_err(PreviewImageError::from_ffmpeg_error)?;

    if let Err(e) = avformat::find_stream_info(fmt_ctx) {
        let mut p = fmt_ctx;
        avformat::close_input(&mut p);
        return Err(PreviewImageError::from_ffmpeg_error(e));
    }

    // SAFETY: fmt_ctx is non-null (open_input succeeded).
    let duration_av = (*fmt_ctx).duration;
    let mut p = fmt_ctx;
    avformat::close_input(&mut p);

    if duration_av <= 0 {
        return Err(PreviewImageError::OperationFailed {
            reason: "cannot determine video duration".to_string(),
        });
    }

    // AV_TIME_BASE = 1_000_000 (microseconds); precision loss is acceptable for duration.
    #[allow(clippy::cast_precision_loss)]
    let secs = duration_av as f64 / 1_000_000.0;
    Ok(secs)
}

/// Generates a sprite sheet PNG from `input`, writing to `output`.
///
/// Filter chain:
/// `movie=filename={input} → fps={N}/{duration} → scale={fw}:{fh} →
///  tile={cols}x{rows}:padding=0:margin=0 → buffersink`
///
/// The `tile` filter accumulates `cols * rows` frames and emits one composite
/// frame, which is then encoded as PNG.
///
/// # Safety
///
/// All raw pointer operations follow avfilter and avcodec ownership rules.
/// Every allocation is freed on every exit path via the `bail!` macro or
/// explicit cleanup at the end of the function.
/// Safe entry point called from [`super`]; all `unsafe` is confined here.
pub(super) fn generate_sprite_sheet(
    input: &Path,
    cols: u32,
    rows: u32,
    frame_width: u32,
    frame_height: u32,
    output: &Path,
) -> Result<(), PreviewImageError> {
    // SAFETY: generate_sprite_sheet_unsafe manages all raw pointer lifetimes
    //         per avfilter and avcodec ownership rules.
    unsafe { generate_sprite_sheet_unsafe(input, cols, rows, frame_width, frame_height, output) }
}

unsafe fn generate_sprite_sheet_unsafe(
    input: &Path,
    cols: u32,
    rows: u32,
    frame_width: u32,
    frame_height: u32,
    output: &Path,
) -> Result<(), PreviewImageError> {
    // ── Step 1: probe duration ────────────────────────────────────────────────
    let duration_secs = probe_video_duration_secs(input)?;
    let n = cols * rows;
    // FPS needed to sample exactly N frames across the full duration.
    let fps_arg = format!("{n}/{duration_secs:.6}");

    // ── Step 2: build filter graph ────────────────────────────────────────────
    macro_rules! bail {
        ($graph:expr, $reason:expr) => {{
            let mut g = $graph;
            avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(PreviewImageError::OperationFailed {
                reason: format!("{}", $reason),
            });
        }};
    }

    // Use forward slashes and escape ':' (Windows drive-letter separator):
    // FFmpeg's filter arg parser uses ':' as key-value separator and '\' as
    // escape; 'C:/foo' would be split at ':' unless written as 'C\:/foo'.
    let path_str = input
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");
    let movie_args = CString::new(format!("filename={path_str}")).map_err(|_| {
        PreviewImageError::OperationFailed {
            reason: "input path contains null byte".to_string(),
        }
    })?;
    let fps_cstr =
        CString::new(fps_arg.as_str()).map_err(|_| PreviewImageError::OperationFailed {
            reason: "fps arg contains null byte".to_string(),
        })?;
    let scale_args = CString::new(format!("{frame_width}:{frame_height}")).map_err(|_| {
        PreviewImageError::OperationFailed {
            reason: "scale args contain null byte".to_string(),
        }
    })?;
    let tile_args = CString::new(format!("{cols}x{rows}:padding=0:margin=0")).map_err(|_| {
        PreviewImageError::OperationFailed {
            reason: "tile args contain null byte".to_string(),
        }
    })?;

    let graph = avfilter_graph_alloc();
    if graph.is_null() {
        return Err(PreviewImageError::OperationFailed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // 1. movie source
    let movie_filt = avfilter_get_by_name(c"movie".as_ptr());
    if movie_filt.is_null() {
        bail!(graph, "filter not found: movie");
    }
    let mut movie_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut movie_ctx,
        movie_filt,
        c"sprite_movie".as_ptr(),
        movie_args.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("movie create_filter failed code={ret}"));
    }

    // 2. fps filter
    let fps_filt = avfilter_get_by_name(c"fps".as_ptr());
    if fps_filt.is_null() {
        bail!(graph, "filter not found: fps");
    }
    let mut fps_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut fps_ctx,
        fps_filt,
        c"sprite_fps".as_ptr(),
        fps_cstr.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("fps create_filter failed code={ret}"));
    }

    // 3. scale filter
    let scale_filt = avfilter_get_by_name(c"scale".as_ptr());
    if scale_filt.is_null() {
        bail!(graph, "filter not found: scale");
    }
    let mut scale_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut scale_ctx,
        scale_filt,
        c"sprite_scale".as_ptr(),
        scale_args.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("scale create_filter failed code={ret}"));
    }

    // 4. tile filter
    let tile_filt = avfilter_get_by_name(c"tile".as_ptr());
    if tile_filt.is_null() {
        bail!(graph, "filter not found: tile");
    }
    let mut tile_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut tile_ctx,
        tile_filt,
        c"sprite_tile".as_ptr(),
        tile_args.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("tile create_filter failed code={ret}"));
    }

    // 5. buffersink
    let buffersink_filt = avfilter_get_by_name(c"buffersink".as_ptr());
    if buffersink_filt.is_null() {
        bail!(graph, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut sink_ctx,
        buffersink_filt,
        c"sprite_sink".as_ptr(),
        ptr::null_mut(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("buffersink create_filter failed code={ret}"));
    }

    // Links: movie → fps → scale → tile → buffersink
    let ret = avfilter_link(movie_ctx, 0, fps_ctx, 0);
    if ret < 0 {
        bail!(graph, format!("avfilter_link movie→fps failed code={ret}"));
    }
    let ret = avfilter_link(fps_ctx, 0, scale_ctx, 0);
    if ret < 0 {
        bail!(graph, format!("avfilter_link fps→scale failed code={ret}"));
    }
    let ret = avfilter_link(scale_ctx, 0, tile_ctx, 0);
    if ret < 0 {
        bail!(graph, format!("avfilter_link scale→tile failed code={ret}"));
    }
    let ret = avfilter_link(tile_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link tile→buffersink failed code={ret}")
        );
    }

    // Configure graph
    let ret = avfilter_graph_config(graph, ptr::null_mut());
    if ret < 0 {
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // ── Step 3: pull one output frame from the tile filter ────────────────────
    let tile_frame = av_frame_alloc();
    if tile_frame.is_null() {
        bail!(graph, "av_frame_alloc failed for tile frame");
    }

    let ret = av_buffersink_get_frame(sink_ctx, tile_frame);
    let got_frame = ret >= 0;

    if !got_frame {
        let mut f = tile_frame;
        av_frame_free(std::ptr::addr_of_mut!(f));
        bail!(graph, "tile filter produced no output frame");
    }

    // ── Step 4: encode the tile frame as PNG ──────────────────────────────────
    let encode_result =
        encode_frame_as_png(tile_frame, output, cols, rows, frame_width, frame_height);

    // Cleanup filter graph and frame regardless of encode result.
    let mut f = tile_frame;
    av_frame_free(std::ptr::addr_of_mut!(f));
    let mut g = graph;
    avfilter_graph_free(std::ptr::addr_of_mut!(g));

    encode_result?;

    log::info!(
        "sprite sheet generated cols={cols} rows={rows} output={}",
        output.display()
    );

    Ok(())
}

/// Encodes a raw `*mut AVFrame` as a PNG file at `output`.
///
/// # Safety
///
/// `frame` must be a valid, non-null frame produced by the tile filter.
/// All allocations are freed on every exit path.
unsafe fn encode_frame_as_png(
    frame: *mut ff_sys::AVFrame,
    output: &Path,
    cols: u32,
    rows: u32,
    frame_width: u32,
    frame_height: u32,
) -> Result<(), PreviewImageError> {
    let _ = (cols, rows, frame_width, frame_height); // used via frame dimensions

    let width = (*frame).width;
    let height = (*frame).height;
    let src_pix_fmt = (*frame).format;

    // ── Convert to rgb24 if the frame pixel format is not PNG-compatible ──────
    // PNG encoder only accepts: rgb24, rgba, rgb48be, rgba64be, pal8, gray, …
    // Filter outputs (tile, palettegen) typically emit yuv420p or bgra.
    // We unconditionally convert to rgb24 to avoid EINVAL from avcodec_open2.
    let converted_frame: *mut ff_sys::AVFrame;
    let needs_conversion = src_pix_fmt != AVPixelFormat_AV_PIX_FMT_RGB24;
    if needs_conversion {
        let cf = av_frame_alloc();
        if cf.is_null() {
            return Err(PreviewImageError::Ffmpeg {
                code: 0,
                message: "av_frame_alloc failed for rgb24 conversion frame".to_string(),
            });
        }
        (*cf).width = width;
        (*cf).height = height;
        (*cf).format = AVPixelFormat_AV_PIX_FMT_RGB24;
        let ret = av_frame_get_buffer(cf, 0);
        if ret < 0 {
            let mut f = cf;
            av_frame_free(std::ptr::addr_of_mut!(f));
            return Err(PreviewImageError::from_ffmpeg_error(ret));
        }
        // SAFETY: src_pix_fmt and AVPixelFormat_AV_PIX_FMT_RGB24 are valid;
        // frame and cf buffers are allocated and large enough.
        let sws_ctx = swscale::get_context(
            width,
            height,
            src_pix_fmt,
            width,
            height,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            swscale::scale_flags::BILINEAR,
        )
        .map_err(|e| {
            let mut f = cf;
            av_frame_free(std::ptr::addr_of_mut!(f));
            PreviewImageError::from_ffmpeg_error(e)
        })?;
        let scale_ret = swscale::scale(
            sws_ctx,
            (*frame).data.as_ptr().cast::<*const u8>(),
            (*frame).linesize.as_ptr(),
            0,
            height,
            (*cf).data.as_mut_ptr().cast_const(),
            (*cf).linesize.as_mut_ptr(),
        );
        swscale::free_context(sws_ctx);
        if let Err(e) = scale_ret {
            let mut f = cf;
            av_frame_free(std::ptr::addr_of_mut!(f));
            return Err(PreviewImageError::from_ffmpeg_error(e));
        }
        converted_frame = cf;
    } else {
        converted_frame = frame;
    }

    let encode_result = encode_frame_as_png_inner(converted_frame, output, width, height);

    if needs_conversion {
        let mut f = converted_frame;
        av_frame_free(std::ptr::addr_of_mut!(f));
    }

    encode_result
}

/// Encodes a rgb24 `*mut AVFrame` as a PNG file at `output`.
///
/// # Safety
///
/// `frame` must be a valid, non-null rgb24 frame with matching `width`/`height`.
/// All allocations are freed on every exit path.
unsafe fn encode_frame_as_png_inner(
    frame: *mut ff_sys::AVFrame,
    output: &Path,
    width: i32,
    height: i32,
) -> Result<(), PreviewImageError> {
    let pix_fmt = AVPixelFormat_AV_PIX_FMT_RGB24;

    // ── Allocate output format context ────────────────────────────────────────
    let mut fmt_ctx: *mut ff_sys::AVFormatContext = ptr::null_mut();
    let c_path =
        CString::new(
            output
                .to_str()
                .ok_or_else(|| PreviewImageError::CannotCreateFile {
                    path: output.to_path_buf(),
                })?,
        )
        .map_err(|_| PreviewImageError::CannotCreateFile {
            path: output.to_path_buf(),
        })?;

    // Use the image2 muxer explicitly: it accepts the png encoder for single-
    // frame output, regardless of the file extension.  The "apng" muxer only
    // accepts the APNG (animated PNG) codec — not the plain PNG codec — and
    // would fail at avformat_write_header.
    let ret = avformat_alloc_output_context2(
        &mut fmt_ctx,
        ptr::null_mut(),
        c"image2".as_ptr(),
        c_path.as_ptr(),
    );
    if ret < 0 || fmt_ctx.is_null() {
        return Err(PreviewImageError::from_ffmpeg_error(ret));
    }

    // ── Create video stream ───────────────────────────────────────────────────
    let stream = avformat_new_stream(fmt_ctx, ptr::null());
    if stream.is_null() {
        avformat_free_context(fmt_ctx);
        return Err(PreviewImageError::Ffmpeg {
            code: 0,
            message: "avformat_new_stream failed".to_string(),
        });
    }

    // ── Find and open PNG encoder ─────────────────────────────────────────────
    let Some(codec) = avcodec::find_encoder(AVCodecID_AV_CODEC_ID_PNG) else {
        avformat_free_context(fmt_ctx);
        return Err(PreviewImageError::UnsupportedCodec {
            codec: "png".to_string(),
        });
    };

    let codec_ctx = match avcodec::alloc_context3(codec) {
        Ok(ctx) => ctx,
        Err(e) => {
            avformat_free_context(fmt_ctx);
            return Err(PreviewImageError::from_ffmpeg_error(e));
        }
    };

    // SAFETY: codec_ctx is non-null (alloc_context3 succeeded).
    (*codec_ctx).width = width;
    (*codec_ctx).height = height;
    (*codec_ctx).time_base = AVRational { num: 1, den: 1 };
    (*codec_ctx).pix_fmt = pix_fmt;

    if let Err(e) = avcodec::open2(codec_ctx, codec, ptr::null_mut()) {
        avcodec::free_context(&mut { codec_ctx });
        avformat_free_context(fmt_ctx);
        return Err(PreviewImageError::from_ffmpeg_error(e));
    }

    // Copy codec parameters to stream.
    // SAFETY: stream and codec_ctx are non-null.
    let par = (*stream).codecpar;
    (*par).codec_id = AVCodecID_AV_CODEC_ID_PNG;
    (*par).codec_type = ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO;
    (*par).width = width;
    (*par).height = height;
    (*par).format = pix_fmt;

    // ── Open output IO and write header ───────────────────────────────────────
    let io_ctx = match avformat::open_output(output, avformat::avio_flags::WRITE) {
        Ok(ctx) => ctx,
        Err(e) => {
            let mut cc = codec_ctx;
            avcodec::free_context(&mut cc);
            avformat_free_context(fmt_ctx);
            return Err(PreviewImageError::from_ffmpeg_error(e));
        }
    };
    (*fmt_ctx).pb = io_ctx;

    let ret = avformat_write_header(fmt_ctx, ptr::null_mut());
    if ret < 0 {
        avformat::close_output(&mut (*fmt_ctx).pb);
        let mut cc = codec_ctx;
        avcodec::free_context(&mut cc);
        avformat_free_context(fmt_ctx);
        return Err(PreviewImageError::from_ffmpeg_error(ret));
    }

    // ── Allocate packet ───────────────────────────────────────────────────────
    let packet = av_packet_alloc();
    if packet.is_null() {
        av_write_trailer(fmt_ctx);
        avformat::close_output(&mut (*fmt_ctx).pb);
        let mut cc = codec_ctx;
        avcodec::free_context(&mut cc);
        avformat_free_context(fmt_ctx);
        return Err(PreviewImageError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    }

    // ── Encode: send frame → flush → drain packets ────────────────────────────
    (*frame).pts = 0;

    let encode_result = (|| -> Result<(), PreviewImageError> {
        avcodec::send_frame(codec_ctx, frame).map_err(PreviewImageError::from_ffmpeg_error)?;
        drain_packets(codec_ctx, fmt_ctx, packet, false)?;
        avcodec::send_frame(codec_ctx, ptr::null())
            .map_err(PreviewImageError::from_ffmpeg_error)?;
        drain_packets(codec_ctx, fmt_ctx, packet, true)?;
        Ok(())
    })();

    av_write_trailer(fmt_ctx);
    avformat::close_output(&mut (*fmt_ctx).pb);
    av_packet_free(&mut { packet });
    avcodec::free_context(&mut { codec_ctx });
    avformat_free_context(fmt_ctx);

    encode_result
}

/// Drains encoded packets from `codec_ctx` and writes them to `fmt_ctx`.
///
/// When `until_eof` is `true`, loops until `AVERROR_EOF`; otherwise also
/// stops on `AVERROR(EAGAIN)`.
///
/// # Safety
///
/// `codec_ctx`, `fmt_ctx`, and `packet` must all be valid non-null pointers.
unsafe fn drain_packets(
    codec_ctx: *mut ff_sys::AVCodecContext,
    fmt_ctx: *mut ff_sys::AVFormatContext,
    packet: *mut ff_sys::AVPacket,
    until_eof: bool,
) -> Result<(), PreviewImageError> {
    loop {
        match avcodec::receive_packet(codec_ctx, packet) {
            Ok(()) => {
                (*packet).stream_index = 0;
                let ret = av_interleaved_write_frame(fmt_ctx, packet);
                av_packet_unref(packet);
                if ret < 0 {
                    return Err(PreviewImageError::from_ffmpeg_error(ret));
                }
            }
            Err(e) if e == ff_sys::error_codes::EOF => break,
            Err(e) if !until_eof && e == ff_sys::error_codes::EAGAIN => break,
            Err(e) => return Err(PreviewImageError::from_ffmpeg_error(e)),
        }
    }
    Ok(())
}

// ── GifPreview implementation ─────────────────────────────────────────────────

/// Generates an animated GIF from `input` using a two-pass palettegen approach.
///
/// Pass 1: builds a palette from the time range via
///   `movie → trim → fps → scale → palettegen → buffersink`
///   and saves it to a temp PNG file.
///
/// Pass 2: composes the GIF via
///   `movie_vid / movie_pal → trim → fps → scale / paletteuse → buffersink`
///   then encodes each frame with the GIF encoder.
///
/// # Safety
///
/// All raw pointer operations follow avfilter and avcodec ownership rules.
/// Every allocation is freed on every exit path.
/// Safe entry point called from [`super`]; all `unsafe` is confined here.
pub(super) fn generate_gif_preview(
    input: &Path,
    start: Duration,
    duration: Duration,
    fps: f64,
    width: u32,
    output: &Path,
) -> Result<(), PreviewImageError> {
    // SAFETY: generate_gif_preview_unsafe manages all raw pointer lifetimes
    //         per avfilter and avcodec ownership rules.
    unsafe { generate_gif_preview_unsafe(input, start, duration, fps, width, output) }
}

unsafe fn generate_gif_preview_unsafe(
    input: &Path,
    start: Duration,
    duration: Duration,
    fps: f64,
    width: u32,
    output: &Path,
) -> Result<(), PreviewImageError> {
    let start_sec = start.as_secs_f64();
    let dur_sec = duration.as_secs_f64();

    // Temp palette file uses the process ID to avoid collisions.
    let palette_path =
        std::env::temp_dir().join(format!("ff_gif_palette_{}.png", std::process::id()));

    // ── Pass 1: generate palette ──────────────────────────────────────────────
    let palette_result =
        generate_palette_unsafe(input, start_sec, dur_sec, fps, width, &palette_path);

    if let Err(e) = palette_result {
        let _ = std::fs::remove_file(&palette_path);
        return Err(e);
    }

    // ── Pass 2: encode GIF ────────────────────────────────────────────────────
    let gif_result =
        encode_gif_unsafe(input, start_sec, dur_sec, fps, width, &palette_path, output);

    // Always clean up the temp palette file.
    let _ = std::fs::remove_file(&palette_path);

    gif_result?;

    log::info!(
        "gif preview generated start={start:?} duration={duration:?} output={}",
        output.display()
    );

    Ok(())
}

/// Pass 1: builds filter graph to generate a palette and saves it to `palette_path`.
///
/// Filter chain: `movie → trim → fps → scale → palettegen → buffersink`
///
/// # Safety
///
/// All FFmpeg pointers are null-checked and freed on every exit path.
unsafe fn generate_palette_unsafe(
    input: &Path,
    start_sec: f64,
    dur_sec: f64,
    fps: f64,
    width: u32,
    palette_path: &Path,
) -> Result<(), PreviewImageError> {
    macro_rules! bail {
        ($graph:expr, $reason:expr) => {{
            let mut g = $graph;
            avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(PreviewImageError::OperationFailed {
                reason: format!("{}", $reason),
            });
        }};
    }

    let path_str = input
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");
    let movie_args = CString::new(format!("filename={path_str}")).map_err(|_| {
        PreviewImageError::OperationFailed {
            reason: "input path contains null byte".to_string(),
        }
    })?;
    let trim_args =
        CString::new(format!("start={start_sec:.6}:duration={dur_sec:.6}")).map_err(|_| {
            PreviewImageError::OperationFailed {
                reason: "trim args contain null byte".to_string(),
            }
        })?;
    let fps_cstr =
        CString::new(format!("{fps:.4}")).map_err(|_| PreviewImageError::OperationFailed {
            reason: "fps arg contains null byte".to_string(),
        })?;
    let scale_args = CString::new(format!("{width}:-2:flags=lanczos")).map_err(|_| {
        PreviewImageError::OperationFailed {
            reason: "scale args contain null byte".to_string(),
        }
    })?;

    let graph = avfilter_graph_alloc();
    if graph.is_null() {
        return Err(PreviewImageError::OperationFailed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // 1. movie source
    let movie_filt = avfilter_get_by_name(c"movie".as_ptr());
    if movie_filt.is_null() {
        bail!(graph, "filter not found: movie");
    }
    let mut movie_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut movie_ctx,
        movie_filt,
        c"pal_movie".as_ptr(),
        movie_args.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("movie create_filter failed code={ret}"));
    }

    // 2. trim filter
    let trim_filt = avfilter_get_by_name(c"trim".as_ptr());
    if trim_filt.is_null() {
        bail!(graph, "filter not found: trim");
    }
    let mut trim_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut trim_ctx,
        trim_filt,
        c"pal_trim".as_ptr(),
        trim_args.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("trim create_filter failed code={ret}"));
    }

    // 3. fps filter
    let fps_filt = avfilter_get_by_name(c"fps".as_ptr());
    if fps_filt.is_null() {
        bail!(graph, "filter not found: fps");
    }
    let mut fps_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut fps_ctx,
        fps_filt,
        c"pal_fps".as_ptr(),
        fps_cstr.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("fps create_filter failed code={ret}"));
    }

    // 4. scale filter
    let scale_filt = avfilter_get_by_name(c"scale".as_ptr());
    if scale_filt.is_null() {
        bail!(graph, "filter not found: scale");
    }
    let mut scale_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut scale_ctx,
        scale_filt,
        c"pal_scale".as_ptr(),
        scale_args.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("scale create_filter failed code={ret}"));
    }

    // 5. palettegen filter
    let palettegen_filt = avfilter_get_by_name(c"palettegen".as_ptr());
    if palettegen_filt.is_null() {
        bail!(graph, "filter not found: palettegen");
    }
    let mut palettegen_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut palettegen_ctx,
        palettegen_filt,
        c"pal_palettegen".as_ptr(),
        c"stats_mode=diff".as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("palettegen create_filter failed code={ret}"));
    }

    // 6. buffersink
    let sink_filt = avfilter_get_by_name(c"buffersink".as_ptr());
    if sink_filt.is_null() {
        bail!(graph, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut sink_ctx,
        sink_filt,
        c"pal_sink".as_ptr(),
        ptr::null_mut(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("buffersink create_filter failed code={ret}"));
    }

    // Links: movie → trim → fps → scale → palettegen → buffersink
    let ret = avfilter_link(movie_ctx, 0, trim_ctx, 0);
    if ret < 0 {
        bail!(graph, format!("avfilter_link movie→trim failed code={ret}"));
    }
    let ret = avfilter_link(trim_ctx, 0, fps_ctx, 0);
    if ret < 0 {
        bail!(graph, format!("avfilter_link trim→fps failed code={ret}"));
    }
    let ret = avfilter_link(fps_ctx, 0, scale_ctx, 0);
    if ret < 0 {
        bail!(graph, format!("avfilter_link fps→scale failed code={ret}"));
    }
    let ret = avfilter_link(scale_ctx, 0, palettegen_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link scale→palettegen failed code={ret}")
        );
    }
    let ret = avfilter_link(palettegen_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link palettegen→sink failed code={ret}")
        );
    }

    let ret = avfilter_graph_config(graph, ptr::null_mut());
    if ret < 0 {
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // Drain until we get the palette frame (palettegen emits one frame on EOF).
    let mut palette_frame: *mut ff_sys::AVFrame = ptr::null_mut();
    loop {
        let candidate = av_frame_alloc();
        if candidate.is_null() {
            break;
        }
        let ret = av_buffersink_get_frame(sink_ctx, candidate);
        if ret >= 0 {
            // Free any previous candidate; keep this one.
            if !palette_frame.is_null() {
                let mut prev = palette_frame;
                av_frame_free(std::ptr::addr_of_mut!(prev));
            }
            palette_frame = candidate;
        } else {
            let mut c = candidate;
            av_frame_free(std::ptr::addr_of_mut!(c));
            break;
        }
    }

    let mut g = graph;
    avfilter_graph_free(std::ptr::addr_of_mut!(g));

    if palette_frame.is_null() {
        return Err(PreviewImageError::OperationFailed {
            reason: "palettegen produced no palette frame".to_string(),
        });
    }

    // Save the palette frame to disk as PNG.
    let encode_result = encode_frame_as_png(palette_frame, palette_path, 0, 0, 0, 0);
    let mut f = palette_frame;
    av_frame_free(std::ptr::addr_of_mut!(f));
    encode_result
}

/// Pass 2: composes the GIF from the video + palette and encodes it.
///
/// Filter chain:
/// ```text
/// movie_vid → trim → fps → scale → paletteuse[0]
/// movie_pal                      → paletteuse[1]
/// paletteuse → buffersink
/// ```
///
/// # Safety
///
/// All FFmpeg pointers are null-checked and freed on every exit path.
unsafe fn encode_gif_unsafe(
    input: &Path,
    start_sec: f64,
    dur_sec: f64,
    fps: f64,
    width: u32,
    palette_path: &Path,
    output: &Path,
) -> Result<(), PreviewImageError> {
    macro_rules! bail {
        ($graph:expr, $reason:expr) => {{
            let mut g = $graph;
            avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(PreviewImageError::OperationFailed {
                reason: format!("{}", $reason),
            });
        }};
    }

    let path_str = input
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");
    let movie_vid_args = CString::new(format!("filename={path_str}")).map_err(|_| {
        PreviewImageError::OperationFailed {
            reason: "input path contains null byte".to_string(),
        }
    })?;
    // FFmpeg filter option strings use ':' as key-value separator and '\' as
    // escape character.  On Windows, absolute paths contain a drive-letter
    // colon (C:/) which must be escaped as \: so the parser treats it as part
    // of the value, not as a new option.
    let pal_str = palette_path
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");
    let movie_pal_args = CString::new(format!("filename={pal_str}")).map_err(|_| {
        PreviewImageError::OperationFailed {
            reason: "palette path contains null byte".to_string(),
        }
    })?;
    let trim_args =
        CString::new(format!("start={start_sec:.6}:duration={dur_sec:.6}")).map_err(|_| {
            PreviewImageError::OperationFailed {
                reason: "trim args contain null byte".to_string(),
            }
        })?;
    let fps_cstr =
        CString::new(format!("{fps:.4}")).map_err(|_| PreviewImageError::OperationFailed {
            reason: "fps arg contains null byte".to_string(),
        })?;
    let scale_args = CString::new(format!("{width}:-2:flags=lanczos")).map_err(|_| {
        PreviewImageError::OperationFailed {
            reason: "scale args contain null byte".to_string(),
        }
    })?;

    let graph = avfilter_graph_alloc();
    if graph.is_null() {
        return Err(PreviewImageError::OperationFailed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // 1. movie source for video
    let movie_filt = avfilter_get_by_name(c"movie".as_ptr());
    if movie_filt.is_null() {
        bail!(graph, "filter not found: movie");
    }
    let mut movie_vid_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut movie_vid_ctx,
        movie_filt,
        c"gif_movie_vid".as_ptr(),
        movie_vid_args.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("movie_vid create_filter failed code={ret}"));
    }

    // 2. movie source for palette PNG
    let mut movie_pal_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut movie_pal_ctx,
        movie_filt,
        c"gif_movie_pal".as_ptr(),
        movie_pal_args.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("movie_pal create_filter failed code={ret}"));
    }

    // 3. trim
    let trim_filt = avfilter_get_by_name(c"trim".as_ptr());
    if trim_filt.is_null() {
        bail!(graph, "filter not found: trim");
    }
    let mut trim_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut trim_ctx,
        trim_filt,
        c"gif_trim".as_ptr(),
        trim_args.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("trim create_filter failed code={ret}"));
    }

    // 4. fps
    let fps_filt = avfilter_get_by_name(c"fps".as_ptr());
    if fps_filt.is_null() {
        bail!(graph, "filter not found: fps");
    }
    let mut fps_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut fps_ctx,
        fps_filt,
        c"gif_fps".as_ptr(),
        fps_cstr.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("fps create_filter failed code={ret}"));
    }

    // 5. scale
    let scale_filt = avfilter_get_by_name(c"scale".as_ptr());
    if scale_filt.is_null() {
        bail!(graph, "filter not found: scale");
    }
    let mut scale_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut scale_ctx,
        scale_filt,
        c"gif_scale".as_ptr(),
        scale_args.as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("scale create_filter failed code={ret}"));
    }

    // 6. paletteuse (2 input pads: pad 0 = video, pad 1 = palette)
    let paletteuse_filt = avfilter_get_by_name(c"paletteuse".as_ptr());
    if paletteuse_filt.is_null() {
        bail!(graph, "filter not found: paletteuse");
    }
    let mut paletteuse_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut paletteuse_ctx,
        paletteuse_filt,
        c"gif_paletteuse".as_ptr(),
        c"dither=bayer:bayer_scale=5:diff_mode=rectangle".as_ptr(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("paletteuse create_filter failed code={ret}"));
    }

    // 7. buffersink
    let sink_filt = avfilter_get_by_name(c"buffersink".as_ptr());
    if sink_filt.is_null() {
        bail!(graph, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = ptr::null_mut();
    let ret = avfilter_graph_create_filter(
        &raw mut sink_ctx,
        sink_filt,
        c"gif_sink".as_ptr(),
        ptr::null_mut(),
        ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("buffersink create_filter failed code={ret}"));
    }

    // Links:
    //   movie_vid → trim → fps → scale → paletteuse[0]
    //   movie_pal                       → paletteuse[1]
    //   paletteuse → buffersink
    let ret = avfilter_link(movie_vid_ctx, 0, trim_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link movie_vid→trim failed code={ret}")
        );
    }
    let ret = avfilter_link(trim_ctx, 0, fps_ctx, 0);
    if ret < 0 {
        bail!(graph, format!("avfilter_link trim→fps failed code={ret}"));
    }
    let ret = avfilter_link(fps_ctx, 0, scale_ctx, 0);
    if ret < 0 {
        bail!(graph, format!("avfilter_link fps→scale failed code={ret}"));
    }
    let ret = avfilter_link(scale_ctx, 0, paletteuse_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link scale→paletteuse[0] failed code={ret}")
        );
    }
    let ret = avfilter_link(movie_pal_ctx, 0, paletteuse_ctx, 1);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link movie_pal→paletteuse[1] failed code={ret}")
        );
    }
    let ret = avfilter_link(paletteuse_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link paletteuse→sink failed code={ret}")
        );
    }

    let ret = avfilter_graph_config(graph, ptr::null_mut());
    if ret < 0 {
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // ── Open GIF output ───────────────────────────────────────────────────────
    let mut fmt_ctx: *mut ff_sys::AVFormatContext = ptr::null_mut();
    let Some(path_str) = output.to_str() else {
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::CannotCreateFile {
            path: output.to_path_buf(),
        });
    };
    let Ok(c_path) = CString::new(path_str) else {
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::CannotCreateFile {
            path: output.to_path_buf(),
        });
    };

    let ret = avformat_alloc_output_context2(
        &mut fmt_ctx,
        ptr::null_mut(),
        c"gif".as_ptr(),
        c_path.as_ptr(),
    );
    if ret < 0 || fmt_ctx.is_null() {
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::from_ffmpeg_error(ret));
    }

    let stream = avformat_new_stream(fmt_ctx, ptr::null());
    if stream.is_null() {
        avformat_free_context(fmt_ctx);
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::Ffmpeg {
            code: 0,
            message: "avformat_new_stream failed".to_string(),
        });
    }

    let Some(codec) = avcodec::find_encoder(AVCodecID_AV_CODEC_ID_GIF) else {
        avformat_free_context(fmt_ctx);
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::UnsupportedCodec {
            codec: "gif".to_string(),
        });
    };

    let codec_ctx = match avcodec::alloc_context3(codec) {
        Ok(ctx) => ctx,
        Err(e) => {
            avformat_free_context(fmt_ctx);
            let mut g = graph;
            avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(PreviewImageError::from_ffmpeg_error(e));
        }
    };

    // Pull a first frame to discover width/height/pix_fmt from the filter output.
    let first_frame = av_frame_alloc();
    if first_frame.is_null() {
        avcodec::free_context(&mut { codec_ctx });
        avformat_free_context(fmt_ctx);
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::Ffmpeg {
            code: 0,
            message: "av_frame_alloc failed".to_string(),
        });
    }
    let ret = av_buffersink_get_frame(sink_ctx, first_frame);
    if ret < 0 {
        let mut f = first_frame;
        av_frame_free(std::ptr::addr_of_mut!(f));
        avcodec::free_context(&mut { codec_ctx });
        avformat_free_context(fmt_ctx);
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::OperationFailed {
            reason: format!("no frames from GIF filter graph code={ret}"),
        });
    }

    let out_width = (*first_frame).width;
    let out_height = (*first_frame).height;
    let out_pix_fmt = (*first_frame).format;

    // Configure GIF encoder.
    // SAFETY: codec_ctx is non-null.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fps_int = fps.round().max(1.0) as u32;
    (*codec_ctx).width = out_width;
    (*codec_ctx).height = out_height;
    (*codec_ctx).time_base = AVRational {
        num: 1,
        den: fps_int as i32,
    };
    (*codec_ctx).pix_fmt = out_pix_fmt;

    // Set GIF to loop infinitely (option "loop" = 0).
    // SAFETY: priv_data is valid after alloc_context3; av_opt_set handles unknown options gracefully.
    let _ = av_opt_set(
        (*codec_ctx).priv_data.cast(),
        c"loop".as_ptr(),
        c"0".as_ptr(),
        0,
    );

    if let Err(e) = avcodec::open2(codec_ctx, codec, ptr::null_mut()) {
        let mut f = first_frame;
        av_frame_free(std::ptr::addr_of_mut!(f));
        avcodec::free_context(&mut { codec_ctx });
        avformat_free_context(fmt_ctx);
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::from_ffmpeg_error(e));
    }

    // Copy codec parameters to stream.
    // SAFETY: stream, codec_ctx, codecpar are non-null.
    let par = (*stream).codecpar;
    (*par).codec_id = AVCodecID_AV_CODEC_ID_GIF;
    (*par).codec_type = ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO;
    (*par).width = out_width;
    (*par).height = out_height;
    (*par).format = out_pix_fmt;

    // Open output IO and write header.
    let io_ctx = match avformat::open_output(output, avformat::avio_flags::WRITE) {
        Ok(ctx) => ctx,
        Err(e) => {
            let mut f = first_frame;
            av_frame_free(std::ptr::addr_of_mut!(f));
            avcodec::free_context(&mut { codec_ctx });
            avformat_free_context(fmt_ctx);
            let mut g = graph;
            avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(PreviewImageError::from_ffmpeg_error(e));
        }
    };
    (*fmt_ctx).pb = io_ctx;

    let ret = avformat_write_header(fmt_ctx, ptr::null_mut());
    if ret < 0 {
        avformat::close_output(&mut (*fmt_ctx).pb);
        let mut f = first_frame;
        av_frame_free(std::ptr::addr_of_mut!(f));
        avcodec::free_context(&mut { codec_ctx });
        avformat_free_context(fmt_ctx);
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::from_ffmpeg_error(ret));
    }

    let packet = av_packet_alloc();
    if packet.is_null() {
        av_write_trailer(fmt_ctx);
        avformat::close_output(&mut (*fmt_ctx).pb);
        let mut f = first_frame;
        av_frame_free(std::ptr::addr_of_mut!(f));
        avcodec::free_context(&mut { codec_ctx });
        avformat_free_context(fmt_ctx);
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    }

    // ── Encode all frames ─────────────────────────────────────────────────────
    let encode_result = (|| -> Result<(), PreviewImageError> {
        let mut frame_counter: i64 = 0;

        // Encode the first frame we already pulled.
        (*first_frame).pts = frame_counter;
        frame_counter += 1;
        avcodec::send_frame(codec_ctx, first_frame)
            .map_err(PreviewImageError::from_ffmpeg_error)?;
        drain_packets(codec_ctx, fmt_ctx, packet, false)?;

        // Pull and encode remaining frames.
        loop {
            let frame = av_frame_alloc();
            if frame.is_null() {
                break;
            }
            let ret = av_buffersink_get_frame(sink_ctx, frame);
            if ret < 0 {
                let mut f = frame;
                av_frame_free(std::ptr::addr_of_mut!(f));
                break;
            }
            (*frame).pts = frame_counter;
            frame_counter += 1;
            let send_result =
                avcodec::send_frame(codec_ctx, frame).map_err(PreviewImageError::from_ffmpeg_error);
            let mut f = frame;
            av_frame_free(std::ptr::addr_of_mut!(f));
            send_result?;
            drain_packets(codec_ctx, fmt_ctx, packet, false)?;
        }

        // Flush encoder.
        avcodec::send_frame(codec_ctx, ptr::null())
            .map_err(PreviewImageError::from_ffmpeg_error)?;
        drain_packets(codec_ctx, fmt_ctx, packet, true)?;
        Ok(())
    })();

    av_write_trailer(fmt_ctx);
    avformat::close_output(&mut (*fmt_ctx).pb);
    av_packet_free(&mut { packet });
    let mut f = first_frame;
    av_frame_free(std::ptr::addr_of_mut!(f));
    avcodec::free_context(&mut { codec_ctx });
    avformat_free_context(fmt_ctx);
    let mut g = graph;
    avfilter_graph_free(std::ptr::addr_of_mut!(g));

    encode_result
}
