# avio

[![Crates.io](https://img.shields.io/crates/v/avio.svg)](https://crates.io/crates/avio)
[![Docs.rs](https://docs.rs/avio/badge.svg)](https://docs.rs/avio)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

An editing engine for video and audio: build a `Timeline` of `Clip`s, edit it with full undo/redo, and render it to a file.

`avio` is the **editing engine** at the top of the `ff-*` crate family. It owns the editing model: an immutable `Timeline` of `Clip`s across video and audio tracks, the derivation that turns that model into rendered frames, and an `Editor` with undo/redo. The `ff-*` primitives it builds on (decode, encode, filter, analysis, remux, stream, preview, GPU render) stay model-free; for standalone primitive work, depend on those crates directly. See the [main repository](https://github.com/itsakeyfut/avio) for the full architecture.

## What is avio?

avio is a **video editing engine**: you describe an edit as data and the engine renders it.

- **An editing model, not a wrapper**: avio owns an immutable `Timeline` of `Clip`s across video and audio tracks, which you build, edit, and render.
- **Non-destructive and undoable**: every edit is a pure function over the model, and `Editor` provides full undo/redo where one edit is one step.
- **Renders the model to a file**: the engine derives frames from the timeline (compositing, transitions, effects, keyframes) and encodes the result.
- **Built on model-free primitives**: the `ff-*` crates do the decode / encode / filter / render work and impose no editing model; avio is one engine on top of them, and you can build a different one on the same primitives.

## Installation

```toml
[dependencies]
# The editing engine: Timeline, Clip, Editor, render (build, edit, export).
avio = "0.16"

# With real-time preview:
avio = { version = "0.16", features = ["preview"] }
```

The editing engine (`Timeline` / `Clip` / `Editor` / `render`), media probing (`open`), and analysis are always available — `cargo add avio` gives you the engine. For standalone primitive work (a raw decoder, encoder, pipeline, stream output, or the GPU compositor), depend on the `ff-*` crate directly (see [Working with the primitives directly](#working-with-the-primitives-directly)).

The `ff-*` crates and `avio` share a single workspace version and are released together, so all versions move in lockstep; each is still a separate crate you can depend on on its own.

## Feature Flags

The editing engine is always present; the flags add opt-in capabilities.

| Feature   | Enables                                             | Default |
|-----------|-----------------------------------------------------|---------|
| `hwaccel` | hardware-accelerated export                         | yes     |
| `preview` | real-time `TimelinePlayer` and `Scene` types        | no      |
| `serde`   | `serde` (de)serialization of the model              | no      |
| `gpl`     | GPL-only codecs (x264 / x265)                       | no      |

## Quick Start

### Build and render a timeline

Place one clip on a video track, size the canvas, and render the timeline to a file.

```rust
use avio::{Clip, EncoderConfig, Timeline};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // One clip on a video track; canvas and frame rate set explicitly.
    let timeline = Timeline::builder()
        .canvas(1920, 1080)
        .frame_rate(30.0)
        .video_track(vec![Clip::new("input.mp4")])
        .build()?;

    // Derive frames from the timeline (composite, encode) in one call.
    timeline.render("output.mp4", EncoderConfig::builder().build())?;

    Ok(())
}
```

### Compose multiple clips and tracks

Tracks composite bottom-up. A clip carries its own timeline `offset`, source `trim`,
opacity, and an optional transition; overlapping two clips on one track with a
crossfade is just an offset plus `with_transition`.

```rust
use std::time::Duration;
use avio::{Clip, Timeline, EncoderConfig, XfadeTransition};

let clip_len = Duration::from_secs(2);
let xfade = Duration::from_millis(500);

let timeline = Timeline::builder()
    .canvas(1920, 1080)
    .frame_rate(30.0)
    // V1: two clips crossfading into each other.
    .video_track(vec![
        Clip::new("a.mp4").trim(Duration::ZERO, clip_len),
        Clip::new("b.mp4")
            .trim(Duration::ZERO, clip_len)
            .offset(clip_len - xfade)
            .with_transition(XfadeTransition::Fade, xfade),
    ])
    // V2: a half-opacity overlay on top.
    .video_track(vec![Clip::new("overlay.mp4").with_opacity(0.5)])
    .build()?;

timeline.render("output.mp4", EncoderConfig::builder().build())?;
```

### Edit with undo/redo

`Timeline` is immutable and edits are pure: `apply(&timeline, &command)` returns a
new timeline. `Editor` wraps that with history, where one `apply` is one undo step.
Clips and tracks are addressed by stable ids that survive inserts, removes, and
undo/redo.

```rust
use avio::{Editor, Command, ClipProperty, EncoderConfig};

let mut editor = Editor::new(timeline);
let clip_id = editor.current().video_tracks()[0].clips[0].id;

editor.apply(&Command::SetClipProperty {
    clip: clip_id,
    property: ClipProperty::Opacity(0.5),
})?;

editor.undo(); // back to full opacity
editor.redo(); // 0.5 again

// `render` consumes a `Timeline`; clone the current version to keep editing.
editor.current().clone().render("output.mp4", EncoderConfig::builder().build())?;
```

### Probe media

```rust
use avio::open;

let info = open("video.mp4")?;
if let Some(video) = info.primary_video() {
    println!("{}x{} @ {:.2} fps ({:?})", video.width(), video.height(), video.fps(), video.codec());
}
```

## Working with the primitives directly

`avio` is the engine; the primitives are the `ff-*` crates. For standalone
decoding, encoding, filtering, analysis, remuxing, streaming, preview, or GPU
rendering, depend on the crate you need directly. Each has its own README with
worked examples:

| Crate         | For                                            |
|---------------|------------------------------------------------|
| `ff-probe`    | Read-only media metadata                       |
| `ff-decode`   | Video / audio / image decoding                 |
| `ff-analysis` | Scene, silence, BPM, keyframe, scopes          |
| `ff-encode`   | Video / audio encoding, per-codec and HDR options |
| `ff-remux`    | Stream-copy trim and audio ops (no re-encode)  |
| `ff-filter`   | libavfilter graph construction                 |
| `ff-pipeline` | Decode → filter → encode transcode             |
| `ff-stream`   | HLS / DASH adaptive streaming                  |
| `ff-preview`  | Real-time playback, seek, proxy workflow       |
| `ff-render`   | GPU compositing (wgpu)                          |

## Crate Family

| Crate         | Purpose                                        |
|---------------|------------------------------------------------|
| `ff-sys`      | Raw bindgen FFI (internal use only)            |
| `ff-common`   | Shared buffer-pooling abstractions             |
| `ff-format`   | Pure-Rust type definitions (no FFmpeg linkage) |
| `ff-probe`    | Read-only media metadata extraction            |
| `ff-decode`   | Video and audio decoding                       |
| `ff-analysis` | Media analysis (scene / silence / BPM / scopes)|
| `ff-encode`   | Video and audio encoding                       |
| `ff-remux`    | Stream-copy remux (trim, audio ops), no re-encode |
| `ff-filter`   | Filter graph operations                        |
| `ff-pipeline` | Decode, filter, encode pipeline                |
| `ff-stream`   | HLS / DASH adaptive streaming output           |
| `ff-preview`  | Real-time preview and proxy workflow           |
| `ff-render`   | GPU compositing pipeline (wgpu)                |
| `avio`        | Editing engine (this crate)                    |

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
