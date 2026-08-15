# Long-term Roadmap Plan: v0.19.0 and Beyond

This document records the planned direction for versions after v0.18.0 (API Finalization).
It is a living reference — not a commitment — and exists so that scope decisions made in
early 2026 are not lost between conversations.

The project follows the same philosophy as wgpu and bevy: stability and feature completeness
take priority over a quick v1.0.0. Each minor version has a clear theme, and each version
pairs new features with stabilization of the previous version's additions.

---

## Design Philosophy for v0.19.0+

After v0.18.0 freezes the public API surface:

- All new additions must be strictly **additive** — no breaking changes to existing APIs.
- Each version targets one of three axes:
  1. **Professional video editing** — features that NLEs (non-linear editors) require.
  2. **Video distribution services** — packaging, delivery, compliance, automation.
  3. **Cross-cutting quality** — stability, benchmarks, security, new compile targets.
- New feature groups are gated behind feature flags where they introduce heavy dependencies
  (e.g. `ai-upscale`, `webrtc`, `wasm`).

---

## v0.19.0 — SMPTE Timecode & Advanced Metadata

**Theme**: Precise time management required by professional editing workflows.

### New Features
- SMPTE timecode read/write (LTC, VITC, drop-frame and non-drop-frame)
- Frame-accurate `Clip` in/out points expressed as timecode addresses
- XMP metadata read/write (preserve camera origination metadata)
- Nested chapters and timeline markers (`Marker` type on `Timeline`)

### Stabilization Targets
- Keyframe animation (v0.12.0): establish performance benchmarks for `AnimationTrack`
  evaluation at 60 fps across 1 000+ keyframes
- `Timeline` / `Clip` serde round-trip: compatibility tests ensuring project files
  written by an older patch version load correctly in a newer patch version

---

## v0.20.0 — Advanced Color Science

**Theme**: Log footage and ACES pipelines for cinema/broadcast-grade color work.

### New Features
- Log color space support: Sony S-Log2/3, Canon C-Log3, ARRI LogC, Blackmagic Film
- ACES AP0/AP1 color space input and output
- LUT chaining: multiple `.cube` files applied in sequence on a single clip
- Gamut compression and out-of-gamut warning (clipping pixel detection)
- False Color filter for HDR monitoring

### Stabilization Targets
- 3D LUT application (v0.9.0): precision validation against reference images (PSNR ≥ 60 dB)
- Color metadata round-trip guarantees (color space + transfer + primaries survive
  MKV and MP4 container round-trips as verified by ffprobe)

---

## v0.21.0 — Multi-camera & Synchronization

**Theme**: Integrating footage from multiple cameras and recording devices.

### New Features
- Timecode-based multi-camera sync (`MulticamGroup` type in `ff-pipeline`)
- Audio fingerprint sync for footage without timecode (cross-correlation algorithm)
- `Timeline` multi-cam grouping: multiple angles for a single scene, switchable by cut point
- Automatic cut-point detection from multi-cam groups (scene detection applied per-angle)

### Stabilization Targets
- `Timeline::render()` (v0.10.0): performance baseline — 1080p/30 fps render must
  complete faster than 1× real-time on a single CPU core
- Frame-accurate seek (`SeekMode::Exact`): guarantee frame index matches expected PTS
  within ±1 ms across H.264, H.265, and VP9 sources

---

## v0.22.0 — WebRTC & Ultra-low Latency

**Theme**: Real-time communication protocols for sub-second live streaming.

### New Features
- WebRTC output via WHIP (WebRTC HTTP Ingest Protocol) — behind `webrtc` feature flag
- WebRTC input via WHEP (WebRTC HTTP Egress Protocol) — behind `webrtc` feature flag
- Sub-second latency pipeline mode (minimal buffering, immediate segment push)
- RTP/RTCP transport layer (required for WebRTC)

### Stabilization Targets
- RTMP/SRT output (v0.8.0): reconnect logic stability under network interruption
- `LiveHlsOutput` / `LiveDashOutput`: 24-hour continuous stream stability test
  (memory growth ≤ 1 MB/hour, no segment gaps > 2× segment duration)

---

## v0.23.0 — 360° Video & Spatial Audio

**Theme**: Immersive media formats for VR and 360° playback.

### New Features
- Equirectangular ↔ cubemap projection conversion
- 360° video metadata embedding (YouTube and Meta spatial media metadata)
- First-person perspective crop from 360° footage (rectilinear extraction)
- Ambisonics audio: B-format decode and encode
- Binaural audio rendering (head-related transfer function, HRTF, for headphone output)

### Stabilization Targets
- Porter-Duff compositing (v0.11.0): numerical precision tests (all 7 operations
  verified against reference RGBA values)
- All 18 blend modes (v0.11.0): reference image regression suite added to CI

---

