//! `FFmpeg` filter graph internals for audio/video analysis.
//!
//! All `unsafe` code lives here; [`super`] exposes safe wrappers.
//!
//! Current `unsafe` entry points:
//! - [`measure_loudness_unsafe`] — EBU R128 loudness via `ebur128` filter
//! - [`compute_ssim_unsafe`] — mean SSIM via `ssim` filter
//! - [`compute_psnr_unsafe`] — mean PSNR via `psnr` filter

#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::cast_possible_truncation)]

use std::ffi::CString;
use std::path::Path;

use crate::FilterError;
use crate::analysis::LoudnessResult;

/// Measures EBU R128 integrated loudness, loudness range, and true peak for
/// `path` using the filter graph:
///
/// `amovie=filename=path → ebur128=metadata=1:peak=true → abuffersink`
///
/// Drains all output frames, reading `lavfi.r128.*` metadata from each.
/// The last seen values for each key are returned.
///
/// # Safety
///
/// All raw pointer operations follow the avfilter ownership rules:
/// - `avfilter_graph_alloc()` returns an owned pointer freed via
///   `avfilter_graph_free()` on error or after draining.
/// - `avfilter_graph_create_filter()` adds contexts owned by the graph.
/// - `avfilter_link()` connects pads owned by the graph.
/// - `avfilter_graph_config()` finalises the graph.
/// - `av_frame_alloc()` / `av_frame_free()` manage frame lifetimes.
pub(super) unsafe fn measure_loudness_unsafe(path: &Path) -> Result<LoudnessResult, FilterError> {
    macro_rules! bail {
        ($graph:ident, $reason:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::AnalysisFailed {
                reason: format!("{}", $reason),
            });
        }};
    }

    // Escape for the amovie filter's `:`-separated option parser (Windows drive
    // colons / backslashes would otherwise break parsing).
    let path_str = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");
    let amovie_args =
        CString::new(format!("filename={path_str}")).map_err(|_| FilterError::AnalysisFailed {
            reason: "path contains null byte".to_string(),
        })?;

    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(FilterError::AnalysisFailed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // 1. amovie source — reads the file directly, no external decoder needed.
    let amovie_filt = ff_sys::avfilter_get_by_name(c"amovie".as_ptr());
    if amovie_filt.is_null() {
        bail!(graph, "filter not found: amovie");
    }
    let mut src_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut src_ctx,
        amovie_filt,
        c"loudness_src".as_ptr(),
        amovie_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("amovie create_filter failed code={ret}"));
    }

    // 2. ebur128=metadata=1:peak=true — writes R128 stats to AVFrame::metadata.
    let ebur128_filt = ff_sys::avfilter_get_by_name(c"ebur128".as_ptr());
    if ebur128_filt.is_null() {
        bail!(graph, "filter not found: ebur128");
    }
    let mut meas_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut meas_ctx,
        ebur128_filt,
        c"loudness_ebur128".as_ptr(),
        c"metadata=1:peak=true".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("ebur128 create_filter failed code={ret}"));
    }

    // 3. abuffersink
    let abuffersink_filt = ff_sys::avfilter_get_by_name(c"abuffersink".as_ptr());
    if abuffersink_filt.is_null() {
        bail!(graph, "filter not found: abuffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        abuffersink_filt,
        c"loudness_sink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            format!("abuffersink create_filter failed code={ret}")
        );
    }

    // Link: src → ebur128 → sink
    let ret = ff_sys::avfilter_link(src_ctx, 0, meas_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link src→ebur128 failed code={ret}")
        );
    }
    let ret = ff_sys::avfilter_link(meas_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link ebur128→sink failed code={ret}")
        );
    }

    // Configure the graph.
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // Drain all output frames; keep the last R128 metadata values seen.
    let mut integrated_lufs = f32::NEG_INFINITY;
    let mut lra_lu: f32 = 0.0;
    let mut true_peak_dbtp = f32::NEG_INFINITY;

    loop {
        let raw_frame = ff_sys::av_frame_alloc();
        if raw_frame.is_null() {
            break;
        }
        let ret = ff_sys::av_buffersink_get_frame(sink_ctx, raw_frame);
        if ret < 0 {
            let mut ptr = raw_frame;
            ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
            break;
        }
        // SAFETY: `(*raw_frame).metadata` is a valid `AVDictionary*` (may be null);
        // `av_dict_get` handles null dictionaries by returning null.
        read_f32_meta(raw_frame, c"lavfi.r128.I".as_ptr(), &mut integrated_lufs);
        read_f32_meta(raw_frame, c"lavfi.r128.LRA".as_ptr(), &mut lra_lu);
        read_f32_meta(
            raw_frame,
            c"lavfi.r128.true_peak".as_ptr(),
            &mut true_peak_dbtp,
        );
        let mut ptr = raw_frame;
        ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
    }

    let mut g = graph;
    ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));

    log::debug!(
        "loudness analysis complete integrated_lufs={integrated_lufs:.1} \
         lra_lu={lra_lu:.1} true_peak_dbtp={true_peak_dbtp:.1}"
    );

    Ok(LoudnessResult {
        integrated_lufs,
        lra: lra_lu,
        true_peak_dbtp,
    })
}

