# Testing Standards

> Test the observable behaviour, not the implementation. Related: [perf.md](./perf.md) (benchmarks).

## References

- [Rust Testing Guide](https://doc.rust-lang.org/book/ch11-00-testing.html)
- [proptest Book](https://proptest-rs.github.io/proptest/intro.html)
- [Criterion Book](https://bheisler.github.io/criterion.rs/book/)

---

## Philosophy

Test **behaviour**. A good test breaks only when observable behaviour changes, not when an internal
field is renamed. Do not mock FFmpeg — call the real API.

## Naming

```
<feature>_should_<expected_result>
```

```rust
fn decode_h264_should_return_video_frame() { ... }
fn scale_filter_should_resize_frame_to_target_dimensions() { ... }
fn open_nonexistent_file_should_return_error() { ... }
```

---

## Test layers

### 1. Unit tests (pure logic; primary)

`#[cfg(test)] mod tests` in the source. Cover FFmpeg-free logic: type conversions, token mapping,
argument-string construction, math (keyframe interpolation, atempo decomposition).

Token / value regressions belong in **deterministic, build-independent string-equality unit
tests** — they must not depend on which FFmpeg is installed.

### 2. Integration tests (real FFmpeg)

`crates/<crate>/tests/`. Drive the real FFmpeg API against short fixtures.

**Skip when FFmpeg is unavailable:**

```rust
fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg").arg("-version").output().is_ok()
}
```

**Probe-gate filter-graph tests.** CI's Linux FFmpeg is built with no filters, so every
filter-graph `push` returns `FilterError::BuildFailed` there. A bare `.expect()` on a push fails
CI. Probe first and skip gracefully:

```rust
match graph.push_video(0, &frame) {
    Ok(()) => { /* assert output ... */ }
    Err(FilterError::BuildFailed) => { println!("skipping: filters unavailable"); return; }
    Err(e) => panic!("unexpected: {e}"),
}
```

Real filter-graph verification effectively runs only on a full FFmpeg build (macOS via Homebrew).

**`build()` is lazy.** `FilterGraph::build()` validates filter *names* only; argument *values* are
validated on the first `push`. A test that checks "FFmpeg accepts these args" must **push a frame**
— `build().expect(...)` proves nothing about the arguments.

**One exception: `parse_desc`.** A `FilterStep::ParseDesc` description is parsed at `build()`
(`avfilter_graph_parse2`), which resolves filter names *and applies their options*, so a bad option
name or value returns `InvalidConfig` there rather than on the first push (ADR-0012). Asserting on
`build()` is therefore correct for that one step kind — but gate it, because the check is skipped
when the filter registry is empty, which is exactly CI's Linux FFmpeg.

### 3. Property tests (proptest)

Verify invariants hold on arbitrary input (e.g. a parameter parser never panics; an interpolated
value stays in range).

### 4. Benchmarks (Criterion)

See [perf.md](./perf.md). Critical paths only; not run in CI.

---

## Fixtures

- **Small (<= 1 MB)**: commit under `tests/fixtures/`.
- **Large (> 1 MB)**: download in CI; tests read the path from `FF_TEST_LARGE_FIXTURE_DIR`.

## Integration test policy

- No mocks — call the real FFmpeg API.
- Short 1-3 second samples.
- Independent (no ordering dependencies).
- Use `tempfile` for temporary outputs (auto-deleted).

---

## What to test per crate

| Crate | Focus |
|---|---|
| `ff-format` / `ff-common` | type conversions, token mapping (string-equality), pure logic — no FFmpeg |
| `ff-probe` | metadata extraction against fixtures |
| `ff-decode` / `ff-encode` | decode/encode round-trips; format conversion; seek accuracy |
| `ff-filter` | filter-graph build + push/pull (probe-gated); token/arg strings (string-equality units) |
| `ff-pipeline` / `ff-stream` / `ff-preview` | end-to-end pipeline against fixtures (probe-gated) |
| `ff-render` | GPU composite output (skip when no adapter) |
| `avio` (engine) | editing-model behaviour; the `avio-examples` harness for end-to-end verification |

## What NOT to test

- Third-party internals (FFmpeg, wgpu).
- Exact pixel equality across FFmpeg builds / GPU drivers (use tolerances, or skip).
- Trivial getters/setters.

---

## CI notes

- `cargo test --workspace` is the gate. Integration tests self-skip where FFmpeg is unavailable, so
  it is expected to always pass.
- CI clippy runs **without** `--tests` / `--all-targets`, so test-only lints (`expect_used`,
  `print_stdout`) do not gate CI. Non-test library code must stay clippy-clean; do not chase local
  `--tests` clippy errors that CI never runs.
