//! `FFmpeg` filter graph internals for whole-file video effects.
//!
//! All `unsafe` code lives here; [`super`] exposes safe wrappers.
//!
//! Current `unsafe` entry points:
//! - [`analyze_vidstab_unsafe`] — motion analysis via `vidstabdetect` filter
//! - [`transform_vidstab_unsafe`] — correction via `vidstabtransform` filter + encode

#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::CString;
use std::path::Path;

use crate::FilterError;
use crate::effects::stabilizer::{AnalyzeOptions, Interpolation, StabilizeOptions};

/// Runs `vidstabdetect` motion analysis on `input`, writing the transform data
/// to `output_trf`.
///
/// Builds and drains the filter graph:
/// `movie=filename={input} → vidstabdetect=... → nullsink`
///
/// The `.trf` file is written as a side effect by `vidstabdetect` as frames
/// pass through it.  `avfilter_graph_request_oldest` drives the graph one
/// frame at a time until EOF.
///
/// # Safety
///
/// All raw pointer operations follow the avfilter ownership rules:
/// - `avfilter_graph_alloc()` returns an owned pointer freed via
///   `avfilter_graph_free()` on every exit path (bail! or normal).
/// - `avfilter_graph_create_filter()` adds filter contexts owned by the graph.
/// - `avfilter_link()` connects pads owned by the graph.
/// - `avfilter_graph_config()` finalises the graph.
/// - All `CString` values are kept alive for the duration of the graph build.
pub(super) unsafe fn analyze_vidstab_unsafe(
    input: &Path,
    output_trf: &Path,
    opts: &AnalyzeOptions,
) -> Result<(), FilterError> {
    macro_rules! bail {
        ($graph:ident, $code:expr, $msg:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::Ffmpeg {
                code: $code,
                message: $msg.to_string(),
            });
        }};
    }

    // Pre-flight: check that vidstabdetect is available in this FFmpeg build.
    // SAFETY: c"vidstabdetect" is a valid null-terminated C string literal.
    let vidstab_filt = ff_sys::avfilter_get_by_name(c"vidstabdetect".as_ptr());
    if vidstab_filt.is_null() {
        return Err(FilterError::Ffmpeg {
            code: 0,
            message: "vidstabdetect filter not available in this FFmpeg build".to_string(),
        });
    }

    // Build CString args before allocating the graph. Escape paths for the
    // filter's `:`-separated option parser (movie `filename=` and vidstabdetect
    // `result=`): Windows drive colons / backslashes would otherwise break it.
    let input_str = input
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");
    let trf_str = output_trf
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");

    let movie_args =
        CString::new(format!("filename={input_str}")).map_err(|_| FilterError::Ffmpeg {
            code: 0,
            message: "input path contains null byte".to_string(),
        })?;

    let shakiness = opts.shakiness.clamp(1, 10);
    let accuracy = opts.accuracy.clamp(1, 15);
    let stepsize = opts.stepsize.clamp(1, 32);

    let vidstab_args = CString::new(format!(
        "shakiness={shakiness}:accuracy={accuracy}:stepsize={stepsize}:result={trf_str}"
    ))
    .map_err(|_| FilterError::Ffmpeg {
        code: 0,
        message: "trf path contains null byte".to_string(),
    })?;

    // Allocate the filter graph.
    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(FilterError::Ffmpeg {
            code: 0,
            message: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // 1. movie source — reads the input file directly through lavfi.
    let movie_filt = ff_sys::avfilter_get_by_name(c"movie".as_ptr());
    if movie_filt.is_null() {
        bail!(graph, 0, "filter not found: movie");
    }
    let mut src_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut src_ctx,
        movie_filt,
        c"vidstab_src".as_ptr(),
        movie_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, ret, format!("movie create_filter failed code={ret}"));
    }

    // 2. vidstabdetect — writes transform data to the .trf file as a side effect.
    log::debug!(
        "filter added name=vidstabdetect args={}",
        vidstab_args.to_string_lossy()
    );
    let mut vstab_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut vstab_ctx,
        vidstab_filt,
        c"vidstab_detect".as_ptr(),
        vidstab_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            ret,
            format!("vidstabdetect create_filter failed code={ret}")
        );
    }

    // 3. buffersink — drains output frames; vidstabdetect writes the .trf file
    //    as frames pass through it, so the sink content is discarded.
    let buffersink_filt = ff_sys::avfilter_get_by_name(c"buffersink".as_ptr());
    if buffersink_filt.is_null() {
        bail!(graph, 0, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        buffersink_filt,
        c"vidstab_sink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            ret,
            format!("buffersink create_filter failed code={ret}")
        );
    }

    // Link: movie → vidstabdetect → buffersink
    let ret = ff_sys::avfilter_link(src_ctx, 0, vstab_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            ret,
            format!("avfilter_link movie→vidstabdetect failed code={ret}")
        );
    }
    let ret = ff_sys::avfilter_link(vstab_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            ret,
            format!("avfilter_link vidstabdetect→buffersink failed code={ret}")
        );
    }

    // Configure the graph — opens the input file via the movie filter.
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        bail!(
            graph,
            ret,
            format!(
                "avfilter_graph_config failed code={ret} message={}",
                ff_sys::av_error_string(ret)
            )
        );
    }

    // Drain all frames.  av_buffersink_get_frame pulls one frame per call;
    // vidstabdetect writes the .trf file incrementally as each frame passes
    // through it.  We discard the frame data — only the side effect matters.
    loop {
        let Ok(mut frame) = ff_sys::Frame::new() else {
            break;
        };
        // SAFETY: sink_ctx is a valid buffersink context; `frame` is an owned
        // frame. The frame data is discarded (only the .trf side effect matters);
        // `frame` drops at the end of each iteration.
        match ff_sys::buffersink_get_frame(sink_ctx, &mut frame) {
            Ok(ff_sys::BufferSinkOutcome::Frame) => {}
            // NeedMore / Drained / Err: all frames processed.
            _ => break,
        }
    }

    let mut g = graph;
    ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));

    log::info!(
        "stabilization analysis complete trf={}",
        output_trf.display()
    );

    Ok(())
}

