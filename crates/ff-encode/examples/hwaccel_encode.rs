//! Encode video with hardware acceleration via `HardwareEncoder`.
//!
//! Demonstrates the `hwaccel` feature (enabled by default in `avio`):
//!
//! - `HardwareEncoder::available()` — probe which hardware backends are present
//!   on the current system (`NVENC`, `QSV`, `AMF`, `VideoToolbox`, `VA-API`)
//! - `HardwareEncoder::is_available()` — check a single backend explicitly
//! - `.hardware_encoder()` on `VideoEncoderBuilder` — request a specific backend
//! - `VideoEncoder::actual_video_codec()` — see the exact encoder `FFmpeg` selected
//! - `VideoEncoder::is_lgpl_compliant()` — confirm the encoder's license status
//!
//! Auto-detection priority (when `--hw auto`):
//!   `NVENC` → `QSV` → `AMF` → `VideoToolbox` → `VA-API` → software fallback
//!
//! The encoder falls back to software automatically when the requested backend
//! is unavailable. This means `build()` never fails due to missing hardware —
//! use `actual_video_codec()` after `build()` to confirm what was selected.
//!
//! # Disabling hardware acceleration
//!
//! Compile without the `hwaccel` feature to strip all hardware encoder candidates
//! from the build (hardware encoder symbols are excluded at compile time):
//!
//! ```bash
//! cargo run --example hwaccel_encode --no-default-features \
//!   -- --input input.mp4 --output output.mp4
//! ```
//!
//! # Usage
//!
//! ```bash
//! cargo run --example hwaccel_encode -- \
//!   --input  input.mp4   \
//!   --output output.mp4  \
//!   [--hw    auto|nvenc|qsv|amf|videotoolbox|vaapi|none]  \
//!   [--codec h264|h265]  \
//!   [--crf   23]
//! ```

use std::{path::Path, process};

use ff_decode::VideoDecoder;
use ff_encode::{BitrateMode, HardwareEncoder, VideoCodecEncodeExt, VideoEncoder};
use ff_format::{AudioCodec, VideoCodec};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut hw_str = "auto".to_string();
    let mut codec_str = "h264".to_string();
    let mut crf: u32 = 23;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--hw" => hw_str = args.next().unwrap_or_else(|| "auto".to_string()),
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
            "Usage: hwaccel_encode --input <file> --output <file> \
             [--hw auto|nvenc|qsv|amf|videotoolbox|vaapi|none] \
             [--codec h264|h265] [--crf N]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // HardwareEncoder::available()
    //
    // Probes FFmpeg for all hardware encoder backends present on this system.
    // Auto and None are control variants, not hardware backends — they are
    // excluded from this list. The result is cached after the first call.

    let available = HardwareEncoder::available();
    if available.is_empty() {
        println!("Hardware encoders: none detected on this system");
    } else {
        let names: Vec<&str> = available
            .iter()
            .map(|hw| match hw {
                HardwareEncoder::Nvenc => "nvenc",
                HardwareEncoder::Qsv => "qsv",
                HardwareEncoder::Amf => "amf",
                HardwareEncoder::VideoToolbox => "videotoolbox",
                HardwareEncoder::Vaapi => "vaapi",
                _ => "unknown",
            })
            .collect();
        println!("Hardware encoders detected: {}", names.join(", "));
    }

    // HardwareEncoder::is_available()
    //
    // Query individual backends. Useful when targeting a specific platform.

    println!(
        "NVENC available: {}  QSV: {}  AMF: {}  VideoToolbox: {}  VA-API: {}",
        HardwareEncoder::Nvenc.is_available(),
        HardwareEncoder::Qsv.is_available(),
        HardwareEncoder::Amf.is_available(),
        HardwareEncoder::VideoToolbox.is_available(),
        HardwareEncoder::Vaapi.is_available(),
    );
    println!();

    // Parse --hw and --codec flags

    let hw = match hw_str.to_lowercase().as_str() {
        "auto" => HardwareEncoder::Auto,
        "none" | "software" => HardwareEncoder::None,
        "nvenc" => HardwareEncoder::Nvenc,
        "qsv" => HardwareEncoder::Qsv,
        "amf" => HardwareEncoder::Amf,
        "videotoolbox" => HardwareEncoder::VideoToolbox,
        "vaapi" => HardwareEncoder::Vaapi,
        other => {
            eprintln!(
                "Unknown hw backend '{other}' (try auto, nvenc, qsv, amf, videotoolbox, vaapi, none)"
            );
            process::exit(1);
        }
    };

    let video_codec = match codec_str.to_lowercase().as_str() {
        "h264" | "avc" => VideoCodec::H264,
        "h265" | "hevc" => VideoCodec::H265,
        other => {
            eprintln!("Unknown codec '{other}' (try h264, h265)");
            process::exit(1);
        }
    };

    // VideoCodecEncodeExt::is_lgpl_compatible()
    //
    // Static check: is this codec's *typical* software encoder LGPL-compatible?
    // H.264 and H.265 return false — their software encoders (libx264/libx265)
    // are GPL. Hardware encoders are always LGPL-compatible regardless.

    println!(
        "Codec family: {:?}  is_lgpl_compatible (static) = {}",
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
    println!("Output: {out_name}  codec={video_codec:?}  hw={hw_str}  crf={crf}");
    println!();

    // Build encoder with hardware acceleration
    //
    // .hardware_encoder() sets the preferred backend. FFmpeg tries the hardware
    // encoder first; if unavailable it falls back to the next candidate and
    // ultimately to a software encoder. build() never fails due to missing
    // hardware — inspect actual_video_codec() afterward to confirm the choice.

    let mut encoder = match VideoEncoder::create(&output)
        .video(width, height, fps)
        .video_codec(video_codec)
        .audio_codec(AudioCodec::Aac)
        .bitrate_mode(BitrateMode::Crf(crf))
        .hardware_encoder(hw)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building encoder: {e}");
            process::exit(1);
        }
    };

    // Inspect the selected encoder
    //
    // actual_video_codec() returns the FFmpeg encoder name that was opened,
    // e.g. "h264_nvenc", "h264_qsv", "libvpx-vp9" (LGPL fallback), etc.
    //
    // is_lgpl_compliant() checks the runtime encoder name: hardware encoders
    // and LGPL software encoders (VP9, AV1) return true; libx264/libx265 return false.

    let actual = encoder.actual_video_codec().to_string();
    let lgpl_ok = encoder.is_lgpl_compliant();

    let hw_label = if actual.contains("nvenc") {
        "NVENC (hardware)"
    } else if actual.contains("qsv") {
        "Quick Sync (hardware)"
    } else if actual.contains("amf") {
        "AMF (hardware)"
    } else if actual.contains("videotoolbox") {
        "VideoToolbox (hardware)"
    } else if actual.contains("vaapi") {
        "VA-API (hardware)"
    } else {
        "software"
    };

    println!("Encoder selected: {actual}  ({hw_label})");
    println!("is_lgpl_compliant (runtime) = {lgpl_ok}");
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
