# ff-remux

Stream-copy remuxing for Rust: trim a clip and replace / extract / add an audio stream, all **without re-encoding**.

`ff-remux` is a safe, ergonomic Rust wrapper over FFmpeg's stream-copy remuxing (libavformat demux + mux, no libavcodec encoding). Errors are typed and carry human-readable context (`RemuxError`), so a failure reads as an actionable message rather than a raw FFmpeg return code.

It is an independent crate: use it on its own, or combine it with the other `ff-*` crates to build any media app or editing model. The `ff-*` crates are model-free primitives that impose no editing model; [`avio`](https://github.com/itsakeyfut/avio) is one editing engine built on top of them.

## Installation

```toml
[dependencies]
ff-remux = "0.17"
```

FFmpeg 7.x or 8.x development libraries must be installed on your system.

## Trim (stream copy)

`StreamCopyTrim` copies the selected time range without re-encoding, so it is fast and lossless.

```rust
use ff_remux::StreamCopyTrim;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Copy the range [10s, 25s) from input.mp4 into clip.mp4 without re-encoding.
    let start = Duration::from_secs(10);
    let end = Duration::from_secs(25);

    // new(input, start, end, output)
    StreamCopyTrim::new("input.mp4", start, end, "clip.mp4").run()?;

    let length = end - start;
    println!("wrote clip.mp4 ({length:?})");
    Ok(())
}
```

## Audio replace / extract / add

```rust
use ff_remux::{AudioReplacement, AudioExtractor, AudioAdder};

// Replace the audio track of a video with a new audio file.
AudioReplacement::new("video.mp4", "music.aac", "out.mp4").run()?;

// Extract the audio track to its own file.
AudioExtractor::new("video.mp4", "audio.aac").run()?;

// Add an audio track to a video that has none.
AudioAdder::new("silent.mp4", "voice.aac", "out.mp4").run()?;
```

## Error Handling

| Variant | When it occurs |
|---|---|
| `RemuxError::InvalidConfig` | A configuration value is missing or invalid |
| `RemuxError::OperationFailed` | A structural precondition failed (e.g. no matching stream) |
| `RemuxError::Ffmpeg` | An underlying FFmpeg call returned an error (`code` + message) |
| `RemuxError::Io` | An I/O error on the input or output file |

`RemuxError` implements `ff_format::MediaError`, so `err.is_recoverable()` / `err.is_fatal()` work uniformly with the other `ff-*` crates.

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
