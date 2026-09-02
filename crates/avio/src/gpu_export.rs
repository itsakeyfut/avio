//! GPU export path: a deterministic decode -> GPU composite -> readback -> encode
//! loop for an eligible timeline (bridge Br4, #1627).
//!
//! The offline compositor ([`MultiTrackComposer`](ff_filter::MultiTrackComposer))
//! fuses decode and composite in one filter graph and never exposes a per-layer
//! frame, so the GPU export cannot reuse it. Instead this module decodes each clip's
//! source directly ([`ff_decode::VideoDecoder`]), composites each output frame on the
//! GPU via the shared [`GpuCompositor`](crate::gpu_compositor::GpuCompositor), reads
//! it back to rgba, and pushes it to the unchanged encoder (whose own sws converts
//! rgba -> yuv420p).
//!
//! v1 handles only a **single active video track of contiguous hard cuts** at unity
//! speed whose every clip is a file source that maps to the GPU with an identity
//! transform and a canvas-matching aspect. A source whose frame rate differs from the
//! timeline's is conformed by the drain (#1660), repeating or skipping source frames so
//! the clip keeps its on-screen duration. Anything else keeps the whole export on the
//! CPU `MultiTrackComposer` path (see [`eligible_track`]); multi-track / overlay GPU
//! export is a follow-up.

use std::time::{Duration, Instant};

use ff_decode::{SeekMode, VideoDecoder};
use ff_encode::VideoEncoder;
use ff_filter::{AnimatedValue, VideoLayer};
use ff_format::{PixelFormat, VideoFrame};
use ff_pipeline::Progress;

use crate::derive;
use crate::error::TimelineError;
use crate::gpu::{GpuMapping, map_scene};
use crate::gpu_compositor::GpuCompositor;
use crate::track::Track;

