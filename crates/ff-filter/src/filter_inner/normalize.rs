//! Standalone loudness/peak/volume analysis filter graphs.

use super::build::audio_buffersrc_args;
use super::convert::{
    audio_pts_ticks, copy_audio_planes_to_av, sample_format_to_av, sample_format_to_av_name,
};
use super::{convert, ffmpeg_err};
use crate::error::FilterError;

/// Converts a linear amplitude to decibels relative to full scale.
///
/// `ebur128`'s `lavfi.r128.true_peak` metadata is a linear amplitude, not the
/// decibel value its name suggests, so every reader of that key needs this
/// (#1822). Digital silence maps to negative infinity rather than the `NaN` or
/// `-inf` that `log10` of a non-positive number would give.
pub(crate) fn linear_to_db(linear: f32) -> f32 {
    if linear > 0.0 {
        20.0 * linear.log10()
    } else {
        f32::NEG_INFINITY
    }
}

/// What pass 1 of the two-pass loudness step measured.
///
/// Each value comes from the last output frame of the `ebur128` graph, which
/// publishes a running maximum, so the last frame carries the whole programme.
#[derive(Debug, Clone, Copy)]
pub(super) struct Pass1Measurement {
    /// Integrated loudness in LUFS from `lavfi.r128.I`, falling back to the
    /// silence level when no metadata was published.
    pub(super) integrated_lufs: f32,
    /// True peak in dBTP from `lavfi.r128.true_peak`.
    ///
    /// `None` means the key was absent, which a caller must not confuse with a
    /// measured silence of negative infinity: one means the ceiling cannot be
    /// applied, the other that it need not be (#1822).
    pub(super) true_peak_db: Option<f32>,
    /// Loudness range in LU from `lavfi.r128.LRA`, or `None` when the key was
    /// absent, so an unchecked range is never reported as a range that fits.
    pub(super) lra: Option<f32>,
}

/// The silence level `ebur128` effectively reports for a programme too short or
/// too quiet to integrate, and what loudness falls back to with no metadata.
const SILENCE_LUFS: f32 = -70.0;

/// Reads one `f32`-valued metadata key from a frame.
///
/// `ebur128` writes its statistics as decimal strings, one key per statistic, so
/// every reader is this same three-step dance; three copies of it would be three
/// `unsafe` blocks to audit instead of one.
///
/// # Safety
///
/// `frame` must be a valid `AVFrame` pointer. Its `metadata` may be null;
/// `av_dict_get` handles that by returning null. The entry's `value` is checked
/// too: `av_dict_set` compares against a null `tag->value` (`dict.c:114`), so the
/// library treats a valueless entry as a state that exists, and reading one as a
/// `CStr` would be undefined.
unsafe fn read_f32_meta(frame: *const ff_sys::AVFrame, key: &std::ffi::CStr) -> Option<f32> {
    let entry = ff_sys::av_dict_get((*frame).metadata, key.as_ptr(), std::ptr::null(), 0);
    if entry.is_null() || (*entry).value.is_null() {
        return None;
    }
    std::ffi::CStr::from_ptr((*entry).value)
        .to_str()
        .ok()?
        .parse::<f32>()
        .ok()
}

