# avio

[![Crates.io](https://img.shields.io/crates/v/avio.svg)](https://crates.io/crates/avio)
[![Docs.rs](https://docs.rs/avio/badge.svg)](https://docs.rs/avio)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A safe, high-level Rust API over FFmpeg for building media applications: decode, encode, filter, compose, and stream.

`avio` is the **editing engine** of the `ff-*` crate family. It owns the editing model (`Timeline` / `Clip`, the model-to-frame derivation, and edit history) and re-exports the model-agnostic `ff-*` primitives behind feature flags, so you can depend on a single crate and opt into the capabilities you need. If your app needs a different editing model, depend on the `ff-*` primitives directly instead; each is usable on its own. See the [main repository](https://github.com/itsakeyfut/avio) for full context.

## Installation

```toml
[dependencies]
# Default: probe + decode + encode + hwaccel
avio = "0.16"

# Add filtering
avio = { version = "0.16", features = ["filter"] }

# Full streaming stack (implies pipeline, which implies filter)
avio = { version = "0.16", features = ["stream"] }

# Async decode/encode (requires tokio runtime)
avio = { version = "0.16", features = ["tokio"] }
```

The crates in this family version independently (each on its own cadence, like `tokio` or `http`); see crates.io for the current version of each.

## Feature Flags

| Feature         | Enables                                        | Default | Implies             |
|-----------------|------------------------------------------------|---------|---------------------|
| `probe`         | `ff-probe`, read-only media metadata           | yes     |                     |
| `decode`        | `ff-decode`, video and audio decoding          | yes     |                     |
| `encode`        | `ff-encode`, video and audio encoding          | yes     |                     |
| `hwaccel`       | hardware-accelerated encoders                  | yes     |                     |
| `filter`        | `ff-filter`, filter graph operations           | no      |                     |
| `pipeline`      | `ff-pipeline`, decode/filter/encode            | no      | `filter`            |
| `stream`        | `ff-stream`, HLS / DASH / live streaming       | no      | `pipeline`          |
| `preview`       | `ff-preview`, real-time playback and seek      | no      |                     |
| `preview-proxy` | proxy generation                               | no      | `preview`           |
| `render`        | `ff-render`, GPU compositing pipeline          | no      | `preview`           |
| `render-gpu`    | wgpu backend                                   | no      | `render`            |
| `tokio`         | async wrappers for decode, encode, and preview | no      | `decode` + `encode` |
| `gpl`           | GPL-licensed encoders                          | no      |                     |
| `srt`           | SRT input/output                               | no      |                     |
| `serde`         | `serde` derives for filter types               | no      |                     |

## Quick Start

### Probe

```rust
use avio::open;

let info = open("video.mp4")?;
if let Some(video) = info.primary_video() {
    println!("{}x{} @ {:.2} fps ({:?})", video.width(), video.height(), video.fps(), video.codec());
}
```

### Decode

```rust
use avio::{VideoDecoder, AudioDecoder, PixelFormat, SampleFormat};

// Video: request RGB24 output (FFmpeg converts internally)
let mut vdec = VideoDecoder::open("video.mp4")
    .output_format(PixelFormat::Rgb24)
    .build()?;
while let Some(frame) = vdec.decode_one()? {
    // frame.data() contains raw pixel bytes
}

// Audio: resample to f32 at 44.1 kHz
let mut adec = AudioDecoder::open("audio.flac")
    .output_format(SampleFormat::F32)
    .output_sample_rate(44_100)
    .build()?;
while let Some(frame) = adec.decode_one()? {
    // frame.to_f32_interleaved() returns interleaved f32 samples
}
```

### Image Sequences

A `%`-pattern path (e.g. `frame%04d.png`) automatically selects the `image2`
demuxer for decode and muxer for encode:

```rust
use avio::{VideoDecoder, VideoEncoder, VideoCodec, HardwareAccel};

// Decode numbered PNGs → video
let mut decoder = VideoDecoder::open("frames/frame%04d.png")
    .hardware_accel(HardwareAccel::None)
    .frame_rate(25)
    .build()?;

// Encode video → numbered PNGs
let mut encoder = VideoEncoder::create("out/frame%04d.png")
    .video(1920, 1080, 25.0)
    .video_codec(VideoCodec::Png)
    .build()?;
```

### Encode

```rust
use avio::{VideoEncoder, VideoCodec, AudioCodec, BitrateMode};

let mut encoder = VideoEncoder::create("output.mp4")
    .video(1920, 1080, 30.0)
    .video_codec(VideoCodec::H264)
    .bitrate_mode(BitrateMode::Crf(23))
    .audio(48_000, 2)
    .audio_codec(AudioCodec::Aac)
    .build()?;

for frame in &video_frames {
    encoder.push_video(frame)?;
}
encoder.finish()?;
```

### Per-Codec Options

`VideoCodecOptions` and `AudioCodecOptions` provide typed, per-codec
configuration applied before the codec is opened:

```rust
use avio::{
    VideoEncoder, VideoCodec, VideoCodecOptions,
    H264Options, H264Profile, H264Preset,
    BitrateMode,
};

let opts = VideoCodecOptions::H264(H264Options {
    profile: H264Profile::High,
    level: Some(41),
    bframes: 2,
    gop_size: 250,
    refs: 3,
    preset: Some(H264Preset::Fast),
    tune: None,
});

let mut encoder = VideoEncoder::create("output.mp4")
    .video(1920, 1080, 30.0)
    .video_codec(VideoCodec::H264)
    .bitrate_mode(BitrateMode::Crf(23))
    .codec_options(opts)
    .build()?;
```

Available option structs: `H264Options`, `H265Options`, `Av1Options`,
`SvtAv1Options`, `Vp9Options`, `ProResOptions`, `DnxhdOptions`,
`OpusOptions`, `AacOptions`, `Mp3Options`, `FlacOptions`.

### Professional Formats

```rust
use avio::{
    VideoEncoder, VideoCodec, VideoCodecOptions,
    ProResOptions, ProResProfile, PixelFormat,
};

let mut encoder = VideoEncoder::create("output.mov")
    .video(1920, 1080, 25.0)
    .video_codec(VideoCodec::ProRes)
    .pixel_format(PixelFormat::Yuv422p10le)
    .codec_options(VideoCodecOptions::ProRes(ProResOptions {
        profile: ProResProfile::Hq,
        vendor: None,
    }))
    .build()?;
```

### HDR Metadata

```rust
use avio::{
    VideoEncoder, VideoCodec, VideoCodecOptions,
    H265Options, H265Profile, PixelFormat, BitrateMode,
    Hdr10Metadata, MasteringDisplay, ColorTransfer,
};

// HDR10: PQ transfer + MaxCLL/MaxFALL side data
let mut encoder = VideoEncoder::create("output.mkv")
    .video(3840, 2160, 24.0)
    .video_codec(VideoCodec::H265)
    .bitrate_mode(BitrateMode::Crf(22))
    .pixel_format(PixelFormat::Yuv420p10le)
    .codec_options(VideoCodecOptions::H265(H265Options {
        profile: H265Profile::Main10,
        ..H265Options::default()
    }))
    .hdr10_metadata(Hdr10Metadata {
        max_cll: 1000, max_fall: 400,
        mastering_display: MasteringDisplay {
            red_x: 17000, red_y: 8500,
            green_x: 13250, green_y: 34500,
            blue_x: 7500, blue_y: 3000,
            white_x: 15635, white_y: 16450,
            min_luminance: 50,
            max_luminance: 10_000_000,
        },
    })
    .build()?;

// HLG: broadcast HDR without MaxCLL/MaxFALL
use avio::{ColorSpace, ColorPrimaries};
let mut encoder = VideoEncoder::create("output.mkv")
    .video(1920, 1080, 50.0)
    .video_codec(VideoCodec::H265)
    .pixel_format(PixelFormat::Yuv420p10le)
    .color_transfer(ColorTransfer::Hlg)
    .color_space(ColorSpace::Bt2020Ncl)
    .color_primaries(ColorPrimaries::Bt2020)
    .build()?;
```

### Async (tokio feature)

```rust
use avio::{AsyncVideoDecoder, AsyncVideoEncoder, VideoEncoder, VideoCodec};
use futures::StreamExt;

let mut encoder = AsyncVideoEncoder::from_builder(
    VideoEncoder::create("output.mp4")
        .video(1920, 1080, 30.0)
        .video_codec(VideoCodec::H264),
)?;

let stream = AsyncVideoDecoder::open("input.mp4").await?.into_stream();
tokio::pin!(stream);
while let Some(Ok(frame)) = stream.next().await {
    encoder.push(frame).await?;
}
encoder.finish().await?;
```

### Pipeline (pipeline feature)

```rust
use avio::{Pipeline, EncoderConfig, VideoCodec, AudioCodec, BitrateMode};

Pipeline::builder()
    .input("input.mp4")
    .output("output.mp4", EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .audio_codec(AudioCodec::Aac)
        .bitrate_mode(BitrateMode::Crf(23))
        .build())
    .build()?
    .run()?;
```

## Crate Family

| Crate         | Purpose                                        |
|---------------|------------------------------------------------|
| `ff-sys`      | Raw bindgen FFI (internal use only)            |
| `ff-common`   | Shared buffer-pooling abstractions             |
| `ff-format`   | Pure-Rust type definitions (no FFmpeg linkage) |
| `ff-probe`    | Read-only media metadata extraction            |
| `ff-decode`   | Video and audio decoding                       |
| `ff-encode`   | Video and audio encoding                       |
| `ff-filter`   | Filter graph operations                        |
| `ff-pipeline` | Decode, filter, encode pipeline                |
| `ff-stream`   | HLS / DASH adaptive streaming output           |
| `ff-preview`  | Real-time preview and proxy workflow           |
| `ff-render`   | GPU compositing pipeline (wgpu)                |
| `avio`        | Editing engine + facade (this crate)           |

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
