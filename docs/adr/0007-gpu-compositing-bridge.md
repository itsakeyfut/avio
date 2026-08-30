---
status: accepted
date: 2026-08-30
decision-makers: itsakeyfut
---

# Drive ff-render's GPU compositor from avio for preview and export, GPU by default with automatic CPU fallback

## Context and Problem Statement

`avio` composites a timeline on the CPU through two parallel libavfilter paths:
`ff_filter::MultiTrackComposer` (export) and `ff_filter::RealtimeComposer` (preview, via
`ff_preview::SceneRunner`). The v0.18.0 axis B built a GPU compositor (`ff-render`, wgpu) with a real node
set (color grade, scale, transform, overlay, crossfade, blend, masks, YUV upload), but nothing in `avio`
uses it: `ff-render` has no Rust consumers, and `avio` has no dependency on it. The v0.18.0 bridge (#1365)
makes the GPU compositor the default for both preview and export, with automatic CPU fallback. This ADR
records the cross-crate decision so Br2-Br5 implement against a fixed shape; the mapping detail lives in
[`docs/specs/gpu-compositing-bridge.md`](../specs/gpu-compositing-bridge.md).

## Decision Drivers

* The editing model and its derivation live at the top (`avio`); the `ff-*` primitives must stay model-free by
  dependency direction (ADR-0004, `engine-and-primitives.md`). `ff-render` depends only on `ff-preview` +
  `ff-format` and knows nothing about `FilterStep` or the timeline.
* `ff-render` is entirely behind its non-default `wgpu` feature; pulling `wgpu` into every `avio` build would
  burden headless / export-only / CI consumers.
* `ff-render`'s node set covers only a subset of `avio`'s derived effect vocabulary, so some frames cannot be
  composited on the GPU and must fall back.
* The two CPU compositors already exist, are the behavioural reference, and their output is what a host expects;
  the GPU path must not change results for unsupported effects.
* `avio` already derives to primitive descriptions (`VideoLayer` for `ff-filter`, `Scene` for `ff-preview`), so
  adding `ff-render` as one more derivation target is consistent with how the engine already works.

## Considered Options

* **Mapping placement:** A. a new `gpu`-gated module in `avio`; B. a new dedicated bridge crate between `avio`
  and `ff-render`; C. a `from_scene` adapter inside `ff-render`.
* **Feature / default:** A. a new `gpu` feature on `avio`, not in `default` (GPU is the runtime default when
  built); B. put `gpu` in `avio`'s `default`.
* **Fallback granularity:** A. whole-frame, decided per frame; B. whole-timeline, decided once; C. per-layer
  hybrid GPU/CPU within one frame.

## Decision Outcome

Chosen: **mapping in a `gpu`-gated `avio` module (A)**, a **`gpu` feature not in `default` (A)**, and
**whole-frame per-frame fallback (A)**.

The mapping from `avio`'s derived layer set (`VideoLayer` / `RealtimeLayerDescriptor`, which share one
transform + blend + composite + effect-chain shape) to `ff-render`'s `Compositor` (z-ordered `FrameLayer`
stack) plus per-layer `RenderGraph` (effects) lives in `avio`, so the `FilterStep -> RenderNode` translation
stays out of `ff-render` and its node vocabulary stays independent of libavfilter. A new `avio` `gpu` feature
turns on `dep:ff-render` + `ff-render/wgpu` (+ `ff-render/display` for the zero-copy preview path) but is not
in `default`, so a build that does not ask for GPU never links `wgpu`. When the feature is built, GPU is the
**runtime** default (auto-selected when a wgpu adapter is present) and the existing CPU compositor is the
automatic fallback: a frame composites on the GPU only when every step in its layer set maps to a node,
otherwise that whole frame composites on the CPU path. Export is composite -> readback -> the existing
encoder, unchanged; preview hands the composited texture to `ff_preview`'s `FrameSink` (zero-copy where the
sink accepts a GPU frame, else readback). The CPU compositor remains the correctness reference; cross-driver
differences within tolerance are accepted.

`avio -> ff-render` is a valid downward dependency (`avio` is the top of the graph), so no cycle is
introduced.

### Confirmation

Br1 lands no behavioural code, so this ADR is confirmed by the bridge slices it governs, not by a test that
ships with it:

- **Dependency direction** is a compile-time guard: `avio` gains a `gpu`-gated `ff-render` dependency and
  `ff-render` gains none on `avio` / `ff-filter`; a cycle would fail `cargo build`.
