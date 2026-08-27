//! Unsafe FFmpeg filter graph calls for preview generation.
//!
//! All `unsafe` code is isolated here; [`super`] exposes safe wrappers.
//!
//! Entry points:
//! - [`generate_sprite_sheet_unsafe`] — filter graph + PNG encode for sprite sheets
//! - [`generate_gif_preview_unsafe`]  — two-pass palettegen + GIF encode

#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]
// FFmpeg-boundary lints: casts at the C ABI, pointer idioms, C-string literals,
// and FFI-wrapper ergonomics concentrate in this unsafe module.
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
use std::time::Duration;

use ff_sys::{
    AVCodecID_AV_CODEC_ID_GIF, AVCodecID_AV_CODEC_ID_PNG, AVPixelFormat_AV_PIX_FMT_RGB24,
    AVRational, InputFormatContext, OutputFormatContext, avfilter_get_by_name,
    avfilter_graph_alloc, avfilter_graph_config, avfilter_graph_create_filter, avfilter_graph_free,
    avfilter_link,
};

use crate::PreviewImageError;

/// Probes the video at `path` and returns its duration in seconds.
fn probe_video_duration_secs(path: &Path) -> Result<f64, PreviewImageError> {
    let mut fmt_ctx = InputFormatContext::open(path)
        .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;
    fmt_ctx
        .find_stream_info()
        .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;

    let duration_av = fmt_ctx.duration();
    // `fmt_ctx` drops here, closing the input.

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
    let Ok(mut tile_frame) = ff_sys::Frame::new() else {
        bail!(graph, "av_frame_alloc failed for tile frame");
    };

    if !matches!(
        ff_sys::buffersink_get_frame(sink_ctx, &mut tile_frame),
        Ok(ff_sys::BufferSinkOutcome::Frame)
    ) {
        bail!(graph, "tile filter produced no output frame");
    }

    // ── Step 4: encode the tile frame as PNG ──────────────────────────────────
    let encode_result = encode_frame_as_png(
        &mut tile_frame,
        output,
        cols,
        rows,
        frame_width,
        frame_height,
    );

    // Cleanup filter graph; `tile_frame` drops at end of scope.
    let mut g = graph;
    avfilter_graph_free(std::ptr::addr_of_mut!(g));

    encode_result?;

    log::info!(
        "sprite sheet generated cols={cols} rows={rows} output={}",
        output.display()
    );

    Ok(())
}

/// Encodes an owned [`ff_sys::Frame`] as a PNG file at `output`.
///
/// # Safety
///
/// `frame` must be a valid frame produced by the tile filter.
/// All allocations are freed on every exit path.
unsafe fn encode_frame_as_png(
    frame: &mut ff_sys::Frame,
    output: &Path,
    cols: u32,
    rows: u32,
    frame_width: u32,
    frame_height: u32,
) -> Result<(), PreviewImageError> {
    let _ = (cols, rows, frame_width, frame_height); // used via frame dimensions

    let width = frame.width();
    let height = frame.height();
    let src_pix_fmt = frame.format();

    // ── Convert to rgb24 if the frame pixel format is not PNG-compatible ──────
    // PNG encoder only accepts: rgb24, rgba, rgb48be, rgba64be, pal8, gray, …
    // Filter outputs (tile, palettegen) typically emit yuv420p or bgra.
    // We unconditionally convert to rgb24 to avoid EINVAL from avcodec_open2.

    let needs_conversion = src_pix_fmt != AVPixelFormat_AV_PIX_FMT_RGB24;
    // Holds the owned rgb24 conversion frame (if any) alive until encoding finishes.
    let mut cf_owned: Option<ff_sys::Frame> = if needs_conversion {
        let mut cf =
            ff_sys::Frame::new().map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;
        cf.set_width(width);
        cf.set_height(height);
        cf.set_format(AVPixelFormat_AV_PIX_FMT_RGB24);
        cf.get_buffer(0)
            .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;
        // `sws_ctx` and `cf` drop on any `?` below, freeing their resources.
        let mut sws_ctx = ff_sys::ScaleContext::new(
            width,
            height,
            src_pix_fmt,
            width,
            height,
            AVPixelFormat_AV_PIX_FMT_RGB24,
            ff_sys::swscale::scale_flags::BILINEAR,
        )
        .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;
        sws_ctx
            .scale_frames(frame, &mut cf)
            .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;
        Some(cf)
    } else {
        None
    };

    let converted_frame: &mut ff_sys::Frame = match cf_owned.as_mut() {
        Some(cf) => cf,
        None => frame,
    };

    // `cf_owned` (if any) drops at end of scope, freeing the conversion frame.
    encode_frame_as_png_inner(converted_frame, output, width, height)
}

