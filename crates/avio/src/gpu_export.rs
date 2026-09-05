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
//! The route takes a **single active video track** at unity speed whose every clip is a
//! file source. Several restrictions the first version had are gone: a source whose frame
//! rate differs from the timeline's is conformed by the drain (#1660), one whose aspect
//! differs from the canvas is letterboxed by the shared compositing core (#1661), a
//! **cross-fade at any boundary** is rendered by the drain rather than rejected (#1659),
//! and a clip carrying a position or scale is no longer turned away — the drain
//! composites one layer per output, and a base layer's transform is ignored on both paths
//! (#1633; see `gpu_compositor::layer_transform`).
//!
//! What still keeps the whole export on the CPU `MultiTrackComposer` path (see
//! [`eligible_track`]): more than one active video track, a lavfi overlay, a generated
//! (non-file) source, a speed other than 1, clips that do not tile the timeline, and a
//! transition whose kind or window the GPU cannot render. Multi-track / overlay GPU
//! export is #1633's remaining half.

use std::time::{Duration, Instant};

use ff_decode::{SeekMode, VideoDecoder};
use ff_encode::VideoEncoder;
use ff_filter::{AnimatedValue, BlendMode, CompositeOp, VideoLayer, XfadeTransition};
use ff_format::{PixelFormat, VideoFrame};
use ff_pipeline::Progress;
use ff_render::BlendMode as RenderBlendMode;

use crate::clip::Clip;
use crate::derive;
use crate::error::TimelineError;
use crate::gpu::{GpuEffect, GpuLayerPlan, GpuMapping, map_scene};
use crate::gpu_compositor::GpuCompositor;
use crate::gpu_transition::{GpuTransition, map_transition};
use crate::track::Track;

/// Whether the GPU export renders `kind` itself, or leaves the whole export to the CPU.
///
/// Every kind [`map_transition`] covers **except `Dissolve`**, now that each node
/// reproduces `FFmpeg`'s own
/// formula rather than an approximation of it (#1732). Worst-frame mean between the two
/// export routes, as printed by
/// `gpu_export_tests::gpu_export_should_match_the_cpu_export_for_every_rendered_transition`
/// (so the numbers are reproducible from the suite that guards them, not from a
/// throwaway harness):
///
/// | kind | mean |
/// |---|---|
/// | `Fade` | 2.0 |
/// | `WipeLeft` / `WipeRight` / `WipeUp` / `WipeDown` | 2.1 - 2.3 |
/// | `FadeBlack` / `FadeWhite` | 2.0 - 2.1 |
///
/// A hard cut's own GPU-vs-CPU floor on the same sources is 1.4, so every rendered kind
/// sits just above the colour round trip and nowhere near a real divergence.
///
/// **`Dissolve` is excluded, and not because of its formula.** Its selection is
/// `ff_filter::xfade_frand`, which is `sinf` of an argument large enough that the result
/// depends on the libm evaluating it. The GPU route builds the mask with **Rust's**
/// `sinf` while the CPU route runs **`FFmpeg`'s**, and the two agree only where their
/// libms do: measured worst-frame mean 3.6 between the routes on Windows but 6.6 on
/// macOS, i.e. a different set of pixels turning over. A viewer toggling force-CPU would
/// see different noise for the same timeline, so the export declines it rather than
/// render what the other route would not (RK-020). Nothing else here depends on libm
/// agreement -- the blends are arithmetic and the wipes are integer comparisons.
///
/// This was `Fade`-only before #1732, when the nodes were pinned to
/// `ff_preview::apply_xfade` and that reference had itself drifted from `FFmpeg` --
/// `Dissolve` chose a different set of pixels (mean 54) and the dips followed a
/// different curve (mean 78). The function stays as the export's explicit policy point:
/// a kind that maps to a node but does *not* reproduce `FFmpeg` belongs on the CPU, and
/// this is where it would be excluded (RK-020).
fn export_maps_to_gpu(kind: XfadeTransition) -> bool {
    !matches!(kind, XfadeTransition::Dissolve) && map_transition(kind).is_some()
}

/// A transition's length in output frames at the timeline rate.
///
/// This is exactly how many outputs the CPU route's `xfade` consumes: measured on a
/// 30 fps timeline, a 0.5 s transition between two 1 s clips turns the hard cut's 60
/// output frames into 45, blending across the 15 in between.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn window_frames(d: Duration, frame_rate: f64) -> u64 {
    (d.as_secs_f64() * frame_rate).round().max(0.0) as u64
}

/// A clip's output-frame budget (its trimmed duration at the timeline rate), or `None`
/// when the clip runs to end-of-file.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn budget_frames(clip: &Clip, frame_rate: f64) -> Option<u64> {
    clip.duration()
        .map(|d| (d.as_secs_f64() * frame_rate).round().max(0.0) as u64)
}

/// The clip's export [`VideoLayer`], with any transition removed.
///
/// The drain runs the transition itself (see [`drain_video_gpu`]), so the layer must not
/// also carry a `FilterStep::XFade` -- `map_scene` does not map that step, so the whole
/// timeline would fall back. Passing `prev_end: None` keeps the step out too, but makes
/// `derive` log "transition on a track's first clip ignored" for every eligible clip,
/// which is not what happened; clearing the field says what is meant.
///
/// A transition on the track's *first* clip is genuinely ignored, matching `derive` on
/// the CPU route (there is no preceding clip to cross-fade from).
fn transitionless_layer(clip: &Clip, track: &Track, canvas: (u32, u32)) -> VideoLayer {
    if clip.transition.is_none() {
        // `Placement::default()`: the drain runs the transition itself, and reads past the
        // out-point through `ClipSource` rather than through a widened trim.
        return derive::video_layer(
            clip,
            0,
            &track.automation,
            canvas.0,
            canvas.1,
            &derive::Placement::default(),
            None,
        );
    }
    let mut without = clip.clone();
    without.transition = None;
    derive::video_layer(
        &without,
        0,
        &track.automation,
        canvas.0,
        canvas.1,
        &derive::Placement::default(),
        None,
    )
}

/// Decides whether a timeline can be exported on the GPU export path, returning the
/// indices of the eligible video tracks **bottom to top**, or `None` to keep the whole
/// export on the CPU `MultiTrackComposer` path.
///
/// The checks are per track (structural, transition and contiguity are I/O-free; only
/// the probe pass reads each source), and **every** active track has to pass -- the
/// fallback is whole-frame, so a partial stack is never composited:
/// - no lavfi overlay (a generated source the drain has no decoder for),
/// - at least one **active** video track, each with at least one clip,
/// - every clip is a **file** source (a generated Solid/Text source has no decoder
///   here; it renders via lavfi on the CPU path),
/// - unity speed (the drain conforms frame *rate* but does not resample time),
/// - each clip's derived [`VideoLayer`] maps to [`GpuMapping::Gpu`] (a supported
///   blend / composite / effect set). A position or scale is fine since #1767; a
///   rotated overlay is not, and falls back through the compositor,
/// - transitions of a kind the GPU renders the same way the CPU export does (see
///   [`export_maps_to_gpu`]), at any boundary, **on the base track only** -- the rest of
///   the restrictions are spelled out in [`eligible_transition`],
/// - the clips **tile the timeline with no gap or overlap** (each `clip.offset`
///   equals the sum of the preceding clips' durations): the decode loop concatenates
///   clips in order without honouring `clip.offset`, so a leading gap, an inter-clip
///   gap, or an overlap would diverge from the CPU compositor (which places each clip
///   via `OffsetPts`),
/// - each source's native frame rate is usable (positive and finite). Neither the rate
///   nor the aspect has to *match* the timeline any more: the drain conforms the rate
///   (#1660), repeating or skipping source frames so the clip keeps its on-screen
///   duration, and the shared compositing core letterboxes a differently-shaped frame
///   into the canvas (#1661).
pub(crate) fn eligible_tracks(
    video_tracks: &[Track],
    lavfi_overlay: Option<&str>,
    any_video_solo: bool,
    canvas: (u32, u32),
    frame_rate: f64,
) -> Option<Vec<usize>> {
    // A generated (lavfi) overlay is not a file source, so the drain has nothing to
    // decode for it. Still the whole export's answer, not just that layer's.
    if lavfi_overlay.is_some() {
        return None;
    }

    let active: Vec<(usize, &Track)> = video_tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_active(any_video_solo))
        .collect();
    if active.is_empty() {
        return None;
    }
    // Every track has to pass: the fallback is whole-frame, so one ineligible track
    // keeps the entire export on the CPU rather than compositing a partial stack.
    let stacked = active.len() > 1;
    for (stack_pos, (_, track)) in active.iter().enumerate() {
        eligible_one_track(track, stack_pos == 0, stacked, canvas, frame_rate)?;
    }
    Some(active.into_iter().map(|(idx, _)| idx).collect())
}

