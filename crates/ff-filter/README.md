# ff-filter

Apply video and audio transformations without writing FFmpeg filter-graph strings. Build a chain with method calls; the graph description is generated and validated internally.

> **Project status (as of 2026-06-04):** The library foundation is in place. Development continues through [**avio-editor-demo**](https://github.com/itsakeyfut/avio-editor-demo), a real-world video editing application built on `avio`, which surfaces bugs and drives API improvements. Pull requests, bug reports, and feature requests are welcome — see the [main repository](https://github.com/itsakeyfut/avio) for full context.

## Installation

```toml
[dependencies]
ff-filter = "0.12"
```

## Building a Filter Chain

```rust
use ff_filter::FilterGraph;

let graph = FilterGraph::builder()
    .trim(10.0, 30.0)   // keep seconds 10–30
    .scale(1280, 720)   // resize to 720p
    .fade_in(0.5)       // 0.5-second fade in
    .fade_out(0.5)      // 0.5-second fade out at end
    .build()?;
```

`build()` validates the graph before any frames are processed. An `Err` is returned if the combination is unsupported, not at the first `push_video` call.

## Available Video Operations

| Method                     | Effect                                               |
|----------------------------|------------------------------------------------------|
| `trim(start, end)`         | Discard frames outside the given time range (secs)   |
| `scale(w, h)`              | Resize frames, preserving aspect ratio if w or h is 0|
| `crop(x, y, w, h)`         | Extract a rectangular region                         |
| `overlay(x, y)`            | Composite a second video stream at (x, y)            |
| `fade_in(duration)`        | Fade from black over the given duration in seconds   |
| `fade_out(duration)`       | Fade to black over the given duration in seconds     |
| `rotate(degrees)`          | Rotate by an arbitrary angle; edges are filled       |
| `tone_map(ToneMap::Hable)` | HDR-to-SDR tone mapping with the selected curve      |

## Available Audio Operations

| Method                        | Effect                                     |
|-------------------------------|--------------------------------------------|
| `volume(gain_db)`             | Adjust loudness by the given number of dB  |
| `equalizer(band_hz, gain_db)` | Boost or cut a frequency band              |
| `amix(inputs)`                | Mix multiple audio streams into one        |

## Hardware Acceleration

```rust
use ff_filter::{FilterGraph, HwAccel};

let graph = FilterGraph::builder()
    .scale(1920, 1080)
    .hardware(HwAccel::Cuda)
    .build()?;
```

If the requested device is unavailable, the graph falls back to CPU processing automatically.

## Using the Filter Graph

```rust
// Push decoded frames in and pull transformed frames out.
while let Some(input_frame) = decoder.decode_frame()? {
    graph.push_video(input_frame)?;
    while let Some(output_frame) = graph.pull_video()? {
        encoder.push_video(&output_frame)?;
    }
}
// Flush remaining frames from the graph.
graph.flush()?;
while let Some(output_frame) = graph.pull_video()? {
    encoder.push_video(&output_frame)?;
}
```

## Error Handling

| Variant                     | When it occurs                                          |
|-----------------------------|---------------------------------------------------------|
| `FilterError::InvalidGraph` | Filter combination is unsupported or self-contradictory |
| `FilterError::HardwareInit` | Requested HwAccel device could not be initialised       |
| `FilterError::Push`         | FFmpeg returned an error while buffering a frame        |
| `FilterError::Pull`         | FFmpeg returned an error while retrieving a frame       |

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