- **The mapping and node-coverage / fallback boundary** are confirmed by the **Br2** mapping unit tests
  (#1625): a derived layer set maps to the specified nodes, and an unsupported step is surfaced as fallback,
  not dropped.
- **GPU-default / automatic CPU fallback / parity** are confirmed by the **Br5** tests (#1628): GPU and CPU
  outputs match within tolerance for the supported node set, and no-adapter / forced-error / unsupported-step
  each route to the CPU path without panic or hang.

If any of those guards is absent when its slice lands, the decision is being violated.

### Consequences

* Good: `ff-render` stays model-free (no `ff-filter` / `avio` dependency); the bridge is the only place that
  understands both the derived vocabulary and the GPU nodes.
* Good: headless / export-only / CI builds stay light (no `wgpu`) because `gpu` is opt-in at build time.
* Good: whole-frame fallback keeps the CPU path as a single consistent correctness reference and avoids mixing
  GPU and CPU colour spaces within one frame.
* Bad / limitation: some frames run on CPU purely because one layer uses an unsupported effect (coarser than a
  per-layer hybrid would be); and each effected layer currently incurs a GPU->CPU->GPU roundtrip because
  `ff-render`'s `Compositor` ingests a `VideoFrame`, not a texture.
* Deferred: BT.709 YUV upload (ff-render is BT.601 only), GPU xfade *kinds*, full node coverage, zero-copy
  GPU->encoder, and exact preview==export pixel convergence.
* What would reverse this: extending `ff-render` to accept textures / apply per-layer effects (removing the
  roundtrip), or a future decision to make the GPU path the sole compositor (retiring the CPU compositors),
  which would move the parity reference.

## Pros and Cons of the Options

### Mapping placement A: a `gpu`-gated `avio` module

* Good: consistent with `avio`'s existing model->primitive derivation; keeps `ff-render` independent of
  libavfilter; no new crate; `avio` already sits above `ff-render`.
* Bad: `avio` performs a primitive-to-primitive translation (`FilterStep` -> `RenderNode`), which is slightly
  outside "pure model" territory, accepted because it is the only place with both dependencies and the editing
  intent.

### Mapping placement B: a dedicated bridge crate

* Good: keeps `avio` free of the `ff-render` dependency.
* Bad: an extra crate for work `avio` already does (it derives `VideoLayer` and `Scene`); more version /
  release surface for no structural gain.

### Mapping placement C: a `from_scene` adapter in `ff-render`

* Good: colocates the mapping with the nodes.
* Bad: `ff-render` would need `ff-filter` (for `VideoLayer` / `FilterStep` / `BlendMode`), coupling the GPU node
  set to libavfilter's vocabulary and inverting the intended independence; export uses `VideoLayer`
  (`ff-filter`), which `ff-render` cannot see today.

### Feature A: `gpu` not in `default`

* Good: `wgpu` stays optional; the base build is light; GPU is still the runtime default when built.
* Bad: a consumer must opt in to the feature to get GPU (a one-line `features = ["gpu"]`).

### Feature B: `gpu` in `default`

* Good: GPU is on with no opt-in.
* Bad: forces `wgpu` on every `avio` consumer, including headless / server / export-only, bloating the build.

### Fallback A: whole-frame per frame

* Good: simple; CPU stays a single correctness reference; no intra-frame colour-space seams; still uses the GPU
  for every fully supported frame.
* Bad: one unsupported layer sends the whole frame to CPU.

### Fallback B: whole-timeline

* Good: simplest; one decision per render.
* Bad: one unsupported clip anywhere forces the entire preview/export onto CPU, wasting the GPU for the
  supported majority.

### Fallback C: per-layer hybrid

* Good: maximal GPU utilization.
* Bad: mixing GPU and CPU results within a frame introduces colour-space seams and substantial complexity for a
  first version.

## More Information

* Bridge tracking issue #1365; sub-steps Br1-Br5 (#1624-#1628); milestone tracker #1593.
* Mapping contract: [`docs/specs/gpu-compositing-bridge.md`](../specs/gpu-compositing-bridge.md).
* Builds on [ADR-0004](./0004-avio-engine-not-facade.md) (avio owns model-facing derivation; primitives stay
  model-free) and the `Scene` seam in [`engine-and-primitives.md`](../specs/engine-and-primitives.md) (the C4c
  note already forward-references #1365 for GPU-default compositing).
