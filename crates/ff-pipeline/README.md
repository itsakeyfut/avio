# ff-pipeline

Wire decode, filter, and encode into a single configured pipeline. Instead of managing three separate contexts, set an input path, an output path with codec settings, and an optional filter chain; the builder validates the configuration before any processing begins.

Part of the [`avio`](https://github.com/itsakeyfut/avio) crate family.

## Installation

```toml
[dependencies]
ff-pipeline = "0.15"
```

## Building a Pipeline

```rust
use ff_pipeline::{Pipeline, EncoderConfig};
use ff_format::{VideoCodec, AudioCodec};
use ff_encode::BitrateMode;

let config = EncoderConfig::builder()
    .video_codec(VideoCodec::H264)
    .audio_codec(AudioCodec::Aac)
    .bitrate_mode(BitrateMode::Cbr(4_000_000))
    .resolution(1280, 720)
    .build();

let pipeline = Pipeline::builder()
    .input("input.mp4")
    .output("output.mp4", config)
    .on_progress(|p| {
        println!("frame={} elapsed={:.1}s", p.frames_processed, p.elapsed.as_secs_f64());
        true // return false to cancel
    })
    .build()?;

pipeline.run()?;
```

## Configuration Validation

`build()` validates the full configuration before allocating any FFmpeg context:

| Error variant                              | Condition                                              |
|--------------------------------------------|--------------------------------------------------------|
| `PipelineError::NoInput`                   | No input path was provided to the builder              |
| `PipelineError::NoOutput`                  | `output()` was not called                              |
| `PipelineError::SecondaryInputWithoutFilter` | `secondary_input()` was called without a filter graph |

These errors are returned from `build()`, not from `run()`.

## Progress and Cancellation

The progress callback receives a `Progress` value on each encoded frame:

| Field / Method       | Type              | Description                               |
|----------------------|-------------------|-------------------------------------------|
| `p.frames_processed` | `u64`             | Number of frames encoded so far           |
| `p.total_frames`     | `Option<u64>`     | Total frames if known from container      |
| `p.elapsed`          | `Duration`        | Wall-clock time since `run()` was called  |
| `p.percent()`        | `Option<f64>`     | `(frames_processed / total_frames) * 100` |

Return `false` from the callback to stop processing. The pipeline drains in-flight frames and returns `Err(PipelineError::Cancelled)`.

## Error Handling

| Variant                    | When it occurs                                |
|----------------------------|-----------------------------------------------|
| `PipelineError::NoInput`   | Builder has no input path                     |
| `PipelineError::NoOutput`  | `output()` was not called                     |
| `PipelineError::Decode`    | Wrapped `DecodeError` from the decode stage   |
| `PipelineError::Filter`    | Wrapped `FilterError` from the filter stage   |
| `PipelineError::Encode`    | Wrapped `EncodeError` from the encode stage   |
| `PipelineError::Cancelled` | Progress callback returned `false`            |

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
