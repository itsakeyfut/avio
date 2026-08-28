//! Apply visual effects to a video using `FilterGraphBuilder` + `Pipeline`.
//!
//! Available effects:
//!   `fade-in`  — fade from black at the start of the video
//!   `fade-out` — fade to black at the end of the video
//!   `rotate`   — rotate by a fixed angle (degrees, clockwise)
//!   `crop`     — crop to a sub-region
//!
//! # Usage
//!
//! ```bash
//! cargo run --example video_effects -- \
//!   --input   input.mp4  \
//!   --output  effect.mp4 \
//!   --effect  fade-in    \
//!   [--duration 2.0]      # fade duration in seconds (default: 2.0)
//!   [--angle   90.0]      # rotation angle in degrees (default: 90.0)
//!   [--x 0 --y 0 --width 1280 --height 720]  # crop region
//! ```

use std::{
    io::{self, Write as _},
    path::Path,
    process,
};

use ff_filter::FilterGraphBuilder;
use ff_format::{AudioCodec, VideoCodec};
use ff_pipeline::{EncoderConfig, Pipeline, Progress};

fn render_progress(p: &Progress) {
    match p.percent() {
        Some(pct) => {
            let bar_width = 20usize;
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
            let filled = filled.min(bar_width);
            let bar = "=".repeat(filled) + &" ".repeat(bar_width - filled);
            print!("\r{pct:5.1}%  [{bar}]    ");
        }
        None => {
            print!("\r{} frames    ", p.frames_processed);
        }
    }
    let _ = io::stdout().flush();
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None::<String>;
    let mut output = None::<String>;
    let mut effect = None::<String>;
    let mut duration_secs: f64 = 2.0;
    let mut angle: f64 = 90.0;
    let mut crop_x: u32 = 0;
    let mut crop_y: u32 = 0;
    let mut crop_w = None::<u32>;
    let mut crop_h = None::<u32>;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--effect" | "-e" => effect = Some(args.next().unwrap_or_default()),
            "--duration" => {
                let v = args.next().unwrap_or_default();
                duration_secs = v.parse().unwrap_or(2.0);
            }
            "--angle" => {
                let v = args.next().unwrap_or_default();
                angle = v.parse().unwrap_or(90.0);
            }
            "--x" => {
                let v = args.next().unwrap_or_default();
                crop_x = v.parse().unwrap_or(0);
            }
            "--y" => {
                let v = args.next().unwrap_or_default();
                crop_y = v.parse().unwrap_or(0);
            }
            "--width" => {
                let v = args.next().unwrap_or_default();
                crop_w = v.parse().ok();
            }
            "--height" => {
                let v = args.next().unwrap_or_default();
                crop_h = v.parse().ok();
            }
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: video_effects --input <file> --output <file> \
             --effect fade-in|fade-out|rotate|crop [options]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let effect = effect.unwrap_or_else(|| {
        eprintln!("--effect is required (fade-in|fade-out|rotate|crop)");
        process::exit(1);
    });

    let in_name = Path::new(&input)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&input);
    let out_name = Path::new(&output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&output);

    // ── Build filter graph ────────────────────────────────────────────────────

    let filter_result = match effect.as_str() {
        "fade-in" => {
            println!("Input:   {in_name}");
            println!("Effect:  fade-in  (duration={duration_secs} s)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .fade_in(0.0, duration_secs)
                .build()
        }
        "fade-out" => {
            println!("Input:   {in_name}");
            println!("Effect:  fade-out  (duration={duration_secs} s)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .fade_out(0.0, duration_secs)
                .build()
        }
        "rotate" => {
            println!("Input:   {in_name}");
            println!("Effect:  rotate  (angle={angle}°)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().rotate(angle, "black").build()
        }
        "crop" => {
            let w = crop_w.unwrap_or_else(|| {
                eprintln!("--width is required for crop");
                process::exit(1);
            });
            let h = crop_h.unwrap_or_else(|| {
                eprintln!("--height is required for crop");
                process::exit(1);
            });
            println!("Input:   {in_name}");
            println!("Effect:  crop  (x={crop_x}, y={crop_y}, {w}×{h})");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().crop(crop_x, crop_y, w, h).build()
        }
        other => {
            eprintln!("Unknown effect '{other}' (try fade-in, fade-out, rotate, crop)");
            process::exit(1);
        }
    };

    let filter = match filter_result {
        Ok(fg) => fg,
        Err(e) => {
            eprintln!("Error building filter graph: {e}");
            process::exit(1);
        }
    };

    println!();

    // ── Assemble pipeline ─────────────────────────────────────────────────────

    let config = EncoderConfig::builder()
        .video_codec(VideoCodec::H264)
        .audio_codec(AudioCodec::Aac)
        .crf(23)
        .build();

    let pipeline = match Pipeline::builder()
        .input(&input)
        .filter(filter)
        .output(&output, config)
        .on_progress(|p: &Progress| {
            render_progress(p);
            true
        })
        .build()
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    if let Err(e) = pipeline.run() {
        println!();
        eprintln!("Error: {e}");
        process::exit(1);
    }

    println!();

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
