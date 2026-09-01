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
//! transform and a canvas-matching aspect. Anything else keeps the whole export on
//! the CPU `MultiTrackComposer` path (see [`eligible_track`]); multi-track / overlay
//! GPU export is a follow-up.

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
/// - unity speed (the one-frame-per-output decode loop does not resample time),
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
///   rate matches the timeline `frame_rate` (the 1-frame-per-output decode loop does
///   not resample, so a mismatch would change the clip's on-screen duration).
pub(crate) fn eligible_track(
    video_tracks: &[Track],
    lavfi_overlay: Option<&str>,
    any_video_solo: bool,
    canvas: (u32, u32),
    frame_rate: f64,
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

    // Probe pass (I/O): each source's native aspect and frame rate must match the
    // canvas / timeline rate.
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
        if (decoder.frame_rate() - frame_rate).abs() > 1e-3 {
            return None;
        }
    }

    Some(idx)
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

        let mut produced: u64 = 0;
        loop {
            if frame_budget.is_some_and(|budget| produced >= budget) {
                break;
            }
            let Some(frame) = decoder.decode_one()? else {
                break; // EOF before the budget is met: the clip is shorter than declared.
            };
            #[allow(clippy::cast_precision_loss)] // frame index fits the f64 mantissa
            let t = Duration::from_secs_f64(f64::from(video_idx) / frame_rate);
            // Move the freshly-decoded frame into the compositor: a no-effects layer
            // then avoids a full-frame clone on this hot path (#1634).
            let (rgba, w, h) = core
                .composite_owned(vec![(&layer, frame)], canvas, t)
                .ok_or_else(|| TimelineError::TimelineRenderFailed {
                    reason: "gpu export: a frame fell back mid-export (precluded by eligibility)"
                        .to_string(),
                })?;
            let out = VideoFrame::from_rgba(w, h, rgba).map_err(|e| {
                TimelineError::TimelineRenderFailed {
                    reason: format!("gpu export: readback frame invalid: {e}"),
                }
            })?;
            encoder.push_video(&out)?;
            video_idx = video_idx.saturating_add(1);
            produced += 1;
            let progress = Progress {
                frames_processed: u64::from(video_idx),
                total_frames,
                elapsed: start.elapsed(),
            };
            if !on_progress(&progress) {
                return Err(TimelineError::Cancelled);
            }
        }
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
            timeline.frame_rate,
        )
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
        // The one-frame-per-output decode loop does not resample time -> CPU.
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
