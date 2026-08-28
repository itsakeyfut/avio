//! Decode video using hardware acceleration.
//!
//! Demonstrates:
//! - `VideoDecoderBuilder::hardware_accel()` — request a hardware decode backend
//! - `HardwareAccel` variants: `Auto`, `None`, `Nvdec`, `Qsv`, `Amf`,
//!   `VideoToolbox`, `Vaapi`
//! - `HardwareAccel::name()` — canonical name string
//! - `HardwareAccel::is_specific()` — whether a concrete backend was requested
//! - `SeekMode::Backward` — backward seek to the nearest keyframe
//!
//! Gracefully skips if the requested hardware backend is unavailable.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example hardware_decode -- \
//!   --input  input.mp4                                     \
//!   [--accel nvdec|qsv|amf|videotoolbox|vaapi|auto|none]
//! ```

use std::{path::Path, process, time::Duration};

use ff_decode::{HardwareAccel, SeekMode, VideoDecoder};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut accel_str = "auto".to_string();

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--accel" => accel_str = args.next().unwrap_or_else(|| "auto".to_string()),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: hardware_decode --input <file> \
             [--accel nvdec|qsv|amf|videotoolbox|vaapi|auto|none]"
        );
        process::exit(1);
    });

    // ── Parse HardwareAccel variant ───────────────────────────────────────────
    //
    // Each variant maps to a specific FFmpeg hardware decoder:
    //   Auto         — probe and use the best available backend
    //   None         — software decoding only
    //   Nvdec        — NVIDIA CUVID/NVDEC (CUDA)
    //   Qsv          — Intel Quick Sync Video
    //   Amf          — AMD Advanced Media Framework
    //   VideoToolbox — Apple VideoToolbox (macOS/iOS)
    //   Vaapi        — VA-API (Linux, Intel/AMD/Nouveau)

    let accel = match accel_str.to_lowercase().as_str() {
        "auto" => HardwareAccel::Auto,
        "none" => HardwareAccel::None,
        "nvdec" => HardwareAccel::Nvdec,
        "qsv" => HardwareAccel::Qsv,
        "amf" => HardwareAccel::Amf,
        "videotoolbox" => HardwareAccel::VideoToolbox,
        "vaapi" => HardwareAccel::Vaapi,
        other => {
            eprintln!(
                "Unknown accel '{other}' \
                 (try auto, none, nvdec, qsv, amf, videotoolbox, vaapi)"
            );
            process::exit(1);
        }
    };

    // name() returns the canonical lowercase string ("nvdec", "auto", …).
    // is_specific() is false for Auto and None, true for all hardware backends.
    println!(
        "Requested accel: {}  (is_specific={})",
        accel.name(),
        accel.is_specific()
    );

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);

    // ── Open decoder with hardware acceleration ───────────────────────────────
    //
    // hardware_accel() configures the decode backend. If the requested backend
    // is unavailable, build() returns a DecodeError — skip gracefully.

    let mut decoder = match VideoDecoder::open(&input).hardware_accel(accel).build() {
        Ok(d) => d,
        Err(e) => {
            println!(
                "Skipping: hardware backend '{}' unavailable — {e}",
                accel.name()
            );
            return;
        }
    };

    println!(
        "Input:   {in_name}  {}×{}  {:.2} fps  codec={}",
        decoder.width(),
        decoder.height(),
        decoder.frame_rate(),
        decoder.stream_info().codec_name(),
    );

    // ── SeekMode::Backward ────────────────────────────────────────────────────
    //
    // Backward seek moves to the nearest keyframe at or before the target.
    // It is the mirror of Keyframe (which seeks forward to the next keyframe)
    // and is useful when you want to ensure you don't overshoot a target time.

    let seek_to = Duration::from_secs(1);
    println!(
        "Seeking to {:.1}s with SeekMode::Backward ...",
        seek_to.as_secs_f64()
    );

    if let Err(e) = decoder.seek(seek_to, SeekMode::Backward) {
        println!("Seek not supported or failed: {e}");
        // Non-fatal — continue from start
        if let Err(e2) = VideoDecoder::open(&input)
            .hardware_accel(accel)
            .build()
            .map(|_| ())
        {
            eprintln!("Could not re-open: {e2}");
            process::exit(1);
        }
    }

    // ── Decode a few frames ───────────────────────────────────────────────────

    let mut count = 0u32;
    let limit = 30;

    loop {
        match decoder.decode_one() {
            Ok(Some(frame)) => {
                if count == 0 {
                    println!(
                        "First frame: {}×{}  format={}  pts={:.3}s",
                        frame.width(),
                        frame.height(),
                        frame.format(),
                        frame.timestamp().as_secs_f64(),
                    );
                }
                count += 1;
                if count >= limit {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("Decode error: {e}");
                process::exit(1);
            }
        }
    }

    println!("Decoded {count} frames using accel='{}'", accel.name());
}