/// Decides whether a timeline can be exported on the GPU export path, returning the
/// index of the single eligible video track, or `None` to keep the whole export on
/// the CPU `MultiTrackComposer` path.
///
/// v1 is deliberately narrow (structural + contiguity checks are I/O-free; only the
/// probe pass reads each source):
/// - no lavfi overlay (a second compositing layer v1 does not handle),
/// - exactly one **active** video track with at least one clip,
/// - every clip is a **file** source (a generated Solid/Text source has no decoder
///   here; it renders via lavfi on the CPU path),
/// - hard cuts only (a transition needs an xfade node the GPU path lacks),
/// - unity speed (the drain conforms frame *rate* but does not resample time),
/// - each clip's derived [`VideoLayer`] maps to [`GpuMapping::Gpu`] (a supported
///   blend / composite / effect set) with a **static, neutral transform** (RK-020:
///   the model's pixels/degrees are not `ff_render`'s UV/radians, so any placement
///   falls back),
/// - the clips **tile the timeline with no gap or overlap** (each `clip.offset`
///   equals the sum of the preceding clips' durations): the decode loop concatenates
///   clips in order without honouring `clip.offset`, so a leading gap, an inter-clip
///   gap, or an overlap would diverge from the CPU compositor (which places each clip
///   via `OffsetPts`),
/// - each source's native aspect matches the canvas (the compositor would stretch a
///   differently-shaped frame where the CPU path letterboxes) and its native frame
///   rate is usable (positive and finite). The rate no longer has to *match* the
///   timeline `frame_rate`: the drain conforms it (#1660), repeating or skipping
///   source frames so the clip keeps its on-screen duration.
pub(crate) fn eligible_track(
    video_tracks: &[Track],
    lavfi_overlay: Option<&str>,
    any_video_solo: bool,
    canvas: (u32, u32),
) -> Option<usize> {
    if lavfi_overlay.is_some() {
        return None;
    }

    // Exactly one active video track.
    let mut active = video_tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_active(any_video_solo));
    let (idx, track) = active.next()?;
    if active.next().is_some() {
        return None;
    }
    if track.clips.is_empty() {
        return None;
    }

    // Structural pass (no I/O): reject before any source probe so an ineligible
    // clip anywhere on the track keeps the export on the CPU path deterministically.
    for clip in &track.clips {
        if clip.source_path().is_none() || clip.transition.is_some() {
            return None;
        }
        if (clip.speed - 1.0).abs() > 1e-9 {
            return None;
        }
        let layer = derive::video_layer(clip, 0, &track.automation, canvas.0, canvas.1, None, None);
        if !matches!(
            map_scene(std::slice::from_ref(&layer), canvas, Duration::ZERO),
            GpuMapping::Gpu(_)
        ) || !is_static_neutral_transform(&layer)
        {
            return None;
        }
    }

    // Contiguity pass (no I/O): the decode loop concatenates clips in order without
    // honouring `clip.offset`, so it only matches the CPU compositor when the clips
    // tile the timeline with no gap or overlap. Each clip must start exactly where the
    // previous ended; only the final clip may run to end-of-file (unknown duration).
    let mut expected = Duration::ZERO;
    let last = track.clips.len() - 1;
    for (i, clip) in track.clips.iter().enumerate() {
        if clip.offset != expected {
            return None;
        }
        match clip.duration() {
            Some(d) => expected += d,
            None if i == last => {}
            None => return None,
        }
    }

    // Probe pass (I/O): each source's native aspect must match the canvas, and its
    // frame rate must be usable (it no longer has to match the timeline rate).
    for clip in &track.clips {
        let src = clip.source_path()?;
        let Ok(decoder) = VideoDecoder::open(src).build() else {
            return None;
        };
        if u64::from(decoder.width()) * u64::from(canvas.1)
            != u64::from(decoder.height()) * u64::from(canvas.0)
        {
            return None;
        }
        // The frame rate no longer has to match: the drain conforms the source to the
        // timeline rate from the frames' own timestamps (#1660). The rate is still
        // required to be usable, since a source that reports none is one whose timing
        // cannot be trusted at all — it stays on the CPU path rather than risking wrong
        // output (RK-020).
        let src_fps = decoder.frame_rate();
        if !src_fps.is_finite() || src_fps <= 0.0 {
            return None;
        }
    }

    Some(idx)
}

/// The presentation time of output frame `k` **within the clip**, at the timeline rate.
///
/// Conform compares this against the source's own frame timestamps rather than against
/// a nominal rate: a container's reported frame rate is unreliable (a short clip's
/// `avg_frame_rate` comes out as `n/(n-1) * fps`, e.g. 32.14 for a 15-frame 30 fps
/// file), so driving the mapping from it would stretch or shorten the clip. The CPU
/// path's `fps` filter is likewise PTS-driven, so this keeps both routes on one basis.
#[allow(clippy::cast_precision_loss)] // frame index fits the f64 mantissa
fn clip_output_time(k: u64, out_fps: f64) -> Duration {
    Duration::from_secs_f64(k as f64 / out_fps)
}

/// Whether a layer's geometric transform is static and neutral for all `t` (no
/// translate / scale / rotate). Combined with the aspect check this is the v1
/// identity gate: an animated or non-neutral transform makes the timeline
/// ineligible (RK-020) so it never renders wrong output on the GPU.
fn is_static_neutral_transform(layer: &VideoLayer) -> bool {
    matches!(layer.x, AnimatedValue::Static(v) if v.abs() < 1e-9)
        && matches!(layer.y, AnimatedValue::Static(v) if v.abs() < 1e-9)
        && matches!(layer.scale_x, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9)
        && matches!(layer.scale_y, AnimatedValue::Static(v) if (v - 1.0).abs() < 1e-9)
        && matches!(layer.rotation, AnimatedValue::Static(v) if v.abs() < 1e-9)
}