## v0.24.0 — AI-assisted Processing

**Theme**: Optional machine-learning acceleration for common editing tasks.

All AI features are gated behind opt-in feature flags and have a CPU/non-AI fallback
so the crate remains usable without any ML runtime.

### New Features
- AI super-resolution upscaling (`ai-upscale` feature — wgpu or CUDA back-end selectable)
- Scene classification hook: `SceneClassifier` trait for user-provided models
  (returns hints such as `TalkingHead`, `ActionScene`, `MusicPerformance`)
- Auto-subtitle integration point: `AutoSubtitleProvider` trait for connecting
  external speech-to-text models (e.g. Whisper)
- Frame interpolation for slow-motion generation (`frame-interp` feature)

### Stabilization Targets
- CI gate: building with all AI feature flags disabled must not change the public API
  surface (checked via `cargo doc` diff)
- Binary size baseline when all AI features are disabled (documented in `benches/README.md`)

---

## v0.25.0 — Broadcast & Accessibility

**Theme**: Broadcast industry standards and legal compliance requirements.

### New Features
- CEA-608 / CEA-708 closed caption read and write
- DVB / Teletext subtitle support
- EBU R128 / ATSC A/85 loudness compliance report (LUFS, LRA, true peak as output data)
- Broadcast-safe color check: detect and optionally clamp out-of-legal-range pixels
- SMPTE ST 2094 dynamic HDR metadata (HDR10+ and Dolby Vision signalling)

### Stabilization Targets
- EBU R128 loudness normalization (v0.9.0): precision validation (integrated loudness
  within ±0.1 LUFS of reference tool output)
- HDR metadata round-trip (MKV and MP4): ST 2086 mastering display values must survive
  a decode → re-encode cycle unchanged

---

## v0.26.0 — Batch Processing & Automation

**Theme**: High-throughput, unattended processing for production pipelines.

### New Features
- Job queue: submit multiple transcode / filter jobs; configurable worker concurrency
- Watch folder: directory monitor → automatic processing on new file arrival
- Checkpoint-based resume: failed jobs restart from last completed segment
- Batch progress reporting: overall progress + per-job progress callbacks
- Distributed rendering hook: `RenderScheduler` trait for connecting external
  job schedulers (e.g. Kubernetes jobs, render farms)

### Stabilization Targets
- `ff-preview` (v0.13.0): long-run stability test (8-hour playback loop, memory growth
  ≤ 10 MB, no A/V sync drift > 1 frame)
- Async pipeline back-pressure (v0.6.0 `tokio` feature): stress test with 100 concurrent
  encode jobs, verify no deadlocks and bounded memory

---

## v0.27.0 — WebAssembly Target

**Theme**: In-browser video processing without a native runtime.

### New Features
- `wasm32-unknown-unknown` compile target for `ff-format`, `ff-common`, and the
  subtitle parser/writer (`ff-format::subtitle`)
- Pure-Rust fallback codecs (no FFmpeg) for the wasm subset:
  VP8/VP9 decode (via `dav1d` or pure-Rust alternative), AAC decode, GIF encode
- `wasm-bindgen` TypeScript bindings for the wasm-compatible API subset
- Browser-compatible I/O: `File` API / `Blob` as input/output instead of `PathBuf`
- `no_std` compatibility for `ff-format` and `ff-common`

### Stabilization Targets
- `wasm-pack test` added to CI (runs on every PR against the wasm-compatible crates)
- MSRV + wasm target combination guaranteed: the minimum Rust version compiles for wasm

---

## v1.0.0 — Stable API

**Prerequisites (all must be met before tagging 1.0.0):**

1. v0.27.0 is complete and all CI checks pass.
2. At least one production video editing application and one production streaming
   service are using avio in production (documented in README).
3. A public commitment to semantic versioning (no breaking changes without a major bump).
4. A stable C FFI layer so non-Rust consumers can link against avio.
5. The MSRV is pinned and the upgrade policy is documented.
6. All `#[non_exhaustive]` audits from v0.18.0 are confirmed still valid.

---

## Summary Timeline

```
v0.19  SMPTE timecode & XMP metadata          ← professional editing ①
v0.20  Advanced color science (ACES / Log)    ← professional editing ②
v0.21  Multi-camera sync                      ← professional editing ③
v0.22  WebRTC & ultra-low latency             ← distribution services ①
v0.23  360° video & spatial audio             ← new media formats
v0.24  AI-assisted processing (opt-in)        ← emerging technology
v0.25  Broadcast & accessibility              ← distribution services ②
v0.26  Batch processing & automation          ← operational workflows
v0.27  WebAssembly target                     ← new compile targets
v1.0   Stable API
```

Each version pairs new features with explicit stabilization targets from the version
before it. This ensures that every shipped milestone is both forward-looking and
retrospectively verified.
