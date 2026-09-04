//! Inner implementation details for media analysis tools.
//!
//! All `unsafe` `FFmpeg` filter-graph and packet-level calls live here.
//! Per-type safe APIs live in sibling files (`scene_detector.rs`, etc.)
//! and call the entry points below directly, so no sibling needs an `unsafe`
//! block. Each entry point is a safe wrapper over a private `*_unsafe` body.
//!
//! Entry points:
//! - [`detect_scenes`] — `SceneDetector`
//! - [`detect_silence`] — `SilenceDetector`
//! - [`enumerate_keyframes`] — `KeyframeEnumerator`
//! - [`detect_black_frames`] — `BlackFrameDetector`

#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::too_many_lines)]

use std::ffi::CString;
use std::path::Path;
use std::time::Duration;

use super::silence_detector::SilenceRange;
use crate::AnalysisError;

// SceneDetector inner

/// Safe entry point for [`detect_scenes_unsafe`].
///
/// # Safety argument (RK-017)
///
/// Takes no raw pointer and imposes no precondition on the caller: the
/// callee's `# Safety` section documents only its own internal ownership
/// rules, and every pointer it creates it also owns and frees. No value of
/// these argument types can cause UB, so an `unsafe` marker here would
/// communicate nothing actionable to a caller.
pub(super) fn detect_scenes(path: &Path, threshold: f64) -> Result<Vec<Duration>, AnalysisError> {
    // SAFETY: see the safety argument above.
    unsafe { detect_scenes_unsafe(path, threshold) }
}

