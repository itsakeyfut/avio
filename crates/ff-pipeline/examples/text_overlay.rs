//! Burn text overlays into video using `FilterGraphBuilder` + `Pipeline`.
//!
//! Available effects:
//!   `drawtext` — static text at a fixed position with optional background box
//!   `box`      — static text with a semi-transparent background box
//!   `ticker`   — scrolling news-ticker text from right to left
//!
//! # Usage
//!
//! ```bash
//! cargo run --example text_overlay --features pipeline -- \
//!   --input   input.mp4   \
//!   --output  out.mp4     \
//!   --effect  drawtext    \
//!   [--text       "Hello World"]   # text content (default: "Hello World")
//!   [--x          10]              # X position expression (default: "10")
//!   [--y          10]              # Y position expression (default: "10")
//!   [--font-size  36]              # font size in points (default: 36)
//!   [--font-color white]           # font color (default: white)
//!   [--speed      200.0]           # ticker: pixels per second (default: 200.0)
//! ```

use std::{
    io::{self, Write as _},
    path::Path,
    process,
};

use ff_filter::{DrawTextOptions, FilterGraphBuilder};
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
    let mut text = "Hello World".to_owned();
    let mut x = "10".to_owned();
    let mut y = "10".to_owned();
    let mut font_size: u32 = 36;
    let mut font_color = "white".to_owned();
    let mut speed: f32 = 200.0;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--input" | "-i" => input = Some(args.next().unwrap_or_default()),
            "--output" | "-o" => output = Some(args.next().unwrap_or_default()),
            "--effect" | "-e" => effect = Some(args.next().unwrap_or_default()),
            "--text" => text = args.next().unwrap_or_default(),
            "--x" => x = args.next().unwrap_or_default(),
            "--y" => y = args.next().unwrap_or_default(),
            "--font-size" => {
                font_size = args.next().unwrap_or_default().parse().unwrap_or(36);
            }
            "--font-color" => font_color = args.next().unwrap_or_default(),
            "--speed" => speed = args.next().unwrap_or_default().parse().unwrap_or(200.0),
            other => {
                eprintln!("Unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let input = input.unwrap_or_else(|| {
        eprintln!(
            "Usage: text_overlay --input <file> --output <file> \
             --effect drawtext|box|ticker [options]"
        );
        process::exit(1);
    });
    let output = output.unwrap_or_else(|| {
        eprintln!("--output is required");
        process::exit(1);
    });
    let effect = effect.unwrap_or_else(|| {
        eprintln!("--effect is required (drawtext|box|ticker)");
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
        "drawtext" => {
            println!("Input:   {in_name}");
            println!("Effect:  drawtext  (text={text:?}  x={x}  y={y}  size={font_size})");
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .drawtext(DrawTextOptions {
                    text: text.clone(),
                    x: x.clone(),
                    y: y.clone(),
                    font_size,
                    font_color: font_color.clone(),
                    font_file: None,
                    opacity: 1.0,
                    box_color: None,
                    box_border_width: 0,
                })
                .build()
        }
        "box" => {
            println!("Input:   {in_name}");
            println!(
                "Effect:  drawtext+box  (text={text:?}  x={x}  y={y}  \
                 size={font_size}  box=black@0.5)"
            );
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .drawtext(DrawTextOptions {
                    text: text.clone(),
                    x: x.clone(),
                    y: y.clone(),
                    font_size,
                    font_color: font_color.clone(),
                    font_file: None,
                    opacity: 1.0,
                    box_color: Some("black@0.5".to_owned()),
                    box_border_width: 6,
                })
                .build()
        }
        "ticker" => {
            println!("Input:   {in_name}");
            println!(
                "Effect:  ticker  (text={text:?}  y=h-50  speed={speed:.0} px/s  size={font_size})"
            );
            println!("Output:  {out_name}");
            FilterGraphBuilder::new()
                .ticker(&text, "h-50", speed, font_size, &font_color)
                .build()
        }
        other => {
            eprintln!("Unknown effect '{other}' (try drawtext, box, ticker)");
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
