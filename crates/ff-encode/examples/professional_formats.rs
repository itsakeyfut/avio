//! Encode video to Apple `ProRes` or Avid `DNxHD`/`DNxHR` professional formats.
//!
//! Demonstrates:
//! - `ProResOptions` + `ProResProfile` — 422 Proxy / LT / Standard / HQ / 4444 / 4444 XQ
//! - `DnxhdOptions` + `DnxhdVariant` — legacy `DNxHD` (fixed bitrate, 1920×1080) and
//!   resolution-agnostic `DNxHR` (LB / SQ / HQ / HQX / R444)
//! - Pixel format requirements:
//!   - `ProRes` 422 profiles → `yuv422p10le`
//!   - `ProRes` 4444 profiles → `yuva444p10le`
//!   - `DNxHD` / `DNxHR` 8-bit profiles → `yuv422p`
//!   - `DNxHD` 220x / `DNxHR` HQX → `yuv422p10le`
//!
//! Both codecs skip gracefully when the required encoder (`prores_ks` or
//! `dnxhd`) is absent from the `FFmpeg` build.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example professional_formats -- \
//!   --input   input.mp4       \
//!   --output  output.mov      \
//!   --codec   prores          # prores | dnxhd  (default: prores)
//!   [--profile hq]            # ProRes: proxy|lt|standard|hq|4444|4444xq
//!                             # DNxHD:  dnxhd115|dnxhd145|dnxhd220|dnxhr_sq|dnxhr_hq|...
//! ```

use std::{path::Path, process};

use ff_decode::VideoDecoder;
use ff_encode::{
    DnxhdOptions, DnxhdVariant, ProResOptions, ProResProfile, VideoCodecOptions, VideoEncoder,
};
use ff_format::VideoCodec;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut codec_str = "prores".to_string();
    let mut profile_str = "hq".to_string();

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--codec" | "-c" => codec_str = args.next().unwrap_or_else(|| "prores".to_string()),
            "--profile" => profile_str = args.next().unwrap_or_else(|| "hq".to_string()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: professional_formats --input <file> --output <file> \
             [--codec prores|dnxhd] [--profile <profile>]\n\
             ProRes profiles: proxy | lt | standard | hq | 4444 | 4444xq\n\
             DNxHD profiles:  dnxhd115 | dnxhd145 | dnxhd220 | dnxhd220x |\n\
                              dnxhr_lb | dnxhr_sq | dnxhr_hq | dnxhr_hqx | dnxhr_444"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // Probe source

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

    // Select codec + VideoCodecOptions

    let (video_codec, codec_options, description) = match codec_str.to_lowercase().as_str() {
        "prores" => {
            // ProResProfile controls quality and chroma sampling:
            //   Proxy    (0) — lowest bitrate, offline editing
            //   Lt       (1) — lightweight proxy
            //   Standard (2) — production quality (default)
            //   Hq       (3) — high quality, mastering
            //   P4444    (4) — full chroma, supports alpha
            //   P4444Xq  (5) — maximum quality 4444
            //
            // 422 profiles store yuv422p10le; 4444 profiles store yuva444p10le.
            let profile = match profile_str.to_lowercase().as_str() {
                "proxy" => ProResProfile::Proxy,
                "lt" => ProResProfile::Lt,
                "standard" => ProResProfile::Standard,
                "hq" => ProResProfile::Hq,
                "4444" => ProResProfile::P4444,
                "4444xq" => ProResProfile::P4444Xq,
                other => {
                    eprintln!(
                        "Unknown ProRes profile '{other}' \
                         (try proxy, lt, standard, hq, 4444, 4444xq)"
                    );
                    process::exit(1);
                }
            };
            let desc = format!("Apple ProRes {profile:?}");
            let opts = ProResOptions {
                profile,
                vendor: None,
            };
            (VideoCodec::ProRes, VideoCodecOptions::ProRes(opts), desc)
        }

        "dnxhd" => {
            // DnxhdVariant: legacy DNxHD variants (Dnxhd*) require 1920×1080 or
            // 1280×720 and apply a fixed bitrate automatically.
            // DNxHR variants (Dnxhr*) work at any resolution.
            let variant = match profile_str.to_lowercase().as_str() {
                "dnxhd115" => DnxhdVariant::Dnxhd115,
                "dnxhd145" => DnxhdVariant::Dnxhd145,
                "dnxhd220" => DnxhdVariant::Dnxhd220,
                "dnxhd220x" => DnxhdVariant::Dnxhd220x,
                "dnxhr_lb" => DnxhdVariant::DnxhrLb,
                "dnxhr_sq" | "sq" => DnxhdVariant::DnxhrSq,
                "dnxhr_hq" | "hq" => DnxhdVariant::DnxhrHq,
                "dnxhr_hqx" | "hqx" => DnxhdVariant::DnxhrHqx,
                "dnxhr_444" | "444" => DnxhdVariant::DnxhrR444,
                other => {
                    eprintln!(
                        "Unknown DNxHD profile '{other}' \
                         (try dnxhd115, dnxhd145, dnxhd220, dnxhr_sq, dnxhr_hq, …)"
                    );
                    process::exit(1);
                }
            };
            let desc = format!("Avid DNxHD/HR {variant:?}");
            (
                VideoCodec::DnxHd,
                VideoCodecOptions::Dnxhd(DnxhdOptions { variant }),
                desc,
            )
        }

        other => {
            eprintln!("Unknown codec '{other}' (try prores, dnxhd)");
            process::exit(1);
        }
    };

    println!("Codec:  {description}");
    println!("Output: {out_name}");
    println!();

    // Build encoder

    let mut encoder = match VideoEncoder::create(&output)
        .video(width, height, fps)
        .video_codec(video_codec)
        .codec_options(codec_options)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            eprintln!(
                "Note: ProRes requires 'prores_ks'; DNxHD requires 'dnxhd' in your FFmpeg build."
            );
            process::exit(1);
        }
    };

    println!(
        "Encoder opened: actual_codec={}",
        encoder.actual_video_codec()
    );
    println!("Encoding...");

    // Decode + encode loop

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
