# Unsafe Code Conventions

> avio wraps FFmpeg's C libraries, so **`unsafe` is central**: every FFmpeg call is `unsafe`. The
> rule is not "avoid `unsafe`" but "**isolate and justify** it." Related: [rust.md](./rust.md); GPU
> `unsafe` in [gpu.md](./gpu.md).

## Principles

- Isolate all `unsafe` in `*_inner.rs` files, kept `pub(crate)`. Do not leak `unsafe` out of `pub`
  functions — the external API is entirely safe.
- Every `unsafe` block and `unsafe impl` has a `// SAFETY:` comment stating why it is sound. An
  `unsafe` block with no such comment must be flagged in review.
- The pure-Rust crates (`ff-format`, `ff-common`) contain **no `unsafe`**.
- A change that adds or alters `unsafe` triggers Tier-2 review (`/code-review-deep`).

## Lints

The workspace sets `unsafe_code = "warn"` (not `deny`), because the `ff-sys` FFI bindings and the
`*_inner.rs` layer legitimately use `unsafe`. Keep it confined there; the safe layers stay
`unsafe`-free.

---

## SAFETY comments

Immediately before an `unsafe` block, write the soundness argument.

```rust
// SAFETY: ptr is non-null because avcodec_alloc_context3 succeeded,
//         and we are the sole owner of this context.
unsafe { avcodec_free_context(&mut self.ctx.as_ptr()) };
```

---

## Pointer management

### Null-check on the inner side

When an FFmpeg function may return null, check it on the inner side and convert it into an error.
Do not hand raw pointers to the outer layer.

```rust
// inside filter_inner.rs
let graph = unsafe { avfilter_graph_alloc() };
if graph.is_null() {
    return Err(FilterError::Ffmpeg { code: -1, message: "avfilter_graph_alloc failed".into() });
}
```

### Wrap pointers in `Option<NonNull<T>>` or a newtype

Do not hold a raw `*mut AVCodecContext` as a struct field.

```rust
use std::ptr::NonNull;
struct CodecContextPtr(NonNull<AVCodecContext>);
```

### Free in `Drop` with `take()`

To prevent double frees, invalidate the pointer after freeing.

```rust
impl Drop for FilterGraphInner {
    fn drop(&mut self) {
        if let Some(ptr) = self.graph.take() {
            // SAFETY: graph is non-null (guaranteed by Option), and we own it.
            unsafe {
                let mut raw = ptr.as_ptr();
                avfilter_graph_free(&mut raw);
            }
        }
    }
}
```

---

## FFmpeg ownership and lifetimes

- Reference-count frames/packets with `av_frame_ref` / `av_packet_ref` rather than deep-copying;
  release with the matching unref/free.
- After freeing an FFmpeg object, null the pointer (the `Option` + `take()` pattern) so it cannot be
  freed twice.
- **`CString` lifetime**: when passing `CString::new(...)`'s `.as_ptr()` to FFmpeg, keep the
  `CString` alive until the call returns. Do not let a temporary drop before the pointer is used.
- **Error-path leaks**: on every early return / `?` / `bail!`, free any FFmpeg resource acquired
  earlier in the same function.

## FFmpeg call order

The exact call order is specified per crate in `docs/crates/{name}/design.md` and is part of the
design (see [design.md](./design.md)). Deviating is a bug.

---

## Send / Sync

FFmpeg context types are not thread-safe for concurrent access, but moving them between threads is
safe. Implement `Send` only.

```rust
// SAFETY: AVCodecContext is not thread-safe for concurrent access, but ownership
//         transfer between threads is safe because Rust's ownership model
//         guarantees exclusive access.
unsafe impl Send for VideoDecoderInner {}
```

- Do not implement `Sync`.
- Every `unsafe impl` has a SAFETY comment.

---

## Enforce `pub(crate)`

Every function/type in `*_inner.rs` is `pub(crate)`. Do not expose them with `pub`.

```rust
// filter_inner.rs
pub(crate) struct FilterGraphInner { ... }
pub(crate) fn build_graph(...) -> Result<FilterGraphInner, FilterError> { ... }
```
