//! Apply spatial transform effects using `FilterGraphBuilder` + `Pipeline`.
//!
//! Available effects:
//!   `vignette`  — darken corners with a radial vignette
//!   `flip`      — flip horizontally (`hflip`), vertically (`vflip`), or both
//!   `scale`     — resize with a configurable resampling algorithm
//!   `pad`       — pad to a target resolution with a fill color
//!   `letterbox` — scale-and-pad to fit a target aspect ratio (letterbox / pillarbox)
//!
//! # Usage
//!
//! ```bash
//! cargo run --example video_geometry -- \
//!   --input   input.mp4  \
//!   --output  out.mp4    \
//!   --effect  vignette   \
//!   [--angle  0.628]               # vignette radius angle 0.0–π/2 (default: π/5 ≈ 0.628)
//!   [--flip-axis  h|v|both]        # flip: h=horizontal, v=vertical, both (default: h)
//!   [--width  1280 --height  720]  # scale/pad/letterbox target dimensions
//!   [--algorithm  fast|bilinear|bicubic|lanczos]  # scale algorithm (default: fast)
//!   [--pad-color  black]           # pad fill color (default: black)
//! ```

use std::{
    io::{self, Write as _},
    path::Path,
    process,
};

use ff_filter::{FilterGraphBuilder, ScaleAlgorithm};
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
    let mut angle: f32 = std::f32::consts::PI / 5.0;
    let mut flip_axis = "h".to_owned();
    let mut width: u32 = 1280;
    let mut height: u32 = 720;
    let mut algorithm = "fast".to_owned();
    let mut pad_color = "black".to_owned();

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--effect" | "-e" => effect = Some(args.next().unwrap_or_default()),
            "--angle" => angle = args.next().unwrap_or_default().parse().unwrap_or(0.628),
            "--flip-axis" => flip_axis = args.next().unwrap_or_default(),
            "--width" => width = args.next().unwrap_or_default().parse().unwrap_or(1280),
            "--height" => height = args.next().unwrap_or_default().parse().unwrap_or(720),
            "--algorithm" => algorithm = args.next().unwrap_or_default(),
            "--pad-color" => pad_color = args.next().unwrap_or_default(),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: video_geometry --input <file> --output <file> \
             --effect vignette|flip|scale|pad|letterbox [options]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let effect = effect.unwrap_or_else(|| {
        eprintln!("--effect is required (vignette|flip|scale|pad|letterbox)");
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

    let alg = match algorithm.as_str() {
        "bilinear" => ScaleAlgorithm::Bilinear,
        "bicubic" => ScaleAlgorithm::Bicubic,
        "lanczos" => ScaleAlgorithm::Lanczos,
        _ => ScaleAlgorithm::Fast,
    };

    // Build filter graph

    let filter_result = match effect.as_str() {
        "vignette" => {
            println!("Input:   {in_name}");
            println!("Effect:  vignette  (angle={angle:.3} rad)");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().vignette(angle, 0.0, 0.0).build()
        }
        "flip" => {
            println!("Input:   {in_name}");
            println!("Effect:  flip  (axis={flip_axis})");
            println!("Output:  {out_name}");
            match flip_axis.as_str() {
                "v" => FilterGraphBuilder::new().vflip().build(),
                "both" => FilterGraphBuilder::new().hflip().vflip().build(),
                _ => FilterGraphBuilder::new().hflip().build(),
            }
        }
        "scale" => {
            println!("Input:   {in_name}");
            println!("Effect:  scale  ({width}×{height}  algorithm={algorithm})");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new().scale(width, height, alg).build()
        }
        "pad" => {
            println!("Input:   {in_name}");
            println!("Effect:  pad  ({width}×{height}  color={pad_color})");
            println!("Output:  {out_name}");
            // Center the source on the padded canvas.
            FilterGraphBuilder::new()
                .pad(width, height, -1, -1, &pad_color)
                .build()
        }
        "letterbox" => {
            println!("Input:   {in_name}");
            println!("Effect:  letterbox  ({width}×{height}  color={pad_color})");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .fit_to_aspect(width, height, &pad_color)
                .build()
        }
        other => {
            eprintln!("Unknown effect '{other}' (try vignette, flip, scale, pad, letterbox)");
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

    // Assemble pipeline

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
