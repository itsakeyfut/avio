# ff-probe

Read media file metadata with one function call. `open` returns a structured `MediaInfo` with typed accessors for resolution, frame rate, sample rate, duration, and codec identifiers.

`ff-probe` safely wraps FFmpeg's container inspection (libavformat demuxing and stream discovery) as a read-only metadata reader, with no decoding or encoding. Errors are typed and carry path and message context (`ProbeError`), so a failure reads as an actionable message rather than a raw FFmpeg return code.

It is an independent crate: use it on its own, or combine it with the other `ff-*` crates to assemble whatever media application, or editing model, you need. The `ff-*` crates are purified, model-free primitives, so none imposes an editing model on you; [`avio`](https://github.com/itsakeyfut/avio) is one editing engine built on top of them. Each crate is versioned independently; see crates.io for current versions.

## Installation

```toml
[dependencies]
ff-probe = "0.16"
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

## Returned Fields

`MediaInfo` exposes typed fields, no string parsing required:

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
