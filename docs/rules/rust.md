# Rust Coding Standards

> Scope: the whole avio workspace (`avio` engine facade + `ff-sys` / `ff-common` / `ff-format` /
> `ff-probe` / `ff-decode` / `ff-encode` / `ff-filter` / `ff-pipeline` / `ff-stream` / `ff-preview`
> / `ff-render`).
> Related: [design.md](./design.md) (design / API), [error-handling.md](./error-handling.md),
> [unsafe.md](./unsafe.md), [perf.md](./perf.md), [logging.md](./logging.md).

## References

- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- [Rust Performance Book](https://nnethercote.github.io/perf-book/)

---

## Formatting

- Use `rustfmt` with its default settings.
- CI runs `cargo fmt --check` and blocks on formatting violations.

## Naming

| Target | Convention | Example |
|---|---|---|
| Types / traits | UpperCamelCase | `VideoDecoder`, `FilterGraph` |
| Functions / methods | snake_case | `decode_frame`, `push_video` |
| Constants | SCREAMING_SNAKE_CASE | `MAX_BITRATE` |
| Modules | snake_case | `decoder_inner`, `filter_inner` |
| Files | snake_case | `decoder_inner.rs` |

## Module layout

```
src/
  lib.rs          <- public API surface (via `pub use`)
  error.rs        <- error type definitions
  {feature}/
    mod.rs        <- public types/functions (safe API)
    {feature}_inner.rs <- unsafe FFmpeg calls (`pub(crate)` only)
```

All `unsafe` FFmpeg calls live in `*_inner.rs` (see [unsafe.md](./unsafe.md)).

## Builder pattern

Use consuming builders (they take `self`). See [design.md](./design.md) for when a builder is
warranted.

```rust
pub struct VideoEncoderBuilder { ... }

impl VideoEncoderBuilder {
    pub fn codec(self, codec: VideoCodec) -> Self { ... }
    pub fn bitrate(self, bps: u64) -> Self { ... }
    pub fn build(self) -> Result<VideoEncoder, EncodeError> { ... }
}
```

- `.build()` always returns `Result<T, Error>`.
- Validation is collected in `.build()`, not in the setters.

## Visibility

| Scope | Use |
|---|---|
| `pub` | Types/functions exposed outside the crate |
| `pub(crate)` | Types/functions used only within the crate (the contents of `*_inner.rs`) |
| `pub(super)` | Exposed only to the parent module |
| none (private) | Module-internal only |

---

## Concurrency & runtime

### FFmpeg contexts are `Send` but not `Sync`

FFmpeg context types (`AVCodecContext`, `AVFilterGraph`, …) are not safe for concurrent access,
but ownership can move between threads. The inner types implement `Send` only, never `Sync`
(see [unsafe.md](./unsafe.md)). Do not share a decoder / encoder / filter graph across threads
without external synchronization; give each thread its own.

### avio is runtime-agnostic

The `ff-*` crates do not hard-depend on a specific async runtime. The `tokio` feature adds thin
async wrappers (backed by `spawn_blocking` and bounded channels); the runtime belongs to the
application. Do not pull `tokio` into a crate's non-optional dependencies.

### `rayon` for CPU-bound batches

Parallel, CPU-bound batches (e.g. multi-timestamp thumbnail extraction) may use `rayon`. Each
worker opens its **own** decoder — never share an FFmpeg context between workers. Collect the
results, then reorder. Do not manage OS threads by hand.

---

## Cross-platform

Keep code portable even when a feature targets one platform first.

- File paths use `std::path::Path` / `PathBuf`. Never write OS separators as string literals.
- Localize OS-specific branches (per-platform hardware backends: NVENC / QSV / AMF /
  VideoToolbox / VAAPI) behind the existing abstractions.

```rust
// Bad — hardcoded separator
let p = dir.to_string() + "/out.mp4";
// Good
let p = dir.join("out.mp4");
```

---

## Code quality

### Prefer iterators over manual loops

```rust
let audio_indices: Vec<_> = info.streams().iter()
    .filter(|s| s.is_audio())
    .map(|s| s.index())
    .collect();
```

### Annotate non-obvious clones

```rust
// clone required: the rayon worker needs an owned 'static + Send value
let path = source.clone();
```

### `unsafe` requires justification

Every `unsafe` block has a `// SAFETY:` comment stating the invariant. Details in
[unsafe.md](./unsafe.md).

### No dead code on committed branches

Remove unused `use` / functions / variables. If `#[allow(dead_code)]` is genuinely needed, add a
reason comment.

---

## Comments

- Comments explain **why**, not the obvious **what**.
- Non-obvious algorithms, FFmpeg quirks, or binary layouts may use a multi-line block comment when
  one line will not do.

```rust
// OK — non-obvious why
// asetrate changes the declared sample rate (which shifts pitch); atempo then
// restores the original duration.
let rate = 2f64.powf(f64::from(semitones) / 12.0);

// Bad — obvious what
// loop over frames
for frame in frames { ... }
```

---

## Do not

- Do not use `unwrap()` / `expect()` in library code (under `src/`). Allowed only under
  `#[cfg(test)]`. The one exception is a true, construction-guaranteed invariant, using `expect`
  with an `// INVARIANT:` comment (see [error-handling.md](./error-handling.md)); do not abuse it.
- Do not use `println!` / `eprintln!` in library code (use `log::`, see [logging.md](./logging.md)).
- Do not reach for `#[allow(dead_code)]` casually.
- Do not create circular dependencies. Dependency direction and crate boundaries are in
  [design.md](./design.md).