/// Detects scene changes in `path` using the filter graph:
///
/// `movie=filename=path → select=gt(scene\,threshold) → buffersink`
///
/// Drains all output frames and returns their PTS as [`Duration`] values.
/// Only frames that survive the `select` filter (i.e. whose scene score
/// exceeds `threshold`) appear in the output.
///
/// # Safety
///
/// All raw pointer operations follow the avfilter ownership rules:
/// - `avfilter_graph_alloc()` returns an owned pointer freed via
///   `avfilter_graph_free()` on error or after draining.
/// - `avfilter_graph_create_filter()` adds contexts owned by the graph.
/// - `avfilter_link()` connects pads owned by the graph.
/// - `avfilter_graph_config()` finalises the graph.
/// - `av_frame_alloc()` / `av_frame_free()` manage per-frame lifetimes.
/// - `(*frame).time_base` is set by the filter framework inside
///   `av_buffersink_get_frame` and is valid for the frame's lifetime.
unsafe fn detect_scenes_unsafe(
    path: &Path,
    threshold: f64,
) -> Result<Vec<Duration>, AnalysisError> {
    macro_rules! bail {
        ($graph:ident, $reason:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(AnalysisError::Failed {
                reason: format!("{}", $reason),
            });
        }};
    }

    // On Windows, paths contain backslashes and a drive-letter colon (e.g.
    // "D:\…").  FFmpeg's filter-option parser uses ":" as a key=value separator,
    // so the colon must be escaped as "\:" and backslashes converted to "/".
    let path_str = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");
    let movie_args =
        CString::new(format!("filename={path_str}")).map_err(|_| AnalysisError::Failed {
            reason: "path contains null byte".to_string(),
        })?;

    // select=gt(scene\,{threshold}) — the backslash escapes the comma inside
    // the filter option string so FFmpeg's option parser treats it as a literal
    // comma rather than a separator between filter options.
    let select_args =
        CString::new(format!("gt(scene\\,{threshold:.6})")).map_err(|_| AnalysisError::Failed {
            reason: "select args contained null byte".to_string(),
        })?;

    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(AnalysisError::Failed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // 1. movie source — reads the video file directly.
    let movie_filt = ff_sys::avfilter_get_by_name(c"movie".as_ptr());
    if movie_filt.is_null() {
        bail!(graph, "filter not found: movie");
    }
    let mut src_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut src_ctx,
        movie_filt,
        c"scene_src".as_ptr(),
        movie_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("movie create_filter failed code={ret}"));
    }

    // 2. select=gt(scene\,threshold) — passes only scene-change frames.
    let select_filt = ff_sys::avfilter_get_by_name(c"select".as_ptr());
    if select_filt.is_null() {
        bail!(graph, "filter not found: select");
    }
    let mut sel_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sel_ctx,
        select_filt,
        c"scene_select".as_ptr(),
        select_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("select create_filter failed code={ret}"));
    }

    // 3. buffersink — collects output frames.
    let buffersink_filt = ff_sys::avfilter_get_by_name(c"buffersink".as_ptr());
    if buffersink_filt.is_null() {
        bail!(graph, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        buffersink_filt,
        c"scene_sink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("buffersink create_filter failed code={ret}"));
    }

    // Link: src → select → sink
    let ret = ff_sys::avfilter_link(src_ctx, 0, sel_ctx, 0);
    if ret < 0 {
        bail!(graph, format!("avfilter_link src→select failed code={ret}"));
    }
    let ret = ff_sys::avfilter_link(sel_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link select→sink failed code={ret}")
        );
    }

    // Configure the graph.
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // Read the output time base from the sink context once after configuration.
    // The frame's own `time_base` field is not reliably populated by all FFmpeg
    // versions / platforms, so we use `av_buffersink_get_time_base` instead.
    //
    // SAFETY: `sink_ctx` is a fully configured `AVFilterContext*` produced by
    // `avfilter_graph_create_filter` and validated by `avfilter_graph_config`.
    let sink_time_base = ff_sys::av_buffersink_get_time_base(sink_ctx);
    let tb_num = f64::from(sink_time_base.num);
    let tb_den = f64::from(sink_time_base.den);

    // Drain all output frames; each frame that exits the select filter
    // represents a detected scene change.
    let mut timestamps: Vec<Duration> = Vec::new();

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

        // SAFETY: `raw_frame` was filled by `av_buffersink_get_frame`; its
        // `pts` field is the presentation timestamp in `sink_time_base` units.
        let pts = (*raw_frame).pts;
        if pts != ff_sys::AV_NOPTS_VALUE && tb_den > 0.0 {
            let secs = pts as f64 * tb_num / tb_den;
            if secs >= 0.0 {
                timestamps.push(Duration::from_secs_f64(secs));
            }
        }

        let mut ptr = raw_frame;
        ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
    }

    let mut g = graph;
    ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));

    log::debug!(
        "scene detection complete scenes={} threshold={threshold:.4}",
        timestamps.len()
    );

    Ok(timestamps)
}

// SilenceDetector inner

/// Safe entry point for [`detect_silence_unsafe`].
///
/// # Safety argument (RK-017)
///
/// Takes no raw pointer and imposes no precondition on the caller: the
/// callee's `# Safety` section documents only its own internal ownership
/// rules, and every pointer it creates it also owns and frees. No value of
/// these argument types can cause UB, so an `unsafe` marker here would
/// communicate nothing actionable to a caller.
pub(super) fn detect_silence(
    path: &Path,
    threshold_db: f32,
    min_duration: Duration,
) -> Result<Vec<SilenceRange>, AnalysisError> {
    // SAFETY: see the safety argument above.
    unsafe { detect_silence_unsafe(path, threshold_db, min_duration) }
}