/// Whether one track can be driven by the GPU drain. `is_base` marks the bottom of the
/// stack, whose transform the compositor ignores (see `gpu_compositor::layer_transform`).
fn eligible_one_track(
    track: &Track,
    is_base: bool,
    stacked: bool,
    canvas: (u32, u32),
    frame_rate: f64,
) -> Option<()> {
    if track.clips.is_empty() {
        return None;
    }

    // A transition is only rendered on the **base** track for now. The CPU scales each
    // clip to `canvas * scale` and only then runs `xfade`, so the blend happens on
    // placed-size content; the GPU places via `layer_transform` *after* the blend, which
    // for a non-base track would either apply the transform twice or reorder the
    // resampling against the blend. Neither is measured, so it falls back rather than
    // approximate (RK-020). The base track is unaffected: its transform is ignored, so
    // blending canvas-composited frames is exactly what the CPU does -- which is the
    // path #1659 shipped and this must not regress.
    if !is_base && track.clips.iter().skip(1).any(|c| c.transition.is_some()) {
        return None;
    }

    // Structural pass (no I/O): reject before any source probe so an ineligible
    // clip anywhere on the track keeps the export on the CPU path deterministically.
    //
    // `blendable[i]` records whether clip `i` may take part in a transition: its node
    // effects carry no cross-frame state, and its solo composite is the identity. Both
    // matter only at a transition, where two clips share one layer slot and are
    // composited before they are blended (see `eligible_transition`).
    let mut blendable = vec![false; track.clips.len()];
    for (i, clip) in track.clips.iter().enumerate() {
        clip.source_path()?;
        if (clip.speed - 1.0).abs() > 1e-9 {
            return None;
        }
        let layer = transitionless_layer(clip, track, canvas);
        let GpuMapping::Gpu(plan) = map_scene(std::slice::from_ref(&layer), canvas, Duration::ZERO)
        else {
            return None;
        };
        // No transform gate: the drain composites one layer per output, which is the
        // compositor's **base** layer, and a base layer's x / y / scale / rotation are
        // ignored on both paths (measured; see `gpu_compositor::layer_transform`). A
        // positioned or scaled clip therefore renders identically either way, so keeping
        // it on the CPU bought nothing.
        // A stateful node keeps its state in the cached effect graph, and the stack
        // scheduler alternates the compositor between a one-layer solo composite (the
        // base) and an N-layer stack composite every output frame, which evicts that
        // cache each time (RK-025). A MotionBlur trail would restart every frame instead
        // of accumulating, so a stacked export declines it rather than rendering a
        // different blur than the CPU does.
        if stacked
            && plan
                .layers
                .iter()
                .any(|l| l.effects.iter().any(is_stateful_effect))
        {
            return None;
        }
        blendable[i] = plan
            .layers
            .iter()
            .all(|l| is_neutral_composite(l) && !l.effects.iter().any(is_stateful_effect));
    }

    // Transition pass (no I/O). A transition on the *first* clip is ignored rather than
    // rejected, matching `derive` on the CPU route: there is no preceding clip to
    // cross-fade from, so both routes render a plain clip (`transitionless_layer`).
    let last = track.clips.len() - 1;
    for i in 1..track.clips.len() {
        if track.clips[i].transition.is_some()
            && !eligible_transition(
                &track.clips[i - 1],
                &track.clips[i],
                blendable[i - 1] && blendable[i],
                frame_rate,
            )
        {
            return None;
        }
    }

    // Contiguity pass (no I/O): the decode loop concatenates clips in order without
    // honouring `clip.offset`, so it only matches the CPU compositor when the clips
    // tile the timeline with no gap or overlap. Each clip must start exactly where the
    // previous ended; only the final clip may run to end-of-file (unknown duration).
    //
    // A transitioned clip keeps this requirement even though `xfade` ignores its
    // `OffsetPts` entirely (measured: moving clip B by a second changes nothing on the
    // CPU route). Rejecting a gap the CPU would have swallowed only costs a fallback,
    // and it keeps the accepted set to timelines whose model placement both routes read
    // the same way.
    let mut expected = Duration::ZERO;
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

    // Probe pass (I/O): each source's frame rate must be usable. Its aspect no longer
    // has to match the canvas -- the shared compositing core letterboxes it (#1661).
    for clip in &track.clips {
        let src = clip.source_path()?;
        let Ok(decoder) = VideoDecoder::open(src).build() else {
            return None;
        };
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

    Some(())
}

/// Whether an effect's node carries state across the frames it processes.
///
/// Only [`GpuEffect::MotionBlur`], whose exposure trail *is* its cross-frame reuse
/// (RK-025). Every other mapped effect is fully determined by its `GpuEffect` value, so
/// two clips sharing one cached graph get the same output either way.
fn is_stateful_effect(effect: &GpuEffect) -> bool {
    matches!(effect, GpuEffect::MotionBlur { .. })
}

/// Whether a layer's solo composite onto the empty canvas is the identity, so blending
/// *after* it gives the same answer as blending before.
///
/// The transition window composites each clip alone and then blends the two, while the
/// CPU route blends first (`xfade`) and composites the result. The orders commute only
/// for a layer the solo composite leaves alone. A partially transparent one does not
/// survive it: `blend.wgsl` computes `mix(base.rgb, blend_rgb, overlay.a * opacity)`
/// against the canvas' transparent black, so an `opacity` of 0.5 reaches the blend
/// already darkened, while on the CPU it only sets clip B's alpha -- which `xfade`
/// ignores, mixing full-strength RGB and letting the overlay apply the opacity
/// afterwards. Measured on a 0.5 s `Fade`: luma diverged by 26 at a static opacity of
/// 0.5 and by 42 with an animated one, *inside the window only* (RK-020).
///
/// A non-`Normal` blend mode composes against the canvas in the same place and so has
/// the same problem. `CompositeOp` needs no check here: `map_scene` already rejects
/// anything but `Over` for the whole timeline.
fn is_neutral_composite(plan: &GpuLayerPlan) -> bool {
    (plan.opacity - 1.0).abs() < 1e-6 && plan.blend_mode == RenderBlendMode::Normal
}

/// Whether the transition on `incoming` -- the clip cross-faded *into*, whose
/// predecessor on the track is `outgoing` -- is one the GPU export renders itself.
///
/// Everything here is a *fallback* condition, not an error: a rejected transition keeps
/// the whole export on the CPU route, which handles all of these.
///
/// A transition on any clip qualifies. The "last clip only" restriction this carried
/// before ADR-0009 existed because the CPU route shrank its output by the transition's
/// duration while later clips kept their absolute `OffsetPts`, which opened a hole
/// (measured: 15 frames of pure black for a 0.5 s transition at 30 fps) and made chained
/// transitions fire early. Placement now preserves the timeline length on both routes,
/// so there is nothing left for the restriction to guard (#1731).
///
/// - **A kind both routes render alike** ([`export_maps_to_gpu`]).
/// - **Both clips of known duration**, so every bound below is checkable up front rather
///   than discovered at EOF.
/// - **A window of at least one frame that fits the incoming clip head.** A sub-frame
///   duration has no frames to blend, and a window longer than the incoming clip would
///   consume more head than it has. The outgoing clip's *body* is deliberately not a
///   bound: the blend reads its handle, not its on-screen frames. RK-020: the degenerate
///   corner of a reproduced formula is exactly where silent wrong output comes from.
/// - **A handle long enough to cover the whole window.** When it is not,
///   `transition::effective_duration` shortens the blend on both routes; the GPU one
///   declines the timeline rather than reproduce a clamp, which costs only a fallback.
///
/// The checks are ordered so every I/O-free rejection happens first: the handle is the
/// one fact that needs the source opened, so it is asked for last.
/// - **Both clips are `blendable`** ([`is_neutral_composite`] and no stateful effect).
///   The window composites the two alternately at the *same* layer position and blends
///   the results, so a cached effect graph would evict its neighbour's every frame
///   (RK-025) and a non-identity solo composite would reach the blend already applied
///   (RK-020).
fn eligible_transition(outgoing: &Clip, incoming: &Clip, blendable: bool, frame_rate: f64) -> bool {
    if !blendable {
        return false;
    }
    let Some(kind) = incoming.transition else {
        return false;
    };
    if !export_maps_to_gpu(kind) {
        return false;
    }
    // The outgoing budget is still required to be known: the drain runs it to the end
    // before the window opens, and an end-of-file clip has no counted tail to run.
    let (Some(incoming_budget), Some(_outgoing_budget)) = (
        budget_frames(incoming, frame_rate),
        budget_frames(outgoing, frame_rate),
    ) else {
        return false;
    };
    let authored = window_frames(incoming.transition_duration, frame_rate);
    if authored < 1 || authored > incoming_budget {
        return false;
    }
    // The only check that opens a file, so it runs once everything structural has
    // passed. Equality, not `>=`: a clamped window is a transition the model did not ask
    // for, and the CPU route renders that case correctly on its own.
    window_frames(
        crate::transition::effective_duration(outgoing, incoming),
        frame_rate,
    ) == authored
}

/// The transition window (in output frames) for the transition into `incoming`, or `0`
/// when it carries none. `effective` is that boundary's entry from
/// `transition::effective_durations`, which the caller resolves once for the whole track.
///
/// The window comes out of neither body: it is fed by the outgoing handle and the
/// incoming head, so the track still runs for the sum of the two budgets (ADR-0009). The
/// duration is the same rule the CPU route derives its `xfade` from, so the two cannot
/// blend across different spans.
///
/// Only reached for a track [`eligible_track`] accepted, so a transition here is already
/// known to be a mapped kind with a window that fits. One that is not would otherwise be
/// rendered as a cross-fade in place of what the model asked for, so it surfaces as an
/// error instead (RK-020).
fn transition_window(
    incoming: &Clip,
    effective: Duration,
    frame_rate: f64,
) -> Result<u64, TimelineError> {
    let Some(kind) = incoming.transition else {
        return Ok(0);
    };
    if !export_maps_to_gpu(kind) {
        return Err(TimelineError::TimelineRenderFailed {
            reason: format!(
                "gpu export: transition {kind:?} has no GPU node (precluded by eligibility)"
            ),
        });
    }
    Ok(window_frames(effective, frame_rate))
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

/// The frame one clip shows for one output, and whether the drain may take it.
///
/// The distinction is [`GpuCompositor::composite_owned`]'s (#1634): a matching-rate clip
/// decodes one frame per output and can move it into the compositor, while a conformed
/// clip may show one held frame for several outputs and can only lend it.
enum Pulled<'a> {
    /// Freshly decoded and no longer needed by the source: move it.
    Owned(VideoFrame),
    /// Held for this and possibly later outputs: borrow it.
    Held(&'a VideoFrame),
}

/// One clip's decoded frames, delivered one output frame at a time.
///
/// The drain used to inline this as two loops (matching-rate and PTS-conform) inside
/// `for clip in &track.clips`, which cannot serve a transition: that needs the outgoing
/// clip's tail and the incoming clip's head *alternately*, so both have to be resumable
/// (#1659). Pulling them frame by frame also keeps the drain O(1) in memory -- buffering
/// the incoming clip's head instead would cost a canvas per window frame (124 MB for
/// 0.5 s of 1080p30).
struct ClipSource {
    decoder: VideoDecoder,
    /// Output frames this clip contributes, or `None` to drain to end-of-file.
    ///
    /// [`allow_handle`](ClipSource::allow_handle) raises it for the transition window:
    /// the blend reads the outgoing clip *past* its out-point, so the extra frames are
    /// its handle rather than part of its on-screen body (ADR-0009).
    budget: Option<u64>,
    produced: u64,
    frame_rate: f64,
    /// The source's rate matches the timeline's: one decoded frame per output.
    one_to_one: bool,
    /// Clip-relative zero, so a trimmed clip's timestamps start at 0.
    base: Duration,
    /// The newest source frame at or before the current output's time. One source frame
    /// serves several outputs when conforming up, hence held rather than consumed.
    held: Option<VideoFrame>,
    held_at: Duration,
    /// Lookahead: decoded, but belongs to a later output than the current one.
    pending: Option<(VideoFrame, Duration)>,
    eof: bool,
}

impl ClipSource {
    /// Opens `clip`'s source, seeking to its in-point.
    ///
    /// Decodes straight to rgba: the shared core's effect pass reads rgba, and the
    /// compositor and readback stay in one format (the encoder's own sws converts
    /// rgba -> yuv420p on push).
    fn open(clip: &Clip, frame_rate: f64) -> Result<Self, TimelineError> {
        let src = clip
            .source_path()
            .ok_or_else(|| TimelineError::TimelineRenderFailed {
                reason: "gpu export: clip lost its file source".to_string(),
            })?;
        let mut decoder = VideoDecoder::open(src)
            .output_format(PixelFormat::Rgba)
            .build()?;
        // Eligibility guarantees a positive, finite rate, so the conform maths is well
        // defined; fall back to the timeline rate defensively.
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
        Ok(Self {
            decoder,
            budget: budget_frames(clip, frame_rate),
            produced: 0,
            frame_rate,
            one_to_one: (src_fps - frame_rate).abs() <= 1e-3,
            base: clip.in_point.unwrap_or(Duration::ZERO),
            held: None,
            held_at: Duration::ZERO,
            pending: None,
            eof: false,
        })
    }

    /// Lets this clip yield `frames` more than its trimmed duration, so the transition
    /// window can read its handle.
    ///
    /// The handle is real material: `transition::effective_duration` clamped the window
    /// to what the source holds past the out-point, so this does not invent frames. A
    /// source that still runs out early stops the window, as it always did.
    fn allow_handle(&mut self, frames: u64) {
        if let Some(budget) = self.budget.as_mut() {
            *budget = budget.saturating_add(frames);
        }
    }

    /// The frame for this clip's next output, or `None` when the clip is finished --
    /// its budget is spent, or its source ran out first (a clip shorter than declared).
    fn next(&mut self) -> Result<Option<Pulled<'_>>, TimelineError> {
        if self.budget.is_some_and(|b| self.produced >= b) {
            return Ok(None);
        }
        if self.one_to_one {
            let Some(frame) = self.decoder.decode_one()? else {
                return Ok(None);
            };
            self.produced += 1;
            return Ok(Some(Pulled::Owned(frame)));
        }

        // Conform (#1660), PTS-driven: hold the newest source frame whose timestamp is
        // at or before this output's time, so a slower source repeats a frame and a
        // faster one skips frames while the clip keeps its on-screen duration.
        // Timestamps rather than a nominal rate, because the reported rate is not
        // trustworthy (see `clip_output_time`).
        let want = clip_output_time(self.produced, self.frame_rate);
        // Advance while the next source frame still starts at or before `want`; the last
        // such frame is the one this output shows.
        loop {
            if let Some((frame, at)) = self.pending.take() {
                if self.held.is_none() || at <= want {
                    self.held = Some(frame);
                    self.held_at = at;
                    continue;
                }
                self.pending = Some((frame, at));
                break;
            }
            if self.eof {
                break;
            }
            match self.decoder.decode_one()? {
                Some(frame) => {
                    let at = frame.timestamp().as_duration().saturating_sub(self.base);
                    self.pending = Some((frame, at));
                }
                None => self.eof = true,
            }
        }
        if self.held.is_none() {
            return Ok(None); // The clip decoded no frames at all.
        }
        // The source is spent and this output is past its last frame: the clip ends
        // here, matching the matching-rate path's "shorter than declared" stop.
        if self.eof && self.pending.is_none() && want > self.held_at {
            return Ok(None);
        }
        self.produced += 1;
        Ok(self.held.as_ref().map(Pulled::Held))
    }
}

/// Composites one clip's frame into the canvas, moving it in when the source has
/// finished with it.
fn composite_pulled(
    core: &mut GpuCompositor,
    layer: &VideoLayer,
    pulled: Pulled<'_>,
    canvas: (u32, u32),
    t: Duration,
) -> Option<(Vec<u8>, u32, u32)> {
    match pulled {
        Pulled::Owned(frame) => core.composite_owned(vec![(layer, frame)], canvas, t),
        Pulled::Held(frame) => core.composite(&[(layer, frame)], canvas, t),
    }
}

/// The error for a composite that fell back mid-export, which eligibility has already
/// precluded -- surfaced rather than allowed to become wrong output.
fn fell_back(what: &str) -> TimelineError {
    TimelineError::TimelineRenderFailed {
        reason: format!("gpu export: {what} fell back mid-export (precluded by eligibility)"),
    }
}

/// Drains an eligible single video track to the encoder on the GPU: decode each
/// clip's frames in order, composite each on the GPU, read it back, and push it to
/// the unchanged encoder. `on_progress` is invoked after each pushed frame;
/// returning `false` cancels with [`TimelineError::Cancelled`].
///
/// Clips are concatenated, and a transition changes none of that length (ADR-0009):
/// each clip runs its whole budget, and the window that follows is fed by the outgoing
/// clip's *handle* (frames past its out-point) blended against the incoming clip's head.
/// The incoming clip then resumes from where the window left it, so the track still runs
/// for the sum of the budgets -- the same total the CPU route now produces.
///
/// The caller has already established eligibility ([`eligible_track`]), so a
/// mid-export fallback from the compositor is a should-not-happen and surfaces as
/// [`TimelineError::TimelineRenderFailed`] rather than silent wrong output.
#[allow(clippy::too_many_arguments)]
/// One track, delivered one output frame at a time.
///
/// This is the per-clip loop the drain used to inline, lifted out so several tracks can
/// be advanced in step: the scheduler asks every track what it shows at output `k`, then
/// composites the stack. A track owns its own cuts and transition windows, which is also
/// the order the CPU uses -- `xfade` sits inside the track's chain and the overlay onto
/// the other tracks comes after it (`composition_inner.rs:541-610`).
struct TrackSource<'a> {
    track: &'a Track,
    canvas: (u32, u32),
    frame_rate: f64,
    /// The bottom of the stack. Only the base is pre-composited to the canvas; every
    /// other track hands the stack its raw decoded frame, so its effects, opacity, blend
    /// and placement are applied exactly once -- and it is *stretched* to the base's size
    /// by `layer_transform`, which is what the CPU's `scale = canvas * (sx, sy)` does. A
    /// solo composite would instead letterbox it and bake that in (measured: a 64x32
    /// overlay on a 64x64 canvas landed 32x16 letterboxed where the CPU had 32x32).
    is_base: bool,
    /// `transition::effective_durations`, resolved once for the track: each boundary is
    /// both a clip's own transition and its predecessor's handle, and resolving it per
    /// clip would probe the same source twice.
    boundaries: Vec<Duration>,
    clip_idx: usize,
    cur: ClipSource,
    cur_layer: VideoLayer,
    /// The layer handed to the stack composite: [`composited_base_layer`] for the base,
    /// and `cur_layer` itself for every other track. Kept in step with `cur_layer`.
    stack_layer: VideoLayer,
    /// The incoming clip, open only while a transition window is running.
    inc: Option<(ClipSource, VideoLayer)>,
    node: Option<GpuTransition>,
    window: u64,
    window_pos: u64,
    done: bool,
    /// The size of the last frame this track produced, so an ended track can stand
    /// in with a transparent frame of the same shape (see [`drain_video_gpu`]).
    last_dims: Option<(u32, u32)>,
}

