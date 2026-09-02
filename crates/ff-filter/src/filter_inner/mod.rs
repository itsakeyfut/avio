//! Low-level filter graph wrapper ([`FilterGraphInner`]).

// All FFmpeg FFI lives here; allow unsafe in this module.
#![allow(unsafe_code)]
// Rust 2024: unsafe ops inside unsafe fn still need explicit unsafe blocks.
// We suppress this here because all helpers are private and their callers
// document the invariants.
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::too_many_lines)]

mod build;
mod convert;
mod normalize;
mod push_pull;

pub(crate) use build::add_and_link_step;
pub(crate) use build::add_asetrate_resample_chain;
pub(crate) use build::{
    MIN_INPUT_FRAME_RATE, NormalBlendParams, add_blend_normal_step, add_blend_photographic_step,
    add_composite_step, video_buffersrc_args,
};
pub(crate) use convert::pixel_format_to_av;

use std::ptr::NonNull;

use ff_format::AudioFrame;

use crate::error::FilterError;
use crate::graph::{FilterStep, HwAccel};

// Time base used for the buffersrc

/// The time base numerator used for the video buffersrc (1/90000).
pub(super) const VIDEO_TIME_BASE_NUM: i32 = 1;
/// The time base denominator used for the video buffersrc (1/90000).
pub(super) const VIDEO_TIME_BASE_DEN: i32 = 90_000;

/// The time base numerator used for the audio abuffersrc (`1/sample_rate`).
pub(super) const AUDIO_TIME_BASE_NUM: i32 = 1;

// Type aliases for complex return types

type FilterCtxVec = Vec<Option<NonNull<ff_sys::AVFilterContext>>>;
type BuildResult = Result<(FilterCtxVec, NonNull<ff_sys::AVFilterContext>), FilterError>;
/// Return type for `build_video_graph`: src contexts, sink, and an optional
/// hardware device context that must be stored and freed by the caller.
type VideoGraphResult = Result<
    (
        FilterCtxVec,
        NonNull<ff_sys::AVFilterContext>,
        Option<*mut ff_sys::AVBufferRef>,
    ),
    FilterError,
>;

// FFmpeg error helper

/// Convert a negative `FFmpeg` return code into a [`FilterError::Ffmpeg`].
pub(super) fn ffmpeg_err(code: i32) -> FilterError {
    FilterError::Ffmpeg {
        code,
        message: ff_sys::av_error_string(code),
    }
}

// Build-time validation

/// Best-effort check that every [`FilterStep`]'s libavfilter name is known to
/// the linked `FFmpeg` build.
///
/// On platforms where the filter registry is already populated at [`build()`]
/// time (Windows, macOS), this surfaces unknown-filter errors early.
///
/// On some Linux `FFmpeg` builds the registry is not yet populated when
/// `build()` runs (before any `AVFilterGraph` has been allocated).  In that
/// case `avfilter_get_by_name` returns `null` for *every* filter, including
/// standard ones like `scale`.  We cannot distinguish "filter genuinely
/// missing" from "registry not yet initialised", so we log at `debug` level
/// and return `Ok(())`, deferring the authoritative check to
/// `add_and_link_step` at graph-construction time.
pub(crate) fn validate_filter_steps(steps: &[FilterStep]) -> Result<(), FilterError> {
    for step in steps {
        let name =
            std::ffi::CString::new(step.filter_name()).map_err(|_| FilterError::BuildFailed)?;
        // SAFETY: `avfilter_get_by_name` reads a valid, null-terminated C
        // string and returns a borrowed pointer valid for the process lifetime
        // (or null if the filter is not registered / registry not yet ready).
        // The pointer is never dereferenced here.
        let filter = unsafe { ff_sys::avfilter_get_by_name(name.as_ptr()) };
        if filter.is_null() {
            // Could be "filter genuinely absent" OR "registry not yet
            // initialised" (observed on some Linux FFmpeg installations).
            // Treat as unverifiable and defer to add_and_link_step.
            log::warn!(
                "filter lookup returned null at build time name={}, \
                 registry may not be initialised; will be rechecked at push time",
                step.filter_name()
            );
            return Ok(());
        }
    }
    Ok(())
}

// FilterGraphInner