/// Encodes a rgb24 `*mut AVFrame` as a PNG file at `output`.
///
/// # Safety
///
/// `frame` must be a valid, non-null rgb24 frame with matching `width`/`height`.
/// All allocations are freed on every exit path.
unsafe fn encode_frame_as_png_inner(
    frame: &mut ff_sys::Frame,
    output: &Path,
    width: i32,
    height: i32,
) -> Result<(), PreviewImageError> {
    let pix_fmt = AVPixelFormat_AV_PIX_FMT_RGB24;

    // ── Allocate output format context (owned) ────────────────────────────────
    // Use the image2 muxer explicitly: it accepts the png encoder for single-
    // frame output, regardless of the file extension.  The "apng" muxer only
    // accepts the APNG (animated PNG) codec — not the plain PNG codec — and
    // would fail at the header write. The owned context frees itself and closes
    // its IO on drop, so every early return below is leak-free.
    let mut fmt_ctx = OutputFormatContext::new(Some("image2"), output)
        .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;

    // ── Create video stream ───────────────────────────────────────────────────
    let stream_idx = fmt_ctx
        .new_stream(None)
        .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;

    // ── Find and open PNG encoder ─────────────────────────────────────────────
    let Some(codec) = ff_sys::Codec::find_encoder(AVCodecID_AV_CODEC_ID_PNG) else {
        return Err(PreviewImageError::UnsupportedCodec {
            codec: "png".to_string(),
        });
    };

    let mut codec_ctx = ff_sys::CodecContext::new(Some(codec))
        .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;

    codec_ctx.set_width(width);
    codec_ctx.set_height(height);
    codec_ctx.set_time_base(AVRational { num: 1, den: 1 });
    codec_ctx.set_pix_fmt(pix_fmt);

    codec_ctx
        .open_codec(codec)
        .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;

    // Copy codec parameters to stream (includes width/height/format from the
    // opened PNG encoder context).
    fmt_ctx
        .apply_stream_params_from_context(stream_idx, &codec_ctx)
        .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;

    // ── Open output IO and write header ───────────────────────────────────────
    fmt_ctx
        .open_io(output)
        .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;

    fmt_ctx
        .write_header()
        .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;

    // ── Allocate packet ───────────────────────────────────────────────────────
    let Ok(mut packet) = ff_sys::Packet::new() else {
        return Err(PreviewImageError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    };

    // ── Encode: send frame → flush → drain packets ────────────────────────────
    frame.set_pts(0);

    let encode_result = (|| -> Result<(), PreviewImageError> {
        codec_ctx
            .send_frame(Some(&*frame))
            .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;
        drain_packets(&mut codec_ctx, &mut fmt_ctx, &mut packet, false)?;
        codec_ctx
            .send_frame(None)
            .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;
        drain_packets(&mut codec_ctx, &mut fmt_ctx, &mut packet, true)?;
        Ok(())
    })();

    // Finalise the file, then let `packet`, `codec_ctx`, and `fmt_ctx` free
    // themselves at end of scope (the owned context closes its IO on drop).
    let _ = fmt_ctx.write_trailer();
    fmt_ctx.close_io();

    encode_result
}

/// Drains encoded packets from `codec_ctx` and writes them to `fmt_ctx`.
///
/// When `until_eof` is `true`, loops until `AVERROR_EOF`; otherwise also
/// stops on `AVERROR(EAGAIN)`.
///
/// # Safety
///
/// `codec_ctx`, `fmt_ctx`, and `packet` must all be valid.
unsafe fn drain_packets(
    codec_ctx: &mut ff_sys::CodecContext,
    fmt_ctx: &mut ff_sys::OutputFormatContext,
    packet: &mut ff_sys::Packet,
    until_eof: bool,
) -> Result<(), PreviewImageError> {
    loop {
        match codec_ctx
            .receive_packet(packet)
            .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?
        {
            ff_sys::ReceiveOutcome::Frame => {
                packet.set_stream_index(0);
                let ret = fmt_ctx.write_interleaved(packet);
                packet.unref();
                if let Err(e) = ret {
                    return Err(PreviewImageError::from_ffmpeg_error(e.code()));
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
    let mut palette_frame: Option<ff_sys::Frame> = None;
    loop {
        let Ok(mut candidate) = ff_sys::Frame::new() else {
            break;
        };
        if matches!(
            ff_sys::buffersink_get_frame(sink_ctx, &mut candidate),
            Ok(ff_sys::BufferSinkOutcome::Frame)
        ) {
            // Keep this candidate; reassigning drops (frees) any previous one.
            palette_frame = Some(candidate);
        } else {
            // `candidate` drops here, freeing it.
            break;
        }
    }

    let mut g = graph;
    avfilter_graph_free(std::ptr::addr_of_mut!(g));

    let Some(mut palette_frame) = palette_frame else {
        return Err(PreviewImageError::OperationFailed {
            reason: "palettegen produced no palette frame".to_string(),
        });
    };

    // Save the palette frame to disk as PNG; `palette_frame` drops at end of scope.
    encode_frame_as_png(&mut palette_frame, palette_path, 0, 0, 0, 0)
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

    // ── Open GIF output (owned format context) ────────────────────────────────
    // The owned context frees itself and closes its IO on drop; each early return
    // below still frees the raw filter graph explicitly.
    let mut fmt_ctx = match OutputFormatContext::new(Some("gif"), output) {
        Ok(ctx) => ctx,
        Err(e) => {
            let mut g = graph;
            avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(PreviewImageError::from_ffmpeg_error(e.code()));
        }
    };

    let stream_idx = match fmt_ctx.new_stream(None) {
        Ok(idx) => idx,
        Err(e) => {
            let mut g = graph;
            avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(PreviewImageError::from_ffmpeg_error(e.code()));
        }
    };

    let Some(codec) = ff_sys::Codec::find_encoder(AVCodecID_AV_CODEC_ID_GIF) else {
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::UnsupportedCodec {
            codec: "gif".to_string(),
        });
    };

    let mut codec_ctx = match ff_sys::CodecContext::new(Some(codec)) {
        Ok(ctx) => ctx,
        Err(e) => {
            let mut g = graph;
            avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(PreviewImageError::from_ffmpeg_error(e.code()));
        }
    };

    // Pull a first frame to discover width/height/pix_fmt from the filter output.
    let Ok(mut first_frame) = ff_sys::Frame::new() else {
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::Ffmpeg {
            code: 0,
            message: "av_frame_alloc failed".to_string(),
        });
    };
    // `first_frame` drops on any early return below, freeing it (its ref-counted
    // buffer outlives the filter graph, so graph-then-frame teardown is sound).
    let outcome = ff_sys::buffersink_get_frame(sink_ctx, &mut first_frame);
    if !matches!(&outcome, Ok(ff_sys::BufferSinkOutcome::Frame)) {
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        let reason = match outcome {
            Err(e) => format!("no frames from GIF filter graph code={}", e.code()),
            _ => "no frames from GIF filter graph".to_string(),
        };
        return Err(PreviewImageError::OperationFailed { reason });
    }

    let out_width = first_frame.width();
    let out_height = first_frame.height();
    let out_pix_fmt = first_frame.format();

    // Configure GIF encoder.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fps_int = fps.round().max(1.0) as u32;
    codec_ctx.set_width(out_width);
    codec_ctx.set_height(out_height);
    codec_ctx.set_time_base(AVRational {
        num: 1,
        den: fps_int as i32,
    });
    codec_ctx.set_pix_fmt(out_pix_fmt);

    // Set GIF to loop infinitely (option "loop" = 0); unknown options are ignored.
    let _ = codec_ctx.set_opt("loop", "0");

    if let Err(e) = codec_ctx.open_codec(codec) {
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::from_ffmpeg_error(e.code()));
    }

    // Copy codec parameters to stream (width/height/format from the opened
    // GIF encoder context).
    if let Err(e) = fmt_ctx.apply_stream_params_from_context(stream_idx, &codec_ctx) {
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::from_ffmpeg_error(e.code()));
    }

    // Open output IO and write header.
    if let Err(e) = fmt_ctx.open_io(output) {
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::from_ffmpeg_error(e.code()));
    }

    if let Err(e) = fmt_ctx.write_header() {
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::from_ffmpeg_error(e.code()));
    }

    let Ok(mut packet) = ff_sys::Packet::new() else {
        let mut g = graph;
        avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(PreviewImageError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    };

    // ── Encode all frames ─────────────────────────────────────────────────────
    let encode_result = (|| -> Result<(), PreviewImageError> {
        let mut frame_counter: i64 = 0;

        // Encode the first frame we already pulled.
        first_frame.set_pts(frame_counter);
        frame_counter += 1;
        codec_ctx
            .send_frame(Some(&first_frame))
            .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;
        drain_packets(&mut codec_ctx, &mut fmt_ctx, &mut packet, false)?;

        // Pull and encode remaining frames.
        loop {
            let Ok(mut frame) = ff_sys::Frame::new() else {
                break;
            };
            if !matches!(
                ff_sys::buffersink_get_frame(sink_ctx, &mut frame),
                Ok(ff_sys::BufferSinkOutcome::Frame)
            ) {
                // `frame` drops here, freeing it.
                break;
            }
            frame.set_pts(frame_counter);
            frame_counter += 1;
            // `frame` drops at end of iteration (or on the `?` below), freeing it.
            codec_ctx
                .send_frame(Some(&frame))
                .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;
            drain_packets(&mut codec_ctx, &mut fmt_ctx, &mut packet, false)?;
        }

        // Flush encoder.
        codec_ctx
            .send_frame(None)
            .map_err(|e| PreviewImageError::from_ffmpeg_error(e.code()))?;
        drain_packets(&mut codec_ctx, &mut fmt_ctx, &mut packet, true)?;
        Ok(())
    })();

    // Finalise the GIF, then free the raw filter graph. `packet`, `first_frame`,
    // `codec_ctx`, and `fmt_ctx` free themselves at end of scope (the owned
    // context closes its IO on drop).
    let _ = fmt_ctx.write_trailer();
    fmt_ctx.close_io();
    let mut g = graph;
    avfilter_graph_free(std::ptr::addr_of_mut!(g));

    encode_result
}
