# Performance Rules

> Media processing is throughput-sensitive: the decode / filter / encode loop runs once per frame,
> often millions of times. Related: [rust.md](./rust.md), [gpu.md](./gpu.md) (GPU),
> [test.md](./test.md) (benchmarks).

## References

- [Rust Performance Book](https://nnethercote.github.io/perf-book/)
- [Criterion Benchmarking](https://bheisler.github.io/criterion.rs/book/)

---

## Principles

- Measure in release (`cargo build --release`). Do not micro-optimize on guesswork; benchmark the
  path first.
- The frame loop is the hot path. Keep it allocation-light.

---

## Hot paths (the frame loop)

### Avoid per-frame heap allocation

```rust
// Bad: allocate a fresh buffer every frame
let mut buf = vec![0u8; size];

// Good: reuse a buffer across frames (clear() keeps the capacity)
self.scratch.clear();
self.scratch.resize(size, 0);
```

### Reuse frame buffers via the pool

avio provides buffer pooling (`PooledBuffer`, `VecPool`, and the `FramePool` trait). Decode and
conversion paths draw buffers from a pool and return them, rather than allocating fresh every frame.

### Reference-count frames; do not deep-copy

Share frame/packet data with `av_frame_ref` / `av_packet_ref` (in the inner layer) instead of
copying pixel or sample data. Deep copies inside the frame loop are a common, avoidable cost.

### Build once, run many

Construct the `FilterGraph`, encoder, and decoder contexts **once** during initialization, then
push frames through them. Never rebuild a filter graph or reopen a codec inside the frame loop.

> `FilterGraph::build()` is lazy: it validates filter *names* and builds the FFmpeg graph on the
> first `push`. Still build it once, outside the loop. (`parse_desc` is the one step kind that also
> parses its description at `build()` — see ADR-0012 — which is another reason to keep `build()` out
> of the frame loop.)

---

## GPU

GPU compositing (`ff-render`) has its own discipline: reuse growable buffers, cache pipelines, batch
uploads, do not allocate GPU resources per frame. See [gpu.md](./gpu.md).

---

## Benchmarks (Criterion)

Put Criterion benches on the critical paths, under each crate's `benches/`.

| Crate | Target |
|---|---|
| `ff-decode` / `ff-encode` | decode / encode throughput; format conversion (swscale) |
| `ff-filter` | filter-graph build + per-frame push/pull |
| `ff-render` | GPU composite pass; buffer upload |

Run manually before/after a performance-relevant change:

```bash
cargo bench -p ff-encode -- --save-baseline before
# ...make the change...
cargo bench -p ff-encode -- --load-baseline before --save-baseline after
```

Benches are **not** run in CI; run them by hand around perf-relevant changes.

---

## Profiling

- Coarse phase timing (decode / filter / encode / mux) via `log` timing.
- Standard CPU profilers (`perf`, Instruments, VTune).
- GPU: RenderDoc, wgpu timestamp queries.
