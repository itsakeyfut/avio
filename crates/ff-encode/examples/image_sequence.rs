//! Decode an image sequence to video, or extract video frames to an image sequence.
//!
//! Demonstrates the image-sequence support added in v0.7.0:
//!
//! **Decode mode** (`--decode`): a printf-style pattern such as
//! `frames/frame%04d.png` is opened with the `image2` demuxer and decoded
//! frame-by-frame.  An optional `--fps` override sets the frame rate assigned
//! to each image (default: 25).
//!
//! **Encode mode** (`--encode`): a printf-style output pattern such as
//! `output/frame%04d.png` selects the `image2` muxer automatically.  Each
//! video frame is written as a separate file.  Supported extensions:
//! `.png`, `.jpg` / `.jpeg`.
//!
//! The `%` character in the path is what triggers the image-sequence path in
//! `VideoDecoder` / `VideoEncoder`.
//!
//! # Usage
//!
//! ```bash
//! # Assemble PNGs into a video
//! cargo run --example image_sequence -- \
//!   --decode "frames/frame%04d.png" \
//!   --output output.mp4 \
//!   [--fps 25]
//!
//! # Extract video frames to PNGs
//! cargo run --example image_sequence -- \
//!   --encode input.mp4 \
//!   --output "frames/frame%04d.png"
//! ```

use std::{path::Path, process};

use ff_decode::{HardwareAccel, VideoDecoder};
use ff_encode::VideoEncoder;
use ff_format::VideoCodec;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut decode_pattern = None::<String>;
    let mut encode_input = None::<String>;
    let mut output = None::<String>;
    let mut fps: u32 = 25;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--decode" => decode_pattern = Some(args.next().unwrap_or_default()),
            "--encode" => encode_input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--fps" => {
                let v = args.next().unwrap_or_default();
                fps = v.parse().unwrap_or(25);
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let output = output.unwrap_or_else(|| {
        eprintln!(
            "Usage:\n\
             Decode: image_sequence --decode \"frame%04d.png\" --output video.mp4 [--fps 25]\n\
             Encode: image_sequence --encode input.mp4 --output \"frame%04d.png\""
        );
        process::exit(1);
    });

    match (decode_pattern, encode_input) {
        (Some(pattern), None) => decode_sequence(&pattern, &output, fps),
        (None, Some(input)) => encode_sequence(&input, &output),
        _ => {
            eprintln!("Provide exactly one of --decode <pattern> or --encode <input>");
            process::exit(1);
        }
    }
}

// ── Decode mode: image sequence → video ──────────────────────────────────────

fn decode_sequence(pattern: &str, output: &str, fps: u32) {
    let out_name = Path::new(output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(output);

    println!("Decode: image sequence pattern='{pattern}'  fps={fps}");
    println!("Output: {out_name}");
    println!();

    // VideoDecoder::open() detects the `%` in the path and uses the `image2`
    // demuxer automatically.  .frame_rate() overrides the default 25 fps.
    //
    // .hardware_accel(HardwareAccel::None) avoids hardware decoder selection
    // for still-image formats, which is rarely supported.
    let mut decoder = match VideoDecoder::open(pattern)
        .hardware_accel(HardwareAccel::None)
        .frame_rate(fps)
        .build()
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error opening image sequence: {e}");
            process::exit(1);
        }
    };

    let width = decoder.width();
    let height = decoder.height();
    let actual_fps = decoder.frame_rate();

    println!("Sequence: {width}×{height}  actual_fps={actual_fps:.2}");

    // Encode decoded frames to a video file.
    let mut encoder = match VideoEncoder::create(output)
        .video(width, height, actual_fps)
        .video_codec(VideoCodec::H264)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building video encoder: {e}");
            process::exit(1);
        }
    };

    let mut frames: u64 = 0;

    loop {
        let frame = match decoder.decode_one() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("Decode error at frame {frames}: {e}");
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
    let kb = std::fs::metadata(output).map_or(0, |m| m.len()) as f64 / 1024.0;
    let size_str = if kb < 1024.0 {
        format!("{kb:.0} KB")
    } else {
        format!("{:.1} MB", kb / 1024.0)
    };

    println!("Done. {out_name}  {size_str}  {frames} frames assembled");
}

// ── Encode mode: video → image sequence ──────────────────────────────────────

fn encode_sequence(input: &str, pattern: &str) {
    let in_name = Path::new(input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(input);

    println!("Encode: {in_name} → image sequence pattern='{pattern}'");
    println!();

    // Ensure the output directory exists.
    if let Some(parent) = Path::new(pattern)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| {
            eprintln!("Cannot create output directory '{}': {e}", parent.display());
            process::exit(1);
        });
    }

    // Probe the source to get dimensions and frame rate.
    let probe = match VideoDecoder::open(input).build() {
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

    println!("Source: {width}×{height}  {fps:.2} fps");

    // VideoEncoder::create() detects `%` in the output path and uses the
    // `image2` muxer.  The codec is selected automatically from the extension:
    //   .png  → VideoCodec::Png
    //   .jpg  → VideoCodec::Mjpeg
    //
    // Explicitly setting the codec avoids ambiguity.
    let ext = Path::new(pattern)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let codec = match ext.as_str() {
        "png" => VideoCodec::Png,
        "jpg" | "jpeg" => VideoCodec::Mjpeg,
        other => {
            eprintln!("Unsupported image extension '.{other}' (try .png or .jpg)");
            process::exit(1);
        }
    };

    let mut encoder = match VideoEncoder::create(pattern)
        .video(width, height, fps)
        .video_codec(codec)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error building image encoder: {e}");
            process::exit(1);
        }
    };

    let mut decoder = match VideoDecoder::open(input).build() {
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
                eprintln!("Decode error at frame {frames}: {e}");
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

    println!("Done. {frames} frames written to '{pattern}'");
    println!(
        "Files: {}  through  {}",
        pattern.replace("%04d", &format!("{:04}", 1)),
        pattern.replace("%04d", &format!("{:04}", frames))
    );
}
