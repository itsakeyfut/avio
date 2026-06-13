# v0.18.0 — API Finalization

**Goal**: Freeze the public API surface across all crates, eliminate all remaining design debt, complete documentation and examples, and establish performance baselines — making the library ready for v1.0.0 declaration once real-world adoption has been validated.

**Prerequisite**: v0.17.0 complete.

**Crates in scope**: All crates

---

## Requirements

### API Freeze Preparation

- All public enums and error types that may gain new variants in future versions carry `#[non_exhaustive]`.
  - Applied to: `VideoCodec`, `AudioCodec`, `PixelFormat`, `SampleFormat`, `ChannelLayout`, `ColorSpace`, `HwAccel`, `GpuAccel`, `EncodeError`, `DecodeError`, `FilterError`, `StreamError`, `PreviewError`, `InterchangeError`.
  - Excluded from: small, closed enums where exhaustive matching is part of the contract (e.g., `BitrateMode`, `H264Profile`, `BlendMode`).
- Every public API that was superseded or renamed during v0.x carries `#[deprecated]` with a `/// Use [replacement] instead.` doc comment.
- The MSRV is pinned to Rust 1.85.0 (minimum for edition 2024) and recorded in every crate's `[package] rust-version` field.
- `Cargo.toml` `[package] edition = "2024"` is confirmed for all crates.
- A breaking-change policy is documented in `CONTRIBUTING.md`: semver minor for additive changes, semver major for removals or signature changes, `#[deprecated]` with one minor-version grace period before removal.

### Documentation

- `#![warn(missing_docs)]` is enabled on all crates and produces zero warnings.
- Every `pub` function, type, field, and variant has a doc comment.
- Every primary API entry point has a `# Examples` section with a working, tested code snippet (`cargo test --doc`).
- All `# Safety` sections on `unsafe fn` items and `unsafe impl` blocks are present and accurate.
- `cargo doc --workspace --no-deps` produces zero warnings.
- `docs.rs` renders all public items with no missing sections.

### Cookbook Examples

The following end-to-end examples are provided under the workspace `examples/` directory, each covering a distinct real-world use case:

- `examples/transcode.rs` — re-encode a file to a different codec and container
- `examples/thumbnails.rs` — extract thumbnails at regular intervals
- `examples/sprite_sheet.rs` — generate a thumbnail sprite sheet for a video player scrub bar
- `examples/filter_trim_scale.rs` — trim a clip to a time range and resize it
- `examples/filter_overlay.rs` — composite a logo PNG over a video with opacity
- `examples/two_pass_encode.rs` — two-pass H.264 encode for accurate bitrate targeting
- `examples/metadata_chapters.rs` — read and write container metadata and chapter marks
- `examples/subtitle_passthrough.rs` — copy a subtitle stream into a new output file
- `examples/hls_output.rs` — package a video file as a static HLS stream
- `examples/abr_ladder.rs` — produce a multi-rendition HLS ABR ladder
- `examples/live_stream.rs` — ingest from RTSP and push to an RTMP server
- `examples/color_grade.rs` — apply a 3D LUT and primary color correction
- `examples/concat_clips.rs` — join multiple clips with a cross-dissolve transition
- `examples/green_screen.rs` — chroma key composite over a background
- `examples/keyframe_fade.rs` — animate opacity and position with Bezier easing
- `examples/realtime_preview.rs` — drive a real-time preview loop using `ff-preview`
- `examples/proxy_workflow.rs` — generate proxies, edit with them, export with originals
- `examples/stabilize.rs` — two-pass video stabilization
- `examples/mix_audio.rs` — mix a voice-over with background music, with ducking
- `examples/export_fcpxml.rs` — export a clip list as FCPXML for Final Cut Pro
- `examples/full_pipeline.rs` — decode → color grade → composite → encode end-to-end

### Performance Benchmarks

- `benches/decode_bench.rs` — 1080p decode throughput (fps) for H.264, H.265, AV1, ProRes.
- `benches/encode_bench.rs` — 1080p encode throughput (fps) and file size for H.264, H.265, AV1.
- `benches/filter_bench.rs` — processing time per frame for: scale, crop, overlay, 3D LUT, chroma key, blur.
- `benches/preview_bench.rs` — `ff-preview` playback loop: frames delivered on time at 1080p/30.
- `benches/animation_bench.rs` — keyframe interpolation throughput (evaluations/sec for a 100-keyframe track).
- A baseline result table is committed to `benches/README.md` and updated with each release.

### API Consistency Audit

- The builder pattern is confirmed uniform across all crates: consuming builders (`self`), all validation in `build()`.
- Error type hierarchy is audited against `docs/dev/error-handling.md`: no `anyhow` in library code, all FFmpeg errors wrapped with context.
- Every `*Inner` type that may cross thread boundaries carries an explicit `unsafe impl Send` (and `unsafe impl Sync` where appropriate) with a `// SAFETY:` comment.
- No `unwrap()` or `expect()` remains in library source (`src/`); only in tests and examples.
- `cargo clippy --workspace -- -D warnings` is clean on stable Rust.
- `cargo fmt --check` passes.

---

## Design Decisions

| Topic | Decision |
|---|---|
| `#[non_exhaustive]` scope | Error types and major enums; excluded from small closed enums where exhaustive matching is useful |
| MSRV | Rust 1.85.0 — minimum for edition 2024 |
| Benchmarks | criterion (already a dev-dep); results committed to repo |
| Examples | Workspace root `examples/`; all examples compile in CI |
| Doc tests | `cargo test --doc --workspace` runs all `# Examples` blocks in CI |

---

## Definition of Done

- All requirements above fulfilled
- `cargo doc --workspace --no-deps` emits zero warnings
- All 21 cookbook examples compile and run successfully against sample media files in CI
- `cargo test --doc --workspace` passes
- Benchmark baseline recorded in `benches/README.md`
- MSRV verified: `cargo +1.85.0 build --workspace` succeeds
- `cargo clippy --workspace -- -D warnings` clean
- `cargo fmt --check` clean
