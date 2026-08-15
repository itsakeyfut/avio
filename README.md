# avio

A safe, high-level Rust API over FFmpeg for building media applications: decode, encode, filter, compose, and stream.

[![Crates.io](https://img.shields.io/crates/v/avio.svg)](https://crates.io/crates/avio)
[![Docs.rs](https://docs.rs/avio/badge.svg)](https://docs.rs/avio)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

## Overview

`avio` is a family of Rust crates over FFmpeg, from decode and encode up to timeline composition, real-time preview, and GPU rendering. The public API is safe: every unsafe FFmpeg call is encapsulated, so application code never needs `unsafe`.

The goal is to be a foundation for video delivery services and video editing applications written in Rust. It does not try to cover every FFmpeg feature.

```rust
use ff_probe::open;
use ff_decode::VideoDecoder;
use ff_encode::{VideoEncoder, VideoCodec, AudioCodec, BitrateMode};

// Inspect a media file
let info = open("input.mp4")?;
if let Some(v) = info.primary_video() {
    println!("{}x{} @ {:.2} fps", v.width(), v.height(), v.fps());
}

// Decode frames
let mut decoder = VideoDecoder::open("input.mp4").build()?;
while let Some(frame) = decoder.decode_one()? {
    // process frame.planes() ...
}

// Re-encode
let mut encoder = VideoEncoder::create("output.mp4")
    .video(1920, 1080, 30.0)
    .video_codec(VideoCodec::H264)
    .bitrate_mode(BitrateMode::Crf(23))
    .audio(48000, 2)
    .audio_codec(AudioCodec::Aac)
    .build()?;
encoder.finish()?;
```

## Installation

Add the facade crate, or just the member crates you need:

```toml
[dependencies]
avio = "0.15"

# Or pick individual crates
ff-probe  = "0.15"
ff-decode = "0.15"
ff-encode = "0.15"
```

FFmpeg 7.x or 8.x development libraries must be installed on your system.

### Windows

```powershell
vcpkg install ffmpeg:x64-windows
$env:VCPKG_ROOT = "C:\vcpkg"
```

### macOS

```bash
brew install ffmpeg
```

### Linux (Debian/Ubuntu)

```bash
sudo apt install libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev
```

## Usage

### Decode

```rust
use ff_decode::{VideoDecoder, AudioDecoder, SeekMode};
use ff_format::{PixelFormat, SampleFormat};
use std::time::Duration;

// Video
let mut decoder = VideoDecoder::open("video.mp4")
    .output_format(PixelFormat::Rgba)
    .build()?;

while let Some(frame) = decoder.decode_one()? {
    // frame.planes() contains pixel data
}

// Seek and decode a single frame
decoder.seek(Duration::from_secs(30), SeekMode::Exact)?;
let frame = decoder.decode_one()?;

// Audio
let mut decoder = AudioDecoder::open("audio.mp3")
    .output_format(SampleFormat::F32)
    .output_sample_rate(48000)
    .build()?;

while let Some(frame) = decoder.decode_one()? {
    // frame.planes() contains audio samples
}
```

### Encode

```rust
use ff_encode::{VideoEncoder, VideoCodec, AudioCodec, BitrateMode, Preset};

// Automatically selects an LGPL-compatible encoder (hardware or VP9/AV1 fallback)
let mut encoder = VideoEncoder::create("output.mp4")
    .video(1920, 1080, 30.0)
    .video_codec(VideoCodec::H264)
    .bitrate_mode(BitrateMode::Crf(23))
    .preset(Preset::Fast)
    .audio(48000, 2)
    .audio_codec(AudioCodec::Aac)
    .build()?;

for frame in video_frames {
    encoder.push_video(&frame)?;
}
encoder.finish()?;
```

### Hardware acceleration

```rust
use ff_decode::{VideoDecoder, HardwareAccel};
use ff_encode::{VideoEncoder, HardwareEncoder};

// Decode with GPU
let decoder = VideoDecoder::open("video.mp4")
    .hardware_accel(HardwareAccel::Auto)
    .build()?;

// Encode with GPU
let encoder = VideoEncoder::create("output.mp4")
    .video(1920, 1080, 60.0)
    .hardware_encoder(HardwareEncoder::Auto)
    .build()?;
```

See [docs.rs/avio](https://docs.rs/avio) for the full API.

## Crates

| Crate | Description | crates.io | docs.rs |
|-------|-------------|-----------|---------|
| [`ff-probe`](./crates/ff-probe) | Media metadata extraction | [![](https://img.shields.io/crates/v/ff-probe.svg)](https://crates.io/crates/ff-probe) | [![](https://docs.rs/ff-probe/badge.svg)](https://docs.rs/ff-probe) |
| [`ff-decode`](./crates/ff-decode) | Video and audio decoding | [![](https://img.shields.io/crates/v/ff-decode.svg)](https://crates.io/crates/ff-decode) | [![](https://docs.rs/ff-decode/badge.svg)](https://docs.rs/ff-decode) |
| [`ff-encode`](./crates/ff-encode) | Video and audio encoding | [![](https://img.shields.io/crates/v/ff-encode.svg)](https://crates.io/crates/ff-encode) | [![](https://docs.rs/ff-encode/badge.svg)](https://docs.rs/ff-encode) |
| [`ff-filter`](./crates/ff-filter) | Filter graph operations | [![](https://img.shields.io/crates/v/ff-filter.svg)](https://crates.io/crates/ff-filter) | [![](https://docs.rs/ff-filter/badge.svg)](https://docs.rs/ff-filter) |
| [`ff-pipeline`](./crates/ff-pipeline) | Decode, filter, encode pipeline | [![](https://img.shields.io/crates/v/ff-pipeline.svg)](https://crates.io/crates/ff-pipeline) | [![](https://docs.rs/ff-pipeline/badge.svg)](https://docs.rs/ff-pipeline) |
| [`ff-stream`](./crates/ff-stream) | HLS/DASH streaming output | [![](https://img.shields.io/crates/v/ff-stream.svg)](https://crates.io/crates/ff-stream) | [![](https://docs.rs/ff-stream/badge.svg)](https://docs.rs/ff-stream) |
| [`ff-preview`](./crates/ff-preview) | Real-time A/V preview and proxy workflow | [![](https://img.shields.io/crates/v/ff-preview.svg)](https://crates.io/crates/ff-preview) | [![](https://docs.rs/ff-preview/badge.svg)](https://docs.rs/ff-preview) |
| [`ff-render`](./crates/ff-render) | GPU compositing pipeline (wgpu) | [![](https://img.shields.io/crates/v/ff-render.svg)](https://crates.io/crates/ff-render) | [![](https://docs.rs/ff-render/badge.svg)](https://docs.rs/ff-render) |
| [`ff-format`](./crates/ff-format) | Shared type definitions | [![](https://img.shields.io/crates/v/ff-format.svg)](https://crates.io/crates/ff-format) | [![](https://docs.rs/ff-format/badge.svg)](https://docs.rs/ff-format) |
| [`ff-common`](./crates/ff-common) | Common traits and buffer pooling | [![](https://img.shields.io/crates/v/ff-common.svg)](https://crates.io/crates/ff-common) | [![](https://docs.rs/ff-common/badge.svg)](https://docs.rs/ff-common) |
| [`ff-sys`](./crates/ff-sys) | Low-level FFmpeg FFI bindings | [![](https://img.shields.io/crates/v/ff-sys.svg)](https://crates.io/crates/ff-sys) | [![](https://docs.rs/ff-sys/badge.svg)](https://docs.rs/ff-sys) |
| [`avio`](./crates/avio) | Facade crate that re-exports all member crates | [![](https://img.shields.io/crates/v/avio.svg)](https://crates.io/crates/avio) | [![](https://docs.rs/avio/badge.svg)](https://docs.rs/avio) |

## Feature flags

The `avio` facade re-exports the member crates behind cargo features:

| Feature | Default | Enables |
|---------|:------:|---------|
| `probe` | yes | Metadata extraction |
| `decode` | yes | Video and audio decoding |
| `encode` | yes | Video and audio encoding |
| `hwaccel` | yes | Hardware encoders (NVENC, QSV, AMF, VideoToolbox, VA-API) |
| `filter` | | libavfilter graph operations |
| `pipeline` | | Decode, filter, encode pipeline |
| `stream` | | HLS/DASH output |
| `preview` | | Real-time preview |
| `preview-proxy` | | Proxy generation |
| `render` | | CPU compositing |
| `render-gpu` | | GPU compositing via wgpu |
| `tokio` | | Async decode/encode API |
| `gpl` | | GPL codecs (libx264, libx265) |
| `srt` | | SRT protocol input and output |
| `serde` | | serde derives for filter types |

## Platform support

| Platform | Status | Hardware acceleration |
|----------|--------|-----------------------|
| Windows | ✅ | NVENC/NVDEC, QSV, AMF |
| macOS | ✅ | VideoToolbox |
| Linux | ✅ | VAAPI, NVENC/NVDEC, QSV |

## Projects using avio

### [ascii-term](https://github.com/itsakeyfut/ascii-term)

A terminal media player that renders video as colored ASCII art with synchronized audio. It was migrated from `ffmpeg-next` / `ffmpeg-sys-next` to `avio`, with no direct `unsafe` FFmpeg code in the application. It uses:

- `VideoDecoder` with `PixelFormat::Rgb24` for per-pixel luminance mapping
- `AudioDecoder` with PCM conversion (`SampleFormat::F32`) feeding [rodio](https://crates.io/crates/rodio)
- Synchronized audio and video across two threads via `crossbeam-channel`

### [avio-editor-demo](https://github.com/itsakeyfut/avio-editor-demo)

A non-linear video editor and the main driver of the library's API. It exercises the full decode, timeline compose, preview, and export path, and is where most bugs and API changes originate. It uses:

- `Timeline` / `Clip` multi-track composition with per-clip colour correction and transitions
- A real-time preview that matches the exported result
- The `ff-preview` proxy workflow, plus scene/silence detection, waveform, and EBU R128 loudness analysis

## Contributing

Pull requests, bug reports, and feature requests are welcome. See [CONTRIBUTING](.github/CONTRIBUTING.md), and look for issues labeled [`good first issue`](https://github.com/itsakeyfut/avio/issues?q=is%3Aopen+label%3A%22good+first+issue%22) or [`help wanted`](https://github.com/itsakeyfut/avio/issues?q=is%3Aopen+label%3A%22help+wanted%22).

`avio-editor-demo` drives most API changes, so it is a good place to see what is needed next.

## Minimum Supported Rust Version

Rust 1.93.0 (edition 2024).

## License

Dual-licensed under either [MIT](./LICENSE-MIT) or [Apache-2.0](./LICENSE-APACHE) at your option.

`avio` links against FFmpeg, which is [LGPL 2.1+](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html) by default. The `gpl` feature of `ff-encode` enables GPL-licensed codecs (libx264, libx265); see [`ff-encode`](./crates/ff-encode/README.md).

## Acknowledgements

The audio fixture used in integration tests is provided by [Music Atelier Amacha](https://amachamusic.chagasi.com/) (甘茶の音楽工房), composed by Amacha. Used with permission under the site's free-use terms.
