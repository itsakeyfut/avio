---
status: "accepted"
date: 2026-09-06
decision-makers: itsakeyfut
---

# Defer the zero-copy GPU-to-encoder handoff; export keeps reading back to the CPU

## Context and Problem Statement

`GpuCompositor::composite` returns its result as a CPU buffer, `Option<(Vec<u8>, u32, u32)>`
(`crates/avio/src/gpu_compositor.rs`). Export wraps that with `VideoFrame::from_rgba` and pushes
it into `VideoEncoder::push_video` (`gpu_export.rs`). **Preview goes through the same function**
(`gpu_preview.rs` calls `self.core.composite(...)`), so the readback is a property of the shared
compositor rather than of export, and both paths pay it every frame. ff-render's own
`GpuFrameSink` has a zero-copy path for a display downstream (#1609), but avio's preview adapter
does not go through it. [ADR-0007](./0007-gpu-compositing-bridge.md) chose this shape for v1 and
named the readback as the known cost.

#1662 asks whether the readback can be removed by handing the composited texture straight to a
hardware encoder, and asks for a proceed/defer decision plus the seams such a change would need.
It scopes the question to export and this record keeps that scope, noting where the shared cost
bears on the size of the prize. This record answers the question and changes no behaviour.

The measurements below were taken on the development machine (Windows 11, NVIDIA RTX 3070 Ti,
vcpkg FFmpeg 8.0.1, wgpu 30.0.0), which is the only environment in the project where a hardware
encoder exists at all. That single-platform basis is itself part of the decision.

## Decision Drivers

* The size of the prize, measured rather than assumed.
* Whether the route exists end to end, verified against the pinned FFmpeg source and the wgpu
  and wgpu-hal sources rather than against documentation.
* Whether a change could be **validated** anywhere the project actually builds. The Linux CI
  FFmpeg is `--disable-everything`, and the Windows CI job compiles with `DOCS_RS=1` against
  pointer stubs instead of linking FFmpeg (`.github/workflows/ci.yml`, the `check-windows` job).
* How many seams it opens across `ff-sys`, `ff-render` and `ff-encode`, and how much of the
  result would be reachable by users.

## Considered Options

* **Defer**, keeping composite → readback → existing encoder.
* **Proceed now** with a D3D12 shared-device path feeding `hevc_d3d12va`.
* **Proceed narrowly**: build only the `ff-sys` hardware-frames layer, leaving the handoff for
  later.

## Decision Outcome

Chosen option: **defer the zero-copy handoff**, primarily because **no environment the project
builds in could test the result**, and secondarily because it requires four new seams across three
crates while the measured saving is on the composite stage rather than on export as a whole.

This defers one *route*, not the problem. Most of the readback's cost turns out to be a CPU copy
that can be attacked without any of the blockers below (see *A cheaper route*).

The reach argument needs stating carefully. In the pinned vcpkg build the only hardware encoder
is `hevc_d3d12va`, so this repository could exercise the path in exactly one configuration:
HEVC, on Windows, over D3D12. That is a property of *our* build, not of the feature. avio is a
library and users link their own FFmpeg; `crates/ff-encode/src/lib.rs` advertises NVENC, QSV,
AMF, `VideoToolbox` and VA-API, so a user build would commonly carry encoders this one does not.
The limitation that actually justifies deferring is therefore testability, not user reach.

The readback is not cheap, and this record should not be read as saying it is. Measured at
1920×1080 over 60 frames after warm-up, on a one-node graph, in **release** (the workspace sets no
`[profile.dev]` optimisation override, so a debug run leaves wgpu unoptimised too):

| Path | Per frame |
|---|---|
| Composite, without waiting for the GPU | 0.68 ms |
| Composite, waiting for the GPU to finish it | 0.90 ms |
| Composite + readback (`RenderGraph::process_gpu`) | 3.00 ms |
| **Readback cost** | **~2.1 ms, 70 % of the stage** |

The middle row matters. `process_gpu_to_texture` submits its passes and returns **without
polling**, while `process_gpu` blocks until the GPU has finished everything, so subtracting one
from the other charges the readback for compositing work the other path merely had not waited for.
An earlier revision of this record did exactly that and reported 80 %. Matching the waits gives
70 %. In debug the same comparison reads 68 %, and optimisation moves the *composite* side far
more than the readback side, because the readback is dominated by transfer and by a CPU copy
rather than by the Rust around them.

This cost is paid by preview as well as export, since both go through `GpuCompositor::composite`.
At 60 fps it is roughly 12 % of a preview frame budget.

What blocks acting on it *by way of a zero-copy handoff* is not the size of the prize but the
testability, and the number of seams that would have to land at once. A cheaper route is set out
below.

### What is actually available

Enumerating every encoder in the pinned build (`av_codec_iterate` + `avcodec_get_hw_config`)
rather than guessing names: **190 encoders, exactly one with a hardware configuration**:
`hevc_d3d12va`, device type `d3d12va`. There is no NVENC, QSV, AMF or VA-API encoder, and no
hardware **H.264** encoder at all. `av_hwdevice_iterate_types` reports three device types:
`dxva2`, `d3d11va`, `d3d12va`; all three create successfully against the RTX 3070 Ti.

A string search over `avutil` had suggested Vulkan, CUDA and VA-API support as well. That probe
was wrong: its negative control matched too (`videotoolbox` and `vaapi` appear on Windows, where
they cannot be compiled in), because hardware type names live in a table that is compiled
unconditionally. `av_hwdevice_find_type_by_name("cuda")` likewise returns a non-`NONE` value for
a backend `av_hwdevice_iterate_types` does not list. Only the iteration is evidence.

Meanwhile wgpu selects **Vulkan** by default here. Dx12 is available on the same GPU but is not
what `Backends::all()` picks, so the two sides do not currently meet.

### The route, if it were taken

Every piece exists. The natural direction is the opposite of the issue's framing, because the
d3d12va frames pool allocates its own resources: FFmpeg allocates the texture and wgpu imports
it. Whether the reverse works through `AVHWFramesContext`'s user-settable `pool` was not
investigated.

1. `AVD3D12VADeviceContext.device` is documented in the pinned header as *"Can be set by the
   user. This is the only mandatory field"*, so FFmpeg can be given wgpu's `ID3D12Device`
   (reached through `wgpu::Device::as_hal::<Dx12>`) and the two share one device.
2. `av_hwframe_get_buffer` allocates an `AVD3D12VAFrame` whose `ID3D12Resource` FFmpeg creates
   itself (`hwcontext_d3d12va.c` calls `ID3D12Device_CreateCommittedResource`).
3. `wgpu_hal::dx12::Device::texture_from_raw` plus `wgpu::Device::create_texture_from_hal`
   wrap that resource as a `wgpu::Texture`, so the compositor renders into the encoder's frame.
4. `hevc_d3d12va` encodes it with no readback.

`RenderGraph::process_gpu_to_texture` and `TextureHandle` (#1609) are the existing precedent for
moving a composited texture out of the pool without a readback.

### A cheaper route that shares none of these blockers

Framing the choice as "zero-copy or nothing" would be wrong, and the measurements say why.
Breaking the ~2.1 ms down: **about 1.5 ms of it is a plain CPU copy** out of the mapped staging
buffer into a freshly allocated `Vec<u8>` (`graph_inner.rs` builds the output row by row; measured
against a single `memcpy` of the same 8.3 MB, the loop is not the cost, the copy is). The
remaining ~0.6 ms covers the per-frame staging-buffer allocation, the texture-to-buffer copy, the
map, and the stall.

So the dominant term is not the GPU transfer at all. It is CPU-side work that needs **no hardware
encoder, no D3D12, no shared device, and no new FFI**, is platform-independent, and is testable in
CI exactly as it stands today. Candidates, none of them measured yet:

* Reuse the output buffer instead of allocating a `Vec` per frame, or hand the caller the mapped
  range so the copy disappears rather than being made cheaper.
* Pool the staging buffer the way textures are already pooled; `graph_inner.rs` calls
  `create_buffer` on every readback.
* Overlap the stall: `poll(wait_indefinitely)` serialises CPU and GPU completely.

Deferring the zero-copy handoff therefore does **not** mean the readback has to stay as expensive
as it is. That work is tracked as #1777.

### The seams a proceed would require

**ff-sys**
* `wrapper.h` includes no `hwcontext_*.h`; `AVD3D12VADeviceContext` and friends are **0 hits** in
  the generated bindings. Adding the header pulls in the Windows SDK `<d3d12.h>`, so it must be
  target-gated. `build.rs` already branches on `CARGO_CFG_TARGET_OS`, but the bindings would
  become platform-shaped for the first time.
* No `HwFramesContext` owned type and no `CodecContext::set_hw_frames_ctx`. `HwDeviceContext`
  exists but its constructor takes only a device *type*, so it cannot wrap an existing device;
  a shared-device path needs a second constructor.
* A COM lifetime hazard to get right: the header states that deallocating the
  `AVHWDeviceContext` *"will always release this interface, and it does not matter whether it was
  user-allocated"*. Handing over wgpu's device without an `AddRef` would have FFmpeg release a
  device wgpu still owns.

**ff-render**
* `wgpu-hal` is not a direct dependency, so `as_hal` is not reachable.
* The compositor would have to be pinned to `Backends::DX12`. `RenderContext::init_with_backend`
  already exists, but every existing GPU test would then run on a different backend than today.
* Effect nodes hardcode `Rgba8Unorm` colour targets. `hevc_d3d12va` declares
  `CODEC_PIXFMTS(AV_PIX_FMT_D3D12)` (`libavcodec/d3d12va_encode_hevc.c`), so it takes hardware
  frames only, and the NV12 the hardware wants is the `sw_format` of the frames context. Either
  way a GPU colour conversion node does not exist yet.

**ff-encode**
* No hardware-frame path of any kind: `hw_frames_ctx`, `hw_device_ctx` and `av_hwframe_*` appear
  **0 times** in `crates/ff-encode/src`. `HardwareEncoder` selects an encoder *name* and nothing
  more, so today a hardware encoder that accepts software frames just has the driver re-upload
  them, and one that does not, like `hevc_d3d12va`, cannot be fed at all.
* `HardwareEncoder`'s variants are `Nvenc`, `Qsv`, `Amf`, `VideoToolbox` and `Vaapi`
  (`crates/ff-encode/src/shared/hardware.rs`). There is **no D3D12VA variant**, so the one
  hardware encoder present in the pinned build cannot be selected even before the frame question
  arises.
* `ff-decode`'s `decoder_inner/hardware.rs` is the in-repo model for the device half.

### Confirmation

Nothing fails if this decision is violated, because it forbids no code, and that is the honest
answer. What holds the shape in place is that `GpuCompositor::composite` returns a CPU buffer and
`gpu_export.rs` builds a `VideoFrame` from it; any zero-copy path has to change that signature,
which is a visible, reviewable event rather than a silent drift. The measurements above are
reproducible with the probe recorded in *More Information*.

### Consequences

* Good, because export keeps one code path on every platform, and the compositor keeps its
  default backend.
* Good, because the milestone does not take on a feature that no CI job could regression-test.
* Bad, because 70 % of the composite stage stays as copy, in preview as well as export. Mitigated
  rather than accepted: most of it is reachable by the cheaper route above.
* Bad, because the gap widens quietly: each new effect node makes the composite cheaper relative
  to a readback that stays constant.
* What would reverse this: an FFmpeg build in the project's own toolchain carrying hardware
  encoders for more than HEVC-on-D3D12 (which also makes the path testable); or a concrete user
  need for hardware HEVC export on Windows; or a measurement showing the readback is a
  significant share of **total export time**, not just of the composite stage. That number was
  not taken here and would be the first thing a proceed decision needs.

## Pros and Cons of the Options

### Defer

* Good, because it costs nothing and keeps one path.
* Good, because the analysis is written down, so a later attempt starts from the seam list rather
  than from scratch.
* Bad, because a measured ~2.1 ms per 1080p frame stays on the table, in preview as well as
  export, until the cheaper route is taken.

### Proceed now

* Good, because the route is real and every piece was verified to exist.
* Bad, because no CI job can test it: Linux CI has no hardware encoder and the Windows job never
   links FFmpeg, so the pinned build could exercise it only as HEVC over D3D12 on Windows.
* Bad, because the shared-device requirement makes it hard to land incrementally: it works or it
  does not.
* Bad, because it forces the compositor off its default backend, changing what every existing GPU
  test exercises.

### Proceed narrowly (ff-sys layer only)

* Good, because `HwFramesContext` and a shared-device constructor are useful on their own, and
  hardware *decode* already exists to exercise them.
* Bad, because on its own it removes no copy, so it buys API surface without the benefit.
* Bad, because a frames layer designed without its consumer tends to fit the consumer badly.

## More Information

* Issue #1662; the cheaper readback work split out as #1777; milestone tracker #1593; [ADR-0007](./0007-gpu-compositing-bridge.md) for the
  readback decision this revisits, and `docs/specs/gpu-compositing-bridge.md` for the deferral
  list this record now backs.
* Code: `crates/avio/src/gpu_compositor.rs` (`composite`), `crates/avio/src/gpu_export.rs`,
  `crates/ff-render/src/graph/mod.rs` (`process_gpu_to_texture`), `crates/ff-render/src/sink/`
  (#1609's zero-copy display path), `crates/ff-sys/src/hwdevice.rs`, `crates/ff-sys/wrapper.h`,
  `crates/ff-decode/src/video/decoder_inner/hardware.rs`,
  `crates/ff-encode/src/shared/hardware.rs`.
* Pinned FFmpeg 8.0.1 sources read for this record: `libavutil/hwcontext_d3d12va.h` (the
  user-settable `device` field and its release semantics) and `libavutil/hwcontext_d3d12va.c`
  (the frames pool creating its own committed resources).
* wgpu 30.0.0 / wgpu-hal 30.0.0 sources: `wgpu::Device::as_hal`,
  `wgpu::Device::create_texture_from_hal`, `wgpu_hal::dx12::Device::texture_from_raw`.
* The measurements come from a throwaway probe run once and not committed. It enumerates
  hardware device types with `av_hwdevice_iterate_types`, creates a context for each, lists every
  encoder carrying an `avcodec_get_hw_config`, prints the wgpu adapter chosen under
  `Backends::all()`, and times three variants over 60 frames at 1920×1080 after warm-up:
  `process_gpu_to_texture` alone, the same followed by `device.poll(wait_indefinitely)` so the GPU
  work is actually waited for, and `process_gpu`. The middle variant is what makes the readback
  share honest. A separate run timed a row-by-row assembly of 8.3 MB against a single `memcpy` to
  attribute the CPU share. Re-creating all of it is a few dozen lines against `ff-sys`, `ff-render`
  and `wgpu`; the numbers are quoted in full above so the record stands without it.