/// Detects silent intervals in `path` using the filter graph:
///
/// `amovie=filename=path → silencedetect=n=threshold_db dB:d=min_sec → abuffersink`
///
/// Drains all output frames, reading `lavfi.silence_start` and
/// `lavfi.silence_end` metadata keys from each frame.  Only complete intervals
/// (both start and end seen) are returned; a trailing silence that reaches
/// end-of-file without an explicit end marker is not included.
///
/// # Safety
///
/// All raw pointer operations follow the avfilter ownership rules:
/// - `avfilter_graph_alloc()` returns an owned pointer freed via
///   `avfilter_graph_free()` on error or after draining.
/// - `avfilter_graph_create_filter()` adds contexts owned by the graph.
/// - `avfilter_link()` connects pads owned by the graph.
/// - `avfilter_graph_config()` finalises the graph.
/// - `av_frame_alloc()` / `av_frame_free()` manage per-frame lifetimes.
/// - `(*frame).metadata` is a valid `AVDictionary*` (may be null); `av_dict_get`
///   handles null dictionaries by returning null.
unsafe fn detect_silence_unsafe(
    path: &Path,
    threshold_db: f32,
    min_duration: Duration,
) -> Result<Vec<SilenceRange>, AnalysisError> {
    macro_rules! bail {
        ($graph:ident, $reason:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(AnalysisError::Failed {
                reason: format!("{}", $reason),
            });
        }};
    }

    let path_str = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");
    let amovie_args =
        CString::new(format!("filename={path_str}")).map_err(|_| AnalysisError::Failed {
            reason: "path contains null byte".to_string(),
        })?;

    let min_sec = min_duration.as_secs_f64();
    // silencedetect=n={threshold_db}dB:d={min_sec}
    // threshold_db is already negative (e.g. -40.0) → formats as "n=-40dB"
    let silence_args =
        CString::new(format!("n={threshold_db}dB:d={min_sec:.6}")).map_err(|_| {
            AnalysisError::Failed {
                reason: "silencedetect args contained null byte".to_string(),
            }
        })?;

    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(AnalysisError::Failed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // 1. amovie source — reads the audio file directly.
    let amovie_filt = ff_sys::avfilter_get_by_name(c"amovie".as_ptr());
    if amovie_filt.is_null() {
        bail!(graph, "filter not found: amovie");
    }
    let mut src_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut src_ctx,
        amovie_filt,
        c"silence_src".as_ptr(),
        amovie_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("amovie create_filter failed code={ret}"));
    }

    // 2. silencedetect=n=threshold_db dB:d=min_sec — annotates frames with metadata.
    let silence_filt = ff_sys::avfilter_get_by_name(c"silencedetect".as_ptr());
    if silence_filt.is_null() {
        bail!(graph, "filter not found: silencedetect");
    }
    let mut sd_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sd_ctx,
        silence_filt,
        c"silence_detect".as_ptr(),
        silence_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            format!("silencedetect create_filter failed code={ret}")
        );
    }

    // 3. abuffersink — collects output frames.
    let abuffersink_filt = ff_sys::avfilter_get_by_name(c"abuffersink".as_ptr());
    if abuffersink_filt.is_null() {
        bail!(graph, "filter not found: abuffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        abuffersink_filt,
        c"silence_sink".as_ptr(),
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

    // Link: src → silencedetect → sink
    let ret = ff_sys::avfilter_link(src_ctx, 0, sd_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link src→silencedetect failed code={ret}")
        );
    }
    let ret = ff_sys::avfilter_link(sd_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link silencedetect→sink failed code={ret}")
        );
    }

    // Configure the graph.
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // Drain all frames; collect silence_start/end metadata from each frame.
    let mut pending_start: Option<Duration> = None;
    let mut ranges: Vec<SilenceRange> = Vec::new();

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

        // SAFETY: `(*raw_frame).metadata` is a valid `AVDictionary*` (possibly null);
        // `av_dict_get` handles null dictionaries safely by returning null.
        if let Some(secs) = read_f64_meta(raw_frame, c"lavfi.silence_start".as_ptr())
            && secs >= 0.0
        {
            pending_start = Some(Duration::from_secs_f64(secs));
        }
        if let Some(secs) = read_f64_meta(raw_frame, c"lavfi.silence_end".as_ptr())
            && let Some(start) = pending_start.take()
            && secs >= 0.0
        {
            ranges.push(SilenceRange {
                start,
                end: Duration::from_secs_f64(secs),
            });
        }

        let mut ptr = raw_frame;
        ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
    }

    // Trailing silence that reaches EOF without a silence_end marker is dropped.

    let mut g = graph;
    ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));

    log::debug!(
        "silence detection complete ranges={} threshold_db={threshold_db:.1} \
         min_duration={min_sec:.3}s",
        ranges.len()
    );

    Ok(ranges)
}

