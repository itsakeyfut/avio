# ff-analysis

Media-analysis primitives for Rust: scene, silence, black-frame and keyframe detection, histogram and waveform extraction, tempo (BPM) results, and video scopes.

`ff-analysis` reads decoded media and reports analytical data; it does not edit or transform it. It builds on [`ff-decode`](https://crates.io/crates/ff-decode) for frame access and drives its own FFmpeg filter graphs where needed. Errors are typed and carry human-readable context (`AnalysisError`), so a failure reads as an actionable message rather than a raw FFmpeg return code.

It is an independent crate: use it on its own, or combine it with the other `ff-*` crates to build any media app or editing model. The `ff-*` crates are model-free primitives that impose no editing model; [`avio`](https://github.com/itsakeyfut/avio) is one editing engine built on top of them.

## Installation

```toml
[dependencies]
ff-analysis = "0.16"
```

FFmpeg 7.x or 8.x development libraries must be installed on your system.

## What it provides

| Tool | Purpose |
|---|---|
| `SceneDetector` | Detect scene-cut timestamps |
| `SilenceDetector` | Detect silent ranges in the audio |
| `BlackFrameDetector` | Detect near-black frames |
| `KeyframeEnumerator` | List keyframe (I-frame) timestamps |
| `HistogramExtractor` | Per-frame luminance / RGB histograms |
| `WaveformAnalyzer` | Audio amplitude waveform samples |
| `ScopeAnalyzer` | Frame-level scopes: waveform, vectorscope, RGB parade, histogram |
| `BpmResult` | Tempo-detection result type (detector planned) |

## Audio waveform

```rust
use ff_analysis::WaveformAnalyzer;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // One peak/RMS sample per 100 ms interval, both in dBFS.
    let samples = WaveformAnalyzer::new("clip.mp4")
        .interval(Duration::from_millis(100))
        .run()?;

    for s in &samples {
        println!("{:?}: peak={:.1} dBFS  rms={:.1} dBFS", s.timestamp, s.peak_db, s.rms_db);
    }

    Ok(())
}
```

## Video scopes

The scope functions are pure pixel arithmetic over a decoded `ff_format::VideoFrame`, so
this example pulls one frame from [`ff-decode`](https://crates.io/crates/ff-decode) (add
`ff-decode` and `ff-format` to your `Cargo.toml`). Scopes support the `yuv420p`, `yuv422p`,
and `yuv444p` pixel formats; requesting `Yuv420p` output keeps the result deterministic.

```rust
use ff_analysis::ScopeAnalyzer;
use ff_decode::VideoDecoder;
use ff_format::PixelFormat;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut decoder = VideoDecoder::open("clip.mp4")
        .output_format(PixelFormat::Yuv420p)
        .build()?;
    let frame = decoder
        .decode_one()?
        .ok_or("clip.mp4 has no video frames")?;

    let hist = ScopeAnalyzer::histogram(&frame);
    let parade = ScopeAnalyzer::rgb_parade(&frame);
    println!("luma[255]={}  parade columns={}", hist.luma[255], parade.r.len());

    Ok(())
}
```

## Error Handling

| Variant | When it occurs |
|---|---|
| `AnalysisError::Failed` | A structural precondition failed (e.g. a zero interval, an unsupported format) |
| `AnalysisError::BpmDetectionFailed` | Tempo detection could not proceed (reserved for the planned detector) |
| `AnalysisError::Decode` | An error propagated from the underlying decoder |

`AnalysisError` implements `ff_format::MediaError`, so `err.is_recoverable()` / `err.is_fatal()` work uniformly with the other `ff-*` crates.

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
