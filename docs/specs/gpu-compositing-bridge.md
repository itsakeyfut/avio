# GPU compositing bridge (Timeline -> ff-render)

> Architecture of record for the v0.18.0 bridge (#1365). States **what** the bridge is and how the
> derived scene maps to `ff-render`; the **why** lives in [ADR-0007](../adr/0007-gpu-compositing-bridge.md).
> This spec is the contract Br2-Br5 (#1625-#1628) implement against.

## Position

`avio` today derives a per-clip description and hands it to one of two CPU compositors:

- **export** -> `avio::derive::video_layer` -> `ff_filter::VideoLayer` -> `ff_filter::MultiTrackComposer`
  (self-decoding libavfilter graph, output `yuv420p`).
- **preview** -> `avio::derive::realtime_descriptor` -> `ff_filter::RealtimeLayerDescriptor` ->
  `ff_preview::SceneRunner` -> `ff_filter::RealtimeComposer` (host-pushed frames, output `rgba`).

The bridge adds a **third compositor path**: `ff-render`'s GPU compositor, made the default for both preview
and export, with the existing CPU compositors as the automatic fallback. GPU is the runtime default when the
`gpu` feature is built and a GPU adapter is present; otherwise the frame composites on the CPU path, which
stays the correctness reference.

## Crate boundary and feature gating

- The mapping lives in a new **`gpu`-gated module in `avio`** (e.g. `avio::gpu`). `avio` is the top of the
  dependency graph, so `avio -> ff-render` is a valid downward dependency with no cycle. `ff-render` never
  depends on `avio` or on `ff-filter`, so its GPU node vocabulary stays independent of libavfilter's
  `FilterStep`; the `FilterStep -> RenderNode` translation is the bridge's job and lives in `avio`.
- A new **`gpu` cargo feature on `avio`**, **not** in `avio`'s `default`. It turns on `dep:ff-render` +
  `ff-render/wgpu`, and `ff-render/display` for the zero-copy preview path. Headless / export-only / CI builds
  that do not enable `gpu` never pull in `wgpu`.
- `ff-render` is entirely behind its own `wgpu` feature (`default = []`); depends on `ff-preview` + `ff-format`.

  These `Cargo.toml` / dependency edges are **specified here and added in Br2** (#1625); Br1 changes no code.

## Input: the derived layer set

The bridge maps from `avio`'s existing derived layer types; it introduces no new scene type.

- `VideoLayer` (export) and `RealtimeLayerDescriptor` (preview) are near-identical: both carry the shared
  `VideoTransform` (`x`, `y`, `scale_x`, `scale_y`, `rotation`, `opacity` as `AnimatedValue<f64>`),
  `blend_mode: ff_filter::BlendMode`, `composite_op: ff_filter::CompositeOp`, and `effects: Vec<FilterStep>`.
  They differ only in `proxy` (export) vs the decode-time `width`/`height`/`pixel_format` (preview).
- The mapping keys off this **shared shape** (transform + opacity + blend + composite + effect chain), so it is
  written once and reused by both paths. Br2 should read the common fields through a small shared view rather
  than duplicating the mapping per type.
- **Temporal steps are not the bridge's concern.** The `effects` chain on the export `VideoLayer` also carries
  `Trim`/`ResetPts`/`OffsetPts`/`Speed` and a trailing `XFade`; these are decode-scheduling / timing concerns.
  The GPU compositor operates on **already-decoded per-layer frames at a given time `t`**. Consequently the GPU
  export path is structurally closer to the preview runner (decode each source at `t`, then composite one
  frame) than to `MultiTrackComposer`'s fused decode+composite graph. Temporal resolution and decode
  scheduling stay upstream; the bridge consumes the resolved per-frame layer set (each layer = a decoded
  `VideoFrame` + spatial transform + opacity + blend + the spatial subset of the effect chain).

## Execution model

Per composited frame:

1. For each layer, in `z_order` (bottom to top), apply the layer's **mappable spatial effect steps** to its
   decoded source frame with a per-layer `ff_render::RenderGraph` (v1: `ColorGradeNode`, `ScaleNode`; more as
   coverage grows, #1630). A layer with no mappable effects passes its decoded frame through unchanged.
2. Wrap each processed frame in an `ff_render::FrameLayer { frame: VideoFrame, transform: LayerTransform{ x,
   y, scale_x, scale_y, rotation }, blend_mode: ff_render::BlendMode, opacity: f32, z_order: i32 }`.
3. `ff_render::Compositor::composite(&mut [FrameLayer]) -> wgpu::Texture` composites the z-ordered stack (it
   sorts by `z_order`, ingests each layer -- planar YUV via `YuvUploadNode`, packed RGB CPU-side -- applies its
   `TransformNode`, and blends).
4. Deliver the result:
   - **export (Br4 v1):** the GPU export composites each output frame with the shared `GpuCompositor` (the same
     executor and identity/aspect gate as preview), reads it back with `Compositor::composite_to_rgba`, and
     pushes the `rgba` `VideoFrame` to the **existing encoder unchanged** (the encoder's own sws converts
     `rgba` -> `yuv420p`). `MultiTrackComposer` fuses decode and composite and never exposes a per-layer frame,
     so the GPU export cannot drive it; `avio::gpu_export` runs its own deterministic per-source decode loop
     (`ff_decode::VideoDecoder` per clip, decoded straight to `rgba`, one frame per output frame at
     `t = frame_idx / fps`). Eligibility is a **whole-export** decision (`avio::gpu_export::eligible_track`): v1
     covers a single active video track of contiguous hard cuts at unity speed whose every clip is a file source
     mapping to an identity, canvas-aspect GPU layer; anything else -- or no adapter, or `render_forcing_cpu` --
     keeps the whole export on `MultiTrackComposer`. Multi-track / overlay GPU export and zero-copy
     GPU->encoder are deferred.
   - **preview (Br3 v1):** `Compositor::composite_to_rgba` (composite + readback) -> the existing
     `FrameSink::push_frame`, so any sink works. The GPU compositor is injected into `ff_preview`'s runner via
     the `PreviewCompositor` seam (the runner cannot depend on `ff-render` directly); the runner tries it per
     frame and falls back to the CPU compositor on `None`. **Deferred:** the zero-copy `push_frame_gpu` /
     `GpuFrameSink` / `display`-feature path (hand a `wgpu::Texture` to the sink without readback).

   **v1 layer coverage (Br3 preview / Br4 export, shared core):** the GPU path renders only layers that need no
   geometric placement -- an identity transform and a frame whose aspect matches the canvas. A non-identity
   transform (the model's pixel/degree units do not yet map to the compositor's UV-space/radian
   `LayerTransform`) or an aspect mismatch (the compositor stretches to the canvas where the CPU path
   letterboxes) falls back to CPU -- per frame in preview, and by making the timeline ineligible (whole-export
   CPU fallback) in export. Correct GPU transforms and letterboxing, with GPU-vs-CPU parity tests, are Br5.

`ff_render::Compositor::new` and `RenderGraph::new` both take an `Arc<RenderContext>`; the bridge builds one
`RenderContext` per session (`RenderContext::init().await`, or `RenderContext::new(device, queue)` to share a
window's device) and reuses it across frames. The `TexturePool` inside `RenderContext` keeps steady-state
allocation at zero.

**Known v1 inefficiency (deferred):** applying per-layer effects with a `RenderGraph` and then feeding a
CPU `VideoFrame` into the `Compositor` incurs a GPU->CPU->GPU roundtrip for each effected layer, because the
`Compositor` ingests `VideoFrame`, not a texture. A fused per-layer path (or a `Compositor` that accepts
textures) is a later optimization, out of scope for the bridge.

## Node coverage

`ff-render`'s node set covers a subset of `avio`'s derived vocabulary. A derived construct either maps to a
node or forces CPU fallback for the whole frame (see below). The mapping **never silently drops** an
unsupported step. The table is the **v1** state (Br2, `avio::gpu::map_scene`); broadening the covered set is
tracked in **#1630**.

| Derived construct (source) | v1 mapping | Status |
|---|---|---|
| `x`/`y`/`scale`/`rotation` transform | evaluated at frame `t` -> the layer's `LayerTransform` scalars | **covered** (all layers) |
| `opacity` | evaluated at `t` -> `FrameLayer.opacity` | **covered** (all layers) |
| `blend_mode: ff_filter::BlendMode` (40) | `ff_render::BlendMode`, **all 40** (#1669; 14 before that) | **covered** (every mode; no blend mode forces a fallback). The GPU formulas reproduce `FFmpeg`'s `vf_blend`, so the GPU and the CPU compositor render a mode alike -- see [ADR-0010](../adr/0010-gpu-blend-modes-follow-ffmpeg.md) for the reference, the `A`=base/`B`=overlay pad wiring, and the bitwise-mode exception. (`ff_render` also has `Hue`/`Saturation`/`Color`/`Luminosity`, but `ff_filter::BlendMode` has no such variants -- removed in #1219 -- so they are unreachable from the derived scene.) |
| `composite_op: ff_filter::CompositeOp` (6) | `ff_render::CompositeOp`, **all 6** (#1670; `Over` only before that) | **covered** (every operator). The GPU evaluates the W3C Porter-Duff `Fa`/`Fb` definitions on premultiplied colour, which needs the coverage alpha #1750 added. The filter path builds `In`/`Out`/`Atop`/`Xor` from `blend`'s `all_expr` on `yuv420p` instead, which is per-channel arithmetic rather than alpha compositing. Preview and export both composite on the GPU, so the split is not between them: **those four render one way with an adapter and another whenever the whole-frame fallback swaps in the filter path** (a headless machine, a forced-CPU run, a frame that falls back for an unrelated reason). ADR-0007 makes the CPU the correctness reference, so this is a gap to close, not a property to rely on -- tracked by #1753. |
| `FilterStep::Eq` (from a const `EffectKind::ColorCorrect`) | `GpuEffect::ColorGrade` -> `ColorGradeNode { brightness, contrast, saturation, temperature=0, tint=0 }` | **covered** |
| `FilterStep::EqAnimated` | `ColorGrade` (params at `t`) **only when gamma is neutral at `t`** (ff-render ColorGrade has no gamma) | **covered** (gamma-neutral); non-neutral gamma -> **fallback** |
| plain `FilterStep::Scale { width, height, algorithm }` | `GpuEffect::Scale` -> `ScaleNode` (ff-render uses a linear filter for all algorithms; `Fast` maps to `Bilinear`) | **covered** |
| temporal steps: `Trim` / `ResetPts` / `OffsetPts` / `Speed` | skipped (decode-scheduling, applied upstream) | **skipped** (not a fallback) |
| other colour: `Hue`, `Curves`, `Gamma`, `Vignette`, `WhiteBalance`, `ColorBalanceAnimated`, `ThreeWayCC`, `ParametricEq` | -- | **fallback** (#1630) |
| `FitToAspect` / `FillToAspect` (fit with pad/crop) | -- | **fallback** (#1630; `ScaleNode` is a plain resize) |
| animated geometry `ScaleAnimated` / `RotateAnimated` | -- | **fallback** (#1630; ADR-0005 neutralizes the scalar, so v1 falls back rather than lose the animation) |
| xfade (`XFade`, any kind) | -- | **fallback** (#1630; needs the 2-input `CrossfadeNode`) |
| keying / masks: `ChromaKey`, `ColorKey`, `AlphaMatte`, `LumaKey`, `RectMask`, `FeatherMask`, ... | -- | **fallback** (#1630; `ChromaKey` needs a colour-string parser, masks need 2-input wiring) |
| everything else (`GBlur`, `Lut3d`, `NoiseReduce`, `Raw`, ...) | -- | **fallback** (#1630) |

**Known ff-render gaps to design around:** `YuvUploadNode` uses a **BT.601** conversion only (no BT.709
selection), and `ScaleNode`'s Bicubic/Lanczos fall back to a linear filter on the GPU. These are `ff-render`
limitations, not bridge bugs; the bridge documents them and the CPU path remains exact.

## GPU-vs-CPU selection (whole-frame fallback)

Fallback is decided **per composited frame**, at whole-frame granularity:

- If `RenderContext::init()` fails (no adapter) or a force-CPU override is set, **every** frame composites on
  the existing CPU path.
- Otherwise, before compositing a frame, a **capability check** walks the frame's layer set. If any layer
  carries a step with no node in the table above, that
  **whole frame** composites on the existing CPU compositor (`MultiTrackComposer` for export,
  `RealtimeComposer` for preview) instead of the GPU path. Otherwise the frame goes GPU.
- A `GpuFrameSink`-style degrade also applies at runtime: a GPU error on a frame falls through to the CPU path
  for that frame rather than erroring.

Whole-frame (not per-layer) fallback keeps the CPU path as a single, consistent correctness reference and
avoids mixing GPU and CPU colour spaces within one frame. Per-layer hybrid compositing is out of scope.

## Fallback boundary and parity

- The **CPU compositor is the correctness reference.** The GPU path must match it within tolerance for the
  supported node set; cross-driver / cross-adapter differences within tolerance are accepted (they are not a
  regression).
- The capability check is the single gate: a frame is GPU only if every step maps. This guarantees the GPU
  path never approximates or drops an unsupported effect -- it defers to CPU instead.
- Br5 (#1628) confirms this in `crates/avio/tests/gpu_parity_tests.rs`:
  - **Parity** compares the GPU compositor and the CPU `RealtimeComposer` directly in rgba (no encode/decode
    noise) over the same `RealtimeLayer`. Identity passthrough is pixel-exact on the dev build (GPU vs input
    and GPU vs CPU both mean 0.0; guarded at mean <= 2.0); a `ColorGrade` (`eq`) is mean ~6.6 (guarded at
    <= 20.0, looser because `ColorGradeNode` and FFmpeg `eq` are different implementations). The GPU **export**
    drain composites through the same `GpuCompositor`, so this covers the export compositing math; the
    end-to-end export-vs-force-CPU smoke lives in `gpu_export_tests.rs`. The tolerance asserts are the
    divergence regression guard.
  - **Fallback** asserts the compositor returns `None` (never panics) for every unsupported input
    (non-identity transform, aspect mismatch, unsupported effect), and the preview runner
    keeps advancing and terminates (never hangs, RK-019) when it falls back; `render_forcing_cpu` and the
    ineligible-timeline gate route export to CPU.
  - Both parity legs are double-gated (RK-002): the GPU leg needs an adapter, the CPU leg needs an
    FFmpeg-with-filters (`RealtimeComposer` is libavfilter-based). Each skips gracefully, so the real parity
    runs on a full dev build / macOS CI and the suite stays green on headless / minimal CI. Exact pixel
    equality across GPU drivers is not asserted (`docs/rules/test.md`).

## Deferred beyond v0.18.0

- Full node coverage (blur/LUT/glow/curves colour science, xfade kinds on GPU, BT.709 YUV upload). The
  blend modes landed in #1669 and the Porter-Duff operators in #1670.
- Zero-copy GPU->encoder for export (v1 reads back to CPU and reuses the existing encoder).
- Exact preview==export pixel convergence (the CPU compositors themselves are not bit-identical across the
  rgba/yuv420p seam, per the C4 Q2 deferral in `engine-and-primitives.md`).
- Per-layer hybrid GPU/CPU compositing and the per-effected-layer readback optimization.

## References

- [ADR-0007](../adr/0007-gpu-compositing-bridge.md) (the decision and rationale).
- Bridge tracking issue #1365; sub-steps Br1-Br5 (#1624-#1628); milestone tracker #1593.
- `ff-render` node/compositor API (`crates/ff-render/src/{compositor,graph,nodes,sink,context}`),
  `avio::derive` (`crates/avio/src/derive.rs`), the CPU compositors
  (`ff_filter::MultiTrackComposer` / `RealtimeComposer`).
