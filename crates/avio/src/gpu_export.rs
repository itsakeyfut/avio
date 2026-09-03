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
//! v1 handles only a **single active video track** at unity speed whose every clip is a
//! file source that maps to the GPU with an identity transform. A source whose frame
//! rate differs from the timeline's is conformed by the drain (#1660), repeating or
//! skipping source frames so the clip keeps its on-screen duration, and one whose aspect
//! differs from the canvas is letterboxed by the shared compositing core (#1661). The
//! clips are otherwise hard cuts, except for a single **cross-fade into the track's last
//! clip** (#1659). Anything else keeps the whole export on the CPU
//! `MultiTrackComposer` path (see [`eligible_track`]); multi-track / overlay GPU export
//! is a follow-up.

use std::time::{Duration, Instant};

use ff_decode::{SeekMode, VideoDecoder};
use ff_encode::VideoEncoder;
use ff_filter::{AnimatedValue, VideoLayer, XfadeTransition};
use ff_format::{PixelFormat, VideoFrame};
use ff_pipeline::Progress;
use ff_render::BlendMode as RenderBlendMode;

use crate::clip::Clip;
use crate::derive;
use crate::error::TimelineError;
use crate::gpu::{GpuEffect, GpuLayerPlan, GpuMapping, map_scene};
use crate::gpu_compositor::GpuCompositor;
use crate::gpu_transition::map_transition;
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
/// index of the single eligible video track, or `None` to keep the whole export on
/// the CPU `MultiTrackComposer` path.
///
/// v1 is deliberately narrow (structural, transition and contiguity checks are I/O-free;
/// only the probe pass reads each source):
/// - no lavfi overlay (a second compositing layer v1 does not handle),
/// - exactly one **active** video track with at least one clip,
/// - every clip is a **file** source (a generated Solid/Text source has no decoder
///   here; it renders via lavfi on the CPU path),
/// - unity speed (the drain conforms frame *rate* but does not resample time),
/// - each clip's derived [`VideoLayer`] maps to [`GpuMapping::Gpu`] (a supported
///   blend / composite / effect set) with a **static, neutral transform** (RK-020:
///   the model's pixels/degrees are not `ff_render`'s UV/radians, so any placement
///   falls back),
/// - at most one transition, on the track's **last** clip, of a kind the GPU renders
///   the same way the CPU export does (see [`export_maps_to_gpu`]) -- the rest of the
///   restrictions are spelled out in [`eligible_transition`],
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
        if !is_static_neutral_transform(&layer) {
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

    Some(idx)
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
    let clips = &track.clips;
    let Some(first) = clips.first() else {
        return Ok(());
    };

    // One pass for the whole track: each boundary is both a clip's own transition and
    // its predecessor's handle, and resolving it is what opens the source file.
    let boundaries = crate::transition::effective_durations(clips);

    let mut video_idx: u32 = 0;
    let mut cur = ClipSource::open(first, frame_rate)?;
    let mut cur_layer = transitionless_layer(first, track, canvas);
    // Start each clip with a clean effect cache: a stateful effect (MotionBlur's
    // exposure trail) must not accumulate across a cut into the next clip (RK-025).
    core.reset_effect_cache();

    for i in 0..clips.len() {
        // What the *next* clip's transition blends across. It comes out of this clip's
        // handle, not its body, so the solo run below is unaffected by it.
        let window = match clips.get(i + 1) {
            Some(next) => transition_window(next, boundaries[i + 1], frame_rate)?,
            None => 0,
        };

        // This clip alone, for its whole budget.
        loop {
            let t = output_time(video_idx, frame_rate);
            let Some(pulled) = cur.next()? else {
                break; // Budget spent, or EOF first (a clip shorter than declared).
            };
            let composited = composite_pulled(core, &cur_layer, pulled, canvas, t);
            emit_frame(
                composited,
                encoder,
                &mut video_idx,
                on_progress,
                start,
                total_frames,
            )?;
        }

        let Some(next) = clips.get(i + 1) else {
            break;
        };
        // Past the out-point for the length of the window: those frames are the handle
        // the blend reads, and are not part of the clip's on-screen duration.
        cur.allow_handle(window);
        let mut inc = ClipSource::open(next, frame_rate)?;
        let inc_layer = transitionless_layer(next, track, canvas);
        core.reset_effect_cache();

        // The node the incoming clip's kind renders as. Resolved once per boundary: it
        // is a pure function of the kind. `None` here is fine when `window` is 0 -- that
        // is just a hard cut -- so it is only an error inside the loop below.
        let node = next.transition.and_then(map_transition);

        // The transition window: both clips are composited to the canvas separately and
        // then blended, matching the CPU route where `xfade` is the trailing step of the
        // incoming layer's chain. Progress runs `0 .. (window-1)/window`, so the first
        // output is the outgoing clip untouched and the incoming clip's own frame
        // `window` is the first one shown alone -- the CPU route's mapping, measured.
        //
        // The incoming clip keeps the frames it spends here out of its own solo run on
        // the next iteration, which is what makes the window cost the track nothing.
        for j in 0..window {
            let Some(node) = node else {
                return Err(TimelineError::TimelineRenderFailed {
                    reason: "gpu export: transitioned clip lost its GPU node".to_string(),
                });
            };
            let t = output_time(video_idx, frame_rate);
            let Some(outgoing) = cur.next()? else {
                break; // The outgoing clip ran out early; end the transition with it.
            };
            let (a_rgba, w, h) = composite_pulled(core, &cur_layer, outgoing, canvas, t)
                .ok_or_else(|| fell_back("the outgoing clip"))?;
            let Some(incoming) = inc.next()? else {
                // The incoming clip ran out early. The outgoing frame just composited
                // goes unused: it belonged to this output, which now has nothing to
                // blend it with. Only reachable when a source is shorter than declared,
                // since eligibility bounds the window by the incoming clip budget.
                break;
            };
            let (b_rgba, _, _) = composite_pulled(core, &inc_layer, incoming, canvas, t)
                .ok_or_else(|| fell_back("the incoming clip"))?;
            #[allow(clippy::cast_precision_loss)] // window frame counts fit the mantissa
            let progress = j as f32 / window as f32;
            let blended = core
                .transition(node, progress, &a_rgba, b_rgba, w, h)
                .ok_or_else(|| TimelineError::TimelineRenderFailed {
                    reason: format!(
                        "gpu export: the transition blend failed at progress {progress}"
                    ),
                })?;
            emit_frame(
                Some((blended, w, h)),
                encoder,
                &mut video_idx,
                on_progress,
                start,
                total_frames,
            )?;
        }

        cur = inc;
        cur_layer = inc_layer;
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

    fn eligible(timeline: &Timeline) -> Option<usize> {
        eligible_track(
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
            Some(0),
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
            Some(0),
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
            Some(0),
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
            Some(0),
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
        assert_eq!(eligible_now, Some(0));
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
            Some(0),
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
        assert_eq!(eligible_now, Some(0));
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
        assert_eq!(eligible_now, Some(0));
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
