//! Graph-building helpers: hardware acceleration and filter linking.

use super::{
    AUDIO_TIME_BASE_NUM, BuildResult, FilterCtxVec, VIDEO_TIME_BASE_DEN, VIDEO_TIME_BASE_NUM,
    VideoGraphResult, ffmpeg_err,
};
use std::ptr::NonNull;
use std::time::Duration;

use crate::blend::BlendMode;
use crate::error::FilterError;
use crate::graph::filter_step::FilterStep;
use crate::graph::types::{EqBand, HwAccel};
use ff_format::AlphaMode;

// ── Hardware acceleration helpers ─────────────────────────────────────────────

/// Map a [`HwAccel`] variant to the corresponding `AVHWDeviceType` constant.
pub(super) fn hw_accel_to_device_type(hw: HwAccel) -> ff_sys::AVHWDeviceType {
    match hw {
        HwAccel::Cuda => ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_CUDA,
        HwAccel::VideoToolbox => ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
        HwAccel::Vaapi => ff_sys::AVHWDeviceType_AV_HWDEVICE_TYPE_VAAPI,
    }
}

/// Create and link a named hardware filter (e.g., `hwupload_cuda`, `hwdownload`)
/// with no arguments.  Sets the filter context's `hw_device_ctx` to a new
/// reference obtained via `av_buffer_ref(hw_ctx)` so the filter owns its own ref.
///
/// # Safety
///
/// `graph`, `prev_ctx`, and `hw_ctx` must be valid non-null pointers.
/// `hw_ctx` must be a valid `AVBufferRef` wrapping an `AVHWDeviceContext`.
pub(super) unsafe fn create_hw_filter(
    graph: *mut ff_sys::AVFilterGraph,
    prev_ctx: *mut ff_sys::AVFilterContext,
    filter_name: &std::ffi::CStr,
    instance_name: &std::ffi::CStr,
    hw_ctx: *mut ff_sys::AVBufferRef,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    // SAFETY: `avfilter_get_by_name` reads a valid null-terminated C string and
    // returns a borrowed, process-lifetime pointer (or null if not found).
    let filter = ff_sys::avfilter_get_by_name(filter_name.as_ptr());
    if filter.is_null() {
        log::warn!(
            "hw filter not found name={}",
            filter_name.to_str().unwrap_or("?")
        );
        return Err(FilterError::BuildFailed);
    }

    let mut ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut ctx,
        filter,
        instance_name.as_ptr(),
        std::ptr::null(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!(
            "hw filter creation failed name={} code={ret}",
            filter_name.to_str().unwrap_or("?")
        );
        return Err(FilterError::BuildFailed);
    }
    log::debug!(
        "hw filter added name={}",
        filter_name.to_str().unwrap_or("?")
    );

    // Give this filter context its own reference to the hardware device context.
    // SAFETY: `hw_ctx` is a valid `AVBufferRef`; `av_buffer_ref` returns a new
    // reference counted ref, or null on allocation failure.
    let filter_hw_ref = ff_sys::av_buffer_ref(hw_ctx);
    if filter_hw_ref.is_null() {
        log::warn!("av_buffer_ref failed for hw device context");
        return Err(FilterError::BuildFailed);
    }
    (*ctx).hw_device_ctx = filter_hw_ref;

    // SAFETY: `prev_ctx` and `ctx` belong to the same graph; pad indices are valid.
    let ret = ff_sys::avfilter_link(prev_ctx, 0, ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    Ok(ctx)
}

// ── Shared graph-building helper ──────────────────────────────────────────────

/// Create an `AVFilterContext` for `step`, link it after `prev_ctx`, and return
/// the new context pointer.
///
/// # Safety
///
/// `graph` and `prev_ctx` must be valid pointers owned by the same
/// `AVFilterGraph`.
pub(crate) unsafe fn add_and_link_step(
    graph: *mut ff_sys::AVFilterGraph,
    prev_ctx: *mut ff_sys::AVFilterContext,
    step: &FilterStep,
    index: usize,
    prefix: &str,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    let filter_name =
        std::ffi::CString::new(step.filter_name()).map_err(|_| FilterError::BuildFailed)?;
    let filter = ff_sys::avfilter_get_by_name(filter_name.as_ptr());
    if filter.is_null() {
        log::warn!("filter not found name={}", step.filter_name());
        return Err(FilterError::BuildFailed);
    }

    let step_name =
        std::ffi::CString::new(format!("{prefix}{index}")).map_err(|_| FilterError::BuildFailed)?;
    let step_args_str = step.args();
    let step_args =
        std::ffi::CString::new(step_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;

    let mut step_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut step_ctx,
        filter,
        step_name.as_ptr(),
        step_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!(
            "filter creation failed name={} args={}",
            step.filter_name(),
            step.args()
        );
        return Err(FilterError::BuildFailed);
    }
    log::debug!(
        "filter added name={} args={}",
        step.filter_name(),
        step.args()
    );

    let ret = ff_sys::avfilter_link(prev_ctx, 0, step_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    Ok(step_ctx)
}

/// Create and link a filter by explicit name and args strings, without a
/// `FilterStep`.  Used when the filter name cannot be determined statically
/// (e.g., `AudioDelay` dispatches to `adelay` or `atrim` depending on sign).
///
/// # Safety
///
/// `graph` and `prev_ctx` must be valid pointers owned by the same
/// `AVFilterGraph`.
pub(super) unsafe fn add_raw_filter_step(
    graph: *mut ff_sys::AVFilterGraph,
    prev_ctx: *mut ff_sys::AVFilterContext,
    filter_name: &str,
    args: &str,
    index: usize,
    prefix: &str,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    let filter_cname = std::ffi::CString::new(filter_name).map_err(|_| FilterError::BuildFailed)?;
    // SAFETY: `avfilter_get_by_name` reads a valid null-terminated C string.
    let filter = ff_sys::avfilter_get_by_name(filter_cname.as_ptr());
    if filter.is_null() {
        log::warn!("filter not found name={filter_name}");
        return Err(FilterError::BuildFailed);
    }

    let step_name =
        std::ffi::CString::new(format!("{prefix}{index}")).map_err(|_| FilterError::BuildFailed)?;
    let step_args = std::ffi::CString::new(args).map_err(|_| FilterError::BuildFailed)?;

    let mut step_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut step_ctx,
        filter,
        step_name.as_ptr(),
        step_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name={filter_name} args={args}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name={filter_name} args={args}");

    // SAFETY: `prev_ctx` and `step_ctx` belong to the same graph; pad indices are valid.
    let ret = ff_sys::avfilter_link(prev_ctx, 0, step_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    Ok(step_ctx)
}

/// Insert a `setpts=PTS-STARTPTS` filter immediately after a `trim` step so
/// that the output stream's timestamps start at zero.
///
/// # Safety
///
/// `graph` and `prev_ctx` must be valid pointers owned by the same
/// `AVFilterGraph`.
pub(super) unsafe fn add_setpts_after_trim(
    graph: *mut ff_sys::AVFilterGraph,
    prev_ctx: *mut ff_sys::AVFilterContext,
    trim_index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    let setpts = ff_sys::avfilter_get_by_name(c"setpts".as_ptr());
    if setpts.is_null() {
        log::warn!("filter not found name=setpts");
        return Err(FilterError::BuildFailed);
    }

    let name = std::ffi::CString::new(format!("setpts{trim_index}"))
        .map_err(|_| FilterError::BuildFailed)?;

    let mut ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut ctx,
        setpts,
        name.as_ptr(),
        c"PTS-STARTPTS".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=setpts args=PTS-STARTPTS");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=setpts args=PTS-STARTPTS");

    let ret = ff_sys::avfilter_link(prev_ctx, 0, ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    Ok(ctx)
}

/// Add a `pad` filter that centres the scaled frame on the target `width × height`
/// canvas, completing the scale-then-pad compound step for [`FilterStep::FitToAspect`].
///
/// # Safety
///
/// `graph` and `prev_ctx` must be valid pointers owned by the same
/// `AVFilterGraph`.
pub(super) unsafe fn add_fit_to_aspect_pad(
    graph: *mut ff_sys::AVFilterGraph,
    prev_ctx: *mut ff_sys::AVFilterContext,
    width: u32,
    height: u32,
    color: &str,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    let pad_filter = ff_sys::avfilter_get_by_name(c"pad".as_ptr());
    if pad_filter.is_null() {
        log::warn!("filter not found name=pad (fit_to_aspect)");
        return Err(FilterError::BuildFailed);
    }

    let name =
        std::ffi::CString::new(format!("fitpad{index}")).map_err(|_| FilterError::BuildFailed)?;
    let args_str = format!("width={width}:height={height}:x=(ow-iw)/2:y=(oh-ih)/2:color={color}");
    let args = std::ffi::CString::new(args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;

    let mut ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut ctx,
        pad_filter,
        name.as_ptr(),
        args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=pad args={args_str}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=pad args={args_str} index={index}");

    let ret = ff_sys::avfilter_link(prev_ctx, 0, ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    Ok(ctx)
}

// ── Speed (atempo chain) ──────────────────────────────────────────────────────

/// Decompose a speed `factor` into a chain of `atempo` values, each in [0.5, 100.0].
///
/// `FFmpeg` 4.0+ (`YAE_ATEMPO_MAX = 100.0`) accepts up to 100.0 per instance.
/// Chaining multiple instances multiplies the effective speed factor:
///
/// - `factor = 4.0`   → `[4.0]`         (single instance)
/// - `factor = 0.25`  → `[0.5, 0.5]`   (0.5 × 0.5 = 0.25)
/// - `factor = 150.0` → `[100.0, 1.5]` (2 instances instead of 8 at 2.0-max)
/// - `factor = 1.5`   → `[1.5]`        (within range directly)
pub(super) fn decompose_atempo(factor: f64) -> Vec<f64> {
    let mut remaining = factor;
    let mut chain = Vec::new();
    while remaining > 100.0 {
        chain.push(100.0);
        remaining /= 100.0;
    }
    while remaining < 0.5 {
        chain.push(0.5);
        remaining /= 0.5;
    }
    chain.push(remaining);
    chain
}

/// Insert a chain of `atempo` filters for the audio path of a `Speed` step.
///
/// Creates as many `atempo` instances as needed (see [`decompose_atempo`]) and
/// links them in series after `prev_ctx`.
///
/// # Safety
///
/// `graph` and `prev_ctx` must be valid pointers owned by the same
/// `AVFilterGraph`.
pub(crate) unsafe fn add_atempo_chain(
    graph: *mut ff_sys::AVFilterGraph,
    prev_ctx: *mut ff_sys::AVFilterContext,
    factor: f64,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    let atempo_filter = ff_sys::avfilter_get_by_name(c"atempo".as_ptr());
    if atempo_filter.is_null() {
        log::warn!("filter not found name=atempo (speed)");
        return Err(FilterError::BuildFailed);
    }

    let chain = decompose_atempo(factor);
    let mut ctx = prev_ctx;
    for (j, &val) in chain.iter().enumerate() {
        let name = std::ffi::CString::new(format!("atempo{index}_{j}"))
            .map_err(|_| FilterError::BuildFailed)?;
        let args_str = format!("{val}");
        let args =
            std::ffi::CString::new(args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
        let mut atempo_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut atempo_ctx,
            atempo_filter,
            name.as_ptr(),
            args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            log::warn!("filter creation failed name=atempo args={val}");
            return Err(FilterError::BuildFailed);
        }
        log::debug!("filter added name=atempo args={val} index={index}_{j}");

        let ret = ff_sys::avfilter_link(ctx, 0, atempo_ctx, 0);
        if ret < 0 {
            return Err(FilterError::BuildFailed);
        }
        ctx = atempo_ctx;
    }
    Ok(ctx)
}

/// Insert a speed-change filter chain for `factor > 2.0`:
///
/// ```text
/// apad=pad_dur=1  →  asetrate=r={new_sr}  →  aresample={output_sr}  →  aeval (NaN clamp)
/// ```
///
/// * `apad` appends 1 s of silence so SWR's polyphase FIR resampler has enough
///   input at EOF to flush its filter window without reading uninitialised memory
///   (which produces NaN → AAC encoder EINVAL at high downsampling ratios).
/// * `aeval` replaces any residual NaN/Inf samples with 0 as a last-resort guard.
///   The expression is generated for `nb_channels` channels so it matches the
///   actual stream layout (the per-track aformat has already normalised the layout
///   before this chain is appended).
/// * Both `apad` and `aeval` are optional: if their filter is unavailable the
///   chain proceeds with just `asetrate → aresample`.
///
/// Audio pitch changes proportionally ("vinyl effect"); no WSOLA processing.
///
/// # Safety
///
/// `graph` and `prev_ctx` must be valid pointers owned by the same
/// `AVFilterGraph`.
pub(crate) unsafe fn add_asetrate_resample_chain(
    graph: *mut ff_sys::AVFilterGraph,
    prev_ctx: *mut ff_sys::AVFilterContext,
    new_sr: u32,
    output_sr: u32,
    nb_channels: u32,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    let mut ctx = prev_ctx;

    // ── 1. apad=pad_dur=1 (optional) ────────────────────────────────────────
    // Prevents SWR polyphase resampler from reading uninitialised memory at EOF
    // when the last output frame cannot be filled from remaining input samples.
    // At speed=150 the 1 s of appended silence shrinks to ~7 ms — imperceptible.
    let apad_filter = ff_sys::avfilter_get_by_name(c"apad".as_ptr());
    if apad_filter.is_null() {
        log::warn!("apad filter unavailable — SWR tail-flush NaN guard skipped index={index}");
    } else {
        let name = std::ffi::CString::new(format!("apad_spd{index}"))
            .map_err(|_| FilterError::BuildFailed)?;
        let args = c"pad_dur=1";
        let mut apad_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut apad_ctx,
            apad_filter,
            name.as_ptr(),
            args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 || apad_ctx.is_null() {
            log::warn!(
                "apad creation FAILED ret={ret} index={index} — continuing without tail padding"
            );
        } else {
            let ret = ff_sys::avfilter_link(ctx, 0, apad_ctx, 0);
            if ret < 0 {
                log::warn!("apad link FAILED ret={ret} index={index}");
            } else {
                ctx = apad_ctx;
            }
        }
    }

    // ── 2. asetrate=r={new_sr} (required) ───────────────────────────────────
    let asetrate_filter = ff_sys::avfilter_get_by_name(c"asetrate".as_ptr());
    if asetrate_filter.is_null() {
        log::warn!("filter not found name=asetrate (speed fallback)");
        return Err(FilterError::BuildFailed);
    }
    let name = std::ffi::CString::new(format!("asetrate_spd{index}"))
        .map_err(|_| FilterError::BuildFailed)?;
    let args =
        std::ffi::CString::new(format!("r={new_sr}")).map_err(|_| FilterError::BuildFailed)?;
    let mut asetrate_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut asetrate_ctx,
        asetrate_filter,
        name.as_ptr(),
        args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=asetrate args=r={new_sr}");
        return Err(FilterError::BuildFailed);
    }
    let ret = ff_sys::avfilter_link(ctx, 0, asetrate_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    ctx = asetrate_ctx;

    // ── 3. aresample={output_sr} (required) ────────────────────────────────
    // aresample accepts the rate as a positional arg (just the number).
    // "r=" is not a valid option name in this FFmpeg version.
    let aresample_filter = ff_sys::avfilter_get_by_name(c"aresample".as_ptr());
    if aresample_filter.is_null() {
        log::warn!("filter not found name=aresample (speed fallback)");
        return Err(FilterError::BuildFailed);
    }
    let name2 = std::ffi::CString::new(format!("aresample_spd{index}"))
        .map_err(|_| FilterError::BuildFailed)?;
    let args2 =
        std::ffi::CString::new(format!("{output_sr}")).map_err(|_| FilterError::BuildFailed)?;
    let mut aresample_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut aresample_ctx,
        aresample_filter,
        name2.as_ptr(),
        args2.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=aresample args={output_sr} code={ret}");
        return Err(FilterError::BuildFailed);
    }
    let ret = ff_sys::avfilter_link(ctx, 0, aresample_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    ctx = aresample_ctx;

    // ── 4. aeval NaN/Inf sanitizer (optional) ───────────────────────────────
    // Replaces any NaN or Inf samples that survive the resampler with 0.
    // One expression per channel, joined by `|`, so the output layout matches
    // the input layout exactly regardless of mono/stereo/surround.
    let aeval_filter = ff_sys::avfilter_get_by_name(c"aeval".as_ptr());
    if aeval_filter.is_null() {
        log::warn!("aeval filter unavailable — NaN sanitizer skipped index={index}");
    } else {
        let name3 = std::ffi::CString::new(format!("aeval_spd{index}"))
            .map_err(|_| FilterError::BuildFailed)?;
        let per_ch = |ch: u32| format!("if(isnan(val({ch}))+isinf(val({ch})),0,val({ch}))");
        let exprs_str = (0..nb_channels.max(1))
            .map(per_ch)
            .collect::<Vec<_>>()
            .join("|");
        let args3 = std::ffi::CString::new(format!("exprs={exprs_str}"))
            .map_err(|_| FilterError::BuildFailed)?;
        let mut aeval_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut aeval_ctx,
            aeval_filter,
            name3.as_ptr(),
            args3.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 || aeval_ctx.is_null() {
            log::warn!("aeval creation FAILED ret={ret} index={index} — NaN may reach encoder");
        } else {
            let ret = ff_sys::avfilter_link(ctx, 0, aeval_ctx, 0);
            if ret < 0 {
                log::warn!("aeval link FAILED ret={ret} index={index}");
            } else {
                ctx = aeval_ctx;
            }
        }
    }

    log::debug!("speed fallback asetrate={new_sr} aresample={output_sr} index={index}");
    Ok(ctx)
}

#[cfg(test)]
mod tests {
    use super::decompose_atempo;
    use super::overlay_alpha_suffix;
    use ff_format::AlphaMode;

    #[test]
    fn overlay_alpha_suffix_should_append_only_for_premultiplied() {
        assert_eq!(
            overlay_alpha_suffix(AlphaMode::Premultiplied),
            ":alpha=premultiplied"
        );
        assert_eq!(overlay_alpha_suffix(AlphaMode::Straight), "");
        assert_eq!(overlay_alpha_suffix(AlphaMode::Unknown), "");
    }

    #[test]
    fn decompose_atempo_should_return_single_value_for_factor_within_range() {
        assert_eq!(decompose_atempo(1.5), vec![1.5]);
    }

    #[test]
    fn decompose_atempo_should_return_single_value_for_factor_1() {
        assert_eq!(decompose_atempo(1.0), vec![1.0]);
    }

    #[test]
    fn decompose_atempo_should_use_single_instance_for_factor_4() {
        assert_eq!(decompose_atempo(4.0), vec![4.0]);
    }

    #[test]
    fn decompose_atempo_should_use_single_instance_for_factor_8() {
        assert_eq!(decompose_atempo(8.0), vec![8.0]);
    }

    #[test]
    fn decompose_atempo_should_chain_two_instances_for_factor_150() {
        let chain = decompose_atempo(150.0);
        assert_eq!(chain.len(), 2);
        let product: f64 = chain.iter().product();
        assert!(
            (product - 150.0).abs() < 1e-6,
            "product should be ~150, got {product}, chain={chain:?}"
        );
    }

    #[test]
    fn decompose_atempo_should_chain_two_instances_for_factor_0_25() {
        let chain = decompose_atempo(0.25);
        let product: f64 = chain.iter().product();
        assert!(
            (product - 0.25).abs() < 1e-9,
            "product should be ~0.25, got {product}, chain={chain:?}"
        );
        for &v in &chain {
            assert!(
                (0.5..=100.0).contains(&v),
                "each value must be in [0.5, 100.0], got {v}"
            );
        }
    }

    #[test]
    fn decompose_atempo_should_produce_values_all_within_valid_range() {
        for factor in [0.1, 0.25, 0.5, 1.0, 1.5, 2.0, 4.0, 8.0, 16.0, 100.0, 150.0] {
            let chain = decompose_atempo(factor);
            assert!(
                !chain.is_empty(),
                "chain must not be empty for factor={factor}"
            );
            let product: f64 = chain.iter().product();
            assert!(
                (product - factor).abs() < 1e-6,
                "product {product} must equal factor {factor}, chain={chain:?}"
            );
            for &v in &chain {
                assert!(
                    (0.5..=100.0).contains(&v),
                    "each value must be in [0.5, 100.0], got {v} for factor={factor}"
                );
            }
        }
    }
}

// ── Overlay image compound step ───────────────────────────────────────────────

/// Insert the compound `movie → lut → overlay` filter chain for an
/// [`FilterStep::OverlayImage`] step.
///
/// Unlike standard steps (which go through [`add_and_link_step`]), this step
/// creates three filter contexts internally:
///
/// 1. `movie` — loads the PNG from `path` as a self-contained video source
///    (no buffersrc input slot is consumed).
/// 2. `lut` — scales the alpha channel by `opacity` (`a = val * opacity`).
/// 3. `overlay` — composites the main stream (pad 0) with the image (pad 1)
///    at position `(x, y)`.
///
/// # Safety
///
/// `graph` and `prev_ctx` must be valid pointers owned by the same
/// `AVFilterGraph`.
pub(super) unsafe fn add_overlay_image_step(
    graph: *mut ff_sys::AVFilterGraph,
    prev_ctx: *mut ff_sys::AVFilterContext,
    path: &str,
    x: &str,
    y: &str,
    opacity: f32,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    use std::ffi::CString;

    // 1. movie filter — self-contained PNG source, no buffersrc slot needed.
    let movie_filter = ff_sys::avfilter_get_by_name(c"movie".as_ptr());
    if movie_filter.is_null() {
        log::warn!("filter not found name=movie (overlay_image)");
        return Err(FilterError::BuildFailed);
    }
    let movie_name = CString::new(format!("movie{index}")).map_err(|_| FilterError::BuildFailed)?;
    let movie_args =
        CString::new(format!("filename={path}")).map_err(|_| FilterError::BuildFailed)?;
    let mut movie_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut movie_ctx,
        movie_filter,
        movie_name.as_ptr(),
        movie_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=movie args=filename={path}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=movie args=filename={path} index={index}");

    // 2. lut filter — scale the alpha channel: a = val * opacity.
    //    For 8-bit RGBA the `lut` filter operates per-channel; `val` is the
    //    current pixel value (0–255) and the expression is evaluated per sample.
    let lut_filter = ff_sys::avfilter_get_by_name(c"lut".as_ptr());
    if lut_filter.is_null() {
        log::warn!("filter not found name=lut (overlay_image)");
        return Err(FilterError::BuildFailed);
    }
    let lut_name = CString::new(format!("lut{index}")).map_err(|_| FilterError::BuildFailed)?;
    let lut_args_str = format!("a=val*{opacity}");
    let lut_args = CString::new(lut_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut lut_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut lut_ctx,
        lut_filter,
        lut_name.as_ptr(),
        lut_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=lut args={lut_args_str}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=lut args={lut_args_str} index={index}");

    // Link: movie → lut (alpha scaling).
    let ret = ff_sys::avfilter_link(movie_ctx, 0, lut_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 3. overlay filter — composite main stream (pad 0) with image (pad 1).
    let overlay_filter = ff_sys::avfilter_get_by_name(c"overlay".as_ptr());
    if overlay_filter.is_null() {
        log::warn!("filter not found name=overlay (overlay_image)");
        return Err(FilterError::BuildFailed);
    }
    let overlay_name =
        CString::new(format!("step{index}")).map_err(|_| FilterError::BuildFailed)?;
    let overlay_args_str = format!("{x}:{y}");
    let overlay_args =
        CString::new(overlay_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut overlay_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut overlay_ctx,
        overlay_filter,
        overlay_name.as_ptr(),
        overlay_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=overlay args={overlay_args_str}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=overlay args={overlay_args_str} index={index}");

    // Link: prev_ctx → overlay pad 0 (main video stream).
    let ret = ff_sys::avfilter_link(prev_ctx, 0, overlay_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // Link: lut → overlay pad 1 (image stream).
    let ret = ff_sys::avfilter_link(lut_ctx, 0, overlay_ctx, 1);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    Ok(overlay_ctx)
}

// ── Blend Normal mode compound step ──────────────────────────────────────────

/// Insert a Normal-mode blend compound step.
///
/// The top layer's `top_steps` are first applied to `top_src_ctx` (the `in1`
/// buffersrc), then optionally followed by `colorchannelmixer=aa=<opacity>` when
/// `opacity < 1.0`.  The result is composited onto `bottom_ctx` using
/// `overlay=format=auto:shortest=1`.
///
/// ```text
/// [in1]top_steps...[top_processed]
/// [top_processed]colorchannelmixer=aa=<opacity>[top_faded]   ← when opacity < 1.0
/// [bottom_ctx][top_faded]overlay=format=auto:shortest=1[out]
/// ```
///
/// # Safety
///
/// `graph`, `bottom_ctx`, and `top_src_ctx` must be valid pointers owned by the
/// same `AVFilterGraph`.
/// Overlay `:alpha=` suffix for `alpha`. `Straight` is the `overlay` default (no extra arg);
/// `Premultiplied` selects premultiplied alpha. Verified `vf_overlay.c` (7.1/8.0).
fn overlay_alpha_suffix(alpha: AlphaMode) -> &'static str {
    match alpha {
        AlphaMode::Premultiplied => ":alpha=premultiplied",
        _ => "",
    }
}

pub(super) unsafe fn add_blend_normal_step(
    graph: *mut ff_sys::AVFilterGraph,
    bottom_ctx: *mut ff_sys::AVFilterContext,
    top_src_ctx: *mut ff_sys::AVFilterContext,
    top_steps: &[FilterStep],
    opacity: f32,
    alpha: AlphaMode,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    use std::ffi::CString;

    // 1. Chain the top builder's steps starting from the in1 buffersrc.
    let mut top_ctx = top_src_ctx;
    for (j, step) in top_steps.iter().enumerate() {
        // Skip audio-only steps; they have no place in the video blend chain.
        if matches!(
            step,
            FilterStep::AReverse
                | FilterStep::AFadeIn { .. }
                | FilterStep::AFadeOut { .. }
                | FilterStep::ParametricEq { .. }
                | FilterStep::ANoiseGate { .. }
                | FilterStep::ACompressor { .. }
                | FilterStep::StereoToMono
                | FilterStep::ChannelMap { .. }
                | FilterStep::AudioDelay { .. }
                | FilterStep::ConcatAudio { .. }
                | FilterStep::LoudnessNormalize { .. }
                | FilterStep::NormalizePeak { .. }
        ) {
            continue;
        }
        top_ctx = add_and_link_step(graph, top_ctx, step, index * 1000 + j, "blend_top")?;
    }

    // 2. When opacity < 1.0, attenuate the top layer's alpha channel.
    if opacity < 1.0 {
        let ccm_name =
            CString::new(format!("blend_ccm{index}")).map_err(|_| FilterError::BuildFailed)?;
        let ccm_args_str = format!("aa={opacity}");
        let ccm_args = CString::new(ccm_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;

        let ccm_filter = ff_sys::avfilter_get_by_name(c"colorchannelmixer".as_ptr());
        if ccm_filter.is_null() {
            log::warn!("filter not found name=colorchannelmixer (blend_normal)");
            return Err(FilterError::BuildFailed);
        }

        let mut ccm_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut ccm_ctx,
            ccm_filter,
            ccm_name.as_ptr(),
            ccm_args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            log::warn!("filter creation failed name=colorchannelmixer args={ccm_args_str}");
            return Err(FilterError::BuildFailed);
        }
        log::debug!("filter added name=colorchannelmixer args={ccm_args_str} index={index}");

        // SAFETY: top_ctx and ccm_ctx belong to the same graph; pad indices valid.
        let ret = ff_sys::avfilter_link(top_ctx, 0, ccm_ctx, 0);
        if ret < 0 {
            return Err(FilterError::BuildFailed);
        }
        top_ctx = ccm_ctx;
    }

    // 3. Create the overlay filter.
    let overlay_filter = ff_sys::avfilter_get_by_name(c"overlay".as_ptr());
    if overlay_filter.is_null() {
        log::warn!("filter not found name=overlay (blend_normal)");
        return Err(FilterError::BuildFailed);
    }
    let overlay_name =
        CString::new(format!("blend_overlay{index}")).map_err(|_| FilterError::BuildFailed)?;
    let overlay_args_str = format!("format=auto:shortest=1{}", overlay_alpha_suffix(alpha));
    let overlay_args =
        CString::new(overlay_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut overlay_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut overlay_ctx,
        overlay_filter,
        overlay_name.as_ptr(),
        overlay_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=overlay args={overlay_args_str} (blend_normal)");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=overlay args={overlay_args_str} index={index} (blend_normal)");

    // 4. Link: bottom → overlay[0], top → overlay[1].
    // SAFETY: bottom_ctx, top_ctx, overlay_ctx are all in the same graph.
    let ret = ff_sys::avfilter_link(bottom_ctx, 0, overlay_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    let ret = ff_sys::avfilter_link(top_ctx, 0, overlay_ctx, 1);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    log::debug!("filter blend_normal expanded opacity={opacity} index={index}");
    Ok(overlay_ctx)
}

// ── Blend photographic-mode compound step ────────────────────────────────────

/// Insert a photographic-mode blend compound step (Multiply, Screen, etc.).
///
/// Chains `top_steps` on `top_src_ctx`, then creates
/// `blend=all_mode=<mode_name>[:all_opacity=<opacity>]` and links both inputs.
///
/// ```text
/// [in1]top_steps...[top_processed]
/// [bottom_ctx][top_processed]blend=all_mode=<mode>[out]
/// ```
///
/// # Safety
///
/// `graph`, `bottom_ctx`, and `top_src_ctx` must be valid pointers owned by the
/// same `AVFilterGraph`.
pub(super) unsafe fn add_blend_photographic_step(
    graph: *mut ff_sys::AVFilterGraph,
    bottom_ctx: *mut ff_sys::AVFilterContext,
    top_src_ctx: *mut ff_sys::AVFilterContext,
    top_steps: &[FilterStep],
    mode_name: &str,
    opacity: f32,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    use std::ffi::CString;

    // 1. Chain the top builder's steps starting from the in1 buffersrc.
    let mut top_ctx = top_src_ctx;
    for (j, step) in top_steps.iter().enumerate() {
        // Skip audio-only steps; they have no place in the video blend chain.
        if matches!(
            step,
            FilterStep::AReverse
                | FilterStep::AFadeIn { .. }
                | FilterStep::AFadeOut { .. }
                | FilterStep::ParametricEq { .. }
                | FilterStep::ANoiseGate { .. }
                | FilterStep::ACompressor { .. }
                | FilterStep::StereoToMono
                | FilterStep::ChannelMap { .. }
                | FilterStep::AudioDelay { .. }
                | FilterStep::ConcatAudio { .. }
                | FilterStep::LoudnessNormalize { .. }
                | FilterStep::NormalizePeak { .. }
        ) {
            continue;
        }
        top_ctx = add_and_link_step(graph, top_ctx, step, index * 1000 + j, "blend_top")?;
    }

    // 2. Create the blend filter.
    let blend_filter = ff_sys::avfilter_get_by_name(c"blend".as_ptr());
    if blend_filter.is_null() {
        log::warn!("filter not found name=blend (blend_photographic)");
        return Err(FilterError::BuildFailed);
    }
    let blend_name =
        CString::new(format!("blend_phot{index}")).map_err(|_| FilterError::BuildFailed)?;
    let blend_args_str = if (opacity - 1.0).abs() < f32::EPSILON {
        format!("all_mode={mode_name}")
    } else {
        format!("all_mode={mode_name}:all_opacity={opacity}")
    };
    let blend_args = CString::new(blend_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut blend_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut blend_ctx,
        blend_filter,
        blend_name.as_ptr(),
        blend_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=blend args={blend_args_str} (blend_photographic)");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=blend args={blend_args_str} index={index} (blend_photographic)");

    // 3. Link: bottom → blend[0], top → blend[1].
    // SAFETY: bottom_ctx, top_ctx, blend_ctx are all in the same graph.
    let ret = ff_sys::avfilter_link(bottom_ctx, 0, blend_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    let ret = ff_sys::avfilter_link(top_ctx, 0, blend_ctx, 1);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    log::debug!(
        "filter blend_photographic expanded mode={mode_name} opacity={opacity} index={index}"
    );
    Ok(blend_ctx)
}

// ── Blend Porter-Duff Under compound step ────────────────────────────────────

/// Insert a Porter-Duff Under blend step.
///
/// Equivalent to Over with inputs swapped: the top layer becomes the
/// background and the bottom layer is composited on top of it.
///
/// ```text
/// [in1]top_steps...[top_processed]
/// [top_processed][bottom_ctx]overlay=format=auto:shortest=1[out]
/// ```
///
/// # Safety
///
/// `graph`, `bottom_ctx`, and `top_src_ctx` must be valid pointers owned by the
/// same `AVFilterGraph`.
pub(super) unsafe fn add_blend_under_step(
    graph: *mut ff_sys::AVFilterGraph,
    bottom_ctx: *mut ff_sys::AVFilterContext,
    top_src_ctx: *mut ff_sys::AVFilterContext,
    top_steps: &[FilterStep],
    opacity: f32,
    alpha: AlphaMode,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    use std::ffi::CString;

    // 1. Chain the top builder's steps starting from the in1 buffersrc.
    let mut top_ctx = top_src_ctx;
    for (j, step) in top_steps.iter().enumerate() {
        if matches!(
            step,
            FilterStep::AReverse
                | FilterStep::AFadeIn { .. }
                | FilterStep::AFadeOut { .. }
                | FilterStep::ParametricEq { .. }
                | FilterStep::ANoiseGate { .. }
                | FilterStep::ACompressor { .. }
                | FilterStep::StereoToMono
                | FilterStep::ChannelMap { .. }
                | FilterStep::AudioDelay { .. }
                | FilterStep::ConcatAudio { .. }
                | FilterStep::LoudnessNormalize { .. }
                | FilterStep::NormalizePeak { .. }
        ) {
            continue;
        }
        top_ctx = add_and_link_step(graph, top_ctx, step, index * 1000 + j, "blend_top")?;
    }

    // 2. When opacity < 1.0, attenuate the bottom (background) layer's alpha channel.
    if opacity < 1.0 {
        let ccm_name = CString::new(format!("blend_under_ccm{index}"))
            .map_err(|_| FilterError::BuildFailed)?;
        let ccm_args_str = format!("aa={opacity}");
        let ccm_args = CString::new(ccm_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
        let ccm_filter = ff_sys::avfilter_get_by_name(c"colorchannelmixer".as_ptr());
        if ccm_filter.is_null() {
            log::warn!("filter not found name=colorchannelmixer (blend_under)");
            return Err(FilterError::BuildFailed);
        }
        let mut ccm_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut ccm_ctx,
            ccm_filter,
            ccm_name.as_ptr(),
            ccm_args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            log::warn!("filter creation failed name=colorchannelmixer args={ccm_args_str}");
            return Err(FilterError::BuildFailed);
        }
        log::debug!("filter added name=colorchannelmixer args={ccm_args_str} index={index}");
        // SAFETY: top_ctx and ccm_ctx belong to the same graph; pad indices valid.
        let ret = ff_sys::avfilter_link(top_ctx, 0, ccm_ctx, 0);
        if ret < 0 {
            return Err(FilterError::BuildFailed);
        }
        top_ctx = ccm_ctx;
    }

    // 3. Create the overlay filter.
    let overlay_filter = ff_sys::avfilter_get_by_name(c"overlay".as_ptr());
    if overlay_filter.is_null() {
        log::warn!("filter not found name=overlay (blend_under)");
        return Err(FilterError::BuildFailed);
    }
    let overlay_name = CString::new(format!("blend_under_overlay{index}"))
        .map_err(|_| FilterError::BuildFailed)?;
    let overlay_args_str = format!("format=auto:shortest=1{}", overlay_alpha_suffix(alpha));
    let overlay_args =
        CString::new(overlay_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut overlay_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut overlay_ctx,
        overlay_filter,
        overlay_name.as_ptr(),
        overlay_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=overlay args={overlay_args_str} (blend_under)");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=overlay args={overlay_args_str} index={index} (blend_under)");

    // 4. Link (swapped vs. Normal): top → overlay[0], bottom → overlay[1].
    // SAFETY: bottom_ctx, top_ctx, overlay_ctx are all in the same graph.
    let ret = ff_sys::avfilter_link(top_ctx, 0, overlay_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    let ret = ff_sys::avfilter_link(bottom_ctx, 0, overlay_ctx, 1);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    log::debug!("filter blend_under expanded opacity={opacity} index={index}");
    Ok(overlay_ctx)
}

// ── Blend expression-based compound step ────────────────────────────────────

/// Insert a Porter-Duff expression-based blend step (In, Out).
///
/// Chains `top_steps` on `top_src_ctx`, then creates
/// `blend=all_expr=<expr>` and links both inputs.
///
/// ```text
/// [in1]top_steps...[top_processed]
/// [bottom_ctx][top_processed]blend=all_expr=<expr>[out]
/// ```
///
/// # Safety
///
/// `graph`, `bottom_ctx`, and `top_src_ctx` must be valid pointers owned by the
/// same `AVFilterGraph`.
pub(super) unsafe fn add_blend_expr_step(
    graph: *mut ff_sys::AVFilterGraph,
    bottom_ctx: *mut ff_sys::AVFilterContext,
    top_src_ctx: *mut ff_sys::AVFilterContext,
    top_steps: &[FilterStep],
    expr: &str,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    use std::ffi::CString;

    // 1. Chain the top builder's steps starting from the in1 buffersrc.
    let mut top_ctx = top_src_ctx;
    for (j, step) in top_steps.iter().enumerate() {
        if matches!(
            step,
            FilterStep::AReverse
                | FilterStep::AFadeIn { .. }
                | FilterStep::AFadeOut { .. }
                | FilterStep::ParametricEq { .. }
                | FilterStep::ANoiseGate { .. }
                | FilterStep::ACompressor { .. }
                | FilterStep::StereoToMono
                | FilterStep::ChannelMap { .. }
                | FilterStep::AudioDelay { .. }
                | FilterStep::ConcatAudio { .. }
                | FilterStep::LoudnessNormalize { .. }
                | FilterStep::NormalizePeak { .. }
        ) {
            continue;
        }
        top_ctx = add_and_link_step(graph, top_ctx, step, index * 1000 + j, "blend_top")?;
    }

    // 2. Create the blend filter with the expression.
    let blend_filter = ff_sys::avfilter_get_by_name(c"blend".as_ptr());
    if blend_filter.is_null() {
        log::warn!("filter not found name=blend (blend_expr)");
        return Err(FilterError::BuildFailed);
    }
    let blend_name =
        CString::new(format!("blend_expr{index}")).map_err(|_| FilterError::BuildFailed)?;
    let blend_args_str = format!("all_expr={expr}");
    let blend_args = CString::new(blend_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut blend_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut blend_ctx,
        blend_filter,
        blend_name.as_ptr(),
        blend_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=blend args={blend_args_str} (blend_expr)");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=blend args={blend_args_str} index={index} (blend_expr)");

    // 3. Link: bottom → blend[0], top → blend[1].
    // SAFETY: bottom_ctx, top_ctx, blend_ctx are all in the same graph.
    let ret = ff_sys::avfilter_link(bottom_ctx, 0, blend_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    let ret = ff_sys::avfilter_link(top_ctx, 0, blend_ctx, 1);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    log::debug!("filter blend_expr expanded expr={expr} index={index}");
    Ok(blend_ctx)
}

// ── FeatherMask compound step ─────────────────────────────────────────────────

/// Insert the feather-mask compound step.
///
/// Splits the input into a color copy and an alpha copy, blurs the alpha
/// using `gblur=sigma=<radius>`, then re-merges:
///
/// ```text
/// [in]split=2[color][with_alpha];
/// [with_alpha]alphaextract[alpha_only];
/// [alpha_only]gblur=sigma=<radius>[alpha_blurred];
/// [color][alpha_blurred]alphamerge[out]
/// ```
///
/// # Safety
///
/// `graph` and `prev_ctx` must be valid pointers owned by the same
/// `AVFilterGraph`.
// ── Glow compound step ────────────────────────────────────────────────────────
/// Insert the glow / bloom compound step.
///
/// ```text
/// prev_ctx → split=2 → pad0 ──────────────────────────────→ blend[0]
///                    → pad1 → curves(hi_lo) → gblur(r) ──→ blend[1]
///                                                            blend(addition, opacity=iv) → out
/// ```
///
/// # Safety
///
/// `graph` and `prev_ctx` must be valid pointers owned by the same
/// `AVFilterGraph`.
pub(super) unsafe fn add_glow_step(
    graph: *mut ff_sys::AVFilterGraph,
    prev_ctx: *mut ff_sys::AVFilterContext,
    threshold: f32,
    radius: f32,
    intensity: f32,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    use std::ffi::CString;

    let t = threshold.clamp(0.0, 1.0);
    let r = radius.clamp(0.5, 50.0);
    let iv = intensity.clamp(0.0, 2.0);

    // 1. split=2 — duplicate the stream into a base path and a highlights path.
    let split_filter = ff_sys::avfilter_get_by_name(c"split".as_ptr());
    if split_filter.is_null() {
        log::warn!("filter not found name=split (glow)");
        return Err(FilterError::BuildFailed);
    }
    let split_name =
        CString::new(format!("glow_split{index}")).map_err(|_| FilterError::BuildFailed)?;
    let mut split_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    // SAFETY: split_filter and graph are non-null; "2" is valid args.
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut split_ctx,
        split_filter,
        split_name.as_ptr(),
        c"2".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=split (glow) code={ret}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=split args=2 index={index} (glow)");

    // Link: prev_ctx → split[0].
    // SAFETY: prev_ctx and split_ctx belong to the same graph; pad indices valid.
    let ret = ff_sys::avfilter_link(prev_ctx, 0, split_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 2. curves — extract highlights: below threshold → black; above → full.
    let curves_filter = ff_sys::avfilter_get_by_name(c"curves".as_ptr());
    if curves_filter.is_null() {
        log::warn!("filter not found name=curves (glow)");
        return Err(FilterError::BuildFailed);
    }
    let curves_name =
        CString::new(format!("glow_curves{index}")).map_err(|_| FilterError::BuildFailed)?;
    let hi_lo = format!("0/0 {t}/0 1/1");
    let curves_args_str = format!("all='{hi_lo}'");
    let curves_args =
        CString::new(curves_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut curves_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut curves_ctx,
        curves_filter,
        curves_name.as_ptr(),
        curves_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=curves args={curves_args_str} (glow) code={ret}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=curves args={curves_args_str} index={index} (glow)");

    // Link: split[1] → curves[0] (highlights path).
    // SAFETY: split_ctx and curves_ctx belong to the same graph; pad 1 of split valid.
    let ret = ff_sys::avfilter_link(split_ctx, 1, curves_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 3. gblur — blur the extracted highlights.
    let gblur_filter = ff_sys::avfilter_get_by_name(c"gblur".as_ptr());
    if gblur_filter.is_null() {
        log::warn!("filter not found name=gblur (glow)");
        return Err(FilterError::BuildFailed);
    }
    let gblur_name =
        CString::new(format!("glow_gblur{index}")).map_err(|_| FilterError::BuildFailed)?;
    let gblur_args_str = format!("sigma={r}");
    let gblur_args = CString::new(gblur_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut gblur_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut gblur_ctx,
        gblur_filter,
        gblur_name.as_ptr(),
        gblur_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=gblur args={gblur_args_str} (glow) code={ret}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=gblur args={gblur_args_str} index={index} (glow)");

    // Link: curves → gblur.
    let ret = ff_sys::avfilter_link(curves_ctx, 0, gblur_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 4. blend — additively blend blurred highlights back over the base image.
    let blend_filter = ff_sys::avfilter_get_by_name(c"blend".as_ptr());
    if blend_filter.is_null() {
        log::warn!("filter not found name=blend (glow)");
        return Err(FilterError::BuildFailed);
    }
    let blend_name =
        CString::new(format!("glow_blend{index}")).map_err(|_| FilterError::BuildFailed)?;
    let blend_args_str = format!("all_mode=addition:all_opacity={iv}");
    let blend_args = CString::new(blend_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut blend_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut blend_ctx,
        blend_filter,
        blend_name.as_ptr(),
        blend_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=blend args={blend_args_str} (glow) code={ret}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=blend args={blend_args_str} index={index} (glow)");

    // Link: split[0] → blend[0] (base path — the bottom stream).
    // SAFETY: split_ctx and blend_ctx belong to the same graph; pad indices valid.
    let ret = ff_sys::avfilter_link(split_ctx, 0, blend_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // Link: gblur → blend[1] (blurred highlights — the top stream).
    let ret = ff_sys::avfilter_link(gblur_ctx, 0, blend_ctx, 1);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    log::debug!(
        "filter glow expanded threshold={threshold} radius={radius} intensity={intensity} index={index}"
    );
    Ok(blend_ctx)
}

pub(super) unsafe fn add_feather_mask_step(
    graph: *mut ff_sys::AVFilterGraph,
    prev_ctx: *mut ff_sys::AVFilterContext,
    radius: u32,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    use std::ffi::CString;

    // 1. split=2 — duplicate the stream into a color copy and an alpha copy.
    let split_filter = ff_sys::avfilter_get_by_name(c"split".as_ptr());
    if split_filter.is_null() {
        log::warn!("filter not found name=split (feather_mask)");
        return Err(FilterError::BuildFailed);
    }
    let split_name =
        CString::new(format!("feather_split{index}")).map_err(|_| FilterError::BuildFailed)?;
    let mut split_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    // SAFETY: split_filter and graph are non-null; "2" is valid args.
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut split_ctx,
        split_filter,
        split_name.as_ptr(),
        c"2".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=split (feather_mask) code={ret}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=split args=2 index={index} (feather_mask)");

    // Link: prev_ctx → split[0].
    // SAFETY: prev_ctx and split_ctx belong to the same graph; pad indices valid.
    let ret = ff_sys::avfilter_link(prev_ctx, 0, split_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 2. alphaextract — convert the alpha plane from split output pad 1 into
    //    a grayscale stream so it can be processed independently.
    let alphaextract_filter = ff_sys::avfilter_get_by_name(c"alphaextract".as_ptr());
    if alphaextract_filter.is_null() {
        log::warn!("filter not found name=alphaextract (feather_mask)");
        return Err(FilterError::BuildFailed);
    }
    let alphaextract_name = CString::new(format!("feather_alphaextract{index}"))
        .map_err(|_| FilterError::BuildFailed)?;
    let mut alphaextract_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut alphaextract_ctx,
        alphaextract_filter,
        alphaextract_name.as_ptr(),
        std::ptr::null(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=alphaextract (feather_mask) code={ret}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=alphaextract index={index} (feather_mask)");

    // Link: split[1] → alphaextract[0].
    // SAFETY: split_ctx and alphaextract_ctx belong to the same graph; pad 1 of split valid.
    let ret = ff_sys::avfilter_link(split_ctx, 1, alphaextract_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 3. gblur=sigma=<radius> — blur the extracted alpha plane.
    let gblur_filter = ff_sys::avfilter_get_by_name(c"gblur".as_ptr());
    if gblur_filter.is_null() {
        log::warn!("filter not found name=gblur (feather_mask)");
        return Err(FilterError::BuildFailed);
    }
    let gblur_name =
        CString::new(format!("feather_gblur{index}")).map_err(|_| FilterError::BuildFailed)?;
    let gblur_args_str = format!("sigma={radius}");
    let gblur_args = CString::new(gblur_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut gblur_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut gblur_ctx,
        gblur_filter,
        gblur_name.as_ptr(),
        gblur_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!(
            "filter creation failed name=gblur args={gblur_args_str} (feather_mask) code={ret}"
        );
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=gblur args={gblur_args_str} index={index} (feather_mask)");

    // Link: alphaextract → gblur.
    let ret = ff_sys::avfilter_link(alphaextract_ctx, 0, gblur_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 4. alphamerge — re-merge color (split pad 0) with blurred alpha (gblur output).
    let alphamerge_filter = ff_sys::avfilter_get_by_name(c"alphamerge".as_ptr());
    if alphamerge_filter.is_null() {
        log::warn!("filter not found name=alphamerge (feather_mask)");
        return Err(FilterError::BuildFailed);
    }
    let alphamerge_name =
        CString::new(format!("feather_alphamerge{index}")).map_err(|_| FilterError::BuildFailed)?;
    let mut alphamerge_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    // SAFETY: alphamerge_filter and graph are non-null; null args are accepted.
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut alphamerge_ctx,
        alphamerge_filter,
        alphamerge_name.as_ptr(),
        std::ptr::null(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=alphamerge (feather_mask) code={ret}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=alphamerge index={index} (feather_mask)");

    // Link: split[0] → alphamerge[0] (color path).
    // SAFETY: split_ctx and alphamerge_ctx belong to the same graph; pad indices valid.
    let ret = ff_sys::avfilter_link(split_ctx, 0, alphamerge_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // Link: gblur → alphamerge[1] (blurred alpha path).
    let ret = ff_sys::avfilter_link(gblur_ctx, 0, alphamerge_ctx, 1);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    log::debug!("filter feather_mask expanded radius={radius} index={index}");
    Ok(alphamerge_ctx)
}

// ── AlphaMatte compound step ─────────────────────────────────────────────────

/// Insert the alphamerge compound step.
///
/// Chains `matte_steps` on `matte_src_ctx` (the `in1` buffersrc), then creates
/// the `alphamerge` filter and links:
///
/// ```text
/// [in1]matte_steps...[matte_processed]
/// [bottom_ctx][matte_processed]alphamerge[out]
/// ```
///
/// # Safety
///
/// `graph`, `bottom_ctx`, and `matte_src_ctx` must be valid pointers owned by
/// the same `AVFilterGraph`.
pub(super) unsafe fn add_alphamerge_step(
    graph: *mut ff_sys::AVFilterGraph,
    bottom_ctx: *mut ff_sys::AVFilterContext,
    matte_src_ctx: *mut ff_sys::AVFilterContext,
    matte_steps: &[FilterStep],
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    use std::ffi::CString;

    // 1. Chain the matte builder's steps starting from the in1 buffersrc.
    let mut matte_ctx = matte_src_ctx;
    for (j, step) in matte_steps.iter().enumerate() {
        // Skip audio-only steps; they have no place in the matte chain.
        if matches!(
            step,
            FilterStep::AReverse
                | FilterStep::AFadeIn { .. }
                | FilterStep::AFadeOut { .. }
                | FilterStep::ParametricEq { .. }
                | FilterStep::ANoiseGate { .. }
                | FilterStep::ACompressor { .. }
                | FilterStep::StereoToMono
                | FilterStep::ChannelMap { .. }
                | FilterStep::AudioDelay { .. }
                | FilterStep::ConcatAudio { .. }
                | FilterStep::LoudnessNormalize { .. }
                | FilterStep::NormalizePeak { .. }
        ) {
            continue;
        }
        matte_ctx = add_and_link_step(graph, matte_ctx, step, index * 1000 + j, "matte")?;
    }

    // 2. Create the alphamerge filter.
    let alphamerge_filter = ff_sys::avfilter_get_by_name(c"alphamerge".as_ptr());
    if alphamerge_filter.is_null() {
        log::warn!("filter not found name=alphamerge");
        return Err(FilterError::BuildFailed);
    }
    let alphamerge_name =
        CString::new(format!("alphamerge{index}")).map_err(|_| FilterError::BuildFailed)?;
    let mut alphamerge_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    // SAFETY: alphamerge_filter and graph are non-null; null opaque and args are accepted.
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut alphamerge_ctx,
        alphamerge_filter,
        alphamerge_name.as_ptr(),
        std::ptr::null(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=alphamerge code={ret}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=alphamerge index={index}");

    // 3. Link: main video → alphamerge[0], matte → alphamerge[1].
    // SAFETY: bottom_ctx, matte_ctx, alphamerge_ctx are all in the same graph.
    let ret = ff_sys::avfilter_link(bottom_ctx, 0, alphamerge_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    let ret = ff_sys::avfilter_link(matte_ctx, 0, alphamerge_ctx, 1);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    log::debug!("filter alphamerge linked index={index}");
    Ok(alphamerge_ctx)
}

// ── buffersrc / buffersink arg-string helpers ──────────────────────────────────

/// Build the `args` string passed to `avfilter_graph_create_filter` when
/// creating a video `buffer` (buffersrc) context.
///
/// The format follows libavfilter's `buffer` filter parameter syntax:
/// `video_size=WxH:pix_fmt=N:time_base=NUM/DEN:pixel_aspect=1/1`.
pub(super) fn video_buffersrc_args(
    width: u32,
    height: u32,
    pix_fmt: std::os::raw::c_int,
) -> String {
    format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect=1/1",
        width, height, pix_fmt, VIDEO_TIME_BASE_NUM, VIDEO_TIME_BASE_DEN,
    )
}

/// Build the `args` string passed to `avfilter_graph_create_filter` when
/// creating an audio `abuffer` (buffersrc) context.
///
/// The format follows libavfilter's `abuffer` filter parameter syntax:
/// `sample_rate=R:sample_fmt=FMT:channels=C:time_base=1/R`.
pub(super) fn audio_buffersrc_args(
    sample_rate: u32,
    sample_fmt_name: &str,
    channels: u32,
) -> String {
    format!(
        "sample_rate={}:sample_fmt={}:channels={}:time_base={}/{}",
        sample_rate, sample_fmt_name, channels, AUDIO_TIME_BASE_NUM, sample_rate,
    )
}

/// Parse the `sample_rate` field from a `buffersrc_args` string.
///
/// The expected format is `sample_rate=R:sample_fmt=FMT:channels=C:...`.
/// Returns `44100` as a safe fallback if the field is absent or unparseable.
pub(super) fn parse_sample_rate_from_buffersrc(buffersrc_args: &str) -> u32 {
    buffersrc_args
        .split(':')
        .find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            if k == "sample_rate" {
                v.parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(44100)
}

// ── Parametric EQ (multi-band chain) ─────────────────────────────────────────

/// Insert a chain of filter nodes for a [`FilterStep::ParametricEq`] step.
///
/// One node per band is created using the band's own filter name (e.g.,
/// `lowshelf`, `highshelf`, `equalizer`) and args, and each is linked in
/// series after `prev_ctx`.
///
/// # Safety
///
/// `graph` and `prev_ctx` must be valid pointers owned by the same
/// `AVFilterGraph`.
pub(super) unsafe fn add_parametric_eq_chain(
    graph: *mut ff_sys::AVFilterGraph,
    prev_ctx: *mut ff_sys::AVFilterContext,
    bands: &[EqBand],
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    let mut ctx = prev_ctx;
    for (j, band) in bands.iter().enumerate() {
        let filter_name =
            std::ffi::CString::new(band.filter_name()).map_err(|_| FilterError::BuildFailed)?;
        let filter = ff_sys::avfilter_get_by_name(filter_name.as_ptr());
        if filter.is_null() {
            log::warn!("filter not found name={}", band.filter_name());
            return Err(FilterError::BuildFailed);
        }

        let name = std::ffi::CString::new(format!("eq{index}_{j}"))
            .map_err(|_| FilterError::BuildFailed)?;
        let args_str = band.args();
        let args =
            std::ffi::CString::new(args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;

        let mut band_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut band_ctx,
            filter,
            name.as_ptr(),
            args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            log::warn!(
                "filter creation failed name={} args={}",
                band.filter_name(),
                args_str
            );
            return Err(FilterError::BuildFailed);
        }
        log::debug!(
            "filter added name={} args={} index={index} band={j}",
            band.filter_name(),
            args_str
        );

        let ret = ff_sys::avfilter_link(ctx, 0, band_ctx, 0);
        if ret < 0 {
            return Err(FilterError::BuildFailed);
        }
        ctx = band_ctx;
    }
    Ok(ctx)
}

// ── JoinWithDissolve compound step ────────────────────────────────────────────

/// Expand a `JoinWithDissolve` step into the following compound filter graph:
///
/// ```text
/// clip_a_src → trim(end=clip_a_end+dissolve_dur) → setpts → xfade[0]
/// clip_b_src → trim(start=max(0, clip_b_start−dissolve_dur)) → setpts → xfade[1]
/// ```
///
/// Returns the `xfade` filter context, which becomes the new `prev_ctx`.
///
/// # Safety
///
/// `graph`, `clip_a_src`, and `clip_b_src` must be valid non-null pointers
/// belonging to the same `AVFilterGraph`.
pub(super) unsafe fn add_join_with_dissolve_step(
    graph: *mut ff_sys::AVFilterGraph,
    clip_a_src: *mut ff_sys::AVFilterContext,
    clip_b_src: *mut ff_sys::AVFilterContext,
    clip_a_end: f64,
    clip_b_start: f64,
    dissolve_dur: f64,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    // 1. trim_a: keep clip A frames up to clip_a_end + dissolve_dur
    let a_trim_end = clip_a_end + dissolve_dur;
    let trim_a = add_raw_filter_step(
        graph,
        clip_a_src,
        "trim",
        &format!("end={a_trim_end}"),
        index,
        "jwd_trima",
    )?;

    // 2. setpts_a: reset clip A timestamps to zero after trim
    let setpts_a = add_raw_filter_step(
        graph,
        trim_a,
        "setpts",
        "PTS-STARTPTS",
        index,
        "jwd_setptsa",
    )?;

    // 3. Create xfade filter; pads are wired manually below.
    let xfade_filter = ff_sys::avfilter_get_by_name(c"xfade".as_ptr());
    if xfade_filter.is_null() {
        log::warn!("filter not found name=xfade (join_with_dissolve)");
        return Err(FilterError::BuildFailed);
    }
    let xfade_name = std::ffi::CString::new(format!("jwd_xfade{index}"))
        .map_err(|_| FilterError::BuildFailed)?;
    let xfade_args_str = format!("transition=dissolve:duration={dissolve_dur}:offset={clip_a_end}");
    let xfade_args =
        std::ffi::CString::new(xfade_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut xfade_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    // SAFETY: all pointers are valid; xfade_filter, xfade_name, and xfade_args
    // are non-null and null-terminated.
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut xfade_ctx,
        xfade_filter,
        xfade_name.as_ptr(),
        xfade_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=xfade args={xfade_args_str} (join_with_dissolve)");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=xfade args={xfade_args_str} (join_with_dissolve)");

    // Link setpts_a (clip A chain) → xfade[0]
    // SAFETY: setpts_a and xfade_ctx belong to the same graph; pad indices valid.
    let ret = ff_sys::avfilter_link(setpts_a, 0, xfade_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 4. trim_b: keep clip B frames from max(0, clip_b_start - dissolve_dur)
    let b_trim_start = (clip_b_start - dissolve_dur).max(0.0);
    let trim_b = add_raw_filter_step(
        graph,
        clip_b_src,
        "trim",
        &format!("start={b_trim_start}"),
        index,
        "jwd_trimb",
    )?;

    // 5. setpts_b: reset clip B timestamps to zero after trim
    let setpts_b = add_raw_filter_step(
        graph,
        trim_b,
        "setpts",
        "PTS-STARTPTS",
        index,
        "jwd_setptsb",
    )?;

    // Link setpts_b (clip B chain) → xfade[1]
    // SAFETY: setpts_b and xfade_ctx belong to the same graph; pad index 1 is valid for xfade.
    let ret = ff_sys::avfilter_link(setpts_b, 0, xfade_ctx, 1);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    log::debug!(
        "filter join_with_dissolve expanded dissolve_dur={dissolve_dur} offset={clip_a_end}"
    );
    Ok(xfade_ctx)
}

// ── Graph orchestrators ───────────────────────────────────────────────────────

use super::FilterGraphInner;

impl FilterGraphInner {
    /// Build the `AVFilterGraph` for video, returning `(src_ctxs, vsink_ctx)`.
    ///
    /// `num_inputs` buffersrc contexts are created (`in0`..`inN-1`).  For
    /// multi-input filters like `overlay`, the extra sources are linked to the
    /// appropriate input pads after the main chain link is established.
    ///
    /// # Safety
    ///
    /// `graph_nn` must be a valid, freshly-allocated `AVFilterGraph`.
    pub(super) unsafe fn build_video_graph(
        graph_nn: NonNull<ff_sys::AVFilterGraph>,
        buffersrc_args: &str,
        num_inputs: usize,
        steps: &[FilterStep],
        hw: Option<&HwAccel>,
    ) -> VideoGraphResult {
        let graph = graph_nn.as_ptr();

        // 0. When hardware acceleration is requested, create a device context
        //    and disable automatic pixel-format conversion so FFmpeg does not
        //    insert implicit hwupload/scale filters that would conflict with the
        //    explicit ones we add below.
        let hw_device_ctx: Option<*mut ff_sys::AVBufferRef> = if let Some(hw) = hw {
            let device_type = hw_accel_to_device_type(*hw);
            let mut raw_hw_ctx: *mut ff_sys::AVBufferRef = std::ptr::null_mut();
            let ret = ff_sys::av_hwdevice_ctx_create(
                &raw mut raw_hw_ctx,
                device_type,
                std::ptr::null(),     // device: null = system default
                std::ptr::null_mut(), // opts: null = defaults
                0,
            );
            if ret < 0 {
                log::warn!("av_hwdevice_ctx_create failed hw={hw:?} code={ret}");
                return Err(FilterError::BuildFailed);
            }
            // AVFILTER_AUTO_CONVERT_NONE = 0: hardware filters must receive
            // frames in exactly the format they expect.
            ff_sys::avfilter_graph_set_auto_convert(graph, 0u32);
            log::debug!("hw device context created hw={hw:?}");
            Some(raw_hw_ctx)
        } else {
            None
        };

        // Helper closure: free hw_device_ctx and return an error.  Used at
        // every early-return failure point that occurs *after* the device
        // context has been allocated so it is not leaked.
        macro_rules! bail {
            ($err:expr) => {{
                if let Some(mut hw_ctx) = hw_device_ctx {
                    ff_sys::av_buffer_unref(std::ptr::addr_of_mut!(hw_ctx));
                }
                return Err($err);
            }};
        }

        // SAFETY: `avfilter_get_by_name` returns a borrowed pointer valid for
        // the process lifetime; we never free it.
        let buffersrc = ff_sys::avfilter_get_by_name(c"buffer".as_ptr());
        if buffersrc.is_null() {
            bail!(FilterError::BuildFailed);
        }

        let Ok(src_args) = std::ffi::CString::new(buffersrc_args) else {
            bail!(FilterError::BuildFailed)
        };
        let mut src_ctxs: FilterCtxVec = Vec::with_capacity(num_inputs);

        // 1. Create in0 (always present).
        let mut raw_ctx0: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut raw_ctx0,
            buffersrc,
            c"in0".as_ptr(),
            src_args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            bail!(FilterError::BuildFailed);
        }
        log::debug!("filter added name=buffersrc slot=0");
        // SAFETY: ret >= 0 guarantees non-null.
        src_ctxs.push(Some(NonNull::new_unchecked(raw_ctx0)));

        // Create in1..inN-1 (for overlay etc.)
        for slot in 1..num_inputs {
            let Ok(ctx_name) = std::ffi::CString::new(format!("in{slot}")) else {
                bail!(FilterError::BuildFailed)
            };
            let mut raw_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut raw_ctx,
                buffersrc,
                ctx_name.as_ptr(),
                src_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                bail!(FilterError::BuildFailed);
            }
            log::debug!("filter added name=buffersrc slot={slot}");
            // SAFETY: ret >= 0 guarantees non-null.
            src_ctxs.push(Some(NonNull::new_unchecked(raw_ctx)));
        }

        // 2. Create buffersink ("buffersink").
        let buffersink = ff_sys::avfilter_get_by_name(c"buffersink".as_ptr());
        if buffersink.is_null() {
            bail!(FilterError::BuildFailed);
        }

        let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut sink_ctx,
            buffersink,
            c"out".as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            bail!(FilterError::BuildFailed);
        }

        // 3. Insert hwupload/hwupload_cuda BEFORE the filter steps so that
        //    subsequent filters receive hardware (CUDA/VAAPI/VTB) frames.
        let mut prev_ctx: *mut ff_sys::AVFilterContext = raw_ctx0;
        if let (Some(hw_ctx), Some(hw_backend)) = (hw_device_ctx, hw) {
            let upload_name = match hw_backend {
                HwAccel::Cuda => c"hwupload_cuda",
                HwAccel::VideoToolbox | HwAccel::Vaapi => c"hwupload",
            };
            prev_ctx = match create_hw_filter(graph, prev_ctx, upload_name, c"hwupload0", hw_ctx) {
                Ok(ctx) => ctx,
                Err(e) => bail!(e),
            };
        }

        // 4-5. Add each `FilterStep`, link the main chain (in0 → step[0] → …),
        // and wire extra input pads for multi-input filters.
        for (i, step) in steps.iter().enumerate() {
            // Audio-only steps; skip them in the video graph.
            if matches!(
                step,
                FilterStep::AReverse
                    | FilterStep::AFadeIn { .. }
                    | FilterStep::AFadeOut { .. }
                    | FilterStep::ParametricEq { .. }
                    | FilterStep::ANoiseGate { .. }
                    | FilterStep::ACompressor { .. }
                    | FilterStep::StereoToMono
                    | FilterStep::ChannelMap { .. }
                    | FilterStep::AudioDelay { .. }
                    | FilterStep::ConcatAudio { .. }
            ) {
                continue;
            }

            // LoudnessNormalize is audio-only and handled via two-pass in
            // push_audio / pull_audio rather than through the filter graph.
            if matches!(step, FilterStep::LoudnessNormalize { .. }) {
                continue;
            }

            // NormalizePeak is audio-only and handled via two-pass buffering in
            // push_audio / pull_audio.
            if matches!(step, FilterStep::NormalizePeak { .. }) {
                continue;
            }

            // OverlayImage is a compound step (movie → lut → overlay).  It
            // creates its own internal source node via the `movie` filter and
            // does not consume a buffersrc slot, so it must bypass the standard
            // `add_and_link_step` path which assumes a single filter per step.
            if let FilterStep::OverlayImage {
                path,
                x,
                y,
                opacity,
            } = step
            {
                prev_ctx = match add_overlay_image_step(graph, prev_ctx, path, x, y, *opacity, i) {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
                continue;
            }

            // JoinWithDissolve is a compound step that expands to:
            //   prev → trim_a → setpts_a → xfade[0]
            //   in1  → trim_b → setpts_b → xfade[1]
            // It bypasses the standard add_and_link_step path entirely.
            if let FilterStep::JoinWithDissolve {
                clip_a_end,
                clip_b_start,
                dissolve_dur,
            } = step
            {
                let Some(b_src) = src_ctxs.get(1).and_then(|o| *o) else {
                    bail!(FilterError::BuildFailed)
                };
                prev_ctx = match add_join_with_dissolve_step(
                    graph,
                    prev_ctx,
                    b_src.as_ptr(),
                    *clip_a_end,
                    *clip_b_start,
                    *dissolve_dur,
                    i,
                ) {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
                continue;
            }

            // Glow — compound step: split → [base | curves → gblur] → blend(addition).
            // All filters operate on the single main stream; no extra buffersrc needed.
            if let FilterStep::Glow {
                threshold,
                radius,
                intensity,
            } = step
            {
                prev_ctx = match add_glow_step(graph, prev_ctx, *threshold, *radius, *intensity, i)
                {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
                continue;
            }

            // FeatherMask — compound step: split → alphaextract → gblur → alphamerge.
            // All filters operate on the single main stream; no extra buffersrc needed.
            if let FilterStep::FeatherMask { radius } = step {
                prev_ctx = match add_feather_mask_step(graph, prev_ctx, *radius, i) {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
                continue;
            }

            // AlphaMatte — compound step: matte pipeline applied to in1, then
            // alphamerge links main video (pad 0) with the matte (pad 1).
            if let FilterStep::AlphaMatte { matte } = step {
                let Some(matte_src) = src_ctxs.get(1).and_then(|o| *o) else {
                    bail!(FilterError::BuildFailed)
                };
                prev_ctx = match add_alphamerge_step(
                    graph,
                    prev_ctx,
                    matte_src.as_ptr(),
                    matte.steps(),
                    i,
                ) {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
                continue;
            }

            // Blend (Normal / PorterDuffOver) — both use overlay=format=auto:shortest=1.
            //   prev → [bottom]overlay=format=auto:shortest=1 ← [top][ccm]
            // where [top] is in1 with the top builder's steps applied.
            // Unimplemented modes are caught by build() before reaching here.
            if let FilterStep::Blend {
                top,
                mode: BlendMode::Normal | BlendMode::PorterDuffOver,
                opacity,
                alpha,
            } = step
            {
                let Some(top_src) = src_ctxs.get(1).and_then(|o| *o) else {
                    bail!(FilterError::BuildFailed)
                };
                prev_ctx = match add_blend_normal_step(
                    graph,
                    prev_ctx,
                    top_src.as_ptr(),
                    top.steps(),
                    *opacity,
                    *alpha,
                    i,
                ) {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
                continue;
            }

            // Blend (photographic modes: Multiply, Screen, Overlay, SoftLight, HardLight)
            if let FilterStep::Blend {
                top,
                mode:
                    mode @ (BlendMode::Multiply
                    | BlendMode::Screen
                    | BlendMode::Overlay
                    | BlendMode::SoftLight
                    | BlendMode::HardLight
                    | BlendMode::ColorDodge
                    | BlendMode::ColorBurn
                    | BlendMode::Darken
                    | BlendMode::Lighten
                    | BlendMode::Difference
                    | BlendMode::Exclusion
                    | BlendMode::Add
                    | BlendMode::Subtract
                    | BlendMode::Hue
                    | BlendMode::Saturation
                    | BlendMode::Color
                    | BlendMode::Luminosity),
                opacity,
                ..
            } = step
            {
                let Some(top_src) = src_ctxs.get(1).and_then(|o| *o) else {
                    bail!(FilterError::BuildFailed)
                };
                let mode_name = match mode {
                    BlendMode::Multiply => "multiply",
                    BlendMode::Screen => "screen",
                    BlendMode::Overlay => "overlay",
                    BlendMode::SoftLight => "softlight",
                    BlendMode::HardLight => "hardlight",
                    BlendMode::ColorDodge => "dodge",
                    BlendMode::ColorBurn => "burn",
                    BlendMode::Darken => "darken",
                    BlendMode::Lighten => "lighten",
                    BlendMode::Difference => "difference",
                    BlendMode::Exclusion => "exclusion",
                    BlendMode::Add => "addition",
                    BlendMode::Subtract => "subtract",
                    BlendMode::Hue => "hue",
                    BlendMode::Saturation => "saturation",
                    BlendMode::Color => "color",
                    BlendMode::Luminosity => "luminosity",
                    _ => unreachable!(),
                };
                prev_ctx = match add_blend_photographic_step(
                    graph,
                    prev_ctx,
                    top_src.as_ptr(),
                    top.steps(),
                    mode_name,
                    *opacity,
                    i,
                ) {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
                continue;
            }

            // Blend (PorterDuffUnder) — overlay with swapped input order.
            if let FilterStep::Blend {
                top,
                mode: BlendMode::PorterDuffUnder,
                opacity,
                alpha,
            } = step
            {
                let Some(top_src) = src_ctxs.get(1).and_then(|o| *o) else {
                    bail!(FilterError::BuildFailed)
                };
                prev_ctx = match add_blend_under_step(
                    graph,
                    prev_ctx,
                    top_src.as_ptr(),
                    top.steps(),
                    *opacity,
                    *alpha,
                    i,
                ) {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
                continue;
            }

            // Blend (PorterDuffIn / PorterDuffOut / PorterDuffAtop / PorterDuffXor) — expression-based blend.
            if let FilterStep::Blend {
                top,
                mode:
                    mode @ (BlendMode::PorterDuffIn
                    | BlendMode::PorterDuffOut
                    | BlendMode::PorterDuffAtop
                    | BlendMode::PorterDuffXor),
                ..
            } = step
            {
                let Some(top_src) = src_ctxs.get(1).and_then(|o| *o) else {
                    bail!(FilterError::BuildFailed)
                };
                let expr = match mode {
                    BlendMode::PorterDuffIn => "B*A/255",
                    BlendMode::PorterDuffOut => "B*(255-A)/255",
                    BlendMode::PorterDuffAtop => "B*A/255 + A*(255-B)/255",
                    BlendMode::PorterDuffXor => "B*(255-A)/255 + A*(255-B)/255",
                    _ => unreachable!(),
                };
                prev_ctx = match add_blend_expr_step(
                    graph,
                    prev_ctx,
                    top_src.as_ptr(),
                    top.steps(),
                    expr,
                    i,
                ) {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
                continue;
            }

            // Animated filter steps use type-specific node names ("crop_N",
            // "gblur_N") so that avfilter_graph_send_command targets the correct
            // context in #363.  The per-type index is the count of preceding steps
            // of the same animated type.
            let (step_prefix, step_index): (&str, usize) = match step {
                FilterStep::CropAnimated { .. } => {
                    let n = steps[..i]
                        .iter()
                        .filter(|s| matches!(s, FilterStep::CropAnimated { .. }))
                        .count();
                    ("crop_", n)
                }
                FilterStep::GBlurAnimated { .. } => {
                    let n = steps[..i]
                        .iter()
                        .filter(|s| matches!(s, FilterStep::GBlurAnimated { .. }))
                        .count();
                    ("gblur_", n)
                }
                FilterStep::EqAnimated { .. } => {
                    let n = steps[..i]
                        .iter()
                        .filter(|s| matches!(s, FilterStep::EqAnimated { .. }))
                        .count();
                    ("eq_", n)
                }
                FilterStep::ColorBalanceAnimated { .. } => {
                    let n = steps[..i]
                        .iter()
                        .filter(|s| matches!(s, FilterStep::ColorBalanceAnimated { .. }))
                        .count();
                    ("colorbalance_", n)
                }
                _ => ("step", i),
            };
            prev_ctx = match add_and_link_step(graph, prev_ctx, step, step_index, step_prefix) {
                Ok(ctx) => ctx,
                Err(e) => bail!(e),
            };

            // After trim, insert setpts=PTS-STARTPTS so the output timestamps
            // are reset to start at zero.
            if matches!(step, FilterStep::Trim { .. }) {
                prev_ctx = match add_setpts_after_trim(graph, prev_ctx, i) {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
            }

            // FitToAspect is a compound step: the scale filter (added by
            // add_and_link_step above) preserves the source aspect ratio; the
            // pad filter added here centres the scaled frame on the target canvas.
            if let FilterStep::FitToAspect {
                width,
                height,
                color,
            } = step
            {
                prev_ctx = match add_fit_to_aspect_pad(graph, prev_ctx, *width, *height, color, i) {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
            }

            // LumaKey with invert=true appends a geq filter that negates the
            // alpha channel, turning the "key out pixels matching threshold"
            // effect into "key out the complementary region".
            if let FilterStep::LumaKey { invert: true, .. } = step {
                prev_ctx = match add_raw_filter_step(
                    graph,
                    prev_ctx,
                    "geq",
                    "r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='255-alpha(X,Y)'",
                    i,
                    "geqluma",
                ) {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
            }

            // Overlay and xfade both consume a second input on pad 1.
            if matches!(step, FilterStep::Overlay { .. } | FilterStep::XFade { .. })
                && let Some(Some(extra_src)) = src_ctxs.get(1)
            {
                let ret = ff_sys::avfilter_link(extra_src.as_ptr(), 0, prev_ctx, 1);
                if ret < 0 {
                    bail!(FilterError::BuildFailed);
                }
                log::debug!("filter linked extra_input=in1 to two-input filter pad=1");
            }

            // ConcatVideo consumes n input pads; link src_ctxs[1..n-1] to pads 1..n-1.
            if let FilterStep::ConcatVideo { n } = step {
                for slot in 1..*n as usize {
                    if let Some(Some(extra_src)) = src_ctxs.get(slot) {
                        let ret =
                            ff_sys::avfilter_link(extra_src.as_ptr(), 0, prev_ctx, slot as u32);
                        if ret < 0 {
                            bail!(FilterError::BuildFailed);
                        }
                        log::debug!("filter linked extra_input=in{slot} to concat pad={slot}");
                    }
                }
            }
        }

        // 6. Insert hwdownload AFTER all filter steps so output frames are
        //    downloaded back to system memory before reaching the buffersink.
        if let Some(hw_ctx) = hw_device_ctx {
            prev_ctx =
                match create_hw_filter(graph, prev_ctx, c"hwdownload", c"hwdownload0", hw_ctx) {
                    Ok(ctx) => ctx,
                    Err(e) => bail!(e),
                };
        }

        // Link last filter to sink.
        let ret = ff_sys::avfilter_link(prev_ctx, 0, sink_ctx, 0);
        if ret < 0 {
            bail!(FilterError::BuildFailed);
        }

        // 7. Configure the graph.
        let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
        if ret < 0 {
            log::warn!("avfilter_graph_config failed code={ret}");
            // If there is a crop step the most likely cause is the rectangle
            // extending beyond the source frame dimensions.
            let crop_info = steps.iter().find_map(|s| match s {
                FilterStep::Crop {
                    x,
                    y,
                    width,
                    height,
                } => Some((
                    f64::from(*x),
                    f64::from(*y),
                    f64::from(*width),
                    f64::from(*height),
                )),
                FilterStep::CropAnimated {
                    x,
                    y,
                    width,
                    height,
                } => Some((
                    x.value_at(Duration::ZERO),
                    y.value_at(Duration::ZERO),
                    width.value_at(Duration::ZERO),
                    height.value_at(Duration::ZERO),
                )),
                _ => None,
            });
            if let Some((x, y, w, h)) = crop_info {
                bail!(FilterError::InvalidConfig {
                    reason: format!("crop rect {x},{y}+{w}x{h} exceeds source frame dimensions"),
                });
            }
            bail!(ffmpeg_err(ret));
        }

        // SAFETY: `avfilter_graph_create_filter` with ret >= 0 guarantees
        // non-null pointers.
        let sink_nn = NonNull::new_unchecked(sink_ctx);
        Ok((src_ctxs, sink_nn, hw_device_ctx))
    }

    /// Build the `AVFilterGraph` for audio, returning `(src_ctxs, asink_ctx)`.
    ///
    /// # Safety
    ///
    /// `graph_nn` must be a valid, freshly-allocated `AVFilterGraph`.
    pub(super) unsafe fn build_audio_graph(
        graph_nn: NonNull<ff_sys::AVFilterGraph>,
        buffersrc_args: &str,
        num_inputs: usize,
        steps: &[FilterStep],
        _hw: Option<&HwAccel>,
    ) -> BuildResult {
        let graph = graph_nn.as_ptr();
        let mut src_ctxs = Vec::with_capacity(num_inputs);

        // 1. Create abuffer sources, one per input slot.
        let abuffer = ff_sys::avfilter_get_by_name(c"abuffer".as_ptr());
        if abuffer.is_null() {
            return Err(FilterError::BuildFailed);
        }
        let src_args =
            std::ffi::CString::new(buffersrc_args).map_err(|_| FilterError::BuildFailed)?;

        // First input slot.
        let first_src_ctx = {
            let mut raw_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut raw_ctx,
                abuffer,
                c"in0".as_ptr(),
                src_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                return Err(FilterError::BuildFailed);
            }
            log::debug!("filter added name=abuffersrc slot=0");
            // SAFETY: ret >= 0 means raw_ctx is non-null.
            let nn = NonNull::new_unchecked(raw_ctx);
            src_ctxs.push(Some(nn));
            nn
        };

        // Additional input slots for amix.
        for slot in 1..num_inputs {
            let ctx_name = std::ffi::CString::new(format!("in{slot}"))
                .map_err(|_| FilterError::BuildFailed)?;
            let mut raw_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
            let ret = ff_sys::avfilter_graph_create_filter(
                &raw mut raw_ctx,
                abuffer,
                ctx_name.as_ptr(),
                src_args.as_ptr(),
                std::ptr::null_mut(),
                graph,
            );
            if ret < 0 {
                return Err(FilterError::BuildFailed);
            }
            log::debug!("filter added name=abuffersrc slot={slot}");
            // SAFETY: ret >= 0 means raw_ctx is non-null.
            src_ctxs.push(Some(NonNull::new_unchecked(raw_ctx)));
        }

        // 2. Create abuffersink.
        let abuffersink = ff_sys::avfilter_get_by_name(c"abuffersink".as_ptr());
        if abuffersink.is_null() {
            return Err(FilterError::BuildFailed);
        }

        let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut sink_ctx,
            abuffersink,
            c"aout".as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            return Err(FilterError::BuildFailed);
        }

        // 3-5. Add each `FilterStep` (audio-relevant steps) and link.
        let mut prev_ctx = first_src_ctx.as_ptr();
        for (i, step) in steps.iter().enumerate() {
            // Video-only steps; skip them in the audio graph.
            if matches!(
                step,
                FilterStep::Reverse
                    | FilterStep::ConcatVideo { .. }
                    | FilterStep::JoinWithDissolve { .. }
                    | FilterStep::Blend { .. }
            ) {
                continue;
            }

            // LoudnessNormalize is handled via two-pass buffering in
            // push_audio / pull_audio; the regular audio graph is never built
            // when this step is present, so this guard is defensive only.
            if matches!(step, FilterStep::LoudnessNormalize { .. }) {
                continue;
            }

            // NormalizePeak is handled via two-pass buffering in
            // push_audio / pull_audio; same reasoning as LoudnessNormalize.
            if matches!(step, FilterStep::NormalizePeak { .. }) {
                continue;
            }

            // Speed uses `setpts` for video but `atempo` for audio.  Bypass the
            // standard `add_and_link_step` path and insert the atempo chain here.
            if let FilterStep::Speed { factor } = step {
                prev_ctx = add_atempo_chain(graph, prev_ctx, *factor, i)?;
                continue;
            }

            // ParametricEq generates one filter node per band; bypass the
            // single-node `add_and_link_step` path.
            if let FilterStep::ParametricEq { bands } = step {
                prev_ctx = add_parametric_eq_chain(graph, prev_ctx, bands, i)?;
                continue;
            }

            // ReverbIr — compound step: amovie[+adelay] → afir.
            // amovie is a self-contained audio source; no buffersrc slot is consumed.
            if let FilterStep::ReverbIr {
                ir_path,
                wet,
                dry,
                pre_delay_ms,
            } = step
            {
                prev_ctx =
                    add_reverb_ir_step(graph, prev_ctx, ir_path, *wet, *dry, *pre_delay_ms, i)?;
                continue;
            }

            // TimeStretch — uses the same atempo chain as the audio path of Speed,
            // but without any video setpts change.
            if let FilterStep::TimeStretch { factor } = step {
                prev_ctx = add_atempo_chain(graph, prev_ctx, f64::from(*factor), i)?;
                continue;
            }

            // SpeedChange — asetrate only (speed and pitch change together).
            // The sample rate is resolved from buffersrc_args so the integer
            // value is substituted literally into the filter args.
            if let FilterStep::SpeedChange { factor } = step {
                let sr = parse_sample_rate_from_buffersrc(buffersrc_args);
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let new_sr = (f64::from(sr) * factor).round() as u64;
                // SAFETY: graph and prev_ctx are valid pointers in the same graph.
                prev_ctx = add_raw_filter_step(
                    graph,
                    prev_ctx,
                    "asetrate",
                    &format!("r={new_sr}"),
                    i,
                    "speed_asetrate",
                )?;
                continue;
            }

            // PitchShift — compound step: asetrate → atempo.
            // asetrate changes the declared sample rate (shifting pitch); atempo
            // restores the original duration.  The actual sample rate is resolved
            // from buffersrc_args so the integer value is substituted literally.
            if let FilterStep::PitchShift { semitones } = step {
                let rate = 2f64.powf(f64::from(*semitones) / 12.0);
                let sr = parse_sample_rate_from_buffersrc(buffersrc_args);
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let new_sr = (f64::from(sr) * rate).round() as u64;
                let atempo = 1.0 / rate;
                // SAFETY: graph and prev_ctx are valid pointers in the same graph.
                prev_ctx = add_raw_filter_step(
                    graph,
                    prev_ctx,
                    "asetrate",
                    &format!("r={new_sr}"),
                    i,
                    "pitch_asetrate",
                )?;
                prev_ctx = add_raw_filter_step(
                    graph,
                    prev_ctx,
                    "atempo",
                    &format!("{atempo:.6}"),
                    i,
                    "pitch_atempo",
                )?;
                continue;
            }

            // Duck — two-input compound step: sidechaincompress.
            // Slot 0 = background (main signal, gets ducked); slot 1 = foreground (sidechain).
            if let FilterStep::Duck {
                threshold_linear,
                ratio,
                attack_ms,
                release_ms,
            } = step
            {
                let side_ctx = src_ctxs
                    .get(1)
                    .and_then(|s| *s)
                    .ok_or(FilterError::BuildFailed)?
                    .as_ptr();
                // SAFETY: graph, prev_ctx, and side_ctx are valid pointers in the same graph.
                prev_ctx = add_sidechain_compress_step(
                    graph,
                    prev_ctx,
                    side_ctx,
                    &DuckArgs {
                        threshold_linear: *threshold_linear,
                        ratio: *ratio,
                        attack_ms: *attack_ms,
                        release_ms: *release_ms,
                    },
                    i,
                )?;
                continue;
            }

            // AudioDelay dispatches to adelay (positive/zero) or atrim (negative).
            if let FilterStep::AudioDelay { ms } = step {
                let (filter_name, args) = if *ms >= 0.0 {
                    ("adelay".to_string(), format!("delays={ms}:all=1"))
                } else {
                    ("atrim".to_string(), format!("start={}", -ms / 1000.0))
                };
                // SAFETY: graph and prev_ctx are valid pointers in the same graph.
                prev_ctx = add_raw_filter_step(graph, prev_ctx, &filter_name, &args, i, "adelay")?;
                continue;
            }

            // Only audio-relevant simple steps reach the generic builder. The
            // compound audio steps (Speed, ParametricEq, ReverbIr, …) are handled
            // above with `continue`. Any step that reaches here and is NOT in this
            // allow-list is video-only (e.g. Scale, Crop, Eq); applying a video
            // filter to the audio buffersrc causes a "Media type mismatch" failure
            // at graph config time, so skip it.
            if !matches!(
                step,
                FilterStep::AFadeIn { .. }
                    | FilterStep::AFadeOut { .. }
                    | FilterStep::Volume(_)
                    | FilterStep::Amix(_)
                    | FilterStep::AReverse
                    | FilterStep::ANoiseGate { .. }
                    | FilterStep::ACompressor { .. }
                    | FilterStep::StereoToMono
                    | FilterStep::ChannelMap { .. }
                    | FilterStep::ConcatAudio { .. }
                    | FilterStep::NoiseReduce { .. }
                    | FilterStep::NoiseReduceProfile { .. }
            ) {
                log::debug!("audio graph: skipping non-audio step {step:?}");
                continue;
            }

            prev_ctx = add_and_link_step(graph, prev_ctx, step, i, "astep")?;

            // ConcatAudio consumes n input pads; link src_ctxs[1..n-1] to pads 1..n-1.
            if let FilterStep::ConcatAudio { n } = step {
                for slot in 1..*n as usize {
                    if let Some(Some(extra_src)) = src_ctxs.get(slot) {
                        let ret =
                            ff_sys::avfilter_link(extra_src.as_ptr(), 0, prev_ctx, slot as u32);
                        if ret < 0 {
                            return Err(FilterError::BuildFailed);
                        }
                        log::debug!("filter linked extra_input=in{slot} to concat pad={slot}");
                    }
                }
            }
        }

        // Link last filter to sink.
        let ret = ff_sys::avfilter_link(prev_ctx, 0, sink_ctx, 0);
        if ret < 0 {
            return Err(FilterError::BuildFailed);
        }

        // 6. Configure the graph.
        let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
        if ret < 0 {
            log::warn!("avfilter_graph_config failed code={ret}");
            return Err(ffmpeg_err(ret));
        }

        // SAFETY: sink_ctx is non-null (ret >= 0 above).
        let sink_nn = NonNull::new_unchecked(sink_ctx);
        Ok((src_ctxs, sink_nn))
    }
}

// ── ReverbIr compound step ────────────────────────────────────────────────────
/// Insert the convolution reverb compound step.
///
/// ```text
/// prev_ctx ──────────────────────────────────────────→ afir[0]
/// amovie(ir_path) [→ adelay(pre_delay_ms)] ─────────→ afir[1]
///                                           afir(dry, wet) → out
/// ```
///
/// `amovie` is a self-contained audio source — no buffersrc input slot is consumed.
///
/// # Safety
///
/// `graph` and `prev_ctx` must be valid pointers owned by the same
/// `AVFilterGraph`.
pub(super) unsafe fn add_reverb_ir_step(
    graph: *mut ff_sys::AVFilterGraph,
    prev_ctx: *mut ff_sys::AVFilterContext,
    ir_path: &str,
    wet: f32,
    dry: f32,
    pre_delay_ms: u32,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    use std::ffi::CString;

    let wet = wet.clamp(0.0, 1.0);
    let dry = dry.clamp(0.0, 1.0);
    let delay = pre_delay_ms.min(500);

    // 1. amovie=filename={ir_path} — self-contained IR source.
    let amovie_filter = ff_sys::avfilter_get_by_name(c"amovie".as_ptr());
    if amovie_filter.is_null() {
        log::warn!("filter not found name=amovie (reverb_ir)");
        return Err(FilterError::BuildFailed);
    }
    let amovie_name =
        CString::new(format!("reverb_amovie{index}")).map_err(|_| FilterError::BuildFailed)?;
    let amovie_args_str = format!("filename={ir_path}");
    let amovie_args =
        CString::new(amovie_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut amovie_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    // SAFETY: amovie_filter and graph are non-null; args are valid null-terminated C strings.
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut amovie_ctx,
        amovie_filter,
        amovie_name.as_ptr(),
        amovie_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!(
            "filter creation failed name=amovie args={amovie_args_str} (reverb_ir) code={ret}"
        );
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=amovie args={amovie_args_str} index={index} (reverb_ir)");

    // 2. [optional] adelay={delay}:all=1 — pre-delay on the IR path.
    let ir_out_ctx = if delay > 0 {
        let adelay_filter = ff_sys::avfilter_get_by_name(c"adelay".as_ptr());
        if adelay_filter.is_null() {
            log::warn!("filter not found name=adelay (reverb_ir)");
            return Err(FilterError::BuildFailed);
        }
        let adelay_name =
            CString::new(format!("reverb_adelay{index}")).map_err(|_| FilterError::BuildFailed)?;
        let adelay_args_str = format!("delays={delay}:all=1");
        let adelay_args =
            CString::new(adelay_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
        let mut adelay_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
        let ret = ff_sys::avfilter_graph_create_filter(
            &raw mut adelay_ctx,
            adelay_filter,
            adelay_name.as_ptr(),
            adelay_args.as_ptr(),
            std::ptr::null_mut(),
            graph,
        );
        if ret < 0 {
            log::warn!(
                "filter creation failed name=adelay args={adelay_args_str} (reverb_ir) code={ret}"
            );
            return Err(FilterError::BuildFailed);
        }
        log::debug!("filter added name=adelay args={adelay_args_str} index={index} (reverb_ir)");
        // SAFETY: amovie_ctx and adelay_ctx belong to the same graph; pad indices valid.
        let ret = ff_sys::avfilter_link(amovie_ctx, 0, adelay_ctx, 0);
        if ret < 0 {
            return Err(FilterError::BuildFailed);
        }
        adelay_ctx
    } else {
        amovie_ctx
    };

    // 3. afir=dry={dry}:wet={wet} — convolution reverb.
    let afir_filter = ff_sys::avfilter_get_by_name(c"afir".as_ptr());
    if afir_filter.is_null() {
        log::warn!("filter not found name=afir (reverb_ir)");
        return Err(FilterError::BuildFailed);
    }
    let afir_name =
        CString::new(format!("reverb_afir{index}")).map_err(|_| FilterError::BuildFailed)?;
    let afir_args_str = format!("dry={dry}:wet={wet}");
    let afir_args = CString::new(afir_args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;
    let mut afir_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut afir_ctx,
        afir_filter,
        afir_name.as_ptr(),
        afir_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!("filter creation failed name=afir args={afir_args_str} (reverb_ir) code={ret}");
        return Err(FilterError::BuildFailed);
    }
    log::debug!("filter added name=afir args={afir_args_str} index={index} (reverb_ir)");

    // Link: prev_ctx → afir[0] (dry audio input).
    // SAFETY: prev_ctx and afir_ctx belong to the same graph; pad indices valid.
    let ret = ff_sys::avfilter_link(prev_ctx, 0, afir_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // Link: ir_out_ctx → afir[1] (impulse response input).
    let ret = ff_sys::avfilter_link(ir_out_ctx, 0, afir_ctx, 1);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    log::debug!(
        "filter reverb_ir expanded ir_path={ir_path} wet={wet} dry={dry} pre_delay_ms={pre_delay_ms} index={index}"
    );
    Ok(afir_ctx)
}

// ── SidechainCompress compound step ──────────────────────────────────────────

/// Parameters for `add_sidechain_compress_step`.
pub(super) struct DuckArgs {
    pub threshold_linear: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
}

/// Wire the two-input `sidechaincompress` filter for audio ducking.
///
/// ```text
/// main_ctx (background, slot 0) ──→ sidechaincompress[0]
/// side_ctx (foreground, slot 1) ──→ sidechaincompress[1]
///                                   sidechaincompress → out
/// ```
///
/// # Safety
///
/// `graph`, `main_ctx`, and `side_ctx` must be valid pointers owned by the
/// same `AVFilterGraph`.
pub(super) unsafe fn add_sidechain_compress_step(
    graph: *mut ff_sys::AVFilterGraph,
    main_ctx: *mut ff_sys::AVFilterContext,
    side_ctx: *mut ff_sys::AVFilterContext,
    args: &DuckArgs,
    index: usize,
) -> Result<*mut ff_sys::AVFilterContext, FilterError> {
    use std::ffi::CString;

    let DuckArgs {
        threshold_linear,
        ratio,
        attack_ms,
        release_ms,
    } = args;

    let filter = ff_sys::avfilter_get_by_name(c"sidechaincompress".as_ptr());
    if filter.is_null() {
        log::warn!("filter not found name=sidechaincompress (duck)");
        return Err(FilterError::BuildFailed);
    }

    let name = CString::new(format!("duck{index}")).map_err(|_| FilterError::BuildFailed)?;
    let args_str = format!(
        "threshold={threshold_linear}:ratio={ratio}:attack={attack_ms}:release={release_ms}"
    );
    let cargs = CString::new(args_str.as_str()).map_err(|_| FilterError::BuildFailed)?;

    let mut sc_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    // SAFETY: filter and graph are non-null; args are valid null-terminated C strings.
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sc_ctx,
        filter,
        name.as_ptr(),
        cargs.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        log::warn!(
            "filter creation failed name=sidechaincompress args={args_str} code={ret} (duck)"
        );
        return Err(ffmpeg_err(ret));
    }
    log::debug!("filter added name=sidechaincompress args={args_str} index={index} (duck)");

    // Link main signal → input pad 0 (the stream to be compressed).
    // SAFETY: main_ctx and sc_ctx belong to the same graph; pad indices are valid.
    let ret = ff_sys::avfilter_link(main_ctx, 0, sc_ctx, 0);
    if ret < 0 {
        log::warn!("avfilter_link failed main→sidechaincompress[0] code={ret}");
        return Err(ffmpeg_err(ret));
    }

    // Link sidechain signal → input pad 1 (the trigger signal).
    // SAFETY: side_ctx and sc_ctx belong to the same graph; pad indices are valid.
    let ret = ff_sys::avfilter_link(side_ctx, 0, sc_ctx, 1);
    if ret < 0 {
        log::warn!("avfilter_link failed sidechain→sidechaincompress[1] code={ret}");
        return Err(ffmpeg_err(ret));
    }

    log::debug!(
        "filter duck expanded threshold_linear={threshold_linear} ratio={ratio} \
         attack_ms={attack_ms} release_ms={release_ms} index={index}"
    );
    Ok(sc_ctx)
}
