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

### Both routes, not only the GPU (#1806)

This record pinned the shaders to a branch and said nothing about the CPU composition, which was
handing `vf_blend` a `yuv420p` stream. The filter applies its formula **per plane** whatever the
planes hold, so that route blended luma against luma and chroma against chroma: `Darken` of a red
and a green returned green, 120 levels from the GPU's answer, and 35 of the 40 modes disagreed.

`DEPTH` is bit depth, not colour space: `blend_modes.c:37` defines `DEPTH == 32` as `PIXEL float`
with `MAX 1.f`. Choosing that branch therefore binds **which planes are fed**, and both routes now
convert both blend inputs to planar float RGB (`gbrpf32`) before the blend, converting straight back
afterwards. The canvas side has to be converted too: normalising only the layer leaves libavfilter to
negotiate the pair back to the canvas's format, which measured identically to not fixing it at all.

Two measurements corrected this record's own expectations:

* **8-bit planar RGB (`gbrp`) would have been enough for all but two modes.** The wrap this record
  describes separates float from 8-bit far enough to matter, at the parity fixture's colours, only
  for `Bleach` (222 levels) and `Stain` (232). Float is still the right choice, because it is the
  arithmetic the shaders were transcribed from, but the expectation that seven modes needed it was
  too broad.
* **`And` / `Or` / `Xor` must not take the float branch.** The exception stated above is
  load-bearing: on `gbrpf32` the C operates on the IEEE-754 bit pattern, which put those three 23, 21
  and 221 levels from the GPU. They are given 8-bit planar RGB so the CPU route runs the same integer
  definition the shaders implement.

What is left between the routes is the yuv/rgb round trip each takes, one or two levels, amplified to
five or seven by the modes that divide by a small number (`ColorDodge` is `B / (1 - A)`, about 4.6
output levels per input level at the fixture's colours).

The conversions cost what they weigh: a CPU render of 120 frames at 640x360 with one blended layer
goes from 0.118s to 0.233s, while the `Normal` path (`overlay`, no conversion) stays at 0.123s. Two
float conversions in and one out, on the route that is already the fallback, bought against a result
that was simply the wrong colour. 8-bit planar RGB would cost a quarter of the samples, but it is not
the branch the shaders were transcribed from, and `Bleach` and `Stain` measurably need the float one.

One consequence is worth stating because it is visible: `VideoFrame` carries no colour range, so
swscale reads YUV as limited. Values inside 16..235 survive the conversion, while a `Y = 0` or
`Y = 255` pushed into a CPU blend now lands on the limited-range endpoint instead of passing through.
Decoded footage lives inside that range, and the GPU route has always worked this way (it composites
in RGB and reads back), so this is the CPU route joining it rather than a new loss.

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
* `every_blend_mode_should_agree_between_the_two_render_routes` in
  `crates/avio/tests/blend_route_parity.rs` renders all 40 modes through both routes on a chromatic
  fixture and fails if either changes colour space (#1806).

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
