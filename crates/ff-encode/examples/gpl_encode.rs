//! Encode H.264 / H.265 with libx264 / libx265 via the `gpl` feature.
//!
//! Demonstrates how the `gpl` feature changes encoder selection and what
//! the LGPL/GPL distinction means at runtime:
//!
//! - **Without `gpl`** (default): H.264 falls back to VP9 (`libvpx-vp9`);
//!   H.265 falls back to AV1 (`libaom-av1`).  Both are LGPL-compatible.
//! - **With `gpl`**: H.264 uses `libx264`; H.265 uses `libx265`.
//!   Both are GPL-licensed — distributing a binary that links them requires
//!   GPL compliance or a commercial MPEG LA licence.
//!
//! Key APIs shown:
//!
//! - `VideoCodecEncodeExt::is_lgpl_compatible()` — static check on the codec
//!   *family*.  H.264 and H.265 return `false` because their canonical software
//!   encoders (libx264/libx265) are GPL, regardless of which encoder is actually
//!   chosen at runtime.
//! - `VideoEncoder::actual_video_codec()` — the exact `FFmpeg` encoder name that
//!   was opened (`"libx264"`, `"libvpx-vp9"`, `"libx265"`, `"libaom-av1"`, …).
//! - `VideoEncoder::is_lgpl_compliant()` — runtime check: `true` when a hardware
//!   or LGPL software encoder is active; `false` when libx264/libx265 is used.
//!
//! # Usage
//!
//! ```bash
//! # Default build — H.264 falls back to VP9 (LGPL)
//! cargo run --example gpl_encode -- \
//!   --input input.mp4 --output output.mp4 --codec h264
//!
//! # GPL build — uses libx264 (GPL)
//! cargo run --example gpl_encode --features gpl -- \
//!   --input input.mp4 --output output.mp4 --codec h264
//! ```

use std::{path::Path, process};

use ff_decode::VideoDecoder;
use ff_encode::{
    BitrateMode, H264Options, H264Profile, H265Options, H265Profile, HardwareEncoder,
    VideoCodecEncodeExt, VideoCodecOptions, VideoEncoder,
};
use ff_format::VideoCodec;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut codec_str = "h264".to_string();
    let mut crf: u32 = 23;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--codec" | "-c" => codec_str = args.next().unwrap_or_else(|| "h264".to_string()),
            "--crf" => {
                let v = args.next().unwrap_or_default();
                crf = v.parse().unwrap_or(23);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: gpl_encode --input <file> --output <file> \
             [--codec h264|h265] [--crf N]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // gpl feature status
    //
    // Report at startup whether the gpl feature is compiled in.
    // This is a compile-time constant; the selected encoder will confirm it.

    #[cfg(feature = "gpl")]
    println!("gpl feature: ENABLED — libx264 / libx265 are candidates");
    #[cfg(not(feature = "gpl"))]
    println!(
        "gpl feature: DISABLED — H.264 falls back to VP9 (LGPL); \
         H.265 falls back to AV1 (LGPL)"
    );
    println!();

    // Parse --codec

    let (video_codec, codec_options, description) = match codec_str.to_lowercase().as_str() {
        "h264" | "avc" => {
            let opts = VideoCodecOptions::H264(H264Options {
                profile: H264Profile::High,
                level: Some(41),
                bframes: 2,
                gop_size: 250,
                refs: 3,
                preset: None, // libx264: "medium"; hardware: ignored
                tune: None,
            });
            (VideoCodec::H264, opts, "H.264 High@4.1")
        }
        "h265" | "hevc" => {
            let opts = VideoCodecOptions::H265(H265Options {
                profile: H265Profile::Main,
                preset: None, // libx265: "medium"; hardware: ignored
                ..H265Options::default()
            });
            (VideoCodec::H265, opts, "H.265 Main")
        }
        other => {
            eprintln!("Unknown codec '{other}' (try h264, h265)");
            process::exit(1);
        }
    };

    // VideoCodecEncodeExt::is_lgpl_compatible()
    //
    // Static check on the codec *family*, not the runtime encoder.
    // H.264 and H.265 both return false because their canonical software
    // encoders (libx264 / libx265) are GPL, even though hardware encoders
    // for these codecs are LGPL-compatible.
    //
    // Use is_lgpl_compliant() on the built encoder for the runtime answer.

    println!(
        "Codec: {:?}  is_lgpl_compatible (static, codec family) = {}",
        video_codec,
        video_codec.is_lgpl_compatible(),
    );

    // Probe input

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
    println!("Output: {out_name}  {description}  crf={crf}");
    println!();

    // Build encoder
    //
    // HardwareEncoder::None forces software-only encoding so the GPL/LGPL
    // fallback logic is clearly visible. With hardware acceleration enabled
    // (HardwareEncoder::Auto), a hardware encoder would be preferred regardless
    // of the gpl feature flag.
    //
    // Encoder candidate priority (software only):
    //   With gpl:    H.264 → libx264 (GPL)   |  H.265 → libx265 (GPL)
    //   Without gpl: H.264 → libvpx-vp9 (LGPL) | H.265 → libaom-av1 (LGPL)

    let mut encoder = match VideoEncoder::create(&output)
        .video(width, height, fps)
        .video_codec(video_codec)
        .bitrate_mode(BitrateMode::Crf(crf))
        .codec_options(codec_options)
        .hardware_encoder(HardwareEncoder::None)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    // Inspect actual encoder chosen
    //
    // actual_video_codec() returns the FFmpeg encoder name that was opened.
    //
    // is_lgpl_compliant() checks the encoder name at runtime:
    //   "libx264" / "libx265"  → false (GPL)
    //   "libvpx-vp9" / "libaom-av1" / hardware variants → true (LGPL)

    let actual = encoder.actual_video_codec().to_string();
    let lgpl_ok = encoder.is_lgpl_compliant();

    println!("Encoder selected:         {actual}");
    println!("is_lgpl_compliant (runtime) = {lgpl_ok}");

    if lgpl_ok {
        println!("  → LGPL-compatible: safe to distribute under LGPL terms");
    } else {
        println!(
            "  → GPL encoder: distributing this binary requires GPL compliance \
             or a commercial MPEG LA licence"
        );
    }
    println!();

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

    println!("Done. {out_name}  {size_str}  {frames} frames  encoder={actual}");
}
