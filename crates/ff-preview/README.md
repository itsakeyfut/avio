# ff-preview

Real-time video preview and proxy workflow for Rust. Provides frame-accurate seek, audio-master A/V sync, a `FrameSink` trait for custom renderers, RGBA pixel delivery, and proxy generation with transparent auto-substitution.

> **Project status (as of 2026-06-04):** The library foundation is in place. Development continues through [**avio-editor-demo**](https://github.com/itsakeyfut/avio-editor-demo), a real-world video editing application built on `avio`, which surfaces bugs and drives API improvements. Pull requests, bug reports, and feature requests are welcome — see the [main repository](https://github.com/itsakeyfut/avio) for full context.

## Installation

```toml
[dependencies]
ff-preview = "0.14"

# Enable async support
ff-preview = { version = "0.14", features = ["tokio"] }

# Enable proxy generation
ff-preview = { version = "0.14", features = ["proxy"] }
```

## Quick Start

### Playback with a custom RGBA sink

```rust
use ff_preview::{PreviewPlayer, RgbaSink};

fn main() -> Result<(), ff_preview::PreviewError> {
    let mut player = PreviewPlayer::open("video.mp4")?;
    player.set_sink(Box::new(RgbaSink::new()));
    player.play();
    player.run()?;
    Ok(())
}
```

### Frame-accurate seek

```rust
use std::time::Duration;
use ff_preview::{DecodeBuffer, FrameResult};

let mut buf = DecodeBuffer::open("video.mp4").build()?;
buf.seek(Duration::from_secs(30))?;

loop {
    match buf.pop_frame() {
        FrameResult::Frame(f) => {
            println!("pts: {:?}", f.timestamp().as_duration());
            break;
        }
        FrameResult::Seeking(_) => std::thread::sleep(Duration::from_millis(5)),
        FrameResult::Eof => break,
    }
}
```

### Proxy generation

```rust
use ff_preview::{ProxyGenerator, ProxyResolution};

let proxy_path = ProxyGenerator::new("original_1080p.mp4")?
    .resolution(ProxyResolution::Quarter)
    .output_dir("/tmp")
    .generate()?;

println!("proxy at {}", proxy_path.display());
```

## Feature Flags

| Feature | What it enables |
|---------|----------------|
| *(default)* | `PreviewPlayer`, `DecodeBuffer`, `PlaybackClock`, `FrameSink`, `RgbaSink`, `RgbaFrame`, seek |
| `tokio` | `AsyncPreviewPlayer` |
| `proxy` | `ProxyGenerator`, `ProxyJob`, `ProxyResolution` |

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
