# GPU / wgpu Conventions

> GPU compositing lives in `ff-render` (wgpu). Shader **code** conventions are in
> [wgsl.md](./wgsl.md). Related: [perf.md](./perf.md), [unsafe.md](./unsafe.md).

## References

- [wgpu](https://docs.rs/wgpu) / [WGSL spec](https://www.w3.org/TR/WGSL/)
- [WebGPU best practices](https://toji.dev/webgpu-best-practices/)

---

## 1. Device / queue ownership

- A single wgpu `instance` / `adapter` / `device` / `queue`, owned once and shared for all
  compositing work.
- Pipelines, bind group layouts, and reusable buffers are created once and reused.

## 2. Resource lifecycle and recreatability

Keep every GPU resource **recreatable from a CPU-side description** (for device loss).

- Pipelines, bind group layouts, buffers, and offscreen targets are regenerable from their
  descriptors and data source.
- Detect device loss -> recreate the device -> rebuild and re-upload all resources -> continue.
- Do not create-and-drop per frame; reuse growable buffers (see [perf.md](./perf.md)).
- **Label every resource** for debugging and profiling.

```rust
let buf = device.create_buffer(&wgpu::BufferDescriptor {
    label: Some("ffrender.composite.instances"),
    size,
    usage,
    mapped_at_creation: false,
});
```

## 3. Pipelines

- Cache pipelines; do not build them per frame.
- WGSL is validated at build time (Naga), so shader errors surface early in dev.

## 4. Instance data (bytemuck)

- Structs sent to the GPU are `#[repr(C)]` + `bytemuck::Pod` / `Zeroable`, uploaded with
  `cast_slice`.
- Field order, size, and padding must match the WGSL `struct` exactly (16-byte alignment; see
  [wgsl.md](./wgsl.md)). A mismatch is a silent rendering bug.
- `bytemuck`'s derive is not hand-written `unsafe`; do not write byte-cast `unsafe` yourself.

```rust
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerInstance {
    transform: [f32; 4], // keep field order/size in sync with the WGSL struct
    opacity: f32,
    _pad: [f32; 3],      // explicit padding to a 16-byte boundary
}
```

## 5. Color

- Composite video in a **linear working space**. Do not blend in an sRGB-encoded space.
- Frame textures carry their color space: convert to the working space on sample, and encode on the
  final output pass. Do not hardcode the working primaries.
- Distinguish display-referred UI color from video color; do not mix them.

## 6. CPU <-> GPU synchronization

- Respect submission order and fences: do not sample a texture whose write has not completed.
- Read `map_async` results only after `queue.submit` + `device.poll`.

## 7. Memory

- Reuse growable buffers; do not allocate GPU resources per frame (see [perf.md](./perf.md)).
- Free textures/buffers that are no longer needed; do not leak.

## 8. unsafe

wgpu's API is safe: prefer the safe `create_surface` and safe resource APIs. Any hand-written
`unsafe` (rare, e.g. a surface-from-raw-handle path) is isolated with a `// SAFETY:` comment (see
[unsafe.md](./unsafe.md)).