/// Reads a metadata value from `frame` by `key` and parses it as `f64`.
/// Returns `None` when the key is absent or the value is not parseable.
///
/// # Safety
///
/// - `frame` must point to a valid, fully-initialised `AVFrame`.
/// - `key` must be a null-terminated C string valid for the duration of the call.
unsafe fn read_f64_meta(
    frame: *mut ff_sys::AVFrame,
    key: *const std::os::raw::c_char,
) -> Option<f64> {
    let entry = ff_sys::av_dict_get((*frame).metadata, key, std::ptr::null(), 0);
    if entry.is_null() {
        return None;
    }
    std::ffi::CStr::from_ptr((*entry).value)
        .to_str()
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
}

// BlackFrameDetector inner

/// Safe entry point for [`detect_black_frames_unsafe`].
///
/// # Safety argument (RK-017)
///
/// Takes no raw pointer and imposes no precondition on the caller: the
/// callee's `# Safety` section documents only its own internal ownership
/// rules, and every pointer it creates it also owns and frees. No value of
/// these argument types can cause UB, so an `unsafe` marker here would
/// communicate nothing actionable to a caller.
pub(super) fn detect_black_frames(
    path: &Path,
    threshold: f64,
) -> Result<Vec<Duration>, AnalysisError> {
    // SAFETY: see the safety argument above.
    unsafe { detect_black_frames_unsafe(path, threshold) }
}

/// Detects black intervals in `path` using the filter graph:
///
/// `movie=filename=path → blackdetect=d=0.1:pic_th={threshold} → buffersink`
///
/// Drains all output frames, reading `lavfi.black_start` from each frame's
/// metadata.  Returns one [`Duration`] per detected black interval start.
///
/// # Safety
///
/// All raw pointer operations follow the avfilter ownership rules:
/// - `avfilter_graph_alloc()` returns an owned pointer freed via
///   `avfilter_graph_free()` on error or after draining.
/// - `avfilter_graph_create_filter()` adds contexts owned by the graph.
/// - `avfilter_link()` connects pads owned by the graph.
/// - `avfilter_graph_config()` finalises the graph.
/// - `av_frame_alloc()` / `av_frame_free()` manage per-frame lifetimes.
/// - `(*frame).metadata` is a valid `AVDictionary*` (may be null); `av_dict_get`
///   handles null dictionaries safely by returning null.
unsafe fn detect_black_frames_unsafe(
    path: &Path,
    threshold: f64,
) -> Result<Vec<Duration>, AnalysisError> {
    macro_rules! bail {
        ($graph:ident, $reason:expr) => {{
            let mut g = $graph;
            ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));
            return Err(AnalysisError::Failed {
                reason: format!("{}", $reason),
            });
        }};
    }

    let path_str = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");
    let movie_args =
        CString::new(format!("filename={path_str}")).map_err(|_| AnalysisError::Failed {
            reason: "path contains null byte".to_string(),
        })?;

    // blackdetect=d=0.1:pic_th={threshold}
    //   d     — minimum black-interval duration (0.1 s)
    //   pic_th — fraction of black pixels required per frame (0.0–1.0)
    let blackdetect_args = CString::new(format!("d=0.1:pic_th={threshold:.6}")).map_err(|_| {
        AnalysisError::Failed {
            reason: "blackdetect args contained null byte".to_string(),
        }
    })?;

    let graph = ff_sys::avfilter_graph_alloc();
    if graph.is_null() {
        return Err(AnalysisError::Failed {
            reason: "avfilter_graph_alloc failed".to_string(),
        });
    }

    // 1. movie source — reads the video file directly.
    let movie_filt = ff_sys::avfilter_get_by_name(c"movie".as_ptr());
    if movie_filt.is_null() {
        bail!(graph, "filter not found: movie");
    }
    let mut src_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut src_ctx,
        movie_filt,
        c"black_src".as_ptr(),
        movie_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("movie create_filter failed code={ret}"));
    }

    // 2. blackdetect — annotates frames with lavfi.black_start / lavfi.black_end.
    let blackdetect_filt = ff_sys::avfilter_get_by_name(c"blackdetect".as_ptr());
    if blackdetect_filt.is_null() {
        bail!(graph, "filter not found: blackdetect");
    }
    let mut bd_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut bd_ctx,
        blackdetect_filt,
        c"black_detect".as_ptr(),
        blackdetect_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(
            graph,
            format!("blackdetect create_filter failed code={ret}")
        );
    }

    // 3. buffersink — collects output frames.
    let buffersink_filt = ff_sys::avfilter_get_by_name(c"buffersink".as_ptr());
    if buffersink_filt.is_null() {
        bail!(graph, "filter not found: buffersink");
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        buffersink_filt,
        c"black_sink".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        bail!(graph, format!("buffersink create_filter failed code={ret}"));
    }

    // Link: src → blackdetect → sink
    let ret = ff_sys::avfilter_link(src_ctx, 0, bd_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link src→blackdetect failed code={ret}")
        );
    }
    let ret = ff_sys::avfilter_link(bd_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        bail!(
            graph,
            format!("avfilter_link blackdetect→sink failed code={ret}")
        );
    }

    // Configure the graph.
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        bail!(graph, format!("avfilter_graph_config failed code={ret}"));
    }

    // Drain all frames; collect lavfi.black_start timestamps.
    let mut timestamps: Vec<Duration> = Vec::new();

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

        // SAFETY: `(*raw_frame).metadata` is a valid `AVDictionary*` (possibly null);
        // `av_dict_get` handles null dictionaries safely by returning null.
        if let Some(secs) = read_f64_meta(raw_frame, c"lavfi.black_start".as_ptr())
            && secs >= 0.0
        {
            timestamps.push(Duration::from_secs_f64(secs));
        }

        let mut ptr = raw_frame;
        ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
    }

    let mut g = graph;
    ff_sys::avfilter_graph_free(std::ptr::addr_of_mut!(g));

    log::debug!(
        "black frame detection complete intervals={} threshold={threshold:.4}",
        timestamps.len()
    );

    Ok(timestamps)
}

