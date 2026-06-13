# v0.17.0 — Professional Interchange & GPU Acceleration

**Goal**: Enable round-trip interoperability with industry-standard NLE tools (Final Cut Pro, Premiere Pro, DaVinci Resolve, Avid) via EDL and FCPXML, add OMF/AAF audio project support, and provide GPU-accelerated filter processing for real-time capable throughput.

**Prerequisite**: v0.16.0 complete.

**Crates in scope**: `ff-interchange` (new crate), `ff-filter`, `ff-decode`, `ff-encode`

---

## Requirements

### EDL (Edit Decision List) Support

- An EDL file (CMX 3600 format) can be parsed into a Rust data structure representing the cut list: clip source, in/out points, record in/out points, and transition type.
- An EDL can be exported from a clip list assembled in the library.
- EDL round-trip: import → reconstruct timeline → export produces a semantically equivalent EDL.
- Supported EDL event types: cut, dissolve, wipe (basic).
- Reel names are preserved through import/export.
- Common use case: conforming an offline edit (proxy) to online (full-res) material.

### Final Cut Pro XML (FCPXML) Support

- An FCPXML file (version 1.9 and 1.10) can be parsed into a Rust data structure representing the project: sequences, clips, asset references, basic effects, and markers.
- An FCPXML can be exported from a project assembled in the library.
- Asset references (file paths) are resolved relative to a configurable media root.
- Supported FCPXML elements: `project`, `sequence`, `clip`, `audio-clip`, `title` (basic), `marker`, `chapter-marker`, `transition`.
- Common use case: exporting a cut list from a Rust application for finishing in Final Cut Pro.

### Premiere Pro / DaVinci Resolve XML

- A Premiere Pro-compatible XML (based on the Final Cut Pro 7 XML schema) can be exported, enabling import into Premiere Pro and DaVinci Resolve.
- Supported elements: sequences, video/audio tracks, clips, in/out points, basic transitions.

### OMF / AAF (Audio Post-Production)

- An OMF (Open Media Framework) or AAF (Advanced Authoring Format) file can be exported containing the audio tracks from a project, suitable for delivery to a Pro Tools or Nuendo audio post session.
- Clip metadata (reel name, timecode, sample rate) is preserved.
- Audio media can be embedded in the OMF or referenced externally.
- Common use case: sending a picture-locked timeline's audio to a mixer for final audio post.

### GPU-Accelerated Filter Processing

- The following commonly used filters can be executed on the GPU when a compatible device is available, falling back to CPU when not:
  - Scale / resize
  - Color correction (brightness, contrast, saturation, curves)
  - 3D LUT application
  - Overlay / composite
  - Blur (Gaussian)
- GPU acceleration is available via:
  - **CUDA** (NVIDIA): requires FFmpeg built with `--enable-cuda-llvm`
  - **VideoToolbox** (Apple Silicon / macOS): hardware-native
  - **VAAPI** (Linux / Intel / AMD): requires `--enable-vaapi`
- GPU acceleration is opt-in: enabled by passing `GpuAccel::Cuda` / `GpuAccel::VideoToolbox` / `GpuAccel::Vaapi` to the filter graph builder.
- When the selected GPU device is unavailable, the library transparently falls back to the CPU path and logs `log::warn!("gpu_accel unavailable, falling back to cpu device={:?}")`.
- GPU texture output: a `GpuFrameSink` variant allows delivering decoded + filtered frames as GPU-resident textures (CUDA device memory or Metal textures) to avoid a GPU→CPU→GPU round-trip in render pipelines.

### Hardware-Accelerated Encode with GPU Filters

- A pipeline of GPU-decoded → GPU-filtered → GPU-encoded is supported end-to-end without any CPU round-trip for the pixel data, using:
  - NVIDIA: `h264_nvenc` / `hevc_nvenc` / `av1_nvenc` after CUDA filter graph
  - Apple: `h264_videotoolbox` / `hevc_videotoolbox` after VideoToolbox decode
- This enables real-time 4K transcoding with filter application on supported hardware.

---

## Design Decisions

| Topic | Decision |
|---|---|
| New crate | `ff-interchange`: pure-Rust parser/writer for EDL, FCPXML, Premiere XML, OMF/AAF; no FFmpeg dependency |
| EDL format | CMX 3600 (industry standard); additional formats added if demand arises |
| FCPXML version | 1.9 and 1.10 (current as of 2025); older versions handled on best-effort basis |
| OMF/AAF | Export-only at this stage; import is complex and deferred |
| GPU filter backend | libavfilter CUDA/VAAPI graph; VideoToolbox via `vf_scale_vt` |
| GPU fallback | Silent CPU fallback with `log::warn!`; no error returned for missing GPU |
| GPU texture sink | Optional; behind `gpu-sink` feature flag |

---

## Definition of Done

- EDL round-trip test: CMX 3600 file imported, timeline reconstructed, re-exported, and diff'd against original (ignoring whitespace)
- FCPXML export test: exported file opens in Final Cut Pro without errors
- Premiere XML import tested in DaVinci Resolve
- GPU scale filter (CUDA or VAAPI) produces pixel-identical output to CPU path on a test frame
- GPU end-to-end pipeline (decode → filter → encode, no CPU copy) completes a 1080p transcode in CI