/// Drains an eligible single video track to the encoder on the GPU: decode each
/// clip's frames in order, composite each on the GPU, read it back, and push it to
/// the unchanged encoder. `on_progress` is invoked after each pushed frame;
/// returning `false` cancels with [`TimelineError::Cancelled`].
///
/// The caller has already established eligibility ([`eligible_track`]), so a
/// mid-export fallback from the compositor is a should-not-happen and surfaces as
/// [`TimelineError::TimelineRenderFailed`] rather than silent wrong output.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drain_video_gpu(
    track: &Track,
    canvas: (u32, u32),
    frame_rate: f64,
    encoder: &mut VideoEncoder,
    core: &mut GpuCompositor,
    on_progress: &(impl Fn(&Progress) -> bool + Send),
    start: Instant,
    total_frames: Option<u64>,
) -> Result<(), TimelineError> {
    let mut video_idx: u32 = 0;
    for clip in &track.clips {
        let src = clip
            .source_path()
            .ok_or_else(|| TimelineError::TimelineRenderFailed {
                reason: "gpu export: clip lost its file source".to_string(),
            })?;
        // Decode straight to rgba: the shared core's effect pass reads rgba, and the
        // compositor and readback stay in one format (the encoder's own sws converts
        // rgba -> yuv420p on push).
        let mut decoder = VideoDecoder::open(src)
            .output_format(PixelFormat::Rgba)
            .build()?;
        // Eligibility guarantees a positive, finite rate, so the conform maths below is
        // well defined; fall back to the timeline rate defensively.
        let src_fps = {
            let f = decoder.frame_rate();
            if f.is_finite() && f > 0.0 {
                f
            } else {
                frame_rate
            }
        };
        if let Some(in_point) = clip.in_point {
            decoder.seek(in_point, SeekMode::Exact)?;
        }

        // Output-frame budget for this clip (its trimmed duration at the timeline
        // rate); `None` when the clip runs to end-of-file, so it drains until EOF.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frame_budget: Option<u64> = clip
            .duration()
            .map(|d| (d.as_secs_f64() * frame_rate).round().max(0.0) as u64);

        let layer = derive::video_layer(clip, 0, &track.automation, canvas.0, canvas.1, None, None);

        // Start each clip with a clean effect cache: a stateful effect (MotionBlur's
        // exposure trail) must not accumulate across the cut into this clip (RK-025).
        core.reset_effect_cache();

        let mut produced: u64 = 0;
        if (src_fps - frame_rate).abs() <= 1e-3 {
            // Matching rates: one decoded frame per output frame, moved into the
            // compositor so a no-effects layer avoids a full-frame clone (#1634).
            loop {
                if frame_budget.is_some_and(|budget| produced >= budget) {
                    break;
                }
                let Some(frame) = decoder.decode_one()? else {
                    break; // EOF before the budget: the clip is shorter than declared.
                };
                let t = output_time(video_idx, frame_rate);
                let composited = core.composite_owned(vec![(&layer, frame)], canvas, t);
                emit_frame(
                    composited,
                    encoder,
                    &mut video_idx,
                    on_progress,
                    start,
                    total_frames,
                )?;
                produced += 1;
            }
        } else {
            // Conform (#1660), PTS-driven: hold the newest source frame whose timestamp
            // is at or before this output's time, so a slower source repeats a frame and
            // a faster one skips frames while the clip keeps its on-screen duration.
            // Timestamps rather than a nominal rate, because the reported rate is not
            // trustworthy (see `clip_output_time`).
            //
            // The held frame is *borrowed* because one source frame can serve several
            // outputs. `composite` clones it internally for a no-effects layer
            // (`gpu_compositor.rs`), so this path pays the per-output clone that the
            // matching-rate path avoids with `composite_owned` (#1634) — accepted for
            // v1 since conform is the uncommon case.
            let base = clip.in_point.unwrap_or(Duration::ZERO);
            let mut held: Option<VideoFrame> = None;
            let mut held_at = Duration::ZERO;
            let mut pending: Option<(VideoFrame, Duration)> = None;
            let mut eof = false;
            loop {
                if frame_budget.is_some_and(|budget| produced >= budget) {
                    break;
                }
                let want = clip_output_time(produced, frame_rate);
                // Advance while the next source frame still starts at or before `want`;
                // the last such frame is the one this output shows. `pending` carries the
                // lookahead frame that already belongs to a later output.
                loop {
                    if let Some((frame, at)) = pending.take() {
                        if held.is_none() || at <= want {
                            held = Some(frame);
                            held_at = at;
                            continue;
                        }
                        pending = Some((frame, at));
                        break;
                    }
                    if eof {
                        break;
                    }
                    match decoder.decode_one()? {
                        Some(frame) => {
                            let at = frame.timestamp().as_duration().saturating_sub(base);
                            pending = Some((frame, at));
                        }
                        None => eof = true,
                    }
                }
                let Some(frame) = held.as_ref() else {
                    break; // The clip decoded no frames at all.
                };
                // The source is spent and this output is past its last frame: the clip
                // ends here, matching the pre-existing "shorter than declared" stop.
                if eof && pending.is_none() && want > held_at {
                    break;
                }
                let t = output_time(video_idx, frame_rate);
                let composited = core.composite(&[(&layer, frame)], canvas, t);
                emit_frame(
                    composited,
                    encoder,
                    &mut video_idx,
                    on_progress,
                    start,
                    total_frames,
                )?;
                produced += 1;
            }
        }
    }
    Ok(())
}

