# ff-decode

Decode video and audio frames without managing codec contexts, packet queues, or timestamp conversions. Open a file, call `decode_one` in a loop, and receive `VideoFrame` objects with their position already expressed as a `Timestamp`.

> **Project status (as of 2026-06-04):** The library foundation is in place. Development continues through [**avio-editor-demo**](https://github.com/itsakeyfut/avio-editor-demo), a real-world video editing application built on `avio`, which surfaces bugs and drives API improvements. Pull requests, bug reports, and feature requests are welcome — see the [main repository](https://github.com/itsakeyfut/avio) for full context.

## Installation

```toml
[dependencies]
ff-decode = "0.15"
ff-format = "0.15"
```

## Video Decoding

```rust
use ff_decode::VideoDecoder;
use ff_format::PixelFormat;

let mut decoder = VideoDecoder::open("video.mp4")
    .output_format(PixelFormat::Rgba)
    .build()?;

while let Some(frame) = decoder.decode_one()? {
    // frame.data()      — raw pixel bytes in RGBA order
    // frame.width()     — frame width in pixels
    // frame.height()    — frame height in pixels
    // frame.timestamp() — position as Timestamp
    process(&frame);
}
```

### Iterator API

`VideoDecoder` and `AudioDecoder` implement `Iterator` and `FusedIterator`:

```rust
for frame in decoder {
    let frame = frame?;   // Iterator<Item = Result<VideoFrame, DecodeError>>
    process(&frame);
}
```

## Audio Decoding

```rust
use ff_decode::AudioDecoder;
use ff_format::SampleFormat;

let mut decoder = AudioDecoder::open("audio.flac")
    .output_format(SampleFormat::F32p)
    .output_sample_rate(44_100)
    .output_channels(2)   // downmix to stereo
    .build()?;

while let Some(frame) = decoder.decode_one()? {
    // frame.to_f32_interleaved() — interleaved f32 samples
    process(&frame);
}
```

## Image Sequence Decoding

When a path contains `%` (printf-style pattern), `VideoDecoder` automatically
uses the `image2` demuxer. Supported extensions: `.png`, `.jpg`, `.bmp`, `.tiff`.

```rust
use ff_decode::{VideoDecoder, HardwareAccel};

// Decode a numbered PNG sequence at 25 fps.
let mut decoder = VideoDecoder::open("frames/frame%04d.png")
    .hardware_accel(HardwareAccel::None)  // recommended for still images
    .frame_rate(25)
    .build()?;

while let Some(frame) = decoder.decode_one()? {
    process(&frame);
}
```

## OpenEXR Sequence Decoding

OpenEXR sequences use the same `%`-pattern mechanism. EXR frames decode as
`gbrpf32le` (32-bit float, three planes ordered G/B/R):

```rust
use ff_decode::{VideoDecoder, HardwareAccel};
use ff_format::PixelFormat;

// Hardware decoders do not support EXR; always use HardwareAccel::None.
let mut decoder = VideoDecoder::open("frames/frame%04d.exr")
    .hardware_accel(HardwareAccel::None)
    .frame_rate(24)
    .build()?;  // returns DecodeError::DecoderUnavailable if --enable-decoder=exr
                // was omitted from the FFmpeg build

while let Some(frame) = decoder.decode_one()? {
    assert_eq!(frame.format(), PixelFormat::Gbrpf32le);
    // Access individual colour planes: plane(0)=G, plane(1)=B, plane(2)=R
    let green_plane = frame.plane(0).unwrap();
    // Each element is a 4-byte IEEE 754 f32 in native byte order.
}
```

## 10-bit and High-Bit-Depth Formats

HDR and professional content often uses 10-bit pixel formats. Request conversion
via `.output_format()` or leave unset to receive frames in the native format:

```rust
use ff_format::PixelFormat;

// Receive frames in the native 10-bit format (no conversion).
let mut decoder = VideoDecoder::open("hdr.mkv").build()?;

// Or convert to a specific format for processing.
let mut decoder = VideoDecoder::open("hdr.mkv")
    .output_format(PixelFormat::Yuv420p10le)
    .build()?;
```

Common 10-bit formats: `Yuv420p10le`, `Yuv422p10le`, `Yuv444p10le`, `P010Le`.

## Scaled Output

```rust
use ff_decode::VideoDecoder;
use ff_format::PixelFormat;

let mut decoder = VideoDecoder::open("4k.mp4")
    .output_format(PixelFormat::Rgb24)
    .output_size(1280, 720)   // scale + pixel-format conversion in one pass
    .build()?;
```

## Seeking

```rust
use ff_decode::{VideoDecoder, SeekMode};
use std::time::Duration;

let mut decoder = VideoDecoder::open("video.mp4").build()?;

// Jump to the nearest keyframe at or before 30 seconds.
decoder.seek(Duration::from_secs(30), SeekMode::Keyframe)?;

// Jump to the exact position (may decode additional frames internally).
decoder.seek(Duration::from_secs(30), SeekMode::Exact)?;
```

Seeking does not re-open the file. The existing codec context is flushed and reused.

## Hardware Acceleration

```rust
use ff_decode::{VideoDecoder, HardwareAccel};

let mut decoder = VideoDecoder::open("video.mp4")
    .hardware_accel(HardwareAccel::Auto)
    .build()?;
```

`HardwareAccel::Auto` probes for available accelerators — NVDEC, QSV, AMF, VideoToolbox, and VAAPI — and falls back to software decoding if none is available.

## Error Handling

| Variant                                | When it occurs                                   |
|----------------------------------------|--------------------------------------------------|
| `DecodeError::FileNotFound`            | The input path does not exist                    |
| `DecodeError::NoVideoStream`           | The container has no video stream                |
| `DecodeError::UnsupportedCodec`        | No decoder available for the stream's codec      |
| `DecodeError::DecoderUnavailable`      | Codec is known but not compiled into FFmpeg      |
| `DecodeError::InvalidOutputDimensions` | Requested output width/height is invalid         |
| `DecodeError::Ffmpeg`                  | An underlying FFmpeg call returned an error      |

## What the Crate Handles for You

- Codec context allocation and lifetime
- PTS-to-`Timestamp` conversion using the stream's time base
- Packet queue management and buffering
- EOF signalled as `Ok(None)` rather than a special error variant
- Pixel format and sample format negotiation via `swscale` / `swresample`
- `image2` demuxer selection for `%`-pattern paths (image sequences)

## Feature Flags

| Flag | Description | Default |
|------|-------------|---------|
| `tokio` | Enables `AsyncVideoDecoder` and `AsyncAudioDecoder`. Wraps each blocking FFmpeg call in `tokio::task::spawn_blocking` and exposes a `futures::Stream` interface via `into_stream()`. Requires a tokio 1.x runtime. | disabled |

```toml
[dependencies]
ff-decode = { version = "0.15", features = ["tokio"] }
```

When the `tokio` feature is disabled, only the synchronous `VideoDecoder` and `AudioDecoder` APIs are compiled. No tokio dependency is pulled in.

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
