---
status: accepted
date: 2026-08-19
decision-makers: itsakeyfut
---

# Carry all per-clip animation in the model; a primitive may static-evaluate what it cannot yet animate

## Context and Problem Statement

`avio` is the editing engine: the model (`Clip`) exposes continuous per-clip
properties as first-class fields, several with an animation track
(`opacity`/`opacity_track`, `x`/`y` + tracks, `scale`/`scale_track`,
`rotation`/`rotation_track`, `volume_db`/`volume_track`, and #1423's
`pitch`/`pitch_track`). `derive` turns a `Clip` into a primitive `VideoLayer` /
`AudioTrack`, and the primitives (`ff-filter` compositor/mixer, `ff-preview`)
execute it.

The primitives do not all animate every property. Today the compositor animates
`opacity`, `x`, and `y` per-frame and the mixer animates `volume` per-sample, but
it evaluates `scale`, `rotation`, and `pitch` at `t=0` only, and preview animates
even less. Each time a new animatable field is added (#1421 scale/rotation, #1423
pitch), the same question recurs: does "animatable" require the primitive to
animate it per-frame/per-sample before the model may expose the track? This record
decides where the boundary sits, because #1421 already ships under it and #1423
depends on the answer.

## Decision Drivers

* The model is the host's editing document; it must express authoring **intent**
  independently of how completely a given executor renders it.
* Per-frame/per-sample animation of some properties is a real primitive cost
  (`ff-filter` `send_command`/`eval=frame` wiring; a per-sample pitch backend such
  as `rubberband`; `ff-preview` audio/video parity), landed incrementally.
* `derive` must stay one pure interpretation shared by export and preview, not fork
  per executor capability.
* A recurring design question deserves one recorded answer, not a fresh ad-hoc call
  per field (this is the third instance).

## Considered Options

* **A — Model carries the track uniformly; a primitive may static-evaluate at `t=0`
  what it cannot yet animate.** The gap is closed per-primitive as follow-up.
* **B — A field is animatable in the model only once the primitive animates it
  per-frame/per-sample.** The model gates on execution capability.
* **C — Emit a distinct "animated" step/field only when animation is real, and a
  plain static field otherwise.** Two shapes per property.

## Decision Outcome

Chosen option: **A**. The model carries the animation track for every continuous
per-clip property uniformly, and `derive` passes it through uniformly
(`AnimatedValue` on the `VideoLayer`/`AudioTrack` field, or a `t=0`-resolved static
`FilterStep` where the primitive has no animated form, as for `pitch`'s
`FilterStep::PitchShift`). A primitive is allowed to **static-evaluate at
`value_at(Duration::ZERO)`** any track it does not yet animate. Closing each gap
(per-frame `scale`/`rotation`, per-sample `pitch`, and preview parity) is deferred,
per-primitive follow-up work, not a change to the model or the derive.

The current partition is:

* **Animated by the primitive:** `opacity`, `x`, `y` (compositor, per-frame via
  `send_command` / `:eval=frame`); `volume` (mixer, per-sample).
* **Static-evaluated at `t=0`:** `scale`, `rotation` (compositor); `pitch` (mixer,
  as a static `PitchShift`). Preview static-evaluates more than export.

"Animatable" is therefore satisfied at the **model/derive level** (the track is
stored, undoable, and flows through the shared derive) for every such field; the
executor animating it per-frame/per-sample is a separate capability that may lag.

### Confirmation

The model-carries-and-passes-through half is guarded by the pure derive unit tests:
`crates/avio/src/derive.rs` `video_layer_*`/`realtime_descriptor_*` scale/rotation
tests (#1421) and `audio_track_*_pitch_*` tests (#1423) fail if a track stops
flowing to the primitive field/step, or if export and preview diverge in what the
derive emits.

The `t=0` static-evaluation half is a property of the primitive code, not a
dedicated "does not animate" test: the `layer.scale_x.value_at(Duration::ZERO)` /
`rotation` / preview call sites in
`crates/ff-filter/src/graph/composition/composition_inner.rs` (and the static
`PitchShift`). Adding per-frame/per-sample animation there later is the intended
evolution, **not** a violation — so nothing fails when a gap is closed. A violation
would be the model gating a track behind executor capability (Option B), which the
derive tests above would surface as a missing field/step.

### Consequences

* Good, because the host can author complete animation intent now, and it renders
  more completely as the primitives catch up, with no model or API change.
* Good, because `derive` stays a single pure interpretation; capability lag lives
  in the executors, not in branching model logic.
* Good, because it is honest and reviewable: the partition above tells a reviewer
  that a static-evaluated track is intended, not a bug.
* Bad, because a set track can render as a constant (`t=0`) value, which can
  surprise a user until the executor animates it; each such field's docs must say
  so (e.g. `Clip::pitch`).
* Bad, because "animatable" means two things (model-level vs executor-level); the
  gap must be stated explicitly in each feature's acceptance-criteria
  reconciliation.
* What would reverse this: if per-frame/per-sample animation of all these
  properties lands, the `t=0` fallback disappears and the distinction is moot; if a
  primitive can never animate a property, Option C (a separate static-only field)
  may fit that property better. Either is a new record superseding this one.

## Pros and Cons of the Options

### A — Uniform track in the model; primitive may static-evaluate (chosen)

* Good, because intent is complete and decoupled from executor capability.
* Good, because the derive stays uniform and pure.
* Bad, because a stored track can silently render at its `t=0` value until the
  executor animates it.

### B — Gate model animatability on primitive capability

* Good, because "animatable" always means fully rendered, with no surprise.
* Bad, because it couples the model to executor progress: a field's public shape
  changes when the primitive gains animation, breaking hosts and churning the API.
* Bad, because it forks the model's completeness across export vs preview, which
  animate different subsets.

### C — Distinct animated vs static shapes per property

* Good, because each shape renders exactly as declared.
* Bad, because it doubles the surface per property and pushes executor capability
  into the model's type shape, which then changes as primitives evolve.

## More Information

* Instances: #1421 (per-clip scale/rotation), #1423 (per-clip pitch); siblings that
  already animate: opacity, x/y (compositor), volume (mixer).
* Implementation: `crates/avio/src/derive.rs` (`video_transform`, `video_layer` /
  `realtime_descriptor`, `audio_track`); the `t=0` sites in
  `crates/ff-filter/src/graph/composition/composition_inner.rs`.
* Related decision: [ADR-0001](./0001-clip-and-track-identity.md) (the model as the
  host's editing document).