/// Low-level filter graph wrapper.
///
/// All fields start as `None`; they are populated lazily on the first
/// `push_video` / `push_audio` call.
pub(crate) struct FilterGraphInner {
    /// The primary `AVFilterGraph` (`None` until the first push call). Holds the
    /// video graph when both video and audio are used, or the audio graph for an
    /// audio-only pipeline.
    graph: Option<NonNull<ff_sys::AVFilterGraph>>,
    /// Secondary `AVFilterGraph` holding the audio chain when a video graph already
    /// occupies [`graph`](Self::graph). Video and audio are built into separate
    /// `AVFilterGraph` allocations; both must be kept alive (and freed in `Drop`).
    audio_graph: Option<NonNull<ff_sys::AVFilterGraph>>,
    /// One `AVFilterContext` per input slot (video or audio buffersrc).
    src_ctxs: Vec<Option<NonNull<ff_sys::AVFilterContext>>>,
    /// Sink context for video output.
    vsink_ctx: Option<NonNull<ff_sys::AVFilterContext>>,
    /// Sink context for audio output.
    asink_ctx: Option<NonNull<ff_sys::AVFilterContext>>,
    /// The ordered list of filter steps to apply.
    steps: Vec<FilterStep>,
    /// Optional hardware acceleration backend.
    hw: Option<HwAccel>,
    /// Owned reference to the hardware device context (`None` when no hardware
    /// acceleration is in use).  Freed in `Drop` after the graph is freed.
    hw_device_ctx: Option<*mut ff_sys::AVBufferRef>,
    /// Buffered raw audio frames for EBU R128 two-pass loudness normalization.
    /// Populated during `push_audio` when a `LoudnessNormalize` step is present.
    loudness_buf: Vec<AudioFrame>,
    /// Corrected audio frames ready to be returned from `pull_audio` (pass-2 output).
    loudness_output: Vec<AudioFrame>,
    /// Index of the next frame to return from `loudness_output`.
    loudness_output_idx: usize,
    /// True once the two-pass measurement + correction has been executed.
    loudness_pass2_done: bool,
    /// Buffered raw audio frames for peak-level two-pass normalization.
    /// Populated during `push_audio` when a `NormalizePeak` step is present.
    peak_buf: Vec<AudioFrame>,
    /// Corrected audio frames ready to be returned from `pull_audio` (pass-2 output).
    peak_output: Vec<AudioFrame>,
    /// Index of the next frame to return from `peak_output`.
    peak_output_idx: usize,
    /// True once the two-pass peak measurement + correction has been executed.
    peak_pass2_done: bool,
    /// Frame rate declared on the video buffersrc, or `None` to leave it unset.
    /// Required by filters that demand a constant frame rate (`xfade`).
    input_frame_rate: Option<f64>,
}

// SAFETY: `FilterGraphInner` owns all raw pointers exclusively.  No other
// thread holds references to the underlying `AVFilterGraph`, any of its
// contexts, or the hardware device context while this struct is alive.
unsafe impl Send for FilterGraphInner {}

impl FilterGraphInner {
    /// Create a new (uninitialised) inner.  No `FFmpeg` calls are made here.
    pub(crate) fn new(
        steps: Vec<FilterStep>,
        hw: Option<HwAccel>,
        input_frame_rate: Option<f64>,
    ) -> Self {
        Self {
            graph: None,
            audio_graph: None,
            src_ctxs: Vec::new(),
            vsink_ctx: None,
            asink_ctx: None,
            steps,
            hw,
            hw_device_ctx: None,
            loudness_buf: Vec::new(),
            loudness_output: Vec::new(),
            loudness_output_idx: 0,
            loudness_pass2_done: false,
            peak_buf: Vec::new(),
            peak_output: Vec::new(),
            peak_output_idx: 0,
            peak_pass2_done: false,
            input_frame_rate,
        }
    }

    /// Append a filter step to the pending chain.
    ///
    /// Must be called before the first [`push_video`] or [`push_audio`] — the
    /// `FFmpeg` graph is constructed lazily on the first push, so steps added
    /// before that point are included. Steps added after graph initialisation
    /// have no effect.
    pub(crate) fn push_step(&mut self, step: FilterStep) {
        self.steps.push(step);
    }

    /// The pending filter steps (test-only accessor for asserting what a builder
    /// method pushed).
    #[cfg(test)]
    pub(crate) fn steps(&self) -> &[FilterStep] {
        &self.steps
    }

