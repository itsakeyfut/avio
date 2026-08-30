---
status: accepted
date: 2026-08-30
decision-makers: itsakeyfut
---

# Animate scale/rotation per frame with self-animating compositor steps, neutralizing the static layer transform

## Context and Problem Statement

ADR-0002 recorded that the model carries per-clip animation while a primitive may
static-evaluate what it cannot yet animate, and named `scale`/`rotation` as
evaluated at `t=0` only. The compositor collapses `layer.scale_x/scale_y/rotation`
via `value_at(0)` on both the offline and realtime paths
(`ff-filter/src/graph/composition/composition_inner.rs`), so an animated scale or
rotation renders frozen. Closing that (issue #1614) requires deciding how the
per-frame animation is realized and how preview is kept equal to export.

## Decision Drivers

* `derive` must stay one pure interpretation shared by export and preview (ADR-0004),
  and preview must equal export within tolerance for the animated case.
* The compositor already animates `opacity`/`x`/`y` per-frame via `send_command`
  (`AnimationEntry`, evaluated at the timeline PTS), and already builds and
  alpha-handles `FilterStep::ScaleAnimated`/`RotateAnimated` in the per-layer effect
  loop (`layer_has_alpha_effects` already lists `RotateAnimated`).
* `ScaleAnimated`/`RotateAnimated` self-animate via an `eval=frame` `t`-expression
  (`AnimationTrack::to_ffmpeg_expr`), so the same expression drives both graphs with
  no `send_command` driver; the realtime runner already stamps each pushed frame with
  the timeline PTS.

## Considered Options

* **A. `derive` emits self-animating `ScaleAnimated`/`RotateAnimated` into the per-clip
  effect chain and neutralizes the static layer scalar** (`scale=1.0`/`rotation=0.0`).
* B. Extend the compositor's `send_command`/`AnimationEntry` mechanism (as for
  `opacity`/`x`/`y`) to the `scale`/`rotate` filter nodes.
* C. Make the compositor's static `scale`/`rotate` nodes self-animate inline.

## Decision Outcome

Chosen option: **A**. When the merged scale or rotation is `AnimatedValue::Track`,
`derive` (`video_layer` and `realtime_descriptor`) splices a self-animating
`ScaleAnimated`/`RotateAnimated` into the effect chain (after the temporal placement,
so the `t`-expression sees timeline time) and sets the layer scalar neutral, so the
compositor's static transform node is skipped and the animation is applied once. The
static (non-animated) case is unchanged, exactly as ADR-0002 describes. This reverses
ADR-0002's "scale/rotation at `t=0` only" for the animated case; the boundary decision
in ADR-0002 (the model may expose a track the primitive static-evaluates) still holds
for any executor that has not caught up.

Preview equals export **by construction**: the identical `t`-expression is placed in
both graphs, and both present timeline `t` at that node (offline after `OffsetPts`;
realtime via the runner's per-frame timeline-PTS stamp). No compositor or preview
source change is needed — the change lives entirely in `avio/derive`.

### Confirmation

`crates/avio/src/derive.rs` unit tests
(`video_layer_animated_scale_should_emit_scale_animated_and_neutralize`,
`..._rotation_...`, `realtime_descriptor_animated_scale_should_emit_scale_animated`,
and `video_layer_static_scale_should_stay_on_the_layer` for the no-regression case)
fail if an animated transform is left on the scalar (frozen) or a static one is
routed through `ScaleAnimated`. The probe-gated integration test
`compositor_should_evaluate_per_frame_scale_and_rotation`
(`ff-filter/tests/composition_tests.rs`) fails if the composited output does not
change across frames.

### Consequences

* Good: preview == export for free (one expression, two graphs); no new
  `send_command` wiring; no `ff-filter`/`ff-preview` source change.
* Good: the static path is byte-identical (AC2) because emission is gated on an
  animated axis and the scalar neutralizes only then.
* Bad: an animated `ScaleAnimated` resizes per frame (a per-frame `scale`), costlier
  than a one-shot scale; acceptable for the animated case only.
* Bad/limitation: `RotateAnimated` is emitted **before** `ScaleAnimated` because the
  rotate supersampling assumes a stable input size, while an animated scale varies the
  output size per frame (the size-varying result feeds the `overlay`, which handles
  it). For the uniform clip `scale` the two commute, so this matches the static
  scale-then-rotate result; a non-uniform track-level `scale_x != scale_y` combined
  with an animated rotation would differ in order (an accepted edge case).
* What would reverse this: a per-sample/​per-frame executor divergence that makes the
  shared `t`-expression insufficient, or moving the transform into a GPU node (#1365).

## Pros and Cons of the Options

### A. derive emits self-animating steps + neutralized scalar

* Good: integrates with the compositor's existing effect-loop and alpha handling;
  preview == export by construction; avio-only change.
* Bad: needs a factor→pixel track conversion (`ScaleAnimated` sizes in pixels).

### B. send_command on scale/rotate nodes

* Good: same mechanism as `opacity`/`x`/`y`.
* Bad: the `scale` filter does not cleanly take per-frame width/height commands;
  needs a driver loop; risks preview/export divergence the `t`-expression avoids.

### C. inline self-animating static nodes

* Good: keeps nodes in place.
* Bad: reimplements `ScaleAnimated`/`RotateAnimated`; the offline overlay alpha
  detection (`layer_has_alpha_effects`) would miss a rotate built outside `effects`.

## More Information

* Issue #1614 (follow-up from #1591); supersedes the `scale`/`rotation` `t=0` note in
  [ADR-0002](./0002-per-clip-animation-in-the-model.md) for the animated case.
* `AnimationTrack::to_ffmpeg_expr` (`ff-filter/src/animation/track.rs`); the realtime
  timeline-PTS stamp (`ff-preview/src/scene/runner.rs`).