/// Reads a single `f32` metadata value from `frame` by `key`.
/// Updates `*out` only when the key is present and its value is parseable.
///
/// # Safety
///
/// - `frame` must point to a valid, fully-initialised `AVFrame`.
/// - `key` must be a null-terminated C string valid for the duration of the call.
unsafe fn read_f32_meta(
    frame: *mut ff_sys::AVFrame,
    key: *const std::os::raw::c_char,
    out: &mut f32,
) {
    let entry = ff_sys::av_dict_get((*frame).metadata, key, std::ptr::null(), 0);
    if !entry.is_null()
        && let Ok(s) = std::ffi::CStr::from_ptr((*entry).value).to_str()
        && let Ok(v) = s.parse::<f32>()
    {
        *out = v;
    }
}

// ── QualityMetrics inner ──────────────────────────────────────────────────────

/// Computes the mean SSIM between `reference` and `distorted` using the filter
/// graph:
///
/// `movie=reference [r]; movie=distorted [d]; [r][d] ssim=eof_action=endall → buffersink`
///
/// Drains all output frames, reading `lavfi.ssim.All` from each frame's
/// metadata.  Returns the arithmetic mean over all frames.
///
/// Performs a pre-flight frame-count check: if the two inputs have detectably
/// different frame counts, returns `FilterError::AnalysisFailed` before
/// building the filter graph.
///
/// # Safety
///
/// All raw pointer operations follow the avfilter ownership rules:
/// - `avfilter_graph_alloc()` returns an owned pointer freed via
///   `avfilter_graph_free()` on every exit path (bail! or normal).
/// - `avfilter_graph_create_filter()` adds contexts owned by the graph.
/// - `avfilter_link()` connects pads owned by the graph.
/// - `avfilter_graph_config()` finalises the graph.
/// - `av_frame_alloc()` / `av_frame_free()` manage per-frame lifetimes.
/// - Frame metadata is read via `av_dict_get`; the returned pointer is valid
///   for the lifetime of the frame.
pub(super) unsafe fn compute_ssim_unsafe(
    reference: &Path,
    distorted: &Path,
) -> Result<f32, FilterError> {
    // ── Pre-flight: reject inputs with different frame counts ──────────────
    let ref_count = probe_video_frame_count(reference);
    let dist_count = probe_video_frame_count(distorted);
    if let (Some(r), Some(d)) = (ref_count, dist_count)
        && (r - d).abs() > 1
    {
        return Err(FilterError::AnalysisFailed {
            reason: format!("frame count mismatch: reference={r} distorted={d}"),
        });
    }

    macro_rules! bail {
        ($graph:ident, $reason:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::AnalysisFailed {
                reason: format!("{}", $reason),
            });
        }};
    }

    let ref_str = reference.to_string_lossy();
    let dist_str = distorted.to_string_lossy();

    let ref_args =
        CString::new(format!("filename={ref_str}")).map_err(|_| FilterError::AnalysisFailed {
            reason: "reference path contains null byte".to_string(),
        })?;
    let dist_args =
        CString::new(format!("filename={dist_str}")).map_err(|_| FilterError::AnalysisFailed {
            reason: "distorted path contains null byte".to_string(),
        })?;

    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(FilterError::AnalysisFailed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // 1. movie source — reference video.
    let movie_filt = ff_sys::avfilter_get_by_name(c"movie".as_ptr());
    if movie_filt.is_null() {
        bail!(graph, "filter not found: movie");
    }
    let mut ref_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut ref_ctx,
        movie_filt,
        c"ssim_ref".as_ptr(),
        ref_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            format!("movie(reference) create_filter failed code={ret}")
        );
    }

    // 2. movie source — distorted video.
    let mut dist_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut dist_ctx,
        movie_filt,
        c"ssim_dist".as_ptr(),
        dist_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            format!("movie(distorted) create_filter failed code={ret}")
        );
    }

    // 3. ssim=eof_action=endall — annotates output frames with lavfi.ssim.* metadata.
    let ssim_filt = ff_sys::avfilter_get_by_name(c"ssim".as_ptr());
    if ssim_filt.is_null() {
        bail!(graph, "filter not found: ssim");
    }
    let mut ssim_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut ssim_ctx,
        ssim_filt,
        c"ssim_compute".as_ptr(),
        c"eof_action=endall".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("ssim create_filter failed code={ret}"));
    }

    // 4. buffersink — drains ssim output frames so we can read their metadata.
    let buffersink_filt = ff_sys::avfilter_get_by_name(c"buffersink".as_ptr());
    if buffersink_filt.is_null() {
        bail!(graph, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        buffersink_filt,
        c"ssim_sink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("buffersink create_filter failed code={ret}"));
    }

    // Link: ref_ctx[0] → ssim[0], dist_ctx[0] → ssim[1], ssim[0] → sink
    let ret = ff_sys::avfilter_link(ref_ctx, 0, ssim_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link ref→ssim[0] failed code={ret}")
        );
    }
    let ret = ff_sys::avfilter_link(dist_ctx, 0, ssim_ctx, 1);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link dist→ssim[1] failed code={ret}")
        );
    }
    let ret = ff_sys::avfilter_link(ssim_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(graph, format!("avfilter_link ssim→sink failed code={ret}"));
    }

    // Configure the graph.
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // Drain all frames; collect per-frame lavfi.ssim.All values.
    let mut ssim_sum: f64 = 0.0;
    let mut frame_count: u64 = 0;

    loop {
        let raw_frame = ff_sys::av_frame_alloc();
        if raw_frame.is_null() {
            break;
        }
        let ret = ff_sys::av_buffersink_get_frame(sink_ctx, raw_frame);
        if ret < 0 {
            let mut ptr = raw_frame;
            ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
            break;
        }

        // SAFETY: `(*raw_frame).metadata` is a valid `AVDictionary*` (may be null);
        // `av_dict_get` handles null dictionaries by returning null.
        let mut ssim_val = 0.0f32;
        read_f32_meta(raw_frame, c"lavfi.ssim.All".as_ptr(), &mut ssim_val);
        if ssim_val > 0.0 {
            ssim_sum += f64::from(ssim_val);
            frame_count += 1;
        }

        let mut ptr = raw_frame;
        ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
    }

    let mut g = graph;
    ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));

    if frame_count == 0 {
        return Err(FilterError::AnalysisFailed {
            reason: "no frames were compared (empty or incompatible inputs)".to_string(),
        });
    }

    #[allow(clippy::cast_precision_loss)]
    let mean_ssim = (ssim_sum / frame_count as f64) as f32;

    log::debug!("ssim complete mean={mean_ssim:.6} frames={frame_count}");

    Ok(mean_ssim)
}

