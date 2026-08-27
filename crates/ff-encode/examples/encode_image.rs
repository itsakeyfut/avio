//! Read one frame from a video at a given timestamp and save it as a still image.
//!
//! Demonstrates all three `SeekMode` variants:
//! - `keyframe` (default) — seeks to the nearest keyframe before the target (fast)
//! - `exact`              — decodes from the previous keyframe to hit the exact PTS (slow)
//! - `backward`           — seeks backward to the nearest keyframe
//!
//! # Usage
//!
//! ```bash
//! cargo run --example encode_image -- \
//!   --input     video.mp4   \
//!   --output    frame.jpg   \
//!   [--time     00:01:23]   \
//!   [--quality  85]         \
//!   [--seek-mode keyframe|exact|backward]
//! ```

use std::{path::Path, process, time::Duration};

use ff_decode::{SeekMode, VideoDecoder};
use ff_encode::ImageEncoder;

fn parse_time(s: &str) -> Result<f64, String> {
    if s.contains(':') {
        let parts: Vec<&str> = s.splitn(3, ':').collect();
        if parts.len() == 3 {
            let h: f64 = parts[0]
                .parse()
                .map_err(|_| format!("invalid hours in '{s}'"))?;
            let m: f64 = parts[1]
                .parse()
                .map_err(|_| format!("invalid minutes in '{s}'"))?;
            let sec: f64 = parts[2]
                .parse()
                .map_err(|_| format!("invalid seconds in '{s}'"))?;
            Ok(h * 3600.0 + m * 60.0 + sec)
        } else {
            Err(format!("invalid time '{s}'"))
        }
    } else {
        s.parse::<f64>().map_err(|_| format!("invalid time '{s}'"))
    }
}

fn format_time(secs: f64) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut time_str = None::<String>;
    let mut quality: u32 = 85;
    let mut seek_mode = SeekMode::Keyframe;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--time" | "-t" => time_str = Some(args.next().unwrap_or_default()),
            "--quality" => {
                let v = args.next().unwrap_or_default();
                quality = v.parse().unwrap_or(85);
            }
            "--seek-mode" => {
                let v = args.next().unwrap_or_default();
                seek_mode = match v.as_str() {
                    "keyframe" => SeekMode::Keyframe,
                    "exact" => SeekMode::Exact,
                    "backward" => SeekMode::Backward,
                    other => {
                        eprintln!("Unknown seek mode '{other}' (try keyframe, exact, backward)");
                        process::exit(1);
                    }
                };
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: encode_image --input <video> --output <frame.jpg> \
             [--time HH:MM:SS] [--quality N] [--seek-mode keyframe|exact|backward]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });

    // Determine output format from extension
    let ext = Path::new(&output)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    let fmt_label = match ext.as_str() {
        "jpg" | "jpeg" => "JPEG",
        "png" => "PNG",
        "bmp" => "BMP",
        _ => {
            eprintln!("Error: unsupported output format '.{ext}' (try .jpg, .png, .bmp)");
            process::exit(1);
        }
    };

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    // Open decoder
    let mut dec = match VideoDecoder::open(&input).build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    let src_w = dec.width();
    let src_h = dec.height();
    println!("Input:   {in_name}  ({src_w}×{src_h})");

    // Seek if a timestamp was requested
    if let Some(ref ts) = time_str {
        let secs = parse_time(ts).unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });
        let mode_label = match seek_mode {
            SeekMode::Keyframe => "keyframe",
            SeekMode::Exact => "exact",
            SeekMode::Backward => "backward",
        };
        println!("Seeking: {}  (mode={})", format_time(secs), mode_label);
        if let Err(e) = dec.seek(Duration::from_secs_f64(secs), seek_mode) {
            eprintln!("Error seeking: {e}");
            process::exit(1);
        }
    }

    // Decode one frame
    let frame = match dec.decode_one() {
        Ok(Some(f)) => f,
        Ok(None) => {
            eprintln!("Error: no frame decoded (end of stream or empty file)");
            process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    let pts_secs = frame.timestamp().as_secs_f64();
    let w = frame.width();
    let h = frame.height();
    let fmt = frame.format();
    println!("Frame:   pts={pts_secs:.3}s  {w}×{h}  {fmt}");
    println!("Output:  {out_name}  ({fmt_label}  quality={quality})");
    println!();

    // Encode to image
    let enc = match ImageEncoder::create(&output)
        .width(w)
        .height(h)
        .quality(quality)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    if let Err(e) = enc.encode(&frame) {
        eprintln!("Error: {e}");
        process::exit(1);
    }

    let size_str = match std::fs::metadata(&output) {
        Ok(m) => {
            #[allow(clippy::cast_precision_loss)]
            let kb = m.len() as f64 / 1024.0;
            if kb < 1024.0 {
                format!("{kb:.0} KB")
            } else {
                format!("{:.1} MB", kb / 1024.0)
            }
        }
        Err(_) => "(unknown size)".to_string(),
    };

    println!("Done. {out_name}  {size_str}");
}