    /// Creates a pre-initialised inner for a source-only video composition graph.
    ///
    /// `graph` and `vsink_ctx` are owned by the returned struct and freed on drop
    /// via the existing `Drop` impl.  No `buffersrc` is needed — all input comes
    /// from self-contained filter sources (`color`, `movie`, etc.).
    pub(crate) fn with_prebuilt_video_graph(
        graph: NonNull<ff_sys::AVFilterGraph>,
        vsink_ctx: NonNull<ff_sys::AVFilterContext>,
    ) -> Self {
        Self {
            graph: Some(graph),
            audio_graph: None,
            src_ctxs: Vec::new(),
            vsink_ctx: Some(vsink_ctx),
            asink_ctx: None,
            steps: Vec::new(),
            hw: None,
            hw_device_ctx: None,
            loudness_buf: Vec::new(),
            loudness_output: Vec::new(),
            loudness_output_idx: 0,
            loudness_pass2_done: false,
            peak_buf: Vec::new(),
            peak_output: Vec::new(),
            peak_output_idx: 0,
            peak_pass2_done: false,
            // Pre-built graphs never format buffersrc args, so the rate is moot here.
            input_frame_rate: None,
        }
    }

    /// Creates a pre-initialised inner for a video graph that has external
    /// `buffersrc` inputs (one per `src_ctxs` slot) as well as a configured sink.
    ///
    /// Unlike [`with_prebuilt_video_graph`](Self::with_prebuilt_video_graph),
    /// callers feed frames via [`push_video`] into the supplied buffersrc
    /// contexts and pull composited frames from `vsink_ctx`. Used by
    /// [`RealtimeComposer`](crate::RealtimeComposer). All pointers are owned by
    /// the returned struct and freed on drop.
    pub(crate) fn with_prebuilt_video_inputs(
        graph: NonNull<ff_sys::AVFilterGraph>,
        src_ctxs: Vec<Option<NonNull<ff_sys::AVFilterContext>>>,
        vsink_ctx: NonNull<ff_sys::AVFilterContext>,
    ) -> Self {
        Self {
            graph: Some(graph),
            audio_graph: None,
            src_ctxs,
            vsink_ctx: Some(vsink_ctx),
            asink_ctx: None,
            steps: Vec::new(),
            hw: None,
            hw_device_ctx: None,
            loudness_buf: Vec::new(),
            loudness_output: Vec::new(),
            loudness_output_idx: 0,
            loudness_pass2_done: false,
            peak_buf: Vec::new(),
            peak_output: Vec::new(),
            peak_output_idx: 0,
            peak_pass2_done: false,
            // Pre-built graphs never format buffersrc args, so the rate is moot here.
            input_frame_rate: None,
        }
    }

