---
status: "accepted"
date: 2026-09-28
decision-makers: itsakeyfut
---

# An `Exact` seek holds back the frame it landed on, instead of consuming it

## Context and Problem Statement

`VideoDecoder::seek(t, SeekMode::Exact)` and its audio counterpart decode forward from
the preceding keyframe until a frame reaches `t`. Reaching that frame requires decoding
it, and nothing held it, so it was dropped with the frames before it: the next
`decode_one()` returned the frame *after* the one asked for. Measured on the repository
assets, every `Exact` seek was late by exactly one frame (33 ms at 30 fps, 26 ms on an
MP3).

The GPU export is the only caller of `Exact` in the workspace, so this surfaced as "the
GPU export is not frame-accurate" (#1811). The CPU export does not seek at all: `derive`
emits `Trim`/`ATrim` and libavfilter applies them by timestamp, keeping the frame at the
in-point. One in-point, two rules, because only one route goes through `seek`.

`seek`'s documentation stated what it errors on and nothing about what it leaves behind,
so the contract every caller assumed was never written down, and nothing contradicted the
implementation.

## Decision Drivers

* A trim must name the same source frame on every route, or preview, CPU export and GPU
  export disagree about what the user edited.
* Whatever fixes it must not depend on the frame interval: `avg_frame_rate` is already
  recorded as unreliable on short files, and it does not exist for a variable frame rate.
* A frame from before a seek must never be handed out after it. That failure is worse
  than the one being fixed, because it is silent and position-dependent.

## Considered Options

* A push-back slot on the decoder, holding the frame the seek landed on.
* Seek one frame interval earlier and let the existing loop stop on the target.
* Leave `Exact` as it is and correct at the call site in `gpu_export`.

## Decision Outcome

Chosen option: "a push-back slot", because it makes the decoder's own contract true
rather than compensating for it somewhere else, and it needs no frame interval.

`VideoDecoderInner` and `AudioDecoderInner` each carry a `pending: Option<Frame>`.
`skip_to_exact` stores the frame that reaches the target instead of dropping it, and
`decode_one` returns it before anything else, ahead of the `drained` check, so a frame
captured before the stream ran out is still delivered. `seek` and `flush` clear the slot,
which is what keeps the staleness hazard closed.

`Keyframe` and `Backward` are deliberately left alone. They stop within
`KEYFRAME_SEEK_TOLERANCE_SECS` *before* the target, so they are approximate by contract,
and their consumer (`ff-preview`'s decode buffer) already decodes forward to the frame it
wants. The two modes are now asymmetric on purpose, and `seek`'s documentation says so.

### Confirmation

`crates/ff-decode/tests/video_seeking_tests.rs`:
`seek_exact_should_land_on_the_target_frame_and_not_consume_it`,
`seek_exact_should_not_return_a_frame_held_back_by_an_earlier_seek`,
`flush_should_discard_the_frame_a_seek_held_back`, and the tightened
`test_seek_exact_mode` (its tolerance is one frame interval, not 500 ms).
`crates/ff-decode/tests/audio_decoder_tests.rs` carries the same three against
`AudioDecoder`. End to end,
`trims_should_start_on_the_same_source_frame_on_both_routes`
(`crates/avio/tests/trim_frame_parity.rs`) fails when the slot is removed, and
`marked_source_should_round_trip_its_own_index` proves its instrument first.

### Consequences

* Good, because the in-point of a trim no longer depends on which route renders it.
* Good, because the contract is stated where a caller reads it, not inferred from one
  caller's behaviour.
* Bad, because the decoder now holds a frame across a seek boundary, so every state
  transition that discards decoded state has to clear it. `seek` and `flush` do; a future
  one that forgets reintroduces a worse defect than the one fixed here.
* `ff-preview`'s own discard loop after a `Backward` seek is unaffected, but its
  equivalent for `Exact` would now be redundant.
* What would reverse this: an `Exact` seek that could position without decoding the
  target frame (a container index precise enough to stop one frame early), which would
  make the slot unnecessary.

## Pros and Cons of the Options

### A push-back slot

* Good, because it needs no frame rate and works on a variable frame rate.
* Good, because the fix is in the decoder that owns the contract.
* Bad, because it adds state that a seek or flush must remember to clear.

### Seek one frame interval earlier

* Good, because it adds no state.
* Bad, because it needs the frame interval, whose source is unreliable on short files and
  meaningless on a variable frame rate.
* Bad, because it would overshoot backwards where the interval is wrong, turning a
  one-frame error into a different one-frame error.

### Correct at the call site

* Good, because it touches one file.
* Bad, because it leaves a published crate's documented behaviour wrong for every
  external caller, which is how this survived to begin with.

## More Information

* #1811 for the measurement (three targets, always +1.001 frames).
* `crates/ff-decode/src/video/decoder_inner/seeking.rs` and
  `crates/ff-decode/src/audio/decoder_inner.rs` for the slot and its clears.
* ADR-0009 for the transition rules that read a clip's in-point.
