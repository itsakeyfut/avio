# WGSL Coding Standards

> Shader **code** conventions for `ff-render`. GPU resource / pipeline conventions are in
> [gpu.md](./gpu.md). Related: [rust.md](./rust.md) (comment policy).

## References

- [WGSL spec](https://www.w3.org/TR/WGSL/) / [Naga](https://github.com/gfx-rs/naga) /
  [wgpu](https://docs.rs/wgpu)

---

## 1. File / module layout

- Shaders live under `ff-render/src/shaders/` as `.wgsl`.
- Shared utilities (color conversion, coordinate transforms) go in a prelude module, composed into
  each shader. Do not copy the same math into every file.
- Validate with Naga at build time (surface shader errors early in dev).

## 2. Naming

| Kind | Convention | Example |
|---|---|---|
| struct | PascalCase | `LayerInstance`, `Globals` |
| function | snake_case | `to_linear`, `sample_layer` |
| constants | UPPER_SNAKE | `PI` |
| variables / fields | snake_case | `opacity` |
| entry points | `vs_main` / `fs_main` | |

Match the corresponding Rust type name (`LayerInstance` <-> Rust `LayerInstance`).

## 3. Struct layout / alignment (most important)

A WGSL `struct` must match the byte layout of its Rust `#[repr(C)]` + `bytemuck::Pod` counterpart
([gpu.md](./gpu.md)). A mismatch is a silent rendering bug.

- **16-byte alignment.** A `vec3<f32>` aligns to 16 bytes and adds 4 bytes of trailing padding —
  pack into `vec4`, or add an explicit padding field.
- `var<uniform>` follows strict (std140-like) alignment; array elements align to 16 bytes.
- Cross-reference field order/size with a Rust-side comment.

```wgsl
// Matches Rust #[repr(C)] LayerInstance (field order and size kept in sync)
struct LayerInstance {
    transform: vec4<f32>,
    opacity:   f32,
    // avoid vec3; keep to 16-byte boundaries
};
```

```wgsl
// Bad: vec3 implicit padding shifts the next field
struct Bad { a: vec3<f32>, b: f32 };
```

## 4. Bindings

Stabilize the `@group` / `@binding` layout.

- **group 0 = frame-global** (viewport, output size, time) in `Globals`.
- **group 1 = per-batch resources** (the layer texture + sampler).
- Instance data is passed via the vertex buffer (`@location`).

```wgsl
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var layer_tex: texture_2d<f32>;
@group(1) @binding(1) var layer_sampler: sampler;
```

## 5. Coordinate system

- Fix one convention for the y-axis direction across the project and state it in the prelude comment.
- Convert to NDC (clip space) via `globals`.

## 6. Color

- Compute in **linear** inside the shader. Do not sRGB-encode in the fragment shader (a dedicated
  output pass does the final encode).
- Prefer premultiplied alpha; state the assumption in the prelude.
- Input colors / textures are converted to linear on the CPU side; do not hardcode a color-space
  conversion in the shader.

## 7. Control flow / Naga portability

- Keep branching minimal; favor arithmetic.
- Do not write unbounded loops.
- Mind backend differences (DX12 / Vulkan / Metal via Naga); do not rely on precision-dependent or
  exotic built-ins.

## 8. Comments

- Explain the **why** of non-obvious math (color transforms, blends). Multi-line block comments are
  allowed when one line will not do (see [rust.md](./rust.md)).
- Do not restate the obvious.

## 9. Testing

Cover shader visual output with tolerance-based comparison against reference frames where practical
(skip when no GPU adapter). See [test.md](./test.md).
