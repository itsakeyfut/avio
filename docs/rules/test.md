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

### 2. Integration tests (real FFmpeg, and anything about the public surface)

`crates/<crate>/tests/`. Drive the real FFmpeg API against short fixtures.

**A test that asserts an item is public belongs here even though it needs no FFmpeg.** A unit
test resolves names through `use super::*`, which sees crate-internal items, so a `pub use`
demoted to `pub(crate) use` still compiles in one; only an external crate path enforces `pub`.
This bites in `avio`, where CLAUDE.md makes re-exporting every new public type into
`avio/src/lib.rs` a rule, so that list is edited constantly, and CI cannot catch a regression in
it any other way (the feature powerset job runs `--no-dev-deps`, which never compiles a test).
`crates/avio/tests/public_surface_tests.rs` is the anchor; `lib.rs`'s own module checks that the
names resolve, which is a different property, so neither replaces the other.

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

**Two exceptions: `parse_desc`, and the composite operators.** A `FilterStep::ParseDesc`
description is parsed at `build()` (`avfilter_graph_parse2`), which resolves filter names *and
applies their options*, so a bad option name or value returns `InvalidConfig` there rather than on
the first push (ADR-0012). Asserting on `build()` is therefore correct for that step kind, but gate
it, because the check is skipped when the filter registry is empty, which is exactly CI's Linux
FFmpeg. The second exception is the opposite shape: `In`/`Out`/`Atop`/`Xor` on a
`FilterStep::Composite`, or on any `MultiTrackComposer` / `RealtimeComposer` layer, are refused at
`build()` with `UnsupportedCompositeOp` (ADR-0014). That check is pure, no registry lookup, so
asserting it needs **no** probe gate and reports identically on every build.

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
- **Drive `SceneRunner` unpaced.** An e2e test that calls `SceneRunner::run` sets
  `runner.set_pacing(Pacing::Unpaced)` first, unless real-time pacing is what it tests
  (`av_sync_test`, and the ignored control-timing tests in `timeline_preview_tests`). The
  real-time loop drops any frame more than a period late, so a lower bound on delivered frames or
  on the frames inside a transition window is flaky by construction on a loaded runner; unpaced,
  every decoded frame is delivered and the bound can be exact (ADR-0015). After `run`, drain
  `handle.poll_event()` and skip on a `PlayerEvent::Error`: a decoder that fails mid-stream ends
  the run as if at EOF, which the macOS runner's automatic hardware decoder does intermittently
  (#1789), and that is the environment, not the runner.

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
- **Nothing here runs tests at any feature configuration other than all-features.** The two jobs
  that execute tests both pass `--all-features` (`ci.yml`'s `cargo test --all --all-features` and
  `coverage.yml`'s `cargo llvm-cov`), and `.claude/scripts/test.sh` adds `--all-features` unless a
  feature flag is given. The jobs that do use other configurations never run a test: `cargo check`
  / `cargo build` only compile, and the feature powerset job passes `--no-dev-deps`, which does not
  compile tests at all.

  So **a property that only holds when a feature is off must be asserted as a feature-independent
  invariant, not inside a `#[cfg(not(feature = "..."))]` body**, because nothing executes such a
  body. Where the feature is not in `default` (`ff-encode`'s `gpl`, for one), the configuration that
  never runs is the one published to crates.io. Prefer asserting the data a decision reads (for example, "the
  H.264 candidate list never contains an encoder from another family") over asserting the decision's
  outcome, which depends on what the local FFmpeg registers. #1835 shipped in v0.18.0 this way: the
  off-side test existed, had never once run, and asserted a property the wrong encoder also
  satisfied.
