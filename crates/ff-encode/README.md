# ff-encode

Encode video and audio with a builder chain. The encoder validates codec, resolution, and bitrate settings before allocating any FFmpeg context. Invalid configurations are returned as `Err`, not discovered at the first `push_video` call.

`ff-encode` is a safe, ergonomic wrapper over FFmpeg's encode path: libavcodec encoding and libavformat muxing, with optional hardware encoders. Settings are validated before any FFmpeg context is allocated, and errors are typed and contextual (`EncodeError`), so a bad codec/container combination or an out-of-range bitrate fails with an actionable message rather than a raw FFmpeg return code.

It is an independent crate: use it on its own, or combine it with the other `ff-*` crates to build any media app or editing model. The `ff-*` crates are model-free primitives that impose no editing model; [`avio`](https://github.com/itsakeyfut/avio) is one editing engine built on top of them.

## Installation

```toml
[dependencies]
ff-encode = "0.16"
ff-format = "0.16"

# Enable GPL-licensed encoders (libx264, libx265).
# Requires GPL compliance or MPEG LA licensing in your project.
# ff-encode = { version = "0.16", features = ["gpl"] }
```

By default, only LGPL-compatible encoders are enabled.

## Quick Start

Encoding needs frames from somewhere; the example below decodes `input.mp4`
with [`ff-decode`](https://crates.io/crates/ff-decode) and re-encodes it as a
video-only H.264 file. Add `ff-decode = "0.16"` to run it.

```rust
use ff_decode::VideoDecoder;
use ff_encode::{VideoEncoder, VideoCodec, BitrateMode};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open the source; the decoder reports its resolution and frame rate.
    let mut decoder = VideoDecoder::open("input.mp4").build()?;
    let (width, height, fps) = (decoder.width(), decoder.height(), decoder.frame_rate());

    let mut encoder = VideoEncoder::create("output.mp4")
        .video(width, height, fps)           // width, height, fps
        .video_codec(VideoCodec::H264)
        .bitrate_mode(BitrateMode::Crf(23))  // 0-51, lower = better
        .build()?;

    while let Some(frame) = decoder.decode_one()? {
        encoder.push_video(&frame)?;
    }
    encoder.finish()?; // required to finalize; Drop does not flush the muxer
    Ok(())
}
```

To add an audio track, configure `.audio(sample_rate, channels)` /
`.audio_codec(...)` and push decoded `AudioFrame`s with `encoder.push_audio(&frame)?`.
A configured audio stream that never receives frames produces an invalid file,
so only declare audio when you actually feed it.

## Quality Modes

```rust
// Constant bitrate: predictable file size and bandwidth.
.bitrate_mode(BitrateMode::Cbr(4_000_000))  // 4 Mbps

// Constant rate factor: quality-driven, file size varies.
.bitrate_mode(BitrateMode::Crf(23))          // 0-51, lower = better

// Variable bitrate: target average with hard ceiling.
.bitrate_mode(BitrateMode::Vbr { target: 3_000_000, max: 6_000_000 })
```

## Per-Codec Video Options

`VideoCodecOptions` provides typed configuration for each codec. Options are
applied via `av_opt_set` before `avcodec_open2`; unsupported values are logged
as warnings and skipped. `build()` never fails due to an unsupported option.

```rust
use ff_encode::{
    VideoEncoder, VideoCodec, VideoCodecOptions,
    H264Options, H264Profile, H264Preset,
    H265Options, H265Profile,
    Av1Options, Av1Usage,
    SvtAv1Options,
    Vp9Options,
};

// H.264: profile, level, B-frames, GOP, refs, preset, tune
let opts = VideoCodecOptions::H264(H264Options {
    profile: H264Profile::High,
    level: Some(41),       // 4.1 supports 1080p30
    bframes: 2,
    gop_size: 250,
    refs: 3,
    preset: Some(H264Preset::Fast),
    tune: None,
});

// H.265: profile (Main / Main10 for 10-bit HDR), preset
let opts = VideoCodecOptions::H265(H265Options {
    profile: H265Profile::Main10,
    preset: Some("fast".to_string()),
    ..H265Options::default()
});

// AV1 (libaom): cpu_used, tile layout, usage mode
let opts = VideoCodecOptions::Av1(Av1Options {
    cpu_used: 6,
    tile_rows: 1,   // 2^1 = 2 rows
    tile_cols: 1,   // 2^1 = 2 columns
    usage: Av1Usage::VoD,
});

// AV1 (SVT-AV1 / libsvtav1): preset 0-13, tiles, raw params
// Requires FFmpeg built with --enable-libsvtav1.
let opts = VideoCodecOptions::Av1Svt(SvtAv1Options {
    preset: 8,
    tile_rows: 1,
    tile_cols: 1,
    svtav1_params: None,
});

// VP9: cpu_used, constrained quality, row-based multithreading
let opts = VideoCodecOptions::Vp9(Vp9Options {
    cpu_used: 4,
    cq_level: Some(33),
    tile_columns: 1,
    tile_rows: 0,
    row_mt: true,
});

let mut encoder = VideoEncoder::create("output.mp4")
    .video(1920, 1080, 30.0)
    .video_codec(VideoCodec::H264)
    .bitrate_mode(BitrateMode::Crf(23))
    .codec_options(opts)
    .build()?;
```

## Per-Codec Audio Options

`AudioCodecOptions` provides typed configuration for Opus, AAC, MP3, and FLAC.

```rust
use ff_encode::{
    AudioEncoder, AudioCodec, AudioCodecOptions,
    OpusOptions, OpusApplication,
    AacOptions, AacProfile,
    Mp3Options, Mp3Quality,
    FlacOptions,
    OutputContainer,
};

// Opus: application mode + frame duration, OGG container
let opts = AudioCodecOptions::Opus(OpusOptions {
    application: OpusApplication::Audio,
    frame_duration_ms: Some(20),
});
let mut encoder = AudioEncoder::create("output.ogg")
    .audio(48_000, 2)
    .audio_codec(AudioCodec::Opus)
    .audio_bitrate(128_000)
    .container(OutputContainer::Ogg)
    .codec_options(opts)
    .build()?;

// AAC: profile (LC / HE / HEv2), optional VBR quality
let opts = AudioCodecOptions::Aac(AacOptions {
    profile: AacProfile::Lc,
    vbr_quality: None,  // None = CBR
});

// MP3: VBR quality scale 0 (best) to 9 (smallest)
let opts = AudioCodecOptions::Mp3(Mp3Options {
    quality: Mp3Quality::Vbr(2),  // ~190 kbps
});

// FLAC: compression level 0-12, native FLAC container
let opts = AudioCodecOptions::Flac(FlacOptions {
    compression_level: 6,
});
let mut encoder = AudioEncoder::create("output.flac")
    .audio(44_100, 2)
    .audio_codec(AudioCodec::Flac)
    .container(OutputContainer::Flac)
    .codec_options(opts)
    .build()?;
```

## Professional Formats

ProRes and DNxHD/DNxHR require specific pixel formats and FFmpeg encoder
support (`prores_ks` and `dnxhd` respectively).

```rust
use ff_encode::{
    VideoEncoder, VideoCodec, VideoCodecOptions,
    ProResOptions, ProResProfile,
    DnxhdOptions, DnxhdVariant,
};
use ff_format::PixelFormat;

// Apple ProRes HQ: yuv422p10le, .mov container
let opts = VideoCodecOptions::ProRes(ProResOptions {
    profile: ProResProfile::Hq,
    vendor: None,
});
let mut encoder = VideoEncoder::create("output.mov")
    .video(1920, 1080, 25.0)
    .video_codec(VideoCodec::ProRes)
    .pixel_format(PixelFormat::Yuv422p10le)
    .codec_options(opts)
    .build()?;

// Avid DNxHR SQ: yuv422p, any resolution
let opts = VideoCodecOptions::Dnxhd(DnxhdOptions {
    variant: DnxhdVariant::DnxhrSq,
});
let mut encoder = VideoEncoder::create("output.mxf")
    .video(1920, 1080, 25.0)
    .video_codec(VideoCodec::DnxHd)
    .codec_options(opts)
    .build()?;
```

| ProRes Profile | Pixel Format | Notes |
|---|---|---|
| `Proxy`, `Lt`, `Standard`, `Hq` | `yuv422p10le` | 422 chroma |
| `P4444`, `P4444Xq` | `yuva444p10le` | 444 chroma + alpha |

| DNxHD/HR Variant | Pixel Format | Notes |
|---|---|---|
| `Dnxhd115`, `Dnxhd145`, `Dnxhd220` | `yuv422p` | Fixed bitrate, 1080p/720p only |
| `Dnxhd220x`, `DnxhrHqx` | `yuv422p10le` | 10-bit |
| `DnxhrLb`, `DnxhrSq`, `DnxhrHq` | `yuv422p` | Any resolution |
| `DnxhrR444` | `yuv444p10le` | Any resolution, 4:4:4 10-bit |

## HDR Metadata

```rust
use ff_encode::{
    VideoEncoder, VideoCodec, VideoCodecOptions,
    H265Options, H265Profile, BitrateMode,
};
use ff_format::{
    Hdr10Metadata, MasteringDisplay,
    ColorTransfer, ColorSpace, ColorPrimaries,
    PixelFormat,
};

// HDR10: static metadata (MaxCLL, MaxFALL, mastering display)
// hdr10_metadata() automatically sets BT.2020 primaries, PQ transfer,
// and BT.2020 NCL colour matrix on the codec context.
let meta = Hdr10Metadata {
    max_cll: 1000,   // nits
    max_fall: 400,   // nits
    mastering_display: MasteringDisplay {
        red_x: 17000, red_y: 8500,
        green_x: 13250, green_y: 34500,
        blue_x: 7500, blue_y: 3000,
        white_x: 15635, white_y: 16450,
        min_luminance: 50,
        max_luminance: 10_000_000,
    },
};
let mut encoder = VideoEncoder::create("output.mkv")
    .video(3840, 2160, 24.0)
    .video_codec(VideoCodec::H265)
    .bitrate_mode(BitrateMode::Crf(22))
    .pixel_format(PixelFormat::Yuv420p10le)
    .codec_options(VideoCodecOptions::H265(H265Options {
        profile: H265Profile::Main10,
        ..H265Options::default()
    }))
    .hdr10_metadata(meta)
    .build()?;

// HLG: broadcast HDR without MaxCLL/MaxFALL side data
let mut encoder = VideoEncoder::create("output.mkv")
    .video(1920, 1080, 50.0)
    .video_codec(VideoCodec::H265)
    .pixel_format(PixelFormat::Yuv420p10le)
    .color_transfer(ColorTransfer::Hlg)
    .color_space(ColorSpace::Bt2020Ncl)
    .color_primaries(ColorPrimaries::Bt2020)
    .build()?;
```

## Hardware Encoding

```rust
use ff_encode::{VideoEncoder, VideoCodec, HardwareEncoder};

let mut encoder = VideoEncoder::create("output.mp4")
    .video(1920, 1080, 60.0)
    .video_codec(VideoCodec::H264)
    .hardware_encoder(HardwareEncoder::Auto)
    .build()?;
```

`HardwareEncoder::Auto` selects NVENC, QuickSync, AMF, or VideoToolbox based on what is available at runtime. If no hardware encoder is found, the builder falls back to a software encoder automatically.

## Progress Tracking

```rust
let mut encoder = VideoEncoder::create("output.mp4")
    .video(1920, 1080, 30.0)
    .video_codec(VideoCodec::H264)
    .bitrate_mode(BitrateMode::Cbr(4_000_000))
    .on_progress(|p| {
        println!(
            "{:.1}%, {} frames, {:.1}s elapsed",
            p.percent(),
            p.frames_encoded,
            p.elapsed.as_secs_f64()
        );
    })
    .build()?;
```

## LGPL Compliance

By default, `ff-encode` only links encoders compatible with the LGPL license: hardware encoders (NVENC, QSV, AMF, VideoToolbox) or software encoders for VP9 and AV1.

Enable the `gpl` feature to add libx264 and libx265. This changes the license terms of your binary; ensure you comply with the GPL or hold an appropriate MPEG LA commercial license before distributing.

## Error Handling

Common variants (not exhaustive):

| Variant                              | When it occurs                                     |
|--------------------------------------|----------------------------------------------------|
| `EncodeError::InvalidConfig`         | Codec, resolution, or bitrate settings are invalid |
| `EncodeError::UnsupportedCodec`      | Requested codec not available in this FFmpeg build |
| `EncodeError::HwEncoderUnavailable`  | Hardware encoder requested but no device found     |
| `EncodeError::Io`                    | Write error on the output file                     |
| `EncodeError::EncodingFailed`        | FFmpeg returned an error during frame encoding     |

## Feature Flags

| Flag | Description | Default |
|------|-------------|---------|
| `hwaccel` | Enables hardware encoder detection (NVENC, QSV, AMF, VideoToolbox, VA-API). | enabled |
| `gpl` | Enables GPL-licensed encoders (libx264, libx265). Requires GPL compliance or MPEG LA licensing. | disabled |
| `tokio` | Enables `AsyncVideoEncoder` and `AsyncAudioEncoder`. Each encoder runs a worker thread and exposes an async `push` / `finish` interface backed by a bounded `tokio::sync::mpsc` channel (capacity 8). Requires a tokio 1.x runtime. | disabled |

```toml
[dependencies]
ff-encode = { version = "0.16", features = ["tokio"] }
```

When the `tokio` feature is disabled, only the synchronous `VideoEncoder`, `AudioEncoder`, and `ImageEncoder` APIs are compiled. No tokio dependency is pulled in.

## MSRV

Rust 1.93.0 (edition 2024).

## License

MIT OR Apache-2.0
