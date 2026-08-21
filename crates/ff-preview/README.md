# ff-preview

Real-time video preview and proxy workflow for Rust. Provides frame-accurate seek, audio-master A/V sync, a `FrameSink` trait for custom renderers, RGBA pixel delivery, and proxy generation with auto-substitution.

`ff-preview` adds a real-time, A/V-synchronised playback and seek loop on top of the decode primitives, converting frames to RGBA via libswscale for display. Decoding is delegated to `ff-decode`; this crate owns the playback clock, frame-accurate seek, and a `FrameSink` trait for custom renderers. Errors are typed and chain their source (`PreviewError`), so a failure reads as an actionable message rather than a raw FFmpeg return code.

It is an independent crate: use it on its own, or combine it with the other `ff-*` crates to build any media app or editing model. The `ff-*` crates are model-free primitives that impose no editing model; [`avio`](https://github.com/itsakeyfut/avio) is one editing engine built on top of them.

## Installation

```toml
[dependencies]
ff-preview = "0.16"

# Enable async support
ff-preview = { version = "0.16", features = ["tokio"] }

# Enable proxy generation
ff-preview = { version = "0.16", features = ["proxy"] }
```

## Quick Start

### Playback with a custom RGBA sink

`PreviewPlayer::open` probes the file and prepares the pipeline. Call `split()`
to obtain an exclusive `PlayerRunner` (owns the decode pipeline; register the
sink and drive it with `run()`) and a cloneable `PlayerHandle` (non-blocking
`play` / `pause` / `seek` / `stop` controls).

```rust
use std::thread;
use ff_preview::{PreviewPlayer, RgbaSink};

fn main() -> Result<(), ff_preview::PreviewError> {
    let (mut runner, handle) = PreviewPlayer::open("video.mp4")?.split();

    let sink = RgbaSink::new();
    let frames = sink.frame_handle(); // Arc<Mutex<Option<RgbaFrame>>> for the render thread
    runner.set_sink(Box::new(sink));

    thread::spawn(move || {
        let _ = runner.run();
    });

    handle.play();

    // In the render loop (any thread):
    if let Some(frame) = frames.lock().unwrap().as_ref() {
        // upload_to_gpu(&frame.data, frame.width, frame.height);
        let _ = (&frame.data, frame.width, frame.height, frame.pts);
    }

    Ok(())
}
```

### Frame-accurate seek

```rust
use std::path::Path;
use std::time::Duration;
use ff_preview::{DecodeBuffer, FrameResult};

let mut buf = DecodeBuffer::open(Path::new("video.mp4")).build()?;
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
use std::path::Path;
use ff_preview::{ProxyGenerator, ProxyResolution};

let proxy_path = ProxyGenerator::new(Path::new("original_1080p.mp4"))?
    .resolution(ProxyResolution::Quarter)
    .output_dir(Path::new("/tmp"))
    .generate()?;

println!("proxy at {}", proxy_path.display());
```

## Feature Flags

| Feature | What it enables |
|---------|----------------|
| *(default)* | `PreviewPlayer`, `PlayerRunner`, `PlayerHandle`, `DecodeBuffer`, `PlaybackClock`, `FrameSink`, `RgbaSink`, `RgbaFrame`, seek |
| `tokio` | `AsyncPreviewPlayer` |
| `proxy` | `ProxyGenerator`, `ProxyJob`, `ProxyResolution` |
| `timeline` | `Scene`, `ScenePlayer`, `SceneRunner` |

## Error Handling

Common variants (not exhaustive):

| Variant | When it occurs |
|---|---|
| `PreviewError::FileNotFound` | The media file was not found |
| `PreviewError::NoVideoStream` | The file has no video stream |
| `PreviewError::SeekFailed` | A seek operation failed |
| `PreviewError::Decode` | Wrapped `DecodeError` from the decode stage |
| `PreviewError::Io` | An I/O error during file operations |

`PreviewError` implements `ff_format::MediaError`, so `err.is_recoverable()` / `err.is_fatal()` work uniformly with the other `ff-*` crates.

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