    /// Creates a pre-initialised inner for a source-only audio mix graph.
    ///
    /// `graph` and `asink_ctx` are owned by the returned struct and freed on drop
    /// via the existing `Drop` impl.  No `abuffersrc` is needed — all input comes
    /// from self-contained filter sources (`amovie`, etc.).
    pub(crate) fn with_prebuilt_audio_graph(
        graph: NonNull<ff_sys::AVFilterGraph>,
        asink_ctx: NonNull<ff_sys::AVFilterContext>,
    ) -> Self {
        Self {
            graph: Some(graph),
            audio_graph: None,
            src_ctxs: Vec::new(),
            vsink_ctx: None,
            asink_ctx: Some(asink_ctx),
            steps: Vec::new(),
            hw: None,
            hw_device_ctx: None,
            loudness_buf: Vec::new(),
            loudness_output: Vec::new(),
            loudness_output_idx: 0,
            loudness_pass2_done: false,
            peak_buf: Vec::new(),
            peak_output: Vec::new(),
            peak_output_idx: 0,
            peak_pass2_done: false,
            // Pre-built graphs never format buffersrc args, so the rate is moot here.
            input_frame_rate: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build::{audio_buffersrc_args, video_buffersrc_args};
    use super::*;
    use crate::graph::FilterStep;

    /// `FilterGraphInner::new` must not call `avfilter_graph_alloc`.
    /// The graph field starts as `None`; allocation is deferred to the first push.
    #[test]
    fn new_should_start_with_no_graph_allocated() {
        let inner = FilterGraphInner::new(
            vec![FilterStep::Scale {
                width: 1280,
                height: 720,
                algorithm: crate::graph::ScaleAlgorithm::Fast,
            }],
            None,
            None,
        );
        assert!(
            inner.graph.is_none(),
            "avfilter_graph_alloc must not be called at construction time"
        );
        assert!(
            inner.src_ctxs.is_empty(),
            "src_ctxs should be empty before first push"
        );
        assert!(
            inner.vsink_ctx.is_none(),
            "vsink_ctx should be None before first push"
        );
        assert!(
            inner.asink_ctx.is_none(),
            "asink_ctx should be None before first push"
        );
    }

    /// Dropping an uninitialised `FilterGraphInner` (graph == None) must be a
    /// no-op — no `avfilter_graph_free` call and no panic.
    #[test]
    fn drop_uninitialised_should_be_a_no_op() {
        let inner = FilterGraphInner::new(
            vec![FilterStep::Scale {
                width: 640,
                height: 360,
                algorithm: crate::graph::ScaleAlgorithm::Fast,
            }],
            None,
            None,
        );
        drop(inner); // must not panic or double-free
    }

    /// `FilterGraphInner` must implement `Send` so the filter graph can be
    /// moved across threads (e.g. into a worker thread for processing).
    #[test]
    fn filter_graph_inner_should_impl_send() {
        fn assert_send<T: Send>() {}
        assert_send::<FilterGraphInner>();
    }

    // buffersrc / buffersink arg-string helpers

    /// The video buffersrc args string must contain all fields required by the
    /// libavfilter `buffer` filter: `video_size`, `pix_fmt`, `time_base`, and
    /// `pixel_aspect`.
    #[test]
    fn video_buffersrc_args_should_contain_size_pix_fmt_and_time_base() {
        // pix_fmt 0 = AV_PIX_FMT_YUV420P
        let args = video_buffersrc_args(1920, 1080, 0, None);
        assert!(
            args.contains("video_size=1920x1080"),
            "missing video_size: {args}"
        );
        assert!(args.contains("pix_fmt=0"), "missing pix_fmt: {args}");
        assert!(
            args.contains("time_base=1/90000"),
            "missing time_base: {args}"
        );
        assert!(
            args.contains("pixel_aspect=1/1"),
            "missing pixel_aspect: {args}"
        );
    }

    #[test]
    fn video_buffersrc_args_should_omit_frame_rate_when_unset() {
        // Leaving the rate out is the shape every non-CFR filter has always seen; only
        // `xfade` and friends need it, so an undeclared graph must not change.
        let args = video_buffersrc_args(1920, 1080, 0, None);
        assert!(
            !args.contains("frame_rate"),
            "an undeclared rate must emit no frame_rate: {args}"
        );
    }

    #[test]
    fn video_buffersrc_args_should_emit_a_reduced_rational_frame_rate() {
        // A rational, not a decimal: libavfilter would have to parse `29.97` through
        // `av_parse_video_rate`, and this pins the emitted token without depending on
        // that. The reduction keeps the common case readable (`30/1`, not `30000/1000`).
        for (fps, want) in [
            (30.0, "frame_rate=30/1"),
            (60.0, "frame_rate=60/1"),
            (25.0, "frame_rate=25/1"),
            (29.97, "frame_rate=2997/100"),
            (23.976, "frame_rate=2997/125"),
        ] {
            let args = video_buffersrc_args(64, 64, 0, Some(fps));
            assert!(args.contains(want), "fps {fps} should emit {want}: {args}");
        }
    }

    /// The audio buffersrc args string must contain all fields required by the
    /// libavfilter `abuffer` filter: `sample_rate`, `sample_fmt`, `channels`,
    /// and `time_base` (which uses `1/sample_rate`).
    #[test]
    fn audio_buffersrc_args_should_contain_sample_rate_format_and_channels() {
        let args = audio_buffersrc_args(44100, "fltp", 2);
        assert!(
            args.contains("sample_rate=44100"),
            "missing sample_rate: {args}"
        );
        assert!(
            args.contains("sample_fmt=fltp"),
            "missing sample_fmt: {args}"
        );
        assert!(args.contains("channels=2"), "missing channels: {args}");
        assert!(
            args.contains("time_base=1/44100"),
            "missing time_base: {args}"
        );
    }

    /// Changing the sample rate must update the `time_base` denominator too,
    /// since audio time base is `1/sample_rate`.
    #[test]
    fn audio_buffersrc_args_time_base_should_match_sample_rate() {
        let args = audio_buffersrc_args(48000, "s16", 1);
        assert!(
            args.contains("time_base=1/48000"),
            "time_base denominator must equal sample_rate: {args}"
        );
    }

    // video_input_count

    /// Single-input steps (no overlay) require exactly one buffersrc.
    #[test]
    fn video_input_count_should_return_1_for_single_input_steps() {
        let inner = FilterGraphInner::new(
            vec![FilterStep::Scale {
                width: 1280,
                height: 720,
                algorithm: crate::graph::ScaleAlgorithm::Fast,
            }],
            None,
            None,
        );
        assert_eq!(inner.video_input_count(), 1);
    }

    /// Overlay requires two buffersrc contexts (main + secondary).
    #[test]
    fn video_input_count_should_return_2_for_overlay() {
        let inner = FilterGraphInner::new(vec![FilterStep::Overlay { x: 10, y: 10 }], None, None);
        assert_eq!(inner.video_input_count(), 2);
    }

    /// A chain without overlay must still report 1, even with multiple steps.
    #[test]
    fn video_input_count_should_return_1_with_no_overlay_in_chain() {
        let inner = FilterGraphInner::new(
            vec![
                FilterStep::Trim {
                    start: Some(0.0),
                    end: Some(5.0),
                },
                FilterStep::Scale {
                    width: 640,
                    height: 360,
                    algorithm: crate::graph::ScaleAlgorithm::Fast,
                },
            ],
            None,
            None,
        );
        assert_eq!(inner.video_input_count(), 1);
    }

    // ffmpeg_err helper

    /// `ffmpeg_err` must return the `Ffmpeg` variant carrying the original code.
    #[test]
    fn ffmpeg_err_should_return_ffmpeg_variant_with_code() {
        let err = ffmpeg_err(-22);
        assert!(
            matches!(err, FilterError::Ffmpeg { code: -22, .. }),
            "expected Ffmpeg variant with code -22, got {err:?}"
        );
    }

    /// `ffmpeg_err` must populate a non-empty message string for a known error code.
    #[test]
    fn ffmpeg_err_should_populate_non_empty_message() {
        let err = ffmpeg_err(-22);
        if let FilterError::Ffmpeg { message, .. } = err {
            assert!(
                !message.is_empty(),
                "message must not be empty for a known error code"
            );
        } else {
            panic!("expected Ffmpeg variant");
        }
    }

    // apply_animations

    /// `apply_animations` must be a no-op when the graph has not yet been
    /// initialised (i.e. `graph == None`).  It must not panic or access any
    /// FFmpeg API.
    #[test]
    fn apply_animations_with_no_graph_should_be_a_no_op() {
        use crate::animation::{AnimationEntry, AnimationTrack, Easing, Keyframe};
        use std::time::Duration;

        let inner = FilterGraphInner::new(
            vec![FilterStep::Scale {
                width: 1280,
                height: 720,
                algorithm: crate::graph::ScaleAlgorithm::Fast,
            }],
            None,
            None,
        );

        assert!(
            inner.graph.is_none(),
            "graph must be None before first push"
        );

        let track = AnimationTrack::new()
            .push(Keyframe::new(Duration::ZERO, 0.0_f64, Easing::Linear))
            .push(Keyframe::new(
                Duration::from_secs(1),
                1.0_f64,
                Easing::Linear,
            ));

        let animations = vec![AnimationEntry {
            node_name: "gblur_0".to_owned(),
            param: "sigma",
            track,
            suffix: "",
        }];

        // Must not panic even though graph == None.
        inner.apply_animations(&animations, Duration::from_millis(500));
    }

    /// Calling `apply_animations` with an empty slice must be a no-op
    /// regardless of graph state.
    #[test]
    fn apply_animations_with_empty_slice_should_be_a_no_op() {
        use std::time::Duration;

        let inner = FilterGraphInner::new(vec![], None, None);
        // No FFmpeg calls expected; must not panic.
        inner.apply_animations(&[], Duration::ZERO);
    }

    // validate_filter_steps

    /// `validate_filter_steps` must return `Ok` for a known-good filter name.
    ///
    /// On platforms where the filter registry is already populated (Windows,
    /// macOS) this exercises the real lookup path.  On Linux builds where the
    /// registry is not yet initialised at unit-test time, `avfilter_get_by_name`
    /// returns null and the function gracefully defers, also returning `Ok`.
    /// Either way the result must never be `Err` for a standard filter like
    /// `scale`.
    #[test]
    fn validate_filter_steps_should_succeed_for_known_filters() {
        let steps = vec![FilterStep::Scale {
            width: 640,
            height: 360,
            algorithm: crate::graph::ScaleAlgorithm::Fast,
        }];
        assert!(
            validate_filter_steps(&steps).is_ok(),
            "validate_filter_steps must not return Err for a standard filter: \
             either the filter was found, or the registry is not yet initialised \
             and validation is deferred"
        );
    }
}