/// Computes the mean PSNR (Peak Signal-to-Noise Ratio, in dB) between
/// `reference` and `distorted` using the filter graph:
///
/// `movie=reference [r]; movie=distorted [d]; [r][d] psnr=eof_action=endall → buffersink`
///
/// Drains all output frames, reading `lavfi.psnr.psnr.y` (luminance PSNR) from
/// each frame's metadata.  Returns the arithmetic mean over all frames.
///
/// When both inputs are identical every frame has MSE=0 and therefore infinite
/// PSNR; in that case this function returns `f32::INFINITY`.
///
/// Performs a pre-flight frame-count check identical to [`compute_ssim_unsafe`].
///
/// # Safety
///
/// All raw pointer operations follow the avfilter ownership rules:
/// - `avfilter_graph_alloc()` returns an owned pointer freed via
///   `avfilter_graph_free()` on every exit path (bail! or normal).
/// - `avfilter_graph_create_filter()` adds contexts owned by the graph.
/// - `avfilter_link()` connects pads owned by the graph.
/// - `avfilter_graph_config()` finalises the graph.
/// - `av_frame_alloc()` / `av_frame_free()` manage per-frame lifetimes.
/// - Frame metadata is read via `av_dict_get`; the returned pointer is valid
///   for the lifetime of the frame.
pub(super) unsafe fn compute_psnr_unsafe(
    reference: &Path,
    distorted: &Path,
) -> Result<f32, FilterError> {
    // ── Pre-flight: reject inputs with different frame counts ──────────────
    let ref_count = probe_video_frame_count(reference);
    let dist_count = probe_video_frame_count(distorted);
    if let (Some(r), Some(d)) = (ref_count, dist_count)
        && (r - d).abs() > 1
    {
        return Err(FilterError::AnalysisFailed {
            reason: format!("frame count mismatch: reference={r} distorted={d}"),
        });
    }

    macro_rules! bail {
        ($graph:ident, $reason:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(FilterError::AnalysisFailed {
                reason: format!("{}", $reason),
            });
        }};
    }

    let ref_str = reference.to_string_lossy();
    let dist_str = distorted.to_string_lossy();

    let ref_args =
        CString::new(format!("filename={ref_str}")).map_err(|_| FilterError::AnalysisFailed {
            reason: "reference path contains null byte".to_string(),
        })?;
    let dist_args =
        CString::new(format!("filename={dist_str}")).map_err(|_| FilterError::AnalysisFailed {
            reason: "distorted path contains null byte".to_string(),
        })?;

    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(FilterError::AnalysisFailed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // 1. movie source — reference video.
    let movie_filt = ff_sys::avfilter_get_by_name(c"movie".as_ptr());
    if movie_filt.is_null() {
        bail!(graph, "filter not found: movie");
    }
    let mut ref_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut ref_ctx,
        movie_filt,
        c"psnr_ref".as_ptr(),
        ref_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            format!("movie(reference) create_filter failed code={ret}")
        );
    }

    // 2. movie source — distorted video.
    let mut dist_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut dist_ctx,
        movie_filt,
        c"psnr_dist".as_ptr(),
        dist_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            format!("movie(distorted) create_filter failed code={ret}")
        );
    }

    // 3. psnr=eof_action=endall — annotates output frames with lavfi.psnr.* metadata.
    let psnr_filt = ff_sys::avfilter_get_by_name(c"psnr".as_ptr());
    if psnr_filt.is_null() {
        bail!(graph, "filter not found: psnr");
    }
    let mut psnr_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut psnr_ctx,
        psnr_filt,
        c"psnr_compute".as_ptr(),
        c"eof_action=endall".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("psnr create_filter failed code={ret}"));
    }

    // 4. buffersink — drains psnr output frames so we can read their metadata.
    let buffersink_filt = ff_sys::avfilter_get_by_name(c"buffersink".as_ptr());
    if buffersink_filt.is_null() {
        bail!(graph, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        buffersink_filt,
        c"psnr_sink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("buffersink create_filter failed code={ret}"));
    }

    // Link: ref_ctx[0] → psnr[0], dist_ctx[0] → psnr[1], psnr[0] → sink
    let ret = ff_sys::avfilter_link(ref_ctx, 0, psnr_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link ref→psnr[0] failed code={ret}")
        );
    }
    let ret = ff_sys::avfilter_link(dist_ctx, 0, psnr_ctx, 1);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link dist→psnr[1] failed code={ret}")
        );
    }
    let ret = ff_sys::avfilter_link(psnr_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(graph, format!("avfilter_link psnr→sink failed code={ret}"));
    }

    // Configure the graph.
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // Drain all frames; collect per-frame lavfi.psnr.psnr.y values.
    // Use NEG_INFINITY as a sentinel (actual PSNR is always ≥ 0 or +infinity).
    let mut psnr_sum: f64 = 0.0;
    let mut frame_count: u64 = 0;

    loop {
        let raw_frame = ff_sys::av_frame_alloc();
        if raw_frame.is_null() {
            break;
        }
        let ret = ff_sys::av_buffersink_get_frame(sink_ctx, raw_frame);
        if ret < 0 {
            let mut ptr = raw_frame;
            ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
            break;
        }

        // SAFETY: `(*raw_frame).metadata` is a valid `AVDictionary*` (may be null);
        // `av_dict_get` handles null dictionaries by returning null.
        let mut psnr_val = f32::NEG_INFINITY; // sentinel: "key not present"
        read_f32_meta(raw_frame, c"lavfi.psnr.psnr.y".as_ptr(), &mut psnr_val);
        if psnr_val > f32::NEG_INFINITY {
            psnr_sum += f64::from(psnr_val);
            frame_count += 1;
        }

        let mut ptr = raw_frame;
        ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
    }

    let mut g = graph;
    ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));

    if frame_count == 0 {
        return Err(FilterError::AnalysisFailed {
            reason: "no frames were compared (empty or incompatible inputs)".to_string(),
        });
    }

    #[allow(clippy::cast_precision_loss)]
    let mean_psnr = (psnr_sum / frame_count as f64) as f32;

    log::debug!("psnr complete mean={mean_psnr:.3} frames={frame_count}");

    Ok(mean_psnr)
}