impl<'a> TrackSource<'a> {
    /// Opens the track's first clip, or `None` when it has none.
    fn open(
        track: &'a Track,
        is_base: bool,
        canvas: (u32, u32),
        frame_rate: f64,
    ) -> Result<Option<Self>, TimelineError> {
        let Some(first) = track.clips.first() else {
            return Ok(None);
        };
        let cur_layer = transitionless_layer(first, track, canvas);
        Ok(Some(Self {
            track,
            canvas,
            frame_rate,
            is_base,
            boundaries: crate::transition::effective_durations(&track.clips),
            clip_idx: 0,
            cur: ClipSource::open(first, frame_rate)?,
            stack_layer: stack_layer_for(is_base, &cur_layer),
            cur_layer,
            inc: None,
            node: None,
            window: 0,
            window_pos: 0,
            done: false,
            last_dims: None,
        }))
    }

    /// The layer this track's current content is placed with in the stack composite.
    fn layer(&self) -> &VideoLayer {
        &self.stack_layer
    }

    /// Recomputes [`Self::stack_layer`] after a cut changed `cur_layer`.
    fn refresh_stack_layer(&mut self) {
        self.stack_layer = stack_layer_for(self.is_base, &self.cur_layer);
    }

    /// Opens the window into the next clip, or reports that the track is finished.
    ///
    /// Called once the current clip has spent its budget. A zero-length window is a hard
    /// cut, which advances immediately.
    fn advance(&mut self, core: &mut GpuCompositor) -> Result<bool, TimelineError> {
        let Some(next) = self.track.clips.get(self.clip_idx + 1) else {
            self.done = true;
            return Ok(false);
        };
        let window = transition_window(next, self.boundaries[self.clip_idx + 1], self.frame_rate)?;
        // Past the out-point for the length of the window: those frames are the handle
        // the blend reads, not part of the clip's on-screen duration.
        self.cur.allow_handle(window);
        let inc = ClipSource::open(next, self.frame_rate)?;
        let inc_layer = transitionless_layer(next, self.track, self.canvas);
        // Start each clip with a clean effect cache: a stateful effect (MotionBlur's
        // exposure trail) must not accumulate across a cut into the next clip (RK-025).
        core.reset_effect_cache();
        self.node = next.transition.and_then(map_transition);
        self.window = window;
        self.window_pos = 0;
        if window == 0 {
            self.cur = inc;
            self.cur_layer = inc_layer;
            self.refresh_stack_layer();
            self.clip_idx += 1;
        } else {
            self.inc = Some((inc, inc_layer));
        }
        Ok(true)
    }

