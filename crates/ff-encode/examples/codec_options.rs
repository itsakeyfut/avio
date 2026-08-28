//! Encode video with per-codec options via `VideoCodecOptions`.
//!
//! Demonstrates the `VideoCodecOptions` enum and the codec-specific option
//! structs added in v0.7.0:
//!
//! - `H264Options` — profile (`baseline` / `main` / `high` / `high10`), level,
//!   B-frames, GOP, refs, and libx264 `preset` / `tune`
//! - `H265Options` — profile (`main` / `main10`), tier, libx265 `preset`
//! - `Av1Options` — `cpu_used` (0 = best, 8 = fastest), tile layout, usage
//! - `SvtAv1Options` — SVT-AV1 (`libsvtav1`) preset (0–13), tile layout,
//!   raw `svtav1_params` string; skips gracefully when libsvtav1 is absent
//! - `Vp9Options` — `cpu_used`, constrained-quality (`cq_level`), tile layout
//!
//! All options are applied via `av_opt_set` / direct field assignment before
//! `avcodec_open2`.  Unsupported options are logged as warnings and skipped —
//! `build()` never fails because of an unsupported option value.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example codec_options -- \
//!   --input   input.mp4   \
//!   --output  output.mp4  \
//!   --codec   h264        # h264 | h265 | av1 | svt-av1 | vp9  (default: h264)
//! ```

use std::{path::Path, process};

