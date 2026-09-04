---
status: "accepted"
date: 2026-09-04
decision-makers: itsakeyfut
---

# GPU blend modes reproduce FFmpeg's `vf_blend`, not Photoshop

## Context and Problem Statement

`ff_filter::BlendMode` is documented as a 1:1 mirror of FFmpeg's `blend` filter `all_mode` set (40
modes). `ff-render`'s GPU compositor implemented 18 modes, of which only 14 were reachable from the
model, and three of those 14 (`ColorDodge`, `ColorBurn`, `SoftLight`) were transcribed from
Photoshop / W3C formulas rather than FFmpeg's, so GPU preview and CPU export disagreed for them.
Bringing the remaining 26 modes to the GPU (#1669) forces the question of which definition the whole
set follows, because the answer decides whether the three existing modes are corrected or left as a
second convention.

## Decision Drivers

* ADR-0007 makes the CPU compositor the correctness reference and the GPU an accelerated path that
  falls back to it. Two paths that render the same timeline differently break that premise.
* Most of the 26 added modes (`freeze`, `heat`, `phoenix`, `stain`, `bleach`, `extremity`,
  `hardoverlay`, `softdifference`, `interpolate`, …) exist only in FFmpeg. There is no Photoshop or
  W3C definition to follow even if one were preferred.
* #1671 will compare GPU output against reference images produced by the CPU path. That suite is
  only meaningful if the two paths are supposed to agree.

## Considered Options

* FFmpeg `vf_blend` semantics for every mode, correcting the three divergent ones
* Photoshop / W3C semantics, keeping the three as they are and inventing definitions for the rest
* FFmpeg for the 26 new modes only, leaving the three divergent ones alone

## Decision Outcome

Chosen option: **FFmpeg `vf_blend` semantics for every mode**, transcribed from the `DEPTH == 32`
branch of `libavfilter/blend_modes.c` (byte-identical in `release/7.1` and `release/8.0`, so no
version gating is needed). The `ColorDodge`, `ColorBurn` and `SoftLight` shaders are corrected to
match.

Two details the transcription pins down:

* **Which input is FFmpeg's `A`.** `vf_blend.c` declares pad 0 as `top` (`A`) and pad 1 as `bottom`
  (`B`), and `crates/ff-filter/src/filter_inner/build.rs` links the canvas to pad 0 and the layer to
  pad 1. So `A` = base (canvas) and `B` = overlay (layer) throughout `blend.wgsl` and
  `blend_math.rs`. The opacity form corroborates this independently: FFmpeg computes
  `dst = A + (expr - A) * opacity` and the shader computes `mix(base, blend, overlay.a * opacity)`,
  both mixing toward `A`.
* **`And` / `Or` / `Xor` are the one exception to the float branch.** There the C is bitwise on the
  IEEE-754 bit pattern, which is not an image operation; the GPU implements the 8-bit integer
  definition instead, which is what the compositor's `Rgba8Unorm` working format means.

The `DEPTH == 32` branch applies no clamp, so `Bleach`, `Stain`, `GrainExtract`, `GrainMerge`,
`LinearLight`, `Multiply128` and `Divide` leave `[0, 1]`. The shader's final `clamp` and the
`Rgba8Unorm` write reproduce FFmpeg's float-to-8-bit conversion; the 8-bit C path wraps instead, and
that is deliberately not replicated.

### Confirmation

* `blend_rgb_should_match_the_ffmpeg_reference_for_every_mode` in
  `crates/ff-render/src/nodes/composite/blend_math.rs`: 40 modes against three colour pairs, with
  the expected values transcribed from the same C a second time so a mistranscription in the Rust
  fails rather than passes. `blend_rgb_should_take_the_guarded_branch_at_each_singularity` covers
  the exact-equality escapes a mid-range pair never reaches, and
  `blend_rgb_should_leave_the_unclamped_modes_outside_the_unit_range` pins the no-clamp decision.
* `blend_gpu_should_match_the_cpu_path_for_every_mode` in `crates/ff-render/tests/gpu_nodes.rs`
  (adapter-gated) ties the shader to that Rust for all 44 variants.
* `map_scene_should_map_every_blend_mode` in `crates/avio/src/gpu.rs` fails if any mode the model can
  express stops mapping to a GPU node.

What none of these prove is agreement with a *running* FFmpeg; that comparison is #1671's
reference-image suite.

### Consequences

* Good, because GPU preview and CPU export now render the same image for every blend mode, which is
  what ADR-0007's fallback design assumes.
* Good, because no frame falls back to the CPU compositor on account of its blend mode any more.
* Bad, because `ColorDodge`, `ColorBurn` and `SoftLight` render differently than they did on the GPU
  before. The change moves them toward the exported result, so it corrects a divergence rather than
  introducing one, but a host that calibrated against the old GPU preview will see a shift.
* Bad, because `And` / `Or` / `Xor` are bit-depth dependent by nature and are pinned to the 8-bit
  definition. A future higher-precision working format would have to revisit them.
* What would reverse this: making `ff-render` the correctness reference instead of the CPU
  compositor, which would supersede ADR-0007 first.

## Pros and Cons of the Options

### FFmpeg semantics for every mode

* Good, because one reference covers all 40 modes with no invented definitions.
* Good, because GPU/CPU parity becomes a testable property rather than an aspiration.
* Bad, because it inherits FFmpeg's quirks, including formulas that saturate over most of their
  input range (`bleach`, `stain`) and a `linearlight` branch that is vacuous in float.

### Photoshop / W3C semantics

* Good, because the formulas are the ones a colourist recognises from other tools.
* Bad, because 20-odd of the 40 modes have no such definition, so they would have to be invented and
  would then disagree with the CPU path by construction.

### FFmpeg for the new modes only

* Good, because it is the smallest diff and changes no existing output.
* Bad, because the GPU blend set would follow two references at once, and the divergence would
  survive as an unexplained special case that #1671 has to encode as an expected difference.

## More Information

* Reference C: `libavfilter/blend_modes.c` at
  [release/7.1](https://github.com/FFmpeg/FFmpeg/blob/release/7.1/libavfilter/blend_modes.c) and
  [release/8.0](https://github.com/FFmpeg/FFmpeg/blob/release/8.0/libavfilter/blend_modes.c);
  pad naming in `vf_blend.c` at the same tags.
* [ADR-0007](./0007-gpu-compositing-bridge.md) (the CPU compositor is the correctness reference).
* Issues: #1669 (this work), #1671 (reference-image regression suite), #1219 (the HSL modes, which
  have no `all_mode` token and stay Photoshop-defined and unreachable from the model).