    /// Ends the window early or on completion, promoting the incoming clip.
    fn close_window(&mut self) {
        if let Some((inc, inc_layer)) = self.inc.take() {
            self.cur = inc;
            self.cur_layer = inc_layer;
            self.refresh_stack_layer();
            self.clip_idx += 1;
        }
        self.window = 0;
        self.window_pos = 0;
        self.node = None;
    }

    /// This track's content for the output at `t`, or `None` when the track has ended.
    ///
    /// Inside a transition window both clips are composited to the canvas and blended,
    /// which is what the CPU does for the **base** track. Eligibility keeps transitions
    /// off the other tracks precisely because that equivalence does not hold there.
    fn next(
        &mut self,
        core: &mut GpuCompositor,
        t: Duration,
    ) -> Result<Option<VideoFrame>, TimelineError> {
        loop {
            if self.done {
                return Ok(None);
            }
            if self.window_pos < self.window && self.inc.is_some() {
                if let Some(frame) = self.blend_one(core, t)? {
                    return Ok(Some(frame));
                }
                // A source ran out inside the window; finish with the incoming clip, as
                // the sequential drain did.
                self.close_window();
                continue;
            }
            if self.window_pos >= self.window && self.inc.is_some() {
                self.close_window();
                continue;
            }
            match self.cur.next()? {
                Some(pulled) => {
                    if !self.is_base {
                        // Straight through: the stack composite is this track's only
                        // pass, so it applies the layer once. Compositing here first
                        // would apply the effects, opacity and blend twice over
                        // (measured: an overlay at opacity 0.5 read 51 where the CPU
                        // read 140) and bake a letterbox in on top.
                        let frame = match pulled {
                            Pulled::Owned(frame) => frame,
                            Pulled::Held(frame) => frame.clone(),
                        };
                        self.last_dims = Some((frame.width(), frame.height()));
                        return Ok(Some(frame));
                    }
                    let composited =
                        composite_pulled(core, &self.cur_layer, pulled, self.canvas, t)
                            .ok_or_else(|| fell_back("a track's clip"))?;
                    self.last_dims = Some((composited.1, composited.2));
                    return Ok(Some(wrap_rgba(composited)?));
                }
                None => {
                    if !self.advance(core)? {
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// One frame of a transition window: composite both clips and blend them.
    ///
    /// Reached on the **base** track only -- eligibility declines a transition on any
    /// other track, because the CPU scales each clip to its placed size before `xfade`
    /// while the stack places after the blend (#1768).
    ///
    /// `None` means a source ran out early, which ends the window.
    fn blend_one(
        &mut self,
        core: &mut GpuCompositor,
        t: Duration,
    ) -> Result<Option<VideoFrame>, TimelineError> {
        let Some(node) = self.node else {
            return Err(TimelineError::TimelineRenderFailed {
                reason: "gpu export: transitioned clip lost its GPU node".to_string(),
            });
        };
        let Some(outgoing) = self.cur.next()? else {
            return Ok(None);
        };
        let (a_rgba, w, h) = composite_pulled(core, &self.cur_layer, outgoing, self.canvas, t)
            .ok_or_else(|| fell_back("the outgoing clip"))?;
        let Some((inc, inc_layer)) = self.inc.as_mut() else {
            return Ok(None);
        };
        let Some(incoming) = inc.next()? else {
            // The incoming clip ran out early. The outgoing frame just composited goes
            // unused: it belonged to this output, which now has nothing to blend it
            // with. Only reachable when a source is shorter than declared, since
            // eligibility bounds the window by the incoming clip's budget.
            return Ok(None);
        };
        let (b_rgba, _, _) = composite_pulled(core, inc_layer, incoming, self.canvas, t)
            .ok_or_else(|| fell_back("the incoming clip"))?;
        #[allow(clippy::cast_precision_loss)] // window frame counts fit the mantissa
        let progress = self.window_pos as f32 / self.window as f32;
        let blended = core
            .transition(node, progress, &a_rgba, b_rgba, w, h)
            .ok_or_else(|| TimelineError::TimelineRenderFailed {
                reason: format!("gpu export: the transition blend failed at progress {progress}"),
            })?;
        self.window_pos += 1;
        self.last_dims = Some((w, h));
        Ok(Some(wrap_rgba((blended, w, h))?))
    }
}

/// The base track's layer for the stack pass, with everything its own pass already
/// applied stripped out.
///
/// Only the base is composited to the canvas before the stack (it has to be: a
/// transition blends two canvas frames, and the stack's placement reference is the base's
/// size, which must stay the canvas whether or not a window is open). That pass applies
/// its effects, opacity and blend mode, so the stack pass must not apply them a second
/// time. Its placement is ignored at index 0 either way, so the neutral shell is all the
/// stack needs.
fn stack_layer_for(is_base: bool, cur_layer: &VideoLayer) -> VideoLayer {
    if is_base {
        composited_base_layer(cur_layer)
    } else {
        cur_layer.clone()
    }
}

/// See [`stack_layer_for`]: the base's half of that choice.
fn composited_base_layer(layer: &VideoLayer) -> VideoLayer {
    let mut neutral = layer.clone();
    neutral.effects.clear();
    neutral.opacity = AnimatedValue::Static(1.0);
    neutral.blend_mode = BlendMode::Normal;
    neutral.composite_op = CompositeOp::Over;
    neutral
}

/// A fully transparent `w` x `h` rgba frame: the stand-in for a track that has ended.
///
/// Allocated per call rather than pooled: the replacement would be a clone of a cached
/// buffer, which is not cheaper than the zeroed allocation here, and it runs once per
/// ended track against a readback and an encode on the same output frame. Real pooling
/// belongs with the frame pool, not with a special case for placeholders.
fn transparent_frame(w: u32, h: u32) -> Result<VideoFrame, TimelineError> {
    VideoFrame::from_rgba(w, h, vec![0u8; (w as usize) * (h as usize) * 4]).map_err(|e| {
        TimelineError::TimelineRenderFailed {
            reason: format!("gpu export: could not build a placeholder frame: {e}"),
        }
    })
}

/// Wraps a composited rgba read-back as a [`VideoFrame`] so it can join the stack.
fn wrap_rgba((rgba, w, h): (Vec<u8>, u32, u32)) -> Result<VideoFrame, TimelineError> {
    VideoFrame::from_rgba(w, h, rgba).map_err(|e| TimelineError::TimelineRenderFailed {
        reason: format!("gpu export: could not wrap a composited frame: {e}"),
    })
}

/// Drives the eligible tracks through decode -> GPU composite -> readback -> encode.
///
/// `tracks` is bottom to top. Each output frame asks every track for its content, then
/// composites the z-ordered stack in one pass; a track that has ended contributes
/// nothing further.
///
/// **The export ends when the topmost track ends**, not when the longest does. That is
/// the CPU's rule, not a choice made here: the last overlay is built with
/// `eof_action=endall` (`composition_inner.rs:930-936`), so the graph terminates with it.
/// Measured on the CPU route -- a 15-frame base under a 6-frame overlay exports 5 frames,
/// and the mirror exports 14.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drain_video_gpu(
    tracks: &[&Track],
    canvas: (u32, u32),
    frame_rate: f64,
    encoder: &mut VideoEncoder,
    core: &mut GpuCompositor,
    on_progress: &(impl Fn(&Progress) -> bool + Send),
    start: Instant,
    total_frames: Option<u64>,
) -> Result<(), TimelineError> {
    let mut sources: Vec<TrackSource<'_>> = Vec::with_capacity(tracks.len());
    for track in tracks {
        let is_base = sources.is_empty();
        if let Some(ts) = TrackSource::open(track, is_base, canvas, frame_rate)? {
            sources.push(ts);
        }
    }
    let Some(top) = sources.len().checked_sub(1) else {
        return Ok(());
    };
    core.reset_effect_cache();

    let mut video_idx: u32 = 0;
    loop {
        let t = output_time(video_idx, frame_rate);
        // Pull every track first, then decide: a track's **stack position** is what the
        // compositor reads as "which layer is the base" (`gpu_compositor::layer_transform`),
        // so an ended track may not be dropped from the vector -- that would promote an
        // overlay to base and silently discard its placement.
        let mut pulled: Vec<Option<VideoFrame>> = Vec::with_capacity(sources.len());
        for ts in &mut sources {
            pulled.push(ts.next(core, t)?);
        }
        // The export ends with the **topmost** track, whatever the others are doing.
        if pulled[top].is_none() {
            break;
        }
        // A track that has ended still occupies its slot, contributing nothing visible.
        // Measured on the CPU route: with a 6-frame base under a 15-frame overlay, the
        // base's area goes **black** from frame 6 while the overlay keeps rendering, so
        // letting the canvas show through is exactly right.
        let mut layers: Vec<(&VideoLayer, VideoFrame)> = Vec::with_capacity(sources.len());
        for (ts, frame) in sources.iter().zip(pulled) {
            // A track that has produced nothing yet has no shape of its own to stand in
            // with, so it stands in at canvas size. Dropping it instead would shrink the
            // vector and shift every track above it down a slot, which is exactly the
            // promotion the comment above forbids.
            let frame = if let Some(f) = frame {
                f
            } else {
                let (w, h) = ts.last_dims.unwrap_or(canvas);
                transparent_frame(w, h)?
            };
            layers.push((ts.layer(), frame));
        }
        // Each track already composited its own content to the canvas, so a lone track
        // is finished and needs no second pass -- that keeps the single-track export at
        // exactly the cost it had before the scheduler. A stack goes through the
        // compositor once more to place and blend the layers.
        let composited = if layers.len() == 1 {
            let (_, frame) = layers.pop().unwrap_or_else(|| unreachable!());
            let (w, h) = (frame.width(), frame.height());
            (frame.data(), w, h)
        } else {
            core.composite_owned(layers, canvas, t)
                .ok_or_else(|| fell_back("the track stack"))?
        };
        emit_frame(
            Some(composited),
            encoder,
            &mut video_idx,
            on_progress,
            start,
            total_frames,
        )?;
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
/// several outputs), and so the transition window can pass its blended frame through
/// the same push. A `None` means a frame fell back mid-export, which eligibility has
/// already precluded, so it surfaces as an error rather than wrong output.
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

    use ff_filter::{BlendMode, FilterStep, XfadeTransition};
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

    /// A two-track timeline on the same square canvas, base first.
    fn two_track_timeline(base: Vec<Clip>, over: Vec<Clip>) -> Timeline {
        Timeline::builder()
            .canvas(64, 64)
            .frame_rate(30.0)
            .video_track(base)
            .video_track(over)
            .build()
            .unwrap()
    }

    #[test]
    fn eligible_tracks_should_accept_two_active_video_tracks() {
        // #1633: a second active track used to keep the whole export on the CPU. The
        // returned indices are the stack, bottom to top -- the order the scheduler
        // composites in, and what decides which track is the base.
        let src = std::env::temp_dir().join("avio_eligible_mt_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let t = two_track_timeline(
            vec![Clip::new(&src)],
            vec![Clip::new(&src).with_position(10.0, 4.0).with_scale(0.5)],
        );
        assert_eq!(
            eligible(&t),
            Some(vec![0, 1]),
            "two file-source tracks must both route to the GPU"
        );
    }

    #[test]
    fn eligible_tracks_should_reject_a_transition_on_a_non_base_track() {
        // The CPU scales each clip to its placed size *before* `xfade`, while the GPU
        // places after the blend. For the base that is the same thing (its transform is
        // ignored); for an overlay it is not, and no measurement backs a reordering, so
        // it falls back rather than approximate (RK-020).
        let src = std::env::temp_dir().join("avio_eligible_mt_xfade_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let over = vec![
            placed(src.to_str().unwrap(), 0.0, 1.0),
            placed(src.to_str().unwrap(), 1.0, 1.0)
                .with_transition(XfadeTransition::Fade, Duration::from_millis(200)),
        ];
        let t = two_track_timeline(vec![placed(src.to_str().unwrap(), 0.0, 2.0)], over);
        assert_eq!(
            eligible(&t),
            None,
            "a transition on an overlay track must keep the export on the CPU"
        );
    }

    #[test]
    fn eligible_tracks_should_still_accept_a_transition_on_the_base_track() {
        // The mirror of the test above, and the regression guard for #1659: the shipped
        // single-track transition support must survive the restriction added for
        // overlays.
        let src = std::env::temp_dir().join("avio_eligible_mt_base_xfade_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let base = vec![
            placed(src.to_str().unwrap(), 0.0, 1.0),
            placed(src.to_str().unwrap(), 1.0, 1.0)
                .with_transition(XfadeTransition::Fade, Duration::from_millis(200)),
        ];
        let t = two_track_timeline(base, vec![placed(src.to_str().unwrap(), 0.0, 2.0)]);
        assert_eq!(
            eligible(&t),
            Some(vec![0, 1]),
            "a transition on the base track is still rendered by the drain"
        );
    }

    #[test]
    fn eligible_tracks_should_reject_a_stateful_effect_once_a_second_track_is_stacked() {
        // A MotionBlur trail lives in the cached effect graph, and a stacked export makes
        // the compositor alternate between a one-layer solo composite and an N-layer
        // stack composite every output frame, which evicts that cache each time
        // (RK-025). The trail would restart every frame instead of accumulating, so the
        // export declines rather than rendering a blur the CPU never produces.
        let src = std::env::temp_dir().join("avio_eligible_mt_stateful_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let blurred = || {
            Clip::new(&src).with_video_effect(FilterStep::MotionBlur {
                shutter_angle_degrees: 180.0,
                sub_frames: 4,
            })
        };
        assert_eq!(
            eligible(&square_timeline(vec![blurred()])),
            Some(vec![0]),
            "the control must be eligible on its own, or this test proves nothing"
        );
        let t = two_track_timeline(vec![blurred()], vec![Clip::new(&src)]);
        let stacked = eligible(&t);
        let _ = std::fs::remove_file(&src);
        assert_eq!(
            stacked, None,
            "a stateful effect must keep a stacked export on the CPU"
        );
    }

    #[test]
    fn eligible_tracks_should_still_accept_a_stateless_effect_when_stacked() {
        // The other half of the gate above: only a *stateful* node is rejected, so the
        // rejection stays attributable to the cache interaction rather than reading as a
        // blanket ban on effects in a stacked export.
        let src = std::env::temp_dir().join("avio_eligible_mt_stateless_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let t = two_track_timeline(
            vec![Clip::new(&src).with_video_effect(FilterStep::Hue { degrees: 60.0 })],
            vec![Clip::new(&src).with_opacity(0.5)],
        );
        let stacked = eligible(&t);
        let _ = std::fs::remove_file(&src);
        assert_eq!(
            stacked,
            Some(vec![0, 1]),
            "a stateless effect and an overlay opacity are both rendered by the stack"
        );
    }

    #[test]
    fn eligible_tracks_should_reject_when_any_track_is_ineligible() {
        // The fallback is whole-frame, so one bad track disqualifies the export rather
        // than compositing a partial stack. A non-unity speed on the *overlay* is the
        // cheapest way to make exactly one track fail.
        let src = std::env::temp_dir().join("avio_eligible_mt_bad_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let good = two_track_timeline(vec![Clip::new(&src)], vec![Clip::new(&src)]);
        assert!(
            eligible(&good).is_some(),
            "the control case must be eligible, or this test proves nothing"
        );
        let t = two_track_timeline(
            vec![Clip::new(&src)],
            vec![{
                let mut c = Clip::new(&src);
                c.speed = 2.0;
                c
            }],
        );
        assert_eq!(eligible(&t), None, "one ineligible track fails the export");
    }

    fn eligible(timeline: &Timeline) -> Option<Vec<usize>> {
        eligible_tracks(
            &timeline.video_tracks,
            timeline.lavfi_overlay.as_deref(),
            timeline.video_tracks.iter().any(|t| t.solo),
            (timeline.canvas_width, timeline.canvas_height),
            timeline.frame_rate,
        )
    }

    /// A clip of `secs` seconds starting at `at`, so a track built from these tiles the
    /// timeline and clears the contiguity pass -- what the transition cases need, since
    /// a bare `Clip::new` has no duration and is rejected before the transition is ever
    /// looked at.
    fn placed(path: &str, at: f64, secs: f64) -> Clip {
        Clip::new(path)
            .offset(Duration::from_secs_f64(at))
            .trim(Duration::ZERO, Duration::from_secs_f64(secs))
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

    /// Encodes a tiny `w` x `h` video at `fps`, or `None` when the environment has no
    /// usable encoder (skip). The probe pass needs a real file, so eligibility cannot
    /// be exercised without one.
    fn encode_probe_source(path: &std::path::Path, w: u32, h: u32, fps: f64) -> Option<()> {
        use ff_encode::{VideoCodec, VideoEncoder};
        use ff_format::{PixelFormat as PF, VideoFrame};

        let mut enc = VideoEncoder::create(path)
            .video(w, h, fps)
            .video_codec(VideoCodec::Mpeg4)
            .build()
            .ok()?;
        // Two seconds' worth. The header (size, rate) would fit in a handful of
        // frames, but eligibility also asks how much material sits past a clip's
        // out-point (ADR-0009), and a 4-frame file has none -- which would make every
        // transition here clamp to a hard cut and reject.
        for i in 0..60 {
            let frame = VideoFrame::new_black(w, h, PF::Yuv420p, i);
            enc.push_video(&frame).ok()?;
        }
        enc.finish().ok()?;
        Some(())
    }

    /// Encodes `src` and confirms it can be read back, so a probe-gated eligibility test
    /// can tell "this environment cannot run the check" (skip) from "the gate rejected
    /// the source" (fail). Minimal-`FFmpeg` CI has the `Mpeg4` encoder but not always the
    /// decoder the probe pass opens (RK-002), and gating on the encoder alone reads that
    /// miss as a rejection.
    fn probe_source_or_skip(src: &std::path::Path, w: u32, h: u32, fps: f64) -> bool {
        let _ = std::fs::remove_file(src);
        if encode_probe_source(src, w, h, fps).is_none() {
            return false;
        }
        if VideoDecoder::open(src).build().is_err() {
            let _ = std::fs::remove_file(src);
            return false;
        }
        true
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
        if !probe_source_or_skip(&src, 64, 64, 24.0) {
            return;
        }
        let t = square_timeline(vec![Clip::new(&src)]); // canvas 64x64, timeline 30 fps
        let eligible_now = eligible(&t);
        let _ = std::fs::remove_file(&src);
        assert_eq!(
            eligible_now,
            Some(vec![0]),
            "a 24 fps source in a 30 fps timeline must be GPU-eligible after #1660"
        );
    }

    #[test]
    fn eligible_track_should_accept_a_source_whose_aspect_differs_from_the_canvas() {
        // #1661: the probe pass no longer requires the source aspect to match the canvas
        // — the shared compositing core letterboxes it — so a 16:9 source on a square
        // canvas stays on the GPU route instead of falling back to CPU. This is the
        // direct evidence the gate opened: the end-to-end export cannot show it, because
        // the CPU fallback letterboxes too and would produce the same picture.
        let src = std::env::temp_dir().join("avio_eligible_169_probe.mp4");
        if !probe_source_or_skip(&src, 64, 36, 30.0) {
            return;
        }
        let t = square_timeline(vec![Clip::new(&src)]); // canvas 64x64
        let eligible_now = eligible(&t);
        let _ = std::fs::remove_file(&src);
        assert_eq!(
            eligible_now,
            Some(vec![0]),
            "a 16:9 source on a square canvas must be GPU-eligible after #1661"
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
    fn window_frames_should_match_the_cpu_route_measurement() {
        // The number the whole transition path is built on: a 0.5 s transition at 30 fps
        // blends across 15 outputs. It comes out of neither clip's body -- the outgoing
        // clip's handle feeds it -- so two 1 s clips still produce 60 frames, the
        // hard-cut length (ADR-0009). Before that decision the same 15 frames were
        // *subtracted*, giving 45.
        let window = window_frames(Duration::from_millis(500), 30.0);
        assert_eq!(window, 15);
        assert_eq!(30 + 30, 60);
    }

    #[test]
    fn window_frames_should_round_a_sub_frame_duration_to_zero() {
        // The value `eligible_transition`'s `window >= 1` check keys off: a transition
        // too short to own an output frame has nothing to blend.
        assert_eq!(window_frames(Duration::from_millis(10), 30.0), 0);
    }

    #[test]
    fn export_maps_to_gpu_should_accept_every_libm_independent_kind() {
        // #1732 brought each node onto `FFmpeg`'s own formula, so the export no longer
        // holds back the kinds whose agreement is pure arithmetic. Before that only
        // `Fade` agreed with the CPU export, because the nodes were pinned to a reference
        // that had itself drifted.
        for kind in [
            XfadeTransition::Fade,
            XfadeTransition::WipeLeft,
            XfadeTransition::WipeRight,
            XfadeTransition::WipeUp,
            XfadeTransition::WipeDown,
            XfadeTransition::FadeBlack,
            XfadeTransition::FadeWhite,
        ] {
            assert!(
                export_maps_to_gpu(kind),
                "{kind:?} agrees with the CPU export and must render on the GPU"
            );
        }
    }

    #[test]
    fn export_maps_to_gpu_should_reject_dissolve_despite_it_mapping() {
        // The one kind that maps to a node and still stays on the CPU. Its selection is
        // `sinf` of a large argument, so which pixels turn over depends on the libm: the
        // GPU route uses Rust's and the CPU route FFmpeg's, and they agree on Windows
        // (worst-frame mean 3.6 between the routes) but not macOS (6.6). Rendering it
        // would give a viewer different noise depending on the route they took.
        assert!(
            map_transition(XfadeTransition::Dissolve).is_some(),
            "Dissolve still maps to a node -- the preview and the parity suites use it"
        );
        assert!(
            !export_maps_to_gpu(XfadeTransition::Dissolve),
            "Dissolve must stay on the CPU export"
        );
    }

    #[test]
    fn export_maps_to_gpu_should_reject_a_kind_with_no_node() {
        // The other half: a kind with no faithful node still keeps the whole export on
        // the CPU rather than being approximated by one that merely looks similar.
        for kind in [
            XfadeTransition::SlideLeft,
            XfadeTransition::CircleOpen,
            XfadeTransition::FadeGrays,
            XfadeTransition::Pixelize,
        ] {
            assert!(!export_maps_to_gpu(kind), "{kind:?} has no GPU node");
        }
    }

    #[test]
    fn eligible_track_should_accept_a_fade_into_the_last_clip() {
        // #1659: the structural pass no longer rejects every transition. Probe-backed
        // because eligibility ends in the probe pass, which needs a real file -- and
        // because this test is what proves the *route* is taken: `render()` falls back
        // silently, so the end-to-end parity test alone could not tell a GPU export from
        // a CPU one.
        let src = std::env::temp_dir().join("avio_eligible_fade_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let path = src.to_string_lossy().into_owned();
        let t = square_timeline(vec![
            placed(&path, 0.0, 1.0),
            placed(&path, 1.0, 1.0)
                .with_transition(XfadeTransition::Fade, Duration::from_millis(500)),
        ]);
        let eligible_now = eligible(&t);
        let _ = std::fs::remove_file(&src);
        assert_eq!(
            eligible_now,
            Some(vec![0]),
            "a Fade into the last clip must be GPU-eligible after #1659"
        );
    }

    #[test]
    fn eligible_track_should_accept_a_transition_on_a_middle_clip() {
        // This used to be rejected, and had to be: the CPU route placed a clip *after* a
        // transitioned one at its own absolute offset while the xfade output had shrunk,
        // opening a hole (measured: 15 black frames). Reproducing that here would have
        // fixed the bug in place. ADR-0009 removed the shrink, so the restriction has
        // nothing left to guard and a middle-clip transition belongs on the GPU route
        // like any other (#1731).
        //
        // Probe-backed: eligibility ends in the probe pass, and the transition pass now
        // asks the source for its handle, so fake paths would reject for the wrong
        // reason and this test would pass without asserting anything.
        let src = std::env::temp_dir().join("avio_eligible_middle_tr_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let path = src.to_string_lossy().into_owned();
        let t = square_timeline(vec![
            placed(&path, 0.0, 1.0),
            placed(&path, 1.0, 1.0)
                .with_transition(XfadeTransition::Fade, Duration::from_millis(500)),
            placed(&path, 2.0, 1.0),
        ]);
        let eligible_now = eligible(&t);
        let _ = std::fs::remove_file(&src);
        assert_eq!(
            eligible_now,
            Some(vec![0]),
            "a transition on a middle clip must be GPU-eligible once placement preserves \
             the timeline length"
        );
    }

    #[test]
    fn eligible_track_should_reject_a_transition_with_no_handle_to_feed_it() {
        // The clamp seen from eligibility: a clip trimmed flush to the end of its source
        // has nothing past its out-point, so the effective duration is zero and the
        // window rounds to no frames. Both routes render a hard cut there, and the GPU
        // one declines rather than blend across a window it cannot fill.
        let src = std::env::temp_dir().join("avio_eligible_no_handle_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let path = src.to_string_lossy().into_owned();
        let Ok(info) = ff_probe::open(&src) else {
            let _ = std::fs::remove_file(&src);
            return;
        };
        let flush = info.duration().as_secs_f64();
        let t = square_timeline(vec![
            placed(&path, 0.0, flush),
            placed(&path, flush, 1.0)
                .with_transition(XfadeTransition::Fade, Duration::from_millis(500)),
        ]);
        let eligible_now = eligible(&t);
        let _ = std::fs::remove_file(&src);
        assert!(
            eligible_now.is_none(),
            "with no handle the transition clamps to a hard cut, which the GPU route \
             leaves to the CPU one"
        );
    }

    #[test]
    fn eligible_track_should_ignore_a_transition_on_the_first_clip() {
        // `derive` drops a transition that has no preceding clip to cross-fade from, so
        // the CPU route renders a plain clip; the drain's `transitionless_layer` does the
        // same. Eligibility must therefore not reject on it.
        let src = std::env::temp_dir().join("avio_eligible_first_tr_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let path = src.to_string_lossy().into_owned();
        let t = square_timeline(vec![
            placed(&path, 0.0, 1.0)
                .with_transition(XfadeTransition::Fade, Duration::from_millis(500)),
        ]);
        let eligible_now = eligible(&t);
        let _ = std::fs::remove_file(&src);
        assert_eq!(eligible_now, Some(vec![0]));
    }

    #[test]
    fn eligible_track_should_reject_a_transition_kind_with_no_gpu_node() {
        // `SlideLeft` needs a translating sampler no node provides, so the whole export
        // stays on the CPU rather than rendering something else.
        let t = square_timeline(vec![
            placed("a.mp4", 0.0, 1.0),
            placed("b.mp4", 1.0, 1.0)
                .with_transition(XfadeTransition::SlideLeft, Duration::from_millis(500)),
        ]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_accept_a_transition_longer_than_the_outgoing_clip_body() {
        // This used to be rejected, because the window was taken *out of* the outgoing
        // clip and a 0.3 s clip has no 0.5 s to give. The window now comes from the
        // handle instead (ADR-0009), so the clip's on-screen length stops being a bound
        // and only the source's material past the out-point matters -- which this
        // 2-second fixture has plenty of.
        let src = std::env::temp_dir().join("avio_eligible_short_body_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let path = src.to_string_lossy().into_owned();
        let t = square_timeline(vec![
            placed(&path, 0.0, 0.3),
            placed(&path, 0.3, 1.0)
                .with_transition(XfadeTransition::Fade, Duration::from_millis(500)),
        ]);
        let eligible_now = eligible(&t);
        let _ = std::fs::remove_file(&src);
        assert_eq!(
            eligible_now,
            Some(vec![0]),
            "the outgoing clip's body no longer bounds the window; its handle does"
        );
    }

    #[test]
    fn eligible_track_should_reject_a_sub_frame_transition() {
        // A window of zero frames has nothing to blend (RK-020: the degenerate corner of
        // a reproduced formula is where silent wrong output comes from).
        let t = square_timeline(vec![
            placed("a.mp4", 0.0, 1.0),
            placed("b.mp4", 1.0, 1.0)
                .with_transition(XfadeTransition::Fade, Duration::from_millis(10)),
        ]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_reject_a_transition_into_a_clip_of_unknown_duration() {
        // Without a duration the window cannot be checked against the incoming clip up
        // front, only discovered at EOF -> CPU.
        let t = square_timeline(vec![
            placed("a.mp4", 0.0, 1.0),
            Clip::new("b.mp4")
                .offset(Duration::from_secs(1))
                .with_transition(XfadeTransition::Fade, Duration::from_millis(500)),
        ]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_reject_a_transition_beside_a_stateful_effect() {
        // The window composites both clips at the same layer position, so their cached
        // effect graphs evict each other every frame -- restarting a MotionBlur trail on
        // both (RK-025). Only a stateful node cares, so only it is gated.
        let t = square_timeline(vec![
            placed("a.mp4", 0.0, 1.0).with_video_effect(FilterStep::MotionBlur {
                shutter_angle_degrees: 180.0,
                sub_frames: 4,
            }),
            placed("b.mp4", 1.0, 1.0)
                .with_transition(XfadeTransition::Fade, Duration::from_millis(500)),
        ]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_reject_a_transition_beside_a_transparent_clip() {
        // The window composites each clip alone and *then* blends, while the CPU route
        // blends first and composites the result. A partially transparent clip does not
        // survive that reordering: it reaches the blend already darkened against the
        // canvas, where the CPU's `xfade` would have mixed its full-strength RGB.
        // Measured on a 0.5 s Fade: luma diverged by 26 at opacity 0.5 (42 animated),
        // inside the window only. Nothing panics and no frame falls back, so only this
        // gate stands between that and a silently wrong export (RK-020).
        let t = square_timeline(vec![
            placed("a.mp4", 0.0, 1.0),
            placed("b.mp4", 1.0, 1.0)
                .with_opacity(0.5)
                .with_transition(XfadeTransition::Fade, Duration::from_millis(500)),
        ]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_reject_a_transition_beside_a_non_normal_blend() {
        // Same reordering, other axis: a blend mode composes against the canvas in the
        // same place opacity does, so it cannot survive the solo composite either.
        let t = square_timeline(vec![
            placed("a.mp4", 0.0, 1.0).with_blend_mode(BlendMode::Multiply),
            placed("b.mp4", 1.0, 1.0)
                .with_transition(XfadeTransition::Fade, Duration::from_millis(500)),
        ]);
        assert!(eligible(&t).is_none());
    }

    #[test]
    fn eligible_track_should_accept_a_transparent_clip_without_a_transition() {
        // The other half of the two gates above: opacity alone is fine, because without
        // a transition nothing blends after the solo composite. Keeps the rejections
        // attributable to the transition rather than reading as a blanket ban.
        let src = std::env::temp_dir().join("avio_eligible_opacity_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let path = src.to_string_lossy().into_owned();
        let t = square_timeline(vec![
            placed(&path, 0.0, 1.0).with_opacity(0.5),
            placed(&path, 1.0, 1.0),
        ]);
        let eligible_now = eligible(&t);
        let _ = std::fs::remove_file(&src);
        assert_eq!(eligible_now, Some(vec![0]));
    }

    #[test]
    fn eligible_track_should_accept_a_stateful_effect_without_a_transition() {
        // The other half of the gate above: MotionBlur alone is fine (the drain resets
        // the effect cache at each clip boundary), so the rejection above is the
        // transition's doing and not a blanket ban.
        let src = std::env::temp_dir().join("avio_eligible_motionblur_probe.mp4");
        if !probe_source_or_skip(&src, 64, 64, 30.0) {
            return;
        }
        let path = src.to_string_lossy().into_owned();
        let t = square_timeline(vec![
            placed(&path, 0.0, 1.0).with_video_effect(FilterStep::MotionBlur {
                shutter_angle_degrees: 180.0,
                sub_frames: 4,
            }),
            placed(&path, 1.0, 1.0),
        ]);
        let eligible_now = eligible(&t);
        let _ = std::fs::remove_file(&src);
        assert_eq!(eligible_now, Some(vec![0]));
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
    fn eligible_tracks_should_reject_a_source_that_cannot_be_opened() {
        // This assertion used to read "a second active track -> CPU", which #1633 lifted.
        // It kept passing afterwards for the wrong reason: the fixture names files that
        // do not exist, so the probe pass rejected them whatever the track count was.
        // Pinning what it actually exercises is more useful than deleting it -- an
        // unopenable source must fall back rather than fail mid-export.
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
