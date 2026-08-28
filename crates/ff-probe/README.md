# ff-probe

Read media file metadata with one function call. `open` returns a structured `MediaInfo` with typed accessors for resolution, frame rate, sample rate, duration, and codec identifiers.

`ff-probe` safely wraps FFmpeg's container inspection (libavformat demuxing and stream discovery) as a read-only metadata reader, with no decoding or encoding. Errors are typed and carry path and message context (`ProbeError`), so a failure reads as an actionable message rather than a raw FFmpeg return code.

It is an independent crate: use it on its own, or combine it with the other `ff-*` crates to build any media app or editing model. The `ff-*` crates are model-free primitives that impose no editing model; [`avio`](https://github.com/itsakeyfut/avio) is one editing engine built on top of them.

## Installation

```toml
[dependencies]
ff-probe = "0.17"
```

## Quick Start

```rust
use ff_probe::open;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let info = open("video.mp4")?;

    let format = info.format();
    let duration = info.duration();
    println!("format:   {format}");
    println!("duration: {duration:?}");

    if let Some(video) = info.primary_video() {
        let (width, height, fps) = (video.width(), video.height(), video.fps());
        let codec = video.codec();
        println!("video:    {width}x{height} @ {fps:.2} fps");
        println!("codec:    {codec:?}");
    }

    if let Some(audio) = info.primary_audio() {
        let (sample_rate, channels) = (audio.sample_rate(), audio.channels());
        let codec = audio.codec();
        println!("audio:    {sample_rate} Hz, {channels} channels");
        println!("codec:    {codec:?}");
    }

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
| `ProbeError::FileNotFound` | The path does not exist                    |
| `ProbeError::CannotOpen`   | FFmpeg could not open the container        |
| `ProbeError::InvalidMedia` | The file is not a valid media file         |
| `ProbeError::NoStreams`    | The container has no streams               |
| `ProbeError::Io`           | An underlying I/O error (`std::io::Error`) |
| `ProbeError::Ffmpeg`       | An FFmpeg call failed (`code` + `message`) |

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