use ff_decode::VideoDecoder;
use ff_encode::{
    Av1Options, Av1Usage, BitrateMode, H264Options, H264Preset, H264Profile, H265Options,
    H265Profile, SvtAv1Options, VideoCodecOptions, VideoEncoder, Vp9Options,
};
use ff_format::{PixelFormat, VideoCodec};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut codec_str = "h264".to_string();

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--codec" | "-c" => codec_str = args.next().unwrap_or_else(|| "h264".to_string()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: codec_options --input <file> --output <file> \
             [--codec h264|h265|av1|svt-av1|vp9]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // ── Probe source ──────────────────────────────────────────────────────────

    let probe = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening input: {e}");
            process::exit(1);
        }
    };
    let width = probe.width();
    let height = probe.height();
    let fps = probe.frame_rate();
    drop(probe);

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    println!("Input:  {in_name}  {width}×{height}  {fps:.2} fps");

    // ── Select codec + VideoCodecOptions ─────────────────────────────────────
    //
    // VideoCodecOptions is the main v0.7.0 API for per-codec configuration.
    // Each variant holds a typed options struct specific to that codec.

    let (video_codec, codec_options, pixel_format, description) =
        match codec_str.to_lowercase().as_str() {
            "h264" => {
                // H264Options: profile, level, B-frames, GOP size, reference frames,
                // libx264 preset and tune.
                let opts = H264Options {
                    profile: H264Profile::High,
                    level: Some(41), // 4.1 — supports 1080p30
                    bframes: 2,
                    gop_size: 250,
                    refs: 3,
                    preset: Some(H264Preset::Fast),
                    tune: None,
                };
                (
                    VideoCodec::H264,
                    VideoCodecOptions::H264(opts),
                    None,
                    "H.264 High@4.1, fast preset, 2 B-frames",
                )
            }
            "h265" => {
                // H265Options: profile (Main or Main10), tier, libx265 preset.
                // Main10 with yuv420p10le enables 10-bit HDR-capable output.
                let opts = H265Options {
                    profile: H265Profile::Main10,
                    preset: Some("fast".to_string()),
                    ..H265Options::default()
                };
                (
                    VideoCodec::H265,
                    VideoCodecOptions::H265(opts),
                    Some(PixelFormat::Yuv420p10le),
                    "H.265 Main10 (10-bit), fast preset",
                )
            }
            "av1" => {
                // Av1Options: cpu_used (0=slowest/best, 8=fastest/lowest),
                // tile layout for parallelism, and usage mode.
                let opts = Av1Options {
                    cpu_used: 6,  // balanced speed/quality
                    tile_rows: 1, // 2^1 = 2 tile rows
                    tile_cols: 1, // 2^1 = 2 tile columns
                    usage: Av1Usage::VoD,
                };
                (
                    VideoCodec::Av1,
                    VideoCodecOptions::Av1(opts),
                    None,
                    "AV1 (libaom), cpu_used=6, 2×2 tiles, VoD mode",
                )
            }
            "svt-av1" => {
                // SvtAv1Options: preset controls the speed/quality tradeoff.
                //   0  = best quality / slowest encode
                //   13 = fastest encode / lowest quality
                //
                // tile_rows / tile_cols are log2 counts (0 = 1 tile, 1 = 2, 2 = 4, …).
                //
                // svtav1_params allows passing raw key=value pairs from the
                // libsvtav1 parameter interface (e.g. "fast-decode=1").
                //
                // Requires an FFmpeg build with --enable-libsvtav1.
                // build() returns EncodeError::EncoderUnavailable if absent.
                let opts = SvtAv1Options {
                    preset: 8, // balanced speed/quality
                    tile_rows: 1,
                    tile_cols: 1,
                    svtav1_params: None,
                };
                (
                    VideoCodec::Av1Svt,
                    VideoCodecOptions::Av1Svt(opts),
                    None,
                    "AV1 (SVT-AV1 / libsvtav1), preset=8, 2×2 tiles",
                )
            }
            "vp9" => {
                // Vp9Options: cpu_used, constrained-quality mode (cq_level),
                // and tile configuration.
                let opts = Vp9Options {
                    cpu_used: 4,
                    cq_level: Some(33), // CQ mode: quality-governed VBR
                    tile_columns: 1,
                    tile_rows: 0,
                    row_mt: true,
                };
                (
                    VideoCodec::Vp9,
                    VideoCodecOptions::Vp9(opts),
                    None,
                    "VP9 (libvpx-vp9), cpu_used=4, CQ=33, row-MT enabled",
                )
            }
            other => {
                eprintln!("Unknown codec '{other}' (try h264, h265, av1, svt-av1, vp9)");
                process::exit(1);
            }
        };

    println!("Codec:  {description}");
    println!("Output: {out_name}");
    println!();

    // ── Build encoder ─────────────────────────────────────────────────────────
    //
    // .codec_options() applies the per-codec option struct.
    // .pixel_format() overrides the default pixel format when needed (e.g. 10-bit).

    let mut enc_builder = VideoEncoder::create(&output)
        .video(width, height, fps)
        .video_codec(video_codec)
        .bitrate_mode(BitrateMode::Crf(26))
        .codec_options(codec_options);

    if let Some(fmt) = pixel_format {
        enc_builder = enc_builder.pixel_format(fmt);
    }

    let mut encoder = match enc_builder.build() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    println!(
        "Encoder opened: actual_codec={}",
        encoder.actual_video_codec()
    );

    // ── Decode + encode loop ──────────────────────────────────────────────────

    let mut decoder = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening decoder: {e}");
            process::exit(1);
        }
    };

    let mut frames: u64 = 0;

    loop {
        let frame = match decoder.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("Decode error: {e}");
                process::exit(1);
            }
        };

        if let Err(e) = encoder.push_video(&frame) {
            eprintln!("Encode error: {e}");
            process::exit(1);
        }

        frames += 1;
    }

    if let Err(e) = encoder.finish() {
        eprintln!("Error finalising output: {e}");
        process::exit(1);
    }

    #[allow(clippy::cast_precision_loss)]
    let kb = std::fs::metadata(&output).map_or(0, |m| m.len()) as f64 / 1024.0;
    let size_str = if kb < 1024.0 {
        format!("{kb:.0} KB")
    } else {
        format!("{:.1} MB", kb / 1024.0)
    };

    println!("Done. {out_name}  {size_str}  {frames} frames");
}