/// Build a temporary `abuffer → ebur128=peak=true:metadata=1 → abuffersink` graph,
/// feed all `frames` through it, drain the output, and return what it measured.
///
/// # Safety
///
/// `graph` must be a valid, freshly-allocated `AVFilterGraph`.  The caller is
/// responsible for freeing it with `avfilter_graph_free` after this call returns
/// (whether `Ok` or `Err`).
pub(super) unsafe fn run_ebur128_graph(
    graph: *mut ff_sys::AVFilterGraph,
    frames: &[ff_format::AudioFrame],
) -> Result<Pass1Measurement, FilterError> {
    let first = &frames[0];
    let src_args_str = audio_buffersrc_args(
        first.sample_rate(),
        sample_format_to_av_name(first.format()),
        first.channels(),
    );
    let src_args = std::ffi::CString::new(src_args_str).map_err(|_| FilterError::BuildFailed)?;

    // 1. abuffersrc
    let abuffer = ff_sys::avfilter_get_by_name(c"abuffer".as_ptr());
    if abuffer.is_null() {
        return Err(FilterError::BuildFailed);
    }
    let mut src_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut src_ctx,
        abuffer,
        c"meas_in".as_ptr(),
        src_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 2. ebur128=peak=true:metadata=1
    let ebur128_filt = ff_sys::avfilter_get_by_name(c"ebur128".as_ptr());
    if ebur128_filt.is_null() {
        return Err(FilterError::BuildFailed);
    }
    let mut meas_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut meas_ctx,
        ebur128_filt,
        c"meas_ebur128".as_ptr(),
        c"peak=true:metadata=1".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 3. abuffersink
    let abuffersink = ff_sys::avfilter_get_by_name(c"abuffersink".as_ptr());
    if abuffersink.is_null() {
        return Err(FilterError::BuildFailed);
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        abuffersink,
        c"meas_out".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // Link: src → ebur128 → sink
    let ret = ff_sys::avfilter_link(src_ctx, 0, meas_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    let ret = ff_sys::avfilter_link(meas_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // Configure
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        return Err(ffmpeg_err(ret));
    }

    // Feed all frames
    for frame in frames {
        let raw_frame = ff_sys::av_frame_alloc();
        if raw_frame.is_null() {
            return Err(FilterError::ProcessFailed);
        }
        (*raw_frame).nb_samples = frame.samples() as std::os::raw::c_int;
        (*raw_frame).sample_rate = frame.sample_rate() as std::os::raw::c_int;
        (*raw_frame).format = sample_format_to_av(frame.format());
        (*raw_frame).pts = audio_pts_ticks(frame.timestamp(), frame.sample_rate());
        (*raw_frame).ch_layout.nb_channels = frame.channels() as std::os::raw::c_int;
        let ret = ff_sys::av_frame_get_buffer(raw_frame, 0);
        if ret < 0 {
            let mut ptr = raw_frame;
            ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
            return Err(FilterError::ProcessFailed);
        }
        copy_audio_planes_to_av(frame, raw_frame);
        let ret = ff_sys::av_buffersrc_add_frame_flags(
            src_ctx,
            raw_frame,
            ff_sys::BUFFERSRC_FLAG_KEEP_REF,
        );
        let mut ptr = raw_frame;
        ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
        if ret < 0 {
            return Err(FilterError::ProcessFailed);
        }
    }

    // Signal EOF so the filter flushes all pending frames.
    ff_sys::av_buffersrc_close(src_ctx, ff_sys::AV_NOPTS_VALUE, 0u32);

    // Drain all output, keeping the last value published for each statistic.
    let mut measurement = Pass1Measurement {
        integrated_lufs: SILENCE_LUFS,
        true_peak_db: None,
        lra: None,
    };
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
        // SAFETY: `raw_frame` is a frame `av_buffersink_get_frame` just filled, and
        // `read_f32_meta` tolerates a null `metadata`.
        if let Some(v) = read_f32_meta(raw_frame, c"lavfi.r128.I") {
            measurement.integrated_lufs = v;
        }
        // SAFETY: same frame, same contract as the read above.
        if let Some(v) = read_f32_meta(raw_frame, c"lavfi.r128.true_peak") {
            // The metadata is a linear amplitude despite the key's name.
            measurement.true_peak_db = Some(linear_to_db(v));
        }
        // SAFETY: same frame, same contract as the read above.
        if let Some(v) = read_f32_meta(raw_frame, c"lavfi.r128.LRA") {
            measurement.lra = Some(v);
        }
        let mut ptr = raw_frame;
        ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
    }

    Ok(measurement)
}

/// What the measured loudness range says about the requested one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LraOutcome {
    /// The programme already fits the requested range, so a single gain is a
    /// legitimate way to reach the loudness target.
    Within,
    /// The programme's range is wider than requested. No single gain narrows a
    /// range, so the request cannot be met by this construction.
    Exceeds,
    /// Pass 1 published no range, so nothing can be said about the request.
    NotMeasured,
}