/// Returns the estimated frame count for the first video stream in `path`.
///
/// Uses `AVStream.nb_frames` when non-zero; otherwise estimates from stream
/// `duration × r_frame_rate`.  Returns `None` when the file cannot be opened,
/// no video stream is found, or the duration is unknown.
///
/// # Safety
///
/// - All `AVFormatContext` resources are released before returning.
/// - Only the first video stream's metadata is accessed (no data read).
unsafe fn probe_video_frame_count(path: &Path) -> Option<i64> {
    let mut fmt_ctx = ff_sys::avformat::open_input(path).ok()?;

    // Ignore find_stream_info errors; partial info is still usable.
    let _ = ff_sys::avformat::find_stream_info(fmt_ctx);

    let nb_streams = (*fmt_ctx).nb_streams;
    let mut result: Option<i64> = None;

    for i in 0..nb_streams {
        // SAFETY: streams is a valid array of `nb_streams` pointers.
        let stream = *(*fmt_ctx).streams.add(i as usize);
        if (*(*stream).codecpar).codec_type != ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO {
            continue;
        }

        if (*stream).nb_frames > 0 {
            result = Some((*stream).nb_frames);
        } else {
            // Fall back to duration × frame_rate.
            let dur = (*stream).duration;
            let tb = (*stream).time_base;
            let fps = (*stream).r_frame_rate;

            #[allow(clippy::cast_precision_loss)]
            if dur != ff_sys::AV_NOPTS_VALUE && tb.den > 0 && fps.den > 0 {
                let dur_secs = dur as f64 * f64::from(tb.num) / f64::from(tb.den);
                let fps_val = f64::from(fps.num) / f64::from(fps.den);
                result = Some((dur_secs * fps_val).round() as i64);
            }
        }
        break; // Only inspect the first video stream.
    }

    ff_sys::avformat::close_input(std::ptr::addr_of_mut!(fmt_ctx));
    result
}
