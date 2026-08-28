//! Embed HDR10 static metadata and HLG colour transfer tags when encoding.
//!
//! Demonstrates the HDR and colour-tagging APIs added in v0.7.0:
//!
//! - `Hdr10Metadata` — `MaxCLL` + `MaxFALL` + mastering display (SMPTE ST 2086)
//! - `MasteringDisplay` — chromaticity coordinates (×50000) + luminance (×10000)
//! - `VideoEncoderBuilder::hdr10_metadata()` — attach HDR10 side data to key-
//!   frame packets (`AV_PKT_DATA_CONTENT_LIGHT_LEVEL` +
//!   `AV_PKT_DATA_MASTERING_DISPLAY_METADATA`)
//! - `VideoEncoderBuilder::color_transfer()` — override the OETF tag
//!   (`ColorTransfer::Pq` for HDR10, `ColorTransfer::Hlg` for HLG broadcast)
//! - `VideoEncoderBuilder::color_space()` / `color_primaries()` — BT.2020 tags
//!
//! Setting `hdr10_metadata()` automatically applies BT.2020 primaries, PQ
//! transfer, and BT.2020 NCL colour matrix to the codec context.
//! Use `color_transfer(ColorTransfer::Hlg)` instead to tag HLG content without
//! MaxCLL/MaxFALL side data.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example hdr10_encode -- \
//!   --input    input.mp4     \
//!   --output   output.mkv    \
//!   [--max-cll  1000]        # MaxCLL in nits  (default: 1000)
//!   [--max-fall 400]         # MaxFALL in nits (default: 400)
//!   [--hlg]                  # tag HLG transfer instead of HDR10
//! ```

use std::{path::Path, process};

use ff_decode::VideoDecoder;
use ff_encode::{BitrateMode, H265Options, H265Profile, Preset, VideoCodecOptions, VideoEncoder};
use ff_format::{
    ColorPrimaries, ColorSpace, ColorTransfer, Hdr10Metadata, MasteringDisplay, PixelFormat,
    VideoCodec,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut max_cll: u16 = 1000;
    let mut max_fall: u16 = 400;
    let mut hlg = false;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--max-cll" => {
                let v = args.next().unwrap_or_default();
                max_cll = v.parse().unwrap_or(1000);
            }
            "--max-fall" => {
                let v = args.next().unwrap_or_default();
                max_fall = v.parse().unwrap_or(400);
            }
            "--hlg" => hlg = true,
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: hdr10_encode --input <file> --output <file> \
             [--max-cll N] [--max-fall N] [--hlg]"
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

    // ── Build encoder with HDR metadata ──────────────────────────────────────

    let mut enc_builder = VideoEncoder::create(&output)
        .video(width, height, fps)
        .video_codec(VideoCodec::H265)
        .bitrate_mode(BitrateMode::Crf(22))
        .preset(Preset::Fast)
        // yuv420p10le is the canonical 10-bit pixel format for HDR content.
        .pixel_format(PixelFormat::Yuv420p10le)
        .codec_options(VideoCodecOptions::H265(H265Options {
            profile: H265Profile::Main10,
            ..H265Options::default()
        }));

    if hlg {
        // ── HLG (Hybrid Log-Gamma) colour tags ───────────────────────────────
        //
        // HLG is a broadcast-compatible HDR standard (ARIB STD-B67 / BT.2100).
        // It does not use MaxCLL/MaxFALL side data — instead the OETF tag on
        // the stream signals the display how to render the content.
        //
        // .color_transfer()  sets the OETF (ColorTransfer::Hlg).
        // .color_space()     sets the matrix coefficients (BT.2020 NCL).
        // .color_primaries() sets the chromaticity primaries (BT.2020).
        println!("Mode:   HLG colour tags (no MaxCLL/MaxFALL side data)");
        enc_builder = enc_builder
            .color_transfer(ColorTransfer::Hlg)
            .color_space(ColorSpace::Bt2020Ncl)
            .color_primaries(ColorPrimaries::Bt2020);
    } else {
        // ── HDR10 static metadata ─────────────────────────────────────────────
        //
        // Hdr10Metadata combines:
        //   max_cll   — Maximum Content Light Level (nits)
        //   max_fall  — Maximum Frame-Average Light Level (nits)
        //   mastering_display — SMPTE ST 2086 mastering display colour volume
        //
        // MasteringDisplay chromaticity coordinates use a denominator of 50000
        // (value = n / 50000 in CIE 1931 xy).  The BT.2020 primaries below are
        // the standard values for reference HDR monitors.
        //
        // Luminance uses a denominator of 10000 nits:
        //   min_luminance = 50   → 0.005 nit (deep black)
        //   max_luminance = 10_000_000 → 1000 nit peak
        //
        // hdr10_metadata() also automatically sets:
        //   color_primaries = BT.2020
        //   color_trc       = SMPTE ST 2084 (PQ)
        //   colorspace      = BT.2020 NCL
        println!("Mode:   HDR10  MaxCLL={max_cll} nits  MaxFALL={max_fall} nits");
        let meta = Hdr10Metadata {
            max_cll,
            max_fall,
            mastering_display: MasteringDisplay {
                // BT.2020 D65 primaries (×50000)
                red_x: 17000,
                red_y: 8500,
                green_x: 13250,
                green_y: 34500,
                blue_x: 7500,
                blue_y: 3000,
                // D65 white point (×50000)
                white_x: 15635,
                white_y: 16450,
                // Luminance (×10000 nits)
                min_luminance: 50,
                max_luminance: 10_000_000,
            },
        };
        enc_builder = enc_builder.hdr10_metadata(meta);
    }

    println!("Codec:  H.265 Main10, yuv420p10le");
    println!("Output: {out_name}");
    println!();

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
    println!("Encoding...");

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
    if !hlg {
        println!(
            "Verify with: ffprobe -show_streams {out_name} | grep -E 'max_content|max_average'"
        );
    }
}
