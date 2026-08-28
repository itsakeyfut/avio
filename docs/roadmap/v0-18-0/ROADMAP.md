# v0.18.0: Editing Model and GPU Rendering

**Goal**: Make `avio` a **GPU-default video editing engine**. The engine composites **both preview
and export on the GPU by default**, with automatic **CPU fallback** when no GPU adapter is present or
the GPU path fails, matching how professional NLEs (Resolve, Premiere, Final Cut) work. Two capability
axes support this: the `avio` editing model matures a further step, and the `ff-render` GPU primitive
gains the foundation a real-time compositor needs. A bridge connects them so the engine's `Timeline`
drives GPU compositing.

**Prerequisite**: v0.17.0 complete. The immutable-model redesign that gated GPU compositing (#1327) is
closed.

**Crates in scope**: `avio` (engine), `ff-render` (GPU compositing), `ff-preview` (preview sink),
`ff-format` (high-bit-depth pixel formats), `ff-pipeline` (export path).

**Out of scope (deferred to later milestones)**:
- A per-clip retiming / segment model (freeze that extends clip length, speed ramps / time remap):
  an independent subsystem with its own milestone.
- Nested sequences / compound clips: their own milestone, and the natural substrate for interchange.
- The richer GPU effect stack (LUT, blur, glow, curves, vignette, HSL, color wheels, motion blur) and
  the HDR / color-science nodes (tone mapping, colour-space transform): these consume this milestone's
  GPU foundation and land in later milestones (color science, and the render backlog).
- Zero-copy GPU to hardware-encoder handoff on export: an optimisation after the readback path works.

---

## Requirements: a GPU-default rendering engine

### GPU-default compositing for preview and export

- Preview and export composite on the GPU by default when a GPU adapter is available.
- When no adapter is present, or the GPU path fails, the engine falls back to the existing CPU
  compositor automatically, without the caller choosing a path.
- Export composites on the GPU, reads the result back to a CPU frame, and hands it to the existing
  encoder unchanged; the encoder is not altered by this milestone.
- The choice between GPU and CPU is made per render session, not per node.

### Preview equals export

- For the effect set the GPU path supports, preview and export produce equivalent output within a
  stated tolerance (exact cross-driver equality is not required).
- The editing model no longer drops or statically approximates content that export renders but preview
  does not: generated Text/Solid sources appear in preview, clip pan is applied, and per-frame
  scale/rotation are evaluated where the model animates them.

### Timeline drives GPU compositing

- The engine's `Timeline`, through its existing model-to-scene derivation, maps to the GPU
  compositor's layers, transforms, blends, masks, and colour grade.
- The mapping is shared by the preview and export GPU paths.
- The cross-crate decision (engine model driving the GPU primitive) is recorded as an ADR, and a
  regression test fails if the GPU and CPU paths diverge beyond tolerance or the fallback does not
  engage.

## Requirements: `ff-render` GPU foundation

### Steady-state allocation

- Repeated frames reuse GPU textures rather than allocating per node or per layer, so a running
  pipeline performs no GPU allocations per frame in steady state.

### Multi-pass and multi-input execution

- The graph executor honours a node's declared pass count and input count, so nodes that need more
  than one pass or more than one input are driven correctly rather than assumed to be single-pass,
  single-input.

### High-bit-depth pipeline

- When the input frame is 10- or 12-bit, the internal pipeline runs at a floating-point texture format
  (`Rgba16Float`) so precision is not lost before any node runs, and the format propagates through the
  texture pool and graph. This is the keystone the later colour-science nodes depend on.

### Direct display path

- A GPU-resident sink can present a composited frame without a GPU-to-CPU round trip, removing the
  per-frame readback that dominates preview latency today.

### Correct scaling and colour on ingest

- The scale node produces the requested output dimensions (not merely a same-size blit), with a CPU
  fallback path, so preview and proxy downscaling work.
- Frame ingest into the compositor uses the GPU YUV upload path and applies a single, consistent
  YUV-to-RGB conversion, resolving the current BT.601-versus-BT.709 inconsistency between the
  compositor and the node path.

## Requirements: `avio` editing-model maturity

### Typed, re-editable effect parameters

- A clip's effects are expressed as a typed, re-editable parameter model rather than an opaque
  execution chain, so a host can present and edit individual parameters and keyframe them, and effects
  can be enabled/disabled and reordered.

### Uniform, identity-keyed automation

- Track-level automation is keyed by track identity, not by track index, so reordering or removing a
  track never misaligns automation, and clip-level and track-level automation share one uniform model.

### Serialization completeness

- A saved project round-trips completely: track-level audio effects persist and reload rather than
  deserialising empty.

---

## Definition of Done

- Preview and export composite on the GPU by default with automatic CPU fallback.
- The GPU path covers `ff-render`'s implemented node set; unsupported effects fall back to CPU.
- Preview equals export within tolerance for the supported set.
- The editing model gains the typed effect-parameter model; retiming and nested sequences remain
  deferred.
- The GPU foundation (texture pool, multi-pass execution, high-bit-depth format, direct display) is in
  place, unblocking the colour-science and effect-node work in later milestones.
