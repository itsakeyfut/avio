# ff-filter

Apply video and audio transformations without writing FFmpeg filter-graph strings. Build a chain with method calls; the graph description is generated and validated internally.

`ff-filter` is a safe, ergonomic wrapper over FFmpeg's libavfilter: build and run video and audio filter graphs without hand-writing filter-graph strings. Errors are typed and contextual (`FilterError`), so a bad input slot or an FFmpeg build error surfaces as a readable message rather than a raw return code.

It is an independent crate: use it on its own, or combine it with the other `ff-*` crates to build any media app or editing model. The `ff-*` crates are model-free primitives that impose no editing model; [`avio`](https://github.com/itsakeyfut/avio) is one editing engine built on top of them.

## Installation

```toml
[dependencies]
ff-filter = "0.16"
```

## Building a Filter Chain

Filtering needs input frames, so this example decodes them with
[`ff-decode`](https://crates.io/crates/ff-decode), runs each through the graph,
and encodes the results with [`ff-encode`](https://crates.io/crates/ff-encode)
(add both as dependencies to run it):

```rust
use ff_decode::VideoDecoder;
use ff_encode::{BitrateMode, VideoCodec, VideoEncoder};
use ff_filter::{FilterGraph, ScaleAlgorithm};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Build a chain; the graph description is generated and validated internally.
    let mut graph = FilterGraph::builder()
        .trim(10.0, 30.0)                           // keep seconds 10-30
        .scale(1280, 720, ScaleAlgorithm::Fast)     // resize to 720p
        .fade_in(0.0, 0.5)                          // 0.5-second fade in at the start
        .fade_out(19.5, 0.5)                        // 0.5-second fade out
        .build()?;

    // A decoder for the input, an encoder for the graph's 1280×720 output.
    let mut decoder = VideoDecoder::open("input.mp4").build()?;
    let (width, height) = graph.output_resolution().unwrap_or((1280, 720));
    let mut encoder = VideoEncoder::create("output.mp4")
        .video(width, height, decoder.frame_rate())
        .video_codec(VideoCodec::H264)
        .bitrate_mode(BitrateMode::Crf(23)) // 0-51, lower = higher quality
        .build()?;

    // Push decoded frames into input slot 0 and pull transformed frames out.
    while let Some(input_frame) = decoder.decode_one()? {
        graph.push_video(0, &input_frame)?;
        while let Some(output_frame) = graph.pull_video()? {
            encoder.push_video(&output_frame)?;
        }
    }
    encoder.finish()?; // flush buffered frames and finalize the file

    Ok(())
}
```

`build()` checks the configured steps and returns an `Err` if a value is out of range or the step set is empty. The underlying `FFmpeg` graph is constructed lazily on the first `push_video` / `push_audio` call, using the first frame's format.

## Available Video Operations

A representative selection; see the API docs for the full set
(colour grading, blurs, denoise, keying, transitions, text, and more).

| Method                            | Effect                                               |
|-----------------------------------|------------------------------------------------------|
| `trim(start, end)`                | Discard frames outside the given time range (secs)   |
| `scale(w, h, algorithm)`          | Resize frames using the given resampling algorithm   |
| `fit_to_aspect(w, h, color)`      | Scale to fit `w × h`, preserving aspect (letterbox)  |
| `crop(x, y, w, h)`                | Extract a rectangular region                         |
| `overlay(x, y)`                   | Composite a second video stream at (x, y)            |
| `fade_in(start, duration)`        | Fade from black, starting at `start` (secs)          |
| `fade_out(start, duration)`       | Fade to black, starting at `start` (secs)            |
| `rotate(degrees, fill_color)`     | Rotate clockwise; exposed corners filled with color  |
| `tone_map(ToneMap::Hable)`        | HDR-to-SDR tone mapping with the selected curve      |

## Available Audio Operations

| Method                   | Effect                                            |
|--------------------------|---------------------------------------------------|
| `volume(gain_db)`        | Adjust loudness by the given number of dB         |
| `equalizer(bands)`       | Apply a multi-band parametric EQ (`Vec<EqBand>`)  |
| `amix(inputs)`           | Mix multiple audio streams into one               |

## Hardware Acceleration

```rust
use ff_filter::{FilterGraph, HwAccel, ScaleAlgorithm};

let graph = FilterGraph::builder()
    .scale(1920, 1080, ScaleAlgorithm::Fast)
    .hardware(HwAccel::Cuda)
    .build()?;
```

`HwAccel` selects the device type (`Cuda`, `VideoToolbox`, or `Vaapi`). When
hardware is enabled, `hwupload` / `hwdownload` filters are inserted around the
chain automatically.

## Using the Filter Graph

The single-input loop is shown in [Building a Filter Chain](#building-a-filter-chain)
above: push each decoded frame into slot 0 with `push_video`, then drain the
graph with `pull_video`.

Multi-input filters (such as `xfade` or `overlay`) read from additional slots:
push clip A frames to slot 0 and clip B frames to slot 1.

```rust
// `graph` built with .overlay(x, y); `frame_a` / `frame_b` are VideoFrames
// from two decoders.
graph.push_video(0, &frame_a)?; // main stream → slot 0
graph.push_video(1, &frame_b)?; // overlay stream → slot 1
while let Some(output_frame) = graph.pull_video()? {
    // encode or display `output_frame`
}
```

## Error Handling

| Variant                          | When it occurs                                             |
|----------------------------------|------------------------------------------------------------|
| `FilterError::InvalidConfig`     | A configured value is out of range (returned by `build()`) |
| `FilterError::BuildFailed`       | No steps were added, or the `FFmpeg` graph cannot be built |
| `FilterError::InvalidInput`      | A frame was pushed to an out-of-range input slot           |
| `FilterError::ProcessFailed`     | A push or pull operation failed                            |
| `FilterError::Ffmpeg`            | An underlying `FFmpeg` function returned an error code     |
| `FilterError::CompositionFailed` | A multi-track composition or mixing operation failed       |
| `FilterError::AnalysisFailed`    | An analysis operation (e.g. loudness measurement) failed   |
| `FilterError::GplRequired`       | A filter needing the `gpl` feature was used without it     |

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