/// Compare a measured loudness range against the requested one.
///
/// This is the condition `loudnorm` itself uses to decide whether a single
/// gain is allowed: `measured_lra <= target_lra` at `af_loudnorm.c:812` of the
/// pinned n8.0.1 source, alongside the true-peak test. The construction here is
/// that linear mode, and it used to apply the gain without ever asking (#1854).
pub(super) fn lra_outcome(measured: Option<f32>, target: f32) -> LraOutcome {
    match measured {
        Some(lra) if lra <= target => LraOutcome::Within,
        Some(_) => LraOutcome::Exceeds,
        None => LraOutcome::NotMeasured,
    }
}

/// Build a temporary `abuffer → volume={gain_db}dB → abuffersink` graph,
/// feed all `frames` through it, drain the output, and return the corrected frames.
///
/// # Safety
///
/// `graph` must be a valid, freshly-allocated `AVFilterGraph`.  The caller is
/// responsible for freeing it with `avfilter_graph_free` after this call returns
/// (whether `Ok` or `Err`).
pub(super) unsafe fn run_volume_graph(
    graph: *mut ff_sys::AVFilterGraph,
    frames: &[ff_format::AudioFrame],
    gain_db: f32,
) -> Result<Vec<ff_format::AudioFrame>, FilterError> {
    let first = &frames[0];
    let src_args_str = audio_buffersrc_args(
        first.sample_rate(),
        sample_format_to_av_name(first.format()),
        first.channels(),
    );
    let src_args = std::ffi::CString::new(src_args_str).map_err(|_| FilterError::BuildFailed)?;

    // 1. abuffersrc
    let abuffer = ff_sys::avfilter_get_by_name(c"abuffer".as_ptr());
    if abuffer.is_null() {
        return Err(FilterError::BuildFailed);
    }
    let mut src_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut src_ctx,
        abuffer,
        c"vol_in".as_ptr(),
        src_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 2. volume={gain_db}dB
    let volume_filt = ff_sys::avfilter_get_by_name(c"volume".as_ptr());
    if volume_filt.is_null() {
        return Err(FilterError::BuildFailed);
    }
    let vol_args =
        std::ffi::CString::new(format!("{gain_db:.4}dB")).map_err(|_| FilterError::BuildFailed)?;
    let mut vol_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut vol_ctx,
        volume_filt,
        c"vol_volume".as_ptr(),
        vol_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 3. abuffersink
    let abuffersink = ff_sys::avfilter_get_by_name(c"abuffersink".as_ptr());
    if abuffersink.is_null() {
        return Err(FilterError::BuildFailed);
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        abuffersink,
        c"vol_out".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // Link: src → volume → sink
    let ret = ff_sys::avfilter_link(src_ctx, 0, vol_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    let ret = ff_sys::avfilter_link(vol_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // Configure
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        return Err(ffmpeg_err(ret));
    }

    // Feed all frames
    for frame in frames {
        let raw_frame = ff_sys::av_frame_alloc();
        if raw_frame.is_null() {
            return Err(FilterError::ProcessFailed);
        }
        (*raw_frame).nb_samples = frame.samples() as std::os::raw::c_int;
        (*raw_frame).sample_rate = frame.sample_rate() as std::os::raw::c_int;
        (*raw_frame).format = sample_format_to_av(frame.format());
        (*raw_frame).pts = audio_pts_ticks(frame.timestamp(), frame.sample_rate());
        (*raw_frame).ch_layout.nb_channels = frame.channels() as std::os::raw::c_int;
        let ret = ff_sys::av_frame_get_buffer(raw_frame, 0);
        if ret < 0 {
            let mut ptr = raw_frame;
            ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
            return Err(FilterError::ProcessFailed);
        }
        copy_audio_planes_to_av(frame, raw_frame);
        let ret = ff_sys::av_buffersrc_add_frame_flags(
            src_ctx,
            raw_frame,
            ff_sys::BUFFERSRC_FLAG_KEEP_REF,
        );
        let mut ptr = raw_frame;
        ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
        if ret < 0 {
            return Err(FilterError::ProcessFailed);
        }
    }

    // Signal EOF
    ff_sys::av_buffersrc_close(src_ctx, ff_sys::AV_NOPTS_VALUE, 0u32);

    // Drain all corrected output frames. The owned frame frees itself each
    // iteration; NeedMore / Drained / Err all end the drain.
    let mut output = Vec::new();
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
        if let Ok(af) = convert::av_frame_to_audio_frame(&frame) {
            output.push(af);
        }
    }

    Ok(output)
}

// Peak normalization helper

/// Build a temporary `abuffer → astats=metadata=1 → abuffersink` graph,
/// feed all `frames` through it, drain the output, and return the maximum
/// peak level (dBFS) read from `lavfi.astats.Overall.Peak_level` across all
/// output frames.
///
/// Falls back to `−70.0` (silence level) if no metadata is found.
///
/// # Safety
///
/// `graph` must be a valid, freshly-allocated `AVFilterGraph`.  The caller is
/// responsible for freeing it with `avfilter_graph_free` after this call returns
/// (whether `Ok` or `Err`).
pub(super) unsafe fn run_astats_graph(
    graph: *mut ff_sys::AVFilterGraph,
    frames: &[ff_format::AudioFrame],
) -> Result<f32, FilterError> {
    let first = &frames[0];
    let src_args_str = audio_buffersrc_args(
        first.sample_rate(),
        sample_format_to_av_name(first.format()),
        first.channels(),
    );
    let src_args = std::ffi::CString::new(src_args_str).map_err(|_| FilterError::BuildFailed)?;

    // 1. abuffersrc
    let abuffer = ff_sys::avfilter_get_by_name(c"abuffer".as_ptr());
    if abuffer.is_null() {
        return Err(FilterError::BuildFailed);
    }
    let mut src_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut src_ctx,
        abuffer,
        c"peak_in".as_ptr(),
        src_args.as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 2. astats=metadata=1
    let astats_filt = ff_sys::avfilter_get_by_name(c"astats".as_ptr());
    if astats_filt.is_null() {
        return Err(FilterError::BuildFailed);
    }
    let mut meas_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut meas_ctx,
        astats_filt,
        c"peak_astats".as_ptr(),
        c"metadata=1".as_ptr(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // 3. abuffersink
    let abuffersink = ff_sys::avfilter_get_by_name(c"abuffersink".as_ptr());
    if abuffersink.is_null() {
        return Err(FilterError::BuildFailed);
    }
    let mut sink_ctx: *mut ff_sys::AVFilterContext = std::ptr::null_mut();
    let ret = ff_sys::avfilter_graph_create_filter(
        &raw mut sink_ctx,
        abuffersink,
        c"peak_out".as_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        graph,
    );
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // Link: src → astats → sink
    let ret = ff_sys::avfilter_link(src_ctx, 0, meas_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }
    let ret = ff_sys::avfilter_link(meas_ctx, 0, sink_ctx, 0);
    if ret < 0 {
        return Err(FilterError::BuildFailed);
    }

    // Configure
    let ret = ff_sys::avfilter_graph_config(graph, std::ptr::null_mut());
    if ret < 0 {
        return Err(ffmpeg_err(ret));
    }

    // Feed all frames
    for frame in frames {
        let raw_frame = ff_sys::av_frame_alloc();
        if raw_frame.is_null() {
            return Err(FilterError::ProcessFailed);
        }
        (*raw_frame).nb_samples = frame.samples() as std::os::raw::c_int;
        (*raw_frame).sample_rate = frame.sample_rate() as std::os::raw::c_int;
        (*raw_frame).format = sample_format_to_av(frame.format());
        (*raw_frame).pts = audio_pts_ticks(frame.timestamp(), frame.sample_rate());
        (*raw_frame).ch_layout.nb_channels = frame.channels() as std::os::raw::c_int;
        let ret = ff_sys::av_frame_get_buffer(raw_frame, 0);
        if ret < 0 {
            let mut ptr = raw_frame;
            ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
            return Err(FilterError::ProcessFailed);
        }
        copy_audio_planes_to_av(frame, raw_frame);
        let ret = ff_sys::av_buffersrc_add_frame_flags(
            src_ctx,
            raw_frame,
            ff_sys::BUFFERSRC_FLAG_KEEP_REF,
        );
        let mut ptr = raw_frame;
        ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
        if ret < 0 {
            return Err(FilterError::ProcessFailed);
        }
    }

    // Signal EOF so the filter flushes all pending frames.
    ff_sys::av_buffersrc_close(src_ctx, ff_sys::AV_NOPTS_VALUE, 0u32);

    // Drain all output; read `lavfi.astats.Overall.Peak_level` from each frame.
    // `astats` with no reset accumulates across the whole clip, so the last
    // frame's metadata holds the overall peak.  We track the maximum across all
    // frames as a safety net in case the filter resets per frame.
    let mut max_peak_db: f32 = -70.0;
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
        let entry = ff_sys::av_dict_get(
            (*raw_frame).metadata,
            c"lavfi.astats.Overall.Peak_level".as_ptr(),
            std::ptr::null(),
            0,
        );
        if !entry.is_null()
            && let Ok(s) = std::ffi::CStr::from_ptr((*entry).value).to_str()
            && let Ok(v) = s.parse::<f32>()
        {
            max_peak_db = max_peak_db.max(v);
        }
        let mut ptr = raw_frame;
        ff_sys::av_frame_free(std::ptr::addr_of_mut!(ptr));
    }

    Ok(max_peak_db)
}

#[cfg(test)]
mod tests {
    use super::{LraOutcome, linear_to_db, lra_outcome};

    #[test]
    fn lra_outcome_should_accept_a_range_within_the_target() {
        assert_eq!(lra_outcome(Some(4.2), 7.0), LraOutcome::Within);
    }

    #[test]
    fn lra_outcome_should_reject_a_range_wider_than_the_target() {
        assert_eq!(lra_outcome(Some(11.5), 7.0), LraOutcome::Exceeds);
    }

    #[test]
    fn lra_outcome_should_report_a_range_it_could_not_measure() {
        assert_eq!(lra_outcome(None, 7.0), LraOutcome::NotMeasured);
    }

    /// `af_loudnorm.c:812` tests `<=`, so a programme sitting exactly on the
    /// requested range is inside it.
    #[test]
    fn lra_outcome_should_treat_an_exactly_equal_range_as_within() {
        assert_eq!(lra_outcome(Some(7.0), 7.0), LraOutcome::Within);
    }

    /// The conversion the true-peak ceiling depends on. `ebur128` reports a linear
    /// amplitude under a key named `true_peak`, so every decibel comparison in
    /// this module rests on getting this right (#1822).
    #[test]
    fn linear_to_db_should_convert_a_known_amplitude() {
        // Half scale is -6.02 dBFS; a quarter is -12.04.
        assert!(
            (linear_to_db(0.5) + 6.0206).abs() < 0.001,
            "0.5 should be about -6.02 dB, got {}",
            linear_to_db(0.5)
        );
        assert!(
            (linear_to_db(0.25) + 12.0412).abs() < 0.001,
            "0.25 should be about -12.04 dB, got {}",
            linear_to_db(0.25)
        );
        assert!(
            linear_to_db(1.0).abs() < 0.001,
            "full scale should be 0 dB, got {}",
            linear_to_db(1.0)
        );
    }

    /// Digital silence must not produce `NaN`, which would make every comparison
    /// against it false and let the ceiling pass silently.
    #[test]
    fn linear_to_db_should_map_silence_to_negative_infinity() {
        assert_eq!(linear_to_db(0.0), f32::NEG_INFINITY);
        assert_eq!(linear_to_db(-0.0), f32::NEG_INFINITY);
        assert_eq!(
            linear_to_db(-1.0),
            f32::NEG_INFINITY,
            "a negative amplitude is not meaningful here and must not yield NaN"
        );
    }
}
