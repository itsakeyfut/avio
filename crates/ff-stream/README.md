# ff-stream

Produce HLS and DASH adaptive bitrate output from any video source. Define a rendition ladder,
point it at an input file, and receive a package ready for CDN delivery.

`ff-stream` is a safe, ergonomic wrapper over FFmpeg's adaptive-streaming muxers (the HLS and DASH segmenters in libavformat), driving the encode-and-mux loop from a rendition ladder. Errors are typed and chain their source (`StreamError`), so a failure reads as an actionable message rather than a raw FFmpeg return code.

It is an independent crate: use it on its own, or combine it with the other `ff-*` crates to build any media app or editing model. The `ff-*` crates are model-free primitives that impose no editing model; [`avio`](https://github.com/itsakeyfut/avio) is one editing engine built on top of them.

## Installation

```toml
[dependencies]
ff-stream = "0.17"
```

## HLS Output

`HlsOutput` is a consuming builder. Setters take `self` and return `Self`; validation is
deferred to `build()`, and `write()` performs the encode-and-mux.

```rust
use ff_stream::HlsOutput;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    HlsOutput::new("hls_output/")
        .input("source.mp4") // any file FFmpeg can demux
        .segment_duration(Duration::from_secs(6))
        .keyframe_interval(48)
        .build()?
        .write()?;
    // Writes hls_output/playlist.m3u8 and numbered segments (segment000.ts, …).
    Ok(())
}
```

## DASH Output

```rust
use ff_stream::DashOutput;
use std::time::Duration;

DashOutput::new("dash_output/")
    .input("source.mp4")
    .segment_duration(Duration::from_secs(4))
    .build()?
    .write()?;
// Writes dash_output/manifest.mpd and the corresponding segments.
```

## Rendition Ladder

`AbrLadder` produces multi-rendition HLS or DASH output from a single input. Each `Rendition`
specifies the output resolution and target bitrate. The ladder owns its own terminal methods:
`hls(output_dir)` and `dash(output_dir)`.

```rust
use ff_stream::{AbrLadder, Rendition};

AbrLadder::new("source.mp4")
    .add_rendition(Rendition { width: 1920, height: 1080, bitrate: 6_000_000 })
    .add_rendition(Rendition { width: 1280, height:  720, bitrate: 3_000_000 })
    .add_rendition(Rendition { width:  854, height:  480, bitrate: 1_500_000 })
    .hls("hls_output/")?;
// Writes hls_output/master.m3u8 plus a numbered sub-directory per rendition.
```

| Field | Type | Description |
|---|---|---|
| `width` | `u32` | Output frame width in pixels |
| `height` | `u32` | Output frame height in pixels |
| `bitrate` | `u64` | Target video bitrate in bits per second |

## Error Handling

Common variants (not exhaustive):

| Variant | When it occurs |
|---|---|
| `StreamError::InvalidConfig` | Missing input, empty ladder, or conflicting options |
| `StreamError::UnsupportedCodec` | A codec the HLS/DASH muxer cannot package was requested |
| `StreamError::Encode` | Wrapped `EncodeError` from a rendition encode stage |
| `StreamError::Io` | Write failure on the output directory |
| `StreamError::Ffmpeg` | An underlying FFmpeg call returned an error |

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
