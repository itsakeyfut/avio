# ff-probe

Read media file metadata with one function call. No knowledge of container formats or codec identifiers needed — you get back a structured `MediaInfo` with typed accessors for resolution, frame rate, sample rate, duration, and more.

> **Project status (as of 2026-06-04):** The library foundation is in place. Development continues through [**avio-editor-demo**](https://github.com/itsakeyfut/avio-editor-demo), a real-world video editing application built on `avio`, which surfaces bugs and drives API improvements. Pull requests, bug reports, and feature requests are welcome — see the [main repository](https://github.com/itsakeyfut/avio) for full context.

## Installation

```toml
[dependencies]
ff-probe = "0.14"
```

## Quick Start

```rust
use ff_probe::open;

fn main() -> Result<(), ff_probe::ProbeError> {
    let info = open("video.mp4")?;

    if let Some(video) = info.primary_video() {
        println!("resolution: {}x{}", video.width(), video.height());
        println!("frame rate: {}", video.frame_rate());
        println!("codec:      {:?}", video.codec());
    }

    if let Some(audio) = info.primary_audio() {
        println!("sample rate:  {} Hz", audio.sample_rate());
        println!("channels:     {}", audio.channels());
        println!("audio codec:  {:?}", audio.codec());
    }

    println!("duration: {:?}", info.duration());
    Ok(())
}
```

## What You Get Back

`MediaInfo` provides typed fields — no string parsing required:

| Method                   | Type                       | Description                            |
|--------------------------|----------------------------|----------------------------------------|
| `info.duration()`        | `Duration`                 | Total media duration                   |
| `info.primary_video()`   | `Option<&VideoStreamInfo>` | First video stream, if present         |
| `info.primary_audio()`   | `Option<&AudioStreamInfo>` | First audio stream, if present         |
| `video.width()`          | `u32`                      | Frame width in pixels                  |
| `video.height()`         | `u32`                      | Frame height in pixels                 |
| `video.frame_rate()`     | `Rational`                 | Frames per second as an exact fraction |
| `video.codec()`          | `VideoCodec`               | Typed codec enum, not a string         |
| `video.pixel_format()`   | `PixelFormat`              | Pixel format of the encoded stream     |
| `audio.sample_rate()`    | `u32`                      | Samples per second                     |
| `audio.channels()`       | `u32`                      | Channel count                          |
| `audio.codec()`          | `AudioCodec`               | Typed codec enum                       |
| `audio.sample_format()`  | `SampleFormat`             | Sample format of the encoded stream    |

## Error Handling

| Variant                    | When it occurs                             |
|----------------------------|--------------------------------------------|
| `ProbeError::FileNotFound` | The path does not exist or is not readable |
| `ProbeError::CannotOpen`   | FFmpeg could not open the container        |
| `ProbeError::InvalidMedia` | No valid streams found after demux         |

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
