# ff-format

Shared data types for the ff-* crate family. No FFmpeg dependency — these types exist so crates can exchange frames and stream metadata without coupling to FFmpeg's internal structs.

> **Project status (as of 2026-06-04):** The library foundation is in place. Development continues through [**avio-editor-demo**](https://github.com/itsakeyfut/avio-editor-demo), a real-world video editing application built on `avio`, which surfaces bugs and drives API improvements. Pull requests, bug reports, and feature requests are welcome — see the [main repository](https://github.com/itsakeyfut/avio) for full context.

## Key Types

| Type              | Description                                                          |
|-------------------|----------------------------------------------------------------------|
| `PixelFormat`     | Enumeration of pixel formats (`Yuv420p`, `Rgba`, `Yuv420p10le`, `Yuv422p10le`, `Gbrpf32le`, …) |
| `SampleFormat`    | Audio sample formats (`Fltp`, `S16`, …)                              |
| `ChannelLayout`   | `Mono`, `Stereo`, `Surround51`, and others                           |
| `ColorSpace`      | `Bt601`, `Bt709`, `Bt2020`, and others                               |
| `ColorRange`      | `Limited` (studio swing) or `Full` (PC swing)                        |
| `ColorPrimaries`  | `Bt709`, `Bt2020`, `DciP3`, and others                               |
| `ColorTransfer`   | OETF tag: `Bt709`, `Pq` (HDR10 / SMPTE ST 2084), `Hlg` (BT.2100), … |
| `Hdr10Metadata`   | `MaxCLL` + `MaxFALL` + `MasteringDisplay` for HDR10 static metadata  |
| `MasteringDisplay`| SMPTE ST 2086 mastering display colour volume (chromaticity + luminance) |
| `VideoCodec`      | `H264`, `H265`, `Vp9`, `Av1`, `Av1Svt`, `ProRes`, `DnxHd`, and others |
| `AudioCodec`      | `Aac`, `Opus`, `Mp3`, `Flac`, `Vorbis`, and others                  |
| `VideoFrame`      | Decoded video frame with pixel data and associated `Timestamp`       |
| `AudioFrame`      | Decoded audio frame with sample data and associated `Timestamp`      |
| `Timestamp`       | Media position expressed as a `Rational` fraction of seconds         |

These are the types `ff-decode` hands back when you decode a frame, and the types `ff-encode` expects when you push a frame.

## Color Science Types

HDR and colour-space metadata is represented as pure-Rust enums — no FFmpeg dependency required:

```rust
use ff_format::{ColorTransfer, ColorSpace, ColorPrimaries, ColorRange};

// Tag a stream as HLG broadcast HDR.
let transfer  = ColorTransfer::Hlg;
let space     = ColorSpace::Bt2020;
let primaries = ColorPrimaries::Bt2020;
let range     = ColorRange::Limited;

// Tag a stream as HDR10 (PQ transfer + BT.2020 primaries).
let transfer  = ColorTransfer::Pq;
```

`Hdr10Metadata` and `MasteringDisplay` carry HDR10 static metadata that is
embedded as side data on key-frame packets:

```rust
use ff_format::{Hdr10Metadata, MasteringDisplay};

// BT.2020 D65 primaries, coordinates × 50000; luminance × 10000 nits.
let meta = Hdr10Metadata {
    max_cll: 1000,    // MaxCLL in nits
    max_fall: 400,    // MaxFALL in nits
    mastering_display: MasteringDisplay {
        red_x: 17000, red_y: 8500,
        green_x: 13250, green_y: 34500,
        blue_x: 7500, blue_y: 3000,
        white_x: 15635, white_y: 16450,
        min_luminance: 50,           // 0.005 nit deep black
        max_luminance: 10_000_000,   // 1000 nit peak
    },
};
```

## Example

```rust
use ff_format::{Timestamp, Rational};

// Create a timestamp at 2.5 seconds (5/2).
let ts = Timestamp::new(5, 2);

// Arithmetic on rational timestamps stays exact.
let one_second = Timestamp::new(1, 1);
let three_seconds = ts + one_second * Rational::new(1, 1);

assert_eq!(ts.as_secs_f64(), 2.5);
```

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