/// The composite time of output frame `video_idx` at the timeline rate.
#[allow(clippy::cast_precision_loss)] // frame index fits the f64 mantissa
fn output_time(video_idx: u32, frame_rate: f64) -> Duration {
    Duration::from_secs_f64(f64::from(video_idx) / frame_rate)
}

/// Reads back an already-composited frame, pushes it to the encoder, advances the
/// output-frame counter and reports progress.
///
/// Takes the composite *result* rather than the compositor so the caller keeps the
/// choice of moving the frame in (`composite_owned`, the matching-rate path) or
/// borrowing it (`composite`, the conform path, where one source frame can serve
/// several outputs). A `None` means a frame fell back mid-export, which eligibility
/// has already precluded, so it surfaces as an error rather than wrong output.
fn emit_frame(
    composited: Option<(Vec<u8>, u32, u32)>,
    encoder: &mut VideoEncoder,
    video_idx: &mut u32,
    on_progress: &(impl Fn(&Progress) -> bool + Send),
    start: Instant,
    total_frames: Option<u64>,
) -> Result<(), TimelineError> {
    let (rgba, w, h) = composited.ok_or_else(|| TimelineError::TimelineRenderFailed {
        reason: "gpu export: a frame fell back mid-export (precluded by eligibility)".to_string(),
    })?;
    let out =
        VideoFrame::from_rgba(w, h, rgba).map_err(|e| TimelineError::TimelineRenderFailed {
            reason: format!("gpu export: readback frame invalid: {e}"),
        })?;
    encoder.push_video(&out)?;
    *video_idx = video_idx.saturating_add(1);
    let progress = Progress {
        frames_processed: u64::from(*video_idx),
        total_frames,
        elapsed: start.elapsed(),
    };
    if !on_progress(&progress) {
        return Err(TimelineError::Cancelled);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use ff_filter::XfadeTransition;
    use ff_format::Color;

    use super::*;
    use crate::{Clip, Timeline};

    /// A canvas-sized (square) single hard-cut file-source track is the shape the GPU
    /// export handles; the structural checks accept it (the probe is exercised e2e).
    fn square_timeline(clips: Vec<Clip>) -> Timeline {
        Timeline::builder()
            .canvas(64, 64)
            .frame_rate(30.0)
            .video_track(clips)
            .build()
            .unwrap()
    }

    fn eligible(timeline: &Timeline) -> Option<usize> {
        eligible_track(
            &timeline.video_tracks,
            timeline.lavfi_overlay.as_deref(),
            timeline.video_tracks.iter().any(|t| t.solo),
            (timeline.canvas_width, timeline.canvas_height),
        )
    }

    /// Mirrors the drain's selection rule — show the last source frame whose
    /// clip-relative timestamp is at or before the output's time — so the mapping is
    /// verifiable without decoding. The drain streams and cannot pre-collect timestamps,
    /// hence the small duplication; the integration tests cover the real pipeline.
    fn conform_plan(src_pts: &[Duration], out_fps: f64, outputs: u64) -> Vec<usize> {
        (0..outputs)
            .map(|k| {
                let want = clip_output_time(k, out_fps);
                src_pts.iter().rposition(|at| *at <= want).unwrap_or(0)
            })
            .collect()
    }

    /// `count` frames at `fps`, as clip-relative timestamps.
    fn pts_at(fps: f64, count: usize) -> Vec<Duration> {
        #[allow(clippy::cast_precision_loss)]
        (0..count)
            .map(|i| Duration::from_secs_f64(i as f64 / fps))
            .collect()
    }

    #[test]
    fn conform_should_repeat_frames_when_source_is_slower() {
        // 24 -> 30: outputs at k/30 fall on 24 fps frames 0,0,1,2,3,4,4 — the
        // duplication that keeps the clip's on-screen duration.
        let plan = conform_plan(&pts_at(24.0, 6), 30.0, 7);
        assert_eq!(plan, [0, 0, 1, 2, 3, 4, 4]);
    }

    #[test]
    fn conform_should_skip_frames_when_source_is_faster() {
        // 60 -> 30: every other source frame is dropped.
        let plan = conform_plan(&pts_at(60.0, 9), 30.0, 5);
        assert_eq!(plan, [0, 2, 4, 6, 8]);
    }

    #[test]
    fn conform_should_be_identity_at_matching_rates() {
        let plan = conform_plan(&pts_at(30.0, 5), 30.0, 5);
        assert_eq!(plan, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn conform_should_ignore_a_misreported_container_rate() {
        // The regression that motivated the PTS basis: a 15-frame 30 fps file reports
        // `avg_frame_rate` 32.14 (= 15/14 * 30). Selection driven by that number would
        // skip ahead and end the clip early; driven by timestamps it is the identity.
        let plan = conform_plan(&pts_at(30.0, 15), 30.0, 15);
        assert_eq!(plan, (0..15).collect::<Vec<_>>());
    }

    /// Encodes a tiny square video at `fps`, or `None` when the environment has no
    /// usable encoder (skip). The probe pass needs a real file, so eligibility cannot
    /// be exercised without one.
    fn encode_probe_source(path: &std::path::Path, fps: f64) -> Option<()> {
        use ff_encode::{VideoCodec, VideoEncoder};
        use ff_format::{PixelFormat as PF, VideoFrame};

        let mut enc = VideoEncoder::create(path)
            .video(64, 64, fps)
            .video_codec(VideoCodec::Mpeg4)
            .build()
            .ok()?;
        // A few flat frames are enough: only the stream header (size, rate) is probed.
        for i in 0..4 {
            let frame = VideoFrame::new_black(64, 64, PF::Yuv420p, i);
            enc.push_video(&frame).ok()?;
        }
        enc.finish().ok()?;
        Some(())
    }

    #[test]
    fn eligible_track_should_accept_a_source_whose_rate_differs_from_the_timeline() {
        // #1660: the probe pass no longer requires the source rate to match the
        // timeline rate — the drain conforms it — so a source whose rate differs stays
        // on the GPU route instead of falling back to CPU.
        //
        // This gate mattered more than it looked: a container's reported rate is often
        // *not* the nominal encode rate (a short clip reports `n/(n-1) * fps`, so the
        // 15-frame 30 fps fixture reports 32.14), which meant the old equality check
        // rejected even same-rate sources. The end-to-end "GPU route" export test was
        // therefore silently exercising the CPU path. Keeping this assertion green is
        // what stops that false green from coming back.
        let src = std::env::temp_dir().join("avio_eligible_24fps_probe.mp4");
        let _ = std::fs::remove_file(&src);
        if encode_probe_source(&src, 24.0).is_none() {
            return; // no encoder here -> skip
        }
        // The probe pass opens a decoder, so a build without this decoder cannot reach
        // the gate at all — skip rather than read the miss as a rejection (RK-002).
        if VideoDecoder::open(&src).build().is_err() {
            let _ = std::fs::remove_file(&src);
            return;
        }
        let t = square_timeline(vec![Clip::new(&src)]); // canvas 64x64, timeline 30 fps
        let eligible_now = eligible(&t);
        let _ = std::fs::remove_file(&src);
        assert_eq!(
            eligible_now,
            Some(0),
            "a 24 fps source in a 30 fps timeline must be GPU-eligible after #1660"
        );
    }

    #[test]
    fn eligible_track_should_reject_a_generated_source() {
        // A Solid clip has no decoder on the GPU path, so it stays on the CPU path.
        let t = square_timeline(vec![
            Clip::solid(Color::rgb(1, 2, 3)).trim(Duration::ZERO, Duration::from_secs(1)),
        ]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_reject_a_transition() {
        // A cross-fade needs an xfade node the GPU path lacks -> CPU.
        let t = square_timeline(vec![
            Clip::new("a.mp4"),
            Clip::new("b.mp4").with_transition(XfadeTransition::Fade, Duration::from_millis(500)),
        ]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_reject_non_unity_speed() {
        // The drain conforms frame rate but does not resample time -> CPU.
        let t = square_timeline(vec![Clip::new("a.mp4").with_speed(2.0)]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_reject_a_non_identity_transform() {
        // A scaled layer is not an identity transform (RK-020) -> CPU.
        let t = square_timeline(vec![Clip::new("a.mp4").with_scale(0.5)]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_reject_a_leading_gap() {
        // A single clip starting after t=0 (offset > 0) leaves a leading gap the CPU
        // path renders as black; the offset-ignoring decode loop would drop it -> CPU.
        let t = square_timeline(vec![
            Clip::new("a.mp4")
                .trim(Duration::ZERO, Duration::from_secs(1))
                .offset(Duration::from_secs(1)),
        ]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_reject_an_inter_clip_gap() {
        // Two hard-cut clips with a gap between them (clip 1 starts at 2s but clip 0
        // ends at 1s): the decode loop would concatenate them and drop the gap -> CPU.
        let t = square_timeline(vec![
            Clip::new("a.mp4").trim(Duration::ZERO, Duration::from_secs(1)),
            Clip::new("b.mp4")
                .trim(Duration::ZERO, Duration::from_secs(1))
                .offset(Duration::from_secs(2)),
        ]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_reject_an_interior_clip_of_unknown_duration() {
        // A non-final clip without an out_point cannot be tiled deterministically
        // (its end, hence the next clip's start, is unknown) -> CPU.
        let t = square_timeline(vec![
            Clip::new("a.mp4"), // no out_point -> unknown duration
            Clip::new("b.mp4").offset(Duration::from_secs(1)),
        ]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_reject_a_lavfi_overlay() {
        // A lavfi overlay is a second compositing layer v1 does not handle -> CPU.
        let mut t = square_timeline(vec![Clip::new("a.mp4")]);
        t.lavfi_overlay = Some("color=red".to_string());
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_reject_two_active_video_tracks() {
        // v1 handles a single track; a second active track -> CPU.
        let t = Timeline::builder()
            .canvas(64, 64)
            .frame_rate(30.0)
            .video_track(vec![Clip::new("a.mp4")])
            .video_track(vec![Clip::new("b.mp4")])
            .build()
            .unwrap();
        assert!(eligible(&t).is_none());
    }
}
