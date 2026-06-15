# ff-stream

Produce HLS and DASH adaptive bitrate output from any video source. Define a rendition ladder,
point it at an input file, and receive a standards-compliant package ready for CDN delivery.

> **Project status (as of 2026-06-04):** The library foundation is in place. Development continues through [**avio-editor-demo**](https://github.com/itsakeyfut/avio-editor-demo), a real-world video editing application built on `avio`, which surfaces bugs and drives API improvements. Pull requests, bug reports, and feature requests are welcome — see the [main repository](https://github.com/itsakeyfut/avio) for full context.

## Installation

```toml
[dependencies]
ff-stream = "0.15"
```

## HLS Output

`HlsOutput` is a consuming builder. Setters take `self` and return `Self`; validation is
deferred to `build()`, and `write()` performs the encode-and-mux.

```rust
use ff_stream::HlsOutput;
use std::time::Duration;

HlsOutput::new("hls_output/")
    .input("source.mp4")
    .segment_duration(Duration::from_secs(6))
    .keyframe_interval(48)
    .build()?
    .write()?;
// Writes hls_output/playlist.m3u8 and numbered segments (segment000.ts, …).
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

| Variant | When it occurs |
|---|---|
| `StreamError::InvalidConfig` | Missing input, empty ladder, or conflicting options |
| `StreamError::Encode` | Wrapped `EncodeError` from a rendition encode stage |
| `StreamError::Io` | Write failure on the output directory |

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