// KeyframeEnumerator inner

/// Safe entry point for [`enumerate_keyframes_unsafe`].
///
/// # Safety argument (RK-017)
///
/// Takes no raw pointer and imposes no precondition on the caller: the
/// callee's `# Safety` section documents only its own internal ownership
/// rules, and every pointer it creates it also owns and frees. No value of
/// these argument types can cause UB, so an `unsafe` marker here would
/// communicate nothing actionable to a caller.
pub(super) fn enumerate_keyframes(
    path: &Path,
    stream_index: Option<usize>,
) -> Result<Vec<Duration>, AnalysisError> {
    // SAFETY: see the safety argument above.
    unsafe { enumerate_keyframes_unsafe(path, stream_index) }
}

/// Enumerates all keyframe PTS values for the given stream in `path`.
///
/// Processing flow:
/// 1. Open the input as an owned [`ff_sys::InputFormatContext`] + `find_stream_info`
/// 2. Locate the target stream (first video stream when `stream_index` is `None`)
/// 3. `av_read_frame` loop — no decoder is opened
/// 4. For packets on the target stream with `AV_PKT_FLAG_KEY` set, convert PTS
///    to [`Duration`] using the stream's time base
/// 5. `av_packet_unref` after each packet; `av_packet_free` on exit (the owned
///    `InputFormatContext` closes its input and frees the context on drop)
///
/// # Safety
///
/// All raw pointer operations follow avformat ownership rules:
/// - The owned [`ff_sys::InputFormatContext`] closes its input and frees the
///   format context exactly once on drop, on every exit path.
/// - `av_packet_alloc` returns an owned `AVPacket*` freed via `av_packet_free`.
/// - `av_packet_unref` is called after every successful `av_read_frame`.
/// - Stream pointer access (via `as_ptr` / `as_mut_ptr`) is guarded by bounds
///   checks on `nb_streams`.
unsafe fn enumerate_keyframes_unsafe(
    path: &Path,
    stream_index: Option<usize>,
) -> Result<Vec<Duration>, AnalysisError> {
    // AV_PKT_FLAG_KEY = 1 (from FFmpeg's avcodec.h; not exported by ff_sys constants).
    const AV_PKT_FLAG_KEY: i32 = 1;

    // `bail!` returns an `AnalysisError::Failed` with the given reason. The owned
    // `format_ctx` frees itself on drop, so no manual close is needed.
    macro_rules! bail {
        ($reason:expr) => {{
            return Err(AnalysisError::Failed {
                reason: format!("{}", $reason),
            });
        }};
    }

    // 1. Open input (owned; frees itself on drop, so every early return is leak-free).
    let mut format_ctx =
        ff_sys::InputFormatContext::open(path).map_err(|e| AnalysisError::Failed {
            reason: format!("avformat_open_input failed code={}", e.code()),
        })?;

    // 2. Find stream info.
    format_ctx
        .find_stream_info()
        .map_err(|e| AnalysisError::Failed {
            reason: format!("avformat_find_stream_info failed code={}", e.code()),
        })?;

    // 3. Locate the target stream.
    let nb_streams = format_ctx.nb_streams() as usize;
    let target_stream: usize = if let Some(idx) = stream_index {
        if idx >= nb_streams {
            bail!(format!(
                "stream_index {idx} out of range (file has {nb_streams} streams)"
            ));
        }
        idx
    } else {
        // Select the first video stream.
        let mut found: Option<usize> = None;
        for i in 0..nb_streams {
            let Some(stream) = format_ctx.stream(i) else {
                continue;
            };
            if stream.codecpar().codec_type() == ff_sys::AVMediaType_AVMEDIA_TYPE_VIDEO {
                found = Some(i);
                break;
            }
        }
        if let Some(i) = found {
            i
        } else {
            bail!("no video stream found in file")
        }
    };

    // 4. Read the stream's time base for PTS → Duration conversion.
    let time_base = format_ctx
        .stream(target_stream)
        .map_or(ff_sys::AVRational { num: 0, den: 1 }, |s| s.time_base());
    let tb_num = f64::from(time_base.num);
    let tb_den = f64::from(time_base.den);

    // 5. Allocate a reusable packet.
    let mut pkt = ff_sys::Packet::new().map_err(|e| AnalysisError::Failed {
        reason: format!("av_packet_alloc failed code={}", e.code()),
    })?;

    // 6. Read all packets; record the PTS of every keyframe on the target stream.
    // Stream indices in AVPacket are i32; the cast is bounded by nb_streams which
    // is u32, so values fit in i32 on all supported platforms.
    #[allow(clippy::cast_possible_wrap)]
    let target_i32 = target_stream as i32;
    let mut timestamps: Vec<Duration> = Vec::new();

    loop {
        // `read_frame` returns Err on EOF or a real error; either ends the loop.
        if format_ctx.read_frame(&mut pkt).is_err() {
            break;
        }

        if pkt.stream_index() == target_i32 && pkt.flags() & AV_PKT_FLAG_KEY != 0 {
            // Prefer PTS; fall back to DTS for streams that only write DTS on packets.
            let pts = if pkt.pts() == ff_sys::AV_NOPTS_VALUE {
                pkt.dts()
            } else {
                pkt.pts()
            };
            if pts != ff_sys::AV_NOPTS_VALUE && tb_den > 0.0 {
                let secs = pts as f64 * tb_num / tb_den;
                if secs >= 0.0 {
                    timestamps.push(Duration::from_secs_f64(secs));
                }
            }
        }

        pkt.unref();
    }

    // The owned `pkt` frees itself on drop; the owned `format_ctx` closes its
    // input and frees itself when it drops at the end of this function.

    log::debug!(
        "keyframe enumeration complete keyframes={} stream_index={target_stream}",
        timestamps.len()
    );

    Ok(timestamps)
}
