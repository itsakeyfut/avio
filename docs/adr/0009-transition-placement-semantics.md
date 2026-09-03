---
status: "accepted"
date: 2026-09-03
decision-makers: itsakeyfut
---

# A transition preserves the timeline length and is fed by handles

## Context and Problem Statement

The model says a transition is "applied at the boundary where the preceding clip ends
and this clip begins" and stops there. It never says what the transition does to
*placement*, so the three consumers each assumed something different and none of them
agree:

| consumer | assumption | symptom |
|---|---|---|
| CPU export (`derive::video_layer` + `xfade`) | the transition overlaps the two clips, so the composited stream shortens by the transition duration `D` | a transition on a middle clip opens a hole (15 frames of black for a 0.5 s transition at 30 fps), and chained transitions fire early (#1731) |
| GPU export (`gpu_export::drain_video_gpu`) | reproduces the CPU route, and guards the two bugs above by only accepting a transition into the track's **last** clip | correct output, but a needless restriction |
| preview (`ff-preview` `SceneRunner`) | the clips already overlap by `D` on the timeline | the engine derives `overlap = 0 ns`, so the runner advances to the incoming clip before it can arm the blend and the transition never renders (#1737) |

The disagreement is also visible without either bug. Measured on a two-clip 1 s + 1 s
timeline carrying a 2 s audio track:

```
hard cut  : video 1.967 s / audio 2.000 s
0.5 s Fade: video 1.467 s / audio 2.000 s
```

Adding a *video* transition moves the video 0.5 s out of sync with audio that nothing
asked to change. Fixing the two reported symptoms without settling the semantics would
just re-encode the disagreement, so the semantics have to be decided first.

## Decision Drivers

* Audio and video must stay aligned: a video-only edit cannot shift an audio clip.
* Preview and export must agree, and must be *unable* to drift apart again.
* `xfade`'s own contract is fixed and has to be worked with:
  `output_length = offset + input1_length`, with `offset` relative to input 0's first
  PTS.
* The outgoing clip's frames past its out-point (its *handle*) are a finite resource;
  the rule has to say what happens when there are not enough of them.

## Considered Options

* **Handles, length-preserving** - the transition consumes the outgoing clip's handle;
  no clip moves.
* **Shrink and shift** - the transition overlaps the clips, the timeline shortens by
  `D`, and every later clip moves earlier by `D`.
* **Overlap authored in the model** - the model itself stores the clips overlapping, and
  derivation is a pass-through.

## Decision Outcome

Chosen option: **handles, length-preserving**.

A transition of effective duration `D` into clip `B` occupies `[B.offset, B.offset + D)`
on the timeline. Across that window the outgoing clip `A` is read *past its out-point*
and blended against `B`'s head. Nothing shifts, no clip moves, and audio stays where it
was authored.

**When `A` has less than `D` of handle, `D` shrinks to what exists**, zero being a hard
cut, with a `log::warn!`. The timeline length is preserved either way, so nothing
downstream moves; a shorter blend is the smallest visible degradation available.

The arithmetic follows from `xfade`'s contract. Setting `offset = B`'s authored start on
the track gives `output = B.offset + B_len`, which is the hard-cut length, so the
composited stream no longer shrinks. That removes both #1731 symptoms structurally: a
later clip's absolute `OffsetPts` lands where it was authored, and each `xfade` in a
chain gets its own clip's authored start as its offset.

One function in `avio` computes the effective duration, and every derivation calls it:
the CPU export, the GPU export drain, and the preview projection. They cannot compute
different answers.

### Confirmation

These fail if the decision is violated:

* `crates/avio/tests/transition_placement.rs` - a transitioned timeline encodes the same
  frame count as the hard cut, a middle-clip transition emits no black frames, chained
  transitions each blend at their own boundary, and the video and audio durations match.
* `crates/avio/src/transition.rs` unit tests - the clamp to the available handle, and the
  warn-and-degrade path when the handle is shorter than the authored duration.
* `crates/avio/tests/preview_transition_reach.rs` - a derived scene gives the outgoing
  clip the handle its transition needs, and the real runner reaches the blend on that
  scene (it reached it zero times before).
* The existing parity suites (`gpu_parity_tests`, `xfade_reference_parity`) guard that
  this changes placement only, not pixels.

### Consequences

* Good, because adding or removing a video transition no longer changes the timeline's
  length or its A/V alignment.
* Good, because the GPU export's "last clip only" restriction loses its reason and can be
  dropped, widening GPU coverage.
* Good, because the preview's transition path becomes reachable for the first time.
* Bad, because a clip trimmed flush to the end of its source has no handle and silently
  degrades to a shorter blend (warned, not errored).
* Bad, because the outgoing clip's decode now runs past its out-point, so a source is
  read slightly further than the model's trim suggests.
* What would reverse this: an editing model where a transition is a first-class object
  with its own placement rather than a property of the incoming clip. That is a model
  change and would supersede this record.

## Pros and Cons of the Options

### Handles, length-preserving

* Good, because a video-only edit has video-only consequences.
* Good, because it matches what the `xfade` contract makes cheap: one offset per
  boundary, no accumulated drift.
* Bad, because it needs the source probed to know the handle, and a clamp when there is
  none.

### Shrink and shift

* Good, because it needs no handle: the transition is cut from material already in use.
* Bad, because a video transition would have to move audio clips to stay in sync, which
  is an edit the user did not make. Leaving them put is what produced the 0.5 s drift
  measured above.
* Bad, because every later clip's placement becomes a function of every earlier
  transition, so a chain accumulates offsets - the shape of the #1731 chaining bug.

### Overlap authored in the model

* Good, because derivation becomes a pass-through and the preview's current assumption
  would already be right.
* Bad, because it makes the model store a derived quantity: the overlap has to be
  recomputed on every trim, speed change, or transition-duration edit, and an undo that
  restores one field without the other leaves the model inconsistent.
* Bad, because it exposes the transition mechanism in the persisted document, so a
  change to the mechanism becomes a format change.

## More Information

* Issues [#1731](https://github.com/itsakeyfut/avio/issues/1731) (export placement) and
  [#1737](https://github.com/itsakeyfut/avio/issues/1737) (preview never arms).
* [ADR-0007](./0007-gpu-compositing-bridge.md) - the GPU route this rule also governs.
* Architecture of record: `docs/specs/engine-and-primitives.md`.