// ── Pass 2 — transform ────────────────────────────────────────────────────────

/// Drain all available encoded packets from `enc_ctx` into `out_ctx`, rescaling
/// timestamps from the encoder's time base to the output stream's time base.
///
/// `out_ctx`'s header must have been written; `enc_ctx` should have had at least
/// one `send_frame` call (this drains whatever the encoder has produced).
fn drain_encoded_packets(
    enc_ctx: &mut ff_sys::CodecContext,
    pkt: &mut ff_sys::Packet,
    out_ctx: &mut ff_sys::OutputFormatContext,
    stream_tb: ff_sys::AVRational,
) {
    while matches!(
        enc_ctx.receive_packet(pkt),
        Ok(ff_sys::ReceiveOutcome::Frame)
    ) {
        // Read enc_tb at drain time — some encoders mutate time_base lazily.
        let enc_tb = enc_ctx.time_base();
        pkt.rescale_ts(enc_tb, stream_tb);
        pkt.set_stream_index(0);
        let write_res = out_ctx.write_interleaved(pkt);
        pkt.unref();
        if let Err(e) = write_res {
            log::warn!(
                "av_interleaved_write_frame failed error={}",
                ff_sys::av_error_string(e.code())
            );
            break;
        }
    }
}

/// Runs `vidstabtransform` motion correction on `input` using the `.trf` file
/// produced by [`analyze_vidstab_unsafe`], writing the result to `output`.
///
/// Filter graph: `movie={input} → vidstabtransform=... → format=yuv420p → buffersink`
///
/// Decoded frames are re-encoded with the best available H.264 encoder and
/// multiplexed into `output` (format inferred from the file extension).
///
/// # Safety
///
/// All raw pointer operations follow the avfilter / avcodec / avformat ownership
/// rules. Every allocated resource is freed on all exit paths:
/// - `avfilter_graph_alloc()` / `avfilter_graph_free()` for the filter graph.
/// - `avcodec_alloc_context3()` / `avcodec_free_context()` for the encoder.
/// - Owned `ff_sys::OutputFormatContext` for the muxer and its output file I/O
///   (it allocates the context, opens/closes the `pb`, and frees the context
///   once on drop).
/// - Owned `ff_sys::Frame` / `ff_sys::Packet` manage the frame and packet
///   lifetimes (each freed once on drop).
pub(super) unsafe fn transform_vidstab_unsafe(
    input: &Path,
    trf_path: &Path,
    output: &Path,
    opts: &StabilizeOptions,
) -> Result<(), FilterError> {
    macro_rules! bail {
        ($graph:ident, $code:expr, $msg:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::Ffmpeg {
                code: $code,
                message: format!("{}", $msg),
            });
        }};
    }

    // ── Pre-flight: check that vidstabtransform is available ──────────────────
    let vstab_filt = ff_sys::avfilter_get_by_name(c"vidstabtransform".as_ptr());
    if vstab_filt.is_null() {
        return Err(FilterError::Ffmpeg {
            code: 0,
            message: "vidstabtransform filter not available in this FFmpeg build".to_string(),
        });
    }

    // ── Build CString arguments before allocating the graph ───────────────────
    // Escape paths for the filter's `:`-separated option parser (movie
    // `filename=` and vidstabtransform `input=`): Windows drive colons /
    // backslashes would otherwise break it.
    let input_str = input
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");
    let trf_str = trf_path
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");

    let movie_args =
        CString::new(format!("filename={input_str}")).map_err(|_| FilterError::Ffmpeg {
            code: 0,
            message: "input path contains null byte".to_string(),
        })?;

    let crop_str = if opts.crop_black { "black" } else { "keep" };
    let interpol_str = match opts.interpol {
        Interpolation::Bilinear => "bilinear",
        Interpolation::Bicubic => "bicubic",
    };
    let smoothing = opts.smoothing;
    let zoom = opts.zoom;
    let optzoom = opts.optzoom.clamp(0, 2);

    let vstab_args = CString::new(format!(
        "input={trf_str}:smoothing={smoothing}:crop={crop_str}:zoom={zoom}:optzoom={optzoom}:interpol={interpol_str}"
    ))
    .map_err(|_| FilterError::Ffmpeg {
        code: 0,
        message: "trf path contains null byte".to_string(),
    })?;

    // ── Allocate the filter graph ─────────────────────────────────────────────
    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(FilterError::Ffmpeg {
            code: 0,
            message: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // ── 1. movie source ───────────────────────────────────────────────────────
    let movie_filt = ff_sys::avfilter_get_by_name(c"movie".as_ptr());
    if movie_filt.is_null() {
        bail!(graph, 0, "filter not found: movie");
    }
    let mut src_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut src_ctx,
        movie_filt,
        c"vstab_t_src".as_ptr(),
        movie_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, ret, format!("movie create_filter failed code={ret}"));
    }

    // ── 2. vidstabtransform ───────────────────────────────────────────────────
    log::debug!(
        "filter added name=vidstabtransform args={}",
        vstab_args.to_string_lossy()
    );
    let mut vstab_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut vstab_ctx,
        vstab_filt,
        c"vstab_t_filter".as_ptr(),
        vstab_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            ret,
            format!("vidstabtransform create_filter failed code={ret}")
        );
    }

    // ── 3. format — normalize to yuv420p for encoder compatibility ────────────
    let format_filt = ff_sys::avfilter_get_by_name(c"format".as_ptr());
    if format_filt.is_null() {
        bail!(graph, 0, "filter not found: format");
    }
    let mut fmt_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut fmt_ctx,
        format_filt,
        c"vstab_t_fmt".as_ptr(),
        c"pix_fmts=yuv420p".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            ret,
            format!("format create_filter failed code={ret}")
        );
    }

    // ── 4. buffersink ─────────────────────────────────────────────────────────
    let buffersink_filt = ff_sys::avfilter_get_by_name(c"buffersink".as_ptr());
    if buffersink_filt.is_null() {
        bail!(graph, 0, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        buffersink_filt,
        c"vstab_t_sink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            ret,
            format!("buffersink create_filter failed code={ret}")
        );
    }

    // ── Link: movie → vidstabtransform → format → buffersink ─────────────────
    let ret = ff_sys::avfilter_link(src_ctx, 0, vstab_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            ret,
            format!("avfilter_link movie→vidstabtransform failed code={ret}")
        );
    }
    let ret = ff_sys::avfilter_link(vstab_ctx, 0, fmt_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            ret,
            format!("avfilter_link vidstabtransform→format failed code={ret}")
        );
    }
    let ret = ff_sys::avfilter_link(fmt_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            ret,
            format!("avfilter_link format→buffersink failed code={ret}")
        );
    }

    // ── Configure the graph — opens input file and validates the trf path ─────
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        bail!(
            graph,
            ret,
            format!(
                "avfilter_graph_config failed code={ret} message={}",
                ff_sys::av_error_string(ret)
            )
        );
    }

    // After graph config the output time base is stable.
    let filter_tb = ff_sys::av_buffersink_get_time_base(sink_ctx);

    // ── From here manual cleanup is required (encoder + muxer are allocated) ──

    // Pull the first frame to discover frame dimensions.
    let Ok(mut first_frame) = ff_sys::Frame::new() else {
        let mut g = graph;
        ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(FilterError::Ffmpeg {
            code: 0,
            message: "av_frame_alloc failed".to_string(),
        });
    };
    // `first_frame` drops on any early return below, freeing it.
    let outcome = ff_sys::buffersink_get_frame(sink_ctx, &mut first_frame);
    if !matches!(&outcome, Ok(ff_sys::BufferSinkOutcome::Frame)) {
        let mut g = graph;
        ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
        let code = match outcome {
            Err(e) => e.code(),
            _ => 0,
        };
        return Err(FilterError::Ffmpeg {
            code,
            message: format!(
                "first frame pull failed code={code} message={}",
                ff_sys::av_error_string(code)
            ),
        });
    }
    let frame_width = first_frame.width();
    let frame_height = first_frame.height();

    // ── Find the best available H.264 encoder ─────────────────────────────────
    let enc_codec = {
        let candidates: &[&str] = &[
            "h264_nvenc",
            "h264_qsv",
            "h264_amf",
            "h264_videotoolbox",
            "libx264",
            "mpeg4",
        ];
        let mut found: Option<ff_sys::Codec> = None;
        for name in candidates {
            if let Some(c) = ff_sys::Codec::find_encoder_by_name(name) {
                log::info!("stabilization transform selected encoder encoder={name}");
                found = Some(c);
                break;
            }
        }
        if let Some(c) = found {
            c
        } else {
            let mut g = graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::Ffmpeg {
                code: 0,
                message: "no H.264 encoder available (tried h264_nvenc, libx264, mpeg4, etc.)"
                    .to_string(),
            });
        }
    };

    // ── Allocate and configure the encoder context ────────────────────────────
    let mut enc_ctx = match ff_sys::CodecContext::new(Some(enc_codec)) {
        Ok(ctx) => ctx,
        Err(e) => {
            let mut g = graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::Ffmpeg {
                code: e.code(),
                message: "avcodec_alloc_context3 failed".to_string(),
            });
        }
    };

    enc_ctx.set_width(frame_width);
    enc_ctx.set_height(frame_height);
    enc_ctx.set_pix_fmt(ff_sys::AVPixelFormat_AV_PIX_FMT_YUV420P);
    enc_ctx.set_time_base(filter_tb);

    if let Err(e) = enc_ctx.open(enc_codec, std::ptr::null_mut()) {
        let mut g = graph;
        ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(FilterError::Ffmpeg {
            code: e.code(),
            message: format!("avcodec_open2 failed code={}", e.code()),
        });
    }

    // ── Allocate the output format context (owned) ────────────────────────────
    // The owned context frees itself and closes its IO on drop; each early return
    // below still frees the raw filter graph explicitly. Because the graph is raw,
    // out_ctx operations use explicit `match`/`if let` (not `?`).
    let mut out_ctx = match ff_sys::OutputFormatContext::new(None, output) {
        Ok(c) => c,
        Err(e) => {
            let mut g = graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::Ffmpeg {
                code: e.code(),
                message: format!("avformat_alloc_output_context2 failed code={}", e.code()),
            });
        }
    };

    // ── Add video stream and copy codec parameters ────────────────────────────
    let Ok(out_idx) = out_ctx.new_stream(None) else {
        let mut g = graph;
        ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(FilterError::Ffmpeg {
            code: 0,
            message: "avformat_new_stream failed".to_string(),
        });
    };

    if let Err(e) = out_ctx.apply_stream_params_from_context(out_idx, &enc_ctx) {
        let mut g = graph;
        ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(FilterError::Ffmpeg {
            code: e.code(),
            message: format!("avcodec_parameters_from_context failed code={}", e.code()),
        });
    }

    // ── Open the output file ──────────────────────────────────────────────────
    if let Err(code) = out_ctx.open_io(output) {
        let code = code.code();
        let mut g = graph;
        ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(FilterError::Ffmpeg {
            code,
            message: format!("avio_open failed code={code}"),
        });
    }

    // ── Write container header ────────────────────────────────────────────────
    if let Err(e) = out_ctx.write_header() {
        let mut g = graph;
        ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(FilterError::Ffmpeg {
            code: e.code(),
            message: format!("avformat_write_header failed code={}", e.code()),
        });
    }

    // Read stream time base AFTER write_header — the muxer may adjust it.
    let stream_tb = out_ctx.stream_time_base(out_idx);

    // ── Allocate encode packet ────────────────────────────────────────────────
    let Ok(mut pkt) = ff_sys::Packet::new() else {
        let _ = out_ctx.write_trailer();
        let mut g = graph;
        ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
        return Err(FilterError::Ffmpeg {
            code: 0,
            message: "av_packet_alloc failed".to_string(),
        });
    };

    // ── Encode first frame ────────────────────────────────────────────────────
    let _ = enc_ctx.send_frame(Some(&first_frame));
    drain_encoded_packets(&mut enc_ctx, &mut pkt, &mut out_ctx, stream_tb);
    drop(first_frame);

    // ── Encode remaining frames from the filter graph ─────────────────────────
    loop {
        let Ok(mut frame) = ff_sys::Frame::new() else {
            break;
        };
        if !matches!(
            ff_sys::buffersink_get_frame(sink_ctx, &mut frame),
            Ok(ff_sys::BufferSinkOutcome::Frame)
        ) {
            break;
        }
        let _ = enc_ctx.send_frame(Some(&frame));
        drain_encoded_packets(&mut enc_ctx, &mut pkt, &mut out_ctx, stream_tb);
        // `frame` drops at end of iteration.
    }

    // ── Flush the encoder ─────────────────────────────────────────────────────
    let _ = enc_ctx.send_frame(None);
    drain_encoded_packets(&mut enc_ctx, &mut pkt, &mut out_ctx, stream_tb);

    // ── Finalize output ───────────────────────────────────────────────────────
    // `pkt` drops at end of scope, freeing it. The owned `out_ctx` closes its IO
    // and frees itself when it drops at scope end; only the raw filter graph
    // needs an explicit free here.
    let _ = out_ctx.write_trailer();

    let mut g = graph;
    ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));

    log::info!(
        "stabilization transform complete output={}",
        output.display()
    );

    Ok(())
}
